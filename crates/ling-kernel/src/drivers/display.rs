//! Display-mode preference: the Settings "Display" row. VBE can't switch
//! modes after the kernel is in long mode (the mode-set is a real-mode
//! BIOS call stage2 makes at boot), so "changing resolution" here means
//! *persisting a preference* that stage2 applies on the NEXT boot -- the
//! honest mechanism, disclosed in the Settings row's hint, not a fake
//! instant switch.
//!
//! The preference is one byte at offset 16 of the disk-boot header sector
//! (LBA 17). Writing it does a read-modify-write of just that sector so
//! the kernel header stays intact. On a Live/GRUB boot there's no such
//! header to write, and the mode was chosen by GRUB, so the row reports
//! the current framebuffer size read-only.

use crate::drivers::framebuffer;
use crate::fs::blockdev::{self, SECTOR_SIZE};

const HEADER_LBA: u32 = 17;
const HEADER_MAGIC: u32 = 0x474E_4B4C; // "LKNG" LE -- matches pack_header.py
const PREF_OFFSET: usize = 16;

/// (label, width, height) per preference index -- must match stage2.asm's
/// `display_modes` table order exactly.
pub const MODES: [(&str, u32, u32); 5] = [
    ("1024 x 768", 1024, 768),
    ("800 x 600", 800, 600),
    ("1280 x 1024", 1280, 1024),
    ("640 x 480", 640, 480),
    ("1280 x 720", 1280, 720),
];

pub fn mode_count() -> usize {
    MODES.len()
}

pub fn mode_label(i: usize) -> &'static str {
    MODES.get(i).map(|m| m.0).unwrap_or("")
}

/// Read the persisted preference index from the header sector; 0 if the
/// header isn't present/valid (Live boot, fresh disk).
pub fn preferred() -> usize {
    let mut sec = [0u8; SECTOR_SIZE];
    if blockdev::read_sector(HEADER_LBA, &mut sec).is_err() {
        return 0;
    }
    let magic = u32::from_le_bytes([sec[0], sec[1], sec[2], sec[3]]);
    if magic != HEADER_MAGIC {
        return 0;
    }
    (sec[PREF_OFFSET] as usize).min(MODES.len() - 1)
}

/// Persist a new preference index (applied at next boot). Returns false
/// if there's no valid disk-boot header to write into (e.g. a Live boot),
/// which the Settings row surfaces rather than silently succeeding.
pub fn set_preferred(idx: usize) -> bool {
    if idx >= MODES.len() {
        return false;
    }
    let mut sec = [0u8; SECTOR_SIZE];
    if blockdev::read_sector(HEADER_LBA, &mut sec).is_err() {
        return false;
    }
    let magic = u32::from_le_bytes([sec[0], sec[1], sec[2], sec[3]]);
    if magic != HEADER_MAGIC {
        return false; // no installed-boot header here
    }
    sec[PREF_OFFSET] = idx as u8;
    blockdev::write_sector(HEADER_LBA, &sec).is_ok()
}

/// Is there an installed-boot header we can actually persist into?
pub fn persistable() -> bool {
    let mut sec = [0u8; SECTOR_SIZE];
    if blockdev::read_sector(HEADER_LBA, &mut sec).is_err() {
        return false;
    }
    u32::from_le_bytes([sec[0], sec[1], sec[2], sec[3]]) == HEADER_MAGIC
}

/// Current live framebuffer size as "WxH" -- what the Display row shows
/// as the active mode (the persisted pref is only the *next*-boot target).
static mut CUR_BUF: [u8; 16] = [0; 16];
pub fn current_str() -> &'static str {
    let (w, h) = (framebuffer::width(), framebuffer::height());
    unsafe {
        let buf = &mut *&raw mut CUR_BUF;
        let mut n = 0;
        n += write_u32(&mut buf[n..], w);
        buf[n] = b'x';
        n += 1;
        n += write_u32(&mut buf[n..], h);
        core::str::from_utf8(&buf[..n]).unwrap_or("?")
    }
}

fn write_u32(buf: &mut [u8], mut v: u32) -> usize {
    if v == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 10];
    let mut n = 0;
    while v > 0 {
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    for i in 0..n {
        buf[i] = tmp[n - 1 - i];
    }
    n
}
