//! Loadable wallpapers: decode an uncompressed 24/32bpp BMP from lingfs
//! (multi-block files made images possible at all) into a screen-sized
//! pixel cache, nearest-neighbor scaled at load time so the per-frame
//! cost is a plain row blit. BMP only, deliberately: it decodes in ~60
//! lines with no allocation and no codec surface; PNG/JPEG would mean
//! porting real decompressors -- roadmap, not faked. SVG wallpapers are
//! queued separately as a *build-time* rasterization through the same
//! pipeline that bakes the fonts.

use crate::drivers::framebuffer;
use crate::fs::lingfs;

const MAX_W: usize = 1920;
const MAX_H: usize = 1080;
/// Cache of the wallpaper scaled to the live framebuffer size (0xRRGGBB).
static mut CACHE: [u32; MAX_W * MAX_H] = [0; MAX_W * MAX_H];
static mut LOADED: bool = false;
/// Raw file scratch: biggest supported BMP (1024x768x24 + headers).
static mut FILE_BUF: [u8; 3 * 1024 * 1024] = [0; 3 * 1024 * 1024];

pub fn loaded() -> bool {
    unsafe { LOADED }
}

fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn rd_i32(b: &[u8], off: usize) -> i32 {
    rd_u32(b, off) as i32
}

/// Load `name` from lingfs and scale it into the cache at the current
/// framebuffer size. Returns false (cache untouched) on any parse issue:
/// wrong magic, compressed BMP, bpp other than 24/32, or a file bigger
/// than the scratch buffer.
pub fn load(name: &str) -> bool {
    let fbw = framebuffer::width() as usize;
    let fbh = framebuffer::height() as usize;
    if fbw == 0 || fbw > MAX_W || fbh > MAX_H {
        return false;
    }
    let file = unsafe { &mut *&raw mut FILE_BUF };
    let Ok(Some(len)) = lingfs::read_file_all(name, file) else { return false };
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

    let cache = unsafe { &mut *&raw mut CACHE };
    for y in 0..fbh {
        let sy = y * h / fbh;
        let src_row = if bottom_up { h - 1 - sy } else { sy };
        let row = &file[data_off + src_row * stride..];
        for x in 0..fbw {
            let sx = x * w / fbw;
            let p = sx * bypp;
            // BMP rows are BGR(A); framebuffer colors are 0xRRGGBB.
            let (b, g, r) = (row[p] as u32, row[p + 1] as u32, row[p + 2] as u32);
            cache[y * MAX_W + x] = (r << 16) | (g << 8) | b;
        }
    }
    unsafe { LOADED = true };
    true
}

/// Blit the cached wallpaper (already framebuffer-sized) into the back
/// buffer -- one row-blit per line, not per-pixel calls (a full frame of
/// `back_set_pixel`s measurably crawls under TCG).
pub fn draw() {
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
