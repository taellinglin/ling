//! Intel High Definition Audio driver (ICH6/ICH8-style HDA controller + one
//! codec) -- the audio device new VirtualBox VMs get by default ("Intel HD
//! Audio") and QEMU's `-device intel-hda -device hda-output` /
//! `-device ich9-intel-hda`. PCM-out only, 48 kHz 16-bit stereo, polled:
//! no interrupts, the mixer's per-frame [`pump`] keeps the ring fed (the
//! same disclosed pull model as `ac97.rs`).
//!
//! Bring-up:
//! 1. PCI class 0x04/0x03; memory BAR0 (64-bit capable -- this kernel
//!    identity-maps [0, 4GiB), so a BAR above that is refused, not mapped).
//! 2. Controller reset (GCTL.CRST 0 -> 1), codecs self-enumerate into
//!    STATESTS; the first present codec is used.
//! 3. Codec commands go through the CORB/RIRB rings (polled); if the rings
//!    don't answer, the Immediate Command interface (ICOI/IRII/ICIS) is
//!    used instead.
//! 4. Widget walk of the codec's audio function group: every output-capable
//!    pin whose default config says it is wired (line-out / speaker /
//!    headphone) gets a path back to a DAC, through mixers / selectors;
//!    each widget on the path is powered (D0), its connection selected and
//!    its amps unmuted at 0 dB; pins get OUT_EN (+HP_EN for headphones) and
//!    EAPD. Every analog DAC is put on stream tag 1 / channel 0 at 48 kHz
//!    16-bit stereo (format 0x0011), so whichever DAC an emulator treats as
//!    "front" (VirtualBox ignores routing) plays the stream.
//! 5. One output stream descriptor (index = number of input streams) runs a
//!    cyclic BDL of NUM_BUFFERS buffers (FRAMES_PER_BUFFER stereo frames
//!    each, 128-byte aligned). The ring never stops on its own, so
//!    [`pump`] reads LPIB to find the playing buffer, zeroes buffers the
//!    hardware has finished with (an underrun then plays silence, never a
//!    stale repeat) and keeps QUEUE_DEPTH buffers rendered ahead.
//! 6. Because the ring never stops by itself, [`watchdog`] (called from the
//!    100 Hz timer IRQ) stops the stream once it has run STOP_AFTER_SILENT
//!    buffers into silence; the next pump resets the stream (SRST) and
//!    re-primes it. Stopping only after silence matters: emulated codecs
//!    buffer audio past LPIB and a stop discards it (QEMU: 2 buffers).

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

use crate::arch::{pci, timer};
use crate::drivers::serial;

pub const NUM_BUFFERS: usize = 32;
pub const FRAMES_PER_BUFFER: usize = 1024;
const SAMPLES_PER_BUFFER: usize = FRAMES_PER_BUFFER * 2; // stereo
const BUFFER_BYTES: usize = SAMPLES_PER_BUFFER * 2;
/// Buffers kept rendered ahead of the playing one (~340ms at 48 kHz),
/// matching the AC'97 driver's latency/jitter trade-off.
pub const QUEUE_DEPTH: usize = 16;
/// The watchdog stops a starved stream only after it has fetched this many
/// silent buffers (~170ms) past the last rendered one. Must stay below
/// NUM_BUFFERS - QUEUE_DEPTH: that's how much silence lies ahead of the
/// queue before the ring would wrap into already-played audio.
const STOP_AFTER_SILENT: usize = 8;

/// 48 kHz base, x1, /1, 16 bits/sample, 2 channels.
const STREAM_FORMAT: u16 = 0x0011;
const STREAM_TAG: u32 = 1;

// Controller registers (byte offsets from BAR0).
const GCAP: usize = 0x00; // 16
const GCTL: usize = 0x08; // 32
const STATESTS: usize = 0x0E; // 16
const INTCTL: usize = 0x20; // 32
const CORBLBASE: usize = 0x40;
const CORBUBASE: usize = 0x44;
const CORBWP: usize = 0x48; // 16
const CORBRP: usize = 0x4A; // 16
const CORBCTL: usize = 0x4C; // 8
const CORBSIZE: usize = 0x4E; // 8
const RIRBLBASE: usize = 0x50;
const RIRBUBASE: usize = 0x54;
const RIRBWP: usize = 0x58; // 16
const RINTCNT: usize = 0x5A; // 16
const RIRBCTL: usize = 0x5C; // 8
const RIRBSTS: usize = 0x5D; // 8
const RIRBSIZE: usize = 0x5E; // 8
const ICOI: usize = 0x60; // 32
const IRII: usize = 0x64; // 32
const ICIS: usize = 0x68; // 16
const DPLBASE: usize = 0x70;
const DPUBASE: usize = 0x74;

// Stream-descriptor registers (offsets within a descriptor).
const SD_CTL: usize = 0x00; // 24 bits + STS byte at 0x03
const SD_STS: usize = 0x03; // 8
const SD_LPIB: usize = 0x04;
const SD_CBL: usize = 0x08;
const SD_LVI: usize = 0x0C; // 16
const SD_FMT: usize = 0x12; // 16
const SD_BDPL: usize = 0x18;
const SD_BDPU: usize = 0x1C;
const SD_CTL_SRST: u32 = 1 << 0;
const SD_CTL_RUN: u32 = 1 << 1;

// Codec verbs / parameters.
const VERB_GET_PARAM: u32 = 0xF00;
const VERB_GET_CONN_LIST: u32 = 0xF02;
const VERB_SET_CONN_SELECT: u32 = 0x701;
const VERB_SET_POWER: u32 = 0x705;
const VERB_SET_STREAM: u32 = 0x706;
const VERB_SET_PIN_CTL: u32 = 0x707;
const VERB_SET_EAPD: u32 = 0x70C;
const VERB_GET_CONFIG_DEFAULT: u32 = 0xF1C;
const VERB4_SET_FORMAT: u32 = 0x2;
const VERB4_SET_AMP: u32 = 0x3;

const PARAM_VENDOR_ID: u32 = 0x00;
const PARAM_NODE_COUNT: u32 = 0x04;
const PARAM_FG_TYPE: u32 = 0x05;
const PARAM_WIDGET_CAPS: u32 = 0x09;
const PARAM_PIN_CAPS: u32 = 0x0C;
const PARAM_IN_AMP_CAPS: u32 = 0x0D;
const PARAM_CONN_LEN: u32 = 0x0E;
const PARAM_OUT_AMP_CAPS: u32 = 0x12;

const WT_OUTPUT: u32 = 0;
const WT_MIXER: u32 = 2;
const WT_SELECTOR: u32 = 3;
const WT_PIN: u32 = 4;

const CORB_ENTRIES: usize = 256;

#[derive(Clone, Copy)]
#[repr(C)]
struct BdlEntry {
    addr: u64,
    len: u32,
    ioc: u32,
}

#[repr(C, align(128))]
struct Bdl([BdlEntry; NUM_BUFFERS]);
#[repr(C, align(4096))]
struct DmaBuffers([[i16; SAMPLES_PER_BUFFER]; NUM_BUFFERS]);
#[repr(C, align(1024))]
struct Corb([u32; CORB_ENTRIES]);
#[repr(C, align(2048))]
struct Rirb([u64; CORB_ENTRIES]);

static mut BDL: Bdl = Bdl([BdlEntry { addr: 0, len: 0, ioc: 0 }; NUM_BUFFERS]);
static mut DMA: DmaBuffers = DmaBuffers([[0; SAMPLES_PER_BUFFER]; NUM_BUFFERS]);
static mut CORB: Corb = Corb([0; CORB_ENTRIES]);
static mut RIRB: Rirb = Rirb([0; CORB_ENTRIES]);

static mut BASE: usize = 0;
static mut PRESENT: bool = false;
static mut CODEC: u32 = 0;
/// true = CORB/RIRB rings, false = immediate command interface.
static mut USE_CORB: bool = true;
static mut CORB_WP: usize = 0;
static mut RIRB_RP: usize = 0;
/// Byte offset of our output stream descriptor.
static mut SD: usize = 0;
static mut STARTED: bool = false;
/// Next buffer to render into.
static mut WRITE: usize = 0;
/// Buffer the hardware was playing at the previous pump.
static mut LAST_PLAY: usize = 0;

fn r8(off: usize) -> u8 {
    unsafe { read_volatile((BASE + off) as *const u8) }
}
fn r16(off: usize) -> u16 {
    unsafe { read_volatile((BASE + off) as *const u16) }
}
fn r32(off: usize) -> u32 {
    unsafe { read_volatile((BASE + off) as *const u32) }
}
fn w8(off: usize, v: u8) {
    unsafe { write_volatile((BASE + off) as *mut u8, v) }
}
fn w16(off: usize, v: u16) {
    unsafe { write_volatile((BASE + off) as *mut u16, v) }
}
fn w32(off: usize, v: u32) {
    unsafe { write_volatile((BASE + off) as *mut u32, v) }
}

/// Bounded spin without a cooperative yield (no re-entry during bring-up).
fn wait_us(timeout_us: u64, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = timer::now_us() + timeout_us;
    loop {
        if cond() {
            return true;
        }
        if timer::now_us() >= deadline {
            return false;
        }
        core::hint::spin_loop();
    }
}

fn log_hex(label: &str, v: u32) {
    serial::write_str(label);
    let mut buf = [0u8; 10];
    buf[0] = b'0';
    buf[1] = b'x';
    for i in 0..8 {
        let nib = (v >> ((7 - i) * 4)) & 0xF;
        buf[2 + i] = if nib < 10 { b'0' + nib as u8 } else { b'a' + nib as u8 - 10 };
    }
    serial::write(&buf);
}

// -- codec command transport -------------------------------------------------

fn corb_setup() -> bool {
    // Stop both ring DMAs before touching their registers.
    w8(CORBCTL, 0);
    w8(RIRBCTL, 0);
    wait_us(10_000, || r8(CORBCTL) & 0x02 == 0 && r8(RIRBCTL) & 0x02 == 0);
    let corb = &raw const CORB as u64;
    let rirb = &raw const RIRB as u64;
    w32(CORBLBASE, corb as u32);
    w32(CORBUBASE, (corb >> 32) as u32);
    w32(RIRBLBASE, rirb as u32);
    w32(RIRBUBASE, (rirb >> 32) as u32);
    // 256 entries if offered (size bits 1:0 = 2); emulators often fix it.
    if r8(CORBSIZE) & 0x40 != 0 {
        w8(CORBSIZE, (r8(CORBSIZE) & 0xFC) | 0x02);
    }
    if r8(RIRBSIZE) & 0x40 != 0 {
        w8(RIRBSIZE, (r8(RIRBSIZE) & 0xFC) | 0x02);
    }
    // CORB read pointer reset handshake (set RST, see it, clear it, see
    // it clear) -- bounded, since emulators don't all echo the bit.
    w16(CORBRP, 0x8000);
    wait_us(1_000, || r16(CORBRP) & 0x8000 != 0);
    w16(CORBRP, 0);
    wait_us(1_000, || r16(CORBRP) & 0x8000 == 0);
    w16(CORBWP, 0);
    w16(RIRBWP, 0x8000); // RIRB write pointer reset
    w16(RINTCNT, 0xFF);
    w8(RIRBSTS, 0x05);
    unsafe {
        CORB_WP = 0;
        RIRB_RP = 0;
    }
    w8(CORBCTL, 0x02); // DMA run
    w8(RIRBCTL, 0x02); // DMA run, no interrupts
    wait_us(10_000, || r8(CORBCTL) & 0x02 != 0 && r8(RIRBCTL) & 0x02 != 0)
}

fn corb_command(verb: u32) -> Option<u32> {
    unsafe {
        let wp = (CORB_WP + 1) % CORB_ENTRIES;
        write_volatile(&raw mut CORB.0[wp], verb);
        fence(Ordering::SeqCst);
        CORB_WP = wp;
        w16(CORBWP, wp as u16);
        loop {
            let ok = wait_us(20_000, || (r16(RIRBWP) & 0xFF) as usize != RIRB_RP);
            if !ok {
                return None;
            }
            fence(Ordering::SeqCst);
            RIRB_RP = (RIRB_RP + 1) % CORB_ENTRIES;
            let entry = read_volatile(&raw const RIRB.0[RIRB_RP]);
            w8(RIRBSTS, 0x05);
            // bit 36 (resp_ex bit 4) = unsolicited response: skip it.
            if entry & (1 << 36) == 0 {
                return Some(entry as u32);
            }
        }
    }
}

fn immediate_command(verb: u32) -> Option<u32> {
    if !wait_us(20_000, || r16(ICIS) & 0x01 == 0) {
        return None;
    }
    w16(ICIS, 0x02); // clear IRV
    w32(ICOI, verb);
    w16(ICIS, 0x01); // ICB: send
    if !wait_us(20_000, || r16(ICIS) & 0x03 == 0x02) {
        return None;
    }
    let resp = r32(IRII);
    w16(ICIS, 0x02);
    Some(resp)
}

fn command(verb: u32) -> Option<u32> {
    if unsafe { USE_CORB } {
        corb_command(verb)
    } else {
        immediate_command(verb)
    }
}

/// 12-bit verb with an 8-bit payload.
fn verb12(nid: u32, verb: u32, payload: u32) -> u32 {
    let cad = unsafe { CODEC };
    (cad << 28) | ((nid & 0xFF) << 20) | ((verb & 0xFFF) << 8) | (payload & 0xFF)
}

/// 4-bit verb with a 16-bit payload.
fn verb4(nid: u32, verb: u32, payload: u32) -> u32 {
    let cad = unsafe { CODEC };
    (cad << 28) | ((nid & 0xFF) << 20) | ((verb & 0xF) << 16) | (payload & 0xFFFF)
}

fn param(nid: u32, p: u32) -> u32 {
    command(verb12(nid, VERB_GET_PARAM, p)).unwrap_or(0)
}

fn send(v: u32) {
    let _ = command(v);
}

// -- widget graph --------------------------------------------------------------

static mut CAPS: [u32; 256] = [0; 256];
static mut AFG: u32 = 0;

fn wtype(nid: u32) -> u32 {
    unsafe { (CAPS[nid as usize & 0xFF] >> 20) & 0xF }
}

/// Read up to `out.len()` connection-list entries of `nid`; returns count.
fn conn_list(nid: u32, out: &mut [u32; 32]) -> usize {
    if unsafe { CAPS[nid as usize & 0xFF] } & (1 << 8) == 0 {
        return 0;
    }
    let lenp = param(nid, PARAM_CONN_LEN);
    let long = lenp & 0x80 != 0;
    let len = (lenp & 0x7F) as usize;
    let per = if long { 2 } else { 4 };
    let bits = if long { 16 } else { 8 };
    let mask = (1u32 << bits) - 1;
    let range_bit = 1u32 << (bits - 1);
    let mut n = 0;
    let mut prev = 0u32;
    let mut i = 0;
    while i < len {
        let resp = command(verb12(nid, VERB_GET_CONN_LIST, i as u32)).unwrap_or(0);
        for j in 0..per {
            if i + j >= len {
                break;
            }
            let e = (resp >> (j * bits)) & mask;
            let id = e & (range_bit - 1);
            if e & range_bit != 0 && prev != 0 && id > prev {
                // range: prev+1 ..= id
                let mut k = prev + 1;
                while k <= id && n < out.len() {
                    out[n] = k;
                    n += 1;
                    k += 1;
                }
            } else if n < out.len() {
                out[n] = id;
                n += 1;
            }
            prev = id;
        }
        i += per;
    }
    n
}

/// Depth-first search from `nid` down to an analog output converter.
/// Records (widget, chosen connection index) into `path`.
fn find_dac(nid: u32, path: &mut [(u32, usize); 8], depth: usize) -> Option<usize> {
    if depth + 1 >= path.len() {
        return None;
    }
    let mut conns = [0u32; 32];
    let n = conn_list(nid, &mut conns);
    // Prefer a directly connected DAC, then go through mixers/selectors.
    for pass in 0..2 {
        for (idx, &c) in conns[..n].iter().enumerate() {
            if c == 0 || c > 255 {
                continue;
            }
            let t = wtype(c);
            let digital = unsafe { CAPS[c as usize] } & (1 << 9) != 0;
            if pass == 0 && t == WT_OUTPUT && !digital {
                path[depth] = (nid, idx);
                path[depth + 1] = (c, 0);
                return Some(depth + 2);
            }
            if pass == 1 && (t == WT_MIXER || t == WT_SELECTOR) {
                path[depth] = (nid, idx);
                if let Some(len) = find_dac(c, path, depth + 1) {
                    return Some(len);
                }
            }
        }
    }
    None
}

fn amp_gain(nid: u32, p: u32) -> u32 {
    // Per-widget amp caps when "amp param override" is set, else the AFG's.
    let caps = if unsafe { CAPS[nid as usize] } & (1 << 3) != 0 {
        param(nid, p)
    } else {
        param(unsafe { AFG }, p)
    };
    caps & 0x7F // offset = the step that is 0 dB
}

fn unmute_out(nid: u32) {
    if unsafe { CAPS[nid as usize] } & (1 << 2) != 0 {
        let g = amp_gain(nid, PARAM_OUT_AMP_CAPS);
        // output amp, left + right, unmuted, 0 dB
        send(verb4(nid, VERB4_SET_AMP, 0xB000 | g));
    }
}

fn unmute_in(nid: u32, index: usize) {
    if unsafe { CAPS[nid as usize] } & (1 << 1) != 0 {
        let g = amp_gain(nid, PARAM_IN_AMP_CAPS);
        send(verb4(nid, VERB4_SET_AMP, 0x7000 | ((index as u32 & 0xF) << 8) | g));
    }
}

fn power_on(nid: u32) {
    send(verb12(nid, VERB_SET_POWER, 0));
}

fn setup_dac(nid: u32) {
    power_on(nid);
    send(verb4(nid, VERB4_SET_FORMAT, STREAM_FORMAT as u32));
    send(verb12(nid, VERB_SET_STREAM, (STREAM_TAG << 4) | 0));
    unmute_out(nid);
}

/// Configure the codec's output paths; true if at least one pin got a DAC.
fn setup_codec() -> bool {
    let vendor = param(0, PARAM_VENDOR_ID);
    log_hex("hda: codec vendor ", vendor);
    serial::write(b"\n");
    let root = param(0, PARAM_NODE_COUNT);
    let (fg_start, fg_count) = ((root >> 16) & 0xFF, root & 0xFF);
    let mut afg = 0;
    for nid in fg_start..fg_start + fg_count {
        if param(nid, PARAM_FG_TYPE) & 0xFF == 0x01 {
            afg = nid;
            break;
        }
    }
    if afg == 0 {
        serial::write_str("hda: no audio function group\n");
        return false;
    }
    unsafe { AFG = afg };
    power_on(afg);
    timer::delay_us(1_000);

    let sub = param(afg, PARAM_NODE_COUNT);
    let (start, count) = ((sub >> 16) & 0xFF, sub & 0xFF);
    let end = (start + count).min(256);
    for nid in start..end {
        unsafe { CAPS[nid as usize] = param(nid, PARAM_WIDGET_CAPS) };
    }

    // Every analog DAC listens to our stream (harmless duplicates on real
    // hardware; covers emulators that tie "front" to a fixed DAC).
    for nid in start..end {
        let digital = unsafe { CAPS[nid as usize] } & (1 << 9) != 0;
        if wtype(nid) == WT_OUTPUT && !digital {
            setup_dac(nid);
        }
    }

    // Output pins: wired line-out / speaker / headphone first; if the
    // default configs say nothing useful, take any output-capable pin.
    let mut configured = 0;
    for strict in [true, false] {
        for nid in start..end {
            if wtype(nid) != WT_PIN {
                continue;
            }
            let pcaps = param(nid, PARAM_PIN_CAPS);
            if pcaps & (1 << 4) == 0 {
                continue; // not output capable
            }
            let cfg = command(verb12(nid, VERB_GET_CONFIG_DEFAULT, 0)).unwrap_or(0);
            let conn = cfg >> 30;
            let dev = (cfg >> 20) & 0xF;
            if strict && (conn == 1 || dev > 2) {
                continue;
            }
            let mut path = [(0u32, 0usize); 8];
            let Some(len) = find_dac(nid, &mut path, 0) else {
                continue;
            };
            for &(w, idx) in &path[..len] {
                power_on(w);
                let t = wtype(w);
                if t == WT_PIN || t == WT_SELECTOR {
                    send(verb12(w, VERB_SET_CONN_SELECT, idx as u32));
                }
                if t == WT_MIXER || t == WT_SELECTOR {
                    unmute_in(w, idx);
                }
                unmute_out(w);
            }
            let hp = dev == 2 && pcaps & (1 << 3) != 0;
            send(verb12(nid, VERB_SET_PIN_CTL, if hp { 0xC0 } else { 0x40 }));
            if pcaps & (1 << 16) != 0 {
                send(verb12(nid, VERB_SET_EAPD, 0x02));
            }
            log_hex("hda: pin ", nid);
            log_hex(" cfg ", cfg);
            log_hex(" -> dac ", path[len - 1].0);
            serial::write(b"\n");
            configured += 1;
        }
        if configured > 0 {
            break;
        }
    }
    configured > 0
}

/// Reset our output stream descriptor and (re)program it, stopped: cyclic
/// BDL over all DMA buffers (zeroed), CBL, LVI, format, stream tag. After
/// SRST the DMA position and LPIB are 0 on every implementation -- which
/// is why a restart goes through here rather than a bare RUN 0->1 (QEMU
/// restarts that at BDL entry 0, real hardware resumes in place).
fn stream_program(sd: usize) {
    // Stream reset handshake.
    w32(sd + SD_CTL, 0);
    wait_us(10_000, || r32(sd + SD_CTL) & SD_CTL_RUN == 0);
    w32(sd + SD_CTL, SD_CTL_SRST);
    wait_us(10_000, || r32(sd + SD_CTL) & SD_CTL_SRST != 0);
    w32(sd + SD_CTL, 0);
    wait_us(10_000, || r32(sd + SD_CTL) & SD_CTL_SRST == 0);

    unsafe {
        let dma = &mut *&raw mut DMA;
        let bdl = &mut *&raw mut BDL;
        for i in 0..NUM_BUFFERS {
            dma.0[i].fill(0);
            bdl.0[i] = BdlEntry {
                addr: dma.0[i].as_ptr() as u64,
                len: BUFFER_BYTES as u32,
                ioc: 0,
            };
        }
        fence(Ordering::SeqCst);
        let bdl_addr = &raw const BDL as u64;
        w32(sd + SD_BDPL, bdl_addr as u32);
        w32(sd + SD_BDPU, (bdl_addr >> 32) as u32);
    }
    w32(sd + SD_CBL, (BUFFER_BYTES * NUM_BUFFERS) as u32);
    w16(sd + SD_LVI, (NUM_BUFFERS - 1) as u16);
    w16(sd + SD_FMT, STREAM_FORMAT);
    // Stream tag in CTL bits 23:20; STS byte written as 0 (nothing cleared).
    w32(sd + SD_CTL, STREAM_TAG << 20);
    w8(sd + SD_STS, 0x1C);

}

// -- init / pump -----------------------------------------------------------------

/// Find and bring up an HDA controller + codec. Idempotent; false when
/// absent or when nothing usable answered.
pub fn init() -> bool {
    if unsafe { PRESENT } {
        return true;
    }
    // PCI class 0x04 (multimedia), subclass 0x03 (HD Audio), prog-if 0.
    let Some(loc) = pci::find(0x04, 0x03, 0x00) else {
        return false;
    };
    let bar0 = loc.read32(0x10);
    if bar0 & 1 != 0 {
        return false; // must be a memory BAR
    }
    let mut base = (bar0 & 0xFFFF_FFF0) as u64;
    if (bar0 >> 1) & 3 == 2 {
        base |= (loc.read32(0x14) as u64) << 32;
    }
    if base == 0 || base >= 0x1_0000_0000 {
        log_hex("hda: BAR0 not identity-mapped: hi=", (base >> 32) as u32);
        serial::write(b"\n");
        return false;
    }
    unsafe { BASE = base as usize };
    // Memory space + bus master.
    let cmd = loc.read32(0x04);
    pci::write32(loc.bus, loc.device, loc.function, 0x04, cmd | 0x0006);

    // Controller reset: CRST 0, wait, CRST 1, wait, then >=521us for the
    // codecs to request enumeration (STATESTS).
    w8(CORBCTL, 0);
    w8(RIRBCTL, 0);
    w32(INTCTL, 0);
    w32(DPLBASE, 0);
    w32(DPUBASE, 0);
    w32(GCTL, r32(GCTL) & !1);
    wait_us(100_000, || r32(GCTL) & 1 == 0);
    timer::delay_us(200);
    w32(GCTL, r32(GCTL) | 1);
    if !wait_us(100_000, || r32(GCTL) & 1 != 0) {
        serial::write_str("hda: controller stuck in reset\n");
        return false;
    }
    wait_us(10_000, || r16(STATESTS) & 0x7FFF != 0);
    timer::delay_us(1_000);
    let codecs = r16(STATESTS) & 0x7FFF;
    w16(STATESTS, codecs);
    if codecs == 0 {
        serial::write_str("hda: no codec\n");
        return false;
    }
    unsafe { CODEC = codecs.trailing_zeros() };

    // Command transport: CORB/RIRB, else the immediate interface.
    let corb_ok = corb_setup();
    unsafe { USE_CORB = corb_ok };
    if !corb_ok || corb_command(verb12(0, VERB_GET_PARAM, PARAM_VENDOR_ID)).is_none() {
        w8(CORBCTL, 0);
        w8(RIRBCTL, 0);
        unsafe { USE_CORB = false };
        if immediate_command(verb12(0, VERB_GET_PARAM, PARAM_VENDOR_ID)).is_none() {
            serial::write_str("hda: codec does not answer\n");
            return false;
        }
        serial::write_str("hda: using immediate command interface\n");
    }

    if !setup_codec() {
        serial::write_str("hda: no usable output path\n");
        return false;
    }

    // Output stream descriptor: the first one after the input streams.
    let gcap = r16(GCAP) as usize;
    let iss = (gcap >> 8) & 0xF;
    let oss = (gcap >> 12) & 0xF;
    if oss == 0 {
        serial::write_str("hda: no output streams\n");
        return false;
    }
    let sd = 0x80 + iss * 0x20;
    unsafe { SD = sd };

    stream_program(sd);

    unsafe {
        WRITE = 0;
        LAST_PLAY = 0;
        STARTED = false;
        PRESENT = true;
    }
    log_hex("hda: ready, output stream @", sd as u32);
    serial::write(b"\n");
    true
}

pub fn present() -> bool {
    unsafe { PRESENT }
}

/// Keep QUEUE_DEPTH buffers rendered ahead of the playing one; starts the
/// stream on the first call. Returns how many buffers were rendered.
///
/// `WRITE` / `STARTED` are shared with [`watchdog`] (timer IRQ), hence the
/// volatile accesses; the IRQ only reads them and may clear RUN, which the
/// next pump answers with a stream reset + re-prime, so no interrupt
/// masking is needed.
pub fn pump(render: &mut dyn FnMut(&mut [i16])) -> usize {
    if !present() {
        return 0;
    }
    let sd = unsafe { SD };
    let dma = unsafe { &mut *&raw mut DMA };
    let wr = &raw mut WRITE;
    let mut filled = 0;
    unsafe {
        if read_volatile(&raw const STARTED) && r32(sd + SD_CTL) & SD_CTL_RUN == 0 {
            // The watchdog paused us (pump stalled past the queue), or the
            // emulator reset the stream: everything rendered has played,
            // so re-prime from a freshly reset stream.
            write_volatile(&raw mut STARTED, false);
            stream_program(sd);
        }
        if !read_volatile(&raw const STARTED) {
            for i in 0..QUEUE_DEPTH {
                render(&mut dma.0[i]);
            }
            fence(Ordering::SeqCst);
            write_volatile(wr, QUEUE_DEPTH % NUM_BUFFERS);
            LAST_PLAY = 0;
            write_volatile(&raw mut STARTED, true);
            w32(sd + SD_CTL, (STREAM_TAG << 20) | SD_CTL_RUN);
            return QUEUE_DEPTH;
        }
        let mut write = read_volatile(wr);
        let lpib = r32(sd + SD_LPIB) as usize;
        let play = (lpib / BUFFER_BYTES) % NUM_BUFFERS;
        // Silence what the hardware has finished with; if that sweep
        // crosses our write cursor, the pump fell behind (underrun).
        let mut underrun = false;
        while LAST_PLAY != play {
            if LAST_PLAY == write {
                underrun = true;
            }
            dma.0[LAST_PLAY].fill(0);
            LAST_PLAY = (LAST_PLAY + 1) % NUM_BUFFERS;
        }
        if play == write {
            underrun = true;
        }
        if underrun {
            // The playing buffer is silence (the landing pad below / a
            // swept one); restart the queue right after it.
            write = (play + 1) % NUM_BUFFERS;
            write_volatile(wr, write);
        }
        // Buffers play..write-1 belong to the hardware (play included).
        let mut queued = (write + NUM_BUFFERS - play) % NUM_BUFFERS;
        while queued < QUEUE_DEPTH {
            render(&mut dma.0[write]);
            fence(Ordering::SeqCst);
            write = (write + 1) % NUM_BUFFERS;
            write_volatile(wr, write);
            queued += 1;
            filled += 1;
        }
        // Landing pad: everything outside [play, write) is silence (swept
        // above / zeroed at stream reset; this re-zeroes the first one as
        // cheap insurance), so if pumping stops the hardware runs into
        // silence and the watchdog halts it there -- never into a stale lap.
        if filled > 0 {
            dma.0[write].fill(0);
        }
        fence(Ordering::SeqCst);
        w8(sd + SD_STS, 0x1C);
    }
    filled
}

/// Timer-IRQ hook (100 Hz): an HDA ring never stops by itself, so when
/// nobody pumps (a long blocking load, a screen that doesn't run the audio
/// pump) it would replay the last lap forever. Once playback reaches the
/// silent buffers past the queue, stop the stream (see STOP_AFTER_SILENT);
/// the next [`pump`] resets, refills and restarts it. Reads two registers, no rendering.
pub fn watchdog() {
    unsafe {
        if !read_volatile(&raw const PRESENT) || !read_volatile(&raw const STARTED) {
            return;
        }
        let sd = SD;
        let ctl = r32(sd + SD_CTL);
        if ctl & SD_CTL_RUN == 0 {
            return;
        }
        let play = (r32(sd + SD_LPIB) as usize / BUFFER_BYTES) % NUM_BUFFERS;
        let write = read_volatile(&raw const WRITE);
        let queued = (write + NUM_BUFFERS - play) % NUM_BUFFERS;
        if queued == 0 || queued > QUEUE_DEPTH {
            // `past` buffers of silence fetched since the queue ran dry.
            // Wait until the codec-side FIFO (emulators buffer tens of ms
            // past LPIB, and a stop discards it) can only hold silence.
            let past = (play + NUM_BUFFERS - write) % NUM_BUFFERS;
            if past >= STOP_AFTER_SILENT {
                w32(sd + SD_CTL, ctl & !SD_CTL_RUN & 0x00FF_FFFF);
            }
        }
    }
}
