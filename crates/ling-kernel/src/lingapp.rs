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

/// px_snap_save / px_snap_load: a copy of the framebuffer (the window behind a dialog)
static mut SNAP: Vec<u32> = Vec::new();

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

/// fill rows y0..y1, columns x0..x1 (already clipped) with a colour at alpha `al`
fn fill_box(w: &mut Window, x0: i32, y0: i32, x1: i32, y1: i32, r: u32, g: u32, b: u32, al: f32) {
    let bw = w.w as i32;
    for yy in y0..y1 {
        let row = (yy * bw) as usize;
        for xx in x0..x1 {
            let i = row + xx as usize;
            w.buf[i] = blend(w.buf[i], r, g, b, al);
        }
    }
}

/// px_hatch(x, y, w, h, colour, angle, spacing, a): parallel 1 px lines across the box (the design system's hatching)
fn px_hatch(a: Arg) {
    let Some(w) = win() else { return };
    let (x, y, ww, hh) = (a(0, 0.0), a(1, 0.0), a(2, 0.0), a(3, 0.0));
    let (r, g, b) = tu(w, a(4, 0.0));
    let (ang, sp, al) = (a(5, 45.0), a(6, 4.0).max(1.0), a(7, 1.0).clamp(0.0, 1.0));
    if !(ww >= 1.0 && hh >= 1.0) || al <= 0.0 {
        return;
    }
    let rr = ang * core::f64::consts::PI / 180.0;
    let (nx, ny) = (libm::sin(rr), libm::cos(rr));
    let cs = [x * nx + y * ny, (x + ww) * nx + y * ny, x * nx + (y + hh) * ny, (x + ww) * nx + (y + hh) * ny];
    let lo = cs.iter().copied().fold(f64::MAX, f64::min);
    let off = lo + sp * 0.5;
    let (cx0, cy0, cx1, cy1) = clip_box(w);
    let x0 = (libm::floor(x) as i32).max(cx0);
    let x1 = (libm::ceil(x + ww) as i32).min(cx1);
    let y0 = (libm::floor(y) as i32).max(cy0);
    let y1 = (libm::ceil(y + hh) as i32).min(cy1);
    let bw = w.w as i32;
    for yy in y0..y1 {
        let row = (yy * bw) as usize;
        let pyn = (yy as f64 + 0.5) * ny;
        for xx in x0..x1 {
            let u = ((xx as f64 + 0.5) * nx + pyn - off) / sp;
            let d = libm::fabs(u - libm::round(u)) * sp;
            if d >= 1.0 {
                continue;
            }
            let i = row + xx as usize;
            w.buf[i] = blend(w.buf[i], r.min(255), g.min(255), b.min(255), ((1.0 - d) * al) as f32);
        }
    }
}

fn num_at(list: u64, i: usize) -> f64 {
    crate::lingrt::list_items(list).get(i).map(|&v| to_f64(v)).unwrap_or(0.0)
}

/// px_bars(x0, mid, step, scale, peaks, colour [, a]): a waveform, one bar per peak mirrored around `mid`
fn px_bars(args: &[u64], a: Arg) {
    let Some(w) = win() else { return };
    let (x0, mid, step, scale) = (a(0, 0.0), a(1, 0.0), a(2, 1.0), a(3, 1.0));
    let (r, g, b) = tu(w, a(5, 0.0));
    let al = a(6, 1.0) as f32;
    let Some(&pk) = args.get(4) else { return };
    let n = crate::lingrt::list_items(pk).len();
    let (cx0, cy0, cx1, cy1) = clip_box(w);
    for i in 0..n {
        let hgt = num_at(pk, i) * scale;
        if !(hgt > 0.0) {
            continue;
        }
        let bx = x0 + i as f64 * step;
        let xa = (libm::round(bx) as i32).max(cx0);
        let xb = (libm::round(bx + step.max(1.0)) as i32).max(libm::round(bx) as i32 + 1).min(cx1);
        let ya = (libm::round(mid - hgt) as i32).max(cy0);
        let yb = (libm::round(mid + hgt.max(0.5)) as i32).max(libm::round(mid - hgt) as i32 + 1).min(cy1);
        if xa < xb && ya < yb {
            fill_box(w, xa, ya, xb, yb, r.min(255), g.min(255), b.min(255), al);
        }
    }
}

/// px_notes(notes stride 7, count, x, y, w, h, beat width, lowest key, span, loop beats, visible x0, x1, colour a, colour b)
fn px_notes(args: &[u64], a: Arg) {
    let Some(w) = win() else { return };
    let Some(&nl) = args.first() else { return };
    let (cnt, x, y, ww, hh, bwd, lo, span, ll, vx0, vx1) = (a(1, 0.0) as usize, a(2, 0.0), a(3, 0.0), a(4, 0.0), a(5, 0.0), a(6, 1.0), a(7, 0.0), a(8, 1.0).max(1.0), a(9, 0.0), a(10, 0.0), a(11, 0.0));
    let (ca, cb) = (tu(w, a(12, 0.0)), tu(w, a(13, 0.0)));
    let cnt = cnt.min(crate::lingrt::list_items(nl).len() / 7);
    let (cx0, cy0, cx1, cy1) = clip_box(w);
    let (reps, rp0) = if ll > 0.5 { (libm::ceil((vx1 - x) / bwd / ll).max(0.0) as usize, libm::floor((vx0 - x) / bwd / ll).max(0.0) as usize) } else { (1, 0) };
    for rp in rp0..reps.min(rp0 + 4096) {
        for i in 0..cnt {
            let ns = num_at(nl, i * 7);
            let nx = x + (ns + rp as f64 * ll) * bwd;
            if nx >= vx1 || (ll > 0.5 && ns >= ll) {
                continue;
            }
            let nw = (num_at(nl, i * 7 + 1) * bwd).max(1.5).min(x + ww - nx);
            if nx + nw <= vx0 {
                continue;
            }
            let ny = y + (1.0 - (num_at(nl, i * 7 + 2) - lo) / span) * (hh - 3.0);
            let al = ((num_at(nl, i * 7 + 3) * 200.0 + 30.0).clamp(40.0, 230.0) / 255.0) as f32;
            let (r, g, b) = if i % 2 == 0 { ca } else { cb };
            let xa = (libm::round(nx) as i32).max(cx0);
            let xb = (libm::round(nx + nw) as i32).max(libm::round(nx) as i32 + 1).min(cx1);
            let (ya, yb) = ((libm::round(ny) as i32).max(cy0), (libm::round(ny + 2.0) as i32).min(cy1));
            if xa < xb && ya < yb {
                fill_box(w, xa, ya, xb, yb, r.min(255), g.min(255), b.min(255), al);
            }
        }
    }
}

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

/// Standard modes up to what the back buffer holds, and no larger than the
/// display's preferred (EDID) mode when it is known. The live mode is always
/// in the list.
fn display_modes() -> Vec<(u32, u32)> {
    const CANDIDATES: [(u32, u32); 7] =
        [(640, 480), (800, 600), (1024, 768), (1280, 720), (1366, 768), (1600, 900), (1920, 1080)];
    #[cfg(target_arch = "aarch64")]
    let pref = crate::arch::mailbox::edid_preferred_resolution();
    #[cfg(not(target_arch = "aarch64"))]
    let pref: Option<(u32, u32)> = None;
    let cur = framebuffer::mode_dims();
    let mut out: Vec<(u32, u32)> = CANDIDATES
        .iter()
        .copied()
        .filter(|&(w, h)| match pref {
            Some((pw, ph)) if pw > 0 && ph > 0 => w <= pw && h <= ph,
            _ => true,
        })
        .collect();
    if cur.0 > 0 && !out.contains(&cur) {
        out.push(cur);
        out.sort();
    }
    out
}

/// Switch the screen and re-create the window at the new size (the desktop
/// reads `get_width` / `get_height` every frame; the wallpaper cache is keyed
/// by the window size, so it re-decodes by itself).
fn display_set(w: u32, h: u32) -> bool {
    if (w, h) == framebuffer::mode_dims() {
        return true;
    }
    if !framebuffer::set_mode(w, h) {
        return false;
    }
    let (fw, fh) = (framebuffer::width() as usize, framebuffer::height() as usize);
    unsafe {
        if let Some(win) = (*(&raw mut WIN)).as_mut() {
            win.buf = vec![0u32; fw * fh];
            win.w = fw;
            win.h = fh;
            win.clip = None;
        }
        (*(&raw mut SNAP)).clear();
    }
    true
}

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
    // Suspended earlier (Alt+Tab): pick up where it was.
    if parked_apps().iter().any(|a| a.name == name) {
        return app_resume(name);
    }
    let Some(entry) = crate::app_named(name.as_bytes()) else { return false };
    let folder = app_folder(name);
    let Some(stack) = crate::mm::frame::alloc_frames(APP_STACK_ORDER) else {
        crate::console_write(b"app: no memory for its stack\n");
        return false;
    };
    let top = (stack + (4096u64 << APP_STACK_ORDER)) & !0xF;
    // The desktop's window and file scope go aside while the app has the screen.
    let host = host_put_aside();
    // Its settings and caches under Programs/<App>, its projects under
    // Projects/<App> (both in the user's home), its bundled files from
    // the image's apps/<name>/ folder.
    let home = crate::vfs::home();
    let programs = alloc::format!("{home}/Programs/{folder}");
    let projects = alloc::format!("{home}/Projects/{folder}");
    if crate::vfs::ready() {
        crate::vfs::mkdir(&programs);
        crate::vfs::mkdir(&projects);
    }
    crate::vfs::set_root(&programs);
    crate::vfs::set_projects_root(&projects);
    crate::vfs::set_image_root(&alloc::format!("apps/{name}"));
    crate::console_write(b"app: starting ");
    crate::console_write(name.as_bytes());
    crate::console_write(b"\n");
    let sp = unsafe { fresh_stack(top) };
    unsafe {
        *(&raw mut ACTIVE_APP) = Some(AppCtx { name: String::from(name), stack, top, sp, win: None, dot: false, scope: None, park: 0, done: false });
        *(&raw mut PENDING_ENTRY) = Some(entry);
        // Runs the app on its stack until it suspends or returns.
        ling_switch_stack(&raw mut HOST_SP, sp);
    }
    app_came_back(host);
    true
}

// ── Running apps: each on its own stack, suspendable ─────────────────────────
// An app runs on a 2 MiB stack of its own, entered with a stack switch from
// the desktop's `os_app_run`. Alt+Tab inside the app (seen by `present`) parks
// it: its window, file scope and Ling scope go aside, the desktop's come back,
// and the switch returns to the desktop, which goes on as if the app had
// closed -- except that `os_apps_running` lists it and `os_app_resume` (or a
// second `os_app_run`) switches back into its `present`, which then returns
// to the app's loop. When the app's entry returns, the trampoline switches to
// the desktop for good and the stack is freed. Only one app runs at a time;
// the desktop is always the host. The collector scans every parked stack.

struct AppCtx {
    name: String,
    stack: u64,
    top: u64,
    /// Its sp while it is not running (the switch's saved frame).
    sp: u64,
    win: Option<Window>,
    dot: bool,
    scope: Option<crate::vfs::Scope>,
    /// `lingrt::park_scope` ticket while suspended.
    park: u64,
    done: bool,
}

/// The desktop's state while an app has the screen.
struct Host {
    win: Option<Window>,
    dot: bool,
    scope: crate::vfs::Scope,
}

const APP_STACK_ORDER: usize = 9; // 2 MiB
static mut ACTIVE_APP: Option<AppCtx> = None;
static mut PARKED_APPS: Vec<AppCtx> = Vec::new();
/// The desktop's sp while an app runs (the switch's saved frame).
static mut HOST_SP: u64 = 0;
static mut PENDING_ENTRY: Option<crate::AppEntry> = None;
/// Settings > Sound: pause LingStationX's transport when it is suspended or closes.
static mut PAUSE_AUDIO: bool = true;

fn parked_apps() -> &'static mut Vec<AppCtx> {
    unsafe { &mut *(&raw mut PARKED_APPS) }
}

fn active_app() -> Option<&'static mut AppCtx> {
    unsafe { (*(&raw mut ACTIVE_APP)).as_mut() }
}

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".global ling_switch_stack",
    "ling_switch_stack:",
    "sub sp, sp, #176",
    "stp x19, x20, [sp, #0]",
    "stp x21, x22, [sp, #16]",
    "stp x23, x24, [sp, #32]",
    "stp x25, x26, [sp, #48]",
    "stp x27, x28, [sp, #64]",
    "stp x29, x30, [sp, #80]",
    "stp d8, d9, [sp, #96]",
    "stp d10, d11, [sp, #112]",
    "stp d12, d13, [sp, #128]",
    "stp d14, d15, [sp, #144]",
    "mov x2, sp",
    "str x2, [x0]",
    "mov sp, x1",
    "ldp x19, x20, [sp, #0]",
    "ldp x21, x22, [sp, #16]",
    "ldp x23, x24, [sp, #32]",
    "ldp x25, x26, [sp, #48]",
    "ldp x27, x28, [sp, #64]",
    "ldp x29, x30, [sp, #80]",
    "ldp d8, d9, [sp, #96]",
    "ldp d10, d11, [sp, #112]",
    "ldp d12, d13, [sp, #128]",
    "ldp d14, d15, [sp, #144]",
    "add sp, sp, #176",
    "ret",
);
#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".global ling_switch_stack",
    "ling_switch_stack:",
    "push rbx",
    "push rbp",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov [rdi], rsp",
    "mov rsp, rsi",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    "ret",
);
extern "C" {
    /// Save the callee-saved registers and sp to `*save`, switch to the stack
    /// at `to` and return into whatever saved that one (or `app_trampoline`
    /// for a fresh stack).
    fn ling_switch_stack(save: *mut u64, to: u64);
}

/// A new stack whose first switch-in lands in `app_trampoline`.
unsafe fn fresh_stack(top: u64) -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        let sp = top - 176;
        core::ptr::write_bytes(sp as *mut u8, 0, 176);
        core::ptr::write((sp + 88) as *mut u64, app_trampoline as usize as u64); // x30
        sp
    }
    #[cfg(target_arch = "x86_64")]
    {
        let mut sp = top - 16;
        core::ptr::write(sp as *mut u64, app_trampoline as usize as u64);
        for _ in 0..6 {
            sp -= 8;
            core::ptr::write(sp as *mut u64, 0);
        }
        sp
    }
}

/// First thing on an app's stack: run its entry, then hand the stack back.
extern "C" fn app_trampoline() -> ! {
    let entry = unsafe { (*(&raw mut PENDING_ENTRY)).take() };
    if let Some(e) = entry {
        crate::lingrt::run_isolated(e);
    }
    if let Some(a) = active_app() {
        a.done = true;
    }
    unsafe {
        let mut gone = 0u64;
        ling_switch_stack(&raw mut gone, HOST_SP);
    }
    loop {}
}

fn host_put_aside() -> Host {
    let win = unsafe { core::ptr::replace(&raw mut WIN, None) };
    let dot = unsafe { core::mem::replace(&mut *(&raw mut CURSOR_DOT), false) };
    let scope = crate::vfs::take_scope();
    input_reset();
    Host { win, dot, scope }
}

fn input_reset() {
    let input = unsafe { &mut *(&raw mut INPUT) };
    input.pressed.clear();
    input.text.clear();
    input.quit = false;
    input.esc = 0;
}

/// Back on the desktop's stack after an app suspended or returned.
fn app_came_back(host: Host) {
    let ctx = unsafe { (*(&raw mut ACTIVE_APP)).take() };
    crate::vfs::restore_scope(host.scope);
    unsafe {
        core::ptr::write(&raw mut WIN, host.win);
        CURSOR_DOT = host.dot;
        if PAUSE_AUDIO {
            crate::lingdaw::pause_transport();
        }
    }
    input_reset();
    if let Some(c) = ctx {
        crate::console_write(b"app: ");
        crate::console_write(c.name.as_bytes());
        if c.done {
            crate::console_write(b" returned\n");
            crate::mm::frame::free_frames(c.stack, APP_STACK_ORDER);
        } else {
            crate::console_write(b" suspended\n");
            parked_apps().push(c);
        }
    }
}

/// Switch back into a suspended app; returns when it suspends again or ends.
fn app_resume(name: &str) -> bool {
    let Some(i) = parked_apps().iter().position(|a| a.name == name) else { return false };
    let ctx = parked_apps().remove(i);
    let sp = ctx.sp;
    let host = host_put_aside();
    unsafe {
        *(&raw mut ACTIVE_APP) = Some(ctx);
        ling_switch_stack(&raw mut HOST_SP, sp);
    }
    app_came_back(host);
    true
}

/// Inside an app, from `present`: park it and give the screen back to the
/// desktop; continues here when the desktop resumes it.
fn app_suspend() {
    let Some(a) = active_app() else { return };
    a.win = unsafe { core::ptr::replace(&raw mut WIN, None) };
    a.dot = unsafe { CURSOR_DOT };
    a.scope = Some(crate::vfs::take_scope());
    a.park = crate::lingrt::park_scope();
    let sp_slot = &raw mut a.sp;
    unsafe { ling_switch_stack(sp_slot, HOST_SP) };
    // Resumed: the context is back in ACTIVE_APP (it moved meanwhile).
    if let Some(a) = active_app() {
        unsafe {
            core::ptr::write(&raw mut WIN, a.win.take());
            CURSOR_DOT = a.dot;
        }
        if let Some(s) = a.scope.take() {
            crate::vfs::restore_scope(s);
        }
        crate::lingrt::unpark_scope(a.park);
    }
    input_reset();
}

/// For the collector: the top of the stack we are on, and every parked stack
/// (the desktop's while an app runs, every suspended app's).
pub fn gc_stacks() -> (u64, Vec<(u64, u64)>) {
    let host_top = crate::proc::sched::current_stack_top();
    let mut others: Vec<(u64, u64)> = parked_apps().iter().map(|a| (a.sp, a.top)).collect();
    match active_app() {
        Some(a) => {
            others.push((unsafe { HOST_SP }, host_top));
            (a.top, others)
        },
        None => (host_top, others),
    }
}

fn apps_running() -> Vec<String> {
    parked_apps().iter().map(|a| a.name.clone()).collect()
}

// ── Network settings (Settings > Network) ────────────────────────────────────
fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut n = 0;
    for part in s.trim().split('.') {
        if n == 4 {
            return None;
        }
        out[n] = part.trim().parse::<u8>().ok()?;
        n += 1;
    }
    if n == 4 { Some(out) } else { None }
}

fn fmt_ip(a: [u8; 4]) -> String {
    alloc::format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3])
}

/// "mode\tip\tmask\tgateway\tdns": the static setting, or the live DHCP state.
fn net_get() -> String {
    use crate::drivers::netstack as ns;
    match ns::static_config() {
        Some((ip, mask, gw, dns)) => alloc::format!("static\t{}\t{}\t{}\t{}", fmt_ip(ip), fmt_ip(mask), fmt_ip(gw), fmt_ip(dns)),
        None => alloc::format!("dhcp\t{}\t255.255.255.0\t{}\t{}", fmt_ip(ns::self_ip()), fmt_ip(ns::gateway_ip()), fmt_ip(ns::dns_servers().0)),
    }
}

/// Step by step, what works: ARP to the gateway, DNS, TCP, HTTPS -- the
/// lines the Settings window shows, so a failure can be named without a
/// serial console.
fn net_test() -> String {
    use crate::drivers::netstack as ns;
    let mut out = String::new();
    let ip = ns::self_ip();
    let gw = ns::gateway_ip();
    #[cfg(target_arch = "aarch64")]
    let kind = crate::arch::netdev::kind();
    #[cfg(not(target_arch = "aarch64"))]
    let kind = "ethernet";
    out.push_str(&alloc::format!("address {}  gateway {}  ({})\n", fmt_ip(ip), fmt_ip(gw), kind));
    // No link at all: Wi-Fi not joined and no wired controller -- nothing below can work.
    #[cfg(target_arch = "aarch64")]
    if !crate::wifi::joined() && !crate::arch::netdev::ethernet_present() {
        let note = crate::wifi::link_note();
        out.push_str("link: NONE -- Wi-Fi is not joined");
        if !note.is_empty() {
            out.push_str(": ");
            out.push_str(&note);
        }
        out.push_str("\n   (the Wi-Fi window joins a network; a remembered one is rejoined by itself)\n");
        return out;
    }
    #[cfg(target_arch = "aarch64")]
    if !crate::wifi::joined() && !crate::arch::netdev::link_up() {
        out.push_str("link: Ethernet controller up but no cable / link\n");
        return out;
    }
    // TLS cannot run without hardware randomness (client random, key share).
    let mut rb = [0u8; 8];
    out.push_str(if crate::crypto::random_bytes(&mut rb) { "random source: ok\n" } else { "random source: NONE -- HTTPS cannot work on this board yet\n" });
    if ip == [0; 4] {
        out.push_str("no address: join a network, or set a static address\n");
        return out;
    }
    match ns::arp_resolve(gw) {
        Some(m) => out.push_str(&alloc::format!("ARP gateway: ok ({})\n", fmt_mac(m))),
        None => {
            out.push_str("ARP gateway: no reply (frames do not reach the router, or it is not on this subnet)\n");
            return out;
        },
    }
    // Routing past the router, with no DNS involved.
    match ns::tcp_connect_h([1, 1, 1, 1], 443, 3_000_000) {
        Some(h) => {
            ns::tcp_close_h(h);
            out.push_str("TCP 1.1.1.1:443: connected (the router forwards our traffic)\n");
        },
        None => {
            out.push_str("TCP 1.1.1.1:443: no answer\n");
            out.push_str(&crate::wifi::last_frames());
            out.push('\n');
        },
    }
    let (dns1, _) = ns::dns_servers();
    let host = "ling-lang.org";
    let mut hip: Option<[u8; 4]> = None;
    let mut via = [0u8; 4];
    let mut tried = String::new();
    for server in [dns1, gw, [1, 1, 1, 1], [8, 8, 8, 8]] {
        if server == [0; 4] || tried.contains(&fmt_ip(server)) {
            continue;
        }
        if !tried.is_empty() {
            tried.push_str(", ");
        }
        tried.push_str(&fmt_ip(server));
        if let Some(ip) = ns::dns_query_at(server, host) {
            hip = Some(ip);
            via = server;
            break;
        }
    }
    let Some(hip) = hip else {
        out.push_str(&alloc::format!("DNS {host}: failed via {tried}\n"));
        out.push_str(&crate::wifi::last_frames());
        out.push('\n');
        return out;
    };
    out.push_str(&alloc::format!("DNS {host}: {} via {}{}\n", fmt_ip(hip), fmt_ip(via), if via != dns1 { " (your DNS entry did not answer)" } else { "" }));
    match ns::tcp_connect_h(hip, 443, 4_000_000) {
        Some(h) => {
            ns::tcp_close_h(h);
            out.push_str("TCP 443: connected\n");
        },
        None => {
            out.push_str("TCP 443: no connection (SYN unanswered)\n");
            return out;
        },
    }
    let (code, body) = crate::netapi::http_get("https://ling-lang.org/");
    if code == 0 {
        out.push_str("HTTPS GET: failed (TLS handshake or no response)\n");
    } else {
        out.push_str(&alloc::format!("HTTPS GET: status {code}, {} bytes\n", body.len()));
    }
    out
}

/// Apply a configuration; the text is what to show the user.
fn net_set(mode: &str, ip: &str, mask: &str, gw: &str, dns: &str) -> String {
    use crate::drivers::netstack as ns;
    if mode == "static" {
        let (Some(ip), Some(mask), Some(gw)) = (parse_ip(ip), parse_ip(mask), parse_ip(gw)) else {
            return String::from("address, netmask and gateway must be four numbers 0-255 (like 192.168.1.20)");
        };
        let dns = parse_ip(dns).unwrap_or(gw);
        ns::set_static(Some((ip, mask, gw, dns)));
        alloc::format!("static address {} set (gateway {}, DNS {})", fmt_ip(ip), fmt_ip(gw), fmt_ip(dns))
    } else {
        ns::set_static(None);
        if ns::dhcp_configure() {
            alloc::format!("DHCP gave {}", fmt_ip(ns::self_ip()))
        } else {
            String::from("DHCP: no lease yet (no link, or no server); a static address always works")
        }
    }
}

// ── Accounts (the desktop's login screen) ────────────────────────────────────
fn users_list() -> Vec<String> {
    let mut out = Vec::new();
    if !crate::fs::lingfs::is_mounted() {
        return out;
    }
    let mut name = [0u8; 64];
    let mut i = 0usize;
    while let Some((n, is_dir)) = crate::fs::lingfs::list_entry("users", i, &mut name) {
        if !is_dir {
            if let Ok(s) = core::str::from_utf8(&name[..n]) {
                out.push(String::from(s));
            }
        }
        i += 1;
        if i > 256 {
            break;
        }
    }
    out
}

/// Sign in: the user becomes the owner of new files, gets a home, and the
/// Messenger history key is derived from the password.
fn user_login(name: &str, password: &str) -> bool {
    if !crate::fs::users::verify(name, password) {
        return false;
    }
    let mut group = [0u8; 32];
    let n = crate::fs::users::group_of(name, &mut group).unwrap_or(0);
    let group = core::str::from_utf8(&group[..n]).unwrap_or("users");
    crate::fs::lingfs::set_current_user(name, group);
    crate::fs::users::ensure_home(name);
    // the keyring opens with this password: chat history and private files
    crate::keyring::unlock(name, password);
    crate::drivers::messenger::unlock_for_session(password.as_bytes());
    true
}

fn user_create(name: &str, password: &str) -> bool {
    if name.is_empty() || password.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return false;
    }
    if !crate::fs::users::create(name, password, "", "", "", "", "users") {
        return false;
    }
    user_login(name, password)
}

/// The home folder name of a bundled app (`Projects/<App>`, `Programs/<App>`).
pub fn app_folder(name: &str) -> String {
    String::from(match name {
        "lingstation" => "LingStationX",
        "lingdev" => "LingDev",
        "lingmodeler" => "LingModeler",
        "horizon" => "Horizon",
        n => n,
    })
}

/// Run `body` with a fresh window slot and cursor state, restoring the
/// caller's afterwards: a launched program (bundled app or `ling run` script)
/// opens its own window without disturbing the desktop's.
pub fn with_own_window(body: impl FnOnce()) {
    let saved = unsafe { core::ptr::replace(&raw mut WIN, None) };
    let dot = unsafe { core::mem::replace(&mut *(&raw mut CURSOR_DOT), false) };
    let scope = crate::vfs::take_scope();
    body();
    crate::vfs::restore_scope(scope);
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
pub const HOME_FOLDERS: [&str; 9] =
    ["Documents", "Photos", "Music", "Videos", "Graphics", "3D Models", "Projects", "Programs", "Scripts"];

// 8x8 emblems (bit 7 = left pixel), the same art as the x86 Files window.
const EMB_DOC: [u8; 8] = [0x00, 0x7E, 0x00, 0x7E, 0x00, 0x78, 0x00, 0x00];
const EMB_PHOTO: [u8; 8] = [0x06, 0x06, 0x00, 0x20, 0x72, 0xFF, 0xFF, 0x00];
const EMB_MUSIC: [u8; 8] = [0x1E, 0x12, 0x12, 0x12, 0x72, 0xF6, 0x66, 0x00];
const EMB_VIDEO: [u8; 8] = [0x40, 0x60, 0x70, 0x78, 0x70, 0x60, 0x40, 0x00];
const EMB_GRAPHICS: [u8; 8] = [0x38, 0x44, 0x44, 0x38, 0x10, 0x38, 0x7C, 0x00];
const EMB_MODEL: [u8; 8] = [0x3E, 0x46, 0xFA, 0x8A, 0x8C, 0x88, 0xF8, 0x00];
const EMB_PROGRAM: [u8; 8] = [0xFF, 0x81, 0xA1, 0x91, 0xA1, 0x81, 0xFF, 0x00];
const EMB_SCRIPT: [u8; 8] = [0x24, 0x42, 0x81, 0x00, 0x81, 0x42, 0x24, 0x00];

pub(crate) fn gfx_color() -> u32 {
    unsafe { GFX_COLOR }
}

/// The app window's back buffer (`0xRRGGBB`), width and height -- the
/// native 3-D renderer (`drivers::g3d`) draws into it directly.
pub(crate) fn surface() -> Option<(&'static mut [u32], usize, usize)> {
    let w = win()?;
    let (ww, hh) = (w.w, w.h);
    Some((&mut w.buf[..], ww, hh))
}

/// A loaded font's face (`font_load` id), for glyph textures.
pub(crate) fn font_fd(id: f64) -> Option<&'static fontdue::Font> {
    font(id).map(|f| &f.fd)
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
            "Projects" => (0xE08A4A, Some(&EMB_PROGRAM)),
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
            // .svgz (and any gzip'd picture) comes in compressed.
            let data = crate::drivers::image::gunzip(&data).unwrap_or(data);
            let data: &[u8] = &data;
            let l = path.to_ascii_lowercase();
            if l.ends_with(".bmp") {
                decode_bmp(data)
            } else if l.ends_with(".svg") || l.ends_with(".svgz") || crate::drivers::image::looks_svg(data) {
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

/// The built-in home: the standard folders (an empty `.keep` makes each one
/// listable) and starter files under `home/<user>/`, served from the image
/// behind whatever the SD card's LingOS storage holds for the same paths.
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
        ("Graphics/ling-badge.svg", b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 200 200\"><rect width=\"200\" height=\"200\" fill=\"#1b1b1b\"/><circle cx=\"100\" cy=\"100\" r=\"72\" style=\"fill:#3baaeb;stroke:#f7941e;stroke-width:6\"/><path d=\"M 100 40 A 60 60 0 0 1 160 100 L 100 100 Z\" fill=\"#f7d846\" opacity=\"0.85\"/><path d=\"M 55 125 Q 100 170 145 125 T 145 125\" style=\"fill:none;stroke:#ffffff;stroke-width:5;stroke-opacity:0.9\"/><polygon points=\"100,60 128,120 72,120\" fill=\"#ffffff\" fill-opacity=\"0.7\"/></svg>\n"),
        ("Documents/Welcome.txt", b"Welcome to LingOS!\n\nYour home folders:\n  Documents  - text and notes\n  Photos     - BMP / PNG / SVG pictures\n  Music      - WAV audio\n  Videos     - video files\n  Graphics   - SVG drawings and .ling graphics programs\n  3D Models  - 3D model files\n  Programs   - lingfu programs and downloaded source\n  Projects   - LingStationX / LingDev / LingModeler projects\n  Scripts    - .ling scripts\n\nOn the Raspberry Pi these live on the SD card's LingOS storage\n(Settings > System sets it up); the starter files come from the image.\n"),
    ];
    for (rel, data) in starters {
        crate::appfs::register(leak(alloc::format!("home/{user}/{rel}")), data);
    }
}

/// The signed-in user's name: the lingfs session (both architectures).
fn os_user() -> String {
    let u = crate::fs::lingfs::current_user();
    if u.is_empty() {
        String::from("ling")
    } else {
        String::from(u)
    }
}

/// `os_time` for the console (`date`).
pub(crate) fn wall_time() -> u64 {
    os_time()
}

/// "YYYY-MM-DD HH:MM:SS" of a UTC epoch second.
pub(crate) fn format_utc(t: u64) -> String {
    let days = (t / 86_400) as i64;
    let rem = t % 86_400;
    // civil from days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    alloc::format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m, d, rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// Seconds since 1970-01-01 UTC, or 0 when the board has no clock to ask.
/// x86: the CMOS clock; Pi 5: the firmware RTC, read once and carried by the
/// uptime counter.
fn os_time() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let d = crate::arch::rtc::read();
        if d.year < 2000 {
            return 0;
        }
        let (y, m) = (d.year as i64, d.month as i64);
        let (yy, mm) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
        let era = yy.div_euclid(400);
        let yoe = yy - era * 400;
        let doy = (153 * mm + 2) / 5 + d.day as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        (days * 86_400 + d.hour as i64 * 3600 + d.minute as i64 * 60 + d.second as i64).max(0) as u64
    }
    #[cfg(target_arch = "aarch64")]
    {
        static mut BOOT_EPOCH: Option<u64> = None;
        static mut TRIED: bool = false;
        let up = crate::arch::timer::now_ms() / 1000;
        unsafe {
            if !TRIED {
                TRIED = true;
                BOOT_EPOCH = crate::arch::mailbox::rtc_time().map(|t| (t as u64).saturating_sub(up));
            }
            BOOT_EPOCH.map(|b| b + up).unwrap_or(0)
        }
    }
}

/// `os_info(key)`: facts about the running system for the desktop's About/tray.
/// The USB gamepad's state (none on x86 yet).
fn pad() -> crate::drivers::hidpad::PadState {
    #[cfg(target_arch = "aarch64")]
    return crate::arch::xhci::pad_state();
    #[cfg(not(target_arch = "aarch64"))]
    crate::drivers::hidpad::PadState::default()
}

fn fmt_mac(m: [u8; 6]) -> String {
    alloc::format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
}

fn os_info(key: &str) -> String {
    use alloc::format;
    match key {
        "hostname" => {
            #[cfg(target_arch = "aarch64")]
            return crate::setup::hostname();
            #[allow(unreachable_code)]
            String::from("lingos")
        },
        "time" => format_utc(os_time()),
        "board" => {
            #[cfg(target_arch = "aarch64")]
            if let Some(m) = crate::arch::fdt::model() {
                return String::from(m);
            }
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
        // The hardware addresses (a router's static lease / allow list wants them).
        "mac" => {
            #[cfg(target_arch = "aarch64")]
            {
                if crate::arch::rp1_eth::ensure_init() {
                    fmt_mac(crate::arch::rp1_eth::mac())
                } else {
                    let why = crate::arch::rp1_eth::last_error();
                    if why.is_empty() {
                        String::from("no Ethernet controller")
                    } else {
                        format!("no Ethernet controller ({why})")
                    }
                }
            }
            #[cfg(target_arch = "x86_64")]
            fmt_mac(crate::drivers::net_e1000::mac())
        },
        "mac_wifi" => {
            #[cfg(target_arch = "aarch64")]
            {
                let m = crate::arch::wifi::mac();
                if m == [0; 6] { String::from("radio not started (open Wi-Fi networks once)") } else { fmt_mac(m) }
            }
            #[cfg(not(target_arch = "aarch64"))]
            String::from("no Wi-Fi radio")
        },
        "display" => format!("{} x {}", framebuffer::width(), framebuffer::height()),
        "gamepad" => {
            let p = pad();
            String::from(if !p.connected { "none" } else if p.xbox { "Xbox 360 controller" } else { "USB gamepad" })
        },
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
        "network" => {
            #[cfg(target_arch = "aarch64")]
            {
                crate::arch::netdev::kind()
            }
            #[cfg(not(target_arch = "aarch64"))]
            {
                String::from("ethernet")
            }
        },
        "user" => os_user(),
        _ => String::new(),
    }
}

/// Kept for apps that call `os_cursor`: the pointer is now always the LingOS
/// cursor (`drivers::cursor`), so this is a no-op.
static mut CURSOR_DOT: bool = false;

/// Settings > System "Show frame timing": frame time, builtin calls and
/// collections drawn over every frame (apps included).
static mut PERF: bool = false;
static mut PERF_LAST_US: u64 = 0;
static mut PERF_FRAME_US: f64 = 0.0;
static mut PERF_CALLS: u64 = 0;

fn perf_overlay(w: &mut Window) {
    let now = crate::arch::timer::now_us();
    let (last, calls_before) = unsafe { (PERF_LAST_US, PERF_CALLS) };
    let calls_now = unsafe { crate::lingrt::BUILTIN_CALLS };
    unsafe {
        PERF_LAST_US = now;
        PERF_CALLS = calls_now;
    }
    if last == 0 {
        return;
    }
    let dt = now.saturating_sub(last) as f64;
    let ema = unsafe {
        PERF_FRAME_US = if PERF_FRAME_US == 0.0 { dt } else { PERF_FRAME_US * 0.8 + dt * 0.2 };
        PERF_FRAME_US
    };
    let fps = if ema > 0.0 { 1_000_000.0 / ema } else { 0.0 };
    let line = alloc::format!(
        " frame {:.1} ms ({:.0} fps)  builtins/frame {}  tris {}{}  gc {}  free {} MiB ",
        ema / 1000.0,
        fps,
        calls_now.saturating_sub(calls_before),
        crate::drivers::g3d::take_stats(),
        crate::drivers::g3d::scale_note(),
        crate::lingrt::gc_runs(),
        crate::mm::frame::free_frame_count() * 4 / 1024
    );
    let (x, y) = (8i64, 8i64);
    rect(w, x, y, line.len() as i64 * 8 + 4, 14, 0x10_0C18);
    text8(w, x + 2, y + 3, 0xFF_E0_60, &line);
}

fn present() {
    crate::lingrt::mark_present();
    crate::lingrt::gc_maybe();
    crate::drivers::g3d::end_frame();
    crate::wifi::tick();
    // the background tasks (SSH server, chat listener) get the CPU once a frame
    #[cfg(target_arch = "aarch64")]
    crate::proc::sched::yield_now();
    let (mx, my, _) = mouse_state();
    unsafe {
        let last = MOUSE_LAST;
        MOUSE_DELTA = if last.0.is_finite() && mx > -1000.0 && last.0 > -1000.0 { (mx - last.0, my - last.1) } else { (0.0, 0.0) };
        MOUSE_LAST = (mx, my);
    }
    if unsafe { PERF } {
        if let Some(w) = win() {
            perf_overlay(w);
        }
    }
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
        // Without HDMI audio (QEMU, a board with no sink) the engine still
        // renders a block per frame, so the transport advances the same way
        // and a render-time fault shows up in the emulator too.
        if crate::arch::hdmi_audio::running() {
            crate::arch::hdmi_audio::pump();
        } else {
            let _ = crate::lingdaw::block(512);
        }
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
    // Alt+Tab inside a launched app: back to the desktop, the app kept alive.
    if active_app().is_some() && key_down("alt") && key_pressed("tab") {
        app_suspend();
    }
}

/// Keyboard state between frames: text typed since the last `text_poll`, key
/// names pressed during the last frame (from terminal-style bytes), and the
/// USB keyboard's held keys now and one frame ago (for `key_down` and edge
/// `key_pressed` on keys that send no bytes, like Super or F-keys).
struct Input {
    text: String,
    /// Digits of a `CSI n ~` key sequence being read (F5..F12 over serial).
    esc_num: u32,
    /// `CSI < b ; x ; y M|m` mouse report being read: the three numbers.
    esc_args: [u32; 3],
    esc_argi: usize,
    esc_mouse: bool,
    pressed: Vec<&'static str>,
    esc: u8, // 0 idle, 1 after ESC, 2 after ESC [
    quit: bool,
    held: [u8; 8],
    prev_held: [u8; 8],
    wheel: i32,
}

static mut INPUT: Input = Input {
    text: String::new(),
    esc_num: 0,
    esc_args: [0; 3],
    esc_argi: 0,
    esc_mouse: false,
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
            (2, b'H') => self.pressed.push("home"),
            (2, b'F') => self.pressed.push("end"),
            // `ESC [ < b ; x ; y M` (press) / `m` (release): a terminal's SGR
            // mouse report -- the pointer over the serial line.
            (2, b'<') => {
                self.esc_mouse = true;
                self.esc_args = [0; 3];
                self.esc_argi = 0;
                return;
            },
            (2, b';') if self.esc_mouse => {
                if self.esc_argi < 2 {
                    self.esc_argi += 1;
                }
                return;
            },
            (2, b'M' | b'm') if self.esc_mouse => {
                let (b, x, y) = (self.esc_args[0], self.esc_args[1], self.esc_args[2]);
                let bit = match b & 0x3 {
                    0 => 1u8,
                    1 => 4,
                    2 => 2,
                    _ => 0,
                };
                let m = unsafe { &mut *(&raw mut SERIAL_MOUSE) };
                let mut buttons = m.map(|(_, _, bt)| bt).unwrap_or(0);
                if b & 0x20 == 0 {
                    if c == b'M' {
                        buttons |= bit;
                    } else {
                        buttons &= !bit;
                    }
                }
                *m = Some((x.saturating_sub(1) as i32, y.saturating_sub(1) as i32, buttons));
                self.esc_mouse = false;
            },
            // `ESC [ n ~`: F5..F12, Home/End/PageUp/PageDown/Delete as a terminal sends them.
            (2, b'0'..=b'9') => {
                if self.esc_mouse {
                    let i = self.esc_argi.min(2);
                    self.esc_args[i] = self.esc_args[i] * 10 + (c - b'0') as u32;
                } else {
                    self.esc_num = self.esc_num * 10 + (c - b'0') as u32;
                }
                return;
            },
            (2, b'~') => {
                let name = match self.esc_num {
                    1 | 7 => "home",
                    4 | 8 => "end",
                    5 => "pageup",
                    6 => "pagedown",
                    3 => "delete",
                    11 => "f1",
                    12 => "f2",
                    13 => "f3",
                    14 => "f4",
                    15 => "f5",
                    17 => "f6",
                    18 => "f7",
                    19 => "f8",
                    20 => "f9",
                    21 => "f10",
                    23 => "f11",
                    24 => "f12",
                    _ => "",
                };
                if !name.is_empty() {
                    self.pressed.push(name);
                }
                self.esc_num = 0;
            },
            // `ESC TAB`: Alt+Tab as a terminal sends it (the desktop's window switcher).
            (1, b'\t') => {
                self.pressed.push("alt");
                self.pressed.push("tab");
            },
            // `ESC O P..S`: F1..F4 in application mode.
            (1, b'O') => {
                self.esc = 3;
                return;
            },
            (3, b'P') => self.pressed.push("f1"),
            (3, b'Q') => self.pressed.push("f2"),
            (3, b'R') => self.pressed.push("f3"),
            (3, b'S') => self.pressed.push("f4"),
            (3, _) => {},
            (1, _) => {
                // A lone ESC followed by something else: Escape, then the byte.
                self.pressed.push("escape");
                self.esc = 0;
                return self.feed(c);
            },
            (2, _) => {},
            (_, 0x1B) => {
                self.esc = 1;
                self.esc_num = 0;
                self.esc_mouse = false;
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
            (_, b'\t') => {
                self.pressed.push("tab");
                self.text.push('\t'); // in the text stream too: forms move on in order
            },
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
/// The pointer as last reported over the serial line (x, y, buttons), if any.
static mut SERIAL_MOUSE: Option<(i32, i32, u8)> = None;
/// The pointer's position at the last two presents, for `mouse_dx` / `mouse_dy`.
static mut MOUSE_LAST: (f64, f64) = (f64::NAN, f64::NAN);
static mut MOUSE_DELTA: (f64, f64) = (0.0, 0.0);

fn mouse_state() -> (f64, f64, u8) {
    if let Some((x, y, b)) = unsafe { *(&raw const SERIAL_MOUSE) } {
        return (x as f64, y as f64, b);
    }
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

pub(crate) fn path_arg(args: &[u64], i: usize) -> String {
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
                crate::proc::sched::yield_now();
                #[cfg(target_arch = "aarch64")]
                crate::arch::timer::delay_us(200);
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
        b"px_snap_save" => {
            if let Some(w) = win() {
                let snap = unsafe { &mut *(&raw mut SNAP) };
                snap.clear();
                snap.extend_from_slice(&w.buf);
            }
            UNIT
        },
        b"px_snap_load" => {
            let mut ok = false;
            if let Some(w) = win() {
                let snap = unsafe { &*(&raw const SNAP) };
                if snap.len() == w.buf.len() {
                    w.buf.copy_from_slice(snap);
                    ok = true;
                }
            }
            num(ok as i32 as f64)
        },
        b"px_hatch" => {
            px_hatch(arg);
            UNIT
        },
        b"px_bars" => {
            px_bars(args, arg);
            UNIT
        },
        b"px_notes" => {
            px_notes(args, arg);
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
        // pointer movement since the previous frame (the host's relative-mouse look / paddle)
        b"mouse_dx" => num(unsafe { MOUSE_DELTA.0 }),
        b"mouse_dy" => num(unsafe { MOUSE_DELTA.1 }),
        b"mouse_down" => boolean(mouse_state().2 & 1 != 0),
        b"mouse_down_right" => boolean(mouse_state().2 & 2 != 0),
        b"mouse_down_middle" => boolean(mouse_state().2 & 4 != 0),
        b"mouse_scroll" => num(unsafe { (*(&raw const INPUT)).wheel } as f64),
        b"mouse_scroll_x" => num(0.0),
        b"key_down" => boolean(key_down(&path_arg(args, 0))),
        // Sound (drivers::sfx): the host runtime's audio_* / music_* API.
        b"audio_sample_load" => {
            let path = path_arg(args, 0);
            match crate::vfs::read(&path).and_then(|d| crate::drivers::sfx::load_wav(&d)) {
                Some(id) => num(id as f64),
                None => num(-1.0),
            }
        },
        b"audio_sample_play" => {
            let f = |i: usize, d: f64| args.get(i).map(|&v| to_f64(v)).unwrap_or(d);
            let id = f(0, -1.0);
            if id < 0.0 {
                num(-1.0)
            } else {
                num(crate::drivers::sfx::play_at(id as usize, f(1, 0.0) as f32, f(2, 0.0) as f32, f(3, 0.0) as f32, f(4, 0.0) as f32, f(5, 1.0) as f32, f(6, 0.0) > 0.5).map(|v| v as f64).unwrap_or(-1.0))
            }
        },
        b"audio_sample_play_flat" => {
            let f = |i: usize, d: f64| args.get(i).map(|&v| to_f64(v)).unwrap_or(d);
            let id = f(0, -1.0);
            if id < 0.0 {
                num(-1.0)
            } else {
                num(crate::drivers::sfx::play(id as usize, f(1, 1.0) as f32, 0.0, f(2, 0.0) > 0.5, f(3, 1.0), false).map(|v| v as f64).unwrap_or(-1.0))
            }
        },
        b"audio_sample_stop" => {
            let v = args.first().map(|&v| to_f64(v)).unwrap_or(-1.0);
            if v >= 0.0 {
                crate::drivers::sfx::stop(v as usize);
            }
            UNIT
        },
        b"audio_sample_duration" => num(crate::drivers::sfx::duration(args.first().map(|&v| to_f64(v)).unwrap_or(-1.0).max(0.0) as usize)),
        b"audio_listener" => {
            let f = |i: usize, d: f64| args.get(i).map(|&v| to_f64(v)).unwrap_or(d);
            crate::drivers::sfx::set_listener(f(0, 1.0) as f32, f(1, 0.0) as f32);
            UNIT
        },
        b"audio_volume" => {
            crate::drivers::sfx::set_master(args.first().map(|&v| to_f64(v)).unwrap_or(1.0) as f32);
            UNIT
        },
        b"audio_sfx" => {
            // (x, y, z, w, freq, amp, dur, wave): a synthesised blip at a point.
            let f = |i: usize, d: f64| args.get(i).map(|&v| to_f64(v)).unwrap_or(d);
            let id = crate::drivers::sfx::synth(f(4, 440.0), f(5, 0.5), f(6, 0.1), f(7, 0.0) as u32);
            let _ = crate::drivers::sfx::play_at(id, f(0, 0.0) as f32, f(1, 0.0) as f32, f(2, 0.0) as f32, f(3, 0.0) as f32, 1.0, false);
            UNIT
        },
        b"ui_sound" => {
            let (freq, dur, wave) = match path_arg(args, 0).as_str() {
                "click" | "tick" => (1200.0, 0.03, 1),
                "error" | "deny" => (180.0, 0.18, 1),
                "open" | "rise" => (660.0, 0.12, 0),
                "close" | "fall" => (440.0, 0.12, 0),
                "pickup" | "coin" => (1568.0, 0.08, 3),
                _ => (880.0, 0.05, 0),
            };
            let id = crate::drivers::sfx::synth(freq, 0.35, dur, wave);
            let _ = crate::drivers::sfx::play(id, 1.0, 0.0, false, 1.0, false);
            UNIT
        },
        b"music_load" => {
            let path = path_arg(args, 0);
            match crate::vfs::read(&path).and_then(|d| crate::drivers::sfx::load_wav(&d)) {
                Some(id) => num(id as f64),
                None => num(-1.0),
            }
        },
        b"music_play" => {
            let id = args.first().map(|&v| to_f64(v)).unwrap_or(-1.0);
            if id >= 0.0 {
                let _ = crate::drivers::sfx::play(id as usize, 1.0, 0.0, true, 1.0, true);
            }
            UNIT
        },
        b"music_stop" => {
            crate::drivers::sfx::stop_music();
            UNIT
        },
        b"music_pause" => {
            crate::drivers::sfx::pause_music();
            UNIT
        },
        b"music_seek" => {
            crate::drivers::sfx::seek_music(args.first().map(|&v| to_f64(v)).unwrap_or(0.0));
            UNIT
        },
        b"music_pos" => num(crate::drivers::sfx::music_pos()),
        b"music_playing" => boolean(crate::drivers::sfx::music_playing()),
        b"music_volume" => {
            crate::drivers::sfx::set_music_volume(args.first().map(|&v| to_f64(v)).unwrap_or(1.0) as f32);
            UNIT
        },
        b"music_duration" => num(crate::drivers::sfx::duration(args.first().map(|&v| to_f64(v)).unwrap_or(-1.0).max(0.0) as usize)),
        b"music_fft" | b"music_fft_groups" | b"music_midi_bars" => list_from(Vec::new()),
        b"music_midi_load" | b"music_midi_count" => num(-1.0),
        // Gamepad (USB, the Pi): sticks / triggers -1..1 (triggers 0..1), buttons
        // A B X Y LB RB Back Start LS RS Guide .. d-pad 12..15, hat 0..7 / -1.
        b"pad_connected" => boolean(pad().connected),
        b"pad_axis" => {
            let i = args.first().map(|&v| to_f64(v)).unwrap_or(0.0).clamp(0.0, 7.0) as usize;
            num(pad().axes[i] as f64)
        },
        b"pad_button" => {
            let i = args.first().map(|&v| to_f64(v)).unwrap_or(0.0).clamp(0.0, 31.0) as u32;
            boolean((pad().buttons >> i) & 1 == 1)
        },
        b"pad_buttons" => num(pad().buttons as f64),
        b"pad_hat" => num(pad().hat as f64),
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
        // Picture handles (the PC decodes files into images it draws by id): not on the kernel yet.
        b"image_load" | b"image_w" | b"image_h" => num(-1.0),
        b"image_draw" | b"image_replace" | b"image_free" => UNIT,
        b"os_home_folders" => list_from(HOME_FOLDERS.iter().map(|f| mk_str(f)).collect()),
        b"os_user" => mk_str(&os_user()),
        b"os_apps" => list_from(crate::app_names().into_iter().map(mk_str).collect()),
        // The lingfu package manager: one flat row per catalog entry --
        // name, meta, description, installed, app, mentions LingOS, fits
        // this machine -- then installs by name (its last message back).
        b"lingfu_catalog" => {
            use crate::drivers::lingfu;
            lingfu::ensure_synced();
            let on_pi = cfg!(target_arch = "aarch64");
            let mut v = Vec::new();
            for (name, meta, desc) in lingfu::entries() {
                let (lingos, x86, pi) = lingfu::for_lingos(&desc);
                let here = lingos && if on_pi { pi } else { x86 };
                let inst = lingfu::is_installed(&name);
                let app = inst && lingfu::is_app(&name);
                let meta = if meta.bytes().all(|b| b.is_ascii_digit()) && !meta.is_empty() {
                    alloc::format!("{meta} downloads")
                } else {
                    meta
                };
                v.push(mk_str(&name));
                v.push(mk_str(&meta));
                v.push(mk_str(&desc));
                v.push(num(inst as i32 as f64));
                v.push(num(app as i32 as f64));
                v.push(num(lingos as i32 as f64));
                v.push(num(here as i32 as f64));
            }
            list_from(v)
        },
        b"lingfu_install" => {
            use crate::drivers::lingfu;
            let name = path_arg(args, 0);
            lingfu::begin_capture();
            let _ = lingfu::install(&name);
            let cap = lingfu::end_capture();
            let last = cap.split(|&b| b == b'\n').filter(|l| !l.is_empty()).last().unwrap_or(b"done");
            mk_str(&String::from_utf8_lossy(last))
        },
        b"lingfu_sync" => num(crate::drivers::lingfu::sync() as f64),
        b"os_packages" => list_from(crate::drivers::lingfu::installed_apps().iter().map(|s| mk_str(s)).collect()),
        b"os_platform" => mk_str(if cfg!(target_arch = "aarch64") { "LingOS rpi5" } else { "LingOS x86-64" }),
        b"os_app_run" => boolean(app_run(&path_arg(args, 0))),
        b"os_app_resume" => boolean(app_resume(&path_arg(args, 0))),
        b"os_apps_running" => list_from(apps_running().iter().map(|s| mk_str(s)).collect()),
        b"os_option" => {
            let v = args.get(1).map(|&v| crate::lingrt::truthy(v)).unwrap_or(false);
            match path_arg(args, 0).as_str() {
                "pause_audio" => unsafe { PAUSE_AUDIO = v },
                "perf" => unsafe { PERF = v },
                _ => {},
            }
            UNIT
        },
        b"os_net_get" => mk_str(&net_get()),
        b"os_net_test" => mk_str(&net_test()),
        b"os_net_set" => mk_str(&net_set(&path_arg(args, 0), &path_arg(args, 1), &path_arg(args, 2), &path_arg(args, 3), &path_arg(args, 4))),
        b"os_storage_ready" => boolean(crate::fs::lingfs::is_mounted()),
        b"os_users" => list_from(users_list().iter().map(|s| mk_str(s)).collect()),
        b"os_login" => boolean(user_login(&path_arg(args, 0), &path_arg(args, 1))),
        b"os_user_create" => boolean(user_create(&path_arg(args, 0), &path_arg(args, 1))),
        // Wi-Fi (crate::wifi): the desktop's network picker.
        b"wifi_available" => boolean(crate::wifi::available()),
        b"wifi_status" => mk_str(&crate::wifi::status()),
        b"wifi_ssid" => mk_str(&crate::wifi::ssid()),
        b"wifi_scan" => match crate::wifi::scan() {
            // "ssid\tsignal\tsecurity\tchannel" per network, strongest first;
            // one "!<reason>" row when the scan could not run.
            Ok(list) => list_from(
                list.iter()
                    .map(|n| mk_str(&alloc::format!("{}\t{}\t{}\t{}", n.ssid, n.rssi, ["open", "WEP", "WPA", "WPA2", "WPA3"][n.secure.min(4) as usize], n.channel)))
                    .collect(),
            ),
            Err(e) => list_from(alloc::vec![mk_str(&alloc::format!("!{e}"))]),
        },
        b"wifi_connect" => match crate::wifi::connect(&path_arg(args, 0), &path_arg(args, 1)) {
            Ok(m) => mk_str(&m),
            Err(e) => mk_str(&alloc::format!("!{e}")),
        },
        b"wifi_forget" => {
            crate::wifi::forget();
            UNIT
        },
        b"wifi_disconnect" => {
            crate::wifi::disconnect();
            UNIT
        },
        b"wifi_joined" => boolean(crate::wifi::joined()),
        b"wifi_signal" => num(crate::wifi::signal() as f64),
        // Messenger (drivers::messenger + the ling-messenger crate): the Pi's
        // Ling window drives the same LAN discovery, handshake and chats as
        // the x86 desktop's window.
        b"msg_start" => {
            crate::drivers::messenger::start();
            UNIT
        },
        b"msg_tick" => {
            // the window is in front: whatever arrived has been seen
            crate::drivers::messenger::clear_unread();
            crate::drivers::messenger::tick();
            UNIT
        },
        // chat lines received while the Messenger window was closed or parked
        b"msg_unread" => num(crate::drivers::messenger::unread() as f64),
        b"msg_me" => {
            let (h, id) = crate::drivers::messenger::me();
            mk_str(&alloc::format!("{h}\t{id}"))
        },
        b"msg_rows" => list_from(
            crate::drivers::messenger::rows()
                .iter()
                .map(|(f, i, l, o)| mk_str(&alloc::format!("{}\t{i}\t{l}\t{}", if *f { "friend" } else { "nearby" }, *o as u8)))
                .collect(),
        ),
        b"msg_selected" => num(crate::drivers::messenger::selected_row().map(|r| r as f64).unwrap_or(-1.0)),
        b"msg_select" => {
            crate::drivers::messenger::select_row(a(0, -1.0) as i64);
            UNIT
        },
        b"msg_activate" => {
            crate::drivers::messenger::activate_row(a(0, 0.0).max(0.0) as usize);
            UNIT
        },
        b"msg_lines" => list_from(
            crate::drivers::messenger::selected_lines()
                .iter()
                .map(|(me, t)| mk_str(&alloc::format!("{}\t{t}", if *me { "me" } else { "them" })))
                .collect(),
        ),
        b"msg_connected" => boolean(crate::drivers::messenger::selected_connected()),
        b"msg_submit" => {
            crate::drivers::messenger::submit(&path_arg(args, 0));
            UNIT
        },
        b"msg_status" => mk_str(crate::drivers::messenger::status()),
        b"os_uptime" => num(crate::arch::timer::now_ms() as f64 / 1000.0),
        // os_shell(cmd): run a console command and return what it printed (the
        // desktop's Terminal window). Commands that take the whole screen are
        // refused here; the desktop sends those to the text console instead.
        b"os_shell" => {
            let cmd = path_arg(args, 0);
            let c = cmd.trim();
            #[cfg(target_arch = "aarch64")]
            {
                if c == "setup" || c.starts_with("ling run") || c.starts_with("ling demo") || c == "sd format" || c == "trace" {
                    return Some(mk_str("(this command takes the whole screen: use the Console)"));
                }
                crate::console_capture_begin();
                crate::rpi_shell_exec(c.as_bytes());
                let out = crate::console_capture_end();
                mk_str(&String::from_utf8_lossy(out))
            }
            #[cfg(not(target_arch = "aarch64"))]
            {
                mk_str("(the Terminal window is the window manager's on this machine)")
            }
        },
        // the wall clock in seconds since 1970 UTC (0 = this board has none)
        b"os_time" => num(os_time() as f64),
        // display modes: the list this screen can show, the live one, a switch
        b"os_display_modes" => list_from(display_modes().iter().map(|&(w, h)| mk_str(&alloc::format!("{w}x{h}"))).collect()),
        b"os_display_current" => {
            let (w, h) = framebuffer::mode_dims();
            mk_str(&alloc::format!("{w}x{h}"))
        },
        b"os_display_set" => {
            let w = a(0, 0.0).max(0.0) as u32;
            let h = a(1, 0.0).max(0.0) as u32;
            let ok = display_set(w, h);
            boolean(ok)
        },
        // the keyring: lock on screen lock / log out, status for Settings
        b"os_lock" => {
            crate::drivers::messenger::lock_session();
            crate::keyring::lock();
            UNIT
        },
        b"os_keyring" => mk_str(&crate::keyring::status()),
        // the system clipboard (one text buffer shared by every app)
        b"clipboard_set" => {
            crate::drivers::clipboard::set_str(&path_arg(args, 0));
            UNIT
        },
        b"clipboard_get" => mk_str(core::str::from_utf8(crate::drivers::clipboard::get()).unwrap_or("")),
        // keyboard layouts: names, the active index, choose one
        b"os_kbd_layouts" => {
            let n = crate::drivers::kbdlayout::count();
            list_from((0..n).map(|i| mk_str(crate::drivers::kbdlayout::name(i))).collect())
        },
        b"os_kbd_layout" => num(crate::drivers::kbdlayout::current() as f64),
        b"os_kbd_layout_set" => {
            let i = args.first().map(|&v| to_f64(v)).unwrap_or(0.0).max(0.0) as usize;
            crate::drivers::kbdlayout::set_current(i);
            num(crate::drivers::kbdlayout::current() as f64)
        },
        b"os_reboot" => {
            #[cfg(target_arch = "aarch64")]
            crate::arch::power::reboot();
            #[cfg(target_arch = "x86_64")]
            crate::arch::power::reboot();
        },
        b"os_poweroff" => {
            #[cfg(target_arch = "aarch64")]
            crate::arch::power::poweroff();
            #[cfg(target_arch = "x86_64")]
            crate::arch::power::poweroff();
        },
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
        b"fs_delete" => num(crate::vfs::delete(&path_arg(args, 0)) as i32 as f64),
        b"fs_copy" => num(crate::vfs::copy(&path_arg(args, 0), &path_arg(args, 1)) as i32 as f64),
        b"fs_move" | b"fs_rename" => num(crate::vfs::rename(&path_arg(args, 0), &path_arg(args, 1)) as i32 as f64),
        b"fs_basename" => mk_str(&crate::vfs::basename(&path_arg(args, 0))),
        b"fs_reveal" => num(0.0),
        b"dlg_open" => mk_str(&file_dialog(Dlg::Open, &path_arg(args, 0), &exts_arg(args, 1), &path_arg(args, 2), "")),
        b"dlg_save" => mk_str(&file_dialog(Dlg::Save, &path_arg(args, 0), &exts_arg(args, 1), "", &path_arg(args, 2))),
        b"dlg_folder" => mk_str(&file_dialog(Dlg::Folder, &path_arg(args, 0), &[], &path_arg(args, 1), "")),

        _ => {
            let _ = str_of;
            return crate::drivers::g3d::dispatch(name, args);
        },
    })
}
