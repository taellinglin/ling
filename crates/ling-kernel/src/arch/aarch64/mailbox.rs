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
            return BUFFER.0[1] == 0x8000_0000;
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
                // 16bpp RGB565. Confirmed on real hardware: the firmware lies
                // about width/height/pitch (it reported ~1200x720 for a panel
                // actually running 1920x1080), but NOT about the allocated buffer
                // SIZE. At 16bpp total pixels = size/2, so recover the true mode
                // by matching that against standard display resolutions.
                let total_px = alloc_size / 2;
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
                let mut mw = 0u32;
                let mut mh = 0u32;
                for &(cw, ch) in MODES.iter() {
                    if cw * ch == total_px {
                        mw = cw;
                        mh = ch;
                        break;
                    }
                }
                if mw == 0 {
                    // No standard match — fall back to the reported virtual size.
                    mw = if rw > 0 { rw } else { w };
                    mh = if rh > 0 { rh } else { h };
                }
                rw = mw;
                rh = mh;
                (16u8, mw * 2)
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
