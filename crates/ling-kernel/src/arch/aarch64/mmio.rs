use core::ptr;

/// Peripheral base address for the BCM2837 (Raspberry Pi 3) memory map, which
/// is also what QEMU's `raspi3b` machine emulates — the fast dev-loop target.
/// Pi 4 (BCM2711) uses `0xFE00_0000`; real Pi 5 (BCM2712) puts GPIO/UART
/// behind the RP1 southbridge chip over PCIe with a different layout entirely
/// and needs follow-up HAL work, not just a base-address change.
pub const PERIPHERAL_BASE: usize = 0x3F00_0000;

#[inline(always)]
pub unsafe fn read32(addr: usize) -> u32 {
    ptr::read_volatile(addr as *const u32)
}

#[inline(always)]
pub unsafe fn write32(addr: usize, val: u32) {
    ptr::write_volatile(addr as *mut u32, val);
}
