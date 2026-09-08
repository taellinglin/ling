//! Display-mode selection: the Settings "Display" row.
//!
//! The row offers only the modes the *actual adapter* supports. On a bochs
//! DISPI adapter (QEMU std VGA / VirtualBox VBoxVGA) we query the card's
//! maximum via DISPI GETCAPS (`framebuffer::dispi_max`) and list every
//! candidate that fits both that cap and the back buffer -- so a VirtualBox
//! guest with enough VRAM gets 1600x900 / 1920x1080, a small one doesn't.
//!
//! Two apply mechanisms, chosen automatically by `apply()`:
//!  * LIVE: DISPI reprograms width/height/bpp over port I/O in long mode --
//!    the screen changes the instant you press Enter / click Apply/OK. The
//!    chosen mode is persisted to a lingfs file and re-applied at next boot
//!    (`restore`), so it sticks across reboots without touching stage2.
//!  * NEXT-BOOT: on non-DISPI (real VBE) hardware the mode-set is a real-mode
//!    BIOS call only stage2 can make, so we persist a preference byte in the
//!    disk-boot header that stage2 applies at the next boot. Disclosed in the
//!    row's hint, not a fake instant switch.

use crate::drivers::framebuffer;
use crate::fs::blockdev::{self, SECTOR_SIZE};

const HEADER_LBA: u32 = 17;
const HEADER_MAGIC: u32 = 0x474E_4B4C; // "LKNG" LE -- matches pack_header.py
const PREF_OFFSET: usize = 16;
const MODE_FILE: &str = "/display_mode";

/// Candidate modes, small -> large. The Settings row shows the subset the
/// card actually supports (see `fits`). 1600x900 / 1920x1080 are new and only
/// appear when the adapter reports it can drive them.
pub const CANDIDATES: [(&str, u32, u32); 7] = [
    ("640 x 480", 640, 480),
    ("800 x 600", 800, 600),
    ("1024 x 768", 1024, 768),
    ("1280 x 720", 1280, 720),
    ("1280 x 1024", 1280, 1024),
    ("1600 x 900", 1600, 900),
    ("1920 x 1080", 1920, 1080),
];

/// stage2.asm's `display_modes` order -- the next-boot header byte indexes
/// THIS table, so the persisted preference stays meaningful to the bootloader.
const STAGE2_MODES: [(u32, u32); 5] =
    [(1024, 768), (800, 600), (1280, 1024), (640, 480), (1280, 720)];

/// The ceiling a mode must fit under: the card's DISPI cap if it's a DISPI
/// adapter, else stage2's largest VBE mode (1280x1024) since that's all the
/// non-DISPI next-boot path can set.
fn max_dims() -> (u32, u32) {
    framebuffer::dispi_max().unwrap_or((1280, 1024))
}

/// Can we actually run this mode? It must fit the card's reported maximum and
/// the fixed back buffer (`framebuffer::max_pixels`).
fn fits(w: u32, h: u32) -> bool {
    let (mw, mh) = max_dims();
    w <= mw && h <= mh && (w as usize) * (h as usize) <= framebuffer::max_pixels()
}

/// The k-th *available* (card-supported) candidate.
fn nth(k: usize) -> Option<(&'static str, u32, u32)> {
    CANDIDATES.iter().filter(|m| fits(m.1, m.2)).nth(k).copied()
}

/// How many modes the Display row offers (never zero -- fall back to showing
/// the smallest candidate so the row is never empty).
pub fn mode_count() -> usize {
    CANDIDATES.iter().filter(|m| fits(m.1, m.2)).count().max(1)
}

pub fn mode_label(i: usize) -> &'static str {
    nth(i).map(|m| m.0).unwrap_or_else(|| CANDIDATES[0].0)
}

pub fn mode_dims(i: usize) -> (u32, u32) {
    nth(i).map(|m| (m.1, m.2)).unwrap_or((CANDIDATES[0].1, CANDIDATES[0].2))
}

/// Available-list index of a given (w,h), if present.
fn index_of_dims(w: u32, h: u32) -> Option<usize> {
    CANDIDATES.iter().filter(|m| fits(m.1, m.2)).position(|m| m.1 == w && m.2 == h)
}

/// In-memory selection into the *available* list, seeded on first read from
/// whatever mode is live now (so the row opens on the current resolution).
/// -1 = not yet seeded. Cycled by the arrows; the switch happens on `apply`.
static mut SEL: i32 = -1;
pub fn selected() -> usize {
    unsafe {
        if SEL < 0 {
            let (w, h) = (framebuffer::width(), framebuffer::height());
            SEL = index_of_dims(w, h).unwrap_or(0) as i32;
        }
        (SEL as usize).min(mode_count() - 1)
    }
}
pub fn set_selected(i: usize) {
    unsafe { SEL = i.min(mode_count().saturating_sub(1)) as i32 };
}

/// Apply the selected mode. On a DISPI adapter this switches live and persists
/// the mode to `MODE_FILE` so `restore` brings it back next boot; on non-DISPI
/// it writes the stage2 next-boot header index instead. Returns true if the
/// screen changed size right now.
pub fn apply(sel: usize) -> bool {
    let (w, h) = mode_dims(sel);
    let live = framebuffer::set_mode(w, h);
    if live {
        persist_mode(w, h);
    }
    // Also record a stage2 index when the mode is one the bootloader knows, so
    // an installed non-DISPI disk still comes up right at next boot.
    if let Some(idx) = STAGE2_MODES.iter().position(|m| m.0 == w && m.1 == h) {
        let _ = set_preferred(idx);
    }
    live
}

fn persist_mode(w: u32, h: u32) {
    let mut buf = [0u8; 16];
    let mut n = 0;
    n += write_u32(&mut buf[n..], w);
    buf[n] = b'x';
    n += 1;
    n += write_u32(&mut buf[n..], h);
    let _ = crate::fs::lingfs::write_file(MODE_FILE, &buf[..n]);
}

/// Re-apply the persisted live mode at desktop boot (DISPI adapters only --
/// non-DISPI relies on the stage2 header). No-op if nothing was saved or the
/// saved mode no longer fits the current card.
pub fn restore() {
    if !framebuffer::dispi_capable() {
        return;
    }
    let mut buf = [0u8; 16];
    let Ok(Some(n)) = crate::fs::lingfs::read_file_all(MODE_FILE, &mut buf) else {
        return;
    };
    let Ok(s) = core::str::from_utf8(&buf[..n]) else {
        return;
    };
    let s = s.trim();
    let Some((ws, hs)) = s.split_once('x') else {
        return;
    };
    let (Ok(w), Ok(h)) = (ws.parse::<u32>(), hs.parse::<u32>()) else {
        return;
    };
    if fits(w, h) {
        framebuffer::set_mode(w, h);
        if let Some(i) = index_of_dims(w, h) {
            set_selected(i);
        }
    }
}

/// Read the persisted stage2 preference index; 0 if no valid header.
pub fn preferred() -> usize {
    let mut sec = [0u8; SECTOR_SIZE];
    if blockdev::read_sector(HEADER_LBA, &mut sec).is_err() {
        return 0;
    }
    if u32::from_le_bytes([sec[0], sec[1], sec[2], sec[3]]) != HEADER_MAGIC {
        return 0;
    }
    (sec[PREF_OFFSET] as usize).min(STAGE2_MODES.len() - 1)
}

/// Persist a stage2 next-boot preference index. False if there's no valid
/// disk-boot header (e.g. a Live boot).
pub fn set_preferred(idx: usize) -> bool {
    if idx >= STAGE2_MODES.len() {
        return false;
    }
    let mut sec = [0u8; SECTOR_SIZE];
    if blockdev::read_sector(HEADER_LBA, &mut sec).is_err() {
        return false;
    }
    if u32::from_le_bytes([sec[0], sec[1], sec[2], sec[3]]) != HEADER_MAGIC {
        return false;
    }
    sec[PREF_OFFSET] = idx as u8;
    blockdev::write_sector(HEADER_LBA, &sec).is_ok()
}

/// Is there an installed-boot header we can persist a next-boot pref into?
pub fn persistable() -> bool {
    let mut sec = [0u8; SECTOR_SIZE];
    if blockdev::read_sector(HEADER_LBA, &mut sec).is_err() {
        return false;
    }
    u32::from_le_bytes([sec[0], sec[1], sec[2], sec[3]]) == HEADER_MAGIC
}

/// Current live framebuffer size as "WxH".
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
