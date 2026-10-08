//! VideoCore mailbox property-interface channel. The register block moves per
//! board (BCM283x: peripheral_base + 0xB880; BCM2712/Pi 5: + 0x13880), so the
//! addresses are resolved at call time from the detected board, not `const`.

use crate::arch::mmio::{mailbox_offset, peripheral_base, read32, write32};

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
const TAG_END: u32 = 0;

/// Pixel channel order: 0 = BGR, 1 = RGB. Our `framebuffer::set_pixel` writes a
/// `0xRRGGBB` u32 little-endian (bytes B,G,R in memory), so the panel must read
/// byte 0 as blue — i.e. BGR — for colors to come out right.
const PIXEL_ORDER_BGR: u32 = 0;

#[repr(align(16))]
struct MailboxBuffer([u32; 36]);
static mut BUFFER: MailboxBuffer = MailboxBuffer([0; 36]);

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
        BUFFER.0[0] = 30 * 4;
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
        BUFFER.0[24] = 0; //  (response: framebuffer size)

        BUFFER.0[25] = TAG_GET_PITCH;
        BUFFER.0[26] = 4;
        BUFFER.0[27] = 4;
        BUFFER.0[28] = 0; // (response: pitch)

        BUFFER.0[29] = TAG_END;

        if call(CHANNEL_PROPERTY) {
            let bus_addr = BUFFER.0[23] as u64;
            let pitch = BUFFER.0[28];
            // Convert VC bus address (0xC0000000.. or 0x40000000..) to ARM physical (0x00000000..)
            let arm_addr = bus_addr & 0x3FFF_FFFF;
            Some((arm_addr, pitch, w, h, depth as u8))
        } else {
            None
        }
    }
}
