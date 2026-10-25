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

/// One command sequence on the bus at a time. The waits below yield to other tasks (see `timer::poll_until`), and another task could
/// otherwise select the other drive in the middle of a command: whoever holds the lock finishes its command first.
static mut BUS_LOCK: bool = false;

fn lock() {
    unsafe {
        while BUS_LOCK {
            crate::proc::sched::yield_now();
        }
        BUS_LOCK = true;
    }
}

fn unlock() {
    unsafe { BUS_LOCK = false };
}

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
    lock();
    let r = read_sector_locked(lba, buf);
    unlock();
    r
}

fn read_sector_locked(lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
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
    lock();
    let r = write_sector_locked(lba, buf);
    unlock();
    r
}

fn write_sector_locked(lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
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

// ── the second drive on the primary bus: the model disk ─────────────────────────

/// Size in sectors (LBA28) of the drive on the primary bus's SLAVE position, from IDENTIFY; `None` when nothing that is a plain ATA disk
/// answers there (no drive, or an ATAPI / SATA device).
pub fn slave_sectors() -> Option<u32> {
    lock();
    let r = (|| unsafe {
        outb(DRIVE_HEAD, 0xB0);
        for _ in 0..4 {
            inb(STATUS_COMMAND); // the drive needs 400 ns to answer a select
        }
        outb(SECTOR_COUNT, 0);
        outb(LBA_LOW, 0);
        outb(LBA_MID, 0);
        outb(LBA_HIGH, 0);
        outb(STATUS_COMMAND, 0xEC); // IDENTIFY
        let s = inb(STATUS_COMMAND);
        if s == 0 || s == 0xFF {
            return None; // nothing there (a floating bus reads 0xFF)
        }
        wait_ready().ok()?;
        if inb(LBA_MID) != 0 || inb(LBA_HIGH) != 0 {
            return None; // ATAPI or SATA signature
        }
        wait_drq().ok()?;
        let mut w = [0u16; 256];
        insw(DATA, w.as_mut_ptr(), 256);
        let n = ((w[61] as u32) << 16) | w[60] as u32;
        if n == 0 {
            None
        } else {
            Some(n)
        }
    })();
    unlock();
    r
}

/// Read whole sectors from the slave drive: `buf.len()` must be a multiple of 512. Up to 128 sectors go in one command.
/// Sectors per DRQ block of READ MULTIPLE on the slave drive: 0 = not set up yet / not supported (then plain READ SECTORS, one DRQ per
/// sector). Every port access is a VM exit under QEMU, and several of them per sector made a 600 MB model take twenty minutes to read:
/// a block of 16 sectors costs the same handful of accesses as one sector did.
const MULTI: usize = 16;
static mut MULTI_STATE: u8 = 0; // 0 unknown, 1 on, 2 off

/// Tell the slave drive to use `MULTI`-sector blocks (SET MULTIPLE MODE); false when it refuses. Call with the bus locked.
fn slave_set_multiple() -> bool {
    unsafe {
        if MULTI_STATE != 0 {
            return MULTI_STATE == 1;
        }
        MULTI_STATE = 2;
        if wait_ready().is_err() {
            return false;
        }
        outb(DRIVE_HEAD, 0xF0);
        outb(ERROR_FEATURES, 0x00);
        outb(SECTOR_COUNT, MULTI as u8);
        outb(STATUS_COMMAND, 0xC6);
        if wait_ready().is_err() {
            return false;
        }
        if inb(STATUS_COMMAND) & STATUS_ERR != 0 {
            return false;
        }
        MULTI_STATE = 1;
        true
    }
}

/// Write whole sectors to the SLAVE drive -- the FAT volume media/disk1.
pub fn write_slave(lba: u32, buf: &[u8]) -> Result<(), ()> {
    if buf.len() % SECTOR_SIZE != 0 {
        return Err(());
    }
    lock();
    let mut res = Ok(());
    for (i, sec) in buf.chunks(SECTOR_SIZE).enumerate() {
        let cur = lba + i as u32;
        if wait_ready().is_err() {
            res = Err(());
            break;
        }
        unsafe {
            outb(DRIVE_HEAD, 0xF0 | ((cur >> 24) & 0x0F) as u8);
            outb(ERROR_FEATURES, 0x00);
            outb(SECTOR_COUNT, 1);
            outb(LBA_LOW, (cur & 0xFF) as u8);
            outb(LBA_MID, ((cur >> 8) & 0xFF) as u8);
            outb(LBA_HIGH, ((cur >> 16) & 0xFF) as u8);
            outb(STATUS_COMMAND, CMD_WRITE_SECTORS);
        }
        if wait_ready().is_err() || wait_drq().is_err() {
            res = Err(());
            break;
        }
        unsafe { outsw(DATA, sec.as_ptr() as *const u16, SECTOR_SIZE / 2) };
        if wait_ready().is_err() {
            res = Err(());
            break;
        }
    }
    if res.is_ok() {
        unsafe { outb(STATUS_COMMAND, CMD_CACHE_FLUSH) };
        let _ = wait_ready();
    }
    unlock();
    res
}

pub fn read_slave(lba: u32, buf: &mut [u8]) -> Result<(), ()> {
    if buf.len() % SECTOR_SIZE != 0 {
        return Err(());
    }
    lock();
    let multi = slave_set_multiple();
    let total = buf.len() / SECTOR_SIZE;
    let mut done = 0;
    let mut res = Ok(());
    'outer: while done < total {
        let mut n = (total - done).min(128);
        // whole blocks go by READ MULTIPLE, the tail (under one block) by READ SECTORS
        let use_multi = multi && n >= MULTI;
        if use_multi {
            n -= n % MULTI;
        }
        let cur = lba + done as u32;
        if wait_ready().is_err() {
            res = Err(());
            break;
        }
        if use_multi {
            unsafe {
                outb(DRIVE_HEAD, 0xF0 | ((cur >> 24) & 0x0F) as u8);
                outb(ERROR_FEATURES, 0x00);
                outb(SECTOR_COUNT, n as u8);
                outb(LBA_LOW, (cur & 0xFF) as u8);
                outb(LBA_MID, ((cur >> 8) & 0xFF) as u8);
                outb(LBA_HIGH, ((cur >> 16) & 0xFF) as u8);
                outb(STATUS_COMMAND, 0xC4);
            }
            for b in 0..n / MULTI {
                if wait_ready().is_err() || wait_drq().is_err() {
                    res = Err(());
                    break 'outer;
                }
                let at = (done + b * MULTI) * SECTOR_SIZE;
                unsafe { insw(DATA, buf[at..at + MULTI * SECTOR_SIZE].as_mut_ptr() as *mut u16, MULTI * SECTOR_SIZE / 2) };
            }
            done += n;
            continue;
        }
        unsafe {
            outb(DRIVE_HEAD, 0xF0 | ((cur >> 24) & 0x0F) as u8);
            outb(ERROR_FEATURES, 0x00);
            outb(SECTOR_COUNT, n as u8);
            outb(LBA_LOW, (cur & 0xFF) as u8);
            outb(LBA_MID, ((cur >> 8) & 0xFF) as u8);
            outb(LBA_HIGH, ((cur >> 16) & 0xFF) as u8);
            outb(STATUS_COMMAND, CMD_READ_SECTORS);
        }
        for s in 0..n {
            if wait_ready().is_err() || wait_drq().is_err() {
                res = Err(());
                break 'outer;
            }
            let at = (done + s) * SECTOR_SIZE;
            unsafe { insw(DATA, buf[at..at + SECTOR_SIZE].as_mut_ptr() as *mut u16, SECTOR_SIZE / 2) };
        }
        done += n;
    }
    unlock();
    res
}
