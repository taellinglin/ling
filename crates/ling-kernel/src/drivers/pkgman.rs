//! Package Manager -- a windowed front end over the real `lingfu` client
//! (drivers::lingfu). It lists the synced catalog, moves a selection with the
//! arrow keys, and installs the highlighted package on Enter (`S` re-syncs).
//! The network work -- catalog sync, download, `.lpkg` unpack -- is the exact
//! real-HTTP path the Terminal's `lingfu` command uses; this file is only the
//! GUI over it. Toolchain verbs (new/build/run) need the `ling` compiler and
//! aren't offered here -- see `lingfu`'s module doc for why that's a
//! userspace/loader project, not a missing button.

use crate::drivers::{font8x8, framebuffer, image, lingfu, theme};

static mut SEL: usize = 0;
static mut SYNCED: bool = false;
/// A sync has been requested but not yet run. Set by `open()`/`S`, serviced by
/// `service()` from the WM loop one frame LATER -- so the window paints its
/// "syncing…" state and presents it before the blocking network fetch begins,
/// instead of freezing on a stale frame. See `service()`.
static mut PENDING: bool = false;
static mut STATUS: [u8; 96] = [0; 96];
static mut STATUS_LEN: usize = 0;

// -- Package icons ---------------------------------------------------------
// Each package on fu.ling-lang.org has an auto-generated avatar at
// /avatars/pkg-<name>.auto.png. We fetch them lazily -- one per frame, only
// for rows currently on screen -- decode the PNG, and downscale to a small
// square thumbnail cached per catalog index, so the list fills in
// progressively (like a web page loading its images) instead of blocking on
// 45 TLS handshakes at once. Icons are optional chrome: a failed or
// not-yet-loaded fetch just leaves a placeholder, never blocks the list.
const ICON_DIM: usize = 20;
const MAX_ICONS: usize = 64;
const ICON_EMPTY: u8 = 0;
const ICON_READY: u8 = 1;
const ICON_FAILED: u8 = 2;
static mut ICON_STATE: [u8; MAX_ICONS] = [ICON_EMPTY; MAX_ICONS];
static mut ICON_PX: [[u32; ICON_DIM * ICON_DIM]; MAX_ICONS] =
    [[0u32; ICON_DIM * ICON_DIM]; MAX_ICONS];
/// Download scratch for one avatar PNG. The registry's auto-avatars are
/// 300x300 RGBA PNGs (~84KiB), so this holds the full HTTP response.
static mut ICON_DL: [u8; 160 * 1024] = [0; 160 * 1024];
/// Viewport range recorded by `draw()` so `tick_icons()` only fetches icons
/// for rows the user can actually see right now.
static mut VIS_START: usize = 0;
static mut VIS_END: usize = 0;

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
    // The catalog changed; drop any cached icons so they re-fetch for the new
    // set of packages (indices no longer refer to the same names).
    unsafe {
        let st = &mut *&raw mut ICON_STATE;
        for s in st.iter_mut() {
            *s = ICON_EMPTY;
        }
    }
    crate::drivers::mouse::resync();
}

/// Background icon loader task. Spawned once (see `ensure_icon_task`), it runs
/// on the cooperative scheduler: it fetches one on-screen package icon at a
/// time and yields between, so the desktop keeps rendering while avatars stream
/// in. The network waits inside the fetch yield too (see timer::poll_until), so
/// even a single 84KiB download doesn't freeze the UI -- only the brief PNG
/// decode is CPU-bound.
extern "C" fn icon_task() {
    loop {
        fetch_one_icon();
        crate::proc::sched::yield_now();
    }
}

static mut ICON_TASK_SPAWNED: bool = false;

/// Spawn the background icon loader if it isn't running yet. Called by the WM
/// once the desktop loop is up (the scheduler's slot 0 must exist first).
pub fn ensure_icon_task() {
    unsafe {
        if ICON_TASK_SPAWNED {
            return;
        }
        ICON_TASK_SPAWNED = true;
    }
    crate::proc::sched::spawn(icon_task as *const () as usize as u64);
}

/// Fetch and cache the next not-yet-loaded on-screen package icon, if any.
/// Runs in the background task; a cheap no-op when the Packages catalog isn't
/// synced or every visible icon is already loaded.
fn fetch_one_icon() {
    if !unsafe { SYNCED } {
        return;
    }
    let count = line_count();
    if count == 0 {
        return;
    }
    let (start, end) = unsafe { (VIS_START, VIS_END.min(count).min(MAX_ICONS)) };
    // Find the next visible package whose icon hasn't been fetched yet.
    let mut target = None;
    let mut i = start;
    while i < end {
        if unsafe { ICON_STATE[i] } == ICON_EMPTY {
            target = Some(i);
            break;
        }
        i += 1;
    }
    let Some(idx) = target else { return };
    let Some(line) = nth_line(idx) else { return };
    let name = field(line, 0);

    // Build "/avatars/pkg-<name>.auto.png".
    let mut path = [0u8; 96];
    let prefix = b"/avatars/pkg-";
    let suffix = b".auto.png";
    let mut p = 0usize;
    for &b in prefix {
        path[p] = b;
        p += 1;
    }
    for &b in name {
        if p + suffix.len() + 1 >= path.len() {
            break;
        }
        path[p] = b;
        p += 1;
    }
    for &b in suffix {
        path[p] = b;
        p += 1;
    }
    let Ok(pathstr) = core::str::from_utf8(&path[..p]) else {
        unsafe { ICON_STATE[idx] = ICON_FAILED };
        return;
    };

    let dl = unsafe { &mut *&raw mut ICON_DL };
    // Decode into ICON_PX first, then flip the state to READY as the last
    // step -- the draw task only reads the pixels once state is READY, and no
    // yield happens between the decode and the flip, so it never sees a
    // half-written thumbnail.
    let ok = match lingfu::fetch_official(pathstr, dl) {
        Some(len) if len > 8 => decode_into_icon(&dl[..len], idx),
        _ => false,
    };
    unsafe { ICON_STATE[idx] = if ok { ICON_READY } else { ICON_FAILED } };
}

/// Decode a PNG avatar and nearest-neighbor downscale it into icon slot `idx`.
fn decode_into_icon(png: &[u8], idx: usize) -> bool {
    let Some(img) = image::decode_png(png) else { return false };
    if img.w == 0 || img.h == 0 {
        return false;
    }
    let dst = unsafe { &mut (&mut *&raw mut ICON_PX)[idx] };
    for oy in 0..ICON_DIM {
        let sy = (oy as u32 * img.h / ICON_DIM as u32).min(img.h - 1);
        for ox in 0..ICON_DIM {
            let sx = (ox as u32 * img.w / ICON_DIM as u32).min(img.w - 1);
            dst[oy * ICON_DIM + ox] = img.px[(sy * img.w + sx) as usize];
        }
    }
    true
}

/// Blit icon slot `idx` (if ready) at back-buffer (x,y). Returns true if drawn.
fn blit_icon(idx: usize, x: u32, y: u32) -> bool {
    if idx >= MAX_ICONS || unsafe { ICON_STATE[idx] } != ICON_READY {
        return false;
    }
    let src = unsafe { &(&*&raw const ICON_PX)[idx] };
    for oy in 0..ICON_DIM as u32 {
        for ox in 0..ICON_DIM as u32 {
            framebuffer::back_set_pixel(x + ox, y + oy, src[(oy as usize) * ICON_DIM + ox as usize]);
        }
    }
    true
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

    // Record the visible range so tick_icons() only fetches on-screen icons.
    unsafe {
        VIS_START = start;
        VIS_END = (start + rows).min(count);
    }

    // Icon column: a square inset into each row, text shifted right past it.
    let icon_pad = 4u32;
    let text_x = x + ICON_DIM as u32 + icon_pad * 2;

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
            // Icon (or a placeholder tile until it loads / if it failed).
            let iy = ry.saturating_sub(1);
            if !blit_icon(i, x, iy) {
                framebuffer::back_fill_rounded_rect(x, iy, ICON_DIM as u32, ICON_DIM as u32, 4, dim);
                framebuffer::back_fill_rounded_rect(
                    x + 1,
                    iy + 1,
                    ICON_DIM as u32 - 2,
                    ICON_DIM as u32 - 2,
                    4,
                    rowbg,
                );
            }
            let name = field(line, 0);
            let ver = field(line, 1);
            font8x8::draw_str(text_x, ry, name, if i == sel { accent } else { text }, rowbg);
            font8x8::draw_str(text_x + (name.len() as u32 + 1) * 8, ry, ver, dim, rowbg);
            let d = desc(line);
            let maxd = (w.saturating_sub(text_x - x + 8) / 8) as usize;
            font8x8::draw_str(text_x, ry + 12, &d[..d.len().min(maxd)], dim, rowbg);
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
