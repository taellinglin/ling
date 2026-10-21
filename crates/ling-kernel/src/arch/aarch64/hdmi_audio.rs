//! Raspberry Pi 5 HDMI audio: the HDMI0 MAI (the audio FIFO inside the HDMI
//! block) fed by a cyclic DMA of IEC958 subframes rendered from the kernel's
//! audio source (LingStationX projects, via `lingdaw::block`).
//!
//! Ported from Linux's vc4_hdmi audio path for the BCM2712 register layout
//! ("vc6" hdmi0 tables in vc4_hdmi_regs.h, bit fields from vc4_regs.h), the
//! bcm2711-dvp clock gate for the MAI's 108 MHz, and the bcm2835-dma legacy
//! channel format with the BCM2712 quirks (control-block addresses >> 5, the
//! STRIDE word carrying the upper address bits). The firmware already drives
//! video on this HDMI, so the PHY, pixel clock and packet scheduler are up; we
//! add the audio clock, MAI configuration, N/CTS, the audio infoframe and the
//! data stream. Untested on hardware until the first photo/listen: `test()`
//! prints every register it touches and plays a 2 s 440 Hz tone.

use crate::arch::mmio::{read32, write32};
use crate::arch::timer::{delay_us, now_ms};
use crate::console_write as out;

// BCM2712 HDMI0 register blocks (bcm2712.dtsi, soc bus at CPU 0x10_7c00_0000).
const HDMI: usize = 0x10_7c70_1400; // "hdmi": packet/scheduler/CRP/MAI config
const HD: usize = 0x10_7c72_0000; // "hd": the MAI FIFO
const RAM: usize = 0x10_7c70_3800; // "packet": infoframe RAM
const DVP: usize = 0x10_7c70_0000; // DVP: HDMI clock gates + resets
const DMA: usize = 0x10_0001_0000; // dma32 (legacy-format channels 0-5)
const DMA_CHAN: usize = 0; // channel mask 0x35: 0, 2, 4, 5 are the ARM's
const DREQ_HDMI0_MAI: u32 = 10; // hdmi0 `dmas = <&dma32 10>`

// "hdmi" block (vc6 hdmi0 field table).
const MAI_CHANNEL_MAP: usize = 0x0a4;
const MAI_CONFIG: usize = 0x0a8;
const AUDIO_PACKET_CONFIG: usize = 0x0c0;
const RAM_PACKET_CONFIG: usize = 0x0c4;
const RAM_PACKET_STATUS: usize = 0x0cc;
const CRP_CFG: usize = 0x0d0;
const CTS_0: usize = 0x0d4;
const CTS_1: usize = 0x0d8;
const SCHEDULER_CONTROL: usize = 0x0e8;
const HOTPLUG: usize = 0x1c8;
// "hd" block.
const MAI_CTL: usize = 0x10;
const MAI_THR: usize = 0x14;
const MAI_FMT: usize = 0x18;
const MAI_DATA: usize = 0x1c;
const MAI_SMP: usize = 0x20;

const MAI_CTL_RESET: u32 = 1 << 0;
const MAI_CTL_ERRORF: u32 = 1 << 1;
const MAI_CTL_ERRORE: u32 = 1 << 2;
const MAI_CTL_ENABLE: u32 = 1 << 3;
const MAI_CTL_FLUSH: u32 = 1 << 9;
const MAI_CTL_EMPTY: u32 = 1 << 10;
const MAI_CTL_FULL: u32 = 1 << 11;
const MAI_CTL_WHOLSMP: u32 = 1 << 12;
const MAI_CTL_CHALIGN: u32 = 1 << 13;
const MAI_CTL_BUSY: u32 = 1 << 14;
const MAI_CTL_DLATE: u32 = 1 << 15;
const RAM_PACKET_ENABLE: u32 = 1 << 16;
const AUDIO_INFOFRAME_SLOT: u32 = 4; // packet type 0x84 - 0x80
const PACKET_STRIDE: usize = 0x24;

// Legacy DMA channel registers + bits (bcm2835-dma.c).
const DMA_CS: usize = 0x00;
const DMA_ADDR: usize = 0x04;
const DMA_DEBUG: usize = 0x20;
const CS_ACTIVE: u32 = 1 << 0;
const CS_ERROR: u32 = 1 << 8;
const CS_RESET: u32 = 1 << 31;
const TI_WAIT_RESP: u32 = 1 << 3;
const TI_D_DREQ: u32 = 1 << 6;
const TI_S_INC: u32 = 1 << 8;

const RATE: u32 = 48_000;
const BLOCK_FRAMES: usize = 1024;
const BLOCKS: usize = 8; // 8 x 21 ms of audio in flight
const BLOCK_BYTES: usize = BLOCK_FRAMES * 8; // stereo, 4-byte subframes
const RING_PAGES: usize = BLOCKS * BLOCK_BYTES / 4096;

struct State {
    ring: u64,
    cbs: u64,
    next_fill: usize,
    /// Running frame counter (the IEC958 block position = frame % 192).
    frame: u32,
    /// Play a 440 Hz test tone until this time (0 = play the project).
    tone_until_ms: u64,
    tone_phase: f32,
}

static mut ST: Option<State> = None;

fn hex(label: &[u8], v: u32) {
    out(label);
    out(b"0x");
    let mut b = [0u8; 8];
    for (i, c) in b.iter_mut().enumerate() {
        let d = ((v >> ((7 - i) * 4)) & 0xF) as u8;
        *c = if d < 10 { b'0' + d } else { b'a' + d - 10 };
    }
    out(&b);
    out(b"\n");
}

fn hdmi_rd(off: usize) -> u32 {
    unsafe { read32(HDMI + off) }
}
fn hdmi_wr(off: usize, v: u32) {
    unsafe { write32(HDMI + off, v) }
}
fn hd_rd(off: usize) -> u32 {
    unsafe { read32(HD + off) }
}
fn hd_wr(off: usize, v: u32) {
    unsafe { write32(HD + off, v) }
}
fn dma_rd(off: usize) -> u32 {
    unsafe { read32(DMA + DMA_CHAN * 0x100 + off) }
}
fn dma_wr(off: usize, v: u32) {
    unsafe { write32(DMA + DMA_CHAN * 0x100 + off, v) }
}

/// IEC958 consumer channel status: PCM, copyright permitted, 48 kHz, 24-bit.
const CHANNEL_STATUS: [u8; 5] = [0x04, 0x00, 0x00, 0x02, 0x0B];

/// One IEC958 subframe in ALSA's SUBFRAME_LE layout (what the MAI takes with
/// BIT_REVERSE | FORMAT_REVERSE): preamble in bits 0-3 (B for the first left
/// subframe of a 192-frame block, else M left / W right), the 24-bit sample in
/// bits 4-27, validity 0, channel status in bit 30, even parity in bit 31.
fn subframe(sample: i16, frame: u32, left: bool) -> u32 {
    let f = frame % 192;
    let pre = if left && f == 0 {
        0x08
    } else if left {
        0x02
    } else {
        0x04
    };
    let s24 = (((sample as i32) << 8) as u32) & 0x00FF_FFFF;
    let c = if (f as usize) < CHANNEL_STATUS.len() * 8 { ((CHANNEL_STATUS[f as usize / 8] >> (f % 8)) & 1) as u32 } else { 0 };
    let mut w = pre | (s24 << 4) | (c << 30);
    if (w & 0x7FFF_FFF0).count_ones() & 1 == 1 {
        w |= 1 << 31;
    }
    w
}

/// Render one ring block: the project's audio, a test tone, or silence.
fn fill_block(st: &mut State, idx: usize) {
    let base = st.ring + (idx * BLOCK_BYTES) as u64;
    let now = now_ms();
    let src: Option<&[i16]> = if st.tone_until_ms > now { None } else { crate::lingdaw::block(BLOCK_FRAMES) };
    for i in 0..BLOCK_FRAMES {
        let (l, r) = if st.tone_until_ms > now {
            st.tone_phase += 440.0 / RATE as f32;
            if st.tone_phase >= 1.0 {
                st.tone_phase -= 1.0;
            }
            let v = (libm::sinf(st.tone_phase * core::f32::consts::TAU) * 9000.0) as i16;
            (v, v)
        } else {
            match src {
                Some(s) if i * 2 + 1 < s.len() => (s[i * 2], s[i * 2 + 1]),
                _ => (0, 0),
            }
        };
        let f = st.frame.wrapping_add(i as u32);
        unsafe {
            core::ptr::write_volatile((base + (i * 8) as u64) as *mut u32, subframe(l, f, true));
            core::ptr::write_volatile((base + (i * 8 + 4) as u64) as *mut u32, subframe(r, f, false));
        }
    }
    st.frame = st.frame.wrapping_add(BLOCK_FRAMES as u32);
}

/// Which ring block the DMA is playing now (from its current control block).
fn playing_block(st: &State) -> Option<usize> {
    let cb = (dma_rd(DMA_ADDR) as u64) << 5;
    if cb < st.cbs || cb >= st.cbs + (BLOCKS * 32) as u64 {
        return None;
    }
    Some(((cb - st.cbs) / 32) as usize)
}

/// Refill the ring blocks the DMA has finished with. Call every frame.
pub fn pump() {
    let Some(st) = (unsafe { (*(&raw mut ST)).as_mut() }) else { return };
    let Some(cur) = playing_block(st) else { return };
    let mut guard = 0;
    while st.next_fill != cur && guard < BLOCKS {
        let i = st.next_fill;
        fill_block(st, i);
        st.next_fill = (st.next_fill + 1) % BLOCKS;
        guard += 1;
    }
}

/// True once the MAI + DMA are streaming.
pub fn running() -> bool {
    unsafe { (*(&raw const ST)).is_some() }
}

fn write_audio_infoframe(verbose: bool) -> bool {
    // HDMI audio infoframe: type 0x84, version 1, length 10; PB1 = 2 channels
    // (CC = 1), everything else "refer to stream header" / FL+FR.
    let mut pkt = [0u8; 14];
    pkt[0] = 0x84;
    pkt[1] = 0x01;
    pkt[2] = 0x0A;
    pkt[4] = 0x01;
    let sum: u32 = pkt.iter().map(|&b| b as u32).sum();
    pkt[3] = ((256 - (sum % 256)) % 256) as u8;

    let bit = 1u32 << AUDIO_INFOFRAME_SLOT;
    hdmi_wr(RAM_PACKET_CONFIG, hdmi_rd(RAM_PACKET_CONFIG) & !bit);
    for _ in 0..1000 {
        if hdmi_rd(RAM_PACKET_STATUS) & bit == 0 {
            break;
        }
        delay_us(100);
    }
    let slot = RAM + AUDIO_INFOFRAME_SLOT as usize * PACKET_STRIDE;
    let mut off = 0usize;
    let mut i = 0usize;
    while i < pkt.len() {
        let b = |k: usize| pkt.get(i + k).copied().unwrap_or(0) as u32;
        unsafe {
            write32(slot + off, b(0) | (b(1) << 8) | (b(2) << 16));
            write32(slot + off + 4, b(3) | (b(4) << 8) | (b(5) << 16) | (b(6) << 24));
        }
        off += 8;
        i += 7;
    }
    while off < PACKET_STRIDE {
        unsafe { write32(slot + off, 0) };
        off += 4;
    }
    hdmi_wr(RAM_PACKET_CONFIG, hdmi_rd(RAM_PACKET_CONFIG) | RAM_PACKET_ENABLE | bit);
    let mut ok = false;
    for _ in 0..1000 {
        if hdmi_rd(RAM_PACKET_STATUS) & bit != 0 {
            ok = true;
            break;
        }
        delay_us(100);
    }
    if verbose {
        hex(b"  RAM_PACKET_CONFIG=", hdmi_rd(RAM_PACKET_CONFIG));
        hex(b"  RAM_PACKET_STATUS=", hdmi_rd(RAM_PACKET_STATUS));
        out(if ok { b"  audio infoframe scheduled\n" } else { b"  audio infoframe NOT acknowledged by the packet scheduler\n" });
    }
    ok
}

/// Bring HDMI audio up (idempotent). Returns whether the stream is running.
pub fn init(verbose: bool) -> bool {
    if running() {
        if verbose {
            out(b"HDMI audio: already running\n");
        }
        return true;
    }
    if crate::arch::mmio::board() != crate::arch::mmio::RpiBoard::Pi5 {
        if verbose {
            out(b"HDMI audio: this driver is for the Raspberry Pi 5 (BCM2712 HDMI0)\n");
        }
        return false;
    }
    out(b"HDMI audio (Pi 5, HDMI0 MAI + DMA):\n");
    let hp = hdmi_rd(HOTPLUG);
    let sched = hdmi_rd(SCHEDULER_CONTROL);
    if verbose {
        hex(b"  HOTPLUG=", hp);
        hex(b"  SCHEDULER_CONTROL=", sched);
        hex(b"  RAM_PACKET_CONFIG=", hdmi_rd(RAM_PACKET_CONFIG));
        hex(b"  MAI_CTL before=", hd_rd(MAI_CTL));
    }
    if sched & 0x1 == 0 {
        out(b"  the display link is in DVI mode (no audio packets); config.txt hdmi_drive=2 forces HDMI\n");
    }

    // 1. The MAI's 108 MHz: DVP MISC_CONFIG bit 3 is the hdmi0 gate, SET = disabled.
    let misc = unsafe { read32(DVP + 0x08) };
    if misc & (1 << 3) != 0 {
        unsafe { write32(DVP + 0x08, misc & !(1 << 3)) };
    }
    if verbose {
        hex(b"  DVP MISC_CONFIG was ", misc);
        hex(b"  DVP MISC_CONFIG now ", unsafe { read32(DVP + 0x08) });
    }

    // 2. Reset + flush the MAI FIFO (vc4_hdmi_audio_startup).
    hd_wr(MAI_CTL, MAI_CTL_RESET | MAI_CTL_FLUSH | MAI_CTL_DLATE | MAI_CTL_ERRORE | MAI_CTL_ERRORF);

    // 3. Clock regeneration: N = 128 * rate / 1000; CTS = pixel clock in kHz
    //    (the display's preferred mode from EDID; 1080p60 if unknown).
    let n = 128 * RATE / 1000;
    let cts = crate::arch::mailbox::edid_pixel_clock_khz().unwrap_or(148_500);
    hdmi_wr(CRP_CFG, (1 << 24) | n);
    hdmi_wr(CTS_0, cts);
    hdmi_wr(CTS_1, cts);
    // 4. MAI sample clock from the 108 MHz: N/M = 108e6 / 48e3 = 2250 / 1.
    hd_wr(MAI_SMP, (2250 << 8) | 0);
    // 5. MAI on: 2 channels, whole samples, channel-aligned.
    hd_wr(MAI_CTL, (2 << 4) | MAI_CTL_WHOLSMP | MAI_CTL_CHALIGN | MAI_CTL_ENABLE);
    // 6. Format: PCM (2) at 48 kHz (9).
    hd_wr(MAI_FMT, (2 << 16) | (9 << 8));
    // 7. Audio packets: zero on flat/inactive, B-frame id 8, CEA channels 0-1.
    hdmi_wr(AUDIO_PACKET_CONFIG, (1 << 29) | (1 << 24) | (8 << 10) | 0x3);
    // 8. FIFO thresholds (BCM2712 C-step field layout).
    hd_wr(MAI_THR, (0x10 << 24) | (0x10 << 16) | (0x1c << 8) | 0x1c);
    // 9. Bit/format reversed IEC958 subframes, channel mask 0-1; map ch i -> slot i.
    hdmi_wr(MAI_CONFIG, (1 << 26) | (1 << 27) | 0x3);
    hdmi_wr(MAI_CHANNEL_MAP, 0x10);
    if verbose {
        hex(b"  CRP_CFG=", hdmi_rd(CRP_CFG));
        hex(b"  CTS=", hdmi_rd(CTS_0));
        hex(b"  MAI_SMP=", hd_rd(MAI_SMP));
        hex(b"  MAI_CTL=", hd_rd(MAI_CTL));
        hex(b"  MAI_FMT=", hd_rd(MAI_FMT));
        hex(b"  MAI_THR=", hd_rd(MAI_THR));
        hex(b"  MAI_CONFIG=", hdmi_rd(MAI_CONFIG));
    }
    // 10. Tell the display what's coming.
    write_audio_infoframe(verbose);

    // 11. The data path: a ring of IEC958 subframes streamed to MAI_DATA by a
    //     cyclic DMA paced by the MAI's DREQ.
    let Some(ring) = crate::arch::paging::dma_alloc_pages(RING_PAGES) else {
        out(b"  no DMA memory for the audio ring\n");
        return false;
    };
    let Some(cbs) = crate::arch::paging::dma_alloc_page() else {
        out(b"  no DMA memory for the control blocks\n");
        return false;
    };
    let mut st = State { ring, cbs, next_fill: 0, frame: 0, tone_until_ms: 0, tone_phase: 0.0 };
    for i in 0..BLOCKS {
        fill_block(&mut st, i); // valid (silent) frames before the DMA starts
    }
    let mai_data = (HD + MAI_DATA) as u64;
    let ti = TI_D_DREQ | TI_S_INC | TI_WAIT_RESP | ((DREQ_HDMI0_MAI & 31) << 16);
    for i in 0..BLOCKS {
        let cb = cbs + (i * 32) as u64;
        let src = ring + (i * BLOCK_BYTES) as u64;
        let next = cbs + (((i + 1) % BLOCKS) * 32) as u64;
        unsafe {
            write32(cb as usize, ti);
            write32(cb as usize + 4, src as u32);
            write32(cb as usize + 8, mai_data as u32);
            write32(cb as usize + 12, BLOCK_BYTES as u32);
            // BCM2712: the STRIDE word carries the upper address bits.
            write32(cb as usize + 16, (((mai_data >> 32) as u32) << 8) | (src >> 32) as u32);
            write32(cb as usize + 20, (next >> 5) as u32);
            write32(cb as usize + 24, 0);
            write32(cb as usize + 28, 0);
        }
    }
    unsafe { core::arch::asm!("dsb sy", options(nostack)) };
    dma_wr(DMA_CS, CS_RESET);
    delay_us(1_000);
    dma_wr(DMA_ADDR, (cbs >> 5) as u32);
    dma_wr(DMA_CS, CS_ACTIVE);
    unsafe { core::ptr::write(&raw mut ST, Some(st)) };
    delay_us(60_000);
    let cs = dma_rd(DMA_CS);
    let dbg = dma_rd(DMA_DEBUG);
    let ctl = hd_rd(MAI_CTL);
    if verbose {
        hex(b"  DMA CS=", cs);
        hex(b"  DMA DEBUG=", dbg);
        hex(b"  DMA current CB=", dma_rd(DMA_ADDR));
        hex(b"  MAI_CTL now=", ctl);
    }
    let flowing = cs & CS_ACTIVE != 0 && cs & CS_ERROR == 0 && dbg & 0x7 == 0;
    out(if flowing { b"  -> audio stream running\n" } else { b"  -> DMA did not run cleanly (see CS/DEBUG)\n" });
    if ctl & MAI_CTL_EMPTY != 0 && flowing {
        out(b"  (MAI FIFO empty: DREQ pacing or MAI clock not reaching the FIFO)\n");
    }
    let _ = (MAI_CTL_FULL, MAI_CTL_BUSY);
    flowing
}

/// Console `hdmiaudio`: verbose bring-up, then a 2 s 440 Hz test tone.
pub fn test() {
    if !init(true) {
        return;
    }
    let Some(st) = (unsafe { (*(&raw mut ST)).as_mut() }) else { return };
    st.tone_until_ms = now_ms() + 2000;
    out(b"  playing a 2 s 440 Hz tone...\n");
    let end = now_ms() + 2300;
    while now_ms() < end {
        pump();
        delay_us(2_000);
    }
    hex(b"  after tone: DMA CS=", dma_rd(DMA_CS));
    hex(b"  after tone: MAI_CTL=", hd_rd(MAI_CTL));
    out(b"  (heard it? then LingStationX projects play through HDMI)\n");
}
