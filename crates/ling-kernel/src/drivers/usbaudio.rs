//! USB Audio Class playback (x86_64): class-compliant USB sound cards and
//! audio interfaces -- the PreSonus Studio 26c (UAC2, high speed, asynchronous
//! with an explicit feedback endpoint), headsets and dongles (UAC1), QEMU's
//! `usb-audio` (UAC1, full speed).
//!
//! **Flow.** `arch::xhci` enumerates the device, sets its configuration and
//! hands it here ([`attach`], from `xhci::poll`). We parse the descriptors
//! (`usbaudio_desc`), program the sampling frequency (UAC2: SET_CUR on the
//! clock source, after pointing a clock selector at it; UAC1: SET_CUR on the
//! endpoint, when it has that control), have the controller reserve the
//! isochronous bandwidth (`xhci::audio_open`, Configure Endpoint), then select
//! the streaming alternate setting (SET_INTERFACE).
//!
//! **Streaming.** The mixer's pump (once a frame) calls [`pump`]: it keeps
//! [`target_ms`] of audio queued on the OUT ring. Each packet carries the
//! frames of one service interval; how many comes from the device's feedback
//! endpoint when it has one (an asynchronous sink runs on its own crystal: a
//! fixed 6 frames per 125 µs would slowly over- or under-fill its buffer, and
//! it would click), otherwise from the nominal rate. The fraction is carried
//! from packet to packet (`Pacer`), so 44.1 kHz-style rates come out exact.
//! The mixer renders 48 kHz 16-bit stereo; [`ud::convert`] writes it as the
//! device's slots (24-bit MSB-justified in 4-byte subslots on the 26c) onto
//! channels 1/2, other channels silent.
//!
//! **Latency.** The pump runs once per desktop frame (16.7 ms at 60 Hz), so
//! the queue must outlast a frame and a slow one: 40 ms by default
//! (`audio latency <ms>` changes it, 12..100).
//!
//! **Output choice.** `mixer` plays through the USB device while it is
//! present (unless `audio out 0` chose the onboard codec); unplugging falls
//! back to HD Audio / AC'97.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::arch::timer::now_ms;
use crate::arch::xhci;
use crate::drivers::usbaudio_desc as ud;

/// The mixer's rate; the only one we ask the device for.
pub const RATE: u32 = 48_000;

struct State {
    name: String,
    vid: u16,
    pid: u16,
    plan: ud::StreamPlan,
    hs: bool,
    /// OUT packets per second and (micro)frames per packet.
    pps: u32,
    per_packet: u32,
    max_frames: u32,
    fb: ud::Feedback,
    fb_seen: u32,
    fb_pps: u32,
    pacer: ud::Pacer,
    frames_sent: u64,
    started_ms: u64,
    last_underruns: u32,
    last_missed: u32,
    last_report_ms: u64,
    /// The rate the device reported back (UAC2 GET_CUR), 0 if unknown.
    device_rate: u32,
    clock_valid: Option<bool>,
    rates_text: String,
    /// Adaptive queue length: grows after an underrun, decays back to
    /// [`target_ms`] after quiet stretches.
    adapt_ms: u32,
    adapt_at_ms: u64,
    was_active: bool,
    /// Pump cadence: intervals between pumps (<20, <40, <100, <250, >=250 ms).
    last_pump_us: u64,
    last_stall_log_ms: u64,
    gaps: [u32; 5],
    max_gap_us: u64,
}

static mut STATE: Option<State> = None;
static mut TARGET_MS: u32 = 40;
/// Why the last attach failed (shown by `audio`).
static mut LAST_ERROR: Option<String> = None;
static mut NAME_CACHE: String = String::new();

fn log(s: &str) {
    crate::console_write(s.as_bytes());
}

fn st() -> Option<&'static mut State> {
    unsafe { (*(&raw mut STATE)).as_mut() }
}

pub fn present() -> bool {
    st().is_some()
}

/// Device name for the mixer / Settings ("PreSonus Studio 26c").
pub fn name() -> &'static str {
    unsafe { (*(&raw const NAME_CACHE)).as_str() }
}

pub fn bits() -> u32 {
    st().map(|s| s.plan.bits as u32).unwrap_or(16)
}

pub fn target_ms() -> u32 {
    unsafe { TARGET_MS }
}

pub fn set_target_ms(ms: u32) {
    unsafe { TARGET_MS = ms.clamp(12, 100) };
}

fn fail(msg: String) {
    log(&format!("usbaudio: {msg}\n"));
    unsafe { LAST_ERROR = Some(msg) };
}

fn speed_name(speed: u32) -> &'static str {
    match speed {
        1 => "full speed",
        2 => "low speed",
        3 => "high speed",
        _ => "SuperSpeed",
    }
}

/// UAC2 class request to an entity of the AudioControl interface.
fn ac2(plan: &ud::StreamPlan, bm: u8, req: u8, cs: u8, entity: u8, data: &mut [u8]) -> Option<usize> {
    xhci::audio_control(bm, req, (cs as u16) << 8, ((entity as u16) << 8) | plan.ac_iface as u16, data)
}

/// Called by `xhci::poll` once a device with an AudioStreaming interface is
/// configured: pick the stream, set the rate, open the endpoints.
pub fn attach() {
    unsafe { LAST_ERROR = None };
    let Some(info) = xhci::audio_info() else { return };
    let hs = info.speed >= 3;
    log(&format!(
        "usbaudio: device {:04x}:{:04x} on {} port {} ({}), config descriptor {} bytes\n",
        info.vid,
        info.pid,
        core::str::from_utf8(info.controller).unwrap_or("xhci"),
        info.port,
        speed_name(info.speed),
        info.cfg.len()
    ));
    let plan = match ud::parse_config(&info.cfg) {
        Ok(p) => p,
        Err(e) => return fail(format!("{e} -- device left idle")),
    };
    let maker = xhci::audio_string(info.i_manufacturer);
    let product = xhci::audio_string(info.i_product);
    let name = match (maker, product) {
        (Some(m), Some(p)) if !p.starts_with(m.as_str()) => format!("{m} {p}"),
        (_, Some(p)) => p,
        (Some(m), None) => format!("{m} USB audio"),
        _ => format!("USB audio {:04x}:{:04x}", info.vid, info.pid),
    };
    let ep = plan.data_ep;
    log(&format!(
        "usbaudio: {name}: UAC{} stream on interface {} alt {} ({} playback alt setting(s)), AC interface {}\n",
        plan.uac, plan.iface, plan.alt, plan.candidates, plan.ac_iface
    ));
    log(&format!(
        "usbaudio:   format: {} ch, {}-byte subslots, {} bits; terminal {} -> {}\n",
        plan.channels,
        plan.subslot,
        plan.bits,
        plan.terminal,
        ud::terminal_name(plan.sink_type)
    ));
    log(&format!(
        "usbaudio:   data EP 0x{:02x}: iso OUT {}, wMaxPacketSize 0x{:04x} ({} B x{}), bInterval {}\n",
        ep.addr,
        ["no-sync", "asynchronous", "adaptive", "synchronous"][ep.sync_type() as usize],
        ep.max_packet,
        ep.mps(),
        ep.extra_tx() + 1,
        ep.interval
    ));
    match plan.fb_ep {
        Some(f) => log(&format!(
            "usbaudio:   feedback EP 0x{:02x}: iso IN, {} B, bInterval {}\n",
            f.addr,
            f.mps(),
            f.interval
        )),
        None if ep.sync_type() == ud::SYNC_ASYNC => log("usbaudio:   asynchronous without a feedback endpoint: nominal rate (may drift)\n"),
        None => {},
    }

    // ---- sampling frequency --------------------------------------------
    let mut device_rate = 0u32;
    let mut clock_valid = None;
    let mut rates_text = String::new();
    if plan.uac == 2 {
        if plan.clock_id == 0 {
            log("usbaudio:   no clock source found for the stream terminal; rate left as is\n");
        } else {
            log(&format!(
                "usbaudio:   clock source {} (controls 0x{:02x}){}\n",
                plan.clock_id,
                plan.clock_controls,
                match plan.clock_selector {
                    Some((s, p, _)) => format!(" via selector {s} pin {p}"),
                    None => String::new(),
                }
            ));
            if let Some((sel, pin, ctl)) = plan.clock_selector {
                if ctl & 3 == 3 {
                    let ok = ac2(&plan, 0x21, 0x01, 0x01, sel, &mut [pin]).is_some();
                    log(&format!("usbaudio:   clock selector {sel} -> pin {pin}: {}\n", if ok { "ok" } else { "refused" }));
                }
            }
            // GET_RANGE: supported rates
            let mut hdr = [0u8; 2];
            let mut ranges = Vec::new();
            if ac2(&plan, 0xA1, 0x02, 0x01, plan.clock_id, &mut hdr).is_some() {
                let n = (hdr[0] as usize | (hdr[1] as usize) << 8).min(32);
                let mut b = alloc::vec![0u8; 2 + 12 * n];
                if ac2(&plan, 0xA1, 0x02, 0x01, plan.clock_id, &mut b).is_some() {
                    ranges = ud::parse_ranges(&b);
                }
            }
            for &(lo, hi, res) in &ranges {
                if !rates_text.is_empty() {
                    rates_text.push(' ');
                }
                if lo == hi {
                    rates_text.push_str(&format!("{lo}"));
                } else {
                    rates_text.push_str(&format!("{lo}-{hi}/{res}"));
                }
            }
            log(&format!(
                "usbaudio:   rates: {}\n",
                if rates_text.is_empty() { "(GET_RANGE refused)" } else { rates_text.as_str() }
            ));
            if !ranges.is_empty() && !ud::range_has(&ranges, RATE) {
                return fail(format!("{name} cannot run at {RATE} Hz (rates: {rates_text}) -- left idle"));
            }
            if plan.clock_controls & 3 == 3 {
                let ok = ac2(&plan, 0x21, 0x01, 0x01, plan.clock_id, &mut RATE.to_le_bytes()).is_some();
                log(&format!("usbaudio:   SET_CUR sampling frequency {RATE}: {}\n", if ok { "ok" } else { "refused" }));
            }
            let mut cur = [0u8; 4];
            if ac2(&plan, 0xA1, 0x01, 0x01, plan.clock_id, &mut cur).is_some() {
                device_rate = u32::from_le_bytes(cur);
                log(&format!("usbaudio:   clock reports {device_rate} Hz\n"));
            }
            if plan.clock_controls & 0x0C != 0 {
                let mut v = [0u8; 1];
                if ac2(&plan, 0xA1, 0x01, 0x02, plan.clock_id, &mut v).is_some() {
                    clock_valid = Some(v[0] != 0);
                    log(&format!("usbaudio:   clock valid: {}\n", if v[0] != 0 { "yes" } else { "NO (no sync / external clock missing)" }));
                }
            }
            if device_rate != 0 && device_rate != RATE {
                return fail(format!("{name} stays at {device_rate} Hz, not {RATE} -- left idle"));
            }
        }
    } else {
        rates_text = if plan.rates.is_empty() {
            format!("{}-{}", plan.rate_min, plan.rate_max)
        } else {
            plan.rates.iter().map(|r| format!("{r}")).collect::<Vec<_>>().join(" ")
        };
        log(&format!("usbaudio:   rates: {rates_text}\n"));
        if !plan.rate_listed(RATE) {
            return fail(format!("{name} does not list {RATE} Hz (rates: {rates_text}) -- left idle"));
        }
    }

    // ---- packet geometry -------------------------------------------------
    let pps = ud::packets_per_sec(hs, ep.interval);
    let per_packet = (if hs { 8000 } else { 1000 }) / pps;
    let fbytes = plan.frame_bytes();
    let max_frames = ep.mps() * (if hs { ep.extra_tx() + 1 } else { 1 }) / fbytes.max(1);
    let nominal = RATE.div_ceil(pps);
    // a feedback-paced stream needs room for one frame more than nominal
    let need = if plan.fb_ep.is_some() { nominal + 1 } else { nominal };
    if need > max_frames {
        return fail(format!("packets too small: {need} frames per packet needed, the endpoint carries {max_frames} -- left idle"));
    }
    let slot = ((nominal + 2).min(max_frames) * fbytes).next_power_of_two();
    if let Err(cc) = xhci::audio_open(plan.data_ep, plan.fb_ep, slot) {
        return fail(format!("{name}: the controller would not open the isochronous endpoints (completion code {cc})"));
    }
    // ---- alt setting (after the bandwidth is reserved) ------------------
    if xhci::audio_control(0x01, 11, plan.alt as u16, plan.iface as u16, &mut []).is_none() {
        return fail(format!("{name}: SET_INTERFACE {} alt {} refused", plan.iface, plan.alt));
    }
    log(&format!("usbaudio:   SET_INTERFACE {} alt {}: ok\n", plan.iface, plan.alt));
    if plan.uac == 2 && plan.clock_id != 0 && plan.clock_controls & 3 == 3 {
        // Linux programs the clock after the alt setting; some devices reset
        // it on SET_INTERFACE, so say it again.
        let _ = ac2(&plan, 0x21, 0x01, 0x01, plan.clock_id, &mut RATE.to_le_bytes());
        let mut cur = [0u8; 4];
        if ac2(&plan, 0xA1, 0x01, 0x01, plan.clock_id, &mut cur).is_some() {
            device_rate = u32::from_le_bytes(cur);
            log(&format!("usbaudio:   clock after SET_INTERFACE: {device_rate} Hz\n"));
        }
    }
    if plan.uac == 1 && plan.ep_freq_control {
        let mut r = [RATE as u8, (RATE >> 8) as u8, (RATE >> 16) as u8];
        let ok = xhci::audio_control(0x22, 0x01, 0x0100, ep.addr as u16, &mut r).is_some();
        log(&format!("usbaudio:   SET_CUR endpoint sampling frequency {RATE}: {}\n", if ok { "ok" } else { "refused" }));
    }

    let fb_pps = plan.fb_ep.map(|f| ud::packets_per_sec(hs, f.interval)).unwrap_or(0);
    unsafe {
        NAME_CACHE = name.clone();
    }
    let summary = format!(
        "usbaudio: {name}, UAC{}, {RATE} Hz {}-bit, {}ch{}",
        plan.uac,
        plan.bits,
        plan.channels,
        if plan.channels > 2 { " (stereo on outputs 1/2)" } else { "" }
    );
    unsafe {
        STATE = Some(State {
            name,
            vid: info.vid,
            pid: info.pid,
            plan,
            hs,
            pps,
            per_packet,
            max_frames,
            fb: ud::Feedback::new(ud::nominal_q16(RATE, hs)),
            fb_seen: 0,
            fb_pps,
            pacer: ud::Pacer::new(),
            frames_sent: 0,
            started_ms: now_ms(),
            last_underruns: 0,
            last_missed: 0,
            last_report_ms: 0,
            device_rate,
            clock_valid,
            rates_text,
            adapt_ms: 0,
            adapt_at_ms: 0,
            was_active: false,
            last_pump_us: 0,
            last_stall_log_ms: 0,
            gaps: [0; 5],
            max_gap_us: 0,
        });
    }
    let active = crate::drivers::mixer::usb_attached();
    log(&format!("{summary} -> {}\n", if active { "active output" } else { "available (not selected)" }));
}

/// The device went away (unplug / controller reset): the mixer falls back.
pub fn detach() {
    let Some(s) = (unsafe { (*(&raw mut STATE)).take() }) else { return };
    log(&format!("usbaudio: {} unplugged after {} s of audio\n", s.name, s.frames_sent / RATE as u64));
    crate::drivers::mixer::usb_detached();
}

/// Feed the device. `render` is the mixer (stereo i16 at 48 kHz); `active`
/// false keeps only the feedback endpoint serviced and lets the OUT ring
/// drain (the device then plays silence).
pub fn pump(render: &mut dyn FnMut(&mut [i16]), active: bool) {
    // completion events -> ring progress (and hot-plug)
    xhci::service();
    let Some(s) = st() else { return };
    let now = now_ms();
    let now_us = crate::arch::timer::now_us();
    let mut gap_ms = 0u32;
    if s.last_pump_us != 0 {
        let g = now_us.saturating_sub(s.last_pump_us);
        gap_ms = (g / 1000).min(u32::MAX as u64) as u32;
        if gap_ms >= 150 && active && now.saturating_sub(s.last_stall_log_ms) > 2000 {
            // nothing pumped audio for this long: the desktop's main loop was
            // busy elsewhere (shows in `report` when sound cuts out)
            log(&format!("usbaudio: audio pump starved for {gap_ms} ms (main loop busy)\n"));
            s.last_stall_log_ms = now;
        }
        let b = match g {
            0..20_000 => 0,
            20_000..40_000 => 1,
            40_000..100_000 => 2,
            100_000..250_000 => 3,
            _ => 4,
        };
        s.gaps[b] = s.gaps[b].saturating_add(1);
        s.max_gap_us = s.max_gap_us.max(g);
    }
    s.last_pump_us = now_us;
    xhci::audio_watchdog(300);
    if let Some((raw, len, seq)) = xhci::audio_fb_service((s.fb_pps as usize * 64 / 1000).max(8)) {
        if seq != s.fb_seen {
            s.fb_seen = seq;
            s.fb.update(raw, len as usize);
        }
    }
    let Some((o, _)) = xhci::audio_stats() else { return };
    let base = target_ms();
    let cap_ms = ((o.capacity * 1000 / s.pps.max(1) as u64) as u32).saturating_sub(8).max(base);
    if o.underruns != s.last_underruns && active && s.was_active && s.frames_sent > 0 && gap_ms + 10 <= cap_ms {
        // The pump came too late for the queue, but a longer queue would
        // have covered the wait: queue that much from now on. (A stall
        // longer than the ring can hold is not worth the latency.)
        s.adapt_ms = s.adapt_ms.max(base).max(gap_ms + gap_ms / 4 + 5).min(cap_ms);
        s.adapt_at_ms = now;
    } else if s.adapt_ms > base && now.saturating_sub(s.adapt_at_ms) > 2_000 {
        s.adapt_ms = s.adapt_ms.saturating_sub(5).max(base);
        s.adapt_at_ms = now;
    }
    s.was_active = active;
    if o.underruns != s.last_underruns || o.missed != s.last_missed {
        if now.saturating_sub(s.last_report_ms) > 5000 && active && s.frames_sent > 0 {
            log(&format!(
                "usbaudio: stream gaps: {} underrun(s), {} missed interval(s) so far (pump late?)\n",
                o.underruns, o.missed
            ));
            s.last_report_ms = now;
        }
        s.last_underruns = o.underruns;
        s.last_missed = o.missed;
    }
    if !active {
        return;
    }
    let target = (s.adapt_ms.max(base) as u64 * s.pps as u64 / 1000).min(o.capacity);
    if o.queued >= target {
        return;
    }
    let n = (target - o.queued) as usize;
    // frames for each packet, from the feedback (or nominal) rate
    let rate = s.fb.value;
    let mut counts: Vec<u32> = Vec::with_capacity(n);
    let mut total = 0usize;
    for _ in 0..n {
        let f = s.pacer.next(rate, s.per_packet, s.max_frames);
        counts.push(f);
        total += f as usize;
    }
    let mut pcm = alloc::vec![0i16; total * 2];
    let mut at = 0;
    while at < total {
        let k = (total - at).min(1024);
        render(&mut pcm[at * 2..(at + k) * 2]);
        at += k;
    }
    let (ch, sub) = (s.plan.channels as usize, s.plan.subslot as usize);
    let mut i = 0usize;
    let mut pos = 0usize;
    let queued = xhci::audio_out_push(n, &mut |slot: &mut [u8]| {
        let f = counts[i] as usize;
        i += 1;
        let len = ud::convert(&pcm[pos * 2..], f, ch, sub, slot);
        pos += f;
        len
    });
    s.frames_sent += pos as u64;
    let _ = queued;
}

/// Forget the pump-cadence histogram (`audio reset`).
pub fn reset_stats() {
    if let Some(s) = st() {
        s.gaps = [0; 5];
        s.max_gap_us = 0;
    }
}

fn fmt_rate_q16(q: u32, hs: bool) -> String {
    // frames per (micro)frame -> Hz with two decimals
    let per = if hs { 8000u64 } else { 1000 };
    let centi = (q as u64 * per * 100) >> 16;
    format!("{}.{:02}", centi / 100, centi % 100)
}

/// One-paragraph status for `audio`.
pub fn status() -> String {
    let Some(s) = st() else {
        return match unsafe { (*(&raw const LAST_ERROR)).as_ref() } {
            Some(e) => format!("  usb: {e}\n"),
            None => String::new(),
        };
    };
    let mut t = format!(
        "  usb: {} ({:04x}:{:04x}), UAC{}, {} Hz {}-bit in {}-byte slots, {} ch, {}\n",
        s.name,
        s.vid,
        s.pid,
        s.plan.uac,
        if s.device_rate != 0 { s.device_rate } else { RATE },
        s.plan.bits,
        s.plan.subslot,
        s.plan.channels,
        if s.hs { "high speed" } else { "full speed" }
    );
    if !s.rates_text.is_empty() {
        t.push_str(&format!("       rates {}{}\n", s.rates_text, match s.clock_valid {
            Some(true) => ", clock valid",
            Some(false) => ", CLOCK NOT VALID",
            None => "",
        }));
    }
    if let Some((o, fb)) = xhci::audio_stats() {
        t.push_str(&format!(
            "       stream: {} packets/s, queued {} ({} ms, target {} ms, now {} ms), sent {}, underruns {}, missed {}, errors {}{}, resyncs {}, ep state {}\n",
            s.pps,
            o.queued,
            o.queued * 1000 / s.pps.max(1) as u64,
            target_ms(),
            s.adapt_ms.max(target_ms()),
            o.sent,
            o.underruns,
            o.missed,
            o.errors,
            if o.errors > 0 { format!(" (last cc {})", o.last_err_cc) } else { String::new() },
            o.resyncs,
            o.ep_state
        ));
        if let Some(f) = fb {
            t.push_str(&format!(
                "       feedback: {} Hz (raw 0x{:08x}, {} accepted, {} rejected, shift {:?}), ring sent {} underruns {}\n",
                fmt_rate_q16(s.fb.value, s.hs),
                s.fb.last_raw,
                s.fb.accepted,
                s.fb.rejected,
                s.fb.shift(),
                f.sent,
                f.underruns
            ));
        }
    }
    t.push_str(&format!(
        "       pump intervals: <20ms {}, 20-40 {}, 40-100 {}, 100-250 {}, >250 {}; longest {} ms\n",
        s.gaps[0],
        s.gaps[1],
        s.gaps[2],
        s.gaps[3],
        s.gaps[4],
        s.max_gap_us / 1000
    ));
    let secs = now_ms().saturating_sub(s.started_ms) / 1000;
    t.push_str(&format!("       played {} s of audio in {} s attached\n", s.frames_sent / RATE as u64, secs));
    t
}
