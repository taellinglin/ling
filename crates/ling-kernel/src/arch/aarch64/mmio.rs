use core::arch::asm;
use core::ptr;

/// Which Raspberry Pi SoC we booted on, detected at runtime from the CPU core
/// type (MIDR_EL1) so a single `kernel8.img` works on Pi 3/4/5 — the peripheral
/// MMIO window moved on every generation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RpiBoard {
    /// BCM2837, Cortex-A53. Also what QEMU's `raspi3b` emulates.
    Pi3,
    /// BCM2711, Cortex-A72.
    Pi4,
    /// BCM2712, Cortex-A76. GPIO/UART live on the RP1 southbridge over PCIe;
    /// the VideoCore mailbox + system peripherals are at a high 37-bit base.
    Pi5,
    /// Unrecognized core — fall back to the Pi 3 map (also the QEMU default).
    Unknown,
}

// Runtime-selected peripheral geometry, filled by `init_board()` from MIDR_EL1.
// Defaults are the Pi 3 / QEMU raspi3b map so a kernel that forgets to call
// init_board() still behaves exactly as before on the dev target.
static mut BOARD: RpiBoard = RpiBoard::Pi3;
static mut PBASE: usize = 0x3F00_0000;
static mut MBOX_OFF: usize = 0xB880;

/// Detect the SoC from the CPU core's MIDR_EL1 part number and latch the
/// peripheral base + mailbox offset for this board. Call once, first thing, on
/// the boot core. Reads only a CPU system register — safe before any MMIO.
pub fn init_board() {
    let midr: u64;
    unsafe {
        asm!("mrs {}, midr_el1", out(reg) midr, options(nomem, nostack, preserves_flags));
    }
    // MIDR_EL1[15:4] = primary part number. Raspberry Pi SoC ↔ core:
    //   Pi 3 BCM2837 = Cortex-A53 (0xD03), Pi 4 BCM2711 = Cortex-A72 (0xD08),
    //   Pi 5 BCM2712 = Cortex-A76 (0xD0B).
    let part = ((midr >> 4) & 0xFFF) as u32;
    let (board, base, mbox_off) = match part {
        0xD03 => (RpiBoard::Pi3, 0x3F00_0000usize, 0xB880usize),
        0xD08 => (RpiBoard::Pi4, 0xFE00_0000usize, 0xB880usize),
        // BCM2712: the VideoCore mailbox is at 0x10_7C01_3880 (base 0x10_7C00_0000
        // + offset 0x13880 — note the offset moved from the BCM283x 0xB880).
        0xD0B => (RpiBoard::Pi5, 0x10_7C00_0000usize, 0x1_3880usize),
        _ => (RpiBoard::Unknown, 0x3F00_0000usize, 0xB880usize),
    };
    unsafe {
        BOARD = board;
        PBASE = base;
        MBOX_OFF = mbox_off;
    }
}

/// The detected board.
#[inline]
pub fn board() -> RpiBoard {
    unsafe { BOARD }
}

/// Peripheral MMIO base for the detected board (0x3F000000 on Pi 3, 0xFE000000
/// on Pi 4, 0x10_7C00_0000 on Pi 5).
#[inline]
pub fn peripheral_base() -> usize {
    unsafe { PBASE }
}

/// Mailbox register block offset from [`peripheral_base`] (0xB880 on Pi 3/4,
/// 0x13880 on Pi 5).
#[inline]
pub fn mailbox_offset() -> usize {
    unsafe { MBOX_OFF }
}

#[inline(always)]
pub unsafe fn read32(addr: usize) -> u32 {
    ptr::read_volatile(addr as *const u32)
}

#[inline(always)]
pub unsafe fn write32(addr: usize, val: u32) {
    ptr::write_volatile(addr as *mut u32, val);
}
