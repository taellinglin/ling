//! AC'97 audio driver (Intel 82801AA "ICH" controller) -- the audio device
//! both QEMU (`-device AC97`) and VirtualBox (default audio controller
//! setting) emulate, making it the same "one device covers both VMs" call
//! as the e1000 was for networking. Same driver idiom as every other
//! device here: free functions, PCI discovery, static preallocated DMA
//! buffers, bounded polls, no interrupts (the WM frame loop / shell pump
//! calls [`pump`] often enough to keep the ring fed -- disclosed pull
//! model, not hidden).
//!
//! Verified the honest way: QEMU records what the guest actually plays
//! into a host-side WAV (`-audiodev wav,...`), and the boot test asserts
//! the jingle's real pentatonic frequencies are present in that file --
//! not "the registers looked right", but "the sound came out".
//!
//! Topology (ICH AC'97): two PCI I/O BARs -- BAR0 = NAM (mixer registers:
//! volumes, sample-rate), BAR1 = NABM (bus-master: per-channel Buffer
//! Descriptor List rings). PCM-out only; no capture, no surround. Fixed
//! 48 kHz 16-bit stereo (AC'97's native rate; the synth renders at 48 kHz
//! so no VRA negotiation is needed).

use crate::arch::{io, pci, timer};

const NAM_RESET: u16 = 0x00;
const NAM_MASTER_VOL: u16 = 0x02;
const NAM_PCM_VOL: u16 = 0x18;

const NABM_PO_BDBAR: u16 = 0x10; // PCM-out buffer-descriptor-list base
const NABM_PO_CIV: u16 = 0x14; // current index (read-only)
const NABM_PO_LVI: u16 = 0x15; // last valid index
const NABM_PO_SR: u16 = 0x16; // status
const NABM_PO_CR: u16 = 0x1B; // control
const NABM_GLOB_CNT: u16 = 0x2C;

const CR_RPBM: u8 = 0x01; // run
const CR_RR: u8 = 0x02; // reset registers
const GLOB_COLD_RESET: u32 = 0x02;

/// 32 descriptors is the BDL's architectural size; we cycle a working set
/// of 16 (~340ms of audio at 48kHz). The mixer refills the ring only once
/// per desktop frame, so a deep ring is what absorbs frame-time jitter --
/// with only 4 buffers (~85ms) any long frame (a seed hitch, a heavy
/// redraw, a network poll) starved the ring and the song went choppy every
/// few seconds. 16 buffers tolerate a ~340ms stall before an underrun.
pub const NUM_BUFFERS: usize = 16;
pub const FRAMES_PER_BUFFER: usize = 1024;
const SAMPLES_PER_BUFFER: usize = FRAMES_PER_BUFFER * 2; // stereo

#[derive(Clone, Copy)]
#[repr(C, align(8))]
struct BufferDescriptor {
    addr: u32,
    // bits 0..15: length in *samples*; bit 30 BUP; bit 31 IOC
    control: u32,
}

#[repr(align(4096))]
struct DmaBuffers([[i16; SAMPLES_PER_BUFFER]; NUM_BUFFERS]);

static mut BDL: [BufferDescriptor; 32] = [BufferDescriptor { addr: 0, control: 0 }; 32];
static mut DMA: DmaBuffers = DmaBuffers([[0; SAMPLES_PER_BUFFER]; NUM_BUFFERS]);

static mut NAM_BASE: u16 = 0;
static mut NABM_BASE: u16 = 0;
static mut PRESENT: bool = false;
static mut NEXT_FILL: usize = 0; // next DMA buffer index we'll write

fn nam_write(reg: u16, val: u16) {
    unsafe { io::outw(NAM_BASE + reg, val) };
}

fn nabm_read8(reg: u16) -> u8 {
    unsafe { io::inb(NABM_BASE + reg) }
}

fn nabm_write8(reg: u16, val: u8) {
    unsafe { io::outb(NABM_BASE + reg, val) };
}

fn nabm_write16(reg: u16, val: u16) {
    unsafe { io::outw(NABM_BASE + reg, val) };
}

fn nabm_write32(reg: u16, val: u32) {
    unsafe { io::outl(NABM_BASE + reg, val) };
}

/// Find and initialize the AC'97 controller. Returns true if a device was
/// found and brought up. Safe to call when absent (VirtualBox configured
/// with no audio, the boot-test's default QEMU line) -- everything else in
/// the audio stack no-ops when this returned false.
pub fn init() -> bool {
    // Idempotent: lsh's `play` and the WM both call this without knowing
    // about each other; a re-init would tear down a ring mid-playback.
    if unsafe { PRESENT } {
        return true;
    }
    // PCI class 0x04 (multimedia), subclass 0x01 (audio), prog-if 0.
    // QEMU's AC97 is 8086:2415; VirtualBox emulates the same ICH model.
    let Some(loc) = pci::find(0x04, 0x01, 0x00) else {
        return false;
    };
    // Raw BAR reads (not `Location::bar`, whose mask is the memory-BAR
    // one): bit 0 set = I/O BAR, which both must be on ICH AC'97.
    let bar0 = loc.read32(0x10);
    let bar1 = loc.read32(0x14);
    if bar0 & 1 == 0 || bar1 & 1 == 0 {
        return false;
    }
    unsafe {
        NAM_BASE = (bar0 & 0xFFFC) as u16;
        NABM_BASE = (bar1 & 0xFFFC) as u16;
    }
    // Command register: ensure I/O space + bus mastering (DMA) are on --
    // SeaBIOS usually leaves them on, but "usually" earned the mouse
    // stack three real bugs this same session, so set them explicitly.
    let cmd = loc.read32(0x04);
    pci::write32(loc.bus, loc.device, loc.function, 0x04, cmd | 0x0005);

    // Cold reset, then codec reset, then a short settle (QEMU's codec is
    // ready immediately; the bounded delay is for eventual real hardware).
    nabm_write32(NABM_GLOB_CNT, GLOB_COLD_RESET);
    nam_write(NAM_RESET, 0);
    timer::delay_us(10_000);

    // Volumes: 0 attenuation on master and PCM-out (0x0000 = loudest,
    // 0x8000 = mute bit).
    nam_write(NAM_MASTER_VOL, 0x0000);
    nam_write(NAM_PCM_VOL, 0x0808);

    // Reset the PCM-out channel and install the BDL: NUM_BUFFERS entries
    // cycling over the static DMA buffers, each with IOC clear (we poll
    // CIV instead of taking interrupts).
    nabm_write8(NABM_PO_CR, CR_RR);
    let mut spin = 0;
    while nabm_read8(NABM_PO_CR) & CR_RR != 0 && spin < 1000 {
        spin += 1;
    }
    unsafe {
        for i in 0..32 {
            let buf_idx = i % NUM_BUFFERS;
            BDL[i] = BufferDescriptor {
                addr: DMA.0[buf_idx].as_ptr() as u32,
                control: SAMPLES_PER_BUFFER as u32,
            };
        }
        nabm_write32(NABM_PO_BDBAR, (&raw const BDL) as u32);
        PRESENT = true;
        NEXT_FILL = 0;
    }
    true
}

pub fn present() -> bool {
    unsafe { PRESENT }
}

/// How many DMA buffers are free to fill right now (the gap between the
/// hardware's current index and our fill cursor, over the NUM_BUFFERS
/// working set).
fn buffers_free() -> usize {
    let civ = (nabm_read8(NABM_PO_CIV) as usize) % NUM_BUFFERS;
    unsafe {
        let filled = (NEXT_FILL + NUM_BUFFERS - civ) % NUM_BUFFERS;
        NUM_BUFFERS - 1 - filled
    }
}

/// Feed the PCM-out ring: calls `render` once per free DMA buffer to
/// produce the next FRAMES_PER_BUFFER stereo frames, advances LVI, and
/// (re)starts the channel. The mixer is the only caller. Returns how many
/// buffers were filled (0 = ring full, nothing to do -- the cheap case a
/// per-frame caller hits most of the time).
pub fn pump(render: &mut dyn FnMut(&mut [i16])) -> usize {
    if !present() {
        return 0;
    }
    let mut filled = 0;
    while buffers_free() > 0 {
        unsafe {
            let idx = NEXT_FILL;
            render(&mut DMA.0[idx]);
            NEXT_FILL = (NEXT_FILL + 1) % NUM_BUFFERS;
            // LVI is a 32-entry ring index; keep it one buffer ahead of
            // where we just wrote, mapped onto the 32-entry BDL.
            let lvi = ((nabm_read8(NABM_PO_CIV) as usize + filled + 1) % 32) as u8;
            nabm_write16(NABM_PO_SR, 0x1C); // clear status bits
            nabm_write8(NABM_PO_LVI, lvi);
        }
        filled += 1;
    }
    if filled > 0 && nabm_read8(NABM_PO_CR) & CR_RPBM == 0 {
        nabm_write8(NABM_PO_CR, CR_RPBM);
    }
    filled
}
