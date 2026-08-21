//! ATA PIO disk driver (x86_64) — the primary bus, LBA28 addressing, polled
//! (no IRQs; matches the rest of this "basic" kernel's no-interrupt-handler
//! model). Standard registers/sequence — see the OSDev wiki's "ATA PIO
//! Mode" article. This is `lingfs`'s block device: every read/write goes
//! through here as fixed 512-byte sectors.
use crate::arch::io::{inb, insw, outb, outsw};

const DATA: u16 = 0x1F0;
const ERROR_FEATURES: u16 = 0x1F1;
const SECTOR_COUNT: u16 = 0x1F2;
const LBA_LOW: u16 = 0x1F3;
const LBA_MID: u16 = 0x1F4;
const LBA_HIGH: u16 = 0x1F5;
const DRIVE_HEAD: u16 = 0x1F6;
const STATUS_COMMAND: u16 = 0x1F7;

const CMD_READ_SECTORS: u8 = 0x20;
const CMD_WRITE_SECTORS: u8 = 0x30;
const CMD_CACHE_FLUSH: u8 = 0xE7;

const STATUS_ERR: u8 = 0x01;
const STATUS_DRQ: u8 = 0x08;
const STATUS_BSY: u8 = 0x80;

pub const SECTOR_SIZE: usize = 512;

/// Generous for a real drive spinning up from a cold start, still finite —
/// see `wait_ready`/`wait_drq`'s doc for why unbounded was never safe here:
/// with no drive on the bus at all, the status port floats and can read
/// back with BSY permanently set, so an unbounded poll hangs forever.
const ATA_TIMEOUT_US: u64 = 5_000_000;

fn wait_ready() -> Result<(), ()> {
    // Poll until BSY clears; bail out on ERR (or on timeout — see
    // `ATA_TIMEOUT_US`) rather than spinning forever.
    let mut err = false;
    let ready = crate::arch::timer::poll_until(ATA_TIMEOUT_US, || {
        let status = unsafe { inb(STATUS_COMMAND) };
        if status & STATUS_ERR != 0 {
            err = true;
            return true; // stop polling; caller distinguishes err below
        }
        status & STATUS_BSY == 0
    });
    if err || !ready {
        return Err(());
    }
    Ok(())
}

fn wait_drq() -> Result<(), ()> {
    let mut err = false;
    let ready = crate::arch::timer::poll_until(ATA_TIMEOUT_US, || {
        let status = unsafe { inb(STATUS_COMMAND) };
        if status & STATUS_ERR != 0 {
            err = true;
            return true;
        }
        status & STATUS_DRQ != 0
    });
    if err || !ready {
        return Err(());
    }
    Ok(())
}

/// Read one 512-byte sector at `lba` (LBA28 — addresses up to 128GiB) into
/// `buf`. Primary bus, master drive only — this is a "basic" kernel with
/// one disk, not a general ATA stack.
pub fn read_sector(lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
    wait_ready()?;
    unsafe {
        outb(DRIVE_HEAD, 0xE0 | ((lba >> 24) & 0x0F) as u8);
        outb(ERROR_FEATURES, 0x00);
        outb(SECTOR_COUNT, 1);
        outb(LBA_LOW, (lba & 0xFF) as u8);
        outb(LBA_MID, ((lba >> 8) & 0xFF) as u8);
        outb(LBA_HIGH, ((lba >> 16) & 0xFF) as u8);
        outb(STATUS_COMMAND, CMD_READ_SECTORS);
    }
    wait_ready()?;
    wait_drq()?;
    unsafe { insw(DATA, buf.as_mut_ptr() as *mut u16, SECTOR_SIZE / 2) };
    Ok(())
}

/// Write one 512-byte sector at `lba`, then flush the drive's write cache
/// (so a "the write succeeded" return actually means the data is durable,
/// not just sitting in a volatile cache — matters a lot once this is doing
/// crash-consistent, content-addressed storage).
pub fn write_sector(lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
    wait_ready()?;
    unsafe {
        outb(DRIVE_HEAD, 0xE0 | ((lba >> 24) & 0x0F) as u8);
        outb(ERROR_FEATURES, 0x00);
        outb(SECTOR_COUNT, 1);
        outb(LBA_LOW, (lba & 0xFF) as u8);
        outb(LBA_MID, ((lba >> 8) & 0xFF) as u8);
        outb(LBA_HIGH, ((lba >> 16) & 0xFF) as u8);
        outb(STATUS_COMMAND, CMD_WRITE_SECTORS);
    }
    wait_ready()?;
    wait_drq()?;
    unsafe { outsw(DATA, buf.as_ptr() as *const u16, SECTOR_SIZE / 2) };
    wait_ready()?;
    unsafe { outb(STATUS_COMMAND, CMD_CACHE_FLUSH) };
    wait_ready()?;
    Ok(())
}
