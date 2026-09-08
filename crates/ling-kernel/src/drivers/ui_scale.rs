//! Resolution-independent UI coordinates: author every screen once against
//! a fixed virtual canvas, and this maps it to whatever real resolution
//! GRUB actually negotiated (`framebuffer.rs`'s own module doc: the
//! Multiboot2 header asks for "any" mode and reads back whatever it got,
//! not an assumed one -- confirmed real, not guessed, before this module
//! was written). Without this, every `ling_kernel_fb_*` call in `.ling`
//! screen code took a raw pixel literal, which only looked right at
//! whatever resolution it happened to be tested at.
//!
//! Uniform scale + centered letterbox (not independent x/y stretch) --
//! stretching would warp circles into ellipses and make text look squashed;
//! letterboxing keeps the aspect ratio honest and centers the unused
//! margin instead.
//!
//! Real, disclosed limitation: this scales *positions and rectangle sizes*,
//! not glyph rendering -- `font8x8`/`font_unicode` are fixed-size bitmap
//! atlases baked at `ling build` time (see their own module docs), so text
//! stays a constant pixel size regardless of `scale()`. At a much larger
//! real resolution than the 960x600 virtual canvas, everything else grows
//! but text doesn't -- a real gap, not yet closed. Closing it for real
//! means baking multiple discrete font sizes at build time and picking
//! among them by `scale()`, not scaling bitmaps at runtime (that's what
//! turns text to mush) -- future work, not implemented here.
use crate::drivers::framebuffer;

const VIRTUAL_W: f64 = 960.0;
const VIRTUAL_H: f64 = 600.0;

/// User "DPI" preference (Settings "DPI" row): the factor the desktop
/// multiplies its bitmap-font glyph size by, so text on a large
/// high-resolution panel isn't hair-thin. 100 = native 8px. Deliberately
/// SEPARATE from the auto letterbox `scale()` above (which fits full-screen
/// `.ling` layouts to the panel and must not be user-inflated, or they'd
/// spill off-screen). The desktop applies `dpi_font_scale()` to `font8x8`.
///
/// Only integer steps because the font is an 8x8 bitmap atlas -- 1.5x
/// nearest-neighbor turns text to mush (see `font8x8::draw_char_scaled`), so
/// we offer honest crisp 1x/2x rather than fake fractional DPI.
pub const DPI_PRESETS: [u32; 2] = [100, 200];
static mut DPI_IDX: usize = 0;

pub fn dpi_index() -> usize {
    unsafe { DPI_IDX }
}
pub fn set_dpi_index(i: usize) {
    unsafe { DPI_IDX = i.min(DPI_PRESETS.len() - 1) };
}
pub fn dpi_pct() -> u32 {
    DPI_PRESETS[dpi_index()]
}
/// The integer glyph scale the desktop feeds `font8x8::set_scale`.
pub fn dpi_font_scale() -> u32 {
    (dpi_pct() / 100).max(1)
}

fn scale() -> f64 {
    let w = framebuffer::width() as f64;
    let h = framebuffer::height() as f64;
    if w <= 0.0 || h <= 0.0 {
        return 1.0;
    }
    (w / VIRTUAL_W).min(h / VIRTUAL_H)
}

fn offset() -> (f64, f64) {
    let s = scale();
    let w = framebuffer::width() as f64;
    let h = framebuffer::height() as f64;
    (((w - VIRTUAL_W * s) / 2.0).max(0.0), ((h - VIRTUAL_H * s) / 2.0).max(0.0))
}

/// Map a virtual-canvas x-coordinate (0..960) to a real framebuffer pixel.
pub fn x(vx: u64) -> u32 {
    let (ox, _) = offset();
    (ox + vx as f64 * scale()) as u32
}

/// Map a virtual-canvas y-coordinate (0..600) to a real framebuffer pixel.
pub fn y(vy: u64) -> u32 {
    let (_, oy) = offset();
    (oy + vy as f64 * scale()) as u32
}

/// Scale a length (width/height/radius) by the same factor as `x`/`y`,
/// with no offset -- for the `w`/`h` arguments of a rect, not a position.
pub fn len(v: u64) -> u32 {
    (v as f64 * scale()) as u32
}
