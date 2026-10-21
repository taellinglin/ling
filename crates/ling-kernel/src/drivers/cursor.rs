//! The LingOS pointer: a tailless triangle whose bottom-right edge is scooped
//! inward by a curve, filled with a colour that cycles slowly through the hues
//! and outlined dark. Shared by the x86 desktop (drawn into the back buffer by
//! `ling_kernel_wm_draw_cursor`) and the app runtime (the Pi desktop and every
//! compiled `.ling` app get it from `lingapp::present`).

/// Bounding box of the shape, hot spot at (0, 0).
pub const W: i32 = 11;
pub const H: i32 = 16;

/// Is the centre of pixel (x, y) inside the shape? Tip at (0, 0), left edge
/// x = 0, upper-right edge the diagonal y = x, and the bottom-right edge a line
/// from (0, 15) to (11, 11) bowed 3.5 px toward the tip by a parabola.
fn inside(x: i32, y: i32) -> bool {
    if x < 0 || y < 0 || x >= W || y >= H {
        return false;
    }
    let (xf, yf) = (x as f32 + 0.5, y as f32 + 0.5);
    if yf < xf {
        return false;
    }
    let t = xf / 11.0;
    let edge = 15.0 - 4.0 * t - 3.5 * 4.0 * t * (1.0 - t);
    yf <= edge
}

/// 0 = outside, 1 = outline, 2 = fill, for pixel (dx, dy) relative to the hot spot.
pub fn classify(dx: i32, dy: i32) -> u8 {
    if !inside(dx, dy) {
        return 0;
    }
    let rim = !inside(dx - 1, dy) || !inside(dx + 1, dy) || !inside(dx, dy - 1) || !inside(dx, dy + 1);
    if rim {
        1
    } else {
        2
    }
}

pub const OUTLINE: u32 = 0x1A1423;

/// The fill colour at time `now_ms`: a full trip round the hue circle every
/// 6 seconds, slightly desaturated so it stays readable on any background.
pub fn fill_color(now_ms: u64) -> u32 {
    let hue = ((now_ms % 6000) * 360 / 6000) as u32; // 0..360
    let (s, v) = (200u32, 255u32); // saturation, value in 0..255
    let sector = hue / 60;
    let f = hue % 60;
    let p = v * (255 - s) / 255;
    let q = v * (255 * 60 - s * f) / (255 * 60);
    let t = v * (255 * 60 - s * (60 - f)) / (255 * 60);
    let (r, g, b) = match sector {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    (r.min(255) << 16) | (g.min(255) << 8) | b.min(255)
}
