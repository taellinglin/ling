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
    /// Scaled window (`open_window(w, h, title, 2)`): the screen size the w×h
    /// buffer is aspect-fitted to at present(); None = buffer is the screen.
    screen: Option<(usize, usize)>,
}

/// present() scratch for scaled windows: the screen-sized frame.
static mut SCALE_BUF: Vec<u32> = Vec::new();
/// The screen-fit column map (source size, scaled width) -> source x per output column.
static mut XMAP: ((usize, usize), Vec<u32>) = ((0, 0), Vec::new());

/// Where a w×h buffer lands on an sw×sh screen, aspect kept: (x, y, dw, dh).
fn fit_rect(w: usize, h: usize, sw: usize, sh: usize) -> (usize, usize, usize, usize) {
    if w == 0 || h == 0 {
        return (0, 0, sw, sh);
    }
    if sw * h > sh * w {
        let dw = (sh * w / h).max(1);
        ((sw - dw) / 2, 0, dw, sh)
    } else {
        let dh = (sw * h / w).max(1);
        (0, (sh - dh) / 2, sw, dh)
    }
}

/// Screen-space pointer → the window buffer's coordinates.
fn to_buffer_coords(x: f64, y: f64) -> (f64, f64) {
    match win() {
        Some(w) if w.screen.is_some() && x > -1000.0 => {
            let (sw, sh) = w.screen.unwrap();
            let (ox, oy, dw, dh) = fit_rect(w.w, w.h, sw, sh);
            let bx = ((x - ox as f64) * w.w as f64 / dw as f64).clamp(0.0, (w.w - 1) as f64);
            let by = ((y - oy as f64) * w.h as f64 / dh as f64).clamp(0.0, (w.h - 1) as f64);
            (bx, by)
        },
        _ => (x, y),
    }
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
/// Was the left button down when the last frame was presented? (`ui_button`
/// clicks on the press edge, like the host's `mouse_was_down`.)
static mut MOUSE_WAS_DOWN: bool = false;
/// The left button as this frame's first `ui_button` saw it: every button in
/// a frame agrees, and exactly that value becomes `MOUSE_WAS_DOWN` at present
/// (sampling at present instead dropped a press landing mid-frame).
static mut FRAME_DOWN: Option<bool> = None;
static mut FONTS: Vec<Font> = Vec::new();
/// Per font: the font (and size factor) that draws the chars it has no glyph for (`font_fallback`).
static mut FONT_FB: Vec<Option<(usize, f32)>> = Vec::new();

fn win() -> Option<&'static mut Window> {
    // a service or isolate (coro.rs) has no window: the screen is the app's
    if crate::coro::in_coro() {
        return None;
    }
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
    // The signed distance only matters near the border. Each row has a middle
    // span where d <= e (filled: e = -0.5 -> coverage 1; outline: e = -1 ->
    // coverage 0): with qy fixed, d = max(qx, qy) - rad when qy <= 0, else
    // sqrt(max(qx,0)^2 + qy^2) - rad, so that span is |px| <= hx - rad + qmax
    // with qmax = rad + e (qy <= 0) or sqrt((rad + e)^2 - qy^2). A full-screen
    // panel was 1.7 ms of sqrt per call on the Z590; now it is a plain fill.
    let inner = if outline { rad - 1.0 } else { rad - 0.5 };
    let (cr, cg, cb) = (r.min(255), g.min(255), b.min(255));
    let solid = (cr << 16) | (cg << 8) | cb;
    for yy in y0..y1 {
        let py = (yy as f64 + 0.5) - ccy;
        let qy = libm::fabs(py) - (hy - rad);
        let (mut sx0, mut sx1) = (x1, x1); // [sx0, sx1): the middle span
        if qy <= inner {
            let qmax = if qy <= 0.0 { inner } else { libm::sqrt(inner * inner - qy * qy) };
            let lim = hx - rad + qmax - 1e-6;
            if lim >= 0.0 {
                sx0 = (libm::ceil(ccx - lim - 0.5) as i32).clamp(x0, x1);
                sx1 = ((libm::floor(ccx + lim - 0.5) as i32) + 1).clamp(sx0, x1);
            }
        }
        if !outline && sx0 < sx1 {
            let row = (yy * bw) as usize;
            if al >= 0.999 {
                w.buf[row + sx0 as usize..row + sx1 as usize].fill(solid);
            } else {
                for i in row + sx0 as usize..row + sx1 as usize {
                    w.buf[i] = blend(w.buf[i], cr, cg, cb, al);
                }
            }
        }
        for xx in (x0..sx0).chain(sx1..x1) {
            let px = (xx as f64 + 0.5) - ccx;
            let qx = libm::fabs(px) - (hx - rad);
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
/// (length, FNV-1a hash) of each loaded font's bytes: the same file under another path or in another
/// app (every studio app carries the same DaemonSans.ttf) is parsed once and shared.
static mut FONT_HASHES: BTreeMap<(usize, u64), usize> = BTreeMap::new();

fn fnv64(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Font handles are per program: each app counts its fonts from 0 in the
/// order it loads them (apps written for the desktop runtime rely on that --
/// Ling Wallet keeps its fonts in slots 0..3), mapped onto the shared,
/// deduplicated font table.
static mut FONT_MAPS: BTreeMap<String, Vec<usize>> = BTreeMap::new();

fn font_map() -> &'static mut Vec<usize> {
    let maps = unsafe { &mut *(&raw mut FONT_MAPS) };
    maps.entry(active_app_name()).or_default()
}

/// A program's font handle -> the shared table's index (-1: none).
fn gfont(h: f64) -> f64 {
    if h < 0.0 {
        return -1.0;
    }
    font_map().get(h as usize).map(|&g| g as f64).unwrap_or(-1.0)
}

/// `args` with the font handles at `at` mapped to the shared table.
fn map_font_args(args: &[u64], at: &[usize]) -> Vec<u64> {
    let mut v = args.to_vec();
    for &i in at {
        if let Some(x) = v.get_mut(i) {
            *x = num(gfont(to_f64(*x)));
        }
    }
    v
}

/// Fonts `font_load` could not find (each is reported once).
static mut FONT_MISSING: Vec<String> = Vec::new();

/// Load a font for the running program; its handle in that program. A font file that is not there
/// (not copied to the card, say) is reported on the console once, and the system UI font stands in so
/// the program's text still shows instead of vanishing.
fn font_load(path: &str) -> f64 {
    let mut g = font_load_global(path);
    if g < 0.0 {
        let miss = unsafe { &mut *(&raw mut FONT_MISSING) };
        if !miss.iter().any(|p| p == path) {
            miss.push(String::from(path));
            crate::console_write(alloc::format!("font_load: {path} is missing or unreadable -- drawing with the system UI font\n").as_bytes());
        }
        match ui_font() {
            Some(u) => g = u as f64,
            None => return -1.0,
        }
    }
    let m = font_map();
    let g = g as usize;
    if let Some(i) = m.iter().position(|&x| x == g) {
        return i as f64;
    }
    m.push(g);
    (m.len() - 1) as f64
}

fn font_load_global(path: &str) -> f64 {
    let paths = unsafe { &mut *(&raw mut FONT_PATHS) };
    // keyed by the resolved path: "assets/font.ttf" is a different file in each app
    let key = crate::vfs::resolve(path);
    if let Some(&i) = paths.get(&key) {
        return i as f64;
    }
    // a host-absolute path (C:/.../fonts/X.ttf, /usr/share/...) means nothing
    // here: look for the same file name with the program's own files instead
    let bytes = crate::vfs::read(path).or_else(|| {
        let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
        if base == path {
            return None;
        }
        crate::vfs::read(&alloc::format!("assets/fonts/{base}")).or_else(|| crate::vfs::read(base))
    });
    let Some(bytes) = bytes else { return -1.0 };
    let hashes = unsafe { &mut *(&raw mut FONT_HASHES) };
    let h = (bytes.len(), fnv64(&bytes));
    if let Some(&i) = hashes.get(&h) {
        paths.insert(key, i);
        return i as f64;
    }
    match fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()) {
        Ok(fd) => {
            let f = fonts();
            f.push(Font { fd, cache: BTreeMap::new() });
            paths.insert(key, f.len() - 1);
            hashes.insert(h, f.len() - 1);
            (f.len() - 1) as f64
        },
        Err(_) => -1.0,
    }
}

// ── Unicode in the kernel's own text (desktop, terminal, editor, 8x8 apps) ──
// The 8x8 bitmap font only has ASCII. Everything else is drawn from the UI font: DaemonSans.ttf from
// the image (any app's copy: they are one file) or from storage (system/fonts/DaemonSans.ttf).

static mut UI_FONT: i64 = -2; // -2 = not looked for yet, -1 = none

/// The font index of the UI font, loaded the first time non-ASCII text is drawn.
pub(crate) fn ui_font() -> Option<usize> {
    unsafe {
        if UI_FONT == -2 {
            UI_FONT = -1;
            let mut found: Option<String> = None;
            crate::appfs::each_file(|n| {
                if found.is_none() && n.ends_with("fonts/DaemonSans.ttf") {
                    found = Some(String::from(n));
                }
            });
            let mut cands: Vec<String> = Vec::new();
            if let Some(p) = found {
                cands.push(p);
            }
            cands.push(String::from("system/fonts/DaemonSans.ttf"));
            cands.push(String::from("fonts/DaemonSans.ttf"));
            for p in &cands {
                let id = font_load_global(p);
                if id >= 0.0 {
                    UI_FONT = id as i64;
                    break;
                }
            }
        }
        if UI_FONT >= 0 { Some(UI_FONT as usize) } else { None }
    }
}

/// Characters that are drawn two cells wide (CJK, Hangul, kana, full-width forms).
pub(crate) fn wide_char(c: char) -> bool {
    let u = c as u32;
    (0x1100..=0x115F).contains(&u)
        || (0x2E80..=0xA4CF).contains(&u)
        || (0xAC00..=0xD7A3).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || (0xFE30..=0xFE4F).contains(&u)
        || (0xFF00..=0xFF60).contains(&u)
        || (0xFFE0..=0xFFE6).contains(&u)
        || (0x20000..=0x3FFFD).contains(&u)
}

/// Is `c` a zero-width mark in the UI font (it goes onto the character before it)?
#[allow(dead_code)]
pub(crate) fn ui_is_mark(c: char) -> bool {
    match ui_font() {
        Some(fid) => fonts()[fid].fd.lookup_glyph_index(c) != 0 && fonts()[fid].fd.metrics(c, 16.0).advance_width < 0.5,
        None => false,
    }
}

/// Draw the non-ASCII characters of a UTF-8 `row` laid out in fixed cells (`cell_w` x `cell_h`
/// px; byte `k` sits in cell `k`, as the 8x8 code lays text out). A character that takes L bytes
/// owns L cells and is drawn into the first one, or the first two for wide scripts, so text never
/// runs past where the byte layout ends; zero-width marks go onto the previous character.
/// `put(dx, dy, coverage)` paints one pixel relative to the row's top-left; ASCII is left alone.
/// Returns false when there is no UI font (the caller's ASCII-only drawing stands).
pub(crate) fn utf8_cells_overlay(row: &[u8], cell_w: i32, cell_h: i32, max_cells: usize, mut put: impl FnMut(i32, i32, u8)) -> bool {
    if row.iter().all(|&b| b < 0x80) {
        return true;
    }
    let Some(fid) = ui_font() else { return false };
    let Ok(s) = core::str::from_utf8(row) else {
        // a cut multi-byte char at the end (a full terminal line): draw what decodes
        let good = core::str::from_utf8(row).err().map(|e| e.valid_up_to()).unwrap_or(0);
        return utf8_cells_overlay(&row[..good], cell_w, cell_h, max_cells, put);
    };
    let f = &mut fonts()[fid];
    let mut last_x = 0i32;
    for (k, c) in s.char_indices() {
        if (c as u32) < 0x80 || k >= max_cells {
            continue;
        }
        let wide = wide_char(c);
        let cells = if wide { 2.min(c.len_utf8()) } else { 1 } as i32;
        // narrow text fits the cell height; wide glyphs need 16 px to stay legible
        // narrow text at the 8x8 font's letter size (its x-height), wide glyphs at 16 px or more
        let px = if wide { (cell_h.max(16) as f32) * 0.95 } else { cell_h as f32 * 1.25 };
        if f.fd.lookup_glyph_index(c) == 0 {
            continue;
        }
        let adv = f.fd.metrics(c, px).advance_width;
        let gl = f.cache.entry((c, px.to_bits())).or_insert_with(|| {
            let (m, cov) = f.fd.rasterize(c, px);
            Glyph { w: m.width, h: m.height, xmin: m.xmin, ymin: m.ymin, cov }
        });
        let span = cells * cell_w;
        let cx = if adv < 0.5 {
            last_x // a combining mark: onto the character before it
        } else {
            k as i32 * cell_w + ((span as f32 - adv) / 2.0).max(0.0) as i32
        };
        if adv >= 0.5 {
            last_x = k as i32 * cell_w + ((span as f32 - adv) / 2.0).max(0.0) as i32;
        }
        let baseline = if wide { (cell_h as f32 * 0.78) as i32 + if cell_h < 16 { (16 - cell_h) / 2 } else { 0 } } else { (cell_h as f32 * 0.88) as i32 };
        let ox = cx + gl.xmin;
        let oy = baseline - gl.ymin - gl.h as i32;
        for gy in 0..gl.h {
            for gx in 0..gl.w {
                let cv = gl.cov[gy * gl.w + gx];
                if cv != 0 {
                    put(ox + gx as i32, oy + gy as i32, cv);
                }
            }
        }
    }
    true
}

fn font(id: f64) -> Option<&'static mut Font> {
    if id < 0.0 {
        return None;
    }
    fonts().get_mut(id as usize)
}

fn font_fb() -> &'static mut Vec<Option<(usize, f32)>> {
    unsafe { &mut *(&raw mut FONT_FB) }
}

/// `font_fallback(primary, fallback [, scale])`: the chars `primary` has no glyph for are drawn
/// and measured with `fallback` at `scale` times the size; fallback -1 removes it.
fn font_fallback(id: f64, fb: f64, scale: f64) -> f64 {
    let n = fonts().len();
    if id < 0.0 || id as usize >= n {
        return -1.0;
    }
    let fbs = font_fb();
    if fbs.len() < n {
        fbs.resize(n, None);
    }
    let i = id as usize;
    fbs[i] = if fb >= 0.0 && (fb as usize) < n && fb as usize != i {
        Some((fb as usize, if scale > 0.0 { scale as f32 } else { 1.0 }))
    } else {
        None
    };
    id
}

/// The font that draws `c` for font `id`, and the size factor: `id` when it has the glyph (or
/// none down its fallback chain does), else the first fallback that has it.
fn pick(id: usize, c: char) -> (usize, f32) {
    let fbs = font_fb();
    if c.is_whitespace() || c.is_control() || fbs.get(id).map_or(true, |f| f.is_none()) {
        return (id, 1.0);
    }
    let fs = fonts();
    let (mut cur, mut scale) = (id, 1.0f32);
    for _ in 0..6 {
        if fs[cur].fd.lookup_glyph_index(c) != 0 {
            return (cur, scale);
        }
        match fbs.get(cur).copied().flatten() {
            Some((fb, s)) if fb < fs.len() && fb != cur => {
                cur = fb;
                scale *= s;
            },
            _ => break,
        }
    }
    (id, 1.0)
}

/// Width of `text` at `px` for font `id`: the sum of glyph advances (the host's `measure`),
/// each char measured in the font that draws it.
fn measure(id: usize, text: &str, px: f32) -> f32 {
    let fs = fonts();
    text.chars()
        .map(|c| {
            let (fid, s) = pick(id, c);
            fs[fid].fd.metrics(c, px * s).advance_width
        })
        .sum()
}

fn ascent(f: &Font, px: f32) -> f32 {
    f.fd.horizontal_line_metrics(px).map(|m| m.ascent).unwrap_or(px * 0.8)
}

fn px_text(args: &[u64]) {
    let a = |i: usize| args.get(i).map(|&v| to_f64(v)).unwrap_or(0.0);
    let (x, y, px) = (a(1) as f32, a(2) as f32, a(3) as f32);
    let Some(w) = win() else { return };
    let Some(f) = font(a(0)) else { return };
    let fid0 = a(0) as usize;
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
    let crisp = unsafe { TEXT_CRISP };
    let mut pen = 0.0f32;
    for c in text.chars() {
        // the char's own font: this one, or the fallback that has the glyph
        let (fid, s) = pick(fid0, c);
        let gf = &mut fonts()[fid];
        let gpx = px * s;
        let adv = gf.fd.metrics(c, gpx).advance_width;
        let gx0 = libm::floorf(x + pen) as i32;
        pen += adv;
        if gx0 > cx1 {
            break;
        }
        let gl = gf.cache.entry((c, gpx.to_bits())).or_insert_with(|| {
            crate::prof::count(crate::prof::C_GLYPHS, 1);
            let (m, cov) = gf.fd.rasterize(c, gpx);
            Glyph { w: m.width, h: m.height, xmin: m.xmin, ymin: m.ymin, cov }
        });
        crate::prof::count(crate::prof::C_CHARS, 1);
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
                if cv == 0 || (crisp && cv < 128) {
                    continue;
                }
                let i = row + xx as usize;
                w.buf[i] = if cv == 255 || crisp { col } else { blend(w.buf[i], r, g, b, cv as f32 / 255.0) };
            }
        }
    }
}

/// `px_fit(font, size, max_w, text)`: `text`, or the longest prefix + ".." that fits.
fn px_fit(args: &[u64]) -> u64 {
    let a = |i: usize| args.get(i).map(|&v| to_f64(v)).unwrap_or(0.0);
    let text = args.get(3).map(|&v| to_string(v)).unwrap_or_default();
    let (px, maxw) = (a(1) as f32, a(2) as f32);
    if font(a(0)).is_none() {
        return mk_str(&text);
    }
    let fid = a(0) as usize;
    if measure(fid, &text, px) <= maxw {
        return mk_str(&text);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut end = chars.len();
    while end > 0 {
        let mut cand: String = chars[..end].iter().collect();
        cand.push_str("..");
        if measure(fid, &cand, px) <= maxw {
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
    ui_end();
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

fn open_window(req_w: f64, req_h: f64, mode: f64) {
    // LingOS: the window is the whole screen (the safe area); fall back to the
    // requested size when there's no framebuffer, so the program still runs.
    // Mode 2 (scaled): keep the requested buffer and aspect-fit it to the screen.
    let scaled = mode > 1.5 && framebuffer::available() && req_w >= 1.0 && req_h >= 1.0;
    let (w, h, screen) = if scaled {
        let sc = (framebuffer::width() as usize, framebuffer::height() as usize);
        (req_w as usize, req_h as usize, Some(sc))
    } else if framebuffer::available() {
        (framebuffer::width() as usize, framebuffer::height() as usize, None)
    } else {
        (req_w.max(1.0) as usize, req_h.max(1.0) as usize, None)
    };
    ui_reset();
    unsafe {
        (*(&raw mut CURSOR_HIDE_APP)).clear();
        core::ptr::write(
            &raw mut WIN,
            Some(Window { buf: vec![0u32; w * h], w, h, open: true, clip: None, black: false, screen }),
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
/// Bring up a built-in app: back into it if it is parked (Alt+Tab, the idle
/// lock), else start it. Returns when it parks or quits. (The x86 desktop's
/// Applications menu.)
pub fn launch(name: &str) -> bool {
    if apps_running().iter().any(|a| a == name) {
        return app_resume(name);
    }
    app_run(name)
}

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
    // Programs/<App>/args: the app's command line (e.g. Ling Wallet's `--peer host:port`)
    let args: Vec<String> = crate::vfs::read(&alloc::format!("{programs}/args"))
        .map(|b| String::from_utf8_lossy(&b).split_whitespace().map(String::from).collect())
        .unwrap_or_default();
    if !args.is_empty() {
        crate::console_write(alloc::format!("app: {name} args: {}\n", args.join(" ")).as_bytes());
    }
    crate::lingrt::set_next_args(args);
    crate::console_write(b"app: starting ");
    crate::console_write(name.as_bytes());
    crate::console_write(b"\n");
    let sp = unsafe { fresh_stack(top) };
    unsafe { PEN_ALPHA = 1.0 };
    unsafe {
        *(&raw mut ACTIVE_APP) = Some(AppCtx { name: String::from(name), stack, top, sp, win: None, dot: false, scope: None, park: 0, buf_mode: false, buf: String::new(), done: false, esc_parks: false });
        *(&raw mut PENDING_ENTRY) = Some(entry);
        // Runs the app on its stack until it suspends or returns.
        ling_switch_stack(&raw mut HOST_SP, sp);
    }
    app_came_back(host);
    unsafe { PEN_ALPHA = 1.0 };
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
    /// The app's text-field buffer state while suspended: whole-field mode
    /// (`text_set`) and the buffer. Saved on suspend, restored on resume, so a
    /// text box left open across Alt+Tab keeps working -- and so the mode never
    /// leaks onto the desktop, whose boxes want the typed-since-last behavior.
    buf_mode: bool,
    buf: String,
    done: bool,
    /// `os_option("esc_minimizes", 1)`: an Esc the app does not use parks it
    /// (like Minimize) instead of closing it.
    esc_parks: bool,
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

/// The app parked most recently (the one the idle lock just took off screen).
pub fn last_parked() -> Option<String> {
    parked_apps().last().map(|a| String::from(a.name.as_str()))
}

/// Is this built-in app running (on screen or parked)?
pub fn is_running(name: &str) -> bool {
    apps_running().iter().any(|a| a == name)
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
    // Stale key/composition state must not leak across a session boundary
    // (sign-in -> run app -> log out): a USB key-up report lost during the
    // teardown window would otherwise leave `held` marking a usage down, and a
    // half-finished input-method composition would survive. Both are cleared
    // here, where every app launch/return/suspend already passes through.
    input.held = [0; 8];
    input.prev_held = [0; 8];
    crate::drivers::ime::reset();
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
    // The desktop's text boxes want the typed-since-last behavior: drop any
    // whole-field (`text_set`) buffer an app left on, so one keypress can't
    // repeat into the sign-in password box after running an app (the asterisk
    // flood). The next app re-arms its own mode.
    unsafe {
        let input = &mut *(&raw mut INPUT);
        input.buf_mode = false;
        input.buf.clear();
    }
    if let Some(c) = ctx {
        crate::console_write(b"app: ");
        crate::console_write(c.name.as_bytes());
        if c.done {
            crate::console_write(b" returned\n");
            // its isolates and workers end with it
            crate::coro::kill_owner(&c.name);
            crate::mm::frame::free_frames(c.stack, APP_STACK_ORDER);
            // and LingAnime's paint document with it
            crate::drivers::artpaint::app_ended(&c.name);
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
    unsafe { PEN_ALPHA = 1.0 };
    true
}

/// Inside an app, from `present`: park it and give the screen back to the
/// desktop; continues here when the desktop resumes it.
fn app_suspend() {
    let Some(a) = active_app() else { return };
    // Snapshot the app's last frame for its taskbar tile, before the window is
    // taken aside. Its whole buffer scales into a 124x64 thumbnail.
    let aname = a.name.clone();
    let dims = win().map(|w| (w.w, w.h));
    if let Some((ww, wh)) = dims {
        thumb_store(alloc::format!("app:{aname}"), 0, 0, ww, wh, 124, 64);
    }
    a.win = unsafe { core::ptr::replace(&raw mut WIN, None) };
    a.dot = unsafe { CURSOR_DOT };
    a.scope = Some(crate::vfs::take_scope());
    a.park = crate::lingrt::park_scope();
    // Keep the app's own text-field buffer; the desktop gets a clean slate.
    let input = unsafe { &mut *(&raw mut INPUT) };
    a.buf_mode = input.buf_mode;
    a.buf = core::mem::take(&mut input.buf);
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
        // Put the app's text-field buffer back (a box left open stays usable).
        let input = unsafe { &mut *(&raw mut INPUT) };
        input.buf_mode = a.buf_mode;
        input.buf = core::mem::take(&mut a.buf);
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

/// The app that has the screen ("desktop" when none does).
pub fn active_app_name() -> String {
    match active_app() {
        Some(a) => a.name.clone(),
        None => String::from("desktop"),
    }
}

pub fn apps_running() -> Vec<String> {
    parked_apps().iter().map(|a| a.name.clone()).collect()
}

/// End a suspended app (Task Manager). Mirrors the `app_came_back` done-path:
/// its isolates/workers die, its parked Ling scope and 2 MiB stack are freed.
/// Only ever a *parked* app (never the one with the screen), so it is not
/// executing while we tear it down.
fn app_kill(name: &str) -> bool {
    let Some(i) = parked_apps().iter().position(|a| a.name == name) else { return false };
    let c = parked_apps().remove(i);
    crate::console_write(b"app: ended ");
    crate::console_write(c.name.as_bytes());
    crate::console_write(b" (task manager)\n");
    crate::coro::kill_owner(&c.name);
    crate::lingrt::drop_park(c.park);
    crate::mm::frame::free_frames(c.stack, APP_STACK_ORDER);
    true
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
    // two-factor accounts: the password alone is not enough (`user_login_code`)
    if crate::fs::twofa::enabled(name) {
        crate::fs::users::set_last_error(crate::fs::users::TWOFA_NEEDED);
        return false;
    }
    user_session(name, password)
}

/// Sign in with the password and the authenticator code (or a recovery code).
fn user_login_code(name: &str, password: &str, code: &str) -> bool {
    if !crate::fs::users::verify(name, password) {
        return false;
    }
    if crate::fs::twofa::enabled(name) && !crate::fs::twofa::check(name, code) {
        if crate::fs::twofa::locked_for_ms(name) > 0 {
            crate::fs::users::set_last_error("too many wrong codes -- wait a little, then use the newest code");
        } else if crate::fs::twofa::last_was_replay() {
            crate::fs::users::set_last_error("that code was just used -- wait for the next one (a new code every 30 s)");
        } else {
            crate::fs::users::set_last_error(crate::fs::users::TWOFA_WRONG);
        }
        return false;
    }
    user_session(name, password)
}

/// The signed-in part of a login (the password is known to be right).
fn user_session(name: &str, password: &str) -> bool {
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

/// Sign in while a password reset is pending: the old password still works
/// (and cancels the reset); anything else becomes the new password.
fn user_reset(name: &str, password: &str) -> bool {
    use crate::fs::users;
    if password.chars().count() < 4 {
        users::set_last_error("the new password needs at least 4 characters");
        return false;
    }
    if !crate::fs::lingfs::is_mounted() {
        users::set_last_error("the account storage is not mounted");
        return false;
    }
    if !users::reset_pending(name) {
        return user_login(name, password);
    }
    let all = users_list();
    if !all.iter().any(|u| u == name) {
        // a reset for an account that is not on this card: make it with this password
        if !user_create(name, password) {
            if !matches!(users::last_verify_error(), users::HASH_UNSTABLE | users::RECORD_READBACK) {
                users::set_last_error("could not create the account (storage write failed)");
            }
            return false;
        }
        return true;
    }
    if !users::verify(name, password) {
        if !users::set_password(name, password) {
            if !matches!(users::last_verify_error(), users::HASH_UNSTABLE | users::RECORD_READBACK) {
                users::set_last_error("could not save the new password (storage write failed)");
            }
            return false;
        }
        crate::console_write(b"users: new password set for ");
        crate::console_write(name.as_bytes());
        crate::console_write(b" (reset from the boot partition)\n");
    }
    users::clear_reset(name, &all);
    if !user_login(name, password) {
        // a two-factor account still needs its code: the sign-in screen asks next
        if users::last_verify_error() != users::TWOFA_NEEDED {
            users::set_last_error("the new password was saved but did not check out -- try signing in with it");
        }
        return false;
    }
    true
}

fn user_create(name: &str, password: &str) -> bool {
    if name.is_empty() || password.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return false;
    }
    if !crate::fs::users::create(name, password, "", "", "", "", "users") {
        return false;
    }
    // a pending reset never covers an account that just chose its password
    if crate::fs::users::reset_pending(name) {
        let all = users_list();
        crate::fs::users::clear_reset(name, &all);
    }
    // just created with this password: no second hash needed to sign in
    user_session(name, password)
}

/// The home folder name of a bundled app (`Projects/<App>`, `Programs/<App>`).
pub fn app_folder(name: &str) -> String {
    String::from(match name {
        "lingstation" => "LingStationX",
        "lingdev" => "LingDev",
        "lingmodeler" => "LingModeler",
        "horizon" => "Horizon",
        "wallet" => "LingWallet",
        "lingforfun" => "LingForFun",
        "linganime" => "LingAnime",
        n => n,
    })
}

/// Give a console-launched app the same folder environment a desktop launch
/// gives it: its image folder (`apps/<name>`, so `font_load("assets/...")` and
/// other relative reads find its bundled files) and its home save dirs
/// (`Programs/<App>` as the data root, `Projects/<App>`). Without this a
/// built-in app started by typing its name at the console couldn't find its
/// own fonts/assets. `console_app_leave` puts the console's roots back.
pub fn console_app_enter(name: &str) {
    let folder = app_folder(name);
    if crate::vfs::ready() {
        let home = crate::vfs::home();
        let programs = alloc::format!("{home}/Programs/{folder}");
        let projects = alloc::format!("{home}/Projects/{folder}");
        crate::vfs::mkdir(&programs);
        crate::vfs::mkdir(&projects);
        crate::vfs::set_root(&programs);
        crate::vfs::set_projects_root(&projects);
    }
    crate::vfs::set_image_root(&alloc::format!("apps/{name}"));
}

/// Restore the console's default roots after a console-launched app returns.
pub fn console_app_leave() {
    crate::vfs::set_root("");
    crate::vfs::set_projects_root("");
    crate::vfs::set_image_root("");
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
    // non-ASCII: drawn from the UI font into the byte cells (the 8x8 pass below leaves them blank)
    let (r8, g8, b8) = ((color >> 16) & 0xFF, (color >> 8) & 0xFF, color & 0xFF);
    let ww = w.w;
    let buf = &mut w.buf;
    utf8_cells_overlay(text.as_bytes(), 8 * sc as i32, 8 * sc as i32, usize::MAX, |dx, dy, cv| {
        let (x, y) = (x0 + dx as i64, y0 + dy as i64);
        if x >= cx0 && x < cx1 && y >= cy0 && y < cy1 {
            let i = y as usize * ww + x as usize;
            buf[i] = if cv == 255 { color } else { blend(buf[i], r8, g8, b8, cv as f32 / 255.0) };
        }
    });
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

#[cfg(target_arch = "x86_64")]
use crate::drivers::wallpaper::LING_COUNTRY_BMP as WALL_BMP;

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
        *cache = wall_scaled(w.w, w.h).map(|v| (w.w, w.h, v));
    }
    match cache {
        Some((_, _, px)) if px.len() == w.buf.len() => {
            // 8 MB at 1080p, every frame: in bands over the cores
            let (chunks, per) = crate::par::row_chunks(w.h, 32);
            let dst = crate::par::Shared(w.buf.as_mut_ptr());
            let (src, ww, total) = (&px[..], w.w, w.buf.len());
            crate::par::for_each(chunks, &|c: usize| {
                let (a, b) = ((c * per * ww).min(total), ((c + 1) * per * ww).min(total));
                unsafe { core::ptr::copy_nonoverlapping(src.as_ptr().add(a), dst.get().add(a), b - a) };
            });
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
const EMB_ARCHIVE: [u8; 8] = [0x7E, 0x18, 0x18, 0x18, 0x18, 0x18, 0x7E, 0x00];

pub(crate) fn gfx_color() -> u32 {
    unsafe { GFX_COLOR }
}

/// The pen's `set_alpha` opacity (0..1) -- 3-D triangles take it too.
pub(crate) fn pen_alpha() -> f32 {
    unsafe { PEN_ALPHA }
}

/// The pointer in the app window's pixels (`mouse_ray`).
pub(crate) fn app_mouse_xy() -> (f64, f64) {
    let (x, y, _) = app_mouse_state();
    (x, y)
}

/// The app window's back buffer (`0xRRGGBB`), width and height -- the
/// native 3-D renderer (`drivers::g3d`) draws into it directly.
pub(crate) fn surface() -> Option<(&'static mut [u32], usize, usize)> {
    let w = win()?;
    let (ww, hh) = (w.w, w.h);
    Some((&mut w.buf[..], ww, hh))
}

/// The app window's `px_clip` rectangle [x0, y0, x1, y1) (the whole window when unclipped) -- for
/// `drivers::artpaint`, whose drawing calls honour it like the host's.
pub(crate) fn surface_clip() -> [i32; 4] {
    match win() {
        Some(w) => {
            let (x0, y0, x1, y1) = clip_box(w);
            [x0, y0, x1, y1]
        },
        None => [0, 0, 0, 0],
    }
}

/// A loaded font's face (`font_load` id), for glyph textures.
#[allow(dead_code)]
pub(crate) fn font_fd(id: f64) -> Option<&'static fontdue::Font> {
    font(id).map(|f| &f.fd)
}

/// The face that draws `c` for font `id` (its own, or a `font_fallback` that has the glyph).
pub(crate) fn font_fd_for(id: f64, c: char) -> Option<&'static fontdue::Font> {
    font(id)?;
    let (fid, _) = pick(id as usize, c);
    fonts().get(fid).map(|f| &f.fd)
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
        let i = y as usize * w.w + x as usize;
        w.buf[i] = pen(w.buf[i], c);
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
        let i = row + x as usize;
        w.buf[i] = pen(w.buf[i], c);
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

/// `set_alpha(a)`: the pen's opacity for the gfx-style drawing calls (the host
/// gfx state's `alpha`); 1 = opaque, the default.
static mut PEN_ALPHA: f32 = 1.0;

/// `c` laid over `dst` at the pen's opacity.
#[inline]
fn pen(dst: u32, c: u32) -> u32 {
    let a = unsafe { PEN_ALPHA };
    if a >= 0.999 {
        c
    } else {
        blend(dst, (c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff, a)
    }
}

fn put(w: &mut Window, x: i64, y: i64, c: u32) {
    if x >= 0 && y >= 0 && (x as usize) < w.w && (y as usize) < w.h {
        let i = y as usize * w.w + x as usize;
        w.buf[i] = pen(w.buf[i], c);
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
    if ext(".bmp") || ext(".png") || ext(".jpg") || ext(".jpeg") || ext(".webp") {
        (0x5AC06A, Some(&EMB_PHOTO))
    } else if ext(".svg") || ext(".svgz") {
        (0xB06AE0, Some(&EMB_GRAPHICS))
    } else if ext(".wav") || ext(".mp3") || ext(".ogg") || ext(".flac") || ext(".mid") || ext(".m4a") || ext(".aac") {
        (0xE0A44A, Some(&EMB_MUSIC))
    } else if ext(".mp4") || ext(".mkv") || ext(".webm") || ext(".avi") || ext(".mov") || ext(".gif") {
        (0xE0605A, Some(&EMB_VIDEO))
    } else if ext(".zip") || ext(".tar") || ext(".gz") || ext(".tgz") || ext(".7z") || ext(".rar") || ext(".xz") || ext(".bz2") || ext(".zst") {
        (0xC08A4A, Some(&EMB_ARCHIVE))
    } else if ext(".obj") || ext(".stl") || ext(".glb") || ext(".gltf") || ext(".ply") || ext(".ltm") || ext(".lmesh") {
        (0x40C0C0, Some(&EMB_MODEL))
    } else if ext(".pdf") {
        (0xE05A5A, Some(&EMB_DOC))
    } else if ext(".ttf") || ext(".otf") || ext(".woff") || ext(".woff2") {
        (0xB06AE0, Some(&EMB_DOC))
    } else if ext(".ling") {
        (0xF0D050, Some(&EMB_SCRIPT))
    } else if ext(".py") || ext(".js") || ext(".ts") || ext(".rs") || ext(".c") || ext(".h") || ext(".cpp") || ext(".cc") || ext(".go") || ext(".java") || ext(".rb") || ext(".lua") || ext(".sh") || ext(".html") || ext(".css") || ext(".php") || ext(".sql") {
        (0x6AA8E0, Some(&EMB_SCRIPT))
    } else if ext(".exe") || ext(".elf") || ext(".o") || ext(".so") || ext(".dll") || ext(".bin") || ext(".app") || ext(".wasm") {
        (0x8A9BB8, Some(&EMB_PROGRAM))
    } else if ext(".conf") || ext(".cfg") || ext(".ini") || ext(".yaml") || ext(".yml") || ext(".xml") || ext(".env") || ext(".lock") {
        (0xA0A0B0, Some(&EMB_SCRIPT))
    } else if ext(".txt") || ext(".md") || ext(".json") || ext(".toml") || ext(".csv") || ext(".log") || !l.contains('.') {
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

/// Live window thumbnails for the taskbar: key -> (w, h, downscaled pixels).
static mut THUMBS: BTreeMap<String, (usize, usize, Vec<u32>)> = BTreeMap::new();

/// Store a `tw`x`th` nearest-neighbour downscale of the current window buffer
/// region (sx,sy,sw,sh) under `key`. Shared by the `os_thumb_capture` builtin
/// (the desktop snapshots each window right after drawing it) and `app_suspend`
/// (a full-screen app snapshots its last frame as it parks).
pub(crate) fn thumb_store(key: String, sx: usize, sy: usize, sw: usize, sh: usize, tw: usize, th: usize) {
    let Some(w) = win() else { return };
    let (sw, sh) = (sw.max(1), sh.max(1));
    let (tw, th) = (tw.clamp(1, 320), th.clamp(1, 320));
    let mut out = alloc::vec![0u32; tw * th];
    for ty in 0..th {
        let srcy = sy + ty * sh / th;
        if srcy >= w.h {
            continue;
        }
        let row = srcy * w.w;
        for tx in 0..tw {
            let srcx = sx + tx * sw / tw;
            if srcx < w.w {
                out[ty * tw + tx] = w.buf[row + srcx];
            }
        }
    }
    unsafe { (*(&raw mut THUMBS)).insert(key, (tw, th, out)) };
}

/// `os_thumb_capture(key, sx, sy, sw, sh, tw, th)` -> taskbar thumbnail.
fn os_thumb_capture(args: &[u64]) {
    let key = args.first().map(|&v| to_string(v)).unwrap_or_default();
    let n = |i: usize| args.get(i).map(|&v| to_f64(v)).unwrap_or(0.0);
    thumb_store(
        key,
        n(1).max(0.0) as usize,
        n(2).max(0.0) as usize,
        n(3).max(1.0) as usize,
        n(4).max(1.0) as usize,
        n(5).max(1.0) as usize,
        n(6).max(1.0) as usize,
    );
}

/// `os_thumb_draw(key, tx, ty)`: blit a stored thumbnail 1:1 into the window.
fn os_thumb_draw(args: &[u64]) {
    let Some(w) = win() else { return };
    let key = args.first().map(|&v| to_string(v)).unwrap_or_default();
    let n = |i: usize| args.get(i).map(|&v| to_f64(v)).unwrap_or(0.0);
    let (tx, ty) = (n(1) as i64, n(2) as i64);
    if let Some((tw, th, buf)) = unsafe { (*(&raw const THUMBS)).get(&key) } {
        for yy in 0..*th {
            for xx in 0..*tw {
                put(w, tx + xx as i64, ty + yy as i64, buf[yy * tw + xx]);
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

/// Seconds since 1970-01-01 UTC (`crate::clock`: network time, else the
/// board clock, never earlier than this build).
fn os_time() -> u64 {
    crate::clock::unix_now()
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

/// The machine's name for prompts: the Pi's setup.conf choice, else the
/// lingfs `/hostname` file (the x86 console's `hostname`), else "lingos".
fn host_name() -> String {
    #[cfg(target_arch = "aarch64")]
    return crate::setup::hostname();
    #[cfg(not(target_arch = "aarch64"))]
    {
        let mut buf = [0u8; 64];
        if let Ok(Some(n)) = crate::fs::lingfs::read_file_all("/hostname", &mut buf) {
            let s = core::str::from_utf8(&buf[..n]).unwrap_or("").trim();
            if !s.is_empty() {
                return String::from(s);
            }
        }
        String::from("lingos")
    }
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
        // Live counters for the system monitor (kstats.rs); plain numbers.
        "mem_total" => format!("{}", crate::kstats::total_frames() * 4 / 1024),
        "net_rx" => format!("{}", crate::kstats::get_net_rx()),
        "net_tx" => format!("{}", crate::kstats::get_net_tx()),
        "disk_rd" => format!("{}", crate::kstats::get_disk_rd()),
        "disk_wr" => format!("{}", crate::kstats::get_disk_wr()),
        "switches" => format!("{}", crate::kstats::get_switches()),
        // SoC temperature (whole degrees C) and ARM clock (MHz); "" where the
        // board reports neither (x86).
        "temp" => {
            #[cfg(target_arch = "aarch64")]
            if let Some(mc) = crate::arch::mailbox::temperature_mc() {
                return format!("{}", mc / 1000);
            }
            String::new()
        },
        "clock_mhz" => {
            #[cfg(target_arch = "aarch64")]
            if let Some(hz) = crate::arch::mailbox::arm_clock_hz() {
                return format!("{}", hz / 1_000_000);
            }
            String::new()
        },
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
            #[cfg(target_arch = "x86_64")]
            if crate::arch::xhci::mouse_ready() {
                return String::from("USB mouse");
            }
            #[allow(unreachable_code)]
            String::from("PS/2 mouse")
        },
        "keyboard" => {
            #[cfg(target_arch = "aarch64")]
            return String::from(if crate::arch::xhci::keyboard_ready() { "USB keyboard" } else { "serial only" });
            #[cfg(target_arch = "x86_64")]
            if crate::arch::xhci::keyboard_ready() {
                return String::from("USB keyboard");
            }
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
// Where present() spends its time (EMA, microseconds): the collector, the
// isolates / daemons (coro), the SSH + Messenger tasks (sched), the screen copy.
static mut PERF_GC_US: f64 = 0.0;
static mut PERF_CORO_US: f64 = 0.0;
static mut PERF_SCHED_US: f64 = 0.0;
static mut PERF_BLIT_US: f64 = 0.0;
// the slowest frame of the last 3 s, so a one-second stall shows up
static mut PERF_MAX_US: f64 = 0.0;
static mut PERF_MAX_AT: u64 = 0;

fn perf_ema(slot: *mut f64, v: u64) {
    unsafe {
        let cur = *slot;
        *slot = if cur == 0.0 { v as f64 } else { cur * 0.8 + v as f64 * 0.2 };
    }
}

/// The frame's decorations on `w` (the window, or a scaled window's screen-sized frame): a launched
/// app's title bar, the input-method box and the pointer, at the screen pointer (mx, my). Every
/// pixel changed goes to `saved` first.
fn decorate(w: &mut Window, mx: f64, my: f64, pointer: bool, saved: &mut Vec<(usize, u32)>) {
    // A launched app's title bar (minimize / maximize / close), when shown.
    if crate::lingrt::app_depth() > 0 {
        app_bar_draw(w, mx, my, saved);
    }
    // The input mode (Ctrl+Space) and what is being composed, bottom right, while not Latin.
    let pe = crate::drivers::ime::preedit();
    if crate::drivers::ime::active() || !pe.is_empty() {
        let mut s = String::from(crate::drivers::ime::label());
        if !pe.is_empty() {
            s.push_str("  ");
            s.push_str(&pe);
        }
        let bw = (s.len() as i64 * 8 + 16).min(w.w as i64);
        let bh = 26i64;
        let (bx, by) = (w.w as i64 - bw - 10, w.h as i64 - bh - 10);
        for yy in by.max(0)..(by + bh).min(w.h as i64) {
            for xx in bx.max(0)..(bx + bw).min(w.w as i64) {
                let i = yy as usize * w.w + xx as usize;
                saved.push((i, w.buf[i]));
                let edge = yy == by || yy == by + bh - 1 || xx == bx || xx == bx + bw - 1;
                w.buf[i] = if edge { 0x9A53D6 } else { 0x1B1B1B };
            }
        }
        text8(w, bx + 8, by + 5, 0xF5F5F5, &s);
    }
    if pointer {
        use crate::drivers::cursor;
        let fill = cursor::fill_color(crate::arch::timer::now_ms());
        // `w` is screen-sized either way (an unscaled window, or a scaled one's screen frame)
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
}

/// `cursor_hide()`: the program draws its own pointer, so the system one stays off while that program
/// runs (the app's name; empty = shown).
static mut CURSOR_HIDE_APP: String = String::new();
/// `set_font_antialias(0)`: that program's font_text glyphs are hard-edged (`TEXT_CRISP` while one draws).
static mut FONT_CRISP_APP: String = String::new();
static mut TEXT_CRISP: bool = false;

fn cursor_hidden() -> bool {
    let h = unsafe { &*(&raw const CURSOR_HIDE_APP) };
    !h.is_empty() && *h == active_app_name()
}

// ── The UI scale layer (ui_begin / ui_end) ──────────────────────────────────
//
// The host's layer (ling/src/runtime/mod.rs): a game lays its HUD out for a
// virtual canvas of (frame size / scale). FLY for FUN sizes it to the DISPLAY
// (scale = render height / display height), so its text and frames keep their
// own pixels while the 3-D frame renders at 720p. While a layer is open every
// draw goes into a buffer of the virtual size, seeded with the frame stretched
// to it; get_width / get_height, the mouse and project_3d report virtual
// coordinates. ui_end keeps the pixels the layer changed: a layer bigger than
// the frame of a scaled window is laid over the upscaled frame by present(),
// a smaller one is scaled into the frame, a 1:1 layer is the frame itself.

/// A layer pixel the layer did not draw (window pixels are 0x00RRGGBB).
const UI_CLEAR: u32 = 0xFF00_0000;

struct UiLayer {
    /// The frame put aside while the layer is drawn on (1:1: a snapshot of it), its size and clip.
    real: Vec<u32>,
    rw: usize,
    rh: usize,
    clip: Option<[i32; 4]>,
    direct: bool,
}

static mut UI_LAYER: Option<UiLayer> = None;
/// Layers bigger than the frame, closed this frame, for present(): (pixels -- UI_CLEAR where the
/// layer drew nothing --, width, height).
static mut UI_OVERLAYS: Vec<(Vec<u32>, usize, usize)> = Vec::new();
/// Layer buffers for reuse (a fresh 8 MB buffer for every layer of every frame page-faults).
static mut UI_POOL: Vec<Vec<u32>> = Vec::new();

fn ui_take(n: usize) -> Vec<u32> {
    let pool = unsafe { &mut *(&raw mut UI_POOL) };
    let mut v = match pool.iter().position(|b| b.len() == n).or_else(|| pool.iter().position(|b| b.capacity() >= n)) {
        Some(i) => pool.swap_remove(i),
        None => Vec::with_capacity(n),
    };
    if v.len() != n {
        v.resize(n, 0);
    }
    v
}

fn ui_give(v: Vec<u32>) {
    let pool = unsafe { &mut *(&raw mut UI_POOL) };
    if pool.len() < 6 && !v.is_empty() {
        pool.push(v);
    }
}

/// present() has laid the overlays on the screen: their buffers go back to the pool.
fn ui_overlays_done() {
    let ovs = unsafe { &mut *(&raw mut UI_OVERLAYS) };
    while let Some((v, _, _)) = ovs.pop() {
        ui_give(v);
    }
}

/// A new window: no layer of the old one carries over.
fn ui_reset() {
    unsafe {
        *(&raw mut UI_LAYER) = None;
        (*(&raw mut UI_OVERLAYS)).clear();
    }
}

/// While a layer bigger than the frame is open: the frame's size and virtual pixels per frame pixel
/// (vertical) -- `drivers::g3d` projects against the frame's camera and scales to the layer.
pub(crate) fn ui_layer_scene() -> Option<(usize, usize, f32)> {
    let l = unsafe { (*(&raw const UI_LAYER)).as_ref() }?;
    if l.direct {
        return None;
    }
    let w = win()?;
    Some((l.rw, l.rh, w.h as f32 / l.rh.max(1) as f32))
}

/// Seed rows of a vw×vh layer with the rw×rh frame, nearest-sampled (over the cores).
fn ui_stretch(dst: &mut [u32], vw: usize, vh: usize, src: &[u32], rw: usize, rh: usize) {
    let xs: Vec<u32> = (0..vw).map(|vx| ((vx * rw / vw).min(rw - 1)) as u32).collect();
    let (chunks, per) = crate::par::row_chunks(vh, 32);
    let dp = crate::par::Shared(dst.as_mut_ptr());
    let xs = &xs[..];
    crate::par::for_each(chunks, &|c: usize| {
        let d = unsafe { core::slice::from_raw_parts_mut(dp.get(), vw * vh) };
        for vy in c * per..((c + 1) * per).min(vh) {
            let ry = (vy * rh / vh).min(rh - 1);
            let s = &src[ry * rw..ry * rw + rw];
            for (p, &x) in d[vy * vw..vy * vw + vw].iter_mut().zip(xs.iter()) {
                *p = s[x as usize];
            }
        }
    });
}

/// Mark every pixel of the layer `off` (vw×vh) that still holds the frame's (rw×rh, as seeded)
/// UI_CLEAR; the rest keep their colour.
fn ui_mark(off: &mut [u32], vw: usize, vh: usize, src: &[u32], rw: usize, rh: usize) {
    let xs: Vec<u32> = (0..vw).map(|vx| ((vx * rw / vw).min(rw - 1)) as u32).collect();
    let (chunks, per) = crate::par::row_chunks(vh, 32);
    let dp = crate::par::Shared(off.as_mut_ptr());
    let xs = &xs[..];
    crate::par::for_each(chunks, &|c: usize| {
        let d = unsafe { core::slice::from_raw_parts_mut(dp.get(), vw * vh) };
        for vy in c * per..((c + 1) * per).min(vh) {
            let ry = (vy * rh / vh).min(rh - 1);
            let s = &src[ry * rw..ry * rw + rw];
            for (p, &x) in d[vy * vw..vy * vw + vw].iter_mut().zip(xs.iter()) {
                *p = if *p == s[x as usize] { UI_CLEAR } else { *p & 0xFF_FFFF };
            }
        }
    });
}

fn ui_begin(sc: f64) {
    if unsafe { (*(&raw const UI_LAYER)).is_some() } {
        return;
    }
    // the 3-D drawn so far lands on the frame before drawing moves to the layer
    crate::drivers::g3d::surface_switch(true);
    let Some(w) = win() else { return };
    let sc = if sc.is_finite() { sc.clamp(0.25, 8.0) } else { 1.0 };
    let (rw, rh) = (w.w, w.h);
    if rw == 0 || rh == 0 {
        return;
    }
    let vw = (libm::round(rw as f64 / sc) as usize).max(1);
    let vh = (libm::round(rh as f64 / sc) as usize).max(1);
    let bigger = vw > rw || vh > rh;
    if (vw == rw && vh == rh) || (bigger && w.screen.is_none()) {
        // 1:1 (or bigger than an unscaled window, which has no screen pixels to spare): draw straight
        // into the frame; a snapshot answers ui_pixel_drawn
        let mut snap = ui_take(rw * rh);
        snap.copy_from_slice(&w.buf[..rw * rh]);
        unsafe { *(&raw mut UI_LAYER) = Some(UiLayer { real: snap, rw, rh, clip: w.clip, direct: true }) };
        return;
    }
    let t0 = crate::arch::timer::now_us();
    let mut off = ui_take(vw * vh);
    ui_stretch(&mut off, vw, vh, &w.buf, rw, rh);
    let real = core::mem::replace(&mut w.buf, off);
    let clip = w.clip.take();
    w.w = vw;
    w.h = vh;
    unsafe { *(&raw mut UI_LAYER) = Some(UiLayer { real, rw, rh, clip, direct: false }) };
    crate::prof::add(crate::prof::SCALE, crate::arch::timer::now_us().saturating_sub(t0));
}

fn ui_end() {
    let Some(l) = (unsafe { (*(&raw mut UI_LAYER)).take() }) else { return };
    if l.direct {
        ui_give(l.real);
        return;
    }
    // 3-D drawn inside the layer lands on it (no second tone pass over the UI)
    crate::drivers::g3d::surface_switch(false);
    let Some(w) = win() else { return };
    let t0 = crate::arch::timer::now_us();
    let (vw, vh, rw, rh) = (w.w, w.h, l.rw, l.rh);
    let mut off = core::mem::replace(&mut w.buf, l.real);
    w.w = rw;
    w.h = rh;
    w.clip = l.clip;
    if off.len() == vw * vh && w.buf.len() >= rw * rh {
        ui_mark(&mut off, vw, vh, &w.buf, rw, rh);
        if vw > rw || vh > rh {
            unsafe { (*(&raw mut UI_OVERLAYS)).push((off, vw, vh)) };
        } else {
            // a layer smaller than the frame: its pixels, scaled up, onto the frame
            let xs: Vec<u32> = (0..rw).map(|rx| ((rx * vw / rw).min(vw - 1)) as u32).collect();
            let (chunks, per) = crate::par::row_chunks(rh, 32);
            let fp = crate::par::Shared(w.buf.as_mut_ptr());
            let (o, xs) = (&off[..], &xs[..]);
            crate::par::for_each(chunks, &|c: usize| {
                let f = unsafe { core::slice::from_raw_parts_mut(fp.get(), rw * rh) };
                for ry in c * per..((c + 1) * per).min(rh) {
                    let vy = (ry * vh / rh).min(vh - 1);
                    let orow = &o[vy * vw..vy * vw + vw];
                    for (d, &x) in f[ry * rw..ry * rw + rw].iter_mut().zip(xs.iter()) {
                        let p = orow[x as usize];
                        if p != UI_CLEAR {
                            *d = p;
                        }
                    }
                }
            });
            ui_give(off);
        }
    } else {
        ui_give(off);
    }
    crate::prof::add(crate::prof::SCALE, crate::arch::timer::now_us().saturating_sub(t0));
}

/// `ui_pixel_drawn(x, y)`: did the open layer draw the pixel at virtual (x, y)?
fn ui_pixel_drawn(x: f64, y: f64) -> bool {
    let Some(l) = (unsafe { (*(&raw const UI_LAYER)).as_ref() }) else { return false };
    let Some(w) = win() else { return false };
    if !(x >= 0.0 && y >= 0.0) {
        return false;
    }
    let (xi, yi) = (x as usize, y as usize);
    if xi >= w.w || yi >= w.h {
        return false;
    }
    let cur = w.buf[yi * w.w + xi];
    let was = if l.direct {
        l.real.get(yi * l.rw + xi).copied()
    } else {
        let rx = (xi * l.rw / w.w).min(l.rw - 1);
        let ry = (yi * l.rh / w.h).min(l.rh - 1);
        l.real.get(ry * l.rw + rx).copied()
    };
    was.is_some_and(|v| v != cur)
}

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
    let (ema, max_us) = unsafe {
        PERF_FRAME_US = if PERF_FRAME_US == 0.0 { dt } else { PERF_FRAME_US * 0.8 + dt * 0.2 };
        if dt >= PERF_MAX_US || now.saturating_sub(PERF_MAX_AT) > 3_000_000 {
            PERF_MAX_US = dt;
            PERF_MAX_AT = now;
        }
        (PERF_FRAME_US, PERF_MAX_US)
    };
    let fps = if ema > 0.0 { 1_000_000.0 / ema } else { 0.0 };
    let (gc, coro, sched, blit) = unsafe { (PERF_GC_US, PERF_CORO_US, PERF_SCHED_US, PERF_BLIT_US) };
    let app = (ema - gc - coro - sched - blit).max(0.0);
    let line = alloc::format!(
        " frame {:.1} ms ({:.0} fps) max {:.0} ms  app {:.1} gc {:.1} iso {:.1} tasks {:.1} blit {:.1}  builtins {} tris {}{} gc#{} free {} MiB ",
        ema / 1000.0,
        fps,
        max_us / 1000.0,
        app / 1000.0,
        gc / 1000.0,
        coro / 1000.0,
        sched / 1000.0,
        blit / 1000.0,
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
    let t0 = if crate::prof::on() { crate::arch::timer::now_us() } else { 0 };
    present_inner();
    if t0 != 0 {
        crate::prof::frame_end(t0, crate::arch::timer::now_us());
    }
}

/// When the next frame may go out (frame pacing; 0 = restart the cadence).
#[cfg(target_arch = "x86_64")]
static mut PACE_NEXT_US: u64 = 0;

/// Frame pacing: present at most `framebuffer::present_hz()` frames a second
/// (Settings > Appearance > Refresh rate; 0 = unlimited). Rendering 200+ frames
/// a second for a 60 Hz panel only burns power and heat; the wait goes to the
/// background tasks and isolates instead. The cadence is a fixed grid, so the
/// frames land evenly; a frame that ran over a whole slot restarts the grid
/// from now instead of rushing out catch-up frames.
#[cfg(target_arch = "x86_64")]
fn pace_frame() {
    let hz = crate::drivers::framebuffer::present_hz() as u64;
    if hz == 0 {
        unsafe { PACE_NEXT_US = 0 };
        return;
    }
    let interval = 1_000_000 / hz;
    let now = crate::arch::timer::now_us;
    let t0 = now();
    unsafe {
        let mut next = PACE_NEXT_US;
        if next == 0 || t0 > next + interval || next > t0 + interval {
            next = t0;
        }
        let mut spins = 0u32;
        while now() < next && spins < 1_000_000 {
            crate::coro::yield_now();
            crate::proc::sched::yield_now();
            // the last stretch is spun finely; before it, let the core breathe
            let left = next.saturating_sub(now());
            if left > 200 {
                crate::arch::timer::delay_us(left.min(500) - 100);
            } else {
                core::hint::spin_loop();
            }
            spins += 1;
        }
        PACE_NEXT_US = next + interval;
    }
    crate::prof::add(crate::prof::PACE, now().saturating_sub(t0));
}

fn present_inner() {
    // a headless service's frame loop: just give everyone else a turn
    if crate::coro::in_coro() {
        crate::coro::yield_now();
        return;
    }
    // an app's first frame takes the screen from the boot screen
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    crate::drivers::bootscreen::end();
    crate::startup::boot_once();
    crate::startup::tick(); // restart any daemon whose service crashed (throttled)
    // a UI layer the program left open closes with the frame
    ui_end();
    crate::lingrt::mark_present();
    let t_gc = crate::arch::timer::now_us();
    crate::lingrt::gc_maybe();
    let t_gc_end = crate::arch::timer::now_us();
    crate::prof::add(crate::prof::GC, t_gc_end.saturating_sub(t_gc));
    crate::drivers::g3d::end_frame();
    crate::prof::add_endframe(crate::arch::timer::now_us().saturating_sub(t_gc_end));
    crate::wifi::tick();
    // the program's isolates and workers (coro.rs) get a turn every frame
    let t_coro = crate::arch::timer::now_us();
    perf_ema(&raw mut PERF_GC_US, t_coro.saturating_sub(t_gc));
    crate::coro::yield_now();
    // the background tasks (SSH server, chat listener) get the CPU once a frame
    let t_sched = crate::arch::timer::now_us();
    perf_ema(&raw mut PERF_CORO_US, t_sched.saturating_sub(t_coro));
    // (x86 too: a full-screen app that never calls ling_kernel_yield, like FLY
    // for FUN, otherwise starved the SSH server and the LLM task while it ran)
    crate::proc::sched::yield_now();
    let t_tasks_end = crate::arch::timer::now_us();
    perf_ema(&raw mut PERF_SCHED_US, t_tasks_end.saturating_sub(t_sched));
    crate::prof::add(crate::prof::TASKS, t_tasks_end.saturating_sub(t_coro));
    // hold the frame for its slot at the display refresh, then sample the
    // pointer, so the cursor drawn below is as fresh as it can be
    #[cfg(target_arch = "x86_64")]
    pace_frame();
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
    // PrtSc was pressed: this frame goes out without the pointer, then the
    // screenshot overlay takes it (drivers/screenshot.rs)
    #[cfg(target_arch = "x86_64")]
    let shot = crate::drivers::screenshot::pending();
    #[cfg(not(target_arch = "x86_64"))]
    let shot = false;
    if let Some(w) = win() {
        // The screenshot notice goes in first and comes out last, so the
        // overlays below save and restore pixels on top of it.
        #[cfg(target_arch = "x86_64")]
        let mut toast = crate::drivers::screenshot::toast_patch(w.w, w.h);
        #[cfg(target_arch = "x86_64")]
        if let Some(t) = toast.as_mut() {
            crate::drivers::screenshot::swap_into(&mut w.buf, w.w, t);
        }
        // Draw the pointer into the frame, blit, then put the pixels back so the
        // app's own buffer is untouched. A scaled window gets them after scaling,
        // in screen pixels (sharp, the size they are everywhere else, and above
        // the UI layers' pixels).
        let mut saved: Vec<(usize, u32)> = Vec::new();
        let pointer = mx >= 0.0 && my >= 0.0 && !shot && !cursor_hidden();
        if w.screen.is_none() {
            decorate(w, mx, my, pointer, &mut saved);
        }
        let t_blit = crate::arch::timer::now_us();
        match w.screen {
            Some((sw, sh)) => {
                // nearest-neighbour aspect-fit into a screen-sized frame
                let out = unsafe { &mut *(&raw mut SCALE_BUF) };
                if out.len() != sw * sh {
                    *out = vec![0u32; sw * sh];
                }
                let (ox, oy, dw, dh) = fit_rect(w.w, w.h, sw, sh);
                // the column map only changes with the sizes: keep it between frames
                let xmap = unsafe { &mut *(&raw mut XMAP) };
                if xmap.0 != (w.w, dw) {
                    *xmap = ((w.w, dw), (0..dw).map(|x| (x * w.w / dw) as u32).collect());
                }
                let xm = &xmap.1;
                // the UI layers closed this frame (ui_end), each with its own column map
                // when it is not exactly the scaled frame's width
                let ovs: &Vec<(Vec<u32>, usize, usize)> = unsafe { &*(&raw const UI_OVERLAYS) };
                let ovmaps: Vec<Option<Vec<u32>>> =
                    ovs.iter().map(|&(_, vw, _)| if vw == dw { None } else { Some((0..dw).map(|x| ((x * vw / dw).min(vw - 1)) as u32).collect()) }).collect();
                let layered = !ovs.is_empty();
                // rows are independent: split them over the cores (each chunk
                // starts fresh, then copies a repeated source row from above)
                let (chunks, per) = crate::par::row_chunks(dh, 32);
                let outp = crate::par::Shared(out.as_mut_ptr());
                let (src_buf, ww, wh) = (&w.buf[..], w.w, w.h);
                crate::par::for_each(chunks, &|c: usize| {
                    let out = unsafe { core::slice::from_raw_parts_mut(outp.get(), sw * sh) };
                    let mut prev_sy = usize::MAX;
                    for y in c * per..((c + 1) * per).min(dh) {
                        let sy = y * wh / dh;
                        let row = (oy + y) * sw + ox;
                        if sy == prev_sy && !layered {
                            // the same source row as the line above (any upscale): copy it
                            let from = (oy + y - 1) * sw + ox;
                            out.copy_within(from..from + dw, row);
                        } else {
                            let src = &src_buf[sy * ww..][..ww];
                            let dst = &mut out[row..row + dw];
                            for (d, &sx) in dst.iter_mut().zip(xm.iter()) {
                                *d = src[sx as usize];
                            }
                        }
                        prev_sy = sy;
                        // then every UI layer's own pixels, in the order they were drawn
                        let dst = &mut out[row..row + dw];
                        for ((ov, vw, vh), map) in ovs.iter().zip(ovmaps.iter()) {
                            let vy = (y * vh / dh).min(vh - 1);
                            let orow = &ov[vy * vw..vy * vw + vw];
                            match map {
                                None => {
                                    for (d, &p) in dst.iter_mut().zip(orow.iter()) {
                                        if p != UI_CLEAR {
                                            *d = p;
                                        }
                                    }
                                },
                                Some(m) => {
                                    for (d, &vx) in dst.iter_mut().zip(m.iter()) {
                                        let p = orow[vx as usize];
                                        if p != UI_CLEAR {
                                            *d = p;
                                        }
                                    }
                                },
                            }
                        }
                    }
                });
                // the bar, IME box and pointer in screen pixels, on the scaled frame
                let mut sv = Window { buf: core::mem::take(out), w: sw, h: sh, open: true, clip: None, black: false, screen: None };
                let mut scratch: Vec<(usize, u32)> = Vec::new();
                decorate(&mut sv, mx, my, pointer, &mut scratch);
                *out = sv.buf;
                crate::prof::add(crate::prof::SCALE, crate::arch::timer::now_us().saturating_sub(t_blit));
                let t_fb = crate::arch::timer::now_us();
                framebuffer::blit_rgb(out, sw, sh);
                crate::prof::add(crate::prof::BLIT, crate::arch::timer::now_us().saturating_sub(t_fb));
            },
            None => {
                framebuffer::blit_rgb(&w.buf, w.w, w.h);
                crate::prof::add(crate::prof::BLIT, crate::arch::timer::now_us().saturating_sub(t_blit));
            },
        }
        ui_overlays_done();
        let t_blit_end = crate::arch::timer::now_us();
        perf_ema(&raw mut PERF_BLIT_US, t_blit_end.saturating_sub(t_blit));
        for (i, v) in saved {
            w.buf[i] = v;
        }
        #[cfg(target_arch = "x86_64")]
        if let Some(t) = toast.as_mut() {
            crate::drivers::screenshot::swap_into(&mut w.buf, w.w, t);
        }
        if crate::lingrt::app_depth() > 0 {
            frame_sample(w);
        }
    }
    #[cfg(target_arch = "x86_64")]
    if shot && crate::drivers::screenshot::take_pending() {
        crate::drivers::screenshot::run();
    }
    // Input for the next frame. Ctrl+Q closes a launched app (never the
    // desktop itself) back to whatever started it.
    crate::prof::mark_pump_start();
    let nested = crate::lingrt::app_depth() > 0;
    let input = unsafe { &mut *(&raw mut INPUT) };
    // judged on the frame the app just drew, before its keys are cleared
    let esc_quit = nested && esc_quits(input);
    let esc_quit = esc_quit || (nested && esc_check_due());
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
        // A USB keyboard's held keys (key_down) and a USB mouse's wheel; its
        // key presses arrive below as scancode-decoded bytes like PS/2's.
        input.prev_held = input.held;
        input.held = crate::arch::xhci::held_report();
        input.wheel = crate::arch::xhci::mouse_take_wheel();
        loop {
            match crate::drivers::keyboard::poll_char() {
                0 => break,
                0x91 => input.feed(0x11),
                crate::drivers::keyboard::CTRL_SPACE => crate::drivers::ime::cycle(&mut input.text),
                // Ctrl+letter chords (keyboard.rs): the letter pressed with
                // Ctrl held this frame, as apps test with key_down("ctrl").
                c @ 0x81..=0x9A => {
                    if let Some(name) = key_name_of_byte(b'a' + (c - 0x81)) {
                        input.pressed.push("ctrl");
                        input.pressed.push("lctrl");
                        input.pressed.push(name);
                    }
                },
                // keyboard.rs's navigation codes are not terminal bytes: fed to
                // `feed`, Up (0x11) read as Ctrl+Q and quit the app, and Down /
                // Left / Right / Home / End / PgUp / PgDn did nothing
                crate::drivers::keyboard::UP_ARROW => input.pressed.push("up"),
                crate::drivers::keyboard::DOWN_ARROW => input.pressed.push("down"),
                crate::drivers::keyboard::LEFT_ARROW => input.pressed.push("left"),
                crate::drivers::keyboard::RIGHT_ARROW => input.pressed.push("right"),
                crate::drivers::keyboard::HOME_KEY => input.pressed.push("home"),
                crate::drivers::keyboard::END_KEY => input.pressed.push("end"),
                crate::drivers::keyboard::PGUP_KEY => input.pressed.push("pageup"),
                crate::drivers::keyboard::PGDN_KEY => input.pressed.push("pagedown"),
                // the overlay opens at the next present, over every app
                crate::drivers::keyboard::PRTSC_KEY => crate::drivers::screenshot::request(),
                c => input.feed(c),
            }
        }
    }
    // Esc is the app's Back key; when the app did not even look at it (nothing
    // to go back from), or it comes twice in a row, it quits the app -- or parks
    // it, like Minimize, when the app asked for that (`esc_minimizes`).
    if esc_quit {
        if !input.quit && active_app().is_some_and(|a| a.esc_parks) {
            app_suspend();
            return;
        }
        input.quit = true;
    }
    // The title bar's buttons.
    if nested {
        match app_bar_click() {
            BarAction::Close => input.quit = true,
            BarAction::Minimize if active_app().is_some() => {
                input.quit = false;
                app_suspend();
                return;
            },
            _ => {},
        }
    }
    if input.quit && nested {
        if let Some(w) = win() {
            w.open = false;
        }
    }
    input.quit = false;
    // Any input counts as activity for the idle lock.
    let moved = unsafe { MOUSE_DELTA != (0.0, 0.0) };
    if moved || !input.pressed.is_empty() || !input.text.is_empty() || input.held != input.prev_held || mouse_state().2 != 0 {
        crate::startup::touch();
    }
    // Alt+Tab inside a launched app: back to the desktop, the app kept alive.
    if active_app().is_some() && key_down("alt") && key_pressed("tab") {
        app_suspend();
    } else if active_app().is_some() && crate::startup::take_bg_start() {
        // started for the background (a sign-in app): park after its first frame
        app_suspend();
    } else if active_app().is_some() && crate::startup::lock_due() {
        // idle: the app keeps its state (and its isolates run on); the desktop locks
        crate::startup::request_lock();
        app_suspend();
    }
}

/// Keyboard state between frames: text typed since the last `text_poll`, key
/// names pressed during the last frame (from terminal-style bytes), and the
/// USB keyboard's held keys now and one frame ago (for `key_down` and edge
/// `key_pressed` on keys that send no bytes, like Super or F-keys).
struct Input {
    text: String,
    /// `text_set` was called: `text_poll` works as on the desktop runtime from then on (the whole
    /// field's text, typing appended and Backspace applied) instead of handing out what was typed
    /// since the last call. Apps written for the desktop (LingStationX, LingDev) need that.
    buf_mode: bool,
    buf: String,
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
    buf_mode: false,
    buf: String::new(),
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
            // Ctrl+Space (NUL from a terminal or the USB keyboard): the next input mode
            (_, 0x00) => crate::drivers::ime::cycle(&mut self.text),
            (_, b'\r' | b'\n') => {
                crate::drivers::ime::flush(&mut self.text);
                self.pressed.push("enter");
                self.text.push('\n');
            },
            (_, 0x08 | 0x7F) => {
                // a step back in a composition (a Hangul jamo, romaji) is the input method's
                if !crate::drivers::ime::backspace() {
                    self.pressed.push("backspace");
                    self.text.push('\u{8}');
                }
            },
            (_, b'\t') => {
                crate::drivers::ime::flush(&mut self.text);
                self.pressed.push("tab");
                self.text.push('\t'); // in the text stream too: forms move on in order
            },
            (_, 0x20..=0x7E) => {
                // typed text in the current input mode (drivers/ime.rs); key names stay the keys
                crate::drivers::ime::feed(c as char, &mut self.text);
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
    if n == "escape" || n == "esc" {
        unsafe { ESC_SEEN = true };
    }
    key_usages(&n).iter().any(|&u| report_has(&i.held, u)) || i.pressed.iter().any(|p| *p == n)
}

fn key_pressed(name: &str) -> bool {
    let i = unsafe { &mut *(&raw mut INPUT) };
    i.flush_esc();
    let n = name.to_ascii_lowercase();
    if n == "escape" || n == "esc" {
        unsafe { ESC_SEEN = true };
    }
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
        // draining the 8042 is port I/O (a VM exit each under KVM, ~10 us): a game asking for
        // mouse_x / mouse_y thousands of times a frame drains it at most every 0.5 ms
        static mut LAST_DRAIN_US: u64 = 0;
        let now = crate::arch::timer::now_us();
        if now.wrapping_sub(unsafe { LAST_DRAIN_US }) >= 500 {
            unsafe { LAST_DRAIN_US = now };
            crate::arch::idt::poll_drain_8042();
        }
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

// ── A launched app's title bar and Esc ───────────────────────────────────────
// Apps own the whole screen, so the bar slides in over their top edge: shown for
// a few seconds when an app starts, when the pointer touches the top of the
// screen, and always while pinned (the maximize / restore button).

const BAR_H: i64 = 28;
const BAR_BTN_W: i64 = 44;
static mut BAR_UNTIL_MS: u64 = 0;
static mut BAR_PINNED: bool = false;
static mut BAR_SHOWN: bool = false;
static mut BAR_APP: String = String::new();
static mut BAR_BTN_LAST: u8 = 0;
/// The app asked about Esc since the last frame (`key_pressed("escape")`).
static mut ESC_SEEN: bool = false;
static mut ESC_LAST_MS: u64 = 0;

#[derive(PartialEq)]
enum BarAction {
    None,
    Minimize,
    Close,
}

/// Is the bar on screen this frame?
fn app_bar_visible(my: f64) -> bool {
    let now = crate::arch::timer::now_ms();
    let name = active_app_name();
    unsafe {
        let app = &mut *(&raw mut BAR_APP);
        if *app != name {
            *app = name;
            BAR_UNTIL_MS = now + 3000;
        }
        let near = my >= 0.0 && my < if BAR_SHOWN { BAR_H as f64 } else { 4.0 };
        BAR_SHOWN = BAR_PINNED || now < BAR_UNTIL_MS || near;
        BAR_SHOWN
    }
}

/// The three buttons' left edges: minimize, maximize, close.
fn bar_buttons(w: usize) -> [i64; 3] {
    let r = w as i64;
    [r - 3 * BAR_BTN_W, r - 2 * BAR_BTN_W, r - BAR_BTN_W]
}

fn app_bar_draw(w: &mut Window, mx: f64, my: f64, saved: &mut Vec<(usize, u32)>) {
    if !app_bar_visible(my) {
        return;
    }
    let h = (BAR_H as usize).min(w.h);
    for i in 0..h * w.w {
        saved.push((i, w.buf[i]));
        w.buf[i] = 0x16141C;
    }
    for x in 0..w.w {
        let i = (h - 1) * w.w + x;
        w.buf[i] = 0x6E3FF3;
    }
    // the bundle's short name, spelled the way the menu does
    let mut title = active_app_name();
    let known = [
        ("lingdev", "LingDev"), ("lingstation", "LingStationX"), ("lingmodeler", "LingModeler"),
        ("horizon", "Horizon"), ("messenger", "Messenger"), ("wallet", "Ling Wallet"),
        ("lingforfun", "Ling For Fun"), ("linganime", "LingAnime"),
    ];
    if let Some((_, nice)) = known.iter().find(|(k, _)| title.eq_ignore_ascii_case(k)) {
        title = String::from(*nice);
    } else if let Some(c) = title.get_mut(0..1) {
        c.make_ascii_uppercase();
    }
    text8(w, 12, (BAR_H - 8) / 2, 0xE8E6F0, &title);
    let hint = "Esc: back / quit";
    text8(w, 12 + title.len() as i64 * 8 + 24, (BAR_H - 8) / 2, 0x8A8798, hint);
    let bx = bar_buttons(w.w);
    let cy = BAR_H / 2;
    for (k, &x0) in bx.iter().enumerate() {
        let hover = my >= 0.0 && (my as i64) < BAR_H && mx as i64 >= x0 && (mx as i64) < x0 + BAR_BTN_W;
        if hover {
            let bg = if k == 2 { 0xC42B4B } else { 0x2E2A3A };
            for y in 0..(BAR_H - 1) {
                for x in x0.max(0)..(x0 + BAR_BTN_W).min(w.w as i64) {
                    w.buf[y as usize * w.w + x as usize] = bg;
                }
            }
        }
        let c = 0xE8E6F0;
        let mx0 = x0 + BAR_BTN_W / 2;
        match k {
            0 => {
                for x in mx0 - 6..=mx0 + 6 {
                    put(w, x, cy + 4, c);
                }
            },
            1 => {
                // pinned (restored) draws two stacked squares, like a desktop's restore button
                let s = 6;
                for d in -s..=s {
                    put(w, mx0 + d, cy - s, c);
                    put(w, mx0 + d, cy + s, c);
                    put(w, mx0 - s, cy + d, c);
                    put(w, mx0 + s, cy + d, c);
                }
                if unsafe { BAR_PINNED } {
                    for d in -s + 3..=s + 3 {
                        put(w, mx0 + d, cy - s - 3, c);
                        put(w, mx0 + s + 3, cy + d - 3, c);
                    }
                }
            },
            _ => {
                for d in -6..=6 {
                    put(w, mx0 + d, cy + d, c);
                    put(w, mx0 + d, cy - d, c);
                    put(w, mx0 + d + 1, cy + d, c);
                    put(w, mx0 + d + 1, cy - d, c);
                }
            },
        }
    }
}

/// A click on the bar this frame (a press of the left button over a button).
fn app_bar_click() -> BarAction {
    let (mx, my, b) = mouse_state();
    let pressed = b & 1 != 0 && unsafe { BAR_BTN_LAST } & 1 == 0;
    unsafe { BAR_BTN_LAST = b };
    if !pressed || !unsafe { BAR_SHOWN } || my < 0.0 || my as i64 >= BAR_H {
        return BarAction::None;
    }
    let Some(w) = win() else { return BarAction::None };
    // drawn in screen pixels (over a scaled window too)
    let bx = bar_buttons(w.screen.map_or(w.w, |s| s.0));
    let x = mx as i64;
    if x >= bx[0] && x < bx[0] + BAR_BTN_W {
        BarAction::Minimize
    } else if x >= bx[1] && x < bx[1] + BAR_BTN_W {
        unsafe { BAR_PINNED = !BAR_PINNED };
        BarAction::None
    } else if x >= bx[2] && x < bx[2] + BAR_BTN_W {
        BarAction::Close
    } else {
        BarAction::None
    }
}

/// The pointer as the app sees it: over the bar, its buttons are the bar's.
fn app_mouse_state() -> (f64, f64, u8) {
    let (x, y, b) = mouse_state();
    // the bar is drawn in screen pixels (a scaled window's too)
    let sy = y;
    let (x, y) = to_buffer_coords(x, y);
    if crate::lingrt::app_depth() > 0 && unsafe { BAR_SHOWN } && sy >= 0.0 && (sy as i64) < BAR_H {
        return (x, y, 0);
    }
    (x, y, b)
}

/// A coarse grid of the app's last two frames (every 16th pixel), so an Esc
/// that changed nothing on screen can be told from one that went back.
static mut SAMPLE_CUR: Vec<u32> = Vec::new();
static mut SAMPLE_PREV: Vec<u32> = Vec::new();
/// After an Esc the app looked at: the frame from before it, and how many
/// more frames to wait before judging whether the app did anything with it.
static mut ESC_BASE: Vec<u32> = Vec::new();
static mut ESC_WAIT: u8 = 0;

fn frame_sample(w: &Window) {
    unsafe {
        let prev = &mut *(&raw mut SAMPLE_PREV);
        let cur = &mut *(&raw mut SAMPLE_CUR);
        core::mem::swap(prev, cur);
        cur.clear();
        for y in (8..w.h).step_by(16) {
            for x in (8..w.w).step_by(16) {
                cur.push(w.buf[y * w.w + x]);
            }
        }
    }
}

/// Esc came in during the frame just drawn: quit when the app never looked at
/// it, or on a second Esc within 0.6 s. When the app did look, watch the next
/// frames: an Esc that changed (next to) nothing had nothing to go back from,
/// so it quits (`esc_check_due`).
fn esc_quits(input: &mut Input) -> bool {
    input.flush_esc();
    let esc = input.pressed.iter().any(|p| *p == "escape") || (report_has(&input.held, 0x29) && !report_has(&input.prev_held, 0x29));
    let seen = unsafe { core::mem::replace(&mut *(&raw mut ESC_SEEN), false) };
    if !esc {
        return false;
    }
    let now = crate::arch::timer::now_ms();
    let double = now.saturating_sub(unsafe { ESC_LAST_MS }) < 600;
    unsafe { ESC_LAST_MS = now };
    if !seen || double {
        return true;
    }
    unsafe {
        // the frame before the one that saw the Esc
        *(&mut *(&raw mut ESC_BASE)) = (*(&raw const SAMPLE_PREV)).clone();
        ESC_WAIT = 2;
    }
    false
}

/// The Esc being watched has had its frames: quit if the screen stayed put.
fn esc_check_due() -> bool {
    unsafe {
        if ESC_WAIT == 0 {
            return false;
        }
        ESC_WAIT -= 1;
        if ESC_WAIT > 0 {
            return false;
        }
        let base = core::mem::take(&mut *(&raw mut ESC_BASE));
        let cur = &*(&raw const SAMPLE_CUR);
        if base.is_empty() || base.len() != cur.len() {
            return false;
        }
        let diff = base.iter().zip(cur.iter()).filter(|(a, b)| a != b).count();
        // a clock, a caret or a frame-time readout may tick; going back redraws a lot more
        diff <= 6 + base.len() / 200
    }
}

/// 8x8 text, unclipped (the dialog ignores the program's clip rectangle).
fn text8(w: &mut Window, x: i64, y: i64, color: u32, s: &str) {
    // file names in any script: non-ASCII from the UI font, in the byte cells
    let (r8, g8, b8) = ((color >> 16) & 0xFF, (color >> 8) & 0xFF, color & 0xFF);
    let mut pts: Vec<(i64, i64, u8)> = Vec::new();
    utf8_cells_overlay(s.as_bytes(), 8, 8, usize::MAX, |dx, dy, cv| pts.push((x + dx as i64, y + dy as i64, cv)));
    for (px, py, cv) in pts {
        if px >= 0 && py >= 0 && (px as usize) < w.w && (py as usize) < w.h {
            let i = py as usize * w.w + px as usize;
            w.buf[i] = if cv == 255 { color } else { blend(w.buf[i], r8, g8, b8, cv as f32 / 255.0) };
        }
    }
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
            // each app starts with the kernel's own text_poll meaning until it calls text_set
            unsafe {
                (*(&raw mut INPUT)).buf_mode = false;
                (*(&raw mut INPUT)).buf.clear();
            }
            open_window(a(0, 800.0), a(1, 600.0), a(3, 0.0));
            UNIT
        },
        // the host's borderless fullscreen (title, w, h, render height): here a
        // scaled window at the screen's aspect, its frame no taller than the
        // render height (default 720 -- the software rasterizer's fill cost
        // grows with pixels), stretched to the whole screen by present()
        b"fullscreen" | b"open_fullscreen" | b"open_fullscreen_scaled" => {
            unsafe {
                (*(&raw mut INPUT)).buf_mode = false;
                (*(&raw mut INPUT)).buf.clear();
            }
            let (fw, fh) = (framebuffer::width().max(1) as f64, framebuffer::height().max(1) as f64);
            let want = a(3, 0.0);
            let rh = if want >= 120.0 { want.min(fh) } else { fh.min(720.0) };
            open_window(libm::round(fw * rh / fh), rh, 2.0);
            UNIT
        },
        // the render height of a scaled window: a new frame of that height at
        // the same aspect (0 / out of range = the screen's own height, capped)
        b"set_render_height" => {
            // (a layer open over the old frame closes first)
            ui_end();
            if let Some(w) = win() {
                if w.screen.is_some() {
                    let (fw, fh) = (framebuffer::width().max(1) as f64, framebuffer::height().max(1) as f64);
                    let want = a(0, 0.0);
                    let rh = if want >= 120.0 && want < fh { want } else { fh.min(720.0) } as usize;
                    let rw = libm::round(fw * rh as f64 / fh) as usize;
                    if rh != w.h || rw != w.w {
                        w.w = rw;
                        w.h = rh;
                        w.buf = vec![0u32; rw * rh];
                        w.clip = None;
                    }
                }
            }
            num(win().map(|w| w.h).unwrap_or(0) as f64)
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
            unsafe {
                FRAMES += 1;
                MOUSE_WAS_DOWN = FRAME_DOWN.take().unwrap_or_else(|| app_mouse_state().2 & 1 != 0);
            }
            UNIT
        },
        // the host's immediate-mode button: a filled face in the given colour
        // (r, g, b), lighter under the pointer; 1 on the frame it's pressed
        b"ui_button" => {
            let (x, y, bw, bh) = (a(0, 0.0), a(1, 0.0), a(2, 120.0), a(3, 40.0));
            let (mx, my, b) = app_mouse_state();
            let down = unsafe { *(*(&raw mut FRAME_DOWN)).get_or_insert(b & 1 != 0) };
            let hover = mx >= x && mx < x + bw && my >= y && my < y + bh;
            let clicked = hover && down && unsafe { !MOUSE_WAS_DOWN };
            let (r, g, bl) = if args.len() >= 7 { (a(4, 76.0), a(5, 110.0), a(6, 200.0)) } else { (76.0, 110.0, 200.0) };
            let k = if down && hover { 1.25 } else if hover { 1.12 } else { 1.0 };
            let c = pack_rgb((r * k).min(255.0), (g * k).min(255.0), (bl * k).min(255.0));
            if let Some(w) = win() {
                fill_poly(w, &[(x, y), (x + bw, y), (x + bw, y + bh), (x, y + bh)], c);
            }
            num(if clicked { 1.0 } else { 0.0 })
        },
        b"get_width" => num(win().map(|w| w.w).unwrap_or(0) as f64),
        b"get_height" => num(win().map(|w| w.h).unwrap_or(0) as f64),
        // the UI scale layer (see ui_begin above)
        b"ui_begin" => {
            ui_begin(a(0, 1.0));
            UNIT
        },
        b"ui_end" => {
            ui_end();
            UNIT
        },
        b"ui_pixel_drawn" => num(if ui_pixel_drawn(a(0, -1.0), a(1, -1.0)) { 1.0 } else { 0.0 }),
        b"ui_detach" => UNIT,
        // (the host's native display_size answers the frame's size too)
        b"display_size" => {
            let (w, h) = win().map(|w| (w.w as f64, w.h as f64)).unwrap_or((0.0, 0.0));
            list_from(vec![num(w), num(h)])
        },
        b"cursor_hide" => {
            unsafe { *(&raw mut CURSOR_HIDE_APP) = active_app_name() };
            UNIT
        },
        b"cursor_show" => {
            unsafe { (*(&raw mut CURSOR_HIDE_APP)).clear() };
            UNIT
        },
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
        b"set_alpha" => {
            unsafe { PEN_ALPHA = a(0, 1.0).clamp(0.0, 1.0) as f32 };
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
            if crate::coro::in_coro() {
                // an isolate: the others (and the app) run meanwhile
                crate::coro::sleep_ms(end.saturating_sub(crate::arch::timer::now_ms()));
                return Some(UNIT);
            }
            while crate::arch::timer::now_ms() < end {
                crate::proc::sched::yield_now();
                crate::coro::yield_now();
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
        b"px_flat" | b"px_color" => UNIT,
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
            px_text(&map_font_args(args, &[0]));
            UNIT
        },
        b"px_fit" => px_fit(&map_font_args(args, &[0])),

        // Fonts.
        b"font_load" => num(font_load(&path_arg(args, 0))),
        b"font_width" => {
            let s = args.get(2).map(|&v| to_string(v)).unwrap_or_default();
            let g = gfont(a(0, -1.0));
            num(font(g).map(|_| measure(g as usize, &s, a(1, 16.0) as f32) as f64).unwrap_or(0.0))
        },
        // host gfx: `font_text(font, x, y, px, text)` draws in the current
        // `set_color` colour (GFX_COLOR), the "set colour then draw" API some
        // apps (LingSys) use instead of px_text's inline colour. Reuse px_text
        // with the colour spliced into its colour arg.
        // set_font_antialias(0): font_text / font_text_fill glyphs hard-edged like the host's
        // (whole pixels, a pixel font stays exact) for the program that asked
        b"set_font_antialias" => {
            unsafe {
                *(&raw mut FONT_CRISP_APP) = if a(0, 1.0) > 0.5 { String::new() } else { active_app_name() };
            }
            UNIT
        },
        b"font_text" | b"font_text_fill" => {
            unsafe {
                let c = &*(&raw const FONT_CRISP_APP);
                TEXT_CRISP = !c.is_empty() && *c == active_app_name();
            }
            let color = unsafe { GFX_COLOR };
            let mut v: Vec<u64> = Vec::with_capacity(6);
            v.push(*args.first().unwrap_or(&0)); // font
            v.push(*args.get(1).unwrap_or(&0)); // x
            v.push(*args.get(2).unwrap_or(&0)); // y
            v.push(*args.get(3).unwrap_or(&0)); // px
            v.push(num(color as f64)); // colour (packed 0xRRGGBB)
            v.push(*args.get(4).unwrap_or(&0)); // text
            px_text(&map_font_args(&v, &[0]));
            unsafe { TEXT_CRISP = false };
            UNIT
        },
        b"font_fallback" => num(font_fallback(gfont(a(0, -1.0)), gfont(a(1, -1.0)), a(2, 1.0))),

        // Input: PS/2 (x86) or USB HID (Pi) mouse + keyboard, serial on the Pi.
        b"mouse_x" => num(app_mouse_state().0),
        b"mouse_y" => num(app_mouse_state().1),
        // pointer movement since the previous frame (the host's relative-mouse look / paddle)
        b"mouse_dx" => num(unsafe { MOUSE_DELTA.0 }),
        b"mouse_dy" => num(unsafe { MOUSE_DELTA.1 }),
        b"mouse_down" => boolean(app_mouse_state().2 & 1 != 0),
        b"mouse_down_right" => boolean(app_mouse_state().2 & 2 != 0),
        b"mouse_down_middle" => boolean(app_mouse_state().2 & 4 != 0),
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
        // The host's form is pad_button(pad, "a"): a named button of pad `pad`.
        b"pad_button" if args.get(1).is_some_and(|&v| crate::lingrt::is_str(v)) => {
            boolean(crate::hostcompat::pad_button_named(&to_string(args[1])))
        },
        b"pad_button" => {
            let i = args.first().map(|&v| to_f64(v)).unwrap_or(0.0).clamp(0.0, 31.0) as u32;
            boolean((pad().buttons >> i) & 1 == 1)
        },
        b"pad_buttons" => num(pad().buttons as f64),
        b"pad_hat" => num(pad().hat as f64),
        b"key_pressed" => boolean(key_pressed(&path_arg(args, 0))),
        b"text_poll" | b"text_get" => {
            let input = unsafe { &mut *(&raw mut INPUT) };
            let typed = core::mem::take(&mut input.text);
            if !input.buf_mode {
                return Some(mk_str(&typed));
            }
            for c in typed.chars() {
                match c {
                    '\u{8}' => {
                        input.buf.pop();
                    },
                    '\n' | '\t' => {},
                    c if !c.is_control() => input.buf.push(c),
                    _ => {},
                }
            }
            mk_str(&input.buf)
        },
        b"text_set" => {
            let input = unsafe { &mut *(&raw mut INPUT) };
            input.buf_mode = true;
            input.buf = args.first().map(|&v| to_string(v)).unwrap_or_default();
            UNIT
        },
        b"text_clear" => {
            let input = unsafe { &mut *(&raw mut INPUT) };
            input.buf.clear();
            UNIT
        },
        // Flush all stale keyboard state: the driver's decoded-byte queue and
        // held report, plus the app-runtime's typed text / pressed / held. The
        // sign-in screen calls this when it opens so a key held (or a report
        // lost) during logout can't repeat into the password box.
        b"os_kbd_reset" => {
            #[cfg(target_arch = "aarch64")]
            crate::arch::xhci::reset_key_state();
            #[cfg(target_arch = "x86_64")]
            crate::drivers::keyboard::drain();
            let input = unsafe { &mut *(&raw mut INPUT) };
            input.text.clear();
            input.buf.clear();
            input.pressed.clear();
            input.held = [0; 8];
            input.prev_held = [0; 8];
            input.esc = 0;
            crate::drivers::ime::reset();
            UNIT
        },

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
        // live window thumbnails for the taskbar
        b"os_thumb_capture" => {
            os_thumb_capture(args);
            UNIT
        },
        b"os_thumb_draw" => {
            os_thumb_draw(args);
            UNIT
        },
        b"os_thumb_has" => boolean(unsafe { (*(&raw const THUMBS)).contains_key(&args.first().map(|&v| to_string(v)).unwrap_or_default()) }),
        b"px_image" => boolean(px_image(args)),
        // Picture handles (the PC decodes files into images it draws by id): not on the kernel yet.
        b"image_fetch" | b"image_job" | b"image_svg" | b"image_resized" | b"image_tint" | b"file_fetch" | b"file_job" | b"proc_start" | b"proc_poll" | b"re_compile" => num(-1.0),
        b"image_replace" | b"image_fetch_cancel" => UNIT,
        b"re_exec" | b"re_names" => list_from(alloc::vec::Vec::new()),
        b"re_error" => mk_str("regular expressions are not available on LingOS yet"),
        b"os_home_folders" => list_from(HOME_FOLDERS.iter().map(|f| mk_str(f)).collect()),
        b"os_user" => mk_str(&os_user()),
        // the iso_* programs are isolates of an app (iso.rs), not apps of their own
        b"os_apps" => list_from(crate::app_names().into_iter().filter(|n| !n.starts_with("iso_")).map(mk_str).collect()),
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
        // Bulk jobs (blocking; streamed installs print progress to the console):
        // how many packages were updated / newly installed.
        b"lingfu_upgrade_all" => num(crate::drivers::lingfu::upgrade_all().0 as f64),
        b"lingfu_install_all" => num(crate::drivers::lingfu::install_all().0 as f64),
        b"os_packages" => list_from(crate::drivers::lingfu::installed_apps().iter().map(|s| mk_str(s)).collect()),
        b"os_platform" => mk_str(if cfg!(target_arch = "aarch64") { "LingOS rpi5" } else { "LingOS x86-64" }),
        b"os_app_run" => boolean(app_run(&path_arg(args, 0))),
        b"os_app_resume" => boolean(app_resume(&path_arg(args, 0))),
        // Task Manager: end a suspended app (frees its stack, isolates, scope).
        b"os_app_kill" => boolean(app_kill(&path_arg(args, 0))),
        // start an app straight into the background (it parks after its first frame)
        b"os_app_start_bg" => {
            let n = path_arg(args, 0);
            if apps_running().iter().any(|a| *a == n) {
                return Some(boolean(true));
            }
            crate::startup::set_bg_start(true);
            let ok = app_run(&n);
            crate::startup::set_bg_start(false);
            boolean(ok)
        },
        // boot services (startup.rs)
        b"os_services" => list_from(crate::startup::available().iter().map(|s| mk_str(s)).collect()),
        b"os_service_enabled" => boolean(crate::startup::enabled(&path_arg(args, 0))),
        b"os_service_running" => boolean(crate::startup::running(&path_arg(args, 0))),
        b"os_service_set" => {
            let on = args.get(1).map(|&v| crate::lingrt::truthy(v)).unwrap_or(false);
            boolean(crate::startup::set(&path_arg(args, 0), on))
        },
        // daemon = a service the supervisor restarts when it crashes / exits
        b"os_service_daemon" => boolean(crate::startup::is_daemon(&path_arg(args, 0))),
        b"os_service_set_daemon" => {
            let on = args.get(1).map(|&v| crate::lingrt::truthy(v)).unwrap_or(false);
            boolean(crate::startup::set_daemon(&path_arg(args, 0), on))
        },
        // the idle lock: seconds since input, the timeout, "an app was parked for it"
        b"os_idle_secs" => num(crate::startup::idle_secs()),
        b"os_lock_after" => {
            if let Some(&v) = args.first() {
                crate::startup::set_lock_after(to_f64(v).max(0.0) as u64);
            }
            num(crate::startup::lock_after() as f64)
        },
        b"os_lock_requested" => boolean(crate::startup::take_lock_request()),
        b"os_apps_running" => list_from(apps_running().iter().map(|s| mk_str(s)).collect()),
        b"os_option" => {
            let v = args.get(1).map(|&v| crate::lingrt::truthy(v)).unwrap_or(false);
            match path_arg(args, 0).as_str() {
                "pause_audio" => unsafe { PAUSE_AUDIO = v },
                "perf" => unsafe { PERF = v },
                // the calling app: an Esc it does not use parks it instead of closing it
                "esc_minimizes" => {
                    if let Some(a) = active_app() {
                        a.esc_parks = v;
                    }
                },
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
        // Two-factor sign-in (fs/twofa.rs). The sign-in screen: os_login fails with
        // os_login_error() == os_2fa_needed_msg() when the password was right but a
        // code is needed; then os_login_code(user, password, code).
        b"os_login_code" => boolean(user_login_code(&path_arg(args, 0), &path_arg(args, 1), &path_arg(args, 2))),
        b"os_2fa_needed_msg" => mk_str(crate::fs::users::TWOFA_NEEDED),
        // Settings > Security, always for the signed-in user.
        b"os_2fa_enabled" => boolean(crate::fs::twofa::enabled(&os_user())),
        b"os_2fa_recovery_left" => num(crate::fs::twofa::recovery_left(&os_user()) as f64),
        // [secret, otpauth URI] to show as text + QR, or [] (already on / no storage)
        b"os_2fa_begin" => match crate::fs::twofa::begin(&os_user()) {
            Some((secret, uri)) => list_from(alloc::vec![mk_str(&secret), mk_str(&uri)]),
            None => list_from(Vec::new()),
        },
        // the one-time recovery codes once the app's code checks out, else []
        b"os_2fa_confirm" => match crate::fs::twofa::confirm(&os_user(), &path_arg(args, 0)) {
            Some(codes) => list_from(codes.iter().map(|c| mk_str(c)).collect()),
            None => list_from(Vec::new()),
        },
        b"os_2fa_disable" => boolean(crate::fs::twofa::disable(&os_user(), &path_arg(args, 0))),
        b"os_2fa_cancel" => {
            crate::fs::twofa::cancel(&os_user());
            UNIT
        },
        // codes depend on the time: is the clock from the network or a board clock?
        b"os_clock_set" => boolean(crate::clock::is_set()),
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
                .map(|(me, t)| mk_str(&alloc::format!("{}\t{}", if *me { "me" } else { "them" }, t.replace(['\t', '\n', '\r'], " "))))
                .collect(),
        ),
        b"msg_connected" => boolean(crate::drivers::messenger::selected_connected()),
        b"msg_submit" => {
            // the text as typed (path_arg would turn every backslash into a slash)
            let text = args.first().map(|&v| to_string(v)).unwrap_or_default();
            crate::drivers::messenger::submit(&text);
            UNIT
        },
        b"msg_status" => mk_str(crate::drivers::messenger::status()),
        b"os_uptime" => num(crate::arch::timer::now_ms() as f64 / 1000.0),
        // The shell's working directory ("" = root) -- the Terminal prompt.
        b"os_cwd" => mk_str(crate::fs::lingfs::cwd()),
        // Everything the Terminal's zsh-style prompt line shows, in one call:
        // "user\thost\tcwd\tsoul\tdirty" -- soul is "" outside a .miao shrine,
        // dirty is "1" while offerings sit on the altar (like a git dirty mark).
        b"os_shell_prompt" => {
            let user = crate::fs::lingfs::current_user();
            let host = host_name();
            let cwd = crate::fs::lingfs::cwd();
            let mut soul = String::new();
            let mut dirty = "";
            // Through vfs (card then lingfs): home folders live on the mounted
            // data volume when there is one, so a raw lingfs read would miss it.
            let soul_path = if cwd.is_empty() { String::from(".miao/SOUL") } else { alloc::format!("{cwd}/.miao/SOUL") };
            if let Some(bytes) = crate::vfs::read(&soul_path) {
                soul = String::from(core::str::from_utf8(&bytes).unwrap_or("").split_whitespace().nth(1).unwrap_or("main"));
                let altar = if cwd.is_empty() { String::from(".miao/altar") } else { alloc::format!("{cwd}/.miao/altar") };
                if crate::vfs::read(&altar).is_some_and(|a| !a.is_empty()) {
                    dirty = "1";
                }
            }
            mk_str(&alloc::format!("{user}\t{host}\t{cwd}\t{soul}\t{dirty}"))
        },
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
        // "" when files go to a disk; why they are in RAM this session otherwise
        b"os_storage_ram" => mk_str(&if crate::fs::blockdev::is_ram() { crate::fs::blockdev::ram_reason() } else { String::new() }),
        // the frame-rate cap (the display's refresh; 0 = unlimited)
        b"os_display_hz" => num(crate::drivers::framebuffer::present_hz() as f64),
        b"os_display_set_hz" => {
            let hz = a(0, 60.0).clamp(0.0, 500.0) as u32;
            crate::drivers::framebuffer::set_present_hz(hz);
            #[cfg(target_arch = "x86_64")]
            unsafe {
                PACE_NEXT_US = 0
            };
            UNIT
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
        // Horizon's vault (see vault.rs): every read and write takes the account password
        b"os_vault_status" => mk_str(&crate::vault::status()),
        b"os_vault_error" => mk_str(crate::vault::error()),
        b"os_vault_open" => {
            let pw = args.first().map(|&v| to_string(v)).unwrap_or_default();
            // "ok:" + the vault text, or "err:" + why
            match crate::vault::open(&pw) {
                Some(text) => mk_str(&alloc::format!("ok:{text}")),
                None => mk_str(&alloc::format!("err:{}", crate::vault::error())),
            }
        },
        b"os_vault_save" => {
            let pw = args.first().map(|&v| to_string(v)).unwrap_or_default();
            let text = args.get(1).map(|&v| to_string(v)).unwrap_or_default();
            boolean(crate::vault::save(&pw, &text))
        },
        b"os_totp" => {
            let secret = args.first().map(|&v| to_string(v)).unwrap_or_default();
            mk_str(&crate::vault::totp(&secret, crate::clock::unix_now()))
        },
        b"os_ssh" => boolean(crate::drivers::sshd::enabled()),
        // Horizon's cookie jar (cookies.rs): the page's site, clear, count, on/off
        b"os_cookie_site" => {
            let u = path_arg(args, 0);
            let host = u.split_once("://").map(|(_, r)| r).unwrap_or(&u).split(['/', '?', '#', ':']).next().unwrap_or("");
            crate::cookies::set_site(host);
            UNIT
        },
        b"os_cookies_clear" => {
            crate::cookies::clear();
            UNIT
        },
        b"os_cookies_count" => num(crate::cookies::count() as f64),
        b"os_cookies_enabled" => boolean(crate::cookies::enabled()),
        b"os_cookies_set_enabled" => {
            let on = args.first().map(|&v| crate::lingrt::truthy(v)).unwrap_or(true);
            crate::cookies::set_enabled(on);
            boolean(on)
        },
        // the shared credential store (creds.rs): the kernel knows which app is asking
        b"os_cred_set" => boolean(crate::creds::set(&path_arg(args, 0), &path_arg(args, 1))),
        b"os_cred_get" => mk_str(&crate::creds::get(&path_arg(args, 0)).unwrap_or_default()),
        b"os_cred_list" => list_from(crate::creds::list().iter().map(|s| mk_str(s)).collect()),
        b"os_cred_info" => mk_str(&crate::creds::info(&path_arg(args, 0))),
        b"os_cred_allow" => {
            let on = args.get(2).map(|&v| crate::lingrt::truthy(v)).unwrap_or(false);
            boolean(crate::creds::allow(&path_arg(args, 0), &path_arg(args, 1), on))
        },
        b"os_cred_forget" => boolean(crate::creds::forget(&path_arg(args, 0))),
        b"os_ssh_port" => num(crate::drivers::sshd::port() as f64),
        // the opt-in frame profiler (prof.rs): os_prof(frames) measures the next N presented frames and
        // prints one report (0 stops); os_prof_mark("name") names a section of the frame; the last
        // report text comes back from os_prof_report()
        b"os_prof" => {
            crate::prof::start(a(0, 300.0).max(0.0).min(600.0) as u32);
            UNIT
        },
        b"os_prof_mark" => {
            crate::prof::mark(&path_arg(args, 0));
            UNIT
        },
        b"os_prof_report" => mk_str(&crate::prof::last_report()),
        // the desktop's remote-control queue (remote.rs): "open:<app>", "close:<app>", "lock:screen"
        b"os_remote_poll" => mk_str(&crate::remote::poll().unwrap_or_default()),
        // Settings > Sign-in "2FA over SSH": ask for the authenticator code at SSH / SFTP login
        b"os_ssh_2fa" => boolean(crate::drivers::sshd::twofa_required()),
        b"os_ssh_2fa_set" => {
            let on = args.first().map(|&v| crate::lingrt::truthy(v)).unwrap_or(true);
            crate::drivers::sshd::set_twofa_required(on);
            boolean(crate::drivers::sshd::twofa_required())
        },
        b"os_ssh_port_set" => {
            let p = args.first().map(|&v| to_f64(v)).unwrap_or(0.0);
            if (1.0..=65535.0).contains(&p) {
                crate::drivers::sshd::set_port(p as u16);
            }
            num(crate::drivers::sshd::port() as f64)
        },
        b"os_ssh_set" => {
            let on = args.first().map(|&v| to_f64(v) != 0.0 && v != crate::lingrt::boolean(false)).unwrap_or(false);
            // startup::set persists the at-boot flag on BOTH arches (x86:
            // /services, Pi: setup.conf) and flips the live server -- the
            // x86 Network-tab toggle used to change only the running state.
            crate::startup::set("ssh", on);
            boolean(on)
        },
        b"os_login_error" => mk_str(crate::fs::users::last_verify_error()),
        b"os_user_reset_pending" => boolean(crate::fs::users::reset_pending(&path_arg(args, 0))),
        b"os_user_reset" => {
            let (n, p) = (path_arg(args, 0), path_arg(args, 1));
            boolean(user_reset(&n, &p))
        },
        // the system clipboard (one text buffer shared by every app)
        b"clipboard_set" => {
            crate::drivers::clipboard::set_str(&path_arg(args, 0));
            UNIT
        },
        b"clipboard_get" => mk_str(core::str::from_utf8(crate::drivers::clipboard::get()).unwrap_or("")),
        // the picture clipboard (drivers/clipboard.rs): Files, LingDiffusion and LingAnime share one picture
        b"clipboard_image_copy_file" => boolean(crate::drivers::clipboard::image_copy_file(&path_arg(args, 0), false)),
        b"clipboard_image_cut_file" => boolean(crate::drivers::clipboard::image_copy_file(&path_arg(args, 0), true)),
        b"clipboard_has_image" => boolean(crate::drivers::clipboard::image().is_some()),
        b"clipboard_image_w" => num(crate::drivers::clipboard::image().map(|c| c.w as f64).unwrap_or(0.0)),
        b"clipboard_image_h" => num(crate::drivers::clipboard::image().map(|c| c.h as f64).unwrap_or(0.0)),
        b"clipboard_image_path" => mk_str(crate::drivers::clipboard::image().map(|c| c.src.as_str()).unwrap_or("")),
        b"clipboard_image_is_cut" => boolean(crate::drivers::clipboard::image().is_some_and(|c| c.cut)),
        b"clipboard_image_seq" => num(crate::drivers::clipboard::image_seq() as f64),
        b"clipboard_image_paste_to" => mk_str(&crate::drivers::clipboard::image_paste_to(&path_arg(args, 0)).unwrap_or_default()),
        b"clipboard_image_paste_into" => mk_str(&crate::drivers::clipboard::image_paste_into(&path_arg(args, 0)).unwrap_or_default()),
        b"clipboard_image_take_cut" => mk_str(&crate::drivers::clipboard::image_take_cut()),
        b"clipboard_image_clear" => {
            crate::drivers::clipboard::image_clear();
            UNIT
        },
        b"fs_unique_path" => mk_str(&crate::drivers::clipboard::unique_path(&path_arg(args, 0))),
        b"is_image_file" =>boolean(crate::drivers::clipboard::is_image_name(&path_arg(args, 0))),
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
            // art_* (LingAnime's paint / 2.5D engine)
            if name.starts_with(b"art_") {
                if let Some(v) = crate::drivers::artpaint::dispatch(name, args) {
                    return Some(v);
                }
            }
            // diffusion_* (LingDiffusion: a GPU server on the LAN, or Stable Diffusion on this CPU)
            #[cfg(target_arch = "x86_64")]
            if name.starts_with(b"diffusion_") {
                if let Some(v) = crate::drivers::diffusion::dispatch(name, args) {
                    return Some(v);
                }
            }
            // llm_* (the language model) and daw_ai_available
            if let Some(v) = crate::drivers::llm::dispatch(name, args) {
                return Some(v);
            }
            if let Some(v) = crate::hostcompat::dispatch(name, args) {
                return Some(v);
            }
            return crate::drivers::g3d::dispatch(name, args);
        },
    })
}
