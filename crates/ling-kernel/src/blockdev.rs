//! Block device selection: try AHCI (SATA) first, fall back to legacy ATA
//! PIO. `lingfs` calls only this module, never `ahci`/`ata` directly, so it
//! works whether the disk is attached via modern SATA/AHCI (VirtualBox's
//! default, and most real current hardware) or legacy IDE (what QEMU's
//! `-drive if=ide` gives you, and what real old hardware has).
use crate::ata;

pub const SECTOR_SIZE: usize = ata::SECTOR_SIZE;

static mut USE_AHCI: bool = false;
static mut INITED: bool = false;

pub fn init() {
    if unsafe { INITED } {
        return;
    }
    unsafe {
        USE_AHCI = crate::ahci::init().is_ok();
        INITED = true;
    }
}

pub fn read_sector(lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
    init();
    if unsafe { USE_AHCI } {
        crate::ahci::read_sector(lba, buf)
    } else {
        ata::read_sector(lba, buf)
    }
}

pub fn write_sector(lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
    init();
    if unsafe { USE_AHCI } {
        crate::ahci::write_sector(lba, buf)
    } else {
        ata::write_sector(lba, buf)
    }
}

/// Which backend `read_sector`/`write_sector` actually dispatch to right
/// now — used to populate the synthetic `dev/` directory `lingfs` seeds on
/// mount, so `ls dev` reflects the disk controller this boot actually
/// detected rather than a hardcoded guess.
pub fn active_driver_name() -> &'static str {
    init();
    if unsafe { USE_AHCI } { "ahci0" } else { "ata0" }
}
