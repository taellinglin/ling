//! Package Manager -- a windowed front end over the real `lingfu` client
//! (drivers::lingfu). It lists the synced catalog, moves a selection with the
//! arrow keys, and installs the highlighted package on Enter (`S` re-syncs).
//! The network work -- catalog sync, download, `.lpkg` unpack -- is the exact
//! real-HTTP path the Terminal's `lingfu` command uses; this file is only the
//! GUI over it. Toolchain verbs (new/build/run) need the `ling` compiler and
//! aren't offered here -- see `lingfu`'s module doc for why that's a
//! userspace/loader project, not a missing button.

use crate::drivers::{font8x8, framebuffer, lingfu, theme};

static mut SEL: usize = 0;
static mut SYNCED: bool = false;
static mut STATUS: [u8; 96] = [0; 96];
static mut STATUS_LEN: usize = 0;

fn set_status(s: &[u8]) {
    unsafe {
        let st = &mut *&raw mut STATUS;
        let n = s.len().min(st.len());
        st[..n].copy_from_slice(&s[..n]);
        STATUS_LEN = n;
    }
}

/// Called when the window opens: sync the catalog once (bounded HTTP -- the
/// DNS/ARP fast-fail path keeps this from hanging when no repo is reachable).
pub fn open() {
    unsafe {
        SEL = 0;
        if SYNCED {
            return;
        }
        SYNCED = true;
    }
    lingfu::begin_capture();
    let n = lingfu::sync();
    lingfu::end_capture();
    if n == 0 {
        set_status(b"no catalog -- is a repo reachable? (default 10.0.2.2:8000)");
    } else {
        set_status(b"catalog synced -- up/down to select, Enter to install");
    }
}

fn line_count() -> usize {
    lingfu::catalog_raw().split(|&b| b == b'\n').filter(|l| !l.is_empty()).count()
}

fn nth_line(i: usize) -> Option<&'static [u8]> {
    lingfu::catalog_raw().split(|&b| b == b'\n').filter(|l| !l.is_empty()).nth(i)
}

/// The `idx`-th space-separated field of a catalog line ("name version
/// filename desc...").
fn field(line: &[u8], idx: usize) -> &[u8] {
    line.split(|&b| b == b' ').filter(|f| !f.is_empty()).nth(idx).unwrap_or(b"")
}

/// Everything after the first three fields (name/version/filename) -- the
/// human description.
fn desc(line: &[u8]) -> &[u8] {
    let mut i = 0;
    let mut fields = 0;
    while fields < 3 {
        while i < line.len() && line[i] == b' ' {
            i += 1;
        }
        while i < line.len() && line[i] != b' ' {
            i += 1;
        }
        fields += 1;
    }
    while i < line.len() && line[i] == b' ' {
        i += 1;
    }
    &line[i..]
}

pub fn draw(x: u32, y: u32, w: u32, h: u32) {
    let text = theme::color(theme::SLOT_TEXT);
    let dim = theme::color(theme::SLOT_DIM);
    let panel = theme::color(theme::SLOT_PANEL);
    let accent = theme::color(theme::SLOT_ACCENT);
    let hl = theme::color(theme::SLOT_PANEL_BORDER);

    font8x8::draw_str(x, y, b"lingfu packages", accent, panel);
    font8x8::draw_str(x, y + 14, b"up/down select   Enter install   S resync", dim, panel);

    let count = line_count();
    let list_y = y + 36;
    let row_h = 28u32;
    let avail = h.saturating_sub(36 + 22);
    let rows = (avail / row_h).max(1) as usize;
    let sel = unsafe { SEL.min(count.saturating_sub(1)) };
    let start = if sel >= rows { sel + 1 - rows } else { 0 };

    if count == 0 {
        font8x8::draw_str(x, list_y, b"(no packages -- press S to sync a reachable repo)", dim, panel);
    }
    let mut r = 0;
    while r < rows {
        let i = start + r;
        if i >= count {
            break;
        }
        if let Some(line) = nth_line(i) {
            let ry = list_y + r as u32 * row_h;
            let rowbg = if i == sel { hl } else { panel };
            framebuffer::back_fill_rounded_rect(
                x.saturating_sub(4),
                ry.saturating_sub(2),
                w.saturating_sub(8),
                row_h - 2,
                5,
                rowbg,
            );
            let name = field(line, 0);
            let ver = field(line, 1);
            font8x8::draw_str(x, ry, name, if i == sel { accent } else { text }, rowbg);
            font8x8::draw_str(x + (name.len() as u32 + 1) * 8, ry, ver, dim, rowbg);
            let d = desc(line);
            let maxd = (w.saturating_sub(16) / 8) as usize;
            font8x8::draw_str(x, ry + 12, &d[..d.len().min(maxd)], dim, rowbg);
        }
        r += 1;
    }

    let st = unsafe { &(&*&raw const STATUS)[..STATUS_LEN] };
    font8x8::draw_str(x, y + h.saturating_sub(16), st, dim, panel);
}

fn install_selected() {
    let count = line_count();
    if count == 0 {
        set_status(b"nothing to install -- sync a repo first (S)");
        return;
    }
    let sel = unsafe { SEL.min(count - 1) };
    let Some(line) = nth_line(sel) else { return };
    let name = field(line, 0);
    let mut nb = [0u8; 64];
    let n = name.len().min(nb.len());
    nb[..n].copy_from_slice(&name[..n]);
    let Ok(namestr) = core::str::from_utf8(&nb[..n]) else { return };
    set_status(b"installing...");
    lingfu::begin_capture();
    let _ = lingfu::install(namestr);
    let cap = lingfu::end_capture();
    // Surface the last thing lingfu said as the status line.
    let last = cap.split(|&b| b == b'\n').filter(|l| !l.is_empty()).last().unwrap_or(b"install done");
    set_status(last);
}

pub fn key(k: u8) {
    let count = line_count();
    match k {
        0x11 => unsafe { SEL = SEL.saturating_sub(1) }, // up
        0x12 => unsafe {
            if SEL + 1 < count {
                SEL += 1;
            }
        }, // down
        b'\n' | b'\r' => install_selected(), // enter
        b's' | b'S' => {
            unsafe { SYNCED = false };
            open();
        },
        _ => {},
    }
}
