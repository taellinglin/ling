// src/gfx/raster.rs — software 2-D pixel rasteriser
// All buffers are row-major Vec<u32> with pixels as 0x00RRGGBB.

/// Fill a triangle using incremental edge evaluation (Pineda 1988).
/// For convex triangles we break early once we exit the span — roughly 2× faster
/// than scanning the full bounding box.
pub fn fill_triangle(
    buf: &mut Vec<u32>,
    width: usize,
    height: usize,
    color: u32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
) {
    if width == 0 || height == 0 {
        return;
    }
    // Guard against NaN / Inf from behind-camera projection blowup.
    if !x0.is_finite()
        || !y0.is_finite()
        || !x1.is_finite()
        || !y1.is_finite()
        || !x2.is_finite()
        || !y2.is_finite()
    {
        return;
    }

    let min_x = x0.min(x1).min(x2).max(0.0) as i32;
    let max_x = x0.max(x1).max(x2).min(width as f32 - 1.0) as i32;
    let min_y = y0.min(y1).min(y2).max(0.0) as i32;
    let max_y = y0.max(y1).max(y2).min(height as f32 - 1.0) as i32;
    if min_x > max_x || min_y > max_y {
        return;
    }

    // x-step deltas: adding 1 to fx changes each edge by -(Δy)
    let de0 = -(y1 - y0);
    let de1 = -(y2 - y1);
    let de2 = -(y0 - y2);

    for py in min_y..=max_y {
        let fy = py as f32 + 0.5;
        let fx0 = min_x as f32 + 0.5;
        // Initial edge values at the leftmost column of this row
        let mut e0 = (x1 - x0) * (fy - y0) - (y1 - y0) * (fx0 - x0);
        let mut e1 = (x2 - x1) * (fy - y1) - (y2 - y1) * (fx0 - x1);
        let mut e2 = (x0 - x2) * (fy - y2) - (y0 - y2) * (fx0 - x2);

        let mut in_span = false;
        let row = py as usize * width;
        for px in min_x..=max_x {
            let inside =
                (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0) || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0);
            if inside {
                buf[row + px as usize] = color;
                in_span = true;
            } else if in_span {
                break; // convex — span finished for this scanline
            }
            e0 += de0;
            e1 += de1;
            e2 += de2;
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
    buf: &mut Vec<u32>,
    width: usize,
    height: usize,
    x0: f32,
    y0: f32,
    c0: u32,
    x1: f32,
    y1: f32,
    c1: u32,
    x2: f32,
    y2: f32,
    c2: u32,
    bands: u32,
) {
    if width == 0 || height == 0 {
        return;
    }
    if !x0.is_finite()
        || !y0.is_finite()
        || !x1.is_finite()
        || !y1.is_finite()
        || !x2.is_finite()
        || !y2.is_finite()
    {
        return;
    }

    let min_x = x0.min(x1).min(x2).max(0.0) as i32;
    let max_x = x0.max(x1).max(x2).min(width as f32 - 1.0) as i32;
    let min_y = y0.min(y1).min(y2).max(0.0) as i32;
    let max_y = y0.max(y1).max(y2).min(height as f32 - 1.0) as i32;
    if min_x > max_x || min_y > max_y {
        return;
    }

    // unpack vertex colours to float rgb
    let (r0, g0, b0) = (
        (c0 >> 16 & 0xFF) as f32,
        (c0 >> 8 & 0xFF) as f32,
        (c0 & 0xFF) as f32,
    );
    let (r1, g1, b1) = (
        (c1 >> 16 & 0xFF) as f32,
        (c1 >> 8 & 0xFF) as f32,
        (c1 & 0xFF) as f32,
    );
    let (r2, g2, b2) = (
        (c2 >> 16 & 0xFF) as f32,
        (c2 >> 8 & 0xFF) as f32,
        (c2 & 0xFF) as f32,
    );

    let de0 = -(y1 - y0);
    let de1 = -(y2 - y1);
    let de2 = -(y0 - y2);
    let bandf = bands.max(2) as f32;

    for py in min_y..=max_y {
        let fy = py as f32 + 0.5;
        let fx0 = min_x as f32 + 0.5;
        let mut e0 = (x1 - x0) * (fy - y0) - (y1 - y0) * (fx0 - x0);
        let mut e1 = (x2 - x1) * (fy - y1) - (y2 - y1) * (fx0 - x1);
        let mut e2 = (x0 - x2) * (fy - y2) - (y0 - y2) * (fx0 - x2);
        let mut in_span = false;
        let row = py as usize * width;
        for px in min_x..=max_x {
            let inside =
                (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0) || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0);
            if inside {
                let tot = e0 + e1 + e2;
                if tot.abs() > 1e-6 {
                    // e0→v2, e1→v0, e2→v1
                    let w2 = e0 / tot;
                    let w0 = e1 / tot;
                    let w1 = e2 / tot;
                    let mut r = r0 * w0 + r1 * w1 + r2 * w2;
                    let mut g = g0 * w0 + g1 * w1 + g2 * w2;
                    let mut b = b0 * w0 + b1 * w1 + b2 * w2;
                    // per-pixel luminance posterise (chroma preserved)
                    let lum = 0.299 * r + 0.587 * g + 0.114 * b;
                    if lum > 1.0 {
                        let q = ((lum / 255.0 * bandf).floor() + 0.5) / bandf * 255.0;
                        let k = (q / lum).clamp(0.0, 4.0);
                        r *= k;
                        g *= k;
                        b *= k;
                    }
                    let rr = (r.min(255.0)) as u32;
                    let gg = (g.min(255.0)) as u32;
                    let bb = (b.min(255.0)) as u32;
                    buf[row + px as usize] = (rr << 16) | (gg << 8) | bb;
                }
                in_span = true;
            } else if in_span {
                break;
            }
            e0 += de0;
            e1 += de1;
            e2 += de2;
        }
    }
}

/// Cohen-Sutherland clip then Bresenham integer line drawing.
/// Lines with one endpoint way off-screen (from behind-camera perspective blowup)
/// are clipped to the viewport before rasterisation so Bresenham never iterates
/// millions of steps.
pub fn draw_line(
    buf: &mut Vec<u32>,
    width: usize,
    height: usize,
    color: u32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
) {
    if width == 0 || height == 0 {
        return;
    }
    if !x0.is_finite() || !y0.is_finite() || !x1.is_finite() || !y1.is_finite() {
        return;
    }

    let xmax = (width - 1) as f32;
    let ymax = (height - 1) as f32;

    let (mut ax, mut ay, mut bx, mut by) = (x0, y0, x1, y1);
    if !cs_clip(&mut ax, &mut ay, &mut bx, &mut by, xmax, ymax) {
        return;
    }

    // Bresenham
    let mut x = ax as i32;
    let mut y = ay as i32;
    let x2 = bx as i32;
    let y2 = by as i32;
    let dx = (x2 - x).abs();
    let dy = -((y2 - y).abs());
    let sx: i32 = if x < x2 { 1 } else { -1 };
    let sy: i32 = if y < y2 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        if x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < height {
            buf[y as usize * width + x as usize] = color;
        }
        if x == x2 && y == y2 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

// ── Cohen-Sutherland helpers ─────────────────────────────────────────────────

#[inline]
fn cs_code(x: f32, y: f32, xmax: f32, ymax: f32) -> u8 {
    let mut c = 0u8;
    if x < 0.0 {
        c |= 1;
    }
    if x > xmax {
        c |= 2;
    }
    if y < 0.0 {
        c |= 4;
    }
    if y > ymax {
        c |= 8;
    }
    c
}

/// Returns true if the segment (ax,ay)→(bx,by) has any part inside the viewport;
/// clips the endpoints in-place.  Returns false if entirely outside.
fn cs_clip(ax: &mut f32, ay: &mut f32, bx: &mut f32, by: &mut f32, xmax: f32, ymax: f32) -> bool {
    loop {
        let ca = cs_code(*ax, *ay, xmax, ymax);
        let cb = cs_code(*bx, *by, xmax, ymax);
        if ca | cb == 0 {
            return true;
        } // both inside
        if ca & cb != 0 {
            return false;
        } // both outside same half-plane
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
        if co == ca {
            *ax = nx;
            *ay = ny;
        } else {
            *bx = nx;
            *by = ny;
        }
    }
}

// ── Anti-aliased primitives (used by the vector-font renderer) ───────────────

/// Alpha-blend `color` over the pixel at (x,y) with `cov` ∈ [0,1].
/// `additive` matches `set_blend(1)`: source is added instead of mixed.
#[inline]
pub fn blend_pixel(
    buf: &mut [u32],
    w: usize,
    h: usize,
    x: i32,
    y: i32,
    color: u32,
    cov: f32,
    additive: bool,
) {
    if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
        return;
    }
    let cov = cov.clamp(0.0, 1.0);
    if cov <= 0.0 {
        return;
    }
    let idx = y as usize * w + x as usize;
    let dst = buf[idx];
    let sr = ((color >> 16) & 0xFF) as f32;
    let sg = ((color >> 8) & 0xFF) as f32;
    let sb = (color & 0xFF) as f32;
    let dr = ((dst >> 16) & 0xFF) as f32;
    let dg = ((dst >> 8) & 0xFF) as f32;
    let db = (dst & 0xFF) as f32;
    let (nr, ng, nb) = if additive {
        (
            (dr + sr * cov).min(255.0),
            (dg + sg * cov).min(255.0),
            (db + sb * cov).min(255.0),
        )
    } else {
        (
            sr * cov + dr * (1.0 - cov),
            sg * cov + dg * (1.0 - cov),
            sb * cov + db * (1.0 - cov),
        )
    };
    buf[idx] = ((nr as u32) << 16) | ((ng as u32) << 8) | (nb as u32);
}

/// Xiaolin-Wu anti-aliased line. `additive` selects the blend mode.
pub fn draw_line_aa(
    buf: &mut [u32],
    w: usize,
    h: usize,
    color: u32,
    additive: bool,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
) {
    if w == 0 || h == 0 {
        return;
    }
    if !x0.is_finite() || !y0.is_finite() || !x1.is_finite() || !y1.is_finite() {
        return;
    }

    let (mut x0, mut y0, mut x1, mut y1) = (x0, y0, x1, y1);
    let steep = (y1 - y0).abs() > (x1 - x0).abs();
    if steep {
        std::mem::swap(&mut x0, &mut y0);
        std::mem::swap(&mut x1, &mut y1);
    }
    if x0 > x1 {
        std::mem::swap(&mut x0, &mut x1);
        std::mem::swap(&mut y0, &mut y1);
    }
    let dx = x1 - x0;
    let dy = y1 - y0;
    let grad = if dx.abs() < 1e-6 { 1.0 } else { dy / dx };

    let plot = |buf: &mut [u32], a: i32, b: i32, c: f32| {
        if steep {
            blend_pixel(buf, w, h, b, a, color, c, additive);
        } else {
            blend_pixel(buf, w, h, a, b, color, c, additive);
        }
    };

    // endpoints
    let mut inter_y = y0 + grad * (x0.round() - x0) + grad;
    let xend0 = x0.round();
    let xend1 = x1.round();

    plot(buf, xend0 as i32, y0.floor() as i32, 1.0);
    plot(buf, xend1 as i32, y1.floor() as i32, 1.0);

    let xpxl1 = xend0 as i32;
    let xpxl2 = xend1 as i32;
    let mut x = xpxl1 + 1;
    while x < xpxl2 {
        let fy = inter_y.floor();
        let frac = inter_y - fy;
        plot(buf, x, fy as i32, 1.0 - frac);
        plot(buf, x, fy as i32 + 1, frac);
        inter_y += grad;
        x += 1;
    }
}

/// Coverage-based scanline fill of closed `polylines` (screen space, y-down)
/// using the non-zero winding rule, with 4× vertical supersampling and analytic
/// horizontal span coverage — smooth filled glyph interiors without aliasing.
pub fn fill_contours_aa(
    buf: &mut [u32],
    w: usize,
    h: usize,
    color: u32,
    additive: bool,
    polylines: &[Vec<[f32; 2]>],
) {
    if w == 0 || h == 0 {
        return;
    }
    // Collect directed edges, skipping horizontal ones.
    struct Edge {
        y_lo: f32,
        y_hi: f32,
        x_at_lo: f32,
        dxdy: f32,
        dir: i32,
    }
    let mut edges: Vec<Edge> = Vec::new();
    let (mut ymin, mut ymax) = (f32::MAX, f32::MIN);
    let (mut xmin, mut xmax) = (f32::MAX, f32::MIN);
    for pl in polylines {
        for seg in pl.windows(2) {
            let (a, b) = (seg[0], seg[1]);
            if !a[0].is_finite() || !a[1].is_finite() || !b[0].is_finite() || !b[1].is_finite() {
                continue;
            }
            xmin = xmin.min(a[0]).min(b[0]);
            xmax = xmax.max(a[0]).max(b[0]);
            ymin = ymin.min(a[1]).min(b[1]);
            ymax = ymax.max(a[1]).max(b[1]);
            if (a[1] - b[1]).abs() < 1e-9 {
                continue;
            }
            let dir = if b[1] > a[1] { 1 } else { -1 };
            let (lo, hi) = if a[1] < b[1] { (a, b) } else { (b, a) };
            let dxdy = (hi[0] - lo[0]) / (hi[1] - lo[1]);
            edges.push(Edge { y_lo: lo[1], y_hi: hi[1], x_at_lo: lo[0], dxdy, dir });
        }
    }
    if edges.is_empty() {
        return;
    }

    let y_start = (ymin.floor().max(0.0)) as i32;
    let y_end = (ymax.ceil().min(h as f32)) as i32;
    let x_start = (xmin.floor().max(0.0)) as i32;
    let x_end = (xmax.ceil().min(w as f32)) as i32;
    if x_end <= x_start || y_end <= y_start {
        return;
    }
    let row_w = (x_end - x_start) as usize;

    const SS: usize = 4;
    let sub_w = 1.0 / SS as f32;
    let mut cov = vec![0.0f32; row_w];
    let mut xs: Vec<(f32, i32)> = Vec::with_capacity(16);

    for py in y_start..y_end {
        for c in cov.iter_mut() {
            *c = 0.0;
        }
        for s in 0..SS {
            let sy = py as f32 + (s as f32 + 0.5) * sub_w;
            xs.clear();
            for e in &edges {
                if sy >= e.y_lo && sy < e.y_hi {
                    xs.push((e.x_at_lo + (sy - e.y_lo) * e.dxdy, e.dir));
                }
            }
            if xs.len() < 2 {
                continue;
            }
            xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut wind = 0;
            for i in 0..xs.len() - 1 {
                wind += xs[i].1;
                if wind != 0 {
                    add_span(&mut cov, x_start, xs[i].0, xs[i + 1].0, sub_w);
                }
            }
        }
        for (i, c) in cov.iter().enumerate() {
            if *c > 0.0 {
                blend_pixel(buf, w, h, x_start + i as i32, py, color, *c, additive);
            }
        }
    }
}

/// Accumulate horizontal coverage for the span [xa, xb] into `cov` (indexed from
/// `x0`), weighting partially-covered end pixels by their covered fraction.
#[inline]
fn add_span(cov: &mut [f32], x0: i32, xa: f32, xb: f32, weight: f32) {
    if xb <= xa {
        return;
    }
    let n = cov.len() as f32;
    let xa = (xa - x0 as f32).max(0.0);
    let xb = (xb - x0 as f32).min(n);
    if xb <= xa {
        return;
    }
    let ia = xa.floor() as usize;
    let ib = (xb.ceil() as usize).min(cov.len());
    if ia >= cov.len() {
        return;
    }
    if ib - ia == 1 {
        cov[ia] += (xb - xa) * weight;
        return;
    }
    // first partial pixel
    cov[ia] += ((ia + 1) as f32 - xa) * weight;
    // full interior pixels
    for c in cov.iter_mut().take(ib.saturating_sub(1)).skip(ia + 1) {
        *c += weight;
    }
    // last partial pixel
    let last = ib - 1;
    if last > ia {
        cov[last] += (xb - last as f32) * weight;
    }
}

#[cfg(test)]
mod fill_tests {
    use super::*;
    #[test]
    fn solid_square_is_filled() {
        let (w, h) = (20usize, 20usize);
        let mut buf = vec![0u32; w * h];
        // CCW square 5..15
        let sq = vec![vec![
            [5.0, 5.0],
            [15.0, 5.0],
            [15.0, 15.0],
            [5.0, 15.0],
            [5.0, 5.0],
        ]];
        fill_contours_aa(&mut buf, w, h, 0xFFFFFF, false, &sq);
        let center = buf[10 * w + 10];
        eprintln!("center=0x{center:06X} corner=0x{:06X}", buf[0]);
        assert!(center != 0, "center should be filled");
    }
    #[test]
    fn ring_has_hole() {
        let (w, h) = (40usize, 40usize);
        let mut buf = vec![0u32; w * h];
        // outer CCW, inner CW (reversed) -> hole
        let outer = vec![
            [5.0, 5.0],
            [35.0, 5.0],
            [35.0, 35.0],
            [5.0, 35.0],
            [5.0, 5.0],
        ];
        let inner = vec![
            [15.0, 15.0],
            [15.0, 25.0],
            [25.0, 25.0],
            [25.0, 15.0],
            [15.0, 15.0],
        ];
        fill_contours_aa(&mut buf, w, h, 0xFFFFFF, false, &vec![outer, inner]);
        let body = buf[10 * w + 20]; // in the ring body (top stroke)
        let hole = buf[20 * w + 20]; // center hole
        eprintln!("body=0x{body:06X} hole=0x{hole:06X}");
        assert!(body != 0, "ring body should be filled");
        assert!(hole == 0, "hole should be empty");
    }
}
