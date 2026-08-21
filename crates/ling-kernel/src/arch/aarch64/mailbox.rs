//! BCM2837 mailbox property-interface channel — the only way to ask the
//! VideoCore firmware anything on this hardware (there is no CPU-visible
//! register that just reports installed RAM the way x86's Multiboot2 memory
//! map does). Minimal on purpose: only `arm_memory()` (tag `0x00010005`,
//! what `mm::frame`'s aarch64 detection needs) is implemented here. The
//! framebuffer-allocation property calls this same interface can make are
//! Phase 6's job (`packages/README.md`'s build order puts the graphics
//! framebuffer after this), and belong in their own function when that
//! phase actually needs them rather than being guessed at now.
use crate::arch::mmio::{read32, write32, PERIPHERAL_BASE};

const MBOX_BASE: usize = PERIPHERAL_BASE + 0xB880;
const MBOX_READ: usize = MBOX_BASE + 0x00;
const MBOX_STATUS: usize = MBOX_BASE + 0x18;
const MBOX_WRITE: usize = MBOX_BASE + 0x20;

const MBOX_FULL: u32 = 1 << 31;
const MBOX_EMPTY: u32 = 1 << 30;
const CHANNEL_PROPERTY: u32 = 8;

const TAG_GET_ARM_MEMORY: u32 = 0x0001_0005;
const TAG_END: u32 = 0;

/// The mailbox message buffer must be 16-byte aligned and its physical
/// address fits in 28 bits with the channel number in the low 4 — easiest
/// guaranteed by a page-aligned static buffer well under the 4GiB/28-bit
/// range this kernel already runs in. No MMU is enabled at the point this
/// runs (frame-allocator detection happens before Phase 3's paging work),
/// so this physical address is also its address as accessed here.
#[repr(align(16))]
struct MailboxBuffer([u32; 8]);
static mut BUFFER: MailboxBuffer = MailboxBuffer([0; 8]);

/// Bounded rather than an unbounded spin — same reasoning as `timer.rs`'s
/// `poll_until`: a missing/misbehaving mailbox peer should never hang the
/// whole kernel forever waiting for a response that isn't coming.
const MAILBOX_TIMEOUT_SPINS: u32 = 10_000_000;

unsafe fn call(channel: u32) -> bool {
    let addr = (&raw mut BUFFER) as u32;
    let msg = (addr & !0xF) | (channel & 0xF);

    let mut spins = 0u32;
    while read32(MBOX_STATUS) & MBOX_FULL != 0 {
        core::hint::spin_loop();
        spins += 1;
        if spins > MAILBOX_TIMEOUT_SPINS {
            return false;
        }
    }
    write32(MBOX_WRITE, msg);

    spins = 0;
    loop {
        while read32(MBOX_STATUS) & MBOX_EMPTY != 0 {
            core::hint::spin_loop();
            spins += 1;
            if spins > MAILBOX_TIMEOUT_SPINS {
                return false;
            }
        }
        let resp = read32(MBOX_READ);
        if resp == msg {
            return BUFFER.0[1] == 0x8000_0000; // request succeeded
        }
    }
}

/// `(base, size)` in bytes of the board's ARM-visible memory, or `None` if
/// the firmware call fails (should not happen on real hardware or QEMU's
/// `raspi3b`, but `mm::frame`'s caller falls back to a conservative fixed
/// range rather than trusting this unconditionally).
pub fn arm_memory() -> Option<(u64, u64)> {
    unsafe {
        BUFFER.0[0] = 8 * 4; // total buffer size in bytes
        BUFFER.0[1] = 0; // request
        BUFFER.0[2] = TAG_GET_ARM_MEMORY;
        BUFFER.0[3] = 8; // value buffer size (base + size, 2x u32)
        BUFFER.0[4] = 0; // request/response indicator
        BUFFER.0[5] = 0; // response: base address
        BUFFER.0[6] = 0; // response: size
        BUFFER.0[7] = TAG_END;

        if call(CHANNEL_PROPERTY) {
            Some((BUFFER.0[5] as u64, BUFFER.0[6] as u64))
        } else {
            None
        }
    }
}
