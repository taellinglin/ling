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
use crate::lingrt::{boolean, list_from, mk_str, num, str_of, to_f64, to_string, FALSE, UNIT};

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

fn font_load(path: &str) -> f64 {
    let Some(bytes) = crate::appfs::get(path) else { return -1.0 };
    match fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()) {
        Ok(fd) => {
            let f = fonts();
            f.push(Font { fd, cache: BTreeMap::new() });
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
}

fn present() {
    if let Some(w) = win() {
        framebuffer::blit_rgb(&w.buf, w.w, w.h);
        // Ctrl+Q (USB keyboard or serial) closes the app back to the LingOS
        // console — the way out until the app's own input is wired to HID.
        #[cfg(target_arch = "aarch64")]
        while let Some(c) = crate::poll_input() {
            if c == 0x11 {
                w.open = false;
            }
        }
        // x86: PS/2 keyboard — Ctrl+Q (chord byte 0x80|17) returns to the
        // desktop; printable keys, Enter and Backspace feed `text_poll`.
        #[cfg(target_arch = "x86_64")]
        {
            crate::arch::idt::poll_drain_8042();
            loop {
                let c = crate::drivers::keyboard::poll_char();
                match c {
                    0 => break,
                    0x91 => w.open = false,
                    b'\n' | b'\r' | 0x08 | 0x20..=0x7E => unsafe { (*(&raw mut TEXT)).push(c as char) },
                    _ => {},
                }
            }
        }
    }
}

/// Keys typed since the app last called `text_poll` (x86 PS/2 for now).
static mut TEXT: String = String::new();

/// Pointer state in window coordinates: (x, y, buttons bit0 L / bit1 R / bit2 M).
/// x86 reads the PS/2 mouse; boards without a pointer yet report it off-screen.
fn mouse_state() -> (f64, f64, u8) {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::idt::poll_drain_8042();
        let (ox, oy) = framebuffer::safe_margin();
        use crate::drivers::mouse;
        return ((mouse::x() - ox as i32) as f64, (mouse::y() - oy as i32) as f64, mouse::buttons());
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
    let mut items: Vec<(String, bool, usize)> = Vec::new();
    let base = String::from(dir);
    crate::appfs::list(dir, |name, is_dir| {
        if name.starts_with('.') {
            return;
        }
        let size = if is_dir { 0 } else { crate::appfs::get(&fs_join(&base, name)).map(|d| d.len()).unwrap_or(0) };
        items.push((String::from(name), is_dir, size));
    });
    items.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.to_lowercase().cmp(&y.0.to_lowercase())));
    let mut v = Vec::with_capacity(items.len() * 3);
    for (n, d, s) in items {
        v.push(mk_str(&n));
        v.push(num(d as i32 as f64));
        v.push(num(s as f64));
    }
    list_from(v)
}

// ── Dispatch ─────────────────────────────────────────────────────────────────

/// The app builtins this module implements; `None` for any other name.
pub fn dispatch(name: &[u8], args: &[u64]) -> Option<u64> {
    let a = |i: usize, d: f64| args.get(i).map(|&v| to_f64(v)).unwrap_or(d);
    let arg: Arg = &a;
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
        b"present" => {
            present();
            UNIT
        },
        b"get_width" => num(win().map(|w| w.w).unwrap_or(0) as f64),
        b"get_height" => num(win().map(|w| w.h).unwrap_or(0) as f64),
        b"screenshot" => UNIT,

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

        // Input: the PS/2 mouse + typed text on x86; Pi USB HID is the next step.
        b"mouse_x" => num(mouse_state().0),
        b"mouse_y" => num(mouse_state().1),
        b"mouse_down" => boolean(mouse_state().2 & 1 != 0),
        b"mouse_down_right" => boolean(mouse_state().2 & 2 != 0),
        b"mouse_down_middle" => boolean(mouse_state().2 & 4 != 0),
        b"mouse_scroll" | b"mouse_scroll_x" => num(0.0),
        b"key_down" | b"key_pressed" => FALSE,
        b"text_poll" | b"text_get" => mk_str(&core::mem::take(unsafe { &mut *(&raw mut TEXT) })),
        b"text_set" => UNIT,

        // Files.
        b"file_exists" => boolean(crate::appfs::exists(&path_arg(args, 0))),
        b"read_file" => match crate::appfs::get(&path_arg(args, 0)) {
            Some(bytes) => mk_str(core::str::from_utf8(bytes).unwrap_or("")),
            None => mk_str(""),
        },
        b"write_file" => FALSE,
        b"file_modified" => num(0.0),
        b"fs_cwd" => mk_str("."),
        b"fs_home" => mk_str("."),
        b"fs_join" => mk_str(&fs_join(&path_arg(args, 0), &path_arg(args, 1))),
        b"fs_parent" => mk_str(&fs_parent(&path_arg(args, 0))),
        // fs_* come from the host's daw API, which returns booleans as 1/0.
        b"fs_is_dir" => num(crate::appfs::is_dir(&path_arg(args, 0)) as i32 as f64),
        b"fs_list" => fs_list(&path_arg(args, 0)),
        b"fs_mkdir" | b"fs_delete" | b"fs_copy" | b"fs_reveal" => num(0.0),
        b"dlg_open" | b"dlg_save" | b"dlg_folder" => mk_str(""),

        _ => {
            let _ = str_of;
            return None;
        },
    })
}
