//! Desktop icons: the Applications menu's category and app icons, and the
//! top bar's tray icons (settings, network, sound).
//!
//! Each icon is drawn from a few geometric shapes on a 16-unit grid -- circles,
//! rings, rounded rectangles, line segments, triangles -- evaluated per pixel
//! with 4x4 supersampling and blended into the back buffer, so they stay smooth
//! at any size (16 px in the menu, scaled with the UI's DPI). App icons sit on
//! a coloured rounded tile with a white glyph; category and tray icons are the
//! bare glyph in the caller's colour.

use crate::drivers::framebuffer;

// ── Ids ──────────────────────────────────────────────────────────────────────

pub const NOTE: u8 = 1;
pub const PICTURE: u8 = 2;
pub const PLAY: u8 = 3;
pub const GAMEPAD: u8 = 4;
pub const CUBE: u8 = 5;
pub const GLOBE: u8 = 6;
pub const PAGE_W: u8 = 7;
pub const BUBBLE: u8 = 8;
pub const FOLDER: u8 = 9;
pub const PROMPT: u8 = 10;
pub const PENCIL_PAGE: u8 = 11;
pub const COIN: u8 = 12;
pub const BOX: u8 = 13;
pub const GEAR: u8 = 14;
pub const INFO: u8 = 15;
pub const LOGOUT: u8 = 16;
pub const RESTART: u8 = 17;
pub const POWER: u8 = 18;
pub const CODE: u8 = 19;
pub const TOOLBOX: u8 = 20;
pub const SPEAKER: u8 = 21;
pub const SPEAKER_MUTE: u8 = 22;
pub const WIFI: u8 = 23;
pub const WIFI_OFF: u8 = 24;
pub const SHIELD: u8 = 25;

// ── Shapes (in icon units, 0..16) ────────────────────────────────────────────

fn circle(x: f32, y: f32, cx: f32, cy: f32, r: f32) -> bool {
    let (dx, dy) = (x - cx, y - cy);
    dx * dx + dy * dy <= r * r
}

fn ring(x: f32, y: f32, cx: f32, cy: f32, r: f32, w: f32) -> bool {
    let (dx, dy) = (x - cx, y - cy);
    let d = dx * dx + dy * dy;
    d <= r * r && d >= (r - w) * (r - w)
}

fn rect(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
    x >= x0 && x <= x1 && y >= y0 && y <= y1
}

fn rrect(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> bool {
    if !rect(x, y, x0, y0, x1, y1) {
        return false;
    }
    let cx = x.clamp(x0 + r, x1 - r);
    let cy = y.clamp(y0 + r, y1 - r);
    circle(x, y, cx, cy, r)
}

/// A line segment `w` units thick with round ends.
fn seg(x: f32, y: f32, ax: f32, ay: f32, bx: f32, by: f32, w: f32) -> bool {
    let (vx, vy) = (bx - ax, by - ay);
    let len2 = vx * vx + vy * vy;
    let t = if len2 > 0.0 { (((x - ax) * vx + (y - ay) * vy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    let (px, py) = (ax + vx * t - x, ay + vy * t - y);
    px * px + py * py <= w * w / 4.0
}

fn tri(x: f32, y: f32, a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let s = |p: (f32, f32), q: (f32, f32)| (x - q.0) * (p.1 - q.1) - (p.0 - q.0) * (y - q.1);
    let (d1, d2, d3) = (s(a, b), s(b, c), s(c, a));
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// The upper quarter of a ring (the Wi-Fi arcs).
fn arc_up(x: f32, y: f32, cx: f32, cy: f32, r: f32, w: f32) -> bool {
    let (dx, dy) = (x - cx, y - cy);
    dy < 0.0 && dx.abs() <= -dy * 1.05 && ring(x, y, cx, cy, r, w)
}

fn ellipse_ring(x: f32, y: f32, cx: f32, cy: f32, a: f32, b: f32, w: f32) -> bool {
    let (dx, dy) = (x - cx, y - cy);
    let outer = dx * dx / (a * a) + dy * dy / (b * b) <= 1.0;
    let (ai, bi) = ((a - w).max(0.1), (b - w).max(0.1));
    let inner = dx * dx / (ai * ai) + dy * dy / (bi * bi) < 1.0;
    outer && !inner
}

// ── Glyphs ───────────────────────────────────────────────────────────────────

/// The colour of glyph `id` at (x, y), or None for transparent. `fg` is the
/// glyph, `hole` the colour knocked out of it (the tile, or the background).
fn sample(id: u8, x: f32, y: f32, fg: u32, hole: u32) -> Option<u32> {
    let mut c = None;
    let mut put = |on: bool, col: u32| {
        if on {
            c = Some(col);
        }
    };
    match id {
        NOTE => {
            put(circle(x, y, 6.0, 12.0, 2.6), fg);
            put(rect(x, y, 7.6, 3.0, 9.0, 12.0), fg);
            put(seg(x, y, 8.3, 3.2, 12.6, 6.2, 1.8), fg);
        },
        PICTURE => {
            put(rrect(x, y, 2.5, 3.5, 13.5, 12.5, 1.5), fg);
            put(rect(x, y, 4.0, 5.0, 12.0, 11.0), hole);
            put(tri(x, y, (4.0, 11.0), (7.5, 6.5), (11.0, 11.0)), fg);
            put(circle(x, y, 10.5, 7.0, 1.3), fg);
        },
        PLAY => put(tri(x, y, (5.5, 3.5), (5.5, 12.5), (12.5, 8.0)), fg),
        GAMEPAD => {
            put(rrect(x, y, 1.5, 5.0, 14.5, 11.5, 3.2), fg);
            put(rect(x, y, 3.6, 7.6, 7.4, 8.9), hole);
            put(rect(x, y, 4.9, 6.3, 6.1, 10.2), hole);
            put(circle(x, y, 10.6, 7.3, 1.0), hole);
            put(circle(x, y, 12.2, 9.2, 1.0), hole);
        },
        CUBE => {
            let (t, r, b, l, m) = ((8.0, 2.0), (13.5, 5.0), (8.0, 8.0), (2.5, 5.0), (8.0, 14.5));
            put(tri(x, y, t, r, b) || tri(x, y, t, l, b), fg);
            put(tri(x, y, l, b, (2.5, 11.5)) || tri(x, y, b, m, (2.5, 11.5)), mix(fg, hole, 35));
            put(tri(x, y, r, b, (13.5, 11.5)) || tri(x, y, b, m, (13.5, 11.5)), mix(fg, hole, 60));
        },
        GLOBE => {
            put(ring(x, y, 8.0, 8.0, 6.3, 1.4), fg);
            put(ellipse_ring(x, y, 8.0, 8.0, 2.9, 6.3, 1.3), fg);
            put(rect(x, y, 2.0, 7.4, 14.0, 8.6), fg);
        },
        PAGE_W => {
            put(rrect(x, y, 3.0, 2.0, 13.0, 14.0, 1.5), fg);
            let w = 1.3;
            let on = seg(x, y, 4.8, 5.0, 6.2, 11.0, w)
                || seg(x, y, 6.2, 11.0, 8.0, 7.0, w)
                || seg(x, y, 8.0, 7.0, 9.8, 11.0, w)
                || seg(x, y, 9.8, 11.0, 11.2, 5.0, w);
            put(on, hole);
        },
        BUBBLE => {
            put(rrect(x, y, 1.5, 2.5, 14.5, 11.0, 3.0), fg);
            put(tri(x, y, (3.5, 10.0), (3.5, 14.5), (8.0, 10.0)), fg);
            put(circle(x, y, 5.0, 6.8, 1.1) || circle(x, y, 8.0, 6.8, 1.1) || circle(x, y, 11.0, 6.8, 1.1), hole);
        },
        FOLDER => {
            put(rrect(x, y, 1.5, 3.0, 7.0, 6.0, 1.0), fg);
            put(rrect(x, y, 1.5, 4.5, 14.5, 13.0, 1.5), fg);
            put(rect(x, y, 1.5, 6.2, 14.5, 6.9), hole);
        },
        PROMPT => {
            put(seg(x, y, 3.5, 4.5, 7.5, 8.0, 1.8) || seg(x, y, 7.5, 8.0, 3.5, 11.5, 1.8), fg);
            put(rect(x, y, 8.5, 11.0, 13.0, 12.6), fg);
        },
        PENCIL_PAGE => {
            put(rrect(x, y, 2.5, 1.5, 11.5, 14.5, 1.5), fg);
            put(rect(x, y, 4.5, 4.5, 9.5, 5.3) || rect(x, y, 4.5, 7.0, 9.5, 7.8) || rect(x, y, 4.5, 9.5, 7.5, 10.3), hole);
            put(seg(x, y, 14.0, 6.0, 8.5, 13.0, 2.4), hole);
            put(seg(x, y, 14.0, 6.0, 8.8, 12.6, 1.4), fg);
        },
        COIN => {
            put(circle(x, y, 8.0, 8.0, 6.5), fg);
            put(ring(x, y, 8.0, 8.0, 5.0, 0.9), hole);
            // an "L" for Ling
            put(rect(x, y, 6.3, 4.8, 7.8, 11.0) || rect(x, y, 6.3, 9.6, 10.2, 11.0), hole);
        },
        BOX => {
            put(rect(x, y, 2.5, 6.0, 13.5, 13.5), fg);
            put(rrect(x, y, 1.5, 3.0, 14.5, 6.5, 1.0), fg);
            put(rect(x, y, 2.5, 6.5, 13.5, 7.3), hole);
            put(rect(x, y, 7.2, 3.0, 8.8, 13.5), hole);
        },
        GEAR => {
            let mut teeth = false;
            for k in 0..8 {
                let a = k as f32 * core::f32::consts::FRAC_PI_4;
                let (s, co) = (libm_sin(a), libm_cos(a));
                teeth |= seg(x, y, 8.0 + co * 3.5, 8.0 + s * 3.5, 8.0 + co * 6.3, 8.0 + s * 6.3, 2.4);
            }
            put(teeth || circle(x, y, 8.0, 8.0, 5.0), fg);
            put(circle(x, y, 8.0, 8.0, 2.1), hole);
        },
        INFO => {
            put(circle(x, y, 8.0, 8.0, 6.5), fg);
            put(circle(x, y, 8.0, 4.8, 1.1), hole);
            put(rect(x, y, 7.1, 6.8, 8.9, 11.8), hole);
        },
        LOGOUT => {
            put(rect(x, y, 2.5, 2.5, 4.2, 13.5) || rect(x, y, 2.5, 2.5, 8.0, 4.0) || rect(x, y, 2.5, 12.0, 8.0, 13.5), fg);
            put(seg(x, y, 6.5, 8.0, 13.5, 8.0, 1.7), fg);
            put(seg(x, y, 10.8, 5.0, 13.8, 8.0, 1.7) || seg(x, y, 13.8, 8.0, 10.8, 11.0, 1.7), fg);
        },
        RESTART => {
            let gap = x > 8.0 && y < 7.5;
            put(ring(x, y, 8.0, 8.5, 5.6, 1.7) && !gap, fg);
            put(tri(x, y, (8.0, 0.8), (8.0, 6.2), (11.8, 3.5)), fg);
        },
        POWER => {
            let gap = rect(x, y, 5.6, 0.0, 10.4, 7.0);
            put(ring(x, y, 8.0, 8.8, 5.8, 1.8) && !gap, fg);
            put(seg(x, y, 8.0, 2.0, 8.0, 8.0, 1.9), fg);
        },
        CODE => {
            let w = 1.6;
            put(seg(x, y, 5.0, 4.0, 1.8, 8.0, w) || seg(x, y, 1.8, 8.0, 5.0, 12.0, w), fg);
            put(seg(x, y, 11.0, 4.0, 14.2, 8.0, w) || seg(x, y, 14.2, 8.0, 11.0, 12.0, w), fg);
            put(seg(x, y, 9.3, 3.0, 6.7, 13.0, w), fg);
        },
        TOOLBOX => {
            put(rrect(x, y, 1.5, 5.5, 14.5, 13.5, 1.5), fg);
            put(ring(x, y, 8.0, 5.5, 3.2, 1.3) && y < 5.5, fg);
            put(rect(x, y, 1.5, 8.6, 14.5, 9.4), hole);
            put(rect(x, y, 7.0, 8.0, 9.0, 10.2), fg);
        },
        SPEAKER | SPEAKER_MUTE => {
            put(rect(x, y, 2.0, 6.0, 5.0, 10.0), fg);
            put(tri(x, y, (4.0, 6.0), (9.0, 2.5), (9.0, 13.5)) || tri(x, y, (4.0, 10.0), (4.0, 6.0), (9.0, 13.5)), fg);
            if id == SPEAKER {
                let right = x > 10.0;
                put(right && ring(x, y, 9.0, 8.0, 3.4, 1.3), fg);
                put(right && ring(x, y, 9.0, 8.0, 6.2, 1.3), fg);
            } else {
                put(seg(x, y, 10.5, 5.5, 14.5, 10.5, 1.5) || seg(x, y, 14.5, 5.5, 10.5, 10.5, 1.5), fg);
            }
        },
        WIFI | WIFI_OFF => {
            let a = if id == WIFI { fg } else { mix(fg, hole, 55) };
            put(circle(x, y, 8.0, 13.0, 1.6), a);
            put(arc_up(x, y, 8.0, 13.0, 5.4, 1.6), a);
            put(arc_up(x, y, 8.0, 13.0, 9.0, 1.6), a);
            put(arc_up(x, y, 8.0, 13.0, 12.6, 1.6), a);
            if id == WIFI_OFF {
                put(seg(x, y, 2.5, 2.5, 13.5, 13.5, 1.6), fg);
            }
        },
        SHIELD => {
            put(tri(x, y, (2.5, 3.0), (13.5, 3.0), (8.0, 15.0)) || rrect(x, y, 2.5, 1.5, 13.5, 8.0, 1.5), fg);
        },
        _ => put(rrect(x, y, 3.0, 3.0, 13.0, 13.0, 2.0), fg),
    }
    c
}

/// `pct`% of the way from `a` to `b`.
pub fn mix(a: u32, b: u32, pct: u32) -> u32 {
    let mut out = 0;
    for ch in 0..3 {
        let ca = (a >> (ch * 8)) & 0xFF;
        let cb = (b >> (ch * 8)) & 0xFF;
        out |= ((ca * (100 - pct) + cb * pct) / 100) << (ch * 8);
    }
    out
}

fn libm_sin(a: f32) -> f32 {
    // the eight gear angles only: exact values, no libm needed
    let k = (a / core::f32::consts::FRAC_PI_4 + 0.5) as i32 & 7;
    const S: [f32; 8] = [0.0, 0.7071, 1.0, 0.7071, 0.0, -0.7071, -1.0, -0.7071];
    S[k as usize]
}

fn libm_cos(a: f32) -> f32 {
    libm_sin(a + core::f32::consts::FRAC_PI_2)
}

/// Paint glyph `id` into the `size`-pixel square at (x, y): `fg` for the
/// glyph, `hole` for its cut-outs (pass the colour underneath).
pub fn draw_glyph(id: u8, x: u32, y: u32, size: u32, fg: u32, hole: u32) {
    let s = size.max(8);
    let unit = 16.0 / s as f32;
    for py in 0..s {
        for px in 0..s {
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..4 {
                for sx in 0..4 {
                    let ux = (px as f32 + (sx as f32 + 0.5) / 4.0) * unit;
                    let uy = (py as f32 + (sy as f32 + 0.5) / 4.0) * unit;
                    if let Some(c) = sample(id, ux, uy, fg, hole) {
                        r += (c >> 16) & 0xFF;
                        g += (c >> 8) & 0xFF;
                        b += c & 0xFF;
                        n += 1;
                    }
                }
            }
            if n == 0 {
                continue;
            }
            let c = ((r / n) << 16) | ((g / n) << 8) | (b / n);
            if n == 16 {
                framebuffer::back_fill_rect(x + px, y + py, 1, 1, c);
            } else {
                framebuffer::back_blend_rect(x + px, y + py, 1, 1, c, n * 255 / 16);
            }
        }
    }
}

/// An app icon: a rounded `tile`-coloured square with the glyph in white.
pub fn draw_app(id: u8, tile: u32, x: u32, y: u32, size: u32) {
    let r = (size / 4).max(3);
    framebuffer::back_fill_rounded_rect(x, y, size, size, r, tile);
    let inset = size / 6;
    draw_glyph(id, x + inset, y + inset, size - 2 * inset, 0xFFFFFF, tile);
}
