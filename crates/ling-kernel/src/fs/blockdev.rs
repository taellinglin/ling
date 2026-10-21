//! Block device selection. x86: try AHCI (SATA) first, fall back to legacy
//! ATA PIO -- so the disk works whether it is attached via modern SATA/AHCI
//! (VirtualBox's default, most real current hardware) or legacy IDE (QEMU's
//! `-drive if=ide`, old hardware). Raspberry Pi: the SD card (`arch::sdcard`).
//! `lingfs` calls only this module, never a driver directly.

#[cfg(target_arch = "x86_64")]
use crate::drivers::ata;

#[cfg(target_arch = "x86_64")]
pub const SECTOR_SIZE: usize = ata::SECTOR_SIZE;
#[cfg(target_arch = "aarch64")]
pub const SECTOR_SIZE: usize = crate::arch::sdcard::SECTOR_SIZE;

#[cfg(target_arch = "x86_64")]
static mut USE_AHCI: bool = false;
#[cfg(target_arch = "x86_64")]
static mut INITED: bool = false;

#[cfg(target_arch = "x86_64")]
pub fn init() {
    if unsafe { INITED } {
        return;
    }
    unsafe {
        USE_AHCI = crate::drivers::ahci::init().is_ok();
        INITED = true;
    }
}

#[cfg(target_arch = "aarch64")]
pub fn init() {}

#[cfg(target_arch = "x86_64")]
pub fn read_sector(lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
    init();
    if unsafe { USE_AHCI } {
        crate::drivers::ahci::read_sector(lba, buf)
    } else {
        ata::read_sector(lba, buf)
    }
}

#[cfg(target_arch = "x86_64")]
pub fn write_sector(lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
    init();
    if unsafe { USE_AHCI } {
        crate::drivers::ahci::write_sector(lba, buf)
    } else {
        ata::write_sector(lba, buf)
    }
}

#[cfg(target_arch = "aarch64")]
pub fn read_sector(lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
    crate::arch::sdcard::read_sector(lba, buf)
}

#[cfg(target_arch = "aarch64")]
pub fn write_sector(lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
    crate::arch::sdcard::write_sector(lba, buf)
}

/// Which backend `read_sector`/`write_sector` actually dispatch to right
/// now -- used to populate the synthetic `dev/` directory `lingfs` seeds on
/// mount, so `ls dev` reflects the disk controller this boot actually
/// detected rather than a hardcoded guess.
pub fn active_driver_name() -> &'static str {
    init();
    #[cfg(target_arch = "x86_64")]
    {
        if unsafe { USE_AHCI } {
            return "ahci0";
        }
        return "ata0";
    }
    #[allow(unreachable_code)]
    "sd0"
}
