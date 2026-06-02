// src/gfx/raster.rs — software 2-D pixel rasteriser
// All buffers are row-major Vec<u32> with pixels as 0x00RRGGBB.

/// Fill a triangle using incremental edge evaluation (Pineda 1988).
/// For convex triangles we break early once we exit the span — roughly 2× faster
/// than scanning the full bounding box.
pub fn fill_triangle(
    buf:    &mut Vec<u32>,
    width:  usize,
    height: usize,
    color:  u32,
    x0: f32, y0: f32,
    x1: f32, y1: f32,
    x2: f32, y2: f32,
) {
    if width == 0 || height == 0 { return; }
    // Guard against NaN / Inf from behind-camera projection blowup.
    if !x0.is_finite() || !y0.is_finite()
    || !x1.is_finite() || !y1.is_finite()
    || !x2.is_finite() || !y2.is_finite() { return; }

    let min_x = x0.min(x1).min(x2).max(0.0) as i32;
    let max_x = x0.max(x1).max(x2).min(width  as f32 - 1.0) as i32;
    let min_y = y0.min(y1).min(y2).max(0.0) as i32;
    let max_y = y0.max(y1).max(y2).min(height as f32 - 1.0) as i32;
    if min_x > max_x || min_y > max_y { return; }

    // x-step deltas: adding 1 to fx changes each edge by -(Δy)
    let de0 = -(y1 - y0);
    let de1 = -(y2 - y1);
    let de2 = -(y0 - y2);

    for py in min_y..=max_y {
        let fy  = py as f32 + 0.5;
        let fx0 = min_x as f32 + 0.5;
        // Initial edge values at the leftmost column of this row
        let mut e0 = (x1 - x0) * (fy - y0) - (y1 - y0) * (fx0 - x0);
        let mut e1 = (x2 - x1) * (fy - y1) - (y2 - y1) * (fx0 - x1);
        let mut e2 = (x0 - x2) * (fy - y2) - (y0 - y2) * (fx0 - x2);

        let mut in_span = false;
        let row = py as usize * width;
        for px in min_x..=max_x {
            let inside = (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0)
                      || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0);
            if inside {
                buf[row + px as usize] = color;
                in_span = true;
            } else if in_span {
                break; // convex — span finished for this scanline
            }
            e0 += de0; e1 += de1; e2 += de2;
        }
    }
}

/// Gouraud-interpolated triangle with per-pixel luminance posterisation.
///
/// The three vertex colours (0x00RRGGBB) are interpolated with barycentric
/// weights, then each pixel's colour is banded into `bands` luminance levels
/// (chroma preserved). Smooth vertex normals + this fill = the holographic cel
/// look: no faceted edges, but crisp toon bands that follow the surface.
pub fn fill_triangle_gouraud(
    buf: &mut Vec<u32>, width: usize, height: usize,
    x0: f32, y0: f32, c0: u32,
    x1: f32, y1: f32, c1: u32,
    x2: f32, y2: f32, c2: u32,
    bands: u32,
) {
    if width == 0 || height == 0 { return; }
    if !x0.is_finite()||!y0.is_finite()||!x1.is_finite()||!y1.is_finite()||!x2.is_finite()||!y2.is_finite() { return; }

    let min_x = x0.min(x1).min(x2).max(0.0) as i32;
    let max_x = x0.max(x1).max(x2).min(width  as f32 - 1.0) as i32;
    let min_y = y0.min(y1).min(y2).max(0.0) as i32;
    let max_y = y0.max(y1).max(y2).min(height as f32 - 1.0) as i32;
    if min_x > max_x || min_y > max_y { return; }

    // unpack vertex colours to float rgb
    let (r0,g0,b0)=((c0>>16&0xFF)as f32,(c0>>8&0xFF)as f32,(c0&0xFF)as f32);
    let (r1,g1,b1)=((c1>>16&0xFF)as f32,(c1>>8&0xFF)as f32,(c1&0xFF)as f32);
    let (r2,g2,b2)=((c2>>16&0xFF)as f32,(c2>>8&0xFF)as f32,(c2&0xFF)as f32);

    let de0 = -(y1 - y0);
    let de1 = -(y2 - y1);
    let de2 = -(y0 - y2);
    let bandf = bands.max(2) as f32;

    for py in min_y..=max_y {
        let fy  = py as f32 + 0.5;
        let fx0 = min_x as f32 + 0.5;
        let mut e0 = (x1 - x0) * (fy - y0) - (y1 - y0) * (fx0 - x0);
        let mut e1 = (x2 - x1) * (fy - y1) - (y2 - y1) * (fx0 - x1);
        let mut e2 = (x0 - x2) * (fy - y2) - (y0 - y2) * (fx0 - x2);
        let mut in_span = false;
        let row = py as usize * width;
        for px in min_x..=max_x {
            let inside = (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0)
                      || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0);
            if inside {
                let tot = e0 + e1 + e2;
                if tot.abs() > 1e-6 {
                    // e0→v2, e1→v0, e2→v1
                    let w2 = e0 / tot; let w0 = e1 / tot; let w1 = e2 / tot;
                    let mut r = r0*w0 + r1*w1 + r2*w2;
                    let mut g = g0*w0 + g1*w1 + g2*w2;
                    let mut b = b0*w0 + b1*w1 + b2*w2;
                    // per-pixel luminance posterise (chroma preserved)
                    let lum = 0.299*r + 0.587*g + 0.114*b;
                    if lum > 1.0 {
                        let q = ((lum/255.0*bandf).floor() + 0.5)/bandf*255.0;
                        let k = (q/lum).clamp(0.0,4.0);
                        r*=k; g*=k; b*=k;
                    }
                    let rr=(r.min(255.0))as u32; let gg=(g.min(255.0))as u32; let bb=(b.min(255.0))as u32;
                    buf[row + px as usize] = (rr<<16)|(gg<<8)|bb;
                }
                in_span = true;
            } else if in_span { break; }
            e0 += de0; e1 += de1; e2 += de2;
        }
    }
}

/// Cohen-Sutherland clip then Bresenham integer line drawing.
/// Lines with one endpoint way off-screen (from behind-camera perspective blowup)
/// are clipped to the viewport before rasterisation so Bresenham never iterates
/// millions of steps.
pub fn draw_line(
    buf:    &mut Vec<u32>,
    width:  usize,
    height: usize,
    color:  u32,
    x0: f32, y0: f32,
    x1: f32, y1: f32,
) {
    if width == 0 || height == 0 { return; }
    if !x0.is_finite() || !y0.is_finite()
    || !x1.is_finite() || !y1.is_finite() { return; }

    let xmax = (width  - 1) as f32;
    let ymax = (height - 1) as f32;

    let (mut ax, mut ay, mut bx, mut by) = (x0, y0, x1, y1);
    if !cs_clip(&mut ax, &mut ay, &mut bx, &mut by, xmax, ymax) { return; }

    // Bresenham
    let mut x  = ax as i32;
    let mut y  = ay as i32;
    let x2     = bx as i32;
    let y2     = by as i32;
    let dx     = (x2 - x).abs();
    let dy     = -((y2 - y).abs());
    let sx: i32 = if x < x2 { 1 } else { -1 };
    let sy: i32 = if y < y2 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        if x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < height {
            buf[y as usize * width + x as usize] = color;
        }
        if x == x2 && y == y2 { break; }
        let e2 = 2 * err;
        if e2 >= dy { err += dy; x += sx; }
        if e2 <= dx { err += dx; y += sy; }
    }
}

// ── Cohen-Sutherland helpers ─────────────────────────────────────────────────

#[inline]
fn cs_code(x: f32, y: f32, xmax: f32, ymax: f32) -> u8 {
    let mut c = 0u8;
    if x < 0.0  { c |= 1; }
    if x > xmax { c |= 2; }
    if y < 0.0  { c |= 4; }
    if y > ymax { c |= 8; }
    c
}

/// Returns true if the segment (ax,ay)→(bx,by) has any part inside the viewport;
/// clips the endpoints in-place.  Returns false if entirely outside.
fn cs_clip(
    ax: &mut f32, ay: &mut f32,
    bx: &mut f32, by: &mut f32,
    xmax: f32, ymax: f32,
) -> bool {
    loop {
        let ca = cs_code(*ax, *ay, xmax, ymax);
        let cb = cs_code(*bx, *by, xmax, ymax);
        if ca | cb == 0 { return true; }   // both inside
        if ca & cb != 0 { return false; }  // both outside same half-plane
        let co = if ca != 0 { ca } else { cb };
        let dx = *bx - *ax;
        let dy = *by - *ay;
        let (nx, ny) = if co & 1 != 0 {
            // left edge  x = 0
            (0.0_f32, *ay + dy * (0.0 - *ax) / dx)
        } else if co & 2 != 0 {
            // right edge  x = xmax
            (xmax, *ay + dy * (xmax - *ax) / dx)
        } else if co & 4 != 0 {
            // top edge  y = 0
            (*ax + dx * (0.0 - *ay) / dy, 0.0_f32)
        } else {
            // bottom edge  y = ymax
            (*ax + dx * (ymax - *ay) / dy, ymax)
        };
        if co == ca { *ax = nx; *ay = ny; } else { *bx = nx; *by = ny; }
    }
}
