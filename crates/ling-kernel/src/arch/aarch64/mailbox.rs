//! VideoCore mailbox property-interface channel. The register block moves per
//! board (BCM283x: peripheral_base + 0xB880; BCM2712/Pi 5: + 0x13880), so the
//! addresses are resolved at call time from the detected board, not `const`.

use crate::arch::mmio::{board, mailbox_offset, peripheral_base, read32, write32, RpiBoard};

/// Mailbox register block base for the detected board.
#[inline]
fn mbox_base() -> usize {
    peripheral_base() + mailbox_offset()
}

const MBOX_FULL: u32 = 1 << 31;
const MBOX_EMPTY: u32 = 1 << 30;
const CHANNEL_PROPERTY: u32 = 8;

const TAG_GET_ARM_MEMORY: u32 = 0x0001_0005;
const TAG_SET_PHYS_WH: u32 = 0x0004_8003;
const TAG_SET_VIRT_WH: u32 = 0x0004_8004;
const TAG_SET_DEPTH: u32 = 0x0004_8005;
const TAG_SET_PIXEL_ORDER: u32 = 0x0004_8006;
const TAG_ALLOCATE_BUFFER: u32 = 0x0004_0001;
const TAG_GET_PITCH: u32 = 0x0004_0008;
const TAG_GET_PHYS_WH: u32 = 0x0004_0003;
const TAG_GET_VIRT_WH: u32 = 0x0004_0004;
const TAG_GET_EDID: u32 = 0x0003_0020;
const TAG_END: u32 = 0;

/// Pixel channel order: 0 = BGR, 1 = RGB. Our `framebuffer::set_pixel` writes a
/// `0xRRGGBB` u32 little-endian (bytes B,G,R in memory), so the panel must read
/// byte 0 as blue — i.e. BGR — for colors to come out right.
const PIXEL_ORDER_BGR: u32 = 0;

#[repr(align(16))]
struct MailboxBuffer([u32; 48]);
static mut BUFFER: MailboxBuffer = MailboxBuffer([0; 48]);

/// Raw framebuffer geometry from the last `allocate_framebuffer`, for the
/// on-screen diagnostic: [phys_w, phys_h, virt_w, virt_h, reported_pitch, size_bytes].
pub static mut FB_DIAG: [u32; 6] = [0; 6];

const MAILBOX_TIMEOUT_SPINS: u32 = 10_000_000;

unsafe fn call(channel: u32) -> bool {
    let base = mbox_base();
    let mbox_read = base + 0x00;
    let mbox_status = base + 0x18;
    let mbox_write = base + 0x20;

    let addr = (&raw mut BUFFER) as u32;
    let msg = (addr & !0xF) | (channel & 0xF);
    // The buffer is in cacheable RAM once the MMU is on: push our request out to
    // memory for the VideoCore, and drop stale lines again before reading its reply.
    let buf_len = core::mem::size_of::<MailboxBuffer>() as u64;
    crate::arch::paging::dcache_flush(addr as u64, buf_len);

    let mut spins = 0u32;
    while read32(mbox_status) & MBOX_FULL != 0 {
        core::hint::spin_loop();
        spins += 1;
        if spins > MAILBOX_TIMEOUT_SPINS {
            return false;
        }
    }
    write32(mbox_write, msg);

    spins = 0;
    loop {
        while read32(mbox_status) & MBOX_EMPTY != 0 {
            core::hint::spin_loop();
            spins += 1;
            if spins > MAILBOX_TIMEOUT_SPINS {
                return false;
            }
        }
        let resp = read32(mbox_read);
        if resp == msg {
            crate::arch::paging::dcache_flush(addr as u64, buf_len);
            return BUFFER.0[1] == 0x8000_0000;
        }
    }
}

/// The board revision code (mailbox tag 0x00010002), e.g. 0xd04170 for a Pi 5
/// 8GB rev 1.0. The low nibble is the PCB revision: Pi 5 rev 1.0 boards carry the
/// BCM2712 C1 stepping, rev 1.1+ the D0 stepping (different pin-mux tables).
pub fn board_revision() -> Option<u32> {
    unsafe {
        BUFFER.0[0] = 7 * 4;
        BUFFER.0[1] = 0;
        BUFFER.0[2] = 0x0001_0002;
        BUFFER.0[3] = 4;
        BUFFER.0[4] = 0;
        BUFFER.0[5] = 0;
        BUFFER.0[6] = TAG_END;
        if call(CHANNEL_PROPERTY) {
            Some(BUFFER.0[5])
        } else {
            None
        }
    }
}

pub fn arm_memory() -> Option<(u64, u64)> {
    unsafe {
        BUFFER.0[0] = 8 * 4;
        BUFFER.0[1] = 0;
        BUFFER.0[2] = TAG_GET_ARM_MEMORY;
        BUFFER.0[3] = 8;
        BUFFER.0[4] = 0;
        BUFFER.0[5] = 0;
        BUFFER.0[6] = 0;
        BUFFER.0[7] = TAG_END;

        if call(CHANNEL_PROPERTY) {
            Some((BUFFER.0[5] as u64, BUFFER.0[6] as u64))
        } else {
            None
        }
    }
}

pub fn allocate_framebuffer(w: u32, h: u32, depth: u32) -> Option<(u64, u32, u32, u32, u8)> {
    unsafe {
        BUFFER.0[0] = 40 * 4;
        BUFFER.0[1] = 0; // request

        BUFFER.0[2] = TAG_SET_PHYS_WH;
        BUFFER.0[3] = 8;
        BUFFER.0[4] = 8;
        BUFFER.0[5] = w;
        BUFFER.0[6] = h;

        BUFFER.0[7] = TAG_SET_VIRT_WH;
        BUFFER.0[8] = 8;
        BUFFER.0[9] = 8;
        BUFFER.0[10] = w;
        BUFFER.0[11] = h;

        BUFFER.0[12] = TAG_SET_DEPTH;
        BUFFER.0[13] = 4;
        BUFFER.0[14] = 4;
        BUFFER.0[15] = depth;

        BUFFER.0[16] = TAG_SET_PIXEL_ORDER;
        BUFFER.0[17] = 4;
        BUFFER.0[18] = 4;
        BUFFER.0[19] = PIXEL_ORDER_BGR;

        BUFFER.0[20] = TAG_ALLOCATE_BUFFER;
        BUFFER.0[21] = 8;
        BUFFER.0[22] = 8;
        BUFFER.0[23] = 16; // 16-byte align (response: framebuffer base)
        BUFFER.0[24] = 0; //  (response: framebuffer size in bytes)

        BUFFER.0[25] = TAG_GET_PITCH;
        BUFFER.0[26] = 4;
        BUFFER.0[27] = 4;
        BUFFER.0[28] = 0; // (response: pitch)

        // Read back the firmware's ACTUAL geometry — the SET echoes and the
        // reported pitch have both proven unreliable on Pi 5.
        BUFFER.0[29] = TAG_GET_PHYS_WH;
        BUFFER.0[30] = 8;
        BUFFER.0[31] = 0;
        BUFFER.0[32] = 0; // (response: physical width)
        BUFFER.0[33] = 0; // (response: physical height)

        BUFFER.0[34] = TAG_GET_VIRT_WH;
        BUFFER.0[35] = 8;
        BUFFER.0[36] = 0;
        BUFFER.0[37] = 0; // (response: virtual width)
        BUFFER.0[38] = 0; // (response: virtual height)

        BUFFER.0[39] = TAG_END;

        if call(CHANNEL_PROPERTY) {
            let bus_addr = BUFFER.0[23] as u64;
            let arm_addr = bus_addr & 0x3FFF_FFFF;
            let alloc_size = BUFFER.0[24];
            let reported_pitch = BUFFER.0[28];
            let phys_w = BUFFER.0[32];
            let phys_h = BUFFER.0[33];
            let virt_w = BUFFER.0[37];
            let virt_h = BUFFER.0[38];
            FB_DIAG = [phys_w, phys_h, virt_w, virt_h, reported_pitch, alloc_size];

            // Physical = display mode; virtual = the stored surface (what the
            // pitch is based on). Fall back through the less-trusted values.
            let mut rh = if phys_h > 0 { phys_h } else if virt_h > 0 { virt_h } else { h };
            let mut rw = if virt_w > 0 { virt_w } else if phys_w > 0 { phys_w } else { w };

            // Pi 5 (BCM2712) firmware ignores the requested depth and scans the
            // panel out as 16bpp RGB565, and lies about width/pitch
            // (raspberrypi/firmware#1904): confirmed on real hardware (writing
            // 4-byte pixels -> green stripes, and the content tiled ~3x because
            // the real mode is far wider than the reported width). The one value
            // the firmware can't fake is the ALLOCATED BUFFER SIZE, so derive the
            // true stride from size/height, and the real width from stride/2.
            let (bpp, pitch) = if board() == RpiBoard::Pi5 {
                // The Pi 5 firmware lies about width/height/pitch (it reported
                // ~1200x720 for a panel actually running 1920x1080) and used to
                // hand back 16bpp RGB565 whatever depth was requested
                // (raspberrypi/firmware#1904); config.txt's framebuffer_depth=32
                // asks for full colour. What it can't fake is the ALLOCATED
                // BUFFER SIZE: recover the real depth and mode by matching
                // size/4 (32bpp) then size/2 (16bpp) against the display's EDID
                // mode, then against standard resolutions.
                const MODES: [(u32, u32); 12] = [
                    (1920, 1080),
                    (1280, 720),
                    (3840, 2160),
                    (2560, 1440),
                    (1920, 1200),
                    (1600, 900),
                    (1680, 1050),
                    (1440, 900),
                    (1366, 768),
                    (1280, 1024),
                    (1024, 768),
                    (800, 600),
                ];
                let edid = edid_preferred_resolution();
                let mut found: Option<(u32, u32, u32)> = None; // (w, h, bytes per pixel)
                for bytes in [4u32, 2] {
                    let total_px = alloc_size / bytes;
                    if let Some((ew, eh)) = edid {
                        if ew > 0 && eh > 0 && ew * eh == total_px {
                            found = Some((ew, eh, bytes));
                            break;
                        }
                    }
                    if let Some(&(cw, ch)) = MODES.iter().find(|&&(cw, ch)| cw * ch == total_px) {
                        found = Some((cw, ch, bytes));
                        break;
                    }
                }
                // No match — last-resort reported virtual size, 16bpp.
                let (mw, mh, bytes) =
                    found.unwrap_or((if rw > 0 { rw } else { w }, if rh > 0 { rh } else { h }, 2));
                rw = mw;
                rh = mh;
                ((bytes * 8) as u8, mw * bytes)
            } else {
                let byppx = if rw > 0 { reported_pitch / rw } else { 4 };
                if (2..=4).contains(&byppx) {
                    ((byppx * 8) as u8, reported_pitch)
                } else {
                    (32u8, rw * 4)
                }
            };
            Some((arm_addr, pitch, rw, rh, bpp))
        } else {
            None
        }
    }
}

/// Read the connected display's preferred (native) resolution straight from its
/// EDID over the HDMI DDC, via the mailbox. This is the robust way to auto-detect
/// resolution for ANY panel — it goes to the monitor itself, bypassing the Pi 5
/// firmware's unreliable framebuffer-geometry reports. Returns (width, height)
/// in pixels from the first detailed timing descriptor, or None if no/invalid
/// EDID. Uses the shared BUFFER, so call it when no other mailbox op is in
/// flight (allocate_framebuffer calls it after extracting its own results).
pub fn edid_preferred_resolution() -> Option<(u32, u32)> {
    let e = edid_block0()?;
    // First detailed timing descriptor is at offset 54; horizontal/vertical
    // active pixels are split across low byte + the high nibble of a shared byte.
    let h = e[56] as u32 | (((e[58] as u32) >> 4) << 8);
    let v = e[59] as u32 | (((e[61] as u32) >> 4) << 8);
    if (320..=7680).contains(&h) && (240..=4320).contains(&v) {
        Some((h, v))
    } else {
        None
    }
}

/// Pixel clock of the display's preferred mode in kHz (EDID detailed timing
/// bytes 54-55, units of 10 kHz) — HDMI audio's CTS value.
pub fn edid_pixel_clock_khz() -> Option<u32> {
    let e = edid_block0()?;
    let v = (e[54] as u32 | ((e[55] as u32) << 8)) * 10;
    if (25_000..=600_000).contains(&v) {
        Some(v)
    } else {
        None
    }
}

/// EDID block 0 over the HDMI DDC via the mailbox; None if no/invalid EDID.
fn edid_block0() -> Option<[u8; 128]> {
    unsafe {
        BUFFER.0[0] = 40 * 4;
        BUFFER.0[1] = 0;
        BUFFER.0[2] = TAG_GET_EDID;
        BUFFER.0[3] = 136; // value buffer: 4 (block#) + 4 (status) + 128 (EDID)
        BUFFER.0[4] = 4; // request length (the block number)
        BUFFER.0[5] = 0; // block number 0  -> response: echoed block number
        BUFFER.0[6] = 0; //                 -> response: status (0 = success)
        for i in 0..32 {
            BUFFER.0[7 + i] = 0; // 128 EDID bytes
        }
        BUFFER.0[39] = TAG_END;

        if !call(CHANNEL_PROPERTY) {
            return None;
        }
        if BUFFER.0[6] != 0 {
            return None; // non-zero status: EDID read failed
        }
        // EDID bytes live little-endian in BUFFER.0[7..7+32].
        let mut e = [0u8; 128];
        for (n, b) in e.iter_mut().enumerate() {
            *b = ((BUFFER.0[7 + n / 4] >> (8 * (n % 4))) & 0xFF) as u8;
        }
        // Header must be 00 FF FF FF FF FF FF 00.
        if e[0] != 0x00 || e[1] != 0xFF || e[7] != 0x00 {
            return None;
        }
        Some(e)
    }
}
