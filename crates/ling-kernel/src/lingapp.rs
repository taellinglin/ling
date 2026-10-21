//! App-facing builtins for compiled `.ling` programs: the window, `px_*`
//! drawing, fonts, input, and a read-only file layer — enough for a full UI app
//! like LingStationX to draw itself on LingOS.
//!
//! The window is the whole screen (the framebuffer's safe area). The program
//! draws into a `0xRRGGBB` back buffer exactly as on the host; `present()` blits
//! it to the panel. Every `px_*` primitive is a line-for-line port of the host's
//! (`ling/src/runtime/daw.rs`) so the pixels match: clip-aware, alpha-blended,
//! SDF-antialiased rounded rects and discs, the AA line, and LingStation's
//! "Black" colour remap. Text uses fontdue (pure Rust, no_std) over the app's own
//! TTF/OTF files from [`crate::appfs`], with the host's metrics: `y` is the top of
//! the line, the baseline sits at `y + ascent`, the pen advances by each glyph's
//! advance (no kerning).
//!
//! Input builtins report "nothing happening" until the USB HID path is wired to
//! them; files come from the embedded table until LingOS can read the SD card.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::drivers::framebuffer;
use crate::lingrt::{boolean, list_from, mk_str, num, str_of, to_f64, to_string, UNIT};

// ── State ────────────────────────────────────────────────────────────────────

struct Window {
    buf: Vec<u32>,
    w: usize,
    h: usize,
    open: bool,
    clip: Option<[i32; 4]>,
    black: bool,
}

struct Glyph {
    w: usize,
    h: usize,
    xmin: i32,
    ymin: i32,
    cov: Vec<u8>,
}

struct Font {
    fd: fontdue::Font,
    cache: BTreeMap<(char, u32), Glyph>,
}

static mut WIN: Option<Window> = None;
/// The host gfx API's current drawing colour (0xRRGGBB) and the number of
/// frames presented so far (`frame_count`).
static mut GFX_COLOR: u32 = 0xFF_FFFF;
static mut FRAMES: u64 = 0;
static mut FONTS: Vec<Font> = Vec::new();

fn win() -> Option<&'static mut Window> {
    unsafe { (*(&raw mut WIN)).as_mut() }
}

fn fonts() -> &'static mut Vec<Font> {
    unsafe { &mut *(&raw mut FONTS) }
}

// ── Colour helpers (host semantics) ──────────────────────────────────────────

#[inline]
fn blend(dst: u32, r: u32, g: u32, b: u32, a: f32) -> u32 {
    let mix = |s: u32, d: u32| ((s as f32 * a + d as f32 * (1.0 - a)) as u32).min(255);
    (mix(r, (dst >> 16) & 255) << 16) | (mix(g, (dst >> 8) & 255) << 8) | mix(b, dst & 255)
}

fn unpack(c: f64) -> (u32, u32, u32) {
    let c = if c > 0.0 { c as u32 } else { 0 };
    ((c >> 16) & 255, (c >> 8) & 255, c & 255)
}

/// LingStation's "Black" colour scheme: remap the chrome greys to near-black.
fn black_theme(rgb: (u32, u32, u32)) -> (u32, u32, u32) {
    match rgb {
        (27, 27, 27) => (0, 0, 0),
        (22, 23, 26) => (4, 4, 5),
        (60, 60, 60) => (26, 26, 26),
        (74, 74, 74) => (38, 38, 38),
        (45, 45, 45) => (16, 16, 16),
        (30, 30, 34) => (10, 10, 12),
        (70, 70, 78) => (48, 48, 54),
        (205, 208, 216) => (245, 245, 245),
        (140, 144, 152) => (168, 172, 180),
        other => other,
    }
}

fn tu(w: &Window, c: f64) -> (u32, u32, u32) {
    let rgb = unpack(c);
    if w.black {
        black_theme(rgb)
    } else {
        rgb
    }
}

fn clip_box(w: &Window) -> (i32, i32, i32, i32) {
    let (bw, bh) = (w.w as i32, w.h as i32);
    match w.clip {
        Some(c) => (c[0].max(0), c[1].max(0), c[2].min(bw), c[3].min(bh)),
        None => (0, 0, bw, bh),
    }
}

// ── px_* primitives ──────────────────────────────────────────────────────────

/// Argument accessor: `a(i, default)` as a number (host `Self::num`).
type Arg<'a> = &'a dyn Fn(usize, f64) -> f64;

fn px_rect_like(name: &[u8], a: Arg) {
    let Some(w) = win() else { return };
    let (x, y, mut ww, mut hh) = (a(0, 0.0), a(1, 0.0), a(2, 0.0), a(3, 0.0));
    let (r, g, b, al);
    match name {
        b"px_hline" => {
            hh = 1.0;
            (r, g, b) = tu(w, a(3, 0.0));
            al = a(4, 1.0) as f32;
        },
        b"px_vline" => {
            hh = ww;
            ww = 1.0;
            (r, g, b) = tu(w, a(3, 0.0));
            al = a(4, 1.0) as f32;
        },
        _ => {
            (r, g, b) = tu(w, a(4, 0.0));
            al = a(5, 1.0) as f32;
        },
    }
    let (cx0, cy0, cx1, cy1) = clip_box(w);
    let bw = w.w as i32;
    let col = (r.min(255) << 16) | (g.min(255) << 8) | b.min(255);
    let buf = &mut w.buf;
    let mut fill = |x0: i32, y0: i32, x1: i32, y1: i32| {
        let (x0, y0, x1, y1) = (x0.max(cx0), y0.max(cy0), x1.min(cx1), y1.min(cy1));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        for yy in y0..y1 {
            let row = (yy * bw) as usize;
            if al >= 0.999 {
                buf[row + x0 as usize..row + x1 as usize].fill(col);
            } else {
                for xx in x0..x1 {
                    let i = row + xx as usize;
                    buf[i] = blend(buf[i], r.min(255), g.min(255), b.min(255), al);
                }
            }
        }
    };
    let (x0, y0) = (libm::round(x) as i32, libm::round(y) as i32);
    let (x1, y1) = (libm::round(x + ww) as i32, libm::round(y + hh) as i32);
    if name == b"px_outline" {
        fill(x0, y0, x1, y0 + 1);
        fill(x0, y1 - 1, x1, y1);
        fill(x0, y0 + 1, x0 + 1, y1 - 1);
        fill(x1 - 1, y0 + 1, x1, y1 - 1);
    } else {
        fill(x0, y0, x1.max(x0 + 1), y1.max(y0 + 1));
    }
}

fn px_rrect(outline: bool, a: Arg) {
    let Some(w) = win() else { return };
    let (x, y, ww, hh, rad) = (a(0, 0.0), a(1, 0.0), a(2, 0.0), a(3, 0.0), a(4, 0.0));
    let (r, g, b) = tu(w, a(5, 0.0));
    let al = a(6, 1.0) as f32;
    let (cx0, cy0, cx1, cy1) = clip_box(w);
    let bw = w.w as i32;
    let (hx, hy) = (ww * 0.5, hh * 0.5);
    let rad = rad.min(hx).min(hy).max(0.0);
    let (ccx, ccy) = (x + hx, y + hy);
    let x0 = (libm::floor(x) as i32).max(cx0);
    let x1 = (libm::ceil(x + ww) as i32).min(cx1);
    let y0 = (libm::floor(y) as i32).max(cy0);
    let y1 = (libm::ceil(y + hh) as i32).min(cy1);
    for yy in y0..y1 {
        for xx in x0..x1 {
            let px = (xx as f64 + 0.5) - ccx;
            let py = (yy as f64 + 0.5) - ccy;
            let qx = libm::fabs(px) - (hx - rad);
            let qy = libm::fabs(py) - (hy - rad);
            let (mx, my) = (qx.max(0.0), qy.max(0.0));
            let d = libm::sqrt(mx * mx + my * my) + qx.max(qy).min(0.0) - rad;
            let cov = if outline {
                (0.5 - libm::fabs(d + 0.5)).clamp(0.0, 1.0)
            } else {
                (0.5 - d).clamp(0.0, 1.0)
            } as f32;
            if cov <= 0.0 {
                continue;
            }
            let i = (yy * bw + xx) as usize;
            w.buf[i] = blend(w.buf[i], r, g, b, al * cov);
        }
    }
}

fn px_disc(ring: bool, a: Arg) {
    let Some(w) = win() else { return };
    let (cx, cy, rad) = (a(0, 0.0), a(1, 0.0), a(2, 0.0));
    let (r, g, b) = tu(w, a(3, 0.0));
    let al = a(4, 1.0) as f32;
    let (cx0, cy0, cx1, cy1) = clip_box(w);
    let bw = w.w as i32;
    let x0 = (libm::floor(cx - rad - 1.0) as i32).max(cx0);
    let x1 = (libm::ceil(cx + rad + 1.0) as i32).min(cx1);
    let y0 = (libm::floor(cy - rad - 1.0) as i32).max(cy0);
    let y1 = (libm::ceil(cy + rad + 1.0) as i32).min(cy1);
    for yy in y0..y1 {
        for xx in x0..x1 {
            let (dx, dy) = ((xx as f64 + 0.5) - cx, (yy as f64 + 0.5) - cy);
            let d = libm::sqrt(dx * dx + dy * dy);
            let cov = if ring {
                (1.0 - libm::fabs(d - rad)).clamp(0.0, 1.0)
            } else {
                (rad - d + 0.5).clamp(0.0, 1.0)
            } as f32;
            if cov <= 0.0 {
                continue;
            }
            let i = (yy * bw + xx) as usize;
            w.buf[i] = blend(w.buf[i], r.min(255), g.min(255), b.min(255), al * cov);
        }
    }
}

fn px_line(a: Arg) {
    let Some(w) = win() else { return };
    let (mut x0, mut y0, mut x1, mut y1) = (a(0, 0.0), a(1, 0.0), a(2, 0.0), a(3, 0.0));
    let (r, g, b) = tu(w, a(4, 0.0));
    let al = a(5, 1.0) as f32;
    let (cx0, cy0, cx1, cy1) = clip_box(w);
    let bw = w.w as i32;
    if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
        return;
    }
    let steep = libm::fabs(y1 - y0) > libm::fabs(x1 - x0);
    if steep {
        core::mem::swap(&mut x0, &mut y0);
        core::mem::swap(&mut x1, &mut y1);
    }
    if x0 > x1 {
        core::mem::swap(&mut x0, &mut x1);
        core::mem::swap(&mut y0, &mut y1);
    }
    let dx = x1 - x0;
    let grad = if libm::fabs(dx) < 1e-9 { 1.0 } else { (y1 - y0) / dx };
    let mut y = y0;
    let mut x = libm::round(x0);
    y += grad * (x - x0);
    let mut xe = libm::round(x1);
    let (lo, hi) = if steep {
        (cy0 as f64 - 2.0, cy1 as f64 + 2.0)
    } else {
        (cx0 as f64 - 2.0, cx1 as f64 + 2.0)
    };
    if x < lo {
        y += grad * (lo - x);
        x = lo;
    }
    if xe > hi {
        xe = hi;
    }
    let buf = &mut w.buf;
    let mut plot = |px: i32, py: i32, cov: f64| {
        if px < cx0 || py < cy0 || px >= cx1 || py >= cy1 {
            return;
        }
        let i = (py * bw + px) as usize;
        buf[i] = blend(buf[i], r.min(255), g.min(255), b.min(255), al * cov as f32);
    };
    while x <= xe {
        let yi = libm::floor(y);
        let f = y - yi;
        if steep {
            plot(yi as i32, x as i32, 1.0 - f);
            plot(yi as i32 + 1, x as i32, f);
        } else {
            plot(x as i32, yi as i32, 1.0 - f);
            plot(x as i32, yi as i32 + 1, f);
        }
        y += grad;
        x += 1.0;
    }
}

// ── Fonts ────────────────────────────────────────────────────────────────────

/// Paths already parsed → font index: the desktop and the apps it launches
/// share big fonts (a CJK face takes seconds to parse on a Pi) instead of
/// re-parsing them on every launch.
static mut FONT_PATHS: BTreeMap<String, usize> = BTreeMap::new();

fn font_load(path: &str) -> f64 {
    let paths = unsafe { &mut *(&raw mut FONT_PATHS) };
    if let Some(&i) = paths.get(path) {
        return i as f64;
    }
    let Some(bytes) = crate::vfs::read(path) else { return -1.0 };
    match fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()) {
        Ok(fd) => {
            let f = fonts();
            f.push(Font { fd, cache: BTreeMap::new() });
            paths.insert(String::from(path), f.len() - 1);
            (f.len() - 1) as f64
        },
        Err(_) => -1.0,
    }
}

fn font(id: f64) -> Option<&'static mut Font> {
    if id < 0.0 {
        return None;
    }
    fonts().get_mut(id as usize)
}

/// Width of `text` at `px`: the sum of glyph advances (the host's `measure`).
fn measure(f: &Font, text: &str, px: f32) -> f32 {
    text.chars().map(|c| f.fd.metrics(c, px).advance_width).sum()
}

fn ascent(f: &Font, px: f32) -> f32 {
    f.fd.horizontal_line_metrics(px).map(|m| m.ascent).unwrap_or(px * 0.8)
}

fn px_text(args: &[u64]) {
    let a = |i: usize| args.get(i).map(|&v| to_f64(v)).unwrap_or(0.0);
    let (x, y, px) = (a(1) as f32, a(2) as f32, a(3) as f32);
    let Some(w) = win() else { return };
    let Some(f) = font(a(0)) else { return };
    if px <= 0.0 {
        return;
    }
    let text = args.get(5).map(|&v| to_string(v)).unwrap_or_default();
    if text.is_empty() {
        return;
    }
    let (r, g, b) = tu(w, a(4));
    let col = (r << 16) | (g << 8) | b;
    let (cx0, cy0, cx1, cy1) = clip_box(w);
    if y > cy1 as f32 || y + px * 1.3 < cy0 as f32 || x > cx1 as f32 {
        return;
    }
    let baseline = libm::roundf(y + ascent(f, px)) as i32;
    let bw = w.w as i32;
    let mut pen = 0.0f32;
    for c in text.chars() {
        let adv = f.fd.metrics(c, px).advance_width;
        let gx0 = libm::floorf(x + pen) as i32;
        pen += adv;
        if gx0 > cx1 {
            break;
        }
        let gl = f.cache.entry((c, px.to_bits())).or_insert_with(|| {
            let (m, cov) = f.fd.rasterize(c, px);
            Glyph { w: m.width, h: m.height, xmin: m.xmin, ymin: m.ymin, cov }
        });
        let ox = gx0 + gl.xmin;
        let oy = baseline - gl.ymin - gl.h as i32;
        for gy in 0..gl.h {
            let yy = oy + gy as i32;
            if yy < cy0 || yy >= cy1 {
                continue;
            }
            let row = (yy * bw) as usize;
            for gx in 0..gl.w {
                let xx = ox + gx as i32;
                if xx < cx0 || xx >= cx1 {
                    continue;
                }
                let cv = gl.cov[gy * gl.w + gx];
                if cv == 0 {
                    continue;
                }
                let i = row + xx as usize;
                w.buf[i] = if cv == 255 { col } else { blend(w.buf[i], r, g, b, cv as f32 / 255.0) };
            }
        }
    }
}

/// `px_fit(font, size, max_w, text)`: `text`, or the longest prefix + ".." that fits.
fn px_fit(args: &[u64]) -> u64 {
    let a = |i: usize| args.get(i).map(|&v| to_f64(v)).unwrap_or(0.0);
    let text = args.get(3).map(|&v| to_string(v)).unwrap_or_default();
    let (px, maxw) = (a(1) as f32, a(2) as f32);
    let Some(f) = font(a(0)) else { return mk_str(&text) };
    if measure(f, &text, px) <= maxw {
        return mk_str(&text);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut end = chars.len();
    while end > 0 {
        let mut cand: String = chars[..end].iter().collect();
        cand.push_str("..");
        if measure(f, &cand, px) <= maxw {
            return mk_str(&cand);
        }
        end -= 1;
    }
    mk_str("")
}

// ── Window ───────────────────────────────────────────────────────────────────

fn open_window(req_w: f64, req_h: f64) {
    // LingOS: the window is the whole screen (the safe area); fall back to the
    // requested size when there's no framebuffer, so the program still runs.
    let (w, h) = if framebuffer::available() {
        (framebuffer::width() as usize, framebuffer::height() as usize)
    } else {
        (req_w.max(1.0) as usize, req_h.max(1.0) as usize)
    };
    unsafe {
        core::ptr::write(
            &raw mut WIN,
            Some(Window { buf: vec![0u32; w * h], w, h, open: true, clip: None, black: false }),
        );
    }
    // The window owns the screen now: stop the text console drawing over it,
    // and blank the overscan margin the window doesn't cover.
    #[cfg(target_arch = "aarch64")]
    {
        crate::drivers::fbcon::suspend();
        framebuffer::clear_physical(0);
    }
}

/// Run a built-in app (linked into this image) full-screen from a running
/// program — the desktop's launcher. The caller's window, globals and state are
/// put aside and restored when the app closes.
fn app_run(name: &str) -> bool {
    // `ling:<path>`: a `.ling` script (Files double-click), run by the
    // in-kernel compiler + interpreter like `ling run <path>`.
    if let Some(path) = name.strip_prefix("ling:") {
        return match crate::lingc::run_file(path, &[]) {
            Ok(()) => true,
            Err(e) => {
                crate::console_write(e.as_bytes());
                crate::console_write(b"\n");
                false
            },
        };
    }
    let Some(entry) = crate::app_named(name.as_bytes()) else { return false };
    let folder = match name {
        "lingstation" => String::from("LingStationX"),
        n => String::from(n),
    };
    with_own_window(|| {
        crate::vfs::set_root(&alloc::format!("{}/Programs/{folder}", crate::vfs::home()));
        crate::lingrt::run_isolated(entry);
    });
    true
}

/// Run `body` with a fresh window slot and cursor state, restoring the
/// caller's afterwards: a launched program (bundled app or `ling run` script)
/// opens its own window without disturbing the desktop's.
pub fn with_own_window(body: impl FnOnce()) {
    let saved = unsafe { core::ptr::replace(&raw mut WIN, None) };
    let dot = unsafe { core::mem::replace(&mut *(&raw mut CURSOR_DOT), false) };
    let root = crate::vfs::take_root();
    body();
    crate::vfs::restore_root(root);
    unsafe {
        core::ptr::write(&raw mut WIN, saved);
        CURSOR_DOT = dot;
    }
}

/// `px_text8(x, y, colour, text [, scale])`: the kernel's 8x8 pixel font (the
/// x86 desktop's UI font), transparent background, honours the clip.
fn px_text8(args: &[u64]) {
    let Some(w) = win() else { return };
    let n = |i: usize, d: f64| args.get(i).map(|&v| to_f64(v)).unwrap_or(d);
    let (x0, y0) = (n(0, 0.0) as i64, n(1, 0.0) as i64);
    let color = n(2, 0.0) as u32 & 0xFF_FFFF;
    let text = args.get(3).map(|&v| to_string(v)).unwrap_or_default();
    let sc = (n(4, 1.0) as i64).max(1);
    let (cx0, cy0, cx1, cy1) = match w.clip {
        Some(c) => (c[0] as i64, c[1] as i64, c[2] as i64, c[3] as i64),
        None => (0, 0, w.w as i64, w.h as i64),
    };
    let (cx0, cy0, cx1, cy1) = (cx0.max(0), cy0.max(0), cx1.min(w.w as i64), cy1.min(w.h as i64));
    for (k, ch) in text.bytes().enumerate() {
        let gx = x0 + k as i64 * 8 * sc;
        let rows = crate::drivers::font8x8::glyph(ch);
        for (r, bits) in rows.iter().enumerate() {
            for c in 0..8i64 {
                if (bits >> c) & 1 == 0 {
                    continue;
                }
                for sy in 0..sc {
                    for sx in 0..sc {
                        let (x, y) = (gx + c * sc + sx, y0 + r as i64 * sc + sy);
                        if x >= cx0 && x < cx1 && y >= cy0 && y < cy1 {
                            w.buf[y as usize * w.w + x as usize] = color;
                        }
                    }
                }
            }
        }
    }
}

/// The Ling Country wallpaper the x86 desktop uses (Pi image only: the x86
/// kernel carries its own copy in `drivers::wallpaper`), decoded once and
/// cover-scaled to the window, cached per size.
#[cfg(target_arch = "aarch64")]
static WALL_BMP: &[u8] = include_bytes!("../assets/ling_country_wallpaper.bmp");
static mut WALL: Option<(usize, usize, Vec<u32>)> = None;

#[cfg(target_arch = "aarch64")]
fn wall_scaled(dw: usize, dh: usize) -> Option<Vec<u32>> {
    let b = WALL_BMP;
    let rd32 = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    if b.len() < 54 || &b[0..2] != b"BM" {
        return None;
    }
    let off = rd32(10) as usize;
    let sw = rd32(18) as i32;
    let sh_raw = rd32(22) as i32;
    let bpp = u16::from_le_bytes([b[28], b[29]]) as usize;
    if sw <= 0 || sh_raw == 0 || (bpp != 24 && bpp != 32) {
        return None;
    }
    let (sw, sh, top_down) = (sw as usize, sh_raw.unsigned_abs() as usize, sh_raw < 0);
    let stride = (sw * bpp / 8 + 3) & !3;
    // Cover: scale so the image fills the window, centre-crop the overflow.
    let scale = f64::max(dw as f64 / sw as f64, dh as f64 / sh as f64);
    let ox = (sw as f64 * scale - dw as f64) / 2.0;
    let oy = (sh as f64 * scale - dh as f64) / 2.0;
    let mut out = vec![0u32; dw * dh];
    for y in 0..dh {
        let syf = ((y as f64 + oy) / scale) as usize;
        let sy = syf.min(sh - 1);
        let row = if top_down { sy } else { sh - 1 - sy };
        let base = off + row * stride;
        for x in 0..dw {
            let sx = (((x as f64 + ox) / scale) as usize).min(sw - 1);
            let p = base + sx * bpp / 8;
            if p + 2 < b.len() {
                out[y * dw + x] = ((b[p + 2] as u32) << 16) | ((b[p + 1] as u32) << 8) | b[p] as u32;
            }
        }
    }
    Some(out)
}

/// `os_wallpaper()`: paint the desktop wallpaper over the whole window;
/// false where this image has none.
fn os_wallpaper() -> bool {
    let Some(w) = win() else { return false };
    let cache = unsafe { &mut *(&raw mut WALL) };
    if !matches!(cache, Some((cw, ch, _)) if *cw == w.w && *ch == w.h) {
        #[cfg(target_arch = "aarch64")]
        {
            *cache = wall_scaled(w.w, w.h).map(|v| (w.w, w.h, v));
        }
    }
    match cache {
        Some((_, _, px)) if px.len() == w.buf.len() => {
            w.buf.copy_from_slice(px);
            true
        },
        _ => false,
    }
}

// ── Files: icons, pictures, the Pi's built-in home ──────────────────────────

/// The standard home folders, in display order (same as the x86 desktop's
/// `fs::users::HOME_FOLDERS`).
pub const HOME_FOLDERS: [&str; 8] =
    ["Documents", "Photos", "Music", "Videos", "Graphics", "3D Models", "Programs", "Scripts"];

// 8x8 emblems (bit 7 = left pixel), the same art as the x86 Files window.
const EMB_DOC: [u8; 8] = [0x00, 0x7E, 0x00, 0x7E, 0x00, 0x78, 0x00, 0x00];
const EMB_PHOTO: [u8; 8] = [0x06, 0x06, 0x00, 0x20, 0x72, 0xFF, 0xFF, 0x00];
const EMB_MUSIC: [u8; 8] = [0x1E, 0x12, 0x12, 0x12, 0x72, 0xF6, 0x66, 0x00];
const EMB_VIDEO: [u8; 8] = [0x40, 0x60, 0x70, 0x78, 0x70, 0x60, 0x40, 0x00];
const EMB_GRAPHICS: [u8; 8] = [0x38, 0x44, 0x44, 0x38, 0x10, 0x38, 0x7C, 0x00];
const EMB_MODEL: [u8; 8] = [0x3E, 0x46, 0xFA, 0x8A, 0x8C, 0x88, 0xF8, 0x00];
const EMB_PROGRAM: [u8; 8] = [0xFF, 0x81, 0xA1, 0x91, 0xA1, 0x81, 0xFF, 0x00];
const EMB_SCRIPT: [u8; 8] = [0x24, 0x42, 0x81, 0x00, 0x81, 0x42, 0x24, 0x00];

fn gfx_color() -> u32 {
    unsafe { GFX_COLOR }
}

fn pack_rgb(r: f64, g: f64, b: f64) -> u32 {
    let c = |v: f64| v.max(0.0).min(255.0) as u32;
    (c(r) << 16) | (c(g) << 8) | c(b)
}

/// HSL (h 0-360, s 0-100, l 0-100) to 0xRRGGBB, as the host's `hsl_to_hex`.
fn hsl_rgb(h: f64, s: f64, l: f64) -> u32 {
    let h = (((h % 360.0) + 360.0) % 360.0) / 360.0;
    let s = (s / 100.0).max(0.0).min(1.0);
    let l = (l / 100.0).max(0.0).min(1.0);
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f64| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    pack_rgb(f(h + 1.0 / 3.0) * 255.0, f(h) * 255.0, f(h - 1.0 / 3.0) * 255.0)
}

fn put_clipped(w: &mut Window, x: i64, y: i64, c: u32) {
    let (cx0, cy0, cx1, cy1) = clip_box(w);
    if x >= cx0 as i64 && y >= cy0 as i64 && x < cx1 as i64 && y < cy1 as i64 {
        w.buf[y as usize * w.w + x as usize] = c;
    }
}

fn hline(w: &mut Window, x0: i64, x1: i64, y: i64, c: u32) {
    let (cx0, cy0, cx1, cy1) = clip_box(w);
    if y < cy0 as i64 || y >= cy1 as i64 {
        return;
    }
    let (xa, xb) = (x0.max(cx0 as i64), x1.min(cx1 as i64 - 1));
    let row = y as usize * w.w;
    for x in xa..=xb {
        w.buf[row + x as usize] = c;
    }
}

/// Scanline even-odd fill of a closed polygon (the host's draw_poly / triangle).
fn fill_poly(w: &mut Window, pts: &[(f64, f64)], c: u32) {
    let n = pts.len();
    if n < 3 || pts.iter().any(|p| !p.0.is_finite() || !p.1.is_finite()) {
        return;
    }
    let (_, cy0, _, cy1) = clip_box(w);
    let ymin = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min).max(cy0 as f64);
    let ymax = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max).min(cy1 as f64);
    let mut xs: Vec<f64> = Vec::with_capacity(n);
    let mut y = libm::floor(ymin);
    while y < ymax {
        let sy = y + 0.5;
        xs.clear();
        for i in 0..n {
            let (x0, y0) = pts[i];
            let (x1, y1) = pts[(i + 1) % n];
            if (y0 <= sy && sy < y1) || (y1 <= sy && sy < y0) {
                xs.push(x0 + (sy - y0) / (y1 - y0) * (x1 - x0));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        let mut i = 0;
        while i + 1 < xs.len() {
            hline(w, libm::round(xs[i]) as i64, libm::round(xs[i + 1]) as i64 - 1, y as i64, c);
            i += 2;
        }
        y += 1.0;
    }
}

fn fill_circle(w: &mut Window, cx: i64, cy: i64, r: i64, c: u32) {
    for dy in -r..=r {
        let dx = libm::sqrt((r * r - dy * dy) as f64) as i64;
        hline(w, cx - dx, cx + dx, cy + dy, c);
    }
}

/// Midpoint circle outline.
fn outline_circle(w: &mut Window, cx: i64, cy: i64, r: i64, c: u32) {
    let (mut x, mut y, mut d) = (r, 0i64, 1 - r);
    while x >= y {
        for (px, py) in [(x, y), (y, x), (-y, x), (-x, y), (-x, -y), (-y, -x), (y, -x), (x, -y)] {
            put_clipped(w, cx + px, cy + py, c);
        }
        y += 1;
        if d < 0 {
            d += 2 * y + 1;
        } else {
            x -= 1;
            d += 2 * (y - x) + 1;
        }
    }
}

fn put(w: &mut Window, x: i64, y: i64, c: u32) {
    if x >= 0 && y >= 0 && (x as usize) < w.w && (y as usize) < w.h {
        w.buf[y as usize * w.w + x as usize] = c;
    }
}

fn fill_rr(w: &mut Window, x: i64, y: i64, ww: i64, hh: i64, r: i64, c: u32) {
    for yy in 0..hh {
        for xx in 0..ww {
            let cx = if xx < r { r - xx } else if xx >= ww - r { xx - (ww - r - 1) } else { 0 };
            let cy = if yy < r { r - yy } else if yy >= hh - r { yy - (hh - r - 1) } else { 0 };
            if cx * cx + cy * cy <= r * r + 1 {
                put(w, x + xx, y + yy, c);
            }
        }
    }
}

/// Kind of file by extension: (icon colour, emblem). Shared with the x86 look.
fn file_kind(name: &str, is_dir: bool) -> (u32, Option<&'static [u8; 8]>) {
    if is_dir {
        return match name {
            "Documents" => (0x4A8FE0, Some(&EMB_DOC)),
            "Photos" => (0x5AC06A, Some(&EMB_PHOTO)),
            "Music" => (0xE0A44A, Some(&EMB_MUSIC)),
            "Videos" => (0xE0605A, Some(&EMB_VIDEO)),
            "Graphics" => (0xB06AE0, Some(&EMB_GRAPHICS)),
            "3D Models" => (0x40C0C0, Some(&EMB_MODEL)),
            "Programs" => (0x8A9BB8, Some(&EMB_PROGRAM)),
            "Scripts" => (0xF0D050, Some(&EMB_SCRIPT)),
            _ => (0x4A8FE0, None),
        };
    }
    let l = name.to_ascii_lowercase();
    let ext = |e: &str| l.ends_with(e);
    if ext(".bmp") || ext(".png") {
        (0x5AC06A, Some(&EMB_PHOTO))
    } else if ext(".svg") {
        (0xB06AE0, Some(&EMB_GRAPHICS))
    } else if ext(".wav") || ext(".mp3") || ext(".ogg") || ext(".flac") || ext(".mid") {
        (0xE0A44A, Some(&EMB_MUSIC))
    } else if ext(".mp4") || ext(".mkv") || ext(".webm") || ext(".avi") || ext(".mov") || ext(".gif") {
        (0xE0605A, Some(&EMB_VIDEO))
    } else if ext(".ling") {
        (0xF0D050, Some(&EMB_SCRIPT))
    } else if ext(".obj") || ext(".stl") || ext(".glb") || ext(".gltf") || ext(".ply") {
        (0x40C0C0, Some(&EMB_MODEL))
    } else if ext(".txt") || ext(".md") || ext(".json") || ext(".toml") || ext(".csv") || !l.contains('.') {
        (0xC8C0D8, Some(&EMB_DOC))
    } else {
        (0x9B8FB0, None)
    }
}

/// `os_file_icon(x, y, name, is_dir [, scale])`: the Files icon for an entry —
/// a folder coloured by kind with an emblem for the home folders, or a page
/// coloured and marked by file type. 18x14 at scale 1.
fn os_file_icon(args: &[u64]) {
    let Some(w) = win() else { return };
    let n = |i: usize, d: f64| args.get(i).map(|&v| to_f64(v)).unwrap_or(d);
    let (x, y) = (n(0, 0.0) as i64, n(1, 0.0) as i64);
    let name = args.get(2).map(|&v| to_string(v)).unwrap_or_default();
    let is_dir = args.get(3).map(|&v| crate::lingrt::truthy(v)).unwrap_or(false);
    let sc = (n(4, 1.0) as i64).max(1);
    let (color, emb) = file_kind(&name, is_dir);
    let ink = 0x1A1423;
    if is_dir {
        fill_rr(w, x, y, 8 * sc, 4 * sc, sc, color);
        fill_rr(w, x, y + 2 * sc, 18 * sc, 12 * sc, 2 * sc, color);
    } else {
        fill_rr(w, x + 2 * sc, y, 14 * sc, 14 * sc, 2 * sc, color);
    }
    if let Some(e) = emb {
        let (ex, ey) = if is_dir { (x + 5 * sc, y + 4 * sc) } else { (x + 5 * sc, y + 3 * sc) };
        for (r, bits) in e.iter().enumerate() {
            for c in 0..8i64 {
                if bits & (0x80 >> c) != 0 {
                    for dy in 0..sc {
                        for dx in 0..sc {
                            put(w, ex + c * sc + dx, ey + r as i64 * sc + dy, ink);
                        }
                    }
                }
            }
        }
    }
}

/// Decoded pictures by path (PNG/SVG via drivers::image, BMP here).
static mut PICS: BTreeMap<String, Option<(usize, usize, Vec<u32>)>> = BTreeMap::new();

fn decode_bmp(b: &[u8]) -> Option<(usize, usize, Vec<u32>)> {
    let rd32 = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    if b.len() < 54 || &b[0..2] != b"BM" {
        return None;
    }
    let off = rd32(10) as usize;
    let sw = rd32(18) as i32;
    let sh_raw = rd32(22) as i32;
    let bpp = u16::from_le_bytes([b[28], b[29]]) as usize;
    if sw <= 0 || sh_raw == 0 || (bpp != 24 && bpp != 32) {
        return None;
    }
    let (sw, sh) = (sw as usize, sh_raw.unsigned_abs() as usize);
    let stride = (sw * bpp / 8 + 3) & !3;
    let mut px = vec![0u32; sw * sh];
    for y in 0..sh {
        let row = if sh_raw < 0 { y } else { sh - 1 - y };
        for x in 0..sw {
            let p = off + row * stride + x * bpp / 8;
            if p + 2 < b.len() {
                px[y * sw + x] = ((b[p + 2] as u32) << 16) | ((b[p + 1] as u32) << 8) | b[p] as u32;
            }
        }
    }
    Some((sw, sh, px))
}

fn picture(path: &str) -> Option<&'static (usize, usize, Vec<u32>)> {
    let cache = unsafe { &mut *(&raw mut PICS) };
    if !cache.contains_key(path) {
        let decoded = crate::vfs::read(path).and_then(|data| {
            let data: &[u8] = &data;
            let l = path.to_ascii_lowercase();
            if l.ends_with(".bmp") {
                decode_bmp(data)
            } else if l.ends_with(".svg") || crate::drivers::image::looks_svg(data) {
                crate::drivers::image::decode_svg_capped(data, 1024).map(|i| (i.w as usize, i.h as usize, i.px))
            } else {
                crate::drivers::image::decode_png(data).map(|i| (i.w as usize, i.h as usize, i.px))
            }
        });
        cache.insert(String::from(path), decoded);
    }
    cache.get(path).and_then(|v| v.as_ref())
}

/// `px_image(path, x, y, w, h)`: draw a BMP/PNG/SVG file fitted (aspect kept,
/// centred) inside the rectangle; false if it can't be read or decoded.
fn px_image(args: &[u64]) -> bool {
    let path = path_arg(args, 0);
    let Some((iw, ih, px)) = picture(&path) else { return false };
    let Some(w) = win() else { return false };
    let n = |i: usize| args.get(i).map(|&v| to_f64(v)).unwrap_or(0.0);
    let (rx, ry, rw, rh) = (n(1), n(2), n(3).max(1.0), n(4).max(1.0));
    let scale = f64::min(rw / *iw as f64, rh / *ih as f64);
    let (dw, dh) = ((*iw as f64 * scale) as i64, (*ih as f64 * scale) as i64);
    let (ox, oy) = ((rx + (rw - dw as f64) / 2.0) as i64, (ry + (rh - dh as f64) / 2.0) as i64);
    for y in 0..dh.max(0) {
        let sy = ((y as f64 / scale) as usize).min(ih - 1);
        for x in 0..dw.max(0) {
            let sx = ((x as f64 / scale) as usize).min(iw - 1);
            put(w, ox + x, oy + y, px[sy * iw + sx]);
        }
    }
    true
}

/// The Pi has no storage driver yet, so its home is built in: the standard
/// folders (an empty `.keep` makes each one listable) and starter files, under
/// `home/<user>/`, served read-only from the image like the app's own files.
#[cfg(target_arch = "aarch64")]
pub fn register_builtin_home(user: &str) {
    use alloc::boxed::Box;
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    for f in HOME_FOLDERS {
        crate::appfs::register(leak(alloc::format!("home/{user}/{f}/.keep")), b"");
    }
    for (rel, src) in crate::lingc::EXAMPLES {
        crate::appfs::register(leak(alloc::format!("home/{user}/{rel}")), src.as_bytes());
    }
    let starters: [(&str, &'static [u8]); 3] = [
        ("Scripts/hello.ling", b"// A Ling script -- open it from Files, or `ling run` it from the console.\nbind main = do {\n    println(\"Hello from LingOS!\")\n    println(\"2 + 2 = \", 2 + 2)\n}\n"),
        ("Graphics/ling-badge.svg", b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 200 200\"><rect width=\"200\" height=\"200\" fill=\"#241B33\"/><circle cx=\"100\" cy=\"100\" r=\"70\" fill=\"#4A8FE0\"/><polygon points=\"100,48 146,140 54,140\" fill=\"#FFFFFF\"/></svg>\n"),
        ("Documents/Welcome.txt", b"Welcome to LingOS!\n\nYour home folders:\n  Documents  - text and notes\n  Photos     - BMP / PNG / SVG pictures\n  Music      - WAV audio\n  Videos     - video files\n  Graphics   - SVG drawings and .ling graphics programs\n  3D Models  - 3D model files\n  Programs   - lingfu programs and downloaded source\n  Scripts    - .ling scripts\n\nOn the Raspberry Pi this home is built into the image (read-only)\nuntil LingOS can read and write the SD card.\n"),
    ];
    for (rel, data) in starters {
        crate::appfs::register(leak(alloc::format!("home/{user}/{rel}")), data);
    }
}

/// The signed-in user's name (x86: the lingfs session; Pi: "ling" until the
/// setup screen's accounts persist).
fn os_user() -> String {
    #[cfg(target_arch = "x86_64")]
    return String::from(crate::fs::lingfs::current_user());
    #[allow(unreachable_code)]
    String::from("ling")
}

/// `os_info(key)`: facts about the running system for the desktop's About/tray.
fn os_info(key: &str) -> String {
    use alloc::format;
    match key {
        "board" => {
            #[cfg(target_arch = "aarch64")]
            return String::from(match crate::arch::mmio::board() {
                crate::arch::mmio::RpiBoard::Pi3 => "Raspberry Pi 3",
                crate::arch::mmio::RpiBoard::Pi4 => "Raspberry Pi 4",
                crate::arch::mmio::RpiBoard::Pi5 => "Raspberry Pi 5",
                crate::arch::mmio::RpiBoard::Unknown => "Raspberry Pi",
            });
            #[allow(unreachable_code)]
            String::from("PC (x86-64)")
        },
        "memory" => format!("{} MiB free", crate::mm::frame::free_frame_count() * 4 / 1024),
        // The configured IPv4 address ("" before DHCP / without a NIC).
        "ip" => {
            let ip = crate::drivers::netstack::self_ip();
            if ip == [0, 0, 0, 0] {
                String::new()
            } else {
                format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
            }
        },
        "display" => format!("{} x {}", framebuffer::width(), framebuffer::height()),
        "mouse" => {
            #[cfg(target_arch = "aarch64")]
            return String::from(if crate::arch::xhci::mouse_ready() { "USB mouse" } else { "none" });
            #[allow(unreachable_code)]
            String::from("PS/2 mouse")
        },
        "keyboard" => {
            #[cfg(target_arch = "aarch64")]
            return String::from(if crate::arch::xhci::keyboard_ready() { "USB keyboard" } else { "serial only" });
            #[allow(unreachable_code)]
            String::from("PS/2 keyboard")
        },
        "network" => String::from("not available yet"),
        "user" => os_user(),
        _ => String::new(),
    }
}

/// Kept for apps that call `os_cursor`: the pointer is now always the LingOS
/// cursor (`drivers::cursor`), so this is a no-op.
static mut CURSOR_DOT: bool = false;

fn present() {
    let (mx, my, _) = mouse_state();
    if let Some(w) = win() {
        // Draw the pointer into the frame, blit, then put the pixels back so the
        // app's own buffer is untouched.
        let mut saved: Vec<(usize, u32)> = Vec::new();
        if mx >= 0.0 && my >= 0.0 {
            use crate::drivers::cursor;
            let fill = cursor::fill_color(crate::arch::timer::now_ms());
            let (cx, cy) = (mx as i64, my as i64);
            for dy in 0..cursor::H as i64 {
                for dx in 0..cursor::W as i64 {
                    let k = cursor::classify(dx as i32, dy as i32);
                    let (x, y) = (cx + dx, cy + dy);
                    if k == 0 || x < 0 || y < 0 || x as usize >= w.w || y as usize >= w.h {
                        continue;
                    }
                    let i = y as usize * w.w + x as usize;
                    saved.push((i, w.buf[i]));
                    w.buf[i] = if k == 1 { cursor::OUTLINE } else { fill };
                }
            }
        }
        framebuffer::blit_rgb(&w.buf, w.w, w.h);
        for (i, v) in saved {
            w.buf[i] = v;
        }
    }
    // Input for the next frame. Ctrl+Q closes a launched app (never the
    // desktop itself) back to whatever started it.
    let nested = crate::lingrt::app_depth() > 0;
    let input = unsafe { &mut *(&raw mut INPUT) };
    input.pressed.clear();
    #[cfg(target_arch = "aarch64")]
    {
        // Keep the HDMI audio ring fed (the app loop is the only loop running).
        crate::arch::hdmi_audio::pump();
        // Serial + USB keyboard bytes, decoded the way a terminal sends them.
        while let Some(c) = crate::poll_input() {
            input.feed(c);
        }
        input.prev_held = input.held;
        input.held = crate::arch::xhci::held_report();
        input.wheel = crate::arch::xhci::mouse_take_wheel();
    }
    // x86: PS/2 keyboard — Ctrl+Q is the chord byte 0x80|17. Also keep the
    // AC97 ring fed (the desktop loop that normally pumps it isn't running
    // while an app owns the screen), so LingStationX playback is heard.
    #[cfg(target_arch = "x86_64")]
    {
        crate::drivers::mixer::pump();
        crate::arch::idt::poll_drain_8042();
        loop {
            match crate::drivers::keyboard::poll_char() {
                0 => break,
                0x91 => input.feed(0x11),
                // Ctrl+letter chords (keyboard.rs): the letter pressed with
                // Ctrl held this frame, as apps test with key_down("ctrl").
                c @ 0x81..=0x9A => {
                    if let Some(name) = key_name_of_byte(b'a' + (c - 0x81)) {
                        input.pressed.push("ctrl");
                        input.pressed.push("lctrl");
                        input.pressed.push(name);
                    }
                },
                c => input.feed(c),
            }
        }
    }
    if input.quit && nested {
        if let Some(w) = win() {
            w.open = false;
        }
    }
    input.quit = false;
}

/// Keyboard state between frames: text typed since the last `text_poll`, key
/// names pressed during the last frame (from terminal-style bytes), and the
/// USB keyboard's held keys now and one frame ago (for `key_down` and edge
/// `key_pressed` on keys that send no bytes, like Super or F-keys).
struct Input {
    text: String,
    pressed: Vec<&'static str>,
    esc: u8, // 0 idle, 1 after ESC, 2 after ESC [
    quit: bool,
    held: [u8; 8],
    prev_held: [u8; 8],
    wheel: i32,
}

static mut INPUT: Input = Input {
    text: String::new(),
    pressed: Vec::new(),
    esc: 0,
    quit: false,
    held: [0; 8],
    prev_held: [0; 8],
    wheel: 0,
};

impl Input {
    fn feed(&mut self, c: u8) {
        match (self.esc, c) {
            (1, b'[') => {
                self.esc = 2;
                return;
            },
            (2, b'A') => self.pressed.push("up"),
            (2, b'B') => self.pressed.push("down"),
            (2, b'C') => self.pressed.push("right"),
            (2, b'D') => self.pressed.push("left"),
            (1, _) => {
                // A lone ESC followed by something else: Escape, then the byte.
                self.pressed.push("escape");
                self.esc = 0;
                return self.feed(c);
            },
            (2, _) => {},
            (_, 0x1B) => {
                self.esc = 1;
                return;
            },
            (_, 0x11) => self.quit = true,
            (_, b'\r' | b'\n') => {
                self.pressed.push("enter");
                self.text.push('\n');
            },
            (_, 0x08 | 0x7F) => {
                self.pressed.push("backspace");
                self.text.push('\u{8}');
            },
            (_, b'\t') => self.pressed.push("tab"),
            (_, 0x20..=0x7E) => {
                self.text.push(c as char);
                if let Some(n) = key_name_of_byte(c) {
                    self.pressed.push(n);
                }
            },
            _ => {},
        }
        self.esc = 0;
    }

    /// A trailing ESC with nothing after it by frame end is the Escape key.
    fn flush_esc(&mut self) {
        if self.esc == 1 {
            self.pressed.push("escape");
        }
        self.esc = 0;
    }
}

fn key_name_of_byte(c: u8) -> Option<&'static str> {
    const LETTERS: [&str; 26] = [
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u",
        "v", "w", "x", "y", "z",
    ];
    const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
    match c {
        b'a'..=b'z' => Some(LETTERS[(c - b'a') as usize]),
        b'A'..=b'Z' => Some(LETTERS[(c - b'A') as usize]),
        b'0'..=b'9' => Some(DIGITS[(c - b'0') as usize]),
        b' ' => Some("space"),
        _ => None,
    }
}

/// HID usages for a key name (left and right variants for modifiers).
fn key_usages(name: &str) -> &'static [u8] {
    const L: [u8; 26] = [
        4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29,
    ];
    const D: [u8; 10] = [0x27, 0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26];
    const F: [u8; 12] = [0x3A, 0x3B, 0x3C, 0x3D, 0x3E, 0x3F, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45];
    let b = name.as_bytes();
    if b.len() == 1 && b[0].is_ascii_lowercase() {
        let i = (b[0] - b'a') as usize;
        return &L[i..i + 1];
    }
    if b.len() == 1 && b[0].is_ascii_digit() {
        let i = (b[0] - b'0') as usize;
        return &D[i..i + 1];
    }
    if b.len() >= 2 && b[0] == b'f' {
        if let Ok(n) = name[1..].parse::<usize>() {
            if (1..=12).contains(&n) {
                return &F[n - 1..n];
            }
        }
    }
    match name {
        "enter" | "return" => &[0x28, 0x58],
        "escape" | "esc" => &[0x29],
        "backspace" => &[0x2A],
        "tab" => &[0x2B],
        "space" => &[0x2C],
        "right" => &[0x4F],
        "left" => &[0x50],
        "down" => &[0x51],
        "up" => &[0x52],
        "delete" => &[0x4C],
        "home" => &[0x4A],
        "end" => &[0x4D],
        "pageup" => &[0x4B],
        "pagedown" => &[0x4E],
        "ctrl" | "control" | "leftctrl" | "lctrl" => &[0xE0, 0xE4],
        "rctrl" | "rightctrl" => &[0xE4],
        "shift" | "leftshift" | "lshift" => &[0xE1, 0xE5],
        "rshift" | "rightshift" => &[0xE5],
        "alt" | "leftalt" | "lalt" => &[0xE2, 0xE6],
        "ralt" | "rightalt" => &[0xE6],
        "super" | "win" | "meta" | "cmd" => &[0xE3, 0xE7],
        _ => &[],
    }
}

fn report_has(r: &[u8; 8], u: u8) -> bool {
    if (0xE0..=0xE7).contains(&u) {
        r[0] & (1 << (u - 0xE0)) != 0
    } else {
        r[2..8].contains(&u)
    }
}

fn key_down(name: &str) -> bool {
    let i = unsafe { &*(&raw const INPUT) };
    let n = name.to_ascii_lowercase();
    key_usages(&n).iter().any(|&u| report_has(&i.held, u)) || i.pressed.iter().any(|p| *p == n)
}

fn key_pressed(name: &str) -> bool {
    let i = unsafe { &mut *(&raw mut INPUT) };
    i.flush_esc();
    let n = name.to_ascii_lowercase();
    i.pressed.iter().any(|p| *p == n)
        || key_usages(&n).iter().any(|&u| report_has(&i.held, u) && !report_has(&i.prev_held, u))
}

/// Pointer state in window coordinates: (x, y, buttons bit0 L / bit1 R / bit2 M).
/// x86 reads the PS/2 mouse, the Pi its USB mouse; with no mouse it's off-screen.
fn mouse_state() -> (f64, f64, u8) {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::idt::poll_drain_8042();
        let (ox, oy) = framebuffer::safe_margin();
        use crate::drivers::mouse;
        return ((mouse::x() - ox as i32) as f64, (mouse::y() - oy as i32) as f64, mouse::buttons());
    }
    #[cfg(target_arch = "aarch64")]
    if let Some((x, y, b)) = crate::arch::xhci::mouse_state() {
        return (x as f64, y as f64, b);
    }
    #[allow(unreachable_code)]
    (-10_000.0, -10_000.0, 0)
}

// ── Files (embedded, read-only until LingOS reads the SD card) ───────────────

fn path_arg(args: &[u64], i: usize) -> String {
    args.get(i).map(|&v| to_string(v)).unwrap_or_default().replace('\\', "/")
}

/// The host's `Path::join` (a relative `b` appended with one separator).
fn fs_join(a: &str, b: &str) -> String {
    if a.is_empty() {
        return String::from(b);
    }
    let mut s = String::from(a.trim_end_matches('/'));
    s.push('/');
    s.push_str(b.trim_start_matches('/'));
    s
}

/// The host's `Path::parent` ("" for a bare name).
fn fs_parent(p: &str) -> String {
    match p.trim_end_matches('/').rfind('/') {
        Some(i) => String::from(&p[..i]),
        None => String::new(),
    }
}

/// The host's `fs_list`: a flat list of `name, is_dir (1/0), size` triples,
/// directories first, then by lower-cased name, dotfiles hidden.
fn fs_list(dir: &str) -> u64 {
    let items = crate::vfs::list(dir);
    let mut v = Vec::with_capacity(items.len() * 3);
    for (n, d, s) in items {
        v.push(mk_str(&n));
        v.push(num(d as i32 as f64));
        v.push(num(s as f64));
    }
    list_from(v)
}

// ── File dialogs ─────────────────────────────────────────────────────────────
// `dlg_open(title, exts, start_dir)`, `dlg_save(title, ext, default_name)` and
// `dlg_folder(title, start_dir)`: a modal picker drawn over the program's own
// window and driven by the same keyboard and mouse. Up/Down/PageUp/PageDown
// (or the wheel) move, Enter opens a folder or accepts, Backspace goes up a
// folder (in Save it edits the name first), typing jumps to a name (Open) or
// edits the name (Save), Space picks the current folder (Folder), a click
// selects and a double-click opens, Esc cancels. Returns the path or "".

#[derive(PartialEq, Clone, Copy)]
enum Dlg {
    Open,
    Save,
    Folder,
}

fn exts_arg(args: &[u64], i: usize) -> Vec<String> {
    path_arg(args, i)
        .split(',')
        .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect()
}

fn ext_matches(name: &str, exts: &[String]) -> bool {
    if exts.is_empty() {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    exts.iter().any(|e| lower.ends_with(&alloc::format!(".{e}")))
}

fn dlg_entries(dir: &str, kind: Dlg, exts: &[String]) -> Vec<(String, bool)> {
    crate::vfs::list(dir)
        .into_iter()
        .filter(|(n, d, _)| *d || (kind != Dlg::Folder && ext_matches(n, exts)))
        .map(|(n, d, _)| (n, d))
        .collect()
}

/// 8x8 text, unclipped (the dialog ignores the program's clip rectangle).
fn text8(w: &mut Window, x: i64, y: i64, color: u32, s: &str) {
    for (k, ch) in s.bytes().enumerate() {
        let gx = x + k as i64 * 8;
        let rows = crate::drivers::font8x8::glyph(ch);
        for (r, bits) in rows.iter().enumerate() {
            for c in 0..8i64 {
                if (bits >> c) & 1 != 0 {
                    put(w, gx + c, y + r as i64, color);
                }
            }
        }
    }
}

fn rect(w: &mut Window, x: i64, y: i64, ww: i64, hh: i64, c: u32) {
    for yy in y..y + hh {
        for xx in x..x + ww {
            put(w, xx, yy, c);
        }
    }
}

/// `s` cut to `max` characters, keeping the end (paths) when `tail`.
fn fit(s: &str, max: usize, tail: bool) -> String {
    let cs: Vec<char> = s.chars().collect();
    if cs.len() <= max || max < 4 {
        return String::from(s);
    }
    if tail {
        alloc::format!("...{}", cs[cs.len() - (max - 3)..].iter().collect::<String>())
    } else {
        alloc::format!("{}...", cs[..max - 3].iter().collect::<String>())
    }
}

fn file_dialog(kind: Dlg, title: &str, exts: &[String], start: &str, default_name: &str) -> String {
    let Some(w) = win() else {
        crate::console_write(b"[dialog] no window open\n");
        return String::new();
    };
    if !w.open {
        return String::new();
    }
    let saved_buf = w.buf.clone();
    let saved_clip = w.clip.take();
    let (ww, wh) = (w.w as i64, w.h as i64);
    let pw = (ww - 40).clamp(240, 720);
    let ph = (wh - 40).clamp(200, 520);
    let (px, py) = ((ww - pw) / 2, (wh - ph) / 2);
    let cols = ((pw - 28) / 8).max(10) as usize;
    let row_h = 18i64;
    let list_y = py + 60;
    let list_h = ph - 60 - 54;
    let rows = (list_h / row_h).max(1) as usize;

    let mut dir = if !start.is_empty() && crate::vfs::is_dir(start) { crate::vfs::resolve(start) } else { crate::vfs::home() };
    let mut entries = dlg_entries(&dir, kind, exts);
    let (mut sel, mut scroll) = (0usize, 0usize);
    let mut name = String::from(default_name);
    let mut msg = String::new();
    let mut result = String::new();
    let mut prev_buttons = mouse_state().2;
    let mut last_click: (usize, u64) = (usize::MAX, 0);
    let one_ext = if exts.len() == 1 { Some(exts[0].clone()) } else { None };

    loop {
        let Some(w) = win() else { break };
        w.buf.copy_from_slice(&saved_buf);
        rect(w, px - 2, py - 2, pw + 4, ph + 4, 0x6C_3AFF);
        rect(w, px, py, pw, ph, 0x1B_1626);
        rect(w, px, py, pw, 26, 0x2A_2140);
        text8(w, px + 10, py + 9, 0xFF_FFFF, &fit(title, cols, false));
        text8(w, px + 10, py + 38, 0xB8_B0D0, &fit(&alloc::format!("/{dir}"), cols, true));
        if sel >= entries.len() {
            sel = entries.len().saturating_sub(1);
        }
        if sel < scroll {
            scroll = sel;
        } else if sel >= scroll + rows {
            scroll = sel + 1 - rows;
        }
        if entries.is_empty() {
            text8(w, px + 14, list_y + 4, 0x8A_82A0, "(empty folder)");
        }
        for (i, (n, is_dir)) in entries.iter().enumerate().skip(scroll).take(rows) {
            let y = list_y + (i - scroll) as i64 * row_h;
            if i == sel {
                rect(w, px + 6, y - 2, pw - 12, row_h, 0x3A_2F5C);
            }
            let label = if *is_dir { alloc::format!("{n}/") } else { n.clone() };
            text8(w, px + 14, y + 3, if *is_dir { 0x8F_D0FF } else { 0xE8_E4F0 }, &fit(&label, cols - 1, false));
        }
        let by = py + ph - 48;
        rect(w, px + 6, by, pw - 12, 1, 0x3A_2F5C);
        let help = match kind {
            Dlg::Open => "Enter: open   Backspace: up   Esc: cancel",
            Dlg::Save => "Enter: save   Backspace: edit / up   Esc: cancel",
            Dlg::Folder => "Enter: into   Space: choose this folder   Esc: cancel",
        };
        if kind == Dlg::Save {
            text8(w, px + 10, by + 10, 0xB8_B0D0, "Name:");
            rect(w, px + 58, by + 5, pw - 70, 18, 0x0F_0C18);
            text8(w, px + 62, by + 10, 0xFF_FFFF, &fit(&alloc::format!("{name}_"), cols - 7, true));
            text8(w, px + 10, by + 30, 0x8A_82A0, help);
        } else {
            text8(w, px + 10, by + 10, 0x8A_82A0, help);
            text8(w, px + 10, by + 30, 0xFF_8080, &fit(&msg, cols, false));
        }
        present();

        let input = unsafe { &mut *(&raw mut INPUT) };
        if input.quit {
            break;
        }
        let typed = core::mem::take(&mut input.text);
        let wheel = core::mem::take(&mut input.wheel);
        if key_pressed("escape") {
            break;
        }
        let before = sel;
        let last = entries.len().saturating_sub(1);
        if key_pressed("down") {
            sel = (sel + 1).min(last);
        }
        if key_pressed("up") {
            sel = sel.saturating_sub(1);
        }
        if key_pressed("pagedown") {
            sel = (sel + rows).min(last);
        }
        if key_pressed("pageup") {
            sel = sel.saturating_sub(rows);
        }
        if wheel < 0 {
            sel = (sel + 3).min(last);
        } else if wheel > 0 {
            sel = sel.saturating_sub(3);
        }
        let mut activate = key_pressed("enter");
        let (mx, my, buttons) = mouse_state();
        if buttons & 1 != 0 && prev_buttons & 1 == 0 {
            let (mx, my) = (mx as i64, my as i64);
            if mx >= px && mx < px + pw && my >= list_y && my < list_y + rows as i64 * row_h {
                let i = scroll + ((my - list_y) / row_h) as usize;
                if i < entries.len() {
                    let now = crate::arch::timer::now_ms();
                    if last_click.0 == i && now.saturating_sub(last_click.1) < 450 {
                        activate = true;
                    }
                    sel = i;
                    last_click = (i, now);
                }
            }
        }
        prev_buttons = buttons;
        if !typed.is_empty() {
            match kind {
                Dlg::Save => {
                    for ch in typed.chars() {
                        if ch == '\n' || ch == '\r' {
                            activate = true;
                        } else if !ch.is_control() && ch != '/' && ch != '\\' {
                            name.push(ch);
                        }
                    }
                },
                _ => {
                    let t = typed.to_ascii_lowercase();
                    if let Some(i) = entries.iter().position(|(n, _)| n.to_ascii_lowercase().starts_with(&t)) {
                        sel = i;
                    }
                },
            }
        }
        if kind == Dlg::Save && sel != before {
            if let Some((n, false)) = entries.get(sel) {
                name = n.clone();
            }
        }
        if key_pressed("backspace") {
            if kind == Dlg::Save && !name.is_empty() {
                name.pop();
            } else {
                dir = crate::vfs::parent(&dir);
                entries = dlg_entries(&dir, kind, exts);
                sel = 0;
                scroll = 0;
            }
        }
        if kind == Dlg::Folder && key_pressed("space") {
            result = dir.clone();
            break;
        }
        if activate {
            match entries.get(sel).cloned() {
                Some((n, true)) => {
                    dir = crate::vfs::join(&dir, &n);
                    entries = dlg_entries(&dir, kind, exts);
                    sel = 0;
                    scroll = 0;
                },
                Some((n, false)) if kind == Dlg::Open => {
                    result = crate::vfs::join(&dir, &n);
                    break;
                },
                _ if kind == Dlg::Save => {
                    let nm = name.trim();
                    if nm.is_empty() {
                        msg = String::from("Type a name first");
                    } else {
                        let full = match &one_ext {
                            Some(e) if !nm.to_ascii_lowercase().ends_with(&alloc::format!(".{e}")) => alloc::format!("{nm}.{e}"),
                            _ => String::from(nm),
                        };
                        result = crate::vfs::join(&dir, &full);
                        break;
                    }
                },
                _ => {},
            }
        }
    }
    if let Some(w) = win() {
        w.buf.copy_from_slice(&saved_buf);
        w.clip = saved_clip;
    }
    unsafe {
        let input = &mut *(&raw mut INPUT);
        input.pressed.clear();
        input.text.clear();
    }
    result
}

// ── Dispatch ─────────────────────────────────────────────────────────────────

/// The app builtins this module implements; `None` for any other name.
pub fn dispatch(name: &[u8], args: &[u64]) -> Option<u64> {
    let a = |i: usize, d: f64| args.get(i).map(|&v| to_f64(v)).unwrap_or(d);
    let arg: Arg = &a;
    // The host gfx API's polyglot names (ling/src/runtime/mod.rs) -> canonical.
    let name: &[u8] = match core::str::from_utf8(name).unwrap_or("") {
        "สีดินสอ" | "设色" | "色設定" | "색설정" => b"set_color",
        "颜色HSL" | "色相" | "HSL色" | "HSL색설정" | "สีHSLวาด" => b"set_color_hsl",
        "เติม" | "填" | "塗り潰し" | "채우기" | "清" => b"fill",
        "วาดสามเหลี่ยม" => b"triangle",
        "วาดเส้น" => b"line",
        "วาดจุด" => b"pixel",
        "แสดงผล" | "แสดง" | "显" | "呈现" | "表示" | "표시" => b"present",
        "หยุด" | "นอน" | "睡眠" | "眠る" | "スリープ" | "잠자기" => b"sleep",
        _ => name,
    };
    Some(match name {
        // Window.
        b"open_window" => {
            open_window(a(0, 800.0), a(1, 600.0));
            UNIT
        },
        b"window_is_open" => boolean(win().is_some_and(|w| w.open)),
        b"close_window" | b"quit" => {
            if let Some(w) = win() {
                w.open = false;
            }
            UNIT
        },
        b"present" | b"show" | b"gfx_present" => {
            present();
            unsafe { FRAMES += 1 };
            UNIT
        },
        b"get_width" => num(win().map(|w| w.w).unwrap_or(0) as f64),
        b"get_height" => num(win().map(|w| w.h).unwrap_or(0) as f64),
        b"screenshot" => UNIT,

        // The host's immediate-mode gfx API (the gfx arms of
        // ling/src/runtime/mod.rs): a current colour, whole-window fills,
        // lines, triangles, circles and polygons -- so the programs in
        // ling/examples run here unchanged.
        b"set_color" | b"gfx_color" | b"color" => {
            unsafe { GFX_COLOR = pack_rgb(a(0, 255.0), a(1, 255.0), a(2, 255.0)) };
            UNIT
        },
        b"set_color_hsl" => {
            unsafe { GFX_COLOR = hsl_rgb(a(0, 0.0), a(1, 70.0), a(2, 50.0)) };
            UNIT
        },
        b"fill" | b"clear" | b"gfx_fill" => {
            if let Some(w) = win() {
                let c = pack_rgb(a(0, 0.0), a(1, 0.0), a(2, 0.0));
                w.buf.fill(c);
            }
            UNIT
        },
        b"triangle" | b"draw_triangle" => {
            if let Some(w) = win() {
                let pts = [(a(0, 0.0), a(1, 0.0)), (a(2, 0.0), a(3, 0.0)), (a(4, 0.0), a(5, 0.0))];
                fill_poly(w, &pts, gfx_color());
            }
            UNIT
        },
        b"line" | b"draw_line" => {
            // px_line takes its colour (and alpha) as arguments 4 and 5.
            let c = gfx_color() as f64;
            let l = |i: usize, d: f64| match i {
                4 => c,
                5 => 1.0,
                _ => a(i, d),
            };
            px_line(&l);
            UNIT
        },
        b"pixel" | b"draw_pixel" => {
            if let Some(w) = win() {
                put_clipped(w, a(0, 0.0) as i64, a(1, 0.0) as i64, gfx_color());
            }
            UNIT
        },
        b"draw_circle" => {
            if let Some(w) = win() {
                outline_circle(w, a(0, 0.0) as i64, a(1, 0.0) as i64, a(2, 10.0) as i64, gfx_color());
            }
            UNIT
        },
        b"draw_filled_circle" | b"draw_disc" => {
            if let Some(w) = win() {
                fill_circle(w, a(0, 0.0) as i64, a(1, 0.0) as i64, a(2, 10.0) as i64, gfx_color());
            }
            UNIT
        },
        b"draw_poly" => {
            if let (Some(w), Some(&l)) = (win(), args.first()) {
                let items = crate::lingrt::list_items(l);
                let pts: Vec<(f64, f64)> = items.chunks_exact(2).map(|p| (to_f64(p[0]), to_f64(p[1]))).collect();
                fill_poly(w, &pts, gfx_color());
            }
            UNIT
        },
        b"frame_count" => num(unsafe { FRAMES } as f64),
        b"sleep" | b"sleep_ms" => {
            let end = crate::arch::timer::now_ms() + a(0, 0.0).max(0.0) as u64;
            while crate::arch::timer::now_ms() < end {
                #[cfg(target_arch = "x86_64")]
                crate::proc::sched::yield_now();
                #[cfg(target_arch = "aarch64")]
                crate::arch::timer::delay_us(500);
            }
            UNIT
        },

        // Pixel toolkit.
        b"px_theme" => {
            if let Some(w) = win() {
                w.black = a(0, 0.0) > 0.5;
            }
            UNIT
        },
        b"px_flat" | b"px_color" | b"ui_theme" => UNIT,
        b"px_clip" => {
            if let Some(w) = win() {
                let (x, y, ww, hh) = (a(0, 0.0), a(1, 0.0), a(2, 0.0), a(3, 0.0));
                w.clip = Some([
                    libm::floor(x) as i32,
                    libm::floor(y) as i32,
                    libm::ceil(x + ww) as i32,
                    libm::ceil(y + hh) as i32,
                ]);
            }
            UNIT
        },
        b"px_unclip" => {
            if let Some(w) = win() {
                w.clip = None;
            }
            UNIT
        },
        b"px_rect" | b"px_outline" | b"px_hline" | b"px_vline" => {
            px_rect_like(name, arg);
            UNIT
        },
        b"px_rrect" => {
            px_rrect(false, arg);
            UNIT
        },
        b"px_rrect_outline" => {
            px_rrect(true, arg);
            UNIT
        },
        b"px_disc" => {
            px_disc(false, arg);
            UNIT
        },
        b"px_ring" => {
            px_disc(true, arg);
            UNIT
        },
        b"px_line" => {
            px_line(arg);
            UNIT
        },
        b"px_text" => {
            px_text(args);
            UNIT
        },
        b"px_fit" => px_fit(args),

        // Fonts.
        b"font_load" => num(font_load(&path_arg(args, 0))),
        b"font_width" => {
            let s = args.get(2).map(|&v| to_string(v)).unwrap_or_default();
            num(font(a(0, -1.0)).map(|f| measure(f, &s, a(1, 16.0) as f32) as f64).unwrap_or(0.0))
        },

        // Input: PS/2 (x86) or USB HID (Pi) mouse + keyboard, serial on the Pi.
        b"mouse_x" => num(mouse_state().0),
        b"mouse_y" => num(mouse_state().1),
        b"mouse_down" => boolean(mouse_state().2 & 1 != 0),
        b"mouse_down_right" => boolean(mouse_state().2 & 2 != 0),
        b"mouse_down_middle" => boolean(mouse_state().2 & 4 != 0),
        b"mouse_scroll" => num(unsafe { (*(&raw const INPUT)).wheel } as f64),
        b"mouse_scroll_x" => num(0.0),
        b"key_down" => boolean(key_down(&path_arg(args, 0))),
        b"key_pressed" => boolean(key_pressed(&path_arg(args, 0))),
        b"text_poll" | b"text_get" => mk_str(&core::mem::take(unsafe { &mut (*(&raw mut INPUT)).text })),
        b"text_set" => UNIT,

        // LingOS: the desktop's view of the system.
        b"px_text8" => {
            px_text8(args);
            UNIT
        },
        b"os_wallpaper" => boolean(os_wallpaper()),
        b"os_cursor" => {
            unsafe { CURSOR_DOT = path_arg(args, 0) == "dot" };
            UNIT
        },
        b"os_file_icon" => {
            os_file_icon(args);
            UNIT
        },
        b"px_image" => boolean(px_image(args)),
        b"os_home_folders" => list_from(HOME_FOLDERS.iter().map(|f| mk_str(f)).collect()),
        b"os_user" => mk_str(&os_user()),
        b"os_apps" => list_from(crate::app_names().into_iter().map(mk_str).collect()),
        b"os_app_run" => boolean(app_run(&path_arg(args, 0))),
        b"os_uptime" => num(crate::arch::timer::now_ms() as f64 / 1000.0),
        b"os_info" => mk_str(&os_info(&path_arg(args, 0))),
        #[cfg(target_arch = "aarch64")]
        b"os_boot" => {
            crate::desktop_boot();
            UNIT
        },
        #[cfg(target_arch = "aarch64")]
        b"os_console" => {
            crate::desktop_console(path_arg(args, 0).as_bytes());
            UNIT
        },
        #[cfg(target_arch = "aarch64")]
        b"os_setup" => {
            crate::desktop_setup();
            UNIT
        },

        // Files: storage (lingfs) in front of the image's read-only files, with
        // the program's relative paths under its data folder (crate::vfs).
        b"file_exists" => boolean(crate::vfs::exists(&path_arg(args, 0))),
        b"read_file" => match crate::vfs::read(&path_arg(args, 0)) {
            Some(bytes) => mk_str(core::str::from_utf8(&bytes).unwrap_or("")),
            None => mk_str(""),
        },
        b"write_file" => {
            let text = args.get(1).map(|&v| to_string(v)).unwrap_or_default();
            boolean(crate::vfs::write(&path_arg(args, 0), text.as_bytes()))
        },
        b"file_modified" => num(0.0),
        b"fs_cwd" => mk_str(&crate::vfs::root()),
        b"fs_home" => mk_str(&crate::vfs::home()),
        b"fs_join" => mk_str(&fs_join(&path_arg(args, 0), &path_arg(args, 1))),
        b"fs_parent" => mk_str(&fs_parent(&path_arg(args, 0))),
        // fs_* come from the host's daw API, which returns booleans as 1/0.
        b"fs_is_dir" => num(crate::vfs::is_dir(&path_arg(args, 0)) as i32 as f64),
        b"fs_list" => fs_list(&path_arg(args, 0)),
        b"fs_mkdir" => num(crate::vfs::mkdir(&path_arg(args, 0)) as i32 as f64),
        b"fs_delete" | b"fs_copy" | b"fs_reveal" => num(0.0),
        b"dlg_open" => mk_str(&file_dialog(Dlg::Open, &path_arg(args, 0), &exts_arg(args, 1), &path_arg(args, 2), "")),
        b"dlg_save" => mk_str(&file_dialog(Dlg::Save, &path_arg(args, 0), &exts_arg(args, 1), "", &path_arg(args, 2))),
        b"dlg_folder" => mk_str(&file_dialog(Dlg::Folder, &path_arg(args, 0), &[], &path_arg(args, 1), "")),

        _ => {
            let _ = str_of;
            return None;
        },
    })
}
