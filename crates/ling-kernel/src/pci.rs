//! PCI configuration space access (legacy mechanism #1: the 0xCF8/0xCFC
//! port pair) and a full bus/device/function enumeration. Used by
//! `ahci.rs` to find the AHCI (SATA) controller, since that's a PCI
//! device whose MMIO register window (the ABAR) lives at a
//! firmware-assigned address we can only learn by reading its BAR — there
//! is no fixed port/address for it the way there is for legacy ATA.
use crate::io::{inl, outl};

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

fn address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    (1 << 31)
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | (offset as u32 & 0xFC)
}

pub fn read32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    unsafe {
        outl(CONFIG_ADDRESS, address(bus, device, function, offset));
        inl(CONFIG_DATA)
    }
}

pub fn write32(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    unsafe {
        outl(CONFIG_ADDRESS, address(bus, device, function, offset));
        outl(CONFIG_DATA, value);
    }
}

#[derive(Copy, Clone)]
pub struct Location {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl Location {
    pub fn read32(&self, offset: u8) -> u32 {
        read32(self.bus, self.device, self.function, offset)
    }

    /// Base address of BAR `n` (0..=5), with the low flag bits masked off.
    /// Doesn't handle 64-bit (paired) BARs — every AHCI controller in
    /// practice exposes its ABAR as a 32-bit BAR5, which is all this
    /// kernel needs.
    pub fn bar(&self, n: u8) -> u32 {
        self.read32(0x10 + n * 4) & 0xFFFF_FFF0
    }
}

/// Scan every bus/device/function for a PCI device matching
/// (class, subclass, prog_if) and return its location. Exhaustive
/// (256 * 32 * 8 config reads) but that's a few hundred thousand cheap
/// port I/O ops — fast enough to just always do at boot rather than
/// bother with the "does this bus even exist" shortcuts real firmware
/// uses.
pub fn find(class: u8, subclass: u8, prog_if: u8) -> Option<Location> {
    for bus in 0..=255u8 {
        for device in 0..32u8 {
            let header0 = read32(bus, device, 0, 0x00);
            if (header0 & 0xFFFF) == 0xFFFF {
                continue; // vendor id 0xFFFF: no device here
            }
            let header_type = (read32(bus, device, 0, 0x0C) >> 16) & 0xFF;
            let max_function = if header_type & 0x80 != 0 { 8 } else { 1 };
            for function in 0..max_function {
                let vendor = read32(bus, device, function, 0x00) & 0xFFFF;
                if vendor == 0xFFFF {
                    continue;
                }
                let class_reg = read32(bus, device, function, 0x08);
                let got_class = (class_reg >> 24) as u8;
                let got_subclass = (class_reg >> 16) as u8;
                let got_prog_if = (class_reg >> 8) as u8;
                if got_class == class && got_subclass == subclass && got_prog_if == prog_if {
                    return Some(Location { bus, device, function });
                }
            }
        }
    }
    None
}
