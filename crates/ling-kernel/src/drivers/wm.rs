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
use crate::drivers::{
    browser, display, editor, framebuffer, font8x8, font_unicode, gallery, kbdlayout, locale,
    media, mixer, netstack, pkgman, terminal, theme, wallpaper,
};
use crate::fs::lingfs;

// Same spring feel as wm_liquid.rs, applied per window. Stiffness is a
// runtime knob now (Settings > Window spring): higher snaps windows to
// the cursor faster with less soap-film wobble, lower is looser/jellier.
// Stored x1000 so the flat-u64 FFI / integer Settings row can carry it.
const DAMPING: f64 = 0.22;
const STRETCH_SENSITIVITY: f64 = 0.006;
const MAX_STRETCH: f64 = 0.35;
const SPRING_K_MIN: u32 = 10; // 0.010
const SPRING_K_MAX: u32 = 80; // 0.080
static mut SPRING_K_MILLI: u32 = 28; // 0.028 default

fn spring_k() -> f64 {
    unsafe { SPRING_K_MILLI as f64 / 1000.0 }
}

pub fn spring_stiffness_milli() -> u32 {
    unsafe { SPRING_K_MILLI }
}

pub fn spring_adjust(delta: i32) {
    unsafe {
        let v = (SPRING_K_MILLI as i32 + delta).clamp(SPRING_K_MIN as i32, SPRING_K_MAX as i32);
        SPRING_K_MILLI = v as u32;
    }
}

pub const MAX_WINDOWS: usize = 6;
pub const TITLEBAR_H: u32 = 28;

pub const KIND_ABOUT: u8 = 0;
pub const KIND_SETTINGS: u8 = 1;
pub const KIND_FILES: u8 = 2;
pub const KIND_WEB: u8 = 3;
pub const KIND_EDIT: u8 = 4;
pub const KIND_GALLERY: u8 = 5;
pub const KIND_TERM: u8 = 6;
pub const KIND_MEDIA: u8 = 7;
pub const KIND_PKG: u8 = 8;
const DOCK_APPS: [u8; 8] = [
    KIND_ABOUT,
    KIND_SETTINGS,
    KIND_FILES,
    KIND_WEB,
    KIND_EDIT,
    KIND_GALLERY,
    KIND_TERM,
    KIND_MEDIA,
];

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
    // Soap-film bend: a horizontal lean that trails the drag velocity and
    // springs/wobbles as the window settles (see `step`). Rendered as a
    // per-scanline x-offset in `draw_window`.
    bend: f64,
    // Dissolve-on-close: a window stays in the table and z-order after
    // close() with `closing = true`, its `dissolve` ramping 0->1 over a
    // few frames; `draw_window` sublimates it (diagonal hash sweep + fade)
    // and `step` frees it at 1.0.
    closing: bool,
    dissolve: f64,
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
    bend: 0.0,
    closing: false,
    dissolve: 0.0,
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
// Rows: UI theme / Sound theme / Wallpaper / Display / Window spring /
// Keyboard / Language / Clock / DNS. Changes apply live as you arrow; the
// Apply/OK buttons persist them to lingfs so they survive a reboot.
const SETTINGS_ROWS: usize = 9;
static mut SETTINGS_CURSOR: usize = 0;
static mut CLOCK_24H: bool = true;

// Network: a DNS-server preset cycler (the Settings "DNS" row). Each preset
// is the primary resolver; the NAT resolver stays the guaranteed fallback
// (see netstack). Cutting-edge, privacy-first options first.
const DNS_PRESETS: [(&str, [u8; 4]); 5] = [
    ("Cloudflare 1.1.1.1", [1, 1, 1, 1]),
    ("Quad9 9.9.9.9", [9, 9, 9, 9]),
    ("AdGuard 94.140.14.14", [94, 140, 14, 14]),
    ("LAN 192.168.0.2", [192, 168, 0, 2]),
    ("NAT 10.0.2.3", [10, 0, 2, 3]),
];
static mut DNS_PRESET: usize = 0;

fn apply_dns_preset() {
    let (_, ip) = DNS_PRESETS[unsafe { DNS_PRESET } % DNS_PRESETS.len()];
    netstack::set_dns(ip, netstack::NAT_RESOLVER);
}

/// Persist the live settings to lingfs `/settings` (one line of small
/// integers) so Apply/OK actually mean something across reboots. Format
/// is deliberately trivial and versioned by field order.
pub fn settings_save() {
    let mut line = [0u8; 64];
    let vals = [
        theme::current() as u32,
        mixer::sound_theme() as u32,
        unsafe { WALLPAPER },
        display::preferred() as u32,
        spring_stiffness_milli(),
        kbdlayout::current() as u32,
        locale::selected().unwrap_or(0) as u32,
        unsafe { CLOCK_24H as u32 },
        unsafe { DNS_PRESET as u32 },
    ];
    let mut n = 0;
    for (i, v) in vals.iter().enumerate() {
        if i > 0 && n < line.len() {
            line[n] = b' ';
            n += 1;
        }
        n += write_u32_into(&mut line[n..], *v);
    }
    let _ = lingfs::write_file("settings", &line[..n]);
}

/// Restore settings written by `settings_save`. Called once at desktop
/// start. Silently keeps defaults if the file is absent/garbled.
pub fn settings_load() {
    let mut buf = [0u8; crate::fs::lingfs::BLOCK_SIZE];
    let Ok(Some(len)) = lingfs::read_file("settings", &mut buf) else { return };
    let mut it = buf[..len].split(|&b| b == b' ' || b == b'\n').filter(|s| !s.is_empty());
    let mut next = || -> Option<u32> {
        it.next().and_then(|s| core::str::from_utf8(s).ok()).and_then(|s| s.parse::<u32>().ok())
    };
    if let Some(v) = next() {
        theme::set(v as usize);
    }
    if let Some(v) = next() {
        mixer::set_sound_theme(v as usize);
    }
    if let Some(v) = next() {
        set_wallpaper(v);
    }
    let _display = next(); // display pref lives in the boot header, not here
    if let Some(v) = next() {
        unsafe { SPRING_K_MILLI = v.clamp(SPRING_K_MIN, SPRING_K_MAX) };
    }
    if let Some(v) = next() {
        kbdlayout::set_current(v as usize);
    }
    if let Some(v) = next() {
        locale::select(v as usize);
    }
    if let Some(v) = next() {
        unsafe { CLOCK_24H = v != 0 };
    }
    if let Some(v) = next() {
        unsafe { DNS_PRESET = (v as usize) % DNS_PRESETS.len() };
        apply_dns_preset();
    }
}

fn write_u32_into(buf: &mut [u8], mut v: u32) -> usize {
    if buf.is_empty() {
        return 0;
    }
    if v == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 10];
    let mut k = 0;
    while v > 0 && k < tmp.len() {
        tmp[k] = b'0' + (v % 10) as u8;
        v /= 10;
        k += 1;
    }
    let n = k.min(buf.len());
    for i in 0..n {
        buf[i] = tmp[k - 1 - i];
    }
    n
}

// -- Wallpaper --------------------------------------------------------------
pub const WALL_GRADIENT: u32 = 0;
pub const WALL_ROYGBIV: u32 = 1;
pub const WALL_SOLID: u32 = 2;
pub const WALL_IMAGE: u32 = 3;
const WALL_MODES: u32 = 4;
static mut WALLPAPER: u32 = 0;

pub fn wallpaper() -> u32 {
    unsafe { WALLPAPER }
}

pub fn set_wallpaper(mode: u32) {
    unsafe {
        // Image mode only sticks if one actually loaded; otherwise fall
        // back to the gradient rather than showing a blank cache.
        if mode % WALL_MODES == WALL_IMAGE && !wallpaper::loaded() {
            WALLPAPER = WALL_GRADIENT;
        } else {
            WALLPAPER = mode % WALL_MODES;
        }
    }
}

fn wallpaper_name(mode: u32) -> &'static str {
    match mode {
        WALL_ROYGBIV => "ROYGBIV",
        WALL_SOLID => "Solid",
        WALL_IMAGE => "Image",
        _ => "Gradient",
    }
}

/// Load a BMP from lingfs and switch to Image wallpaper (the Files app's
/// "set as wallpaper"). Returns false if it didn't decode.
pub fn set_wallpaper_image(name: &str) -> bool {
    if wallpaper::load(name) {
        unsafe { WALLPAPER = WALL_IMAGE };
        true
    } else {
        false
    }
}

/// Promote the image already in the decode cache (the one the Gallery is
/// showing) to the desktop wallpaper -- no re-decode.
pub fn set_wallpaper_current() {
    if wallpaper::loaded() {
        unsafe { WALLPAPER = WALL_IMAGE };
    }
}

// -- Applications menu (MATE-style, top-left) -------------------------------
// A categorized launcher dropped from the top bar's brand corner. Headers
// aren't clickable; app rows open (or raise) their window and close the
// menu. Kept a flat always-expanded list -- simplest thing that reads as
// the MATE menu, and honest about how few apps exist to categorize yet.
// Power actions live in the menu as pseudo-"kinds" above the real window
// kinds so one click routing handles both.
const ACT_LOGOUT: u8 = 200;
const ACT_RESTART: u8 = 201;
const ACT_SHUTDOWN: u8 = 202;

static mut LOGOUT_REQUESTED: bool = false;

/// The desktop loop (main.ling) polls this each frame; when true it tears
/// down the session and returns to the greeter/login.
pub fn logout_requested() -> bool {
    unsafe { LOGOUT_REQUESTED }
}
pub fn clear_logout() {
    unsafe { LOGOUT_REQUESTED = false };
}

struct MenuRow {
    header: &'static str, // non-empty => category header (not an app row)
    app: &'static str,
    kind: u8,
}
const MENU: [MenuRow; 17] = [
    MenuRow { header: "Internet", app: "", kind: 255 },
    MenuRow { header: "", app: "bring (web browser)", kind: KIND_WEB },
    MenuRow { header: "Accessories", app: "", kind: 255 },
    MenuRow { header: "", app: "Terminal", kind: KIND_TERM },
    MenuRow { header: "", app: "Editor", kind: KIND_EDIT },
    MenuRow { header: "", app: "Files", kind: KIND_FILES },
    MenuRow { header: "Sound & Video", app: "", kind: 255 },
    MenuRow { header: "", app: "Gallery", kind: KIND_GALLERY },
    MenuRow { header: "", app: "Media Player", kind: KIND_MEDIA },
    MenuRow { header: "System", app: "", kind: 255 },
    MenuRow { header: "", app: "Packages", kind: KIND_PKG },
    MenuRow { header: "", app: "Settings", kind: KIND_SETTINGS },
    MenuRow { header: "", app: "About LingOS", kind: KIND_ABOUT },
    MenuRow { header: "Power", app: "", kind: 255 },
    MenuRow { header: "", app: "Log Out", kind: ACT_LOGOUT },
    MenuRow { header: "", app: "Restart", kind: ACT_RESTART },
    MenuRow { header: "", app: "Shut Down", kind: ACT_SHUTDOWN },
];
const MENU_W: u32 = 210;
const MENU_ROW_H: u32 = 22;
static mut MENU_OPEN: bool = false;

pub fn menu_open() -> bool {
    unsafe { MENU_OPEN }
}

fn menu_rect() -> (u32, u32, u32, u32) {
    let h = MENU.len() as u32 * MENU_ROW_H + 30;
    (6, 31, MENU_W, h)
}

/// Brand corner (top-left of the top bar) toggles the menu.
fn brand_hit(mx: i64, my: i64) -> bool {
    my >= 0 && my < 30 && mx >= 0 && mx < 84
}

/// Returns the app kind if the press landed on an app row, else 255.
/// Also closes the menu on any click (inside picks, outside dismisses).
fn menu_click(mx: i64, my: i64) -> u8 {
    let (rx, ry, rw, rh) = menu_rect();
    let inside = mx >= rx as i64 && mx < (rx + rw) as i64 && my >= ry as i64 && my < (ry + rh) as i64;
    unsafe { MENU_OPEN = false };
    if !inside {
        return 255;
    }
    let mut yy = ry + 22;
    for row in MENU.iter() {
        if row.header.is_empty() {
            if my >= yy as i64 && my < (yy + MENU_ROW_H) as i64 {
                return row.kind;
            }
            yy += MENU_ROW_H;
        } else {
            yy += MENU_ROW_H;
        }
    }
    255
}

/// Draw the applications menu (call last, over windows and dock).
pub fn draw_menu() {
    if !unsafe { MENU_OPEN } {
        return;
    }
    let (rx, ry, rw, rh) = menu_rect();
    let panel = theme::color(theme::SLOT_PANEL);
    let accent = theme::color(theme::SLOT_ACCENT);
    framebuffer::back_blend_rounded_rect(rx + 4, ry + 5, rw, rh, 10, theme::color(theme::SLOT_SHADOW), 80);
    framebuffer::back_fill_rounded_rect(rx, ry, rw, rh, 10, theme::color(theme::SLOT_PANEL_BORDER));
    framebuffer::back_fill_rounded_rect(rx + 1, ry + 1, rw - 2, rh - 2, 9, panel);
    font8x8::draw_str(rx + 12, ry + 6, b"Applications", accent, panel);
    let mut yy = ry + 22;
    for row in MENU.iter() {
        if row.header.is_empty() {
            font8x8::draw_str(rx + 24, yy + 6, row.app.as_bytes(), theme::color(theme::SLOT_TEXT), panel);
            yy += MENU_ROW_H;
        } else {
            font8x8::draw_str(rx + 10, yy + 6, row.header.as_bytes(), theme::color(theme::SLOT_DIM), panel);
            yy += MENU_ROW_H;
        }
    }
}

// -- Browser window state --------------------------------------------------
// The URL/search bar is always visible. WEB_EDITING = the bar has focus
// (typing edits the URL); false = the page has focus (arrows scroll,
// digits follow links). A fresh browser opens with the bar focused so you
// can type a URL immediately; clicking the bar refocuses it, clicking the
// page defocuses it. No more modal `u` key.
const WEB_INPUT_MAX: usize = 200;
static mut WEB_EDITING: bool = true;
static mut WEB_INPUT: [u8; WEB_INPUT_MAX] = [0; WEB_INPUT_MAX];
static mut WEB_INPUT_LEN: usize = 0;
/// Content columns of the currently-focused browser window, captured at
/// draw time so key-driven fetches wrap to the right width.
static mut WEB_COLS: usize = 72;

fn web_key(k: u8) {
    unsafe {
        if WEB_EDITING {
            match k {
                10 => {
                    let url = core::str::from_utf8(&(&*&raw const WEB_INPUT)[..WEB_INPUT_LEN])
                        .unwrap_or("");
                    if browser::navigate(url, WEB_COLS) {
                        WEB_EDITING = false; // hand focus to the page on success
                    }
                },
                0x1B => WEB_EDITING = false,
                0x08 => {
                    if WEB_INPUT_LEN > 0 {
                        WEB_INPUT_LEN -= 1;
                    }
                },
                0x20..=0x7E => {
                    if WEB_INPUT_LEN < WEB_INPUT_MAX {
                        (&mut *&raw mut WEB_INPUT)[WEB_INPUT_LEN] = k;
                        WEB_INPUT_LEN += 1;
                    }
                },
                _ => {},
            }
            return;
        }
        match k {
            0x11 => browser::scroll(-3, 24),
            0x12 => browser::scroll(3, 24),
            b'1'..=b'9' => {
                browser::follow((k - b'0') as usize, WEB_COLS);
            },
            // Any printable key refocuses the URL bar and starts a fresh
            // edit (Chrome-style "just start typing").
            0x20..=0x7E => {
                WEB_EDITING = true;
                WEB_INPUT_LEN = 0;
                (&mut *&raw mut WEB_INPUT)[0] = k;
                WEB_INPUT_LEN = 1;
            },
            _ => {},
        }
    }
}

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
        KIND_WEB => "bring - browser in ling",
        KIND_EDIT => "Editor",
        KIND_GALLERY => "Gallery",
        KIND_TERM => "Terminal",
        KIND_MEDIA => "Media Player",
        KIND_PKG => "Packages",
        _ => "?",
    }
}

fn kind_size(kind: u8) -> (u32, u32) {
    // Real pixels, scaled off the framebuffer so windows stay proportionate
    // (ui_scale's virtual canvas is for full-screen layouts; windows scale
    // against screen height directly).
    let s = (framebuffer::height() as f64 / 600.0).max(0.5);
    let (w, h) = match kind {
        KIND_SETTINGS => (470.0, 430.0),
        KIND_FILES => (470.0, 370.0),
        KIND_WEB => (620.0, 460.0),
        KIND_EDIT => (600.0, 440.0),
        KIND_GALLERY => (560.0, 440.0),
        KIND_TERM => (620.0, 420.0),
        KIND_MEDIA => (480.0, 300.0),
        KIND_PKG => (560.0, 440.0),
        _ => (390.0, 250.0),
    };
    ((w * s) as u32, (h * s) as u32)
}

pub fn dock_letter(i: usize) -> &'static str {
    match DOCK_APPS.get(i) {
        Some(&KIND_ABOUT) => "i",
        Some(&KIND_SETTINGS) => "S",
        Some(&KIND_FILES) => "F",
        Some(&KIND_WEB) => "W",
        Some(&KIND_EDIT) => "E",
        Some(&KIND_GALLERY) => "G",
        Some(&KIND_TERM) => "T",
        Some(&KIND_MEDIA) => "M",
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
        bend: 0.0,
        closing: false,
        dissolve: 0.0,
    };
    z_raise(idx);
    // The package manager syncs its catalog when it first appears (bounded
    // HTTP; fast-fail if no repo). Done here, not in draw, so the network
    // wait happens once at open rather than every frame.
    if kind == KIND_PKG {
        pkgman::open();
    }
    mixer::jingle(mixer::EVENT_OPEN);
}

/// Begin the dissolve-on-close animation. The window keeps its slot and
/// its z position (so it sublimates in place, over lower windows) until
/// `step` finalizes it -- see the Window::dissolve doc.
pub fn close(idx: usize) {
    if idx < MAX_WINDOWS && windows()[idx].used && !windows()[idx].closing {
        windows()[idx].closing = true;
        windows()[idx].dissolve = 0.0001; // nonzero so draw enters the sublimate path
        unsafe {
            if DRAG_WIN == idx as i32 {
                DRAG_WIN = -1;
            }
        }
        mixer::jingle(mixer::EVENT_CLOSE);
    }
}

fn finalize_closed(idx: usize) {
    windows()[idx].used = false;
    windows()[idx].closing = false;
    z_remove(idx);
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
    // A dissolving window is inert -- clicks fall through to whatever's
    // beneath it.
    if w.closing {
        return false;
    }
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
static mut SETTINGS_LOADED: bool = false;
static mut SEED_DONE: bool = false;

/// Read lingfs `/dns` (two dotted-quad lines: primary, secondary) and
/// apply it, so DNS is configurable without a rebuild. Absent/garbled =
/// keep the compiled defaults (192.168.0.2 -> Cloudflare 1.1.1.1).
fn load_dns_config() {
    let mut buf = [0u8; crate::fs::lingfs::BLOCK_SIZE];
    let Ok(Some(len)) = lingfs::read_file("dns", &mut buf) else { return };
    let mut it = buf[..len].split(|&b| b == b'\n' || b == b' ' || b == b'\r').filter(|s| !s.is_empty());
    let parse = |s: &[u8]| -> Option<[u8; 4]> {
        let mut ip = [0u8; 4];
        let mut part = 0;
        let mut acc: u32 = 0;
        let mut any = false;
        for &b in s {
            match b {
                b'0'..=b'9' => {
                    acc = acc * 10 + (b - b'0') as u32;
                    any = true;
                    if acc > 255 {
                        return None;
                    }
                },
                b'.' => {
                    if part >= 3 || !any {
                        return None;
                    }
                    ip[part] = acc as u8;
                    part += 1;
                    acc = 0;
                    any = false;
                },
                _ => return None,
            }
        }
        if part == 3 && any {
            ip[3] = acc as u8;
            Some(ip)
        } else {
            None
        }
    };
    let (mut p, mut s) = netstack::dns_servers();
    if let Some(v) = it.next().and_then(parse) {
        p = v;
    }
    if let Some(v) = it.next().and_then(parse) {
        s = v;
    }
    netstack::set_dns(p, s);
}

pub fn step(mx: i64, my: i64, buttons: u8) {
    unsafe {
        // Restore persisted settings on the first frame (lingfs is mounted
        // by the time the desktop loop starts). Lazy so no extra intrinsic
        // / compiler rebuild is needed just to call settings_load.
        if !SETTINGS_LOADED {
            SETTINGS_LOADED = true;
            settings_load();
            load_dns_config();
        }
        // Seed the music library incrementally -- at most one song per
        // frame, so the ~1.5MiB write never freezes a single frame.
        if !SEED_DONE {
            SEED_DONE = media::seed_pelipo();
        }
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
            // Press edge, overlay-priority order: an open app menu first,
            // then the brand corner that toggles it, then tray, dock,
            // windows.
            if MENU_OPEN {
                let kind = menu_click(mx, my);
                match kind {
                    255 => {},
                    ACT_LOGOUT => LOGOUT_REQUESTED = true,
                    ACT_RESTART => crate::arch::power::reboot(),
                    ACT_SHUTDOWN => crate::arch::power::poweroff(),
                    k => open(k),
                }
                PREV_BUTTONS = buttons;
                return;
            }
            if brand_hit(mx, my) {
                MENU_OPEN = true;
                mixer::jingle(mixer::EVENT_CLICK);
                PREV_BUTTONS = buttons;
                return;
            }
            if tray_hit(mx, my) {
                PREV_BUTTONS = buttons;
                return;
            }
            let dock = HOVER_DOCK;
            if dock >= 0 {
                mixer::jingle(mixer::EVENT_CLICK);
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
                        } else {
                            // Click in the content area -- let the app
                            // handle it (Settings Apply/OK buttons today).
                            content_click(idx, mx, my);
                        }
                    }
                } else {
                    log_hex(b"wm: click missed, my=0x", my as u32);
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

        // Spring advance, shared dt. Clamped like wm_liquid, but ALSO
        // sub-stepped in <=8ms slices: explicit-Euler with this stiffness
        // is numerically unstable at dt=50 (k*dt^2 ~ 70 -- observed under
        // QEMU TCG, whose frame times routinely hit the clamp, as windows
        // overshooting clean off the top of the screen).
        let now = timer::now_ms();
        let dt = if LAST_STEP_MS == 0 { 16 } else { now.saturating_sub(LAST_STEP_MS).min(50) };
        LAST_STEP_MS = now;
        let mut remaining = dt as f64;
        while remaining > 0.0 {
            let step = remaining.min(4.0);
            remaining -= step;
            for w in windows().iter_mut() {
                if !w.used {
                    continue;
                }
                // Semi-implicit Euler with IMPLICIT damping: the explicit
                // form multiplies velocity by (1 - damping*step), which
                // goes sign-alternating once damping*step > 1 (0.22*8 =
                // 1.76 here) -- observed as windows that never settle,
                // drifting to a different rest spot every run. Dividing by
                // (1 + damping*step) decays monotonically at ANY step
                // size; the spring term stays explicit so the liquid
                // overshoot the WM is named for survives.
                let k = spring_k();
                w.vx = (w.vx + (w.tx - w.px) * k * step) / (1.0 + DAMPING * step);
                w.vy = (w.vy + (w.ty - w.py) * k * step) / (1.0 + DAMPING * step);
                w.px += w.vx * step;
                w.py += w.vy * step;
                // Hard floor below the top bar: overshoot may squash
                // against it, but a titlebar must never become unreachable.
                if w.py < 34.0 {
                    w.py = 34.0;
                    if w.vy < 0.0 {
                        w.vy = 0.0;
                    }
                }
                let sx = (w.vx.abs() * STRETCH_SENSITIVITY).min(MAX_STRETCH);
                let sy = (w.vy.abs() * STRETCH_SENSITIVITY).min(MAX_STRETCH);
                w.scale_w = 1.0 + sx - sy * 0.5;
                w.scale_h = 1.0 + sy - sx * 0.5;
                // Soap-film bend: the body trails the horizontal velocity.
                // It rides the same spring (vx already oscillates as the
                // window settles), so it wobbles then relaxes to flat -- no
                // separate oscillator needed. Capped so it can't invert.
                let target_bend = (w.vx * 0.9).clamp(-(w.w as f64) * 0.22, w.w as f64 * 0.22);
                w.bend += (target_bend - w.bend) * (step / 40.0).min(1.0);
            }
        }
        // Dissolve advances per FRAME, not per kernel-ms: the TSC runs
        // tens-of-times fast under TCG, so a ms-based ramp would finish in
        // a couple of frames (invisible). ~1/26 per frame => ~26 frames of
        // sublimation, enjoyable to watch and catchable in a screendump.
        for w in windows().iter_mut() {
            if w.used && w.closing {
                w.dissolve += 0.038;
            }
        }
        // Finalize any window whose dissolve completed (outside the
        // borrow above).
        for i in 0..MAX_WINDOWS {
            if windows()[i].used && windows()[i].closing && windows()[i].dissolve >= 1.0 {
                finalize_closed(i);
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
    // An open tray popover captures keys ahead of the focused window.
    if tray_key(k) {
        return;
    }
    let focused_kind = unsafe {
        if Z_LEN == 0 {
            return;
        }
        windows()[Z[Z_LEN - 1]].kind
    };
    match focused_kind {
        KIND_SETTINGS => settings_key(k),
        KIND_FILES => files_key(k),
        KIND_WEB => web_key(k),
        KIND_EDIT => editor::key(k, crate::drivers::keyboard::shift_down()),
        KIND_GALLERY => gallery::key(k),
        KIND_TERM => terminal::key(k),
        KIND_MEDIA => media::key(k),
        KIND_PKG => pkgman::key(k),
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
                        let n = mixer::sound_theme_count() as i32;
                        let cur = mixer::sound_theme() as i32;
                        mixer::set_sound_theme(((cur + dir + n) % n) as usize);
                        // Audition the theme you just landed on.
                        mixer::jingle(mixer::EVENT_LOGIN);
                    },
                    2 => {
                        let n = WALL_MODES as i32;
                        let cur = WALLPAPER as i32;
                        set_wallpaper(((cur + dir + n) % n) as u32);
                    },
                    3 => {
                        // Display: persist a next-boot mode preference.
                        let n = display::mode_count() as i32;
                        let cur = display::preferred() as i32;
                        display::set_preferred(((cur + dir + n) % n) as usize);
                    },
                    4 => spring_adjust(dir * 2), // window spring stiffness
                    5 => {
                        let n = kbdlayout::count() as i32;
                        let cur = kbdlayout::current() as i32;
                        kbdlayout::set_current(((cur + dir + n) % n) as usize);
                    },
                    6 => {
                        let n = locale::count() as i32;
                        let cur = locale::selected().unwrap_or(0) as i32;
                        locale::select(((cur + dir + n) % n) as usize);
                    },
                    7 => CLOCK_24H = !CLOCK_24H,
                    _ => {
                        // DNS server preset.
                        let n = DNS_PRESETS.len() as i32;
                        DNS_PRESET = ((DNS_PRESET as i32 + dir + n) % n) as usize;
                        apply_dns_preset();
                    },
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

/// Extensions the Editor opens (everything else is a binary peek). Also
/// treats extensionless names as text -- most lingfs synthetic files
/// (hostname, language) are plain text.
fn is_editable(name: &[u8]) -> bool {
    let ext = |suffix: &[u8]| name.len() >= suffix.len() && &name[name.len() - suffix.len()..] == suffix;
    ext(b".ling")
        || ext(b".txt")
        || ext(b".md")
        || ext(b".cfg")
        || ext(b".toml")
        || ext(b".sh")
        || !name.contains(&b'.')
}

fn files_key(k: u8) {
    use crate::drivers::{clipboard, keyboard as kb};
    unsafe {
        // Ctrl+C copies the highlighted entry's path to the clipboard, so
        // it can be pasted into the editor, terminal, or browser URL bar.
        if k == kb::CTRL_C {
            let mut nm = [0u8; FM_NAME_MAX];
            if let Some((len, _)) = lingfs::list_entry(fm_dir(), FM_CURSOR, &mut nm) {
                let dir = fm_dir();
                if dir.is_empty() {
                    clipboard::set(&nm[..len]);
                } else {
                    // "dir/name"
                    let mut full = [0u8; FM_NAME_MAX * 2 + 1];
                    let dl = dir.len();
                    full[..dl].copy_from_slice(dir.as_bytes());
                    full[dl] = b'/';
                    full[dl + 1..dl + 1 + len].copy_from_slice(&nm[..len]);
                    clipboard::set(&full[..dl + 1 + len]);
                }
            }
            return;
        }
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
                    let full = core::str::from_utf8(&vn[..off + len]).unwrap_or("");
                    // Route by extension: .bmp -> Gallery, .wav -> Media
                    // Player, text-ish files -> Editor, else inline peek.
                    if len >= 4 && &name[len - 4..len] == b".bmp" {
                        if gallery::open(full) {
                            open(KIND_GALLERY);
                        }
                    } else if len >= 4 && &name[len - 4..len] == b".wav" {
                        if media::open(full) {
                            open(KIND_MEDIA);
                        }
                    } else if is_editable(&name[..len]) {
                        editor::load(full);
                        open(KIND_EDIT);
                    } else {
                        FM_VIEWING = true;
                    }
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

// -- Top-bar tray: volume + network widgets --------------------------------
// MATE-panel-shaped: two icons at the top bar's right end (left of the
// clock); clicking the speaker opens a volume popover with a master row
// plus one row per app stream (the mixer's per-app volumes), adjusted
// with up/down + left/right while open. The net icon just reports the
// real e1000 state -- ethernet only, because that's the NIC we have.

static mut TRAY_OPEN: bool = false;
static mut TRAY_CURSOR: usize = 0; // 0 = master, 1.. = mixer streams

fn tray_vol_x() -> i64 {
    framebuffer::width() as i64 - 140
}

fn tray_net_x() -> i64 {
    framebuffer::width() as i64 - 110
}

fn tray_hit(mx: i64, my: i64) -> bool {
    if my >= 30 {
        return false;
    }
    if mx >= tray_vol_x() && mx < tray_vol_x() + 24 {
        unsafe {
            TRAY_OPEN = !TRAY_OPEN;
            TRAY_CURSOR = 0;
        }
        mixer::jingle(mixer::EVENT_CLICK);
        return true;
    }
    // The net icon has no popover yet; swallowing the click keeps it from
    // falling through to a window underneath.
    mx >= tray_net_x() && mx < tray_net_x() + 24
}

pub fn tray_open() -> bool {
    unsafe { TRAY_OPEN }
}

fn tray_key(k: u8) -> bool {
    unsafe {
        if !TRAY_OPEN {
            return false;
        }
        let rows = 1 + mixer::stream_count();
        match k {
            0x11 => TRAY_CURSOR = TRAY_CURSOR.saturating_sub(1),
            0x12 => TRAY_CURSOR = (TRAY_CURSOR + 1).min(rows - 1),
            0x13 | 0x14 => {
                let delta: i64 = if k == 0x13 { -10 } else { 10 };
                if TRAY_CURSOR == 0 {
                    let v = (mixer::master_volume() as i64 + delta).clamp(0, 100);
                    mixer::set_master_volume(v as u32);
                } else {
                    let s = TRAY_CURSOR - 1;
                    let v = (mixer::stream_volume(s) as i64 + delta).clamp(0, 100);
                    mixer::set_stream_volume(s, v as u32);
                }
                mixer::jingle(mixer::EVENT_CLICK);
            },
            0x1B | 10 => TRAY_OPEN = false, // Esc or Enter closes
            _ => {},
        }
        true
    }
}

/// Draw the tray icons and (if open) the volume popover. `net_up` is the
/// real e1000 init result the desktop got at boot.
pub fn draw_tray(net_up: bool) {
    let panel = theme::color(theme::SLOT_PANEL);
    let accent = theme::color(theme::SLOT_ACCENT);
    let dim = theme::color(theme::SLOT_DIM);
    let text = theme::color(theme::SLOT_TEXT);

    // Speaker: box + cone triangle-ish (two rects) + a sound arc dot.
    let vx = tray_vol_x() as u32;
    framebuffer::back_fill_rect(vx, 11, 6, 8, if mixer::master_volume() > 0 { text } else { dim });
    framebuffer::back_fill_rect(vx + 6, 8, 5, 14, if mixer::master_volume() > 0 { text } else { dim });
    if mixer::master_volume() > 0 {
        framebuffer::back_fill_circle(vx + 16, 15, 2, accent);
    }

    // Net: rounded plug + stem; accent when the NIC is up, dim otherwise.
    let nx = tray_net_x() as u32;
    let net_color = if net_up { accent } else { dim };
    framebuffer::back_fill_rounded_rect(nx, 9, 14, 10, 3, net_color);
    framebuffer::back_fill_rect(nx + 6, 19, 2, 5, net_color);

    if !unsafe { TRAY_OPEN } {
        return;
    }
    // Popover: rounded card under the speaker with master + stream rows.
    let rows = 1 + mixer::stream_count();
    let row_h = 26u32;
    let pw = 240u32;
    let ph = rows as u32 * row_h + 20;
    let px = (framebuffer::width()).saturating_sub(pw + 16);
    let py = 34u32;
    framebuffer::back_blend_rounded_rect(px + 4, py + 5, pw, ph, 10, theme::color(theme::SLOT_SHADOW), 80);
    framebuffer::back_fill_rounded_rect(px, py, pw, ph, 10, theme::color(theme::SLOT_PANEL_BORDER));
    framebuffer::back_fill_rounded_rect(px + 1, py + 1, pw - 2, ph - 2, 9, panel);
    for r in 0..rows {
        let ry = py + 10 + r as u32 * row_h;
        let selected = unsafe { TRAY_CURSOR == r };
        if selected {
            framebuffer::back_fill_rounded_rect(px + 6, ry.saturating_sub(4), pw - 12, row_h - 4, 5, accent);
            framebuffer::back_fill_rounded_rect(px + 8, ry.saturating_sub(2), pw - 16, row_h - 8, 4, panel);
        }
        let (name, vol): (&str, u32) = if r == 0 {
            ("master", mixer::master_volume())
        } else {
            (mixer::stream_name(r - 1), mixer::stream_volume(r - 1))
        };
        font8x8::draw_str(px + 14, ry, name.as_bytes(), if r == 0 { text } else { dim }, panel);
        // Volume bar: track + fill, 100px wide.
        let bx = px + 110;
        framebuffer::back_fill_rounded_rect(bx, ry + 2, 100, 6, 3, theme::color(theme::SLOT_DOT_DIM));
        if vol > 0 {
            framebuffer::back_fill_rounded_rect(bx, ry + 2, vol, 6, 3, accent);
        }
        // Exclusive-mode marker on the holding stream.
        if r > 0 && mixer::exclusive_holder() == (r - 1) as i32 {
            font8x8::draw_str(px + pw - 24, ry, b"!", theme::color(theme::SLOT_ERROR), panel);
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

/// Mix two 0xRRGGBB colors: `t_num/t_den` of `b`, the rest `a`.
fn mix_rgb(a: u32, b: u32, t_num: u32, t_den: u32) -> u32 {
    let mut out = 0u32;
    for ch in 0..3 {
        let ca = (a >> (ch * 8)) & 0xFF;
        let cb = (b >> (ch * 8)) & 0xFF;
        let m = (ca * (t_den - t_num) + cb * t_num) / t_den;
        out |= m << (ch * 8);
    }
    out
}

/// The seven ROYGBIV hues, each pre-tempered toward the active theme's
/// background so the rainbow reads as *of* the theme (LingOS dusk purple
/// under Dusk, light-washed under Daylight) instead of a paint-store
/// swatch pasted on top.
fn roygbiv_color(i: usize) -> u32 {
    const HUES: [u32; 7] = [
        0xE0453A, // red
        0xE08A3A, // orange
        0xE0D24A, // yellow
        0x4AE06A, // green
        0x3A8AE0, // blue
        0x4A4AE0, // indigo
        0x9A4AE0, // violet
    ];
    mix_rgb(HUES[i % 7], theme::color(theme::SLOT_BG), 55, 100)
}

/// Wallpaper renderer, mode from the Settings row: theme gradient (the
/// default), the LingOS ROYGBIV diagonal (seven theme-tempered stripes
/// with a blended edge band between neighbors), or solid. Per-row span
/// fills only -- no per-pixel work, cheap enough to redraw every frame
/// even under TCG.
pub fn draw_wallpaper() {
    let h = framebuffer::height();
    let w = framebuffer::width();
    if h == 0 {
        return;
    }
    match unsafe { WALLPAPER } {
        WALL_IMAGE => {
            if wallpaper::loaded() {
                wallpaper::draw();
            } else {
                framebuffer::back_clear(theme::color(theme::SLOT_BG));
            }
        },
        WALL_SOLID => framebuffer::back_clear(theme::color(theme::SLOT_BG)),
        WALL_ROYGBIV => {
            // Diagonal stripes: each row shifts the stripe origin left by
            // half a pixel per row. Stripe width spans the diagonal extent
            // so all seven hues are on-screen at once; each stripe's last
            // ~25% blends into the next hue for a soft edge.
            let diag = w + h / 2;
            let sw = (diag / 7).max(8);
            let blend_w = sw / 4;
            for row in 0..h {
                let offset = (row / 2) as i64;
                for i in 0..8i64 {
                    let hue = ((i % 7) + 7) as usize % 7;
                    let next = (hue + 1) % 7;
                    let x0 = i * sw as i64 - offset;
                    let solid_w = sw - blend_w;
                    // Solid body of the stripe.
                    let sx = x0.max(0);
                    let sw_clip = (x0 + solid_w as i64 - sx).max(0) as u32;
                    if sx < w as i64 && sw_clip > 0 {
                        framebuffer::back_fill_rect(sx as u32, row, sw_clip, 1, roygbiv_color(hue));
                    }
                    // Blended edge band, in 4 sub-steps toward the next hue.
                    for b in 0..4u32 {
                        let bx = x0 + solid_w as i64 + (blend_w / 4 * b) as i64;
                        let bw = (blend_w / 4).max(1);
                        if bx + (bw as i64) < 0 || bx >= w as i64 {
                            continue;
                        }
                        let c = mix_rgb(roygbiv_color(hue), roygbiv_color(next), b * 25 + 12, 100);
                        framebuffer::back_fill_rect(bx.max(0) as u32, row, bw, 1, c);
                    }
                }
            }
        },
        _ => {
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
        },
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

// Settings Apply/OK button rects, in absolute screen pixels, from the
// window's deformed rect. OK sits bottom-right, Apply just left of it.
fn settings_ok_rect(wx: i64, wy: i64, dw: u32, dh: u32) -> (u32, u32, u32, u32) {
    let (bw, bh) = (70u32, 28u32);
    let ox = (wx + dw as i64 - bw as i64 - 16).max(0) as u32;
    let oy = (wy + dh as i64 - bh as i64 - 14).max(0) as u32;
    (ox, oy, bw, bh)
}

fn settings_apply_rect(wx: i64, wy: i64, dw: u32, dh: u32) -> (u32, u32, u32, u32) {
    let (ox, oy, bw, bh) = settings_ok_rect(wx, wy, dw, dh);
    (ox.saturating_sub(bw + 10), oy, bw, bh)
}

fn pt_in(px: i64, py: i64, r: (u32, u32, u32, u32)) -> bool {
    px >= r.0 as i64 && px < (r.0 + r.2) as i64 && py >= r.1 as i64 && py < (r.1 + r.3) as i64
}

/// Handle a click inside a window's content area (below the titlebar).
/// Today only Settings has clickable content (Apply/OK). Returns true if
/// the click was consumed.
fn content_click(idx: usize, mx: i64, my: i64) -> bool {
    let w = windows()[idx];
    let (wx, wy, dw, dh) = deformed_rect(&w);
    if w.kind == KIND_SETTINGS {
        if pt_in(mx, my, settings_ok_rect(wx, wy, dw, dh)) {
            settings_save();
            close(idx);
            return true;
        }
        if pt_in(mx, my, settings_apply_rect(wx, wy, dw, dh)) {
            settings_save();
            return true;
        }
        return false;
    }
    if w.kind == KIND_WEB {
        // The URL bar sits at the top of the content; clicking it focuses
        // the bar for editing, clicking the page hands focus back to it.
        let cx = wx.max(0) + 16;
        let cy = wy.max(0) + TITLEBAR_H as i64 + 14;
        let bar = (cx as u32, cy as u32, dw.saturating_sub(24), 22u32);
        unsafe { WEB_EDITING = pt_in(mx, my, bar) };
        return true;
    }
    false
}

/// Render the interior of the window at z `slot` into the back buffer.
/// The `.ling` side has already drawn the shadow/frame/titlebar; `x..y`
/// here is the content origin (below the titlebar).
pub fn draw_content(slot: usize) {
    let Some(w) = slot_window(slot) else { return };
    let (wx, wy, dw, dh) = deformed_rect(w);
    let x = (wx.max(0) as u32) + 16;
    let y = (wy.max(0) as u32) + TITLEBAR_H + 14;
    if w.kind == KIND_EDIT {
        editor::draw(x, y, dw.saturating_sub(24), dh.saturating_sub(TITLEBAR_H + 20));
        return;
    }
    if w.kind == KIND_GALLERY {
        gallery::draw(x, y, dw.saturating_sub(24), dh.saturating_sub(TITLEBAR_H + 20));
        return;
    }
    if w.kind == KIND_TERM {
        terminal::draw(x, y, dw.saturating_sub(24), dh.saturating_sub(TITLEBAR_H + 20));
        return;
    }
    if w.kind == KIND_MEDIA {
        media::draw(x, y, dw.saturating_sub(24), dh.saturating_sub(TITLEBAR_H + 20));
        return;
    }
    if w.kind == KIND_PKG {
        pkgman::draw(x, y, dw.saturating_sub(24), dh.saturating_sub(TITLEBAR_H + 20));
        return;
    }
    if w.kind == KIND_WEB {
        if slot_focused(slot) {
            unsafe { WEB_COLS = (dw.saturating_sub(40) / 8).max(20) as usize };
        }
        // Always-visible URL/search bar across the top of the content.
        let bar_w = dw.saturating_sub(24);
        let bar_h = 22u32;
        let editing = unsafe { WEB_EDITING };
        let border = if editing {
            theme::color(theme::SLOT_ACCENT)
        } else {
            theme::color(theme::SLOT_PANEL_BORDER)
        };
        framebuffer::back_fill_rounded_rect(x, y, bar_w, bar_h, 6, border);
        framebuffer::back_fill_rounded_rect(x + 2, y + 2, bar_w - 4, bar_h - 4, 5, theme::color(theme::SLOT_BG));
        let text = theme::color(theme::SLOT_TEXT);
        let bgc = theme::color(theme::SLOT_BG);
        if editing {
            let buf = unsafe { &(&*&raw const WEB_INPUT)[..WEB_INPUT_LEN] };
            let maxch = ((bar_w - 16) / 8) as usize;
            let shown = buf.len().min(maxch);
            font8x8::draw_str(x + 8, y + 7, &buf[buf.len() - shown..], text, bgc);
            framebuffer::back_fill_rect(x + 8 + shown as u32 * 8, y + 5, 2, 12, theme::color(theme::SLOT_ACCENT));
        } else {
            let url = browser::current_url();
            if url.is_empty() {
                font8x8::draw_str(x + 8, y + 7, b"search or enter a URL", theme::color(theme::SLOT_DIM), bgc);
            } else {
                let ub = url.as_bytes();
                let maxch = ((bar_w - 16) / 8) as usize;
                font8x8::draw_str(x + 8, y + 7, &ub[..ub.len().min(maxch)], theme::color(theme::SLOT_ACCENT), bgc);
            }
        }
        // Page body below the bar.
        browser::draw_page(x, y + bar_h + 6, bar_w, dh.saturating_sub(TITLEBAR_H + bar_h + 26));
        return;
    }
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
            let labels: [&[u8]; SETTINGS_ROWS] = [
                b"UI theme",
                b"Sound theme",
                b"Wallpaper",
                b"Display",
                b"Window spring",
                b"Keyboard",
                b"Language",
                b"Clock",
                b"DNS server",
            ];
            for (i, label) in labels.iter().enumerate() {
                let ry = y + i as u32 * row_h;
                draw_row_ring(x, ry.saturating_sub(6), row_w, row_h - 6, unsafe {
                    SETTINGS_CURSOR == i
                });
                font8x8::draw_str(x + 10, ry, label, text, panel);
                let vx = x + 130;
                match i {
                    0 => font8x8::draw_str(vx, ry, theme::name(theme::current()).as_bytes(), accent, panel),
                    1 => font8x8::draw_str(
                        vx,
                        ry,
                        mixer::sound_theme_name(mixer::sound_theme()).as_bytes(),
                        accent,
                        panel,
                    ),
                    2 => font8x8::draw_str(vx, ry, wallpaper_name(unsafe { WALLPAPER }).as_bytes(), accent, panel),
                    3 => {
                        font8x8::draw_str(vx, ry, display::mode_label(display::preferred()).as_bytes(), accent, panel);
                        if display::persistable() {
                            font8x8::draw_str(vx + 130, ry, b"(next boot)", dim, panel);
                        } else {
                            font8x8::draw_str(vx + 130, ry, display::current_str().as_bytes(), dim, panel);
                        }
                    },
                    4 => {
                        // Spring stiffness as a small bar + numeric milli.
                        let milli = spring_stiffness_milli();
                        let mut nb = [0u8; 8];
                        let mut nn = 0;
                        nb[nn] = b'0'; nb[nn + 1] = b'.'; nn += 2;
                        nb[nn] = b'0' + ((milli / 10) % 10) as u8; nn += 1;
                        nb[nn] = b'0' + (milli % 10) as u8; nn += 1;
                        font8x8::draw_str(vx, ry, &nb[..nn], accent, panel);
                        let bx = vx + 60;
                        framebuffer::back_fill_rounded_rect(bx, ry + 2, 100, 6, 3, theme::color(theme::SLOT_DOT_DIM));
                        let fill = (milli.saturating_sub(SPRING_K_MIN)) * 100 / (SPRING_K_MAX - SPRING_K_MIN);
                        framebuffer::back_fill_rounded_rect(bx, ry + 2, fill.max(1), 6, 3, accent);
                    },
                    5 => font8x8::draw_str(vx, ry, kbdlayout::name(kbdlayout::current()).as_bytes(), accent, panel),
                    6 => {
                        let li = locale::selected().unwrap_or(0);
                        if let Some(l) = locale::get(li) {
                            // Daemon-script locales carry Latin names (e.g.
                            // "Ling Country") that would render through the
                            // 16px daemon atlas -- oversized next to the 8px
                            // rows. Show those in the 8px Latin font; keep
                            // the real 16px native script for CJK/Thai etc.
                            if l.uses_daemon_script {
                                font8x8::draw_str(vx, ry, l.latin_name.as_bytes(), accent, panel);
                            } else {
                                font_unicode::draw_utf8_str(
                                    vx,
                                    ry,
                                    l.native_name.as_bytes(),
                                    accent,
                                    panel,
                                    false,
                                );
                            }
                        }
                    },
                    7 => {
                        let v: &[u8] = if unsafe { CLOCK_24H } { b"24-hour" } else { b"12-hour" };
                        font8x8::draw_str(vx, ry, v, accent, panel);
                    },
                    _ => {
                        let (name, _) = DNS_PRESETS[unsafe { DNS_PRESET } % DNS_PRESETS.len()];
                        font8x8::draw_str(vx, ry, name.as_bytes(), accent, panel);
                    },
                }
            }
            let hint_y = y + SETTINGS_ROWS as u32 * row_h + 8;
            font8x8::draw_str(x, hint_y, b"up/down: row   left/right: change", dim, panel);
            // Apply / OK buttons (mouse-clickable; see settings_content_click).
            let (ax, ay, aw, ah) = settings_apply_rect(wx, wy, dw, dh);
            framebuffer::back_fill_rounded_rect(ax, ay, aw, ah, 6, theme::color(theme::SLOT_PANEL_BORDER));
            framebuffer::back_fill_rounded_rect(ax + 1, ay + 1, aw - 2, ah - 2, 5, panel);
            font8x8::draw_str(ax + 18, ay + 9, b"Apply", text, panel);
            let (ox, oy, ow, oh) = settings_ok_rect(wx, wy, dw, dh);
            framebuffer::back_fill_rounded_rect(ox, oy, ow, oh, 6, accent);
            framebuffer::back_fill_rounded_rect(ox + 1, oy + 1, ow - 2, oh - 2, 5, theme::color(theme::SLOT_TITLEBAR));
            font8x8::draw_str(ox + 28, oy + 9, b"OK", theme::color(theme::SLOT_TITLEBAR_TEXT), theme::color(theme::SLOT_TITLEBAR));
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

/// Deterministic per-cell hash in 0.0..1.0 for the dissolve pattern (no
/// Math.random on this target; a hash of the cell coords gives a stable,
/// ragged sublimation edge that looks the same every frame of one close).
fn cell_hash(cx: u32, cy: u32) -> f64 {
    let mut h = cx.wrapping_mul(374761393).wrapping_add(cy.wrapping_mul(668265263));
    h ^= h >> 13;
    h = h.wrapping_mul(1274126177);
    h ^= h >> 16;
    (h & 0xFFFF) as f64 / 65535.0
}

/// Render a whole window kernel-side: soft shadow, a rounded body that
/// *bends* (per-scanline horizontal shear driven by `bend`, so a dragged
/// window leans and wobbles like a soap film), the titlebar with title +
/// traffic-light buttons, and -- when not mid-dissolve -- its content. A
/// closing window sublimates instead: a diagonal hash-sweep of vanishing
/// cells plus an overall fade. This replaces the old `.ling` rect drawing
/// (which couldn't shear or dissolve) -- `main.ling` now just calls this
/// once per z slot.
pub fn draw_window(slot: usize) {
    let Some(w) = slot_window(slot) else { return };
    let (x0, y0, dw, dh) = deformed_rect(w);
    if dw < 8 || dh < 8 {
        return;
    }
    let x0 = x0.max(-(dw as i64));
    let bend = w.bend;
    let dissolve = w.dissolve;
    let fade = ((1.0 - dissolve) * 255.0).clamp(0.0, 255.0) as u32;
    let focused = slot_focused(slot);
    let radius = 10u32;
    let tb = TITLEBAR_H;

    let border = if focused {
        theme::color(theme::SLOT_ACCENT)
    } else {
        theme::color(theme::SLOT_PANEL_BORDER)
    };
    let body = theme::color(theme::SLOT_PANEL);
    let title_bg = if focused {
        theme::color(theme::SLOT_TITLEBAR)
    } else {
        theme::color(theme::SLOT_TITLEBAR_IDLE)
    };

    // Per-scanline shear: a parallelogram lean around the vertical center,
    // amplitude `bend`. `xoff(dy)` in pixels.
    let shear = |dy: u32| -> i64 { (bend * (dy as f64 / dh as f64 - 0.5) * 2.0) as i64 };

    // Shadow (skipped once mostly dissolved to save the blend cost).
    if dissolve < 0.6 {
        let sh_alpha = (70 * fade / 255).min(70);
        framebuffer::back_blend_rounded_rect(
            (x0 + 6).max(0) as u32,
            (y0 + 8).max(0) as u32,
            dw,
            dh,
            radius,
            theme::color(theme::SLOT_SHADOW),
            sh_alpha,
        );
    }

    let cell = 9u32; // dissolve cell size
    for dy in 0..dh {
        let inset = framebuffer::rounded_row_inset(dy, dh, radius).min(dw / 2);
        let rx = x0 + inset as i64 + shear(dy);
        let ry = y0 + dy as i64;
        if ry < 0 {
            continue;
        }
        let row_w = dw - inset * 2;
        let (fill, is_edge) = if dy < tb {
            (title_bg, false)
        } else {
            (body, false)
        };
        let _ = is_edge;
        if dissolve <= 0.0 {
            // Fast opaque path: 1px border frame + inner fill.
            framebuffer::back_fill_rect(rx.max(0) as u32, ry as u32, row_w, 1, border);
            if dy >= 1 && dy + 1 < dh && row_w > 2 {
                framebuffer::back_fill_rect((rx + 1).max(0) as u32, ry as u32, row_w - 2, 1, fill);
            }
        } else {
            // Sublimating: draw cell by cell, skipping vanished cells and
            // fading the survivors. Diagonal sweep from top-left + hash.
            let mut cxp = 0u32;
            while cxp < row_w {
                let px = rx + cxp as i64;
                let cw = cell.min(row_w - cxp);
                let dprog = (cxp as f64 / dw as f64) * 0.5 + (dy as f64 / dh as f64) * 0.5;
                let gone = dprog + cell_hash(cxp / cell, dy / cell) * 0.5 < dissolve * 1.6;
                if !gone && px >= 0 {
                    let c = if dy < tb { title_bg } else { fill };
                    framebuffer::back_blend_rect(px as u32, ry as u32, cw, 1, c, fade);
                }
                cxp += cw;
            }
        }
    }

    // Titlebar text + traffic lights (fade out as it dissolves).
    if dissolve < 0.55 {
        let tx = (x0 + 12 + shear(6)).max(0) as u32;
        let title = kind_title(w.kind);
        let ttext = if focused {
            theme::color(theme::SLOT_TITLEBAR_TEXT)
        } else {
            theme::color(theme::SLOT_DIM)
        };
        font8x8::draw_str(tx, (y0 + 8).max(0) as u32, title.as_bytes(), ttext, title_bg);
        // minimize (amber) + close (red) at the right end.
        let cyb = (y0 + tb as i64 / 2).max(0) as u32;
        let close_x = x0 + dw as i64 - tb as i64 / 2 + shear(tb / 2);
        let min_x = x0 + dw as i64 - tb as i64 - tb as i64 / 2 + shear(tb / 2);
        if min_x >= 0 {
            framebuffer::back_fill_circle(min_x as u32, cyb, 6, 0xE0A44A);
        }
        if close_x >= 0 {
            framebuffer::back_fill_circle(close_x as u32, cyb, 6, 0xE0605A);
        }
    }

    // Content: only while the window is coherent (not mid-dissolve). The
    // bend is left off the content (it stays axis-aligned inside the bent
    // body) -- the amplitude is small enough that text stays within it.
    if dissolve <= 0.0 {
        draw_content(slot);
    }
}
