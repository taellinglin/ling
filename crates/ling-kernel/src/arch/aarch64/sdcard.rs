//! The SD card on the Raspberry Pi: the SDHCI host behind the card slot (Pi 5:
//! sdio1 on the BCM2712; Pi 4: emmc2; Pi 3 and QEMU's raspi3b: the Arasan
//! controller), brought up with the standard SD identification sequence and
//! driven by PIO single-block reads and writes (CMD17 / CMD24). This is
//! lingfs's block device on the Pi (`fs::blockdev`). Init prints each step so
//! a photo of the console says where a real card stops.

use crate::arch::mmio::{board, read32, write32, RpiBoard};
use crate::arch::timer::delay_us;
use crate::console_write as out;

pub const SECTOR_SIZE: usize = 512;

// SDHCI registers (SD Host Controller spec).
const BLOCK: usize = 0x04; // block size (lo 16) + block count (hi 16)
const ARG: usize = 0x08;
const XFER_CMD: usize = 0x0C; // transfer mode (lo 16) + command (hi 16)
const RESP0: usize = 0x10;
const RESP1: usize = 0x14;
const RESP2: usize = 0x18;
const RESP3: usize = 0x1C;
const BUF_DATA: usize = 0x20;
const PRESENT: usize = 0x24;
const HOST_CTRL: usize = 0x28; // + power 0x29, block gap 0x2A, wakeup 0x2B
const CLOCK_CTRL: usize = 0x2C; // clock (lo 16) + timeout 0x2E + reset 0x2F
const INT_STATUS: usize = 0x30;
const INT_ENABLE: usize = 0x34;
const INT_SIGNAL: usize = 0x38;
const CAPS: usize = 0x40;
const VERSION: usize = 0xFC;

const RESP_NONE: u32 = 0x00;
const RESP_136: u32 = 0x09; // R2 (CID/CSD): 136-bit, CRC checked
const RESP_48: u32 = 0x02; // R3 (OCR): no CRC / index check
const RESP_48_CHK: u32 = 0x1A; // R1 / R6 / R7
const RESP_48_BUSY: u32 = 0x1B; // R1b
const DATA: u32 = 1 << 5; // the command moves a data block
const XFER_READ: u32 = 1 << 4; // transfer mode: card -> host

const INT_CMD_DONE: u32 = 1 << 0;
const INT_XFER_DONE: u32 = 1 << 1;
const INT_WRITE_READY: u32 = 1 << 4;
const INT_READ_READY: u32 = 1 << 5;
const INT_ERROR: u32 = 1 << 15;

struct Card {
    base: usize,
    clock_hz: u32,
    rca: u32,
    high_capacity: bool,
    sectors: u32,
}

static mut CARD: Option<Card> = None;

fn card() -> Option<&'static Card> {
    unsafe { (*(&raw const CARD)).as_ref() }
}

/// Is a card initialised?
pub fn ready() -> bool {
    card().is_some()
}

/// Capacity in 512-byte sectors (0 without a card).
pub fn sectors() -> u32 {
    card().map(|c| c.sectors).unwrap_or(0)
}

fn rd(base: usize, off: usize) -> u32 {
    unsafe { read32(base + off) }
}

fn wr(base: usize, off: usize, v: u32) {
    unsafe { write32(base + off, v) }
}

fn hex(label: &[u8], v: u64, nibbles: usize) {
    out(label);
    out(b"0x");
    let mut b = [0u8; 16];
    for i in 0..nibbles {
        let d = ((v >> ((nibbles - 1 - i) * 4)) & 0xF) as u8;
        b[i] = if d < 10 { b'0' + d } else { b'a' + d - 10 };
    }
    out(&b[..nibbles]);
    out(b"\n");
}

fn dec(label: &[u8], mut v: u64, tail: &[u8]) {
    out(label);
    let mut b = [0u8; 20];
    let mut i = b.len();
    if v == 0 {
        i -= 1;
        b[i] = b'0';
    }
    while v > 0 {
        i -= 1;
        b[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    out(&b[i..]);
    out(tail);
}

/// Wait until (reg & mask) == want, up to `ms` milliseconds.
fn wait(base: usize, off: usize, mask: u32, want: u32, ms: u32) -> bool {
    for _ in 0..ms * 10 {
        if rd(base, off) & mask == want {
            return true;
        }
        delay_us(100);
    }
    false
}

/// Reset the CMD (0x2) and/or DAT (0x4) lines after an error.
fn reset_lines(base: usize, bits: u32) {
    wr(base, CLOCK_CTRL, rd(base, CLOCK_CTRL) | (bits << 24));
    wait(base, CLOCK_CTRL, bits << 24, 0, 100);
}

/// Issue one command (`mode` = transfer-mode bits for data commands); returns
/// RESP0, or None after an error or timeout (reported when `verbose`).
fn cmd(base: usize, idx: u32, arg: u32, flags: u32, mode: u32, verbose: bool) -> Option<u32> {
    if !wait(base, PRESENT, 0x1, 0, 200) {
        out(b"  sd: command line busy\n");
        return None;
    }
    if (flags == RESP_48_BUSY || flags & DATA != 0) && !wait(base, PRESENT, 0x2, 0, 500) {
        out(b"  sd: data line busy\n");
        return None;
    }
    wr(base, INT_STATUS, 0xFFFF_FFFF);
    wr(base, ARG, arg);
    wr(base, XFER_CMD, (((idx << 8) | flags) << 16) | mode);
    for _ in 0..4000 {
        let st = rd(base, INT_STATUS);
        if st & INT_ERROR != 0 {
            wr(base, INT_STATUS, st & !(INT_READ_READY | INT_WRITE_READY));
            reset_lines(base, 0x2);
            if verbose {
                dec(b"  sd: CMD", idx as u64, b"");
                hex(b" error, int status=", st as u64, 8);
            }
            return None;
        }
        if st & INT_CMD_DONE != 0 {
            wr(base, INT_STATUS, INT_CMD_DONE);
            if flags == RESP_48_BUSY {
                wait(base, INT_STATUS, INT_XFER_DONE, INT_XFER_DONE, 500);
                wr(base, INT_STATUS, INT_XFER_DONE);
            }
            return Some(rd(base, RESP0));
        }
        delay_us(50);
    }
    if verbose {
        dec(b"  sd: CMD", idx as u64, b" timeout (no response)\n");
    }
    reset_lines(base, 0x2);
    None
}

/// SD clock: off, divided-clock mode f = base / (2N), internal clock stable, on.
fn set_clock(base: usize, base_hz: u32, hz: u32) {
    let n = if hz >= base_hz { 0 } else { base_hz.div_ceil(2 * hz).min(1023) };
    let clk = ((n & 0xFF) << 8) | (((n >> 8) & 0x3) << 6) | 0x1;
    wr(base, CLOCK_CTRL, rd(base, CLOCK_CTRL) & 0xFFFF_0000); // SD clock off
    wr(base, CLOCK_CTRL, (rd(base, CLOCK_CTRL) & 0xFF00_0000) | (0x0E << 16) | clk); // timeout 2^27
    wait(base, CLOCK_CTRL, 0x2, 0x2, 100);
    wr(base, CLOCK_CTRL, rd(base, CLOCK_CTRL) | 0x4);
    delay_us(2_000);
}

/// Bring the card up (idempotent). Prints what it finds.
pub fn init() -> bool {
    if ready() {
        return true;
    }
    let (base, clock_hz, name): (usize, u32, &[u8]) = match board() {
        RpiBoard::Pi5 => (0x10_00ff_f000, 200_000_000, b"Pi 5 sdio1"),
        RpiBoard::Pi4 => (0xFE34_0000, 100_000_000, b"Pi 4 emmc2"),
        RpiBoard::Pi3 => (0x3F30_0000, 100_000_000, b"Pi 3 Arasan"),
        RpiBoard::Unknown => {
            out(b"sd: unknown board, no controller address\n");
            return false;
        },
    };
    out(b"SD card (");
    out(name);
    out(b"):\n");
    hex(b"  SDHCI version=", (rd(base, VERSION) >> 16) as u64, 4);
    hex(b"  capabilities=", rd(base, CAPS) as u64, 8);
    if board() == RpiBoard::Pi5 {
        // The controller's config block: pins in SD (not eMMC) mode.
        let cfg = base + 0x400;
        unsafe {
            let p = read32(cfg + 0x44) & !0x3;
            write32(cfg + 0x44, p | 0x2);
        }
    }
    wr(base, CLOCK_CTRL, 1 << 24); // software reset (all)
    if !wait(base, CLOCK_CTRL, 1 << 24, 0, 100) {
        out(b"  SDHCI reset timeout\n");
        return false;
    }
    wr(base, HOST_CTRL, 0x0F << 8); // 3.3 V bus power, 1-bit bus
    delay_us(10_000);
    set_clock(base, clock_hz, 400_000);
    wr(base, INT_ENABLE, 0xFFFF_FFFF);
    wr(base, INT_SIGNAL, 0);
    delay_us(10_000);
    let present = rd(base, PRESENT);
    hex(b"  present state=", present as u64, 8);
    if present & (1 << 16) == 0 {
        out(b"  controller reports no card inserted\n");
        if board() == RpiBoard::Pi5 {
            // Try the config block's force-present bit (the Wi-Fi SDIO needed it).
            let cfg = base + 0x400;
            unsafe { write32(cfg, (read32(cfg) & !(1 << 30)) | (1 << 31)) };
            delay_us(10_000);
            hex(b"  present state (forced)=", rd(base, PRESENT) as u64, 8);
        }
    }

    // Identification: CMD0, CMD8 (SD 2.0+), ACMD41 until powered up, CMD2, CMD3.
    cmd(base, 0, 0, RESP_NONE, 0, true);
    delay_us(2_000);
    let v2 = match cmd(base, 8, 0x1AA, RESP_48_CHK, 0, false) {
        Some(r) => {
            hex(b"  CMD8 response=", r as u64, 8);
            r & 0xFFF == 0x1AA
        },
        None => {
            out(b"  no CMD8 answer: SD 1.x card, or no card\n");
            false
        },
    };
    let mut ocr = 0u32;
    for _ in 0..300 {
        if cmd(base, 55, 0, RESP_48_CHK, 0, true).is_none() {
            out(b"  CMD55 failed: no card answering\n");
            return false;
        }
        let Some(r) = cmd(base, 41, if v2 { 0x40FF_8000 } else { 0x00FF_8000 }, RESP_48, 0, true) else {
            out(b"  ACMD41 failed\n");
            return false;
        };
        if r & (1 << 31) != 0 {
            ocr = r;
            break;
        }
        delay_us(10_000);
    }
    if ocr == 0 {
        out(b"  card never left the busy state (ACMD41)\n");
        return false;
    }
    hex(b"  OCR=", ocr as u64, 8);
    let high_capacity = ocr & (1 << 30) != 0;
    if cmd(base, 2, 0, RESP_136, 0, true).is_none() {
        return false;
    }
    let Some(r6) = cmd(base, 3, 0, RESP_48_CHK, 0, true) else { return false };
    let rca = r6 >> 16;
    hex(b"  RCA=", rca as u64, 4);

    // Capacity from the CSD (R2: RESP0..3 hold response bits [127:8]).
    let mut sectors = 0u32;
    if cmd(base, 9, rca << 16, RESP_136, 0, true).is_some() {
        let (r1, r2, r3) = (rd(base, RESP1), rd(base, RESP2), rd(base, RESP3));
        let csd_ver = (r3 >> 22) & 0x3;
        if csd_ver >= 1 {
            let c_size = (r1 >> 8) & 0x3F_FFFF; // bits [69:48]
            sectors = (c_size + 1).saturating_mul(1024);
        } else {
            let read_bl_len = (r2 >> 8) & 0xF; // [83:80]
            let c_size = ((r2 & 0x3) << 10) | (r1 >> 22); // [73:62]
            let mult = (r1 >> 7) & 0x7; // [49:47]
            let bytes = ((c_size as u64 + 1) << (mult + 2)) << read_bl_len;
            sectors = (bytes / 512).min(u32::MAX as u64) as u32;
        }
    }
    if cmd(base, 7, rca << 16, RESP_48_BUSY, 0, true).is_none() {
        out(b"  CMD7 (select card) failed\n");
        return false;
    }
    if !high_capacity {
        cmd(base, 16, 512, RESP_48_CHK, 0, true);
    }
    // 4-bit bus (ACMD6), then the data clock.
    if cmd(base, 55, rca << 16, RESP_48_CHK, 0, false).is_some() && cmd(base, 6, 2, RESP_48_CHK, 0, false).is_some() {
        wr(base, HOST_CTRL, rd(base, HOST_CTRL) | 0x2);
    }
    set_clock(base, clock_hz, 25_000_000);
    out(if high_capacity { b"  SDHC/SDXC (block addressing)" } else { b"  SDSC (byte addressing)" });
    dec(b", ", sectors as u64 / 2048, b" MiB\n");
    unsafe { core::ptr::write(&raw mut CARD, Some(Card { base, clock_hz, rca, high_capacity, sectors })) };
    true
}

/// One 512-byte block in or out by PIO (CMD17 / CMD24).
fn block_io(c: &Card, lba: u32, buf: &mut [u8; SECTOR_SIZE], write: bool) -> Result<(), ()> {
    let base = c.base;
    let arg = if c.high_capacity { lba } else { lba.wrapping_mul(512) };
    wr(base, BLOCK, (1 << 16) | SECTOR_SIZE as u32);
    let (idx, mode) = if write { (24, 0) } else { (17, XFER_READ) };
    cmd(base, idx, arg, RESP_48_CHK | DATA, mode, true).ok_or(())?;
    let mut moved = false;
    for _ in 0..40_000 {
        let st = rd(base, INT_STATUS);
        if st & INT_ERROR != 0 {
            wr(base, INT_STATUS, st);
            hex(b"  sd: data error, int status=", st as u64, 8);
            reset_lines(base, 0x6);
            return Err(());
        }
        if !write && st & INT_READ_READY != 0 && !moved {
            wr(base, INT_STATUS, INT_READ_READY);
            for i in 0..SECTOR_SIZE / 4 {
                buf[i * 4..i * 4 + 4].copy_from_slice(&rd(base, BUF_DATA).to_le_bytes());
            }
            moved = true;
        }
        if write && st & INT_WRITE_READY != 0 && !moved {
            wr(base, INT_STATUS, INT_WRITE_READY);
            for i in 0..SECTOR_SIZE / 4 {
                wr(base, BUF_DATA, u32::from_le_bytes([buf[i * 4], buf[i * 4 + 1], buf[i * 4 + 2], buf[i * 4 + 3]]));
            }
            moved = true;
        }
        if st & INT_XFER_DONE != 0 && moved {
            wr(base, INT_STATUS, INT_XFER_DONE);
            if write {
                // The card programs the block while holding DAT0 low.
                wait(base, PRESENT, 1 << 20, 1 << 20, 1000);
            }
            return Ok(());
        }
        delay_us(25);
    }
    out(b"  sd: data transfer timeout\n");
    reset_lines(base, 0x6);
    Err(())
}

pub fn read_sector(lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
    let c = card().ok_or(())?;
    if lba >= c.sectors && c.sectors != 0 {
        return Err(());
    }
    block_io(c, lba, buf, false).or_else(|_| block_io(c, lba, buf, false))
}

pub fn write_sector(lba: u32, data: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
    let c = card().ok_or(())?;
    if lba >= c.sectors && c.sectors != 0 {
        return Err(());
    }
    let mut buf = *data;
    block_io(c, lba, &mut buf, true).or_else(|_| {
        buf = *data;
        block_io(c, lba, &mut buf, true)
    })
}

/// Where lingfs can live on this card: the free space after the last MBR
/// partition, 1 MiB aligned, if at least `min_sectors` of it fit. Prints the
/// partition table it saw.
pub fn free_region_after_partitions(min_sectors: u32) -> Option<u32> {
    let c = card()?;
    let mut mbr = [0u8; SECTOR_SIZE];
    read_sector(0, &mut mbr).ok()?;
    if mbr[510] != 0x55 || mbr[511] != 0xAA {
        out(b"  sd: no MBR partition table on the card\n");
        return None;
    }
    let mut end = 2048u32;
    for i in 0..4 {
        let e = &mbr[446 + i * 16..446 + (i + 1) * 16];
        if e[4] == 0 {
            continue;
        }
        let start = u32::from_le_bytes([e[8], e[9], e[10], e[11]]);
        let n = u32::from_le_bytes([e[12], e[13], e[14], e[15]]);
        dec(b"  partition ", i as u64 + 1, b": type ");
        hex(b"", e[4] as u64, 2);
        dec(b"    LBA ", start as u64, b" + ");
        dec(b"", n as u64, b" sectors\n");
        end = end.max(start.saturating_add(n));
    }
    let base = (end + 2047) & !2047;
    if base.saturating_add(min_sectors) > c.sectors {
        out(b"  sd: not enough room after the partitions for lingfs\n");
        return None;
    }
    Some(base)
}
