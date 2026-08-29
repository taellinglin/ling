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
/// A sync has been requested but not yet run. Set by `open()`/`S`, serviced by
/// `service()` from the WM loop one frame LATER -- so the window paints its
/// "syncing…" state and presents it before the blocking network fetch begins,
/// instead of freezing on a stale frame. See `service()`.
static mut PENDING: bool = false;
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

/// Called when the window opens: request a one-time catalog sync. The sync
/// itself is deferred to `service()` (next frame) so the window can paint a
/// "syncing…" state first -- a multi-second TLS fetch run inline here would
/// freeze the desktop on a blank frame and, worse, drop enough PS/2 mouse
/// bytes to desync the cursor. Bounded HTTP; DNS/ARP fast-fail if no repo.
pub fn open() {
    unsafe { SEL = 0 };
    if unsafe { SYNCED } {
        return;
    }
    set_status(b"syncing catalog from fu.ling-lang.org ...");
    unsafe { PENDING = true };
}

/// Run a pending catalog sync, if any. Called once per frame by the WM loop
/// AFTER the previous frame (showing "syncing…") was presented, so the
/// blocking network fetch never freezes a stale screen. Cheap no-op when
/// nothing is pending. Resyncs the mouse afterward: the fetch holds the loop
/// off the CPU long enough to drop PS/2 bytes, so the packet framing needs
/// realigning or the cursor jumps erratically.
pub fn service() {
    if !unsafe { PENDING } {
        return;
    }
    unsafe {
        PENDING = false;
        SYNCED = true;
    }
    lingfu::begin_capture();
    let n = lingfu::sync();
    lingfu::end_capture();
    if n == 0 {
        set_status(b"no packages -- is a repo reachable? (S to retry)");
    } else {
        set_status(b"catalog synced -- up/down select, Enter install, S resync");
    }
    crate::drivers::mouse::resync();
}

fn line_count() -> usize {
    lingfu::catalog_raw().split(|&b| b == b'\n').filter(|l| !l.is_empty()).count()
}

fn nth_line(i: usize) -> Option<&'static [u8]> {
    lingfu::catalog_raw().split(|&b| b == b'\n').filter(|l| !l.is_empty()).nth(i)
}

/// The `idx`-th TAB-delimited field of a catalog line
/// ("name \t meta \t filename \t description"). Empty fields are preserved so
/// indices stay stable (a package with no filename still has a description at
/// index 3).
fn field(line: &[u8], idx: usize) -> &[u8] {
    line.split(|&b| b == b'\t').nth(idx).unwrap_or(b"")
}

/// The human description -- the 4th field.
fn desc(line: &[u8]) -> &[u8] {
    field(line, 3)
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
    // The download blocked the loop; realign the mouse packet framing.
    crate::drivers::mouse::resync();
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
            // Request a fresh sync; service() runs it next frame (see open()).
            unsafe { SYNCED = false };
            open();
        },
        _ => {},
    }
}
