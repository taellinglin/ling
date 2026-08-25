//! Multi-window manager core -- the real window system behind the
//! `kernel/x86_64-wm` desktop. Generalizes `wm_liquid`'s single springy
//! rect into a registry of windows with z-order, focus, titlebar dragging,
//! close buttons, minimize-to-dock, a dock with launchers, and per-window
//! damped-oscillator "liquid" motion (same constants and feel as
//! `wm_liquid`, now per-window).
//!
//! Division of labour, same philosophy as before: *state and hit-testing*
//! live here (the `.ling` AOT path has no mutable rebinding -- see
//! `wm_liquid`'s module doc), while *what the chrome looks like* stays in
//! `.ling` (`kernel/x86_64-wm/main.ling` draws every shadow/frame/titlebar
//! itself from the slot getters below). The one exception is window
//! *content* (Settings rows, file listings): those need real loops over
//! runtime-sized data, which `.ling` kernel code cannot express (its loops
//! can't carry a counter), so [`draw_content`] renders window interiors
//! kernel-side.
//!
//! Not claimed: this is not a client/server display protocol. Windows are
//! kernel-side objects drawn by the kernel; there is no per-process
//! surface, no damage protocol, no IPC. The ring-3 process layer
//! (`proc/uproc.rs`) exists but `SYS_FB_MAP`/`SYS_POLL_INPUT` are still
//! ENOSYS -- until a process can map a surface and receive input events,
//! an X-style split would be theater. This is the honest single-address-
//! space ancestor of that design, shaped so windows could become client
//! surfaces later (each window already has its own identity, geometry,
//! and focus state).

use crate::arch::{rtc, timer};
use crate::drivers::{framebuffer, font8x8, font_unicode, kbdlayout, locale, theme};
use crate::fs::lingfs;

// Same spring feel as wm_liquid.rs, applied per window.
const SPRING_K: f64 = 0.028;
const DAMPING: f64 = 0.22;
const STRETCH_SENSITIVITY: f64 = 0.006;
const MAX_STRETCH: f64 = 0.35;

pub const MAX_WINDOWS: usize = 6;
pub const TITLEBAR_H: u32 = 28;

pub const KIND_ABOUT: u8 = 0;
pub const KIND_SETTINGS: u8 = 1;
pub const KIND_FILES: u8 = 2;
const DOCK_APPS: [u8; 3] = [KIND_ABOUT, KIND_SETTINGS, KIND_FILES];

#[derive(Clone, Copy)]
struct Window {
    used: bool,
    minimized: bool,
    kind: u8,
    w: u32,
    h: u32,
    // Spring state: rendered position chases target.
    px: f64,
    py: f64,
    vx: f64,
    vy: f64,
    tx: f64,
    ty: f64,
    scale_w: f64,
    scale_h: f64,
}

const EMPTY_WINDOW: Window = Window {
    used: false,
    minimized: false,
    kind: 0,
    w: 0,
    h: 0,
    px: 0.0,
    py: 0.0,
    vx: 0.0,
    vy: 0.0,
    tx: 0.0,
    ty: 0.0,
    scale_w: 1.0,
    scale_h: 1.0,
};

static mut WINDOWS: [Window; MAX_WINDOWS] = [EMPTY_WINDOW; MAX_WINDOWS];
/// Z-order as indices into WINDOWS: `Z[0]` backmost, `Z[Z_LEN-1]` topmost
/// (focused). Minimized windows stay in the table but leave the z list.
static mut Z: [usize; MAX_WINDOWS] = [0; MAX_WINDOWS];
static mut Z_LEN: usize = 0;

static mut FRAME_NO: u32 = 0;
static mut DRAG_WIN: i32 = -1;
static mut GRAB_DX: f64 = 0.0;
static mut GRAB_DY: f64 = 0.0;
static mut PREV_BUTTONS: u8 = 0;
static mut LAST_STEP_MS: u64 = 0;
static mut SPAWN_COUNT: u32 = 0;
static mut HOVER_DOCK: i32 = -1;

// -- Settings window state -------------------------------------------------
const SETTINGS_ROWS: usize = 4; // UI theme / keyboard / language / clock
static mut SETTINGS_CURSOR: usize = 0;
static mut CLOCK_24H: bool = true;

// -- Files window state ----------------------------------------------------
const FM_NAME_MAX: usize = 60;
static mut FM_DIR: [u8; FM_NAME_MAX] = [0; FM_NAME_MAX]; // "" = root
static mut FM_DIR_LEN: usize = 0;
static mut FM_CURSOR: usize = 0;
static mut FM_SCROLL: usize = 0;
static mut FM_VIEWING: bool = false; // true = file-content view
static mut FM_VIEW_NAME: [u8; FM_NAME_MAX * 2 + 1] = [0; FM_NAME_MAX * 2 + 1];
static mut FM_VIEW_NAME_LEN: usize = 0;
const FM_VISIBLE_ROWS: usize = 10;

fn windows() -> &'static mut [Window; MAX_WINDOWS] {
    unsafe { &mut *&raw mut WINDOWS }
}

/// Serial hex logger (same shape as net_e1000's private one -- console
/// output mirrors to serial regardless of display mode, so this is
/// screendump-independent diagnostics for the input path).
fn log_hex(label: &[u8], v: u32) {
    crate::console_write(label);
    let hex = b"0123456789ABCDEF";
    let mut buf = [0u8; 8];
    for i in 0..8 {
        buf[i] = hex[((v >> ((7 - i) * 4)) & 0xF) as usize];
    }
    crate::console_write(&buf);
    crate::console_write(b"\n");
}

fn kind_title(kind: u8) -> &'static str {
    match kind {
        KIND_ABOUT => "About LingOS",
        KIND_SETTINGS => "Settings",
        KIND_FILES => "Files",
        _ => "?",
    }
}

fn kind_size(kind: u8) -> (u32, u32) {
    // Real pixels, scaled off the framebuffer so windows stay proportionate
    // (ui_scale's virtual canvas is for full-screen layouts; windows scale
    // against screen height directly).
    let s = (framebuffer::height() as f64 / 600.0).max(0.5);
    let (w, h) = match kind {
        KIND_SETTINGS => (470.0, 330.0),
        KIND_FILES => (470.0, 370.0),
        _ => (390.0, 250.0),
    };
    ((w * s) as u32, (h * s) as u32)
}

pub fn dock_letter(i: usize) -> &'static str {
    match DOCK_APPS.get(i) {
        Some(&KIND_ABOUT) => "i",
        Some(&KIND_SETTINGS) => "S",
        Some(&KIND_FILES) => "F",
        _ => "",
    }
}

// -- Dock geometry (real pixels, computed from the live fb size) -----------

pub fn dock_count() -> usize {
    DOCK_APPS.len()
}

pub fn dock_icon_size() -> u32 {
    (framebuffer::height() / 14).max(32)
}

fn dock_gap() -> u32 {
    dock_icon_size() / 4
}

pub fn dock_y() -> u32 {
    framebuffer::height().saturating_sub(dock_icon_size() + dock_icon_size() / 2)
}

pub fn dock_x(i: usize) -> u32 {
    let n = DOCK_APPS.len() as u32;
    let total = n * dock_icon_size() + (n - 1) * dock_gap();
    let start = (framebuffer::width().saturating_sub(total)) / 2;
    start + i as u32 * (dock_icon_size() + dock_gap())
}

pub fn dock_hover(i: usize) -> bool {
    unsafe { HOVER_DOCK == i as i32 }
}

/// Is a window of this dock slot's kind open (running indicator dot)?
pub fn dock_running(i: usize) -> bool {
    let Some(&kind) = DOCK_APPS.get(i) else { return false };
    windows().iter().any(|w| w.used && w.kind == kind)
}

fn find_kind(kind: u8) -> Option<usize> {
    windows().iter().position(|w| w.used && w.kind == kind)
}

fn z_remove(idx: usize) {
    unsafe {
        let z = &mut *&raw mut Z;
        let mut j = 0;
        for i in 0..Z_LEN {
            if z[i] != idx {
                z[j] = z[i];
                j += 1;
            }
        }
        Z_LEN = j;
    }
}

fn z_raise(idx: usize) {
    z_remove(idx);
    unsafe {
        let z = &mut *&raw mut Z;
        if Z_LEN < MAX_WINDOWS {
            z[Z_LEN] = idx;
            Z_LEN += 1;
        }
    }
}

pub fn open(kind: u8) {
    if let Some(idx) = find_kind(kind) {
        windows()[idx].minimized = false;
        z_raise(idx);
        return;
    }
    let Some(idx) = windows().iter().position(|w| !w.used) else { return };
    let (w, h) = kind_size(kind);
    let cascade = unsafe {
        SPAWN_COUNT = SPAWN_COUNT.wrapping_add(1);
        (SPAWN_COUNT % 5) as f64 * 36.0
    };
    let cx = (framebuffer::width().saturating_sub(w) / 2) as f64 + cascade;
    // Never spawn under the top bar (30px panel + 1px divider).
    let cy = ((framebuffer::height().saturating_sub(h) / 3) as f64 + cascade).max(40.0);
    windows()[idx] = Window {
        used: true,
        minimized: false,
        kind,
        w,
        h,
        // Spawn springing up from the dock: start slightly below and
        // scaled-down feel comes free from the spring overshoot.
        px: cx,
        py: cy + 60.0,
        vx: 0.0,
        vy: -0.8,
        tx: cx,
        ty: cy,
        scale_w: 1.0,
        scale_h: 1.0,
    };
    z_raise(idx);
}

pub fn close(idx: usize) {
    if idx < MAX_WINDOWS && windows()[idx].used {
        windows()[idx].used = false;
        z_remove(idx);
        unsafe {
            if DRAG_WIN == idx as i32 {
                DRAG_WIN = -1;
            }
        }
    }
}

fn minimize(idx: usize) {
    if idx < MAX_WINDOWS && windows()[idx].used {
        windows()[idx].minimized = true;
        z_remove(idx);
        unsafe {
            if DRAG_WIN == idx as i32 {
                DRAG_WIN = -1;
            }
        }
    }
}

/// Deformed (squash/stretch-applied) rect of a window, centered on its own
/// middle -- the same center-scale the old `.ling` loop computed by hand.
fn deformed_rect(w: &Window) -> (i64, i64, u32, u32) {
    let dw = (w.w as f64 * w.scale_w) as i64;
    let dh = (w.h as f64 * w.scale_h) as i64;
    let x = w.px as i64 - (dw - w.w as i64) / 2;
    let y = w.py as i64 - (dh - w.h as i64) / 2;
    (x, y, dw.max(0) as u32, dh.max(0) as u32)
}

fn hit_window(w: &Window, mx: i64, my: i64) -> bool {
    let (x, y, dw, dh) = deformed_rect(w);
    mx >= x && mx < x + dw as i64 && my >= y && my < y + dh as i64
}

fn hit_titlebar(w: &Window, mx: i64, my: i64) -> bool {
    let (x, y, dw, _) = deformed_rect(w);
    mx >= x && mx < x + dw as i64 && my >= y && my < y + TITLEBAR_H as i64
}

fn hit_close(w: &Window, mx: i64, my: i64) -> bool {
    // Close button: right end of the titlebar, a TITLEBAR_H-sized square.
    let (x, y, dw, _) = deformed_rect(w);
    let bx = x + dw as i64 - TITLEBAR_H as i64;
    mx >= bx && mx < x + dw as i64 && my >= y && my < y + TITLEBAR_H as i64
}

fn hit_minimize(w: &Window, mx: i64, my: i64) -> bool {
    let (x, y, dw, _) = deformed_rect(w);
    let bx = x + dw as i64 - 2 * TITLEBAR_H as i64;
    mx >= bx && mx < x + dw as i64 - TITLEBAR_H as i64 && my >= y && my < y + TITLEBAR_H as i64
}

fn dock_hit(mx: i64, my: i64) -> i32 {
    let size = dock_icon_size() as i64;
    let y = dock_y() as i64;
    if my < y || my >= y + size {
        return -1;
    }
    for i in 0..DOCK_APPS.len() {
        let x = dock_x(i) as i64;
        if mx >= x && mx < x + size {
            return i as i32;
        }
    }
    -1
}

/// One frame of window-system logic: click routing (dock, close/minimize
/// buttons, titlebar drag starts, raise-on-click), drag tracking, and the
/// per-window spring advance. Call once per frame from the `.ling` loop,
/// before reading any slot getter.
pub fn step(mx: i64, my: i64, buttons: u8) {
    unsafe {
        // Serial diagnostic, once every 512 frames: proves whether IRQ12
        // bytes are flowing and where the driver thinks the cursor is --
        // the framebuffer regression lesson applied to input (a dead mouse
        // looks identical to a hit-test bug on a screendump).
        FRAME_NO = FRAME_NO.wrapping_add(1);
        if FRAME_NO % 512 == 0 {
            log_hex(b"wm: mouse bytes=0x", crate::drivers::mouse::byte_count());
            log_hex(b"wm: last=0x", crate::drivers::mouse::last_byte() as u32);
            // 8042 status: bit0 OBF, bit5 aux-origin -- OBF stuck high with
            // a flat byte count means data is waiting but IRQ12 never
            // fires; OBF low means the mouse isn't streaming at all.
            log_hex(b"wm: 8042=0x", crate::arch::io::inb(0x64) as u32);
            log_hex(b"wm: mx=0x", mx as u32);
        }
        let left = buttons & 1 != 0;
        let was = PREV_BUTTONS & 1 != 0;
        PREV_BUTTONS = buttons;

        HOVER_DOCK = dock_hit(mx, my);

        if left && !was {
            // Press edge: route the click.
            let dock = HOVER_DOCK;
            if dock >= 0 {
                let kind = DOCK_APPS[dock as usize];
                match find_kind(kind) {
                    Some(idx) if windows()[idx].minimized => {
                        windows()[idx].minimized = false;
                        z_raise(idx);
                    },
                    Some(idx) if Z_LEN > 0 && Z[Z_LEN - 1] == idx => minimize(idx),
                    Some(idx) => z_raise(idx),
                    None => open(kind),
                }
            } else {
                // Topmost window under the cursor wins.
                let z = &*&raw const Z;
                let mut clicked: i32 = -1;
                for zi in (0..Z_LEN).rev() {
                    let idx = z[zi];
                    if hit_window(&windows()[idx], mx, my) {
                        clicked = idx as i32;
                        break;
                    }
                }
                if clicked >= 0 {
                    let idx = clicked as usize;
                    if hit_close(&windows()[idx], mx, my) {
                        close(idx);
                    } else if hit_minimize(&windows()[idx], mx, my) {
                        minimize(idx);
                    } else {
                        z_raise(idx);
                        if hit_titlebar(&windows()[idx], mx, my) {
                            DRAG_WIN = idx as i32;
                            GRAB_DX = mx as f64 - windows()[idx].px;
                            GRAB_DY = my as f64 - windows()[idx].py;
                        }
                    }
                }
            }
        }

        if !left {
            DRAG_WIN = -1;
        }
        if DRAG_WIN >= 0 {
            let idx = DRAG_WIN as usize;
            let w = windows()[idx].w as f64;
            // Clamp so the titlebar always stays reachable: below the top
            // bar, at least 60px of window on-screen horizontally, and the
            // titlebar itself never below the bottom edge.
            let max_x = framebuffer::width() as f64 - 60.0;
            let max_y = framebuffer::height() as f64 - TITLEBAR_H as f64;
            windows()[idx].tx = (mx as f64 - GRAB_DX).max(60.0 - w).min(max_x);
            windows()[idx].ty = (my as f64 - GRAB_DY).max(34.0).min(max_y);
        }

        // Spring advance, shared dt (same clamp rationale as wm_liquid).
        let now = timer::now_ms();
        let dt = if LAST_STEP_MS == 0 { 16 } else { now.saturating_sub(LAST_STEP_MS).min(50) };
        LAST_STEP_MS = now;
        let dt = dt as f64;
        if dt > 0.0 {
            for w in windows().iter_mut() {
                if !w.used {
                    continue;
                }
                let ax = (w.tx - w.px) * SPRING_K - w.vx * DAMPING;
                let ay = (w.ty - w.py) * SPRING_K - w.vy * DAMPING;
                w.vx += ax * dt;
                w.vy += ay * dt;
                w.px += w.vx * dt;
                w.py += w.vy * dt;
                let sx = (w.vx.abs() * STRETCH_SENSITIVITY).min(MAX_STRETCH);
                let sy = (w.vy.abs() * STRETCH_SENSITIVITY).min(MAX_STRETCH);
                w.scale_w = 1.0 + sx - sy * 0.5;
                w.scale_h = 1.0 + sy - sx * 0.5;
            }
        }
    }
}

// -- Slot getters for the .ling draw loop (z position, back-to-front) ------

pub fn slot_count() -> usize {
    unsafe { Z_LEN }
}

fn slot_window(slot: usize) -> Option<&'static Window> {
    unsafe {
        if slot >= Z_LEN {
            return None;
        }
        let idx = Z[slot];
        let w = &windows()[idx];
        if w.used && !w.minimized {
            Some(w)
        } else {
            None
        }
    }
}

pub fn slot_x(slot: usize) -> i64 {
    slot_window(slot).map(|w| deformed_rect(w).0).unwrap_or(0)
}
pub fn slot_y(slot: usize) -> i64 {
    slot_window(slot).map(|w| deformed_rect(w).1).unwrap_or(0)
}
pub fn slot_w(slot: usize) -> u32 {
    slot_window(slot).map(|w| deformed_rect(w).2).unwrap_or(0)
}
pub fn slot_h(slot: usize) -> u32 {
    slot_window(slot).map(|w| deformed_rect(w).3).unwrap_or(0)
}
pub fn slot_kind(slot: usize) -> u8 {
    slot_window(slot).map(|w| w.kind).unwrap_or(255)
}
pub fn slot_title(slot: usize) -> &'static str {
    slot_window(slot).map(|w| kind_title(w.kind)).unwrap_or("")
}
pub fn slot_focused(slot: usize) -> bool {
    unsafe { Z_LEN > 0 && slot == Z_LEN - 1 && slot_window(slot).is_some() }
}

// -- Keyboard routing ------------------------------------------------------

/// Route one key to the focused window. Arrow keys arrive as the keyboard
/// driver's DC1/DC2/etc. control bytes, same encoding the pickers use
/// (0x11 up, 0x12 down, 0x13 left, 0x14 right per `drivers/keyboard.rs`).
pub fn key(k: u8) {
    let focused_kind = unsafe {
        if Z_LEN == 0 {
            return;
        }
        windows()[Z[Z_LEN - 1]].kind
    };
    match focused_kind {
        KIND_SETTINGS => settings_key(k),
        KIND_FILES => files_key(k),
        _ => {},
    }
}

fn settings_key(k: u8) {
    unsafe {
        match k {
            0x11 => SETTINGS_CURSOR = SETTINGS_CURSOR.saturating_sub(1),
            0x12 => SETTINGS_CURSOR = (SETTINGS_CURSOR + 1).min(SETTINGS_ROWS - 1),
            0x13 | 0x14 => {
                let dir: i32 = if k == 0x13 { -1 } else { 1 };
                match SETTINGS_CURSOR {
                    0 => {
                        let n = theme::count() as i32;
                        let cur = theme::current() as i32;
                        theme::set(((cur + dir + n) % n) as usize);
                    },
                    1 => {
                        let n = kbdlayout::count() as i32;
                        let cur = kbdlayout::current() as i32;
                        kbdlayout::set_current(((cur + dir + n) % n) as usize);
                    },
                    2 => {
                        let n = locale::count() as i32;
                        let cur = locale::selected().unwrap_or(0) as i32;
                        locale::select(((cur + dir + n) % n) as usize);
                    },
                    _ => CLOCK_24H = !CLOCK_24H,
                }
            },
            _ => {},
        }
    }
}

fn fm_dir() -> &'static str {
    unsafe {
        core::str::from_utf8(&(&*&raw const FM_DIR)[..FM_DIR_LEN]).unwrap_or("")
    }
}

fn fm_entry_count() -> usize {
    let mut n = 0;
    let mut buf = [0u8; FM_NAME_MAX];
    while n < 64 {
        if lingfs::list_entry(fm_dir(), n, &mut buf).is_none() {
            break;
        }
        n += 1;
    }
    n
}

fn files_key(k: u8) {
    unsafe {
        if FM_VIEWING {
            // Any of backspace/left/enter leaves the file view.
            if k == 0x08 || k == 0x13 || k == 10 {
                FM_VIEWING = false;
            }
            return;
        }
        let count = fm_entry_count();
        match k {
            0x11 => FM_CURSOR = FM_CURSOR.saturating_sub(1),
            0x12 => {
                if count > 0 {
                    FM_CURSOR = (FM_CURSOR + 1).min(count - 1);
                }
            },
            0x08 | 0x13 => {
                // Up to root.
                FM_DIR_LEN = 0;
                FM_CURSOR = 0;
                FM_SCROLL = 0;
            },
            10 => {
                let mut name = [0u8; FM_NAME_MAX];
                let Some((len, is_dir)) = lingfs::list_entry(fm_dir(), FM_CURSOR, &mut name)
                else {
                    return;
                };
                if is_dir && FM_DIR_LEN == 0 {
                    // One level deep only -- same real limit as lingfs's own
                    // path model (`write_in_dir` is dir/file, not deeper).
                    let fm = &mut *&raw mut FM_DIR;
                    fm[..len].copy_from_slice(&name[..len]);
                    FM_DIR_LEN = len;
                    FM_CURSOR = 0;
                    FM_SCROLL = 0;
                } else if !is_dir {
                    // Compose "dir/name" (or just "name" at root) for view.
                    let vn = &mut *&raw mut FM_VIEW_NAME;
                    let mut off = 0;
                    if FM_DIR_LEN > 0 {
                        vn[..FM_DIR_LEN].copy_from_slice(&(&*&raw const FM_DIR)[..FM_DIR_LEN]);
                        off = FM_DIR_LEN;
                        vn[off] = b'/';
                        off += 1;
                    }
                    vn[off..off + len].copy_from_slice(&name[..len]);
                    FM_VIEW_NAME_LEN = off + len;
                    FM_VIEWING = true;
                }
            },
            _ => {},
        }
        if FM_CURSOR < FM_SCROLL {
            FM_SCROLL = FM_CURSOR;
        }
        if FM_CURSOR >= FM_SCROLL + FM_VISIBLE_ROWS {
            FM_SCROLL = FM_CURSOR - FM_VISIBLE_ROWS + 1;
        }
    }
}

// -- Clock -----------------------------------------------------------------

static mut CLOCK_BUF: [u8; 16] = [0; 16];

/// "HH:MM" (or "hh:MM am/pm") in the selected locale's UTC offset --
/// the real CMOS clock through the real locale table, not a placeholder.
pub fn clock_str() -> &'static str {
    let ts = rtc::unix_timestamp();
    let offset_min = locale::selected()
        .and_then(locale::get)
        .map(|l| l.utc_offset_min)
        .unwrap_or(0) as i64;
    let local = ts + offset_min * 60;
    let mins_of_day = ((local % 86400 + 86400) % 86400) / 60;
    let (h24, m) = (mins_of_day / 60, mins_of_day % 60);
    unsafe {
        let buf = &mut *&raw mut CLOCK_BUF;
        let (h, suffix): (i64, &[u8]) = if CLOCK_24H {
            (h24, b"")
        } else {
            let h12 = if h24 % 12 == 0 { 12 } else { h24 % 12 };
            (h12, if h24 < 12 { b" am" } else { b" pm" })
        };
        buf[0] = b'0' + (h / 10) as u8;
        buf[1] = b'0' + (h % 10) as u8;
        buf[2] = b':';
        buf[3] = b'0' + (m / 10) as u8;
        buf[4] = b'0' + (m % 10) as u8;
        let mut len = 5;
        for &b in suffix {
            buf[len] = b;
            len += 1;
        }
        core::str::from_utf8(&buf[..len]).unwrap_or("00:00")
    }
}

/// Vertical-gradient wallpaper between the theme's two wallpaper slots --
/// per-row linear interpolation, which is exactly the kind of counter-
/// carrying loop `.ling` kernel code can't write, so the desktop calls
/// this instead of `fb_back_clear`.
pub fn draw_wallpaper() {
    let h = framebuffer::height();
    let w = framebuffer::width();
    if h == 0 {
        return;
    }
    let top = theme::color(theme::SLOT_WALL_TOP);
    let bottom = theme::color(theme::SLOT_WALL_BOTTOM);
    let denom = (h - 1).max(1) as u64;
    for row in 0..h {
        let mut color = 0u32;
        for ch in 0..3 {
            let a = ((top >> (ch * 8)) & 0xFF) as u64;
            let b = ((bottom >> (ch * 8)) & 0xFF) as u64;
            let mixed = (a * (h - 1 - row) as u64 + b * row as u64) / denom;
            color |= (mixed as u32) << (ch * 8);
        }
        framebuffer::back_fill_rect(0, row, w, 1, color);
    }
}

// -- Content rendering -----------------------------------------------------

fn draw_row_ring(x: u32, y: u32, w: u32, h: u32, selected: bool) {
    if selected {
        framebuffer::back_fill_rounded_rect(x, y, w, h, 4, theme::color(theme::SLOT_ACCENT));
        framebuffer::back_fill_rounded_rect(
            x + 2,
            y + 2,
            w.saturating_sub(4),
            h.saturating_sub(4),
            3,
            theme::color(theme::SLOT_PANEL),
        );
    }
}

/// Render the interior of the window at z `slot` into the back buffer.
/// The `.ling` side has already drawn the shadow/frame/titlebar; `x..y`
/// here is the content origin (below the titlebar).
pub fn draw_content(slot: usize) {
    let Some(w) = slot_window(slot) else { return };
    let (wx, wy, dw, _dh) = deformed_rect(w);
    let x = (wx.max(0) as u32) + 16;
    let y = (wy.max(0) as u32) + TITLEBAR_H + 14;
    let text = theme::color(theme::SLOT_TEXT);
    let dim = theme::color(theme::SLOT_DIM);
    let panel = theme::color(theme::SLOT_PANEL);
    let accent = theme::color(theme::SLOT_ACCENT);
    match w.kind {
        KIND_ABOUT => {
            font8x8::draw_str(x, y, b"LingOS", accent, panel);
            font8x8::draw_str(x, y + 18, b"a from-scratch OS written in Ling", text, panel);
            let up_s = timer::now_ms() / 1000;
            let mut line = [0u8; 32];
            let msg = b"uptime: ";
            line[..msg.len()].copy_from_slice(msg);
            let mut n = msg.len();
            let secs = up_s % 60;
            let mins = up_s / 60;
            if mins >= 10 {
                line[n] = b'0' + ((mins / 10) % 10) as u8;
                n += 1;
            }
            line[n] = b'0' + (mins % 10) as u8;
            line[n + 1] = b'm';
            line[n + 2] = b' ';
            line[n + 3] = b'0' + (secs / 10) as u8;
            line[n + 4] = b'0' + (secs % 10) as u8;
            line[n + 5] = b's';
            font8x8::draw_str(x, y + 44, &line[..n + 6], dim, panel);
            font8x8::draw_str(x, y + 62, b"theme:", dim, panel);
            font8x8::draw_str(x + 60, y + 62, theme::name(theme::current()).as_bytes(), text, panel);
            font8x8::draw_str(
                x,
                y + 88,
                b"drag titlebars; dock launches; arrows+enter",
                dim,
                panel,
            );
        },
        KIND_SETTINGS => {
            let row_h = 34u32;
            let row_w = dw.saturating_sub(32).max(40);
            let labels: [&[u8]; SETTINGS_ROWS] =
                [b"UI theme", b"Keyboard", b"Language", b"Clock"];
            for (i, label) in labels.iter().enumerate() {
                let ry = y + i as u32 * row_h;
                draw_row_ring(x, ry.saturating_sub(6), row_w, row_h - 6, unsafe {
                    SETTINGS_CURSOR == i
                });
                font8x8::draw_str(x + 10, ry, label, text, panel);
                let vx = x + 130;
                match i {
                    0 => font8x8::draw_str(vx, ry, theme::name(theme::current()).as_bytes(), accent, panel),
                    1 => font8x8::draw_str(vx, ry, kbdlayout::name(kbdlayout::current()).as_bytes(), accent, panel),
                    2 => {
                        let li = locale::selected().unwrap_or(0);
                        if let Some(l) = locale::get(li) {
                            font_unicode::draw_utf8_str(
                                vx,
                                ry,
                                l.native_name.as_bytes(),
                                accent,
                                panel,
                                l.uses_daemon_script,
                            );
                        }
                    },
                    _ => {
                        let v: &[u8] = if unsafe { CLOCK_24H } { b"24-hour" } else { b"12-hour" };
                        font8x8::draw_str(vx, ry, v, accent, panel);
                    },
                }
            }
            font8x8::draw_str(
                x,
                y + SETTINGS_ROWS as u32 * 34 + 10,
                b"up/down: row   left/right: change",
                dim,
                panel,
            );
        },
        KIND_FILES => {
            unsafe {
                if FM_VIEWING {
                    let name =
                        core::str::from_utf8(&(&*&raw const FM_VIEW_NAME)[..FM_VIEW_NAME_LEN])
                            .unwrap_or("?");
                    font8x8::draw_str(x, y, name.as_bytes(), accent, panel);
                    let mut content = [0u8; 4096];
                    match lingfs::read_file(name, &mut content) {
                        Ok(Some(len)) => {
                            // First ~12 lines, 8px font, CR/LF aware.
                            let mut line_y = y + 20;
                            let mut col = 0u32;
                            let max_cols = dw.saturating_sub(48) / 8;
                            for &b in &content[..len] {
                                if line_y > y + 20 + 12 * 14 {
                                    break;
                                }
                                if b == b'\n' || col >= max_cols {
                                    line_y += 14;
                                    col = 0;
                                    if b == b'\n' {
                                        continue;
                                    }
                                }
                                if b == b'\r' {
                                    continue;
                                }
                                font8x8::draw_char(x + col * 8, line_y, b, text, panel);
                                col += 1;
                            }
                        },
                        _ => font8x8::draw_str(x, y + 20, b"(unreadable)", dim, panel),
                    }
                    font8x8::draw_str(x, y + 20 + 13 * 14, b"backspace: back", dim, panel);
                    return;
                }
                // Directory listing.
                let header: &[u8] = if FM_DIR_LEN == 0 { b"/" } else { fm_dir().as_bytes() };
                font8x8::draw_str(x, y, header, accent, panel);
                let count = fm_entry_count();
                if count == 0 {
                    font8x8::draw_str(x, y + 22, b"(empty)", dim, panel);
                }
                let row_h = 24u32;
                let row_w = dw.saturating_sub(32).max(40);
                let mut buf = [0u8; FM_NAME_MAX];
                for vis in 0..FM_VISIBLE_ROWS {
                    let i = FM_SCROLL + vis;
                    let Some((len, is_dir)) = lingfs::list_entry(fm_dir(), i, &mut buf) else {
                        break;
                    };
                    let ry = y + 22 + vis as u32 * row_h;
                    draw_row_ring(x, ry.saturating_sub(5), row_w, row_h - 4, FM_CURSOR == i);
                    if is_dir {
                        framebuffer::back_fill_rounded_rect(x + 8, ry, 12, 10, 2, accent);
                        font8x8::draw_str(x + 28, ry, &buf[..len], text, panel);
                    } else {
                        framebuffer::back_fill_rounded_rect(x + 9, ry, 10, 12, 2, dim);
                        font8x8::draw_str(x + 28, ry, &buf[..len], text, panel);
                    }
                }
                font8x8::draw_str(
                    x,
                    y + 22 + FM_VISIBLE_ROWS as u32 * row_h + 6,
                    b"enter: open   backspace: up",
                    dim,
                    panel,
                );
            }
        },
        _ => {},
    }
}
