//! Loadable wallpapers: decode an uncompressed 24/32bpp BMP (from lingfs or
//! the embedded built-in) into a screen-sized pixel cache, nearest-neighbor
//! scaled at load time so the per-frame cost is a plain row blit. BMP only,
//! deliberately: it decodes in ~60 lines with no allocation and no codec
//! surface; PNG/JPEG would mean porting real decompressors -- roadmap, not
//! faked. SVG wallpapers are baked to BMP at build time through the same
//! pipeline that rasterizes the fonts.
//!
//! Scaling modes (Settings > Wallpaper scale): cover / fit / stretch / tile /
//! center -- the same set a desktop OS offers, computed into the cache once.
//! The cache is re-decoded whenever the framebuffer size changes (a live
//! resolution switch), from whatever source was last loaded (embedded or a
//! file), so the wallpaper always fills the current screen.

use crate::drivers::framebuffer;
use crate::fs::lingfs;

const MAX_W: usize = 1920;
const MAX_H: usize = 1080;
/// Cache of the wallpaper scaled to the live framebuffer size (0xRRGGBB).
static mut CACHE: [u32; MAX_W * MAX_H] = [0; MAX_W * MAX_H];
static mut LOADED: bool = false;
/// Raw file scratch: big enough for a full 1920x1080x32 BMP + headers.
static mut FILE_BUF: [u8; MAX_W * MAX_H * 4 + 256] = [0; MAX_W * MAX_H * 4 + 256];

// Scaling modes.
pub const SCALE_COVER: u8 = 0;
pub const SCALE_FIT: u8 = 1;
pub const SCALE_STRETCH: u8 = 2;
pub const SCALE_TILE: u8 = 3;
pub const SCALE_CENTER: u8 = 4;
pub const SCALE_MODES: u8 = 5;
static mut SCALE_MODE: u8 = SCALE_COVER;
/// Letterbox/margin fill for fit/center (a neutral desktop dark).
const MARGIN: u32 = 0x0A_0A_12;

// The size the cache was last decoded at + the source to re-decode from when
// the framebuffer resolution changes.
static mut DECODED_W: u32 = 0;
static mut DECODED_H: u32 = 0;
static mut SRC_IS_FILE: bool = false;
static mut SRC_PATH: [u8; 128] = [0; 128];
static mut SRC_PATH_LEN: usize = 0;

pub fn loaded() -> bool {
    unsafe { LOADED }
}

pub fn scale_mode() -> u8 {
    unsafe { SCALE_MODE }
}
pub fn set_scale_mode(m: u8) {
    unsafe { SCALE_MODE = m % SCALE_MODES };
    reload();
}
pub fn scale_mode_name(m: u8) -> &'static str {
    match m % SCALE_MODES {
        SCALE_COVER => "Cover",
        SCALE_FIT => "Fit",
        SCALE_STRETCH => "Stretch",
        SCALE_TILE => "Tile",
        _ => "Center",
    }
}

/// Native size of the last decoded image (before screen-scaling).
pub fn cached_size() -> (u32, u32) {
    (framebuffer::width(), framebuffer::height())
}

/// One pixel of the (framebuffer-sized) decode cache.
pub fn cached_pixel(x: u32, y: u32) -> u32 {
    let (fw, fh) = (framebuffer::width(), framebuffer::height());
    if x >= fw || y >= fh {
        return 0;
    }
    let cache = unsafe { &*&raw const CACHE };
    cache[y as usize * MAX_W + x as usize]
}

fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn rd_i32(b: &[u8], off: usize) -> i32 {
    rd_u32(b, off) as i32
}

pub static LING_COUNTRY_BMP: &[u8] = include_bytes!("../../assets/ling_country_wallpaper.bmp");

/// Load the built-in wallpaper from its embedded (already-rasterized) BMP.
pub fn load_embedded() -> bool {
    unsafe {
        SRC_IS_FILE = false;
    }
    decode(LING_COUNTRY_BMP, LING_COUNTRY_BMP.len())
}

/// Load a BMP wallpaper by lingfs path (used by "set as wallpaper" + Settings).
pub fn load(name: &str) -> bool {
    let file = unsafe { &mut *&raw mut FILE_BUF };
    let Ok(Some(len)) = lingfs::read_file_all(name, file) else { return false };
    let ok = decode(unsafe { &*&raw const FILE_BUF }, len);
    if ok {
        unsafe {
            SRC_IS_FILE = true;
            let n = name.len().min(SRC_PATH.len());
            SRC_PATH[..n].copy_from_slice(&name.as_bytes()[..n]);
            SRC_PATH_LEN = n;
        }
    }
    ok
}

/// Re-decode from the current source (embedded or file) at the current
/// framebuffer size / scale mode. Cheap no-op cost is only paid on demand.
pub fn reload() -> bool {
    if unsafe { SRC_IS_FILE } {
        let path = unsafe {
            core::str::from_utf8(&(&*&raw const SRC_PATH)[..SRC_PATH_LEN]).unwrap_or("")
        };
        if path.is_empty() {
            return false;
        }
        // Re-read the file (decode records the source again -- harmless).
        let file = unsafe { &mut *&raw mut FILE_BUF };
        let Ok(Some(len)) = lingfs::read_file_all(path, file) else { return false };
        decode(unsafe { &*&raw const FILE_BUF }, len)
    } else if loaded() {
        decode(LING_COUNTRY_BMP, LING_COUNTRY_BMP.len())
    } else {
        false
    }
}

/// Re-decode if the framebuffer size changed since the last decode (a live
/// resolution switch). Called once per frame from `draw`; only does work when
/// the size actually differs.
pub fn ensure_size() {
    let (fw, fh) = (framebuffer::width(), framebuffer::height());
    let stale = unsafe { LOADED && (DECODED_W != fw || DECODED_H != fh) };
    if stale {
        reload();
    }
}

/// Decode an uncompressed 24/32bpp BMP from `file[..len]`, scaling it into the
/// framebuffer-sized cache per the active scale mode.
fn decode(file: &[u8], len: usize) -> bool {
    let fbw = framebuffer::width() as usize;
    let fbh = framebuffer::height() as usize;
    if fbw == 0 || fbw > MAX_W || fbh > MAX_H {
        return false;
    }
    if len < 54 || file[0] != b'B' || file[1] != b'M' {
        return false;
    }
    let data_off = rd_u32(file, 10) as usize;
    let hdr_size = rd_u32(file, 14) as usize;
    if hdr_size < 40 {
        return false;
    }
    let w = rd_i32(file, 18);
    let h_raw = rd_i32(file, 22);
    let bpp = u16::from_le_bytes([file[28], file[29]]) as usize;
    let compression = rd_u32(file, 30);
    if w <= 0 || w as usize > 4096 || h_raw == 0 || compression != 0 || (bpp != 24 && bpp != 32) {
        return false;
    }
    let (h, bottom_up) = if h_raw > 0 { (h_raw as usize, true) } else { ((-h_raw) as usize, false) };
    let w = w as usize;
    let bypp = bpp / 8;
    let stride = (w * bypp + 3) & !3;
    if data_off + stride * h > len {
        return false;
    }
    let mode = unsafe { SCALE_MODE };

    // Sample one image pixel (image coords) as 0xRRGGBB.
    let sample = |sx: usize, sy: usize| -> u32 {
        let sx = sx.min(w - 1);
        let sy = sy.min(h - 1);
        let src_row = if bottom_up { h - 1 - sy } else { sy };
        let p = data_off + src_row * stride + sx * bypp;
        let (b, g, r) = (file[p] as u32, file[p + 1] as u32, file[p + 2] as u32);
        (r << 16) | (g << 8) | b
    };

    // Cover geometry (fill screen, crop overflow, centered).
    let cover_by_height = w * fbh >= h * fbw;
    let (cover_crop_x, cover_crop_y) = if cover_by_height {
        (((w * fbh / h).saturating_sub(fbw)) / 2, 0)
    } else {
        (0, ((h * fbw / w).saturating_sub(fbh)) / 2)
    };
    // Fit geometry (contain inside screen, letterbox margins, centered).
    let (fit_w, fit_h) = if fbw * h >= w * fbh {
        (w * fbh / h, fbh) // screen wider than image -> height-limited
    } else {
        (fbw, h * fbw / w) // width-limited
    };
    let fit_ox = fbw.saturating_sub(fit_w) / 2;
    let fit_oy = fbh.saturating_sub(fit_h) / 2;
    // Center geometry (native size, centered; may crop if larger than screen).
    let cen_ox = (fbw as i64 - w as i64) / 2;
    let cen_oy = (fbh as i64 - h as i64) / 2;

    let cache = unsafe { &mut *&raw mut CACHE };
    for y in 0..fbh {
        for x in 0..fbw {
            let px = match mode {
                SCALE_STRETCH => sample(x * w / fbw, y * h / fbh),
                SCALE_TILE => sample(x % w, y % h),
                SCALE_FIT => {
                    if x >= fit_ox && x < fit_ox + fit_w && y >= fit_oy && y < fit_oy + fit_h && fit_w > 0 && fit_h > 0 {
                        sample((x - fit_ox) * w / fit_w, (y - fit_oy) * h / fit_h)
                    } else {
                        MARGIN
                    }
                },
                SCALE_CENTER => {
                    let ix = x as i64 - cen_ox;
                    let iy = y as i64 - cen_oy;
                    if ix >= 0 && (ix as usize) < w && iy >= 0 && (iy as usize) < h {
                        sample(ix as usize, iy as usize)
                    } else {
                        MARGIN
                    }
                },
                _ => {
                    // SCALE_COVER (default).
                    let sx = if cover_by_height { (x + cover_crop_x) * h / fbh } else { x * w / fbw };
                    let sy = if cover_by_height { y * h / fbh } else { (y + cover_crop_y) * w / fbw };
                    sample(sx, sy)
                },
            };
            cache[y * MAX_W + x] = px;
        }
    }
    unsafe {
        LOADED = true;
        DECODED_W = fbw as u32;
        DECODED_H = fbh as u32;
    }
    true
}

/// Blit the cached wallpaper (framebuffer-sized) into the back buffer -- one
/// row-blit per line. Re-decodes first if the resolution changed.
pub fn draw() {
    ensure_size();
    if !loaded() {
        return;
    }
    let fbh = framebuffer::height();
    let fbw = framebuffer::width() as usize;
    let cache = unsafe { &*&raw const CACHE };
    for y in 0..fbh {
        framebuffer::back_blit_row(y, &cache[y as usize * MAX_W..y as usize * MAX_W + fbw]);
    }
}
