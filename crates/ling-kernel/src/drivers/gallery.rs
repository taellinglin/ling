//! Gallery: view image files from lingfs. Reuses the `wallpaper` BMP
//! decoder (same 24/32bpp uncompressed support, same honest limits) but
//! draws the decoded image *fit inside the window* rather than as the
//! desktop background, with integer zoom and the "set as wallpaper"
//! convenience. One image at a time -- a viewer, not a thumbnail grid
//! (that needs a directory-scan UI this doesn't have yet, disclosed).
//!
//! It shares `wallpaper`'s decode cache: opening an image in the Gallery
//! decodes it into that cache, and "set as wallpaper" (w key) then just
//! flips the desktop's wallpaper mode to Image -- no second decode.

use crate::drivers::{font8x8, framebuffer, image, theme, wallpaper};

static mut HAS_IMAGE: bool = false;
static mut IMG_W: u32 = 0;
static mut IMG_H: u32 = 0;
static mut ZOOM: u32 = 1;
static mut NAME: [u8; 64] = [0; 64];
static mut NAME_LEN: usize = 0;
/// A PNG/SVG picture decoded by `drivers::image` (BMPs live in the shared
/// wallpaper cache instead, so "set as wallpaper" stays a no-copy flip).
static mut PIC: Option<image::Image> = None;

fn set_name(path: &str) {
    unsafe {
        let n = path.len().min(64);
        let nm = &mut *&raw mut NAME;
        nm[..n].copy_from_slice(&path.as_bytes()[..n]);
        NAME_LEN = n;
    }
}

/// Open a PNG or SVG (by extension or content sniff) through `drivers::image`.
fn open_decoded(path: &str) -> bool {
    let lower = |s: &str, ext: &str| s.len() >= ext.len() && s[s.len() - ext.len()..].eq_ignore_ascii_case(ext);
    let Some(size) = crate::fs::lingfs::file_size(path) else { return false };
    let mut data = alloc::vec![0u8; size.min(16 * 1024 * 1024)];
    let Ok(Some(n)) = crate::fs::lingfs::read_file_all(path, &mut data) else { return false };
    let data = &data[..n];
    let img = if lower(path, ".svg") || image::looks_svg(data) {
        image::decode_svg_capped(data, 1024)
    } else {
        image::decode_png(data)
    };
    let Some(img) = img else { return false };
    unsafe {
        IMG_W = img.w;
        IMG_H = img.h;
        PIC = Some(img);
        HAS_IMAGE = true;
        ZOOM = 1;
    }
    set_name(path);
    true
}

pub fn open(path: &str) -> bool {
    let lower = |ext: &str| path.len() >= ext.len() && path[path.len() - ext.len()..].eq_ignore_ascii_case(ext);
    if lower(".png") || lower(".svg") {
        return open_decoded(path);
    }
    if wallpaper::load(path) {
        unsafe {
            PIC = None;
            HAS_IMAGE = true;
            let (w, h) = wallpaper::cached_size();
            IMG_W = w;
            IMG_H = h;
            ZOOM = 1;
            let n = path.len().min(64);
            let nm = &mut *&raw mut NAME;
            nm[..n].copy_from_slice(&path.as_bytes()[..n]);
            NAME_LEN = n;
        }
        true
    } else {
        false
    }
}

pub fn key(k: u8) {
    use crate::drivers::keyboard as kb;
    unsafe {
        match k {
            kb::CTRL_ZOOM_IN => ZOOM = (ZOOM + 1).min(4),
            kb::CTRL_ZOOM_OUT => ZOOM = (ZOOM - 1).max(1),
            kb::CTRL_ZOOM_RESET => ZOOM = 1,
            b'w' => {
                // Promote the currently-viewed image to the wallpaper (BMPs:
                // they are already in the wallpaper's decode cache).
                if HAS_IMAGE && (*(&raw const PIC)).is_none() {
                    crate::drivers::wm::set_wallpaper_current();
                }
            },
            _ => {},
        }
    }
}

fn name() -> &'static str {
    unsafe { core::str::from_utf8(&(&*&raw const NAME)[..NAME_LEN]).unwrap_or("") }
}

pub fn draw(x: u32, y: u32, w: u32, h: u32) {
    let panel = theme::color(theme::SLOT_PANEL);
    let dim = theme::color(theme::SLOT_DIM);
    let accent = theme::color(theme::SLOT_ACCENT);
    if !unsafe { HAS_IMAGE } {
        font8x8::draw_str(x, y, b"Gallery", accent, panel);
        font8x8::draw_str(x, y + 20, b"open a .bmp, .png or .svg from Files to view it here", dim, panel);
        return;
    }
    font8x8::draw_str(x, y, name().as_bytes(), accent, panel);
    font8x8::draw_str(x, y + 16, b"ctrl +/- zoom, ctrl 0 reset, w = set wallpaper", dim, panel);
    // Fit the cached image inside the content area (below the 2 header
    // lines), honoring zoom, centered, nearest-neighbor.
    let area_y = y + 36;
    let area_h = h.saturating_sub(44);
    let (iw, ih) = unsafe { (IMG_W, IMG_H) };
    if iw == 0 || ih == 0 {
        return;
    }
    let z = unsafe { ZOOM };
    // Base fit scale as a percentage so it fills without overflowing.
    let fit_num = (w * 100 / iw).min(area_h * 100 / ih).max(1);
    let eff = fit_num * z; // percent
    let dw = (iw * eff / 100).min(w);
    let dh = (ih * eff / 100).min(area_h);
    let ox = x + (w.saturating_sub(dw)) / 2;
    let oy = area_y + (area_h.saturating_sub(dh)) / 2;
    for row in 0..dh {
        let sy = row * ih / dh.max(1);
        for col in 0..dw {
            let sx = col * iw / dw.max(1);
            let px = match unsafe { &*(&raw const PIC) } {
                Some(img) => img.px.get((sy * iw + sx) as usize).copied().unwrap_or(0),
                None => wallpaper::cached_pixel(sx, sy),
            };
            framebuffer::back_set_pixel(ox + col, oy + row, px);
        }
    }
}
