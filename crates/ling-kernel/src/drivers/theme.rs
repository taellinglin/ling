//! UI theme table for every graphics-mode surface (GUI installer, WM
//! desktop, greeter) -- one kernel-side source of truth for the palette,
//! switchable at runtime from the desktop's Settings window, so "theme"
//! means one thing across the whole system instead of each `.ling` screen
//! hardcoding its own copy of the colors (which is exactly what
//! `apps/gui-common` did before this existed, and why its palette and the
//! WM's could have drifted).
//!
//! Selection state lives here (kernel-side `static mut`) for the same
//! reason `wm_liquid`'s does: the `.ling` AOT path has no mutable
//! rebinding, so a "current theme" that survives frame to frame has
//! nowhere else to live. Switching also re-themes the VGA text console
//! (`vga::apply_theme`) so a later drop to text mode matches -- one theme
//! choice, every surface.
//!
//! Colors are 0xRRGGBB u32s, same convention as `framebuffer.rs`. Slots
//! are a fixed contract with `.ling` callers (`ling_kernel_theme_color(n)`)
//! -- add to the end, never renumber.

use crate::drivers::vga;

pub const SLOT_BG: usize = 0; // desktop / screen background
pub const SLOT_PANEL: usize = 1; // window body, cards
pub const SLOT_PANEL_BORDER: usize = 2; // card borders, dividers
pub const SLOT_TEXT: usize = 3; // primary text
pub const SLOT_DIM: usize = 4; // secondary text, hints
pub const SLOT_ERROR: usize = 5; // error text
pub const SLOT_TITLEBAR: usize = 6; // focused window titlebar
pub const SLOT_TITLEBAR_IDLE: usize = 7; // unfocused window titlebar
pub const SLOT_TITLEBAR_TEXT: usize = 8;
pub const SLOT_DOCK: usize = 9; // dock plate (blended over wallpaper)
pub const SLOT_ACCENT: usize = 10; // selection rings, focused highlights
pub const SLOT_SHADOW: usize = 11; // window drop shadow (alpha at draw time)
pub const SLOT_WALL_TOP: usize = 12; // wallpaper gradient, top edge
pub const SLOT_WALL_BOTTOM: usize = 13; // wallpaper gradient, bottom edge
pub const SLOT_DOT_DIM: usize = 14; // stepper dot, not yet reached
pub const SLOT_COUNT: usize = 15;

pub struct UiTheme {
    pub name: &'static str,
    pub colors: [u32; SLOT_COUNT],
    /// Whether the VGA text console should use its dark or light palette
    /// when this theme is active -- keeps a text-mode drop consistent.
    pub vga_dark: bool,
}

/// Theme 0 is the palette `apps/gui-common` shipped with (bg 0x1A1423 dark
/// purple etc.) -- existing screens look identical by default, they just
/// stop owning the numbers.
pub static THEMES: [UiTheme; 9] = [
    UiTheme {
        name: "Dusk",
        colors: [
            0x1A1423, // bg
            0x241B33, // panel
            0x3A2F4D, // panel border
            0xFFFFFF, // text
            0x9B8FB0, // dim
            0xE55800, // error (amber)
            0x362A4D, // titlebar focused
            0x241B33, // titlebar idle
            0xFFFFFF, // titlebar text
            0x120E1A, // dock
            0x4A8FE0, // accent (blue)
            0x000000, // shadow
            0x2B2140, // wallpaper top
            0x120E1A, // wallpaper bottom
            0x4A4058, // dot dim
        ],
        vga_dark: true,
    },
    UiTheme {
        name: "Daylight",
        colors: [
            0xE9E5F2, // bg
            0xFAF8FE, // panel
            0xC8C0DA, // panel border
            0x241B33, // text
            0x6E6486, // dim
            0xB33A14, // error
            0xD9D2E8, // titlebar focused
            0xEDE9F5, // titlebar idle
            0x241B33, // titlebar text
            0xF3F0FA, // dock
            0x3A6FD0, // accent
            0x241B33, // shadow
            0xF2EFF9, // wallpaper top
            0xCFC6E0, // wallpaper bottom
            0xB4ABc8, // dot dim
        ],
        vga_dark: false,
    },
    hue_theme("Red", 0xE0453A),
    hue_theme("Orange", 0xE08A3A),
    hue_theme("Yellow", 0xE0D24A),
    // Green keeps the old "Jade" palette's soul (same accent hue) under
    // its plain-color name, per request.
    hue_theme("Green", 0x4AE0A0),
    hue_theme("Blue", 0x3A8AE0),
    hue_theme("Indigo", 0x5A4AE0),
    hue_theme("Violet", 0x9A4AE0),
];

/// Per-channel mix: `pct`% of `tint` into `base` -- const so the ROYGBIV
/// themes are baked at compile time, not computed per lookup.
const fn mix(base: u32, tint: u32, pct: u32) -> u32 {
    let mut out = 0u32;
    let mut ch = 0;
    while ch < 3 {
        let b = (base >> (ch * 8)) & 0xFF;
        let t = (tint >> (ch * 8)) & 0xFF;
        let m = (b * (100 - pct) + t * pct) / 100;
        out |= m << (ch * 8);
        ch += 1;
    }
    out
}

/// One dark theme derived from a single hue -- every surface is the same
/// near-black tinted by a slot-specific amount of the hue, so all seven
/// ROYGBIV themes share structure (and readability) and differ only in
/// color. The accent is the hue itself lifted toward white.
const fn hue_theme(name: &'static str, hue: u32) -> UiTheme {
    UiTheme {
        name,
        colors: [
            mix(0x0E0E14, hue, 12),  // bg
            mix(0x16161E, hue, 15),  // panel
            mix(0x232330, hue, 30),  // panel border
            0xF2F2F8,                // text
            mix(0x9898A8, hue, 20),  // dim
            0xE05840,                // error (fixed across hues)
            mix(0x1C1C26, hue, 35),  // titlebar focused
            mix(0x16161E, hue, 15),  // titlebar idle
            0xF2F2F8,                // titlebar text
            mix(0x0A0A10, hue, 15),  // dock
            mix(hue, 0xFFFFFF, 25),  // accent
            0x000000,                // shadow
            mix(0x181820, hue, 25),  // wallpaper top
            mix(0x0A0A10, hue, 10),  // wallpaper bottom
            mix(0x3A3A48, hue, 15),  // dot dim
        ],
        vga_dark: true,
    }
}

static mut CURRENT: usize = 0;

pub fn count() -> usize {
    THEMES.len()
}

pub fn current() -> usize {
    unsafe { CURRENT }
}

pub fn name(idx: usize) -> &'static str {
    THEMES.get(idx).map(|t| t.name).unwrap_or("")
}

pub fn color(slot: usize) -> u32 {
    let t = &THEMES[unsafe { CURRENT } % THEMES.len()];
    t.colors.get(slot).copied().unwrap_or(0xFF00FF)
}

/// Switch the active theme (index wraps, so a Settings "next theme" button
/// can just pass `current()+1`) and re-theme the VGA console to match.
pub fn set(idx: usize) {
    unsafe {
        CURRENT = idx % THEMES.len();
    }
    if THEMES[unsafe { CURRENT }].vga_dark {
        vga::apply_theme(&vga::THEME_LINGOS);
    } else {
        vga::apply_theme(&vga::THEME_LIGHT);
    }
}
