//! USB Audio Class descriptor parsing and stream arithmetic -- the pure part
//! of `drivers::usbaudio`, kept free of `crate::` so it can be unit-tested on
//! the host (`rustc --test`, see the tests at the bottom; they include the
//! real PreSonus Studio 26c configuration descriptor, reconstructed byte for
//! byte from a published `lsusb -v` dump).
//!
//! Supports UAC1 (USB Audio 1.0 -- QEMU's `usb-audio`, cheap headsets) and
//! UAC2 (USB Audio 2.0 -- class-compliant interfaces such as the Studio 26c).
//! What it extracts is one *playback stream plan*: which AudioStreaming
//! interface / alternate setting to select, the sample format, the data
//! endpoint, the explicit feedback endpoint of an asynchronous sink, and (UAC2)
//! the clock source whose sampling frequency must be programmed.

use alloc::vec::Vec;

pub const CLASS_AUDIO: u8 = 1;
pub const SUB_CONTROL: u8 = 1;
pub const SUB_STREAMING: u8 = 2;
pub const PROTO_UAC2: u8 = 0x20;

const DT_INTERFACE: u8 = 4;
const DT_ENDPOINT: u8 = 5;
const DT_CS_INTERFACE: u8 = 0x24;
const DT_CS_ENDPOINT: u8 = 0x25;

// AudioControl descriptor subtypes (UAC1 and UAC2 share 1..3).
const AC_HEADER: u8 = 1;
const AC_INPUT_TERMINAL: u8 = 2;
const AC_OUTPUT_TERMINAL: u8 = 3;
const AC2_CLOCK_SOURCE: u8 = 0x0A;
const AC2_CLOCK_SELECTOR: u8 = 0x0B;
const AC2_CLOCK_MULTIPLIER: u8 = 0x0C;
// AudioStreaming subtypes.
const AS_GENERAL: u8 = 1;
const AS_FORMAT_TYPE: u8 = 2;

/// Isochronous endpoint synchronisation types (bmAttributes bits 3:2).
pub const SYNC_NONE: u8 = 0;
pub const SYNC_ASYNC: u8 = 1;
pub const SYNC_ADAPTIVE: u8 = 2;
pub const SYNC_SYNC: u8 = 3;

/// One isochronous endpoint as its descriptor states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IsoEndpoint {
    pub addr: u8,
    /// bmAttributes (transfer type 1:0, sync 3:2, usage 5:4).
    pub attrs: u8,
    /// wMaxPacketSize raw: bits 10:0 bytes, 12:11 extra transactions (HS).
    pub max_packet: u16,
    pub interval: u8,
    /// UAC1 only: bRefresh / bSynchAddress (9-byte audio endpoint descriptor).
    pub refresh: u8,
    pub synch_addr: u8,
}

impl IsoEndpoint {
    pub fn sync_type(&self) -> u8 {
        (self.attrs >> 2) & 3
    }
    pub fn usage(&self) -> u8 {
        (self.attrs >> 4) & 3
    }
    pub fn is_in(&self) -> bool {
        self.addr & 0x80 != 0
    }
    /// Bytes per transaction.
    pub fn mps(&self) -> u32 {
        (self.max_packet & 0x7FF) as u32
    }
    /// Transactions per (micro)frame - 1 (HS high-bandwidth).
    pub fn extra_tx(&self) -> u32 {
        ((self.max_packet >> 11) & 3) as u32
    }
}

/// The playback stream LingOS will run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamPlan {
    /// 1 or 2 (USB Audio Class version).
    pub uac: u8,
    /// The AudioControl interface (UAC2 clock requests are addressed to it).
    pub ac_iface: u8,
    pub iface: u8,
    pub alt: u8,
    pub channels: u8,
    /// Bytes per sample slot (2, 3 or 4).
    pub subslot: u8,
    /// Valid bits within the slot (MSB-justified).
    pub bits: u8,
    pub data_ep: IsoEndpoint,
    /// Explicit feedback endpoint (asynchronous sinks).
    pub fb_ep: Option<IsoEndpoint>,
    /// UAC1 only: the endpoint has a Sampling Frequency control.
    pub ep_freq_control: bool,
    /// UAC1 only: discrete rates (empty = continuous `rate_min..rate_max`).
    pub rates: Vec<u32>,
    pub rate_min: u32,
    pub rate_max: u32,
    /// UAC2: the clock source feeding the stream's terminal, its bmControls,
    /// and the selector (id, input pin) on the way, if any.
    pub clock_id: u8,
    pub clock_controls: u8,
    pub clock_selector: Option<(u8, u8, u8)>,
    /// The USB-streaming input terminal the stream is linked to.
    pub terminal: u8,
    /// wTerminalType of the output terminal the audio ends in (0x0301
    /// speaker, 0x0302 headphones, 0x0603 line ...), 0 if not found.
    pub sink_type: u16,
    /// Number of playback alternate settings considered.
    pub candidates: u8,
}

impl StreamPlan {
    pub fn frame_bytes(&self) -> u32 {
        self.channels as u32 * self.subslot as u32
    }
    /// Can the stream run at `rate` according to the descriptors? (UAC2
    /// rates live in the clock and are asked for with GET_RANGE.)
    pub fn rate_listed(&self, rate: u32) -> bool {
        if self.uac == 2 {
            return true;
        }
        if self.rates.is_empty() {
            return rate >= self.rate_min && rate <= self.rate_max;
        }
        self.rates.contains(&rate)
    }
}

#[derive(Clone, Copy, Debug)]
struct Entity {
    ac_iface: u8,
    uac2: bool,
    subtype: u8,
    id: u8,
    term_type: u16,
    /// IT/OT: bCSourceID (UAC2); clock multiplier: its source.
    clock: u8,
    /// OT / units: bSourceID.
    source: u8,
    /// Clock source: bmControls; selector: bmControls.
    controls: u8,
    attrs: u8,
    /// Clock selector input pins (up to 8).
    pins: [u8; 8],
    n_pins: u8,
}

#[derive(Clone, Debug, Default)]
struct AltInfo {
    iface: u8,
    alt: u8,
    uac2: bool,
    terminal: u8,
    format_type: u8,
    pcm: bool,
    channels: u8,
    subslot: u8,
    bits: u8,
    rates: Vec<u32>,
    rate_min: u32,
    rate_max: u32,
    eps: Vec<IsoEndpoint>,
    /// CS endpoint bmAttributes of the data endpoint (UAC1 bit0 = freq control).
    ep_cs_attrs: u8,
}

fn rd16(d: &[u8], o: usize) -> u16 {
    d[o] as u16 | (d[o + 1] as u16) << 8
}
fn rd24(d: &[u8], o: usize) -> u32 {
    d[o] as u32 | (d[o + 1] as u32) << 8 | (d[o + 2] as u32) << 16
}

/// Walk a full configuration descriptor and pick the best playback stream.
/// `Err` says why nothing usable was found (shown in the boot log).
pub fn parse_config(cfg: &[u8]) -> Result<StreamPlan, &'static str> {
    let mut ents: Vec<Entity> = Vec::new();
    let mut alts: Vec<AltInfo> = Vec::new();
    let mut ac_ifaces: Vec<(u8, bool)> = Vec::new();
    // Current interface: (number, alt, class, subclass, protocol).
    let mut cur: Option<(u8, u8, u8, u8, u8)> = None;
    let mut off = 0usize;
    let mut saw_audio = false;
    while off + 2 <= cfg.len() {
        let len = cfg[off] as usize;
        if len < 2 || off + len > cfg.len() {
            break;
        }
        let d = &cfg[off..off + len];
        match d[1] {
            DT_INTERFACE if len >= 9 => {
                cur = Some((d[2], d[3], d[5], d[6], d[7]));
                if d[5] == CLASS_AUDIO {
                    saw_audio = true;
                }
                if d[5] == CLASS_AUDIO && d[6] == SUB_CONTROL {
                    ac_ifaces.push((d[2], d[7] == PROTO_UAC2));
                }
                if d[5] == CLASS_AUDIO && d[6] == SUB_STREAMING && d[4] > 0 {
                    alts.push(AltInfo { iface: d[2], alt: d[3], uac2: d[7] == PROTO_UAC2, ..Default::default() });
                }
            },
            DT_CS_INTERFACE if len >= 3 => {
                let Some((num, alt, class, sub, proto)) = cur else {
                    off += len;
                    continue;
                };
                if class != CLASS_AUDIO {
                    off += len;
                    continue;
                }
                let uac2 = proto == PROTO_UAC2;
                if sub == SUB_CONTROL {
                    if let Some(e) = parse_entity(num, uac2, d) {
                        ents.push(e);
                    }
                } else if sub == SUB_STREAMING {
                    if let Some(a) = alts.last_mut().filter(|a| a.iface == num && a.alt == alt) {
                        parse_as(a, d);
                    }
                }
            },
            DT_ENDPOINT if len >= 7 => {
                if let Some((num, alt, CLASS_AUDIO, SUB_STREAMING, _)) = cur {
                    if let Some(a) = alts.last_mut().filter(|a| a.iface == num && a.alt == alt) {
                        a.eps.push(IsoEndpoint {
                            addr: d[2],
                            attrs: d[3],
                            max_packet: rd16(d, 4),
                            interval: d[6],
                            refresh: if len >= 9 { d[7] } else { 0 },
                            synch_addr: if len >= 9 { d[8] } else { 0 },
                        });
                    }
                }
            },
            DT_CS_ENDPOINT if len >= 4 => {
                if let Some((num, alt, CLASS_AUDIO, SUB_STREAMING, _)) = cur {
                    if let Some(a) = alts.last_mut().filter(|a| a.iface == num && a.alt == alt) {
                        // belongs to the endpoint just read; only the data one matters
                        if a.eps.len() == 1 && d[2] == 1 {
                            a.ep_cs_attrs = d[3];
                        }
                    }
                }
            },
            _ => {},
        }
        off += len;
    }
    if !saw_audio {
        return Err("no audio interface");
    }

    let mut best: Option<(i32, StreamPlan)> = None;
    let mut candidates = 0u8;
    for a in &alts {
        // The data endpoint: isochronous OUT, usage "data" (or implicit-feedback data).
        let Some(data) = a.eps.iter().find(|e| e.attrs & 3 == 1 && !e.is_in() && e.usage() != 1).copied() else {
            continue;
        };
        if a.format_type != 1 || !a.pcm || !(1..=4).contains(&a.subslot) || a.channels == 0 {
            continue;
        }
        candidates = candidates.saturating_add(1);
        // Explicit feedback: UAC1 names it with bSynchAddress, UAC2 marks its usage.
        let fb = a
            .eps
            .iter()
            .find(|e| e.attrs & 3 == 1 && e.is_in() && ((data.synch_addr != 0 && e.addr == data.synch_addr) || e.usage() == 1))
            .copied();
        let ac_iface = ac_ifaces.iter().find(|x| x.1 == a.uac2).or(ac_ifaces.first()).map(|x| x.0).unwrap_or(0);
        let mut plan = StreamPlan {
            uac: if a.uac2 { 2 } else { 1 },
            ac_iface,
            iface: a.iface,
            alt: a.alt,
            channels: a.channels,
            subslot: a.subslot,
            bits: if a.bits == 0 { a.subslot * 8 } else { a.bits },
            data_ep: data,
            fb_ep: if data.sync_type() == SYNC_ASYNC { fb } else { None },
            ep_freq_control: !a.uac2 && a.ep_cs_attrs & 1 != 0,
            rates: a.rates.clone(),
            rate_min: a.rate_min,
            rate_max: a.rate_max,
            clock_id: 0,
            clock_controls: 0,
            clock_selector: None,
            terminal: a.terminal,
            sink_type: 0,
            candidates: 0,
        };
        if a.uac2 {
            resolve_clock(&ents, ac_iface, a.terminal, &mut plan);
        }
        plan.sink_type = sink_type(&ents, ac_iface, a.terminal);
        // Score: 48 kHz possible, then stereo, then resolution.
        let mut score = 0i32;
        if plan.rate_listed(48_000) {
            score += 10_000;
        }
        score += match plan.channels {
            2 => 1000,
            c if c > 2 => 800,
            _ => 300,
        };
        score += plan.bits as i32;
        if plan.data_ep.sync_type() == SYNC_ASYNC && plan.fb_ep.is_none() {
            score -= 500; // implicit feedback: works, but drifts without the capture side
        }
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, plan));
        }
    }
    let Some((_, mut plan)) = best else {
        return Err("audio device without a PCM playback stream");
    };
    plan.candidates = candidates;
    Ok(plan)
}

fn parse_entity(ac_iface: u8, uac2: bool, d: &[u8]) -> Option<Entity> {
    let len = d.len();
    let mut e = Entity { ac_iface, uac2, subtype: d[2], id: 0, term_type: 0, clock: 0, source: 0, controls: 0, attrs: 0, pins: [0; 8], n_pins: 0 };
    match d[2] {
        AC_HEADER => return None,
        AC_INPUT_TERMINAL if len >= 8 => {
            e.id = d[3];
            e.term_type = rd16(d, 4);
            if uac2 && len >= 8 {
                e.clock = d[7];
            }
        },
        AC_OUTPUT_TERMINAL if len >= 8 => {
            e.id = d[3];
            e.term_type = rd16(d, 4);
            e.source = d[7];
            if uac2 && len >= 9 {
                e.clock = d[8];
            }
        },
        AC2_CLOCK_SOURCE if uac2 && len >= 6 => {
            e.id = d[3];
            e.attrs = d[4];
            e.controls = d[5];
        },
        AC2_CLOCK_SELECTOR if uac2 && len >= 6 => {
            e.id = d[3];
            let n = (d[4] as usize).min(8).min(len.saturating_sub(6));
            e.n_pins = n as u8;
            e.pins[..n].copy_from_slice(&d[5..5 + n]);
            e.controls = d.get(5 + d[4] as usize).copied().unwrap_or(0);
        },
        AC2_CLOCK_MULTIPLIER if uac2 && len >= 5 => {
            e.id = d[3];
            e.clock = d[4];
        },
        // Mixer / selector / feature / processing units: id + first source
        // (enough to walk a chain to its output terminal).
        4..=8 if len >= 5 => {
            e.id = d[3];
            e.source = if d[2] == 4 || d[2] == 5 { d.get(5).copied().unwrap_or(0) } else { d[4] };
            if d[2] == 7 || d[2] == 8 {
                // processing / extension unit: wProcessType/wExtensionCode, then bNrInPins, baSourceID
                e.source = d.get(7).copied().unwrap_or(0);
            }
        },
        _ => return None,
    }
    Some(e)
}

fn parse_as(a: &mut AltInfo, d: &[u8]) {
    let len = d.len();
    match d[2] {
        AS_GENERAL => {
            if a.uac2 && len >= 16 {
                a.terminal = d[3];
                a.format_type = d[5];
                let fmts = u32::from_le_bytes([d[6], d[7], d[8], d[9]]);
                a.pcm = fmts & 1 != 0;
                a.channels = d[10];
            } else if !a.uac2 && len >= 7 {
                a.terminal = d[3];
                a.pcm = rd16(d, 5) == 1; // wFormatTag PCM
            }
        },
        AS_FORMAT_TYPE => {
            if a.uac2 && len >= 6 {
                if a.format_type == 0 {
                    a.format_type = d[3];
                }
                if d[3] != 1 {
                    a.format_type = d[3];
                }
                a.subslot = d[4];
                a.bits = d[5];
            } else if !a.uac2 && len >= 8 {
                a.format_type = d[3];
                a.channels = d[4];
                a.subslot = d[5];
                a.bits = d[6];
                let n = d[7] as usize;
                if n == 0 && len >= 14 {
                    a.rate_min = rd24(d, 8);
                    a.rate_max = rd24(d, 11);
                } else {
                    for i in 0..n {
                        if 8 + i * 3 + 3 <= len {
                            a.rates.push(rd24(d, 8 + i * 3));
                        }
                    }
                }
            }
        },
        _ => {},
    }
}

fn find(ents: &[Entity], ac: u8, id: u8) -> Option<&Entity> {
    ents.iter().find(|e| e.id == id && e.ac_iface == ac).or_else(|| ents.iter().find(|e| e.id == id && e.uac2))
}

/// UAC2: follow terminal -> (selector / multiplier)* -> clock source.
fn resolve_clock(ents: &[Entity], ac: u8, terminal: u8, plan: &mut StreamPlan) {
    let Some(t) = find(ents, ac, terminal) else { return };
    let mut id = t.clock;
    for _ in 0..8 {
        let Some(e) = find(ents, ac, id) else { return };
        match e.subtype {
            AC2_CLOCK_SOURCE => {
                plan.clock_id = e.id;
                plan.clock_controls = e.controls;
                return;
            },
            AC2_CLOCK_SELECTOR => {
                if e.n_pins == 0 {
                    return;
                }
                plan.clock_selector = Some((e.id, 1, e.controls));
                id = e.pins[0];
            },
            AC2_CLOCK_MULTIPLIER => id = e.clock,
            _ => return,
        }
    }
}

/// The type of the output terminal the stream's audio reaches.
fn sink_type(ents: &[Entity], ac: u8, terminal: u8) -> u16 {
    // Output terminals whose source chain contains `terminal`.
    for ot in ents.iter().filter(|e| e.subtype == AC_OUTPUT_TERMINAL && e.ac_iface == ac) {
        let mut id = ot.source;
        for _ in 0..16 {
            if id == terminal {
                return ot.term_type;
            }
            match find(ents, ac, id) {
                Some(e) if e.subtype != AC_INPUT_TERMINAL && e.source != 0 => id = e.source,
                _ => break,
            }
        }
    }
    0
}

pub fn terminal_name(t: u16) -> &'static str {
    match t {
        0x0301 => "speaker",
        0x0302 => "headphones",
        0x0304 => "desktop speaker",
        0x0305 => "room speaker",
        0x0402 | 0x0403 => "headset",
        0x0601 => "analog out",
        0x0602 => "digital out",
        0x0603 => "line out",
        0x0605 => "S/PDIF out",
        _ => "output",
    }
}

/// UAC2 RANGE parameter block (4-byte layout): wNumSubRanges, then
/// (dMIN, dMAX, dRES) per subrange.
pub fn parse_ranges(b: &[u8]) -> Vec<(u32, u32, u32)> {
    let mut v = Vec::new();
    if b.len() < 2 {
        return v;
    }
    let n = rd16(b, 0) as usize;
    for i in 0..n {
        let o = 2 + i * 12;
        if o + 12 > b.len() {
            break;
        }
        let g = |k: usize| u32::from_le_bytes([b[o + k], b[o + k + 1], b[o + k + 2], b[o + k + 3]]);
        v.push((g(0), g(4), g(8)));
    }
    v
}

pub fn range_has(ranges: &[(u32, u32, u32)], rate: u32) -> bool {
    ranges.iter().any(|&(lo, hi, res)| rate >= lo && rate <= hi && (res == 0 || lo == hi || (rate - lo) % res == 0))
}

// ── Stream arithmetic ─────────────────────────────────────────────────────

/// Service intervals per second of an isochronous endpoint. Both full- and
/// high-speed isochronous bInterval are exponents: 2^(bInterval-1) frames
/// (1 ms) or microframes (125 µs).
pub fn packets_per_sec(high_speed: bool, interval: u8) -> u32 {
    let base = if high_speed { 8000 } else { 1000 };
    let exp = (interval.clamp(1, 16) - 1) as u32;
    (base >> exp.min(13)).max(1)
}

/// The xHCI endpoint-context Interval field (2^n × 125 µs) for an
/// isochronous endpoint.
pub fn xhci_interval(high_speed: bool, interval: u8) -> u32 {
    let e = (interval.clamp(1, 16) - 1) as u32;
    if high_speed { e } else { (e + 3).min(15) }
}

/// Nominal frames per (micro)frame in Q16.16 -- the unit feedback uses.
pub fn nominal_q16(rate: u32, high_speed: bool) -> u32 {
    let per = if high_speed { 8000u64 } else { 1000 };
    (((rate as u64) << 16) / per) as u32
}

/// Feedback decoder: the device reports the rate it consumes in frames per
/// (micro)frame -- Q10.14 in 3 bytes at full speed, Q16.16 in 4 bytes at high
/// speed (some devices get the format wrong). Like Linux, the shift is found
/// from the first plausible value, then values within -12.5%..+25% of the
/// nominal are accepted.
#[derive(Clone, Copy, Debug)]
pub struct Feedback {
    pub nominal: u32,
    shift: Option<i32>,
    /// Current rate in Q16.16 frames per (micro)frame.
    pub value: u32,
    pub accepted: u32,
    pub rejected: u32,
    pub last_raw: u32,
}

impl Feedback {
    pub fn new(nominal: u32) -> Self {
        Feedback { nominal, shift: None, value: nominal, accepted: 0, rejected: 0, last_raw: 0 }
    }

    pub fn shift(&self) -> Option<i32> {
        self.shift
    }

    /// Feed one received feedback packet (`len` bytes). True if it was used.
    pub fn update(&mut self, raw: u32, len: usize) -> bool {
        let mut f = if len == 3 { raw & 0x00FF_FFFF } else { raw & 0x0FFF_FFFF };
        self.last_raw = raw;
        if f == 0 || len < 3 {
            return false;
        }
        let n = self.nominal;
        match self.shift {
            None => {
                let mut s = 0i32;
                while (f as u64) < (n - n / 4) as u64 && s < 8 {
                    f <<= 1;
                    s += 1;
                }
                while f > n + n / 2 && s > -8 {
                    f >>= 1;
                    s -= 1;
                }
                self.shift = Some(s);
            },
            Some(s) => {
                f = if s >= 0 { f << s } else { f >> (-s) };
            },
        }
        if f >= n - n / 8 && f <= n + n / 4 {
            self.value = f;
            self.accepted += 1;
            true
        } else {
            self.rejected += 1;
            // a run of nonsense: guess the format again
            if self.rejected % 16 == 0 {
                self.shift = None;
            }
            false
        }
    }
}

/// Frames per packet from a Q16.16 rate: carries the fraction over, so the
/// long-run average is exact (44.1 kHz at 1 ms: 44,44,...,45).
#[derive(Clone, Copy, Debug)]
pub struct Pacer {
    acc: u32,
}

impl Pacer {
    pub fn new() -> Self {
        Pacer { acc: 0 }
    }
    /// `rate_q16` per (micro)frame, `frames_per_interval` (micro)frames per
    /// packet; capped at `max_frames`.
    pub fn next(&mut self, rate_q16: u32, frames_per_interval: u32, max_frames: u32) -> u32 {
        self.acc = self.acc.wrapping_add(rate_q16.wrapping_mul(frames_per_interval));
        let n = (self.acc >> 16).min(max_frames);
        self.acc -= n << 16;
        // never let a capped backlog grow without bound
        if self.acc > 4 << 16 {
            self.acc = 4 << 16;
        }
        n
    }
}

impl Default for Pacer {
    fn default() -> Self {
        Self::new()
    }
}

/// Write `frames` stereo i16 frames as `channels` × `subslot`-byte slots
/// (little-endian, MSB-justified; extra channels silent, mono = average).
pub fn convert(src: &[i16], frames: usize, channels: usize, subslot: usize, out: &mut [u8]) -> usize {
    let fb = channels * subslot;
    let mut o = 0;
    for f in 0..frames {
        let l = src[f * 2] as i32;
        let r = src[f * 2 + 1] as i32;
        for c in 0..channels {
            let s = match (channels, c) {
                (1, _) => (l + r) / 2,
                (_, 0) => l,
                (_, 1) => r,
                _ => 0,
            };
            let v = (s << (subslot * 8 - 16).min(16)) as u32;
            for k in 0..subslot {
                out[o + k] = (v >> (8 * k)) as u8;
            }
            o += subslot;
        }
    }
    debug_assert_eq!(o, frames * fb);
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PreSonus Studio 26c (194f:010a, bcdDevice 1.12) configuration 1, 354
    /// bytes, rebuilt from `lsusb -v` (linux-hardware.org probe b87a3ed6f4).
    pub fn studio26c() -> Vec<u8> {
        let mut v: Vec<u8> = Vec::new();
        let mut d = |b: &[u8]| {
            assert_eq!(b[0] as usize, b.len());
            v.extend_from_slice(b)
        };
        d(&[9, 2, 0x62, 0x01, 6, 1, 0, 0x80, 250]); // config, wTotalLength 354
        d(&[8, 11, 0, 3, 1, 0, 0x20, 0]); // IAD
        d(&[9, 4, 0, 0, 0, 1, 1, 0x20, 3]); // AC interface
        d(&[9, 0x24, 1, 0x00, 0x02, 8, 101, 0, 0x01]); // AC header 2.00
        d(&[8, 0x24, 0x0A, 41, 0x03, 0x07, 0, 9]); // clock source 41
        d(&[8, 0x24, 0x0B, 40, 1, 41, 0x03, 8]); // clock selector 40
        d(&[17, 0x24, 2, 2, 0x01, 0x01, 0, 40, 4, 0, 0, 0, 0, 13, 0, 0, 6]); // IT 2 USB streaming
        d(&[12, 0x24, 3, 20, 0x01, 0x03, 0, 2, 40, 0, 0, 0]); // OT 20 speaker
        d(&[17, 0x24, 2, 1, 0x01, 0x02, 0, 40, 2, 0, 0, 0, 0, 17, 0, 0, 0]); // IT 1 mic
        d(&[18, 0x24, 6, 11, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // FU 11
        d(&[12, 0x24, 3, 22, 0x01, 0x01, 0, 11, 40, 0, 0, 7]); // OT 22 USB streaming
        d(&[9, 4, 1, 0, 0, 1, 2, 0x20, 4]); // AS out alt 0
        d(&[9, 4, 1, 1, 2, 1, 2, 0x20, 4]); // AS out alt 1
        d(&[16, 0x24, 1, 2, 0, 1, 1, 0, 0, 0, 4, 0, 0, 0, 0, 13]); // AS_GENERAL 4ch PCM
        d(&[6, 0x24, 2, 1, 4, 24]); // format I, 4-byte subslot, 24 bits
        d(&[7, 5, 0x01, 0x05, 0x90, 0x01, 1]); // EP 1 OUT iso async 400
        d(&[8, 0x25, 1, 0, 0, 2, 8, 0]); // CS EP
        d(&[7, 5, 0x81, 0x11, 4, 0, 4]); // EP 1 IN feedback
        d(&[9, 4, 2, 0, 0, 1, 2, 0x20, 5]); // AS in alt 0
        d(&[9, 4, 2, 1, 1, 1, 2, 0x20, 5]); // AS in alt 1
        d(&[16, 0x24, 1, 22, 0, 1, 1, 0, 0, 0, 2, 0, 0, 0, 0, 17]);
        d(&[6, 0x24, 2, 1, 4, 24]);
        d(&[7, 5, 0x82, 0x05, 0xC8, 0x00, 1]); // EP 2 IN iso async 200
        d(&[8, 0x25, 1, 0, 0, 2, 8, 0]);
        d(&[9, 4, 3, 0, 0, 1, 1, 0, 0]); // stray UAC1 AC interface
        d(&[9, 0x24, 1, 0x00, 0x01, 9, 0, 1, 1]);
        d(&[9, 4, 4, 0, 2, 1, 3, 0, 0]); // MIDI streaming
        d(&[7, 0x24, 1, 0x00, 0x01, 65, 0]);
        d(&[6, 0x24, 2, 1, 1, 0]);
        d(&[6, 0x24, 2, 2, 2, 12]);
        d(&[9, 0x24, 3, 1, 3, 1, 2, 1, 0]);
        d(&[9, 0x24, 3, 2, 4, 1, 1, 1, 11]);
        d(&[9, 5, 0x02, 0x02, 0x00, 0x02, 0, 0, 0]);
        d(&[5, 0x25, 1, 1, 1]);
        d(&[9, 5, 0x83, 0x02, 0x00, 0x02, 0, 0, 0]);
        d(&[5, 0x25, 1, 1, 3]);
        d(&[9, 4, 5, 0, 0, 0xFE, 1, 1, 10]); // DFU
        d(&[9, 0x21, 7, 250, 0, 64, 0, 0x10, 0x01]);
        v
    }

    /// QEMU `usb-audio` (hw/usb/dev-audio.c, stereo): UAC1, full speed,
    /// synchronous iso OUT (bmAttributes 0x0D), 48 kHz 16-bit.
    pub fn qemu_usb_audio() -> Vec<u8> {
        let mut v: Vec<u8> = Vec::new();
        let mut d = |b: &[u8]| {
            assert_eq!(b[0] as usize, b.len());
            v.extend_from_slice(b)
        };
        d(&[9, 2, 0, 0, 2, 1, 0, 0xC0, 0x32]);
        d(&[9, 4, 0, 0, 0, 1, 1, 0, 0]);
        d(&[9, 0x24, 1, 0x00, 0x01, 0x2b, 0, 1, 1]);
        d(&[12, 0x24, 2, 1, 0x01, 0x01, 0, 2, 3, 0, 0, 0]);
        d(&[13, 0x24, 6, 2, 1, 2, 1, 0, 2, 0, 2, 0, 0]);
        d(&[9, 0x24, 3, 3, 0x01, 0x03, 0, 2, 0]);
        d(&[9, 4, 1, 0, 0, 1, 2, 0, 0]);
        d(&[9, 4, 1, 1, 1, 1, 2, 0, 0]);
        d(&[7, 0x24, 1, 1, 0, 1, 0]);
        d(&[11, 0x24, 2, 1, 2, 2, 16, 1, 0x80, 0xBB, 0x00]);
        d(&[9, 5, 0x01, 0x0D, 192, 0, 1, 0, 0]);
        d(&[7, 0x25, 1, 0, 0, 0, 0]);
        let n = v.len() as u16;
        v[2] = n as u8;
        v[3] = (n >> 8) as u8;
        v
    }

    #[test]
    fn studio26c_length() {
        assert_eq!(studio26c().len(), 354);
    }

    #[test]
    fn studio26c_plan() {
        let p = parse_config(&studio26c()).unwrap();
        assert_eq!(p.uac, 2);
        assert_eq!(p.ac_iface, 0);
        assert_eq!((p.iface, p.alt), (1, 1));
        assert_eq!((p.channels, p.subslot, p.bits), (4, 4, 24));
        assert_eq!(p.data_ep.addr, 0x01);
        assert_eq!(p.data_ep.sync_type(), SYNC_ASYNC);
        assert_eq!(p.data_ep.mps(), 400);
        assert_eq!(p.data_ep.extra_tx(), 0);
        assert_eq!(p.data_ep.interval, 1);
        let fb = p.fb_ep.unwrap();
        assert_eq!((fb.addr, fb.mps(), fb.interval), (0x81, 4, 4));
        assert_eq!(p.clock_id, 41);
        assert_eq!(p.clock_controls, 0x07);
        assert_eq!(p.clock_selector, Some((40, 1, 0x03)));
        assert_eq!(p.terminal, 2);
        assert_eq!(p.sink_type, 0x0301);
        assert_eq!(p.candidates, 1);
        assert_eq!(p.frame_bytes(), 16);
    }

    #[test]
    fn qemu_plan() {
        let p = parse_config(&qemu_usb_audio()).unwrap();
        assert_eq!(p.uac, 1);
        assert_eq!((p.iface, p.alt), (1, 1));
        assert_eq!((p.channels, p.subslot, p.bits), (2, 2, 16));
        assert_eq!(p.data_ep.addr, 0x01);
        assert_eq!(p.data_ep.sync_type(), SYNC_SYNC); // 0x0D: iso, synchronous
        assert_eq!(p.data_ep.mps(), 192);
        assert!(p.fb_ep.is_none());
        assert_eq!(p.rates, alloc::vec![48000]);
        assert!(p.rate_listed(48000));
        assert!(!p.rate_listed(44100));
        assert!(!p.ep_freq_control);
        assert_eq!(p.sink_type, 0x0301);
    }

    #[test]
    fn not_audio() {
        let kbd = [9u8, 2, 34, 0, 1, 1, 0, 0xA0, 50, 9, 4, 0, 0, 1, 3, 1, 1, 0, 7, 5, 0x81, 3, 8, 0, 10];
        assert!(parse_config(&kbd).is_err());
    }

    #[test]
    fn intervals() {
        assert_eq!(packets_per_sec(true, 1), 8000);
        assert_eq!(packets_per_sec(true, 4), 1000);
        assert_eq!(packets_per_sec(false, 1), 1000);
        assert_eq!(xhci_interval(true, 1), 0);
        assert_eq!(xhci_interval(true, 4), 3);
        assert_eq!(xhci_interval(false, 1), 3);
    }

    #[test]
    fn ranges() {
        // Studio-26c-like: 44.1, 48, 88.2, 96, 176.4, 192 as single-point ranges
        let rates = [44100u32, 48000, 88200, 96000, 176400, 192000];
        let mut b = alloc::vec![rates.len() as u8, 0];
        for r in rates {
            for _ in 0..2 {
                b.extend_from_slice(&r.to_le_bytes());
            }
            b.extend_from_slice(&0u32.to_le_bytes());
        }
        let r = parse_ranges(&b);
        assert_eq!(r.len(), 6);
        assert!(range_has(&r, 48000));
        assert!(!range_has(&r, 32000));
        assert!(range_has(&[(8000, 192000, 1)], 48000));
    }

    #[test]
    fn feedback_hs_q16() {
        let n = nominal_q16(48000, true);
        assert_eq!(n, 6 << 16);
        let mut f = Feedback::new(n);
        assert!(f.update(0x0006_0010, 4));
        assert_eq!(f.shift(), Some(0));
        assert_eq!(f.value, 0x0006_0010);
        assert!(!f.update(0x0010_0000, 4)); // 16 frames/uframe: nonsense
        assert_eq!(f.value, 0x0006_0010);
    }

    #[test]
    fn feedback_fs_q10_14() {
        let n = nominal_q16(48000, false);
        let mut f = Feedback::new(n);
        // 48.0 frames/ms in 10.14 = 48 << 14
        assert!(f.update(48 << 14, 3));
        assert_eq!(f.shift(), Some(2));
        assert_eq!(f.value, 48 << 16);
    }

    #[test]
    fn feedback_wrong_format_hs() {
        // Some HS devices send 10.14-ish values in 4 bytes (shifted by 2).
        let n = nominal_q16(48000, true);
        let mut f = Feedback::new(n);
        assert!(f.update((6 << 14) + 3, 4));
        assert_eq!(f.shift(), Some(2));
        assert_eq!(f.value, (6 << 16) + 12);
    }

    #[test]
    fn pacer_exact() {
        let mut p = Pacer::new();
        let q = nominal_q16(44100, false);
        let mut total = 0;
        for _ in 0..1000 {
            let n = p.next(q, 1, 64);
            assert!(n == 44 || n == 45);
            total += n;
        }
        assert!((44099..=44100).contains(&total));
        let mut p = Pacer::new();
        let q = nominal_q16(48000, true);
        let t: u32 = (0..8000).map(|_| p.next(q, 1, 25)).sum();
        assert_eq!(t, 48000);
        // slightly fast device clock: 6.001 frames/uframe
        let mut p = Pacer::new();
        let q = (6 << 16) + 66; // ~6.001
        let t: u32 = (0..8000).map(|_| p.next(q, 1, 25)).sum();
        assert!((48007..=48009).contains(&t), "{t}");
    }

    #[test]
    fn convert_formats() {
        let src = [0x1234i16, -2, 0x7FFF, -32768];
        let mut out = [0u8; 64];
        // 4 channels × 4-byte slots, 24 valid bits MSB-justified
        let n = convert(&src, 2, 4, 4, &mut out);
        assert_eq!(n, 32);
        assert_eq!(&out[0..4], &[0, 0, 0x34, 0x12]);
        assert_eq!(&out[4..8], &[0, 0, 0xFE, 0xFF]);
        assert_eq!(&out[8..16], &[0; 8]);
        assert_eq!(&out[16..20], &[0, 0, 0xFF, 0x7F]);
        assert_eq!(&out[20..24], &[0, 0, 0x00, 0x80]);
        // 2 ch × 3 bytes
        let n = convert(&src, 1, 2, 3, &mut out);
        assert_eq!(n, 6);
        assert_eq!(&out[0..6], &[0, 0x34, 0x12, 0, 0xFE, 0xFF]);
        // 2 ch × 2 bytes
        let n = convert(&src, 1, 2, 2, &mut out);
        assert_eq!(&out[..n], &[0x34, 0x12, 0xFE, 0xFF]);
    }
}
