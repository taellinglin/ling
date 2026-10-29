//! AC'97 audio driver (Intel 82801AA "ICH" controller) -- the audio device
//! both QEMU (`-device AC97`) and VirtualBox (the "ICH AC97" audio
//! controller setting) emulate. Same driver idiom as every other device
//! here: free functions, PCI discovery, static preallocated DMA buffers,
//! bounded polls, no interrupts (the WM frame loop / shell pump calls
//! [`pump`] often enough to keep the ring fed -- disclosed pull model, not
//! hidden). New VirtualBox VMs default to Intel HD Audio instead; that is
//! `hda.rs`, and `mixer.rs` picks whichever controller exists.
//!
//! Verified the honest way: QEMU records what the guest actually plays
//! into a host-side WAV (`-audiodev wav,...`), and the boot test asserts
//! the jingle's real pentatonic frequencies are present in that file --
//! not "the registers looked right", but "the sound came out".
//!
//! Topology (ICH AC'97): two PCI I/O BARs -- BAR0 = NAM (mixer registers:
//! volumes, sample-rate), BAR1 = NABM (bus-master: per-channel Buffer
//! Descriptor List rings). PCM-out only; no capture, no surround. 48 kHz
//! 16-bit stereo; when the codec has VRA the front DAC rate is set to
//! 48000 explicitly rather than trusting the reset default.
//!
//! Ring model: the 32-entry BDL maps 1:1 onto 32 distinct DMA buffers, and
//! the fill cursor lives in BDL-index space. LVI always names the most
//! recently written buffer, so the controller can never run past real data
//! into a stale buffer: if the pump is late it halts at LVI (DCH) and the
//! next pump queues more data and restarts it. (The old driver set LVI to
//! `CIV + buffers-filled-this-call + 1`, which ignored data queued by
//! earlier calls -- LVI landed *behind* real data, the channel halted
//! mid-stream and replayed stale buffers: audible stutter on VirtualBox.)

use crate::arch::{io, pci, timer};

const NAM_RESET: u16 = 0x00;
const NAM_MASTER_VOL: u16 = 0x02;
const NAM_PCM_VOL: u16 = 0x18;
const NAM_EXT_AUDIO_ID: u16 = 0x28; // bit 0: VRA supported
const NAM_EXT_AUDIO_CTRL: u16 = 0x2A; // bit 0: VRA enable
const NAM_FRONT_DAC_RATE: u16 = 0x2C;

const NABM_PO_BDBAR: u16 = 0x10; // PCM-out buffer-descriptor-list base
const NABM_PO_CIV: u16 = 0x14; // current index (read-only)
const NABM_PO_LVI: u16 = 0x15; // last valid index
const NABM_PO_SR: u16 = 0x16; // status
const NABM_PO_CR: u16 = 0x1B; // control
const NABM_GLOB_CNT: u16 = 0x2C;
const NABM_GLOB_STA: u16 = 0x30;

const CR_RPBM: u8 = 0x01; // run
const CR_RR: u8 = 0x02; // reset registers
const SR_DCH: u16 = 0x01; // DMA controller halted
/// LVBCI | BCIS | FIFOE: the write-1-to-clear status bits (DCH and CELV are
/// read-only).
const SR_CLEAR: u16 = 0x1C;
/// GLOB_CNT bit 1 is "AC'97 Cold Reset#", active LOW: 0 holds the link in
/// cold reset, 1 releases it.
const GLOB_CNT_COLD_RESET_N: u32 = 0x02;
const GLOB_STA_PCR: u32 = 1 << 8; // primary codec ready

/// The BDL's architectural size; each entry owns its own DMA buffer.
pub const NUM_BUFFERS: usize = 32;
pub const FRAMES_PER_BUFFER: usize = 1024;
const SAMPLES_PER_BUFFER: usize = FRAMES_PER_BUFFER * 2; // stereo
/// How many buffers we keep queued ahead of the one playing (~340ms at
/// 48kHz). The mixer refills the ring only once per desktop frame, so a
/// deep queue absorbs frame-time jitter (a seed hitch, a heavy redraw, a
/// network poll); deeper than this only adds latency to UI sounds.
pub const QUEUE_DEPTH: usize = 16;

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
/// BDL index of the next buffer we'll write (LVI = WRITE - 1 once started).
static mut WRITE: usize = 0;
/// False until the first pump queued data and set RPBM.
static mut STARTED: bool = false;

fn nam_read(reg: u16) -> u16 {
    unsafe { io::inw(NAM_BASE + reg) }
}

fn nam_write(reg: u16, val: u16) {
    unsafe { io::outw(NAM_BASE + reg, val) };
}

fn nabm_read8(reg: u16) -> u8 {
    unsafe { io::inb(NABM_BASE + reg) }
}

fn nabm_read16(reg: u16) -> u16 {
    unsafe { io::inw(NABM_BASE + reg) }
}

fn nabm_read32(reg: u16) -> u32 {
    unsafe { io::inl(NABM_BASE + reg) }
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

/// Bounded spin (no cooperative yield: init must not let another task
/// re-enter the audio stack half-way through bring-up).
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

/// Find and initialize the AC'97 controller. Returns true if a device was
/// found and brought up. Safe to call when absent (VirtualBox configured
/// with no audio / HD Audio, the boot-test's default QEMU line) -- the
/// mixer then tries HDA, and everything else no-ops if neither exists.
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
    // Command register: ensure I/O space + bus mastering (DMA) are on.
    let cmd = loc.read32(0x04);
    pci::write32(loc.bus, loc.device, loc.function, 0x04, cmd | 0x0005);

    // Cold reset: assert Cold Reset# (bit 1 = 0), hold it briefly, release
    // it (bit 1 = 1), then wait -- bounded -- for the primary codec to
    // report ready. Proceed anyway on timeout: some emulations never set
    // PCR, and the codec registers answer regardless.
    nabm_write32(NABM_GLOB_CNT, 0);
    timer::delay_us(100);
    nabm_write32(NABM_GLOB_CNT, GLOB_CNT_COLD_RESET_N);
    wait_us(200_000, || nabm_read32(NABM_GLOB_STA) & GLOB_STA_PCR != 0);
    // Codec register reset (any write to NAM 0x00).
    nam_write(NAM_RESET, 0);
    timer::delay_us(1_000);

    // Volumes: 0 attenuation on master (0x0000 = loudest, 0x8000 = mute),
    // PCM-out at 0x0808 (a little headroom against clipping).
    nam_write(NAM_MASTER_VOL, 0x0000);
    nam_write(NAM_PCM_VOL, 0x0808);

    // Variable Rate Audio: if the codec has it, enable it and pin the front
    // DAC to 48 kHz (the mixer's rate). Without VRA the codec is fixed at
    // 48 kHz anyway.
    if nam_read(NAM_EXT_AUDIO_ID) & 1 != 0 {
        nam_write(NAM_EXT_AUDIO_CTRL, nam_read(NAM_EXT_AUDIO_CTRL) | 1);
        nam_write(NAM_FRONT_DAC_RATE, 48_000);
    }

    // Reset the PCM-out channel and install the BDL: 32 entries, one DMA
    // buffer each, IOC clear (we poll CIV instead of taking interrupts).
    nabm_write8(NABM_PO_CR, 0);
    nabm_write8(NABM_PO_CR, CR_RR);
    wait_us(10_000, || nabm_read8(NABM_PO_CR) & CR_RR == 0);
    unsafe {
        let dma = &mut *&raw mut DMA;
        let bdl = &mut *&raw mut BDL;
        for i in 0..32 {
            dma.0[i].fill(0);
            bdl[i] = BufferDescriptor {
                addr: dma.0[i].as_ptr() as u32,
                control: SAMPLES_PER_BUFFER as u32,
            };
        }
        nabm_write32(NABM_PO_BDBAR, (&raw const BDL) as u32);
        nabm_write16(NABM_PO_SR, SR_CLEAR);
        WRITE = 0;
        STARTED = false;
        PRESENT = true;
    }
    true
}

pub fn present() -> bool {
    unsafe { PRESENT }
}

/// Feed the PCM-out ring: calls `render` once per buffer that may be
/// queued right now, each producing the next FRAMES_PER_BUFFER stereo
/// frames, keeps LVI on the last written buffer and (re)starts the channel
/// if it is idle or halted. The mixer is the only caller. Returns how many
/// buffers were filled (0 = queue full, nothing to do -- the cheap case a
/// per-frame caller hits most of the time).
pub fn pump(render: &mut dyn FnMut(&mut [i16])) -> usize {
    if !present() {
        return 0;
    }
    let halted = nabm_read16(NABM_PO_SR) & SR_DCH != 0;
    let civ = (nabm_read8(NABM_PO_CIV) & 31) as usize;
    let mut write = unsafe { WRITE };
    // Buffers owned by the hardware: CIV (playing) up to WRITE-1 (= LVI).
    // Once halted, CIV's buffer has been played too.
    let mut queued = if unsafe { STARTED } {
        let q = (write + 32 - civ) % 32;
        if halted { q.saturating_sub(1) } else { q }
    } else {
        0
    };
    if queued > QUEUE_DEPTH {
        // Can't happen with LVI kept honest, but never let the cursor
        // trail the hardware: resynchronize just past the playing buffer.
        write = (civ + 1) % 32;
        queued = 0;
    }
    let mut filled = 0;
    unsafe {
        let dma = &mut *&raw mut DMA;
        while queued < QUEUE_DEPTH {
            // `queued < QUEUE_DEPTH < 32` guarantees `write != civ` while
            // the channel runs: the buffer being played is never touched.
            render(&mut dma.0[write]);
            nabm_write8(NABM_PO_LVI, write as u8);
            write = (write + 1) % 32;
            queued += 1;
            filled += 1;
        }
        WRITE = write;
    }
    nabm_write16(NABM_PO_SR, SR_CLEAR);
    if filled > 0 {
        if nabm_read8(NABM_PO_CR) & CR_RPBM == 0 {
            nabm_write8(NABM_PO_CR, CR_RPBM);
            unsafe { STARTED = true };
        } else if nabm_read16(NABM_PO_SR) & SR_DCH != 0 {
            // Still halted after the LVI update (an emulation that doesn't
            // auto-resume on a new LVI): bounce RPBM to restart; the
            // controller resumes at the buffer after the one it stopped on.
            nabm_write8(NABM_PO_CR, 0);
            nabm_write8(NABM_PO_CR, CR_RPBM);
        }
    }
    filled
}
