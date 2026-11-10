// src/gfx/raster.rs â€” software 2-D pixel rasteriser
// All buffers are row-major Vec<u32> with pixels as 0x00RRGGBB.

/// Depth-test modes for the z-buffered fills (const generic, so each mode is
/// its own specialised loop): the normal `<` test; a depth-only prepass that
/// just records the nearest depth; and the shading pass after a prepass, which
/// shades only where its depth equals (`<=`) the recorded nearest one — each
/// visible pixel is shaded once instead of once per overlapping triangle.
pub const Z_LT: u8 = 0;
pub const Z_PREPASS: u8 = 1;
pub const Z_LE: u8 = 2;

#[inline(always)]
fn z_pass<const M: u8>(z: f32, zb: f32) -> bool {
    if M == Z_LE { z <= zb } else { z < zb }
}

/// Shared edge-function walker for the solid triangle fills (Pineda 1988).
///
/// Calls `f(idx, px, py, w0, w1, w2)` for every covered pixel centre, with the
/// barycentric weights of vertices 0/1/2. Per triangle it hoists what the old
/// per-fill loops recomputed per pixel — the edge sum `e0+e1+e2` is twice the
/// signed area, constant over the triangle, so one reciprocal replaces three
/// divides per pixel — and per row it solves each edge for the covered pixel
/// range, so the empty bounding-box corners of thin / diagonal triangles are
/// skipped instead of scanned. The exact per-pixel inside test is kept, so
/// coverage is the same as before.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn raster_tri<F: FnMut(usize, i32, i32, f32, f32, f32)>(
    width: usize,
    height: usize,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    mut f: F,
) {
    if width == 0 || height == 0 {
        return;
    }
    if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite() && x2.is_finite() && y2.is_finite()) {
        return;
    }
    let min_x = x0.min(x1).min(x2).max(0.0) as i32;
    let max_x = x0.max(x1).max(x2).min(width as f32 - 1.0) as i32;
    let min_y = y0.min(y1).min(y2).max(0.0) as i32;
    let max_y = y0.max(y1).max(y2).min(height as f32 - 1.0) as i32;
    if min_x > max_x || min_y > max_y {
        return;
    }
    // x-step deltas: adding 1 to px changes each edge by -(Δy)
    let de0 = -(y1 - y0);
    let de1 = -(y2 - y1);
    let de2 = -(y0 - y2);
    let fx0 = min_x as f32 + 0.5;
    let tot = (x1 - x0) * (y2 - y0) - (y1 - y0) * (x2 - x0);
    if tot.abs() <= 1e-6 {
        return; // zero-area sliver: covers no pixel centre
    }
    let inv = 1.0 / tot;
    let sgn = tot.signum();
    let dmax = max_x - min_x;
    for py in min_y..=max_y {
        let fy = py as f32 + 0.5;
        let mut e0 = (x1 - x0) * (fy - y0) - (y1 - y0) * (fx0 - x0);
        let mut e1 = (x2 - x1) * (fy - y1) - (y2 - y1) * (fx0 - x1);
        let mut e2 = (x0 - x2) * (fy - y2) - (y0 - y2) * (fx0 - x2);
        // analytic span: each edge (in the winding's sign) is linear in px, so
        // solve it >= 0 for the pixel offset and widen by one pixel each side
        let mut lo = 0i32;
        let mut hi = dmax;
        for (ev, d) in [(e0, de0), (e1, de1), (e2, de2)] {
            let a = ev * sgn;
            let d = d * sgn;
            if d.abs() < 1e-12 {
                if a < 0.0 {
                    lo = 1;
                    hi = 0;
                }
            } else {
                let t = -a / d;
                if d > 0.0 {
                    lo = lo.max((t.floor() as i32).saturating_sub(1));
                } else {
                    hi = hi.min((t.ceil() as i32).saturating_add(1));
                }
            }
        }
        if lo > hi {
            continue;
        }
        let lof = lo as f32;
        e0 += de0 * lof;
        e1 += de1 * lof;
        e2 += de2 * lof;
        let row = py as usize * width;
        let mut in_span = false;
        for px in (min_x + lo)..=(min_x + hi) {
            let inside = (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0) || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0);
            if inside {
                in_span = true;
                f(row + px as usize, px, py, e1 * inv, e2 * inv, e0 * inv);
            } else if in_span {
                break; // convex — span finished for this scanline
            }
            e0 += de0;
            e1 += de1;
            e2 += de2;
        }
    }
}

/// Fill a triangle with a flat colour (no depth test).
#[allow(clippy::too_many_arguments)]
pub fn fill_triangle(
    buf: &mut [u32],
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
    raster_tri(width, height, x0, y0, x1, y1, x2, y2, |idx, _, _, _, _, _| buf[idx] = color);
}

/// Posterise one interpolated Gouraud pixel (0..255 channels) into `bands`
/// discrete luminance levels, chroma preserved, softened by `softness`. Runs
/// once per pixel on the smoothly interpolated colour, so band edges follow
/// the shaded surface instead of being pinned to triangle/mesh vertices —
/// this is `ling_graphics::shading::posterize_soft` evaluated directly in
/// 0..255 space (no /255 · ×255 round trip per channel).
/// `bands < 2` skips the work entirely (smooth Gouraud, unchanged).
#[inline(always)]
fn posterize_span(r: f32, g: f32, b: f32, bands: u32, softness: f32) -> (f32, f32, f32) {
    if bands < 2 {
        return (r, g, b);
    }
    let nb = bands as f32;
    let lum = 0.299 * r + 0.587 * g + 0.114 * b;
    if lum < 255.0e-5 {
        return (r, g, b);
    }
    let band = (lum * (nb / 255.0)).floor().min(nb - 1.0);
    let k = ((band + 0.5) * (255.0 / nb) / lum).clamp(0.0, 4.0);
    let (hr, hg, hb) = ((r * k).min(255.0), (g * k).min(255.0), (b * k).min(255.0));
    let s = softness.clamp(0.0, 1.0);
    if s <= 0.0 {
        (hr, hg, hb)
    } else {
        (hr + (r - hr) * s, hg + (g - hg) * s, hb + (b - hb) * s)
    }
}

#[inline(always)]
fn pack_clamped(r: f32, g: f32, b: f32) -> u32 {
    ((r.clamp(0.0, 255.0) as u32) << 16) | ((g.clamp(0.0, 255.0) as u32) << 8) | (b.clamp(0.0, 255.0) as u32)
}

/// Gouraud-interpolated triangle with per-pixel cel-shade posterisation.
///
/// Vertex colours are linearly interpolated with barycentric weights; the
/// resulting per-pixel luminance is snapped to `bands` discrete toon bands
/// — giving a smooth gradient of bands across the face rather than a
/// per-vertex flat-shade seam.
///
/// `alpha` + `mode`: when `alpha < 1.0` or `mode != 0` the pixel is composited
/// over the existing buffer instead of directly overwritten.
/// `bands == 0` skips posterisation (full smooth shading).
#[allow(clippy::too_many_arguments)]
pub fn fill_triangle_gouraud(
    buf: &mut [u32],
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
    softness: f32,
    alpha: f32,
    mode: u8,
    tag_unlit: bool,
) {
    let (r0, g0, b0) = ((c0 >> 16 & 0xFF) as f32, (c0 >> 8 & 0xFF) as f32, (c0 & 0xFF) as f32);
    let (r1, g1, b1) = ((c1 >> 16 & 0xFF) as f32, (c1 >> 8 & 0xFF) as f32, (c1 & 0xFF) as f32);
    let (r2, g2, b2) = ((c2 >> 16 & 0xFF) as f32, (c2 >> 8 & 0xFF) as f32, (c2 & 0xFF) as f32);
    let blended = alpha < 0.999 || mode != 0;
    let tag = if tag_unlit { crate::gfx::UNLIT } else { 0 };
    raster_tri(width, height, x0, y0, x1, y1, x2, y2, |idx, px, py, w0, w1, w2| {
        let r = r0 * w0 + r1 * w1 + r2 * w2;
        let g = g0 * w0 + g1 * w1 + g2 * w2;
        let b = b0 * w0 + b1 * w1 + b2 * w2;
        let (r, g, b) = posterize_span(r, g, b, bands, softness);
        let packed = pack_clamped(r, g, b);
        if blended {
            composite_pixel(buf, width, height, px, py, packed, alpha, mode, false);
            buf[idx] |= tag;
        } else {
            buf[idx] = packed | tag;
        }
    });
}

/// Alpha-composite `color` over the pixel at (x,y) at coverage `cov` using the
/// modern compositing path: blend `mode` (0 normal Â· 1 add Â· 2 multiply Â·
/// 3 screen Â· 4 subtract Â· 5 overlay) optionally in linear light (`linear`).
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn composite_pixel(
    buf: &mut [u32],
    w: usize,
    h: usize,
    x: i32,
    y: i32,
    color: u32,
    cov: f32,
    mode: u8,
    linear: bool,
) {
    if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
        return;
    }
    let cov = cov.clamp(0.0, 1.0);
    if cov <= 0.0 {
        return;
    }
    let idx = y as usize * w + x as usize;
    buf[idx] = crate::gfx::color::composite(
        buf[idx],
        color,
        cov,
        crate::gfx::color::BlendMode::from_u8(mode),
        linear,
    );
}

/// Smooth gradient-interpolated triangle blended at a constant `alpha`. The
/// "gradient surface" fill: give each vertex a colour and the interior blends
/// smoothly â€” point the bright vertex toward a light to fake directional
/// lighting without a shader. When `oklab` the interior is interpolated
/// perceptually through OkLab (no muddy mid-tones); otherwise in sRGB. `mode`
/// + `linear` select the compositing operator (see `composite_pixel`).
#[allow(clippy::too_many_arguments)]
pub fn fill_triangle_grad(
    buf: &mut [u32],
    width: usize,
    height: usize,
    alpha: f32,
    mode: u8,
    linear: bool,
    oklab: bool,
    x0: f32,
    y0: f32,
    c0: u32,
    x1: f32,
    y1: f32,
    c1: u32,
    x2: f32,
    y2: f32,
    c2: u32,
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
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }

    let min_x = x0.min(x1).min(x2).max(0.0) as i32;
    let max_x = x0.max(x1).max(x2).min(width as f32 - 1.0) as i32;
    let min_y = y0.min(y1).min(y2).max(0.0) as i32;
    let max_y = y0.max(y1).max(y2).min(height as f32 - 1.0) as i32;
    if min_x > max_x || min_y > max_y {
        return;
    }

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

    for py in min_y..=max_y {
        let fy = py as f32 + 0.5;
        let fx0 = min_x as f32 + 0.5;
        let mut e0 = (x1 - x0) * (fy - y0) - (y1 - y0) * (fx0 - x0);
        let mut e1 = (x2 - x1) * (fy - y1) - (y2 - y1) * (fx0 - x1);
        let mut e2 = (x0 - x2) * (fy - y2) - (y0 - y2) * (fx0 - x2);
        let mut in_span = false;
        for px in min_x..=max_x {
            let inside =
                (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0) || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0);
            if inside {
                let tot = e0 + e1 + e2;
                if tot.abs() > 1e-6 {
                    let w2 = e0 / tot;
                    let w0 = e1 / tot;
                    let w1 = e2 / tot;
                    let col = if oklab {
                        crate::gfx::color::bary_oklab(c0, c1, c2, w0, w1, w2)
                    } else {
                        let r = r0 * w0 + r1 * w1 + r2 * w2;
                        let g = g0 * w0 + g1 * w1 + g2 * w2;
                        let b = b0 * w0 + b1 * w1 + b2 * w2;
                        ((r.clamp(0.0, 255.0) as u32) << 16)
                            | ((g.clamp(0.0, 255.0) as u32) << 8)
                            | (b.clamp(0.0, 255.0) as u32)
                    };
                    composite_pixel(buf, width, height, px, py, col, alpha, mode, linear);
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

/// Depth-tested flat triangle (true z-buffer). Interpolates camera-space z
/// barycentrically; a pixel is written only if it is nearer than `zbuf`.
#[allow(clippy::too_many_arguments)]
pub fn fill_triangle_z(
    buf: &mut [u32],
    zbuf: &mut [f32],
    width: usize,
    height: usize,
    color: u32,
    x0: f32,
    y0: f32,
    z0: f32,
    x1: f32,
    y1: f32,
    z1: f32,
    x2: f32,
    y2: f32,
    z2: f32,
) {
    fill_triangle_z_mode::<Z_LT>(buf, zbuf, width, height, color, x0, y0, z0, x1, y1, z1, x2, y2, z2)
}

/// [`fill_triangle_z`] in depth mode `M` ([`Z_LT`] / [`Z_PREPASS`] / [`Z_LE`]).
/// The prepass writes depth only (`buf` untouched).
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn fill_triangle_z_mode<const M: u8>(
    buf: &mut [u32],
    zbuf: &mut [f32],
    width: usize,
    height: usize,
    color: u32,
    x0: f32,
    y0: f32,
    z0: f32,
    x1: f32,
    y1: f32,
    z1: f32,
    x2: f32,
    y2: f32,
    z2: f32,
) {
    let pp = persp_iw(z0, z1, z2);
    raster_tri(width, height, x0, y0, x1, y1, x2, y2, |idx, _, _, w0, w1, w2| {
        let z = persp_z(pp, w0, w1, w2, z0, z1, z2);
        if z_pass::<M>(z, zbuf[idx]) {
            zbuf[idx] = z;
            if M != Z_PREPASS {
                buf[idx] = color;
            }
        }
    });
}

/// Solid triangle composited with blend `mode` + `alpha` (no depth test).
/// Used for translucent 3-D FX (slashes, ring trails, orbs) in the painter's
/// path so they glow/fade instead of painting opaque black.
#[allow(clippy::too_many_arguments)]
pub fn fill_triangle_blend(
    buf: &mut [u32],
    width: usize,
    height: usize,
    color: u32,
    mode: u8,
    alpha: f32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
) {
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }
    let fast_add = mode == 1; // integer saturating-add fast path
    let src = if fast_add && alpha < 0.999 { scale_rgb(color, alpha) } else { color };
    raster_tri(width, height, x0, y0, x1, y1, x2, y2, |idx, px, py, _, _, _| {
        if fast_add {
            buf[idx] = add_sat(buf[idx], src);
        } else {
            composite_pixel(buf, width, height, px, py, color, alpha, mode, false);
        }
    });
}

/// Depth-tested translucent triangle: composites with blend `mode` + `alpha`
/// where it passes the z-buffer, but does NOT write depth (so the opaque scene
/// still occludes it and other translucent layers behind it remain visible).
#[allow(clippy::too_many_arguments)]
pub fn fill_triangle_z_blend(
    buf: &mut [u32],
    zbuf: &[f32],
    width: usize,
    height: usize,
    color: u32,
    mode: u8,
    alpha: f32,
    x0: f32,
    y0: f32,
    z0: f32,
    x1: f32,
    y1: f32,
    z1: f32,
    x2: f32,
    y2: f32,
    z2: f32,
) {
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }
    let fast_add = mode == 1; // integer saturating-add fast path
    let src = if fast_add && alpha < 0.999 { scale_rgb(color, alpha) } else { color };
    let pp = persp_iw(z0, z1, z2);
    raster_tri(width, height, x0, y0, x1, y1, x2, y2, |idx, px, py, w0, w1, w2| {
        let z = persp_z(pp, w0, w1, w2, z0, z1, z2);
        if z < zbuf[idx] {
            if fast_add {
                buf[idx] = add_sat(buf[idx], src);
            } else {
                composite_pixel(buf, width, height, px, py, color, alpha, mode, false);
            }
        }
    });
}

/// Depth-tested Gouraud + cel-shade triangle (z-buffer path).
/// Same cel-shade logic as `fill_triangle_gouraud` with `alpha`/`mode` support.
#[allow(clippy::too_many_arguments)]
pub fn fill_triangle_gouraud_z(
    buf: &mut [u32],
    zbuf: &mut [f32],
    width: usize,
    height: usize,
    x0: f32,
    y0: f32,
    z0: f32,
    c0: u32,
    x1: f32,
    y1: f32,
    z1: f32,
    c1: u32,
    x2: f32,
    y2: f32,
    z2: f32,
    c2: u32,
    bands: u32,
    softness: f32,
    alpha: f32,
    mode: u8,
    tag_unlit: bool,
) {
    fill_triangle_gouraud_z_mode::<Z_LT>(buf, zbuf, width, height, x0, y0, z0, c0, x1, y1, z1, c1, x2, y2, z2, c2, bands, softness, alpha, mode, tag_unlit)
}

/// [`fill_triangle_gouraud_z`] in depth mode `M` (see [`Z_LE`]).
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn fill_triangle_gouraud_z_mode<const M: u8>(
    buf: &mut [u32],
    zbuf: &mut [f32],
    width: usize,
    height: usize,
    x0: f32,
    y0: f32,
    z0: f32,
    c0: u32,
    x1: f32,
    y1: f32,
    z1: f32,
    c1: u32,
    x2: f32,
    y2: f32,
    z2: f32,
    c2: u32,
    bands: u32,
    softness: f32,
    alpha: f32,
    mode: u8,
    tag_unlit: bool,
) {
    let (r0, g0, b0) = ((c0 >> 16 & 0xFF) as f32, (c0 >> 8 & 0xFF) as f32, (c0 & 0xFF) as f32);
    let (r1, g1, b1) = ((c1 >> 16 & 0xFF) as f32, (c1 >> 8 & 0xFF) as f32, (c1 & 0xFF) as f32);
    let (r2, g2, b2) = ((c2 >> 16 & 0xFF) as f32, (c2 >> 8 & 0xFF) as f32, (c2 & 0xFF) as f32);
    let blended = alpha < 0.999 || mode != 0;
    let tag = if tag_unlit { crate::gfx::UNLIT } else { 0 };
    let pp = persp_iw(z0, z1, z2);
    raster_tri(width, height, x0, y0, x1, y1, x2, y2, |idx, px, py, w0, w1, w2| {
        let z = persp_z(pp, w0, w1, w2, z0, z1, z2);
        if z_pass::<M>(z, zbuf[idx]) {
            let r = r0 * w0 + r1 * w1 + r2 * w2;
            let g = g0 * w0 + g1 * w1 + g2 * w2;
            let b = b0 * w0 + b1 * w1 + b2 * w2;
            let (r, g, b) = posterize_span(r, g, b, bands, softness);
            let packed = pack_clamped(r, g, b);
            if blended {
                // Translucent: test depth but don't write it so layers stack correctly.
                composite_pixel(buf, width, height, px, py, packed, alpha, mode, false);
                buf[idx] |= tag;
            } else {
                zbuf[idx] = z;
                buf[idx] = packed | tag;
            }
        }
    });
}

/// Axis-aligned rectangle filled with a gradient from `c0` to `c1`.
/// `dir`: 0 = horizontal (leftâ†’right), anything else = vertical (topâ†’bottom).
/// `oklab` interpolates the gradient perceptually; `mode`+`linear` select the
/// compositing operator. A quick way to fake a lit wall/floor or a sky band.
#[allow(clippy::too_many_arguments)]
pub fn fill_rect_grad(
    buf: &mut [u32],
    width: usize,
    height: usize,
    alpha: f32,
    mode: u8,
    linear: bool,
    oklab: bool,
    x: f32,
    y: f32,
    rw: f32,
    rh: f32,
    c0: u32,
    c1: u32,
    dir: u8,
) {
    if width == 0 || height == 0 || rw <= 0.0 || rh <= 0.0 {
        return;
    }
    if !x.is_finite() || !y.is_finite() || !rw.is_finite() || !rh.is_finite() {
        return;
    }
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }

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

    let x0 = x.floor().max(0.0) as i32;
    let y0 = y.floor().max(0.0) as i32;
    let x1 = (x + rw).ceil().min(width as f32) as i32;
    let y1 = (y + rh).ceil().min(height as f32) as i32;
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    // ── fast paths: opaque + normal blend writes whole rows at once ──
    // (UI chrome tends to be large solid / gradient rects; the per-pixel
    // composite_pixel path below costs ~20ns/px, these ~0.3ns/px.)
    if alpha >= 1.0 && mode == 0 {
        let (xs, xe) = (x0 as usize, x1 as usize);
        if c0 == c1 {
            // solid rect → memset each row
            for py in y0 as usize..y1 as usize {
                buf[py * width + xs..py * width + xe].fill(c0);
            }
            return;
        }
        if dir != 0 {
            // vertical gradient → one colour per row
            for py in y0..y1 {
                let t = ((py as f32 + 0.5 - y) / rh).clamp(0.0, 1.0);
                let col = if oklab {
                    crate::gfx::color::mix_oklab(c0, c1, t)
                } else {
                    let r = r0 + (r1 - r0) * t;
                    let g = g0 + (g1 - g0) * t;
                    let b = b0 + (b1 - b0) * t;
                    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
                };
                let py = py as usize;
                buf[py * width + xs..py * width + xe].fill(col);
            }
        } else {
            // horizontal gradient → compute one row, copy it to the rest
            let mut row: Vec<u32> = Vec::with_capacity(xe - xs);
            for px in x0..x1 {
                let t = ((px as f32 + 0.5 - x) / rw).clamp(0.0, 1.0);
                row.push(if oklab {
                    crate::gfx::color::mix_oklab(c0, c1, t)
                } else {
                    let r = r0 + (r1 - r0) * t;
                    let g = g0 + (g1 - g0) * t;
                    let b = b0 + (b1 - b0) * t;
                    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
                });
            }
            for py in y0 as usize..y1 as usize {
                buf[py * width + xs..py * width + xe].copy_from_slice(&row);
            }
        }
        return;
    }

    for py in y0..y1 {
        let tv = ((py as f32 + 0.5 - y) / rh).clamp(0.0, 1.0);
        for px in x0..x1 {
            let t = if dir == 0 {
                ((px as f32 + 0.5 - x) / rw).clamp(0.0, 1.0)
            } else {
                tv
            };
            let col = if oklab {
                crate::gfx::color::mix_oklab(c0, c1, t)
            } else {
                let r = r0 + (r1 - r0) * t;
                let g = g0 + (g1 - g0) * t;
                let b = b0 + (b1 - b0) * t;
                ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
            };
            composite_pixel(buf, width, height, px, py, col, alpha, mode, linear);
        }
    }
}

/// Soft-edged filled ellipse â€” the blob/contact shadow primitive. Pixels within
/// the `(rx, ry)` radii are composited in `color` at `alpha`; from `(1 - soft)`
/// of the radius out to the rim the coverage falls off linearly for a feather.
/// `soft` âˆˆ [0,1] (0 = hard edge, 1 = fully feathered). `mode`+`linear` select
/// the compositing operator (normal/linear give a natural darkening shadow).
#[allow(clippy::too_many_arguments)]
pub fn fill_disc_soft(
    buf: &mut [u32],
    width: usize,
    height: usize,
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    color: u32,
    alpha: f32,
    soft: f32,
    mode: u8,
    linear: bool,
) {
    if width == 0 || height == 0 {
        return;
    }
    if !cx.is_finite() || !cy.is_finite() || !rx.is_finite() || !ry.is_finite() {
        return;
    }
    let rx = rx.max(0.5);
    let ry = ry.max(0.5);
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }
    let soft = soft.clamp(0.0, 1.0);
    let inner = 1.0 - soft; // normalised distance where the falloff begins

    let x0 = (cx - rx).floor().max(0.0) as i32;
    let x1 = (cx + rx).ceil().min(width as f32 - 1.0) as i32;
    let y0 = (cy - ry).floor().max(0.0) as i32;
    let y1 = (cy + ry).ceil().min(height as f32 - 1.0) as i32;

    for py in y0..=y1 {
        let dy = (py as f32 + 0.5 - cy) / ry;
        for px in x0..=x1 {
            let dx = (px as f32 + 0.5 - cx) / rx;
            let d = (dx * dx + dy * dy).sqrt();
            if d >= 1.0 {
                continue;
            }
            let cov = if d <= inner || soft <= 1e-6 {
                1.0
            } else {
                (1.0 - d) / soft
            };
            composite_pixel(
                buf,
                width,
                height,
                px,
                py,
                color,
                alpha * cov.clamp(0.0, 1.0),
                mode,
                linear,
            );
        }
    }
}

/// Walk the pixels of the aliased line (x0,y0)→(x1,y1), calling `plot(idx)`
/// for each one inside the buffer.
///
/// Clip-invariant: the pixel at each step along the major axis is a pure
/// function of the *unclipped* line (integer endpoints by `floor`, which —
/// unlike truncation — commutes with the whole-pixel y shift of a raster
/// strip; minor coordinate rounded from the exact slope). So a line drawn into
/// several strips, or clipped by the screen edge, lights exactly the pixels the
/// whole line would — no one-pixel kinks at strip seams, as restarting
/// Bresenham from clipped endpoints gave. Only the visible run is walked.
#[inline(always)]
fn walk_line<F: FnMut(usize)>(width: usize, height: usize, x0: f32, y0: f32, x1: f32, y1: f32, mut plot: F) {
    let (ax, ay, bx, by) = (x0.floor() as i64, y0.floor() as i64, x1.floor() as i64, y1.floor() as i64);
    let steep = (by - ay).abs() > (bx - ax).abs();
    let (ma0, mi0, ma1, mi1, mlen, nlen) = if steep { (ay, ax, by, bx, height, width) } else { (ax, ay, bx, by, width, height) };
    let (ma0, mi0, ma1, mi1) = if ma0 > ma1 { (ma1, mi1, ma0, mi0) } else { (ma0, mi0, ma1, mi1) };
    let span = ma1 - ma0;
    let slope = if span == 0 { 0.0 } else { (mi1 - mi0) as f64 / span as f64 };
    let mut lo = ma0.max(0);
    let mut hi = ma1.min(mlen as i64 - 1);
    if slope != 0.0 {
        let ta = ma0 as f64 + (-1.0 - mi0 as f64) / slope;
        let tb = ma0 as f64 + (nlen as f64 - mi0 as f64) / slope;
        lo = lo.max(ta.min(tb).floor() as i64 - 1);
        hi = hi.min(ta.max(tb).ceil() as i64 + 1);
    } else if mi0 < 0 || mi0 >= nlen as i64 {
        return;
    }
    let mut t = lo;
    while t <= hi {
        let m = mi0 + ((t - ma0) as f64 * slope).round() as i64;
        let (x, y) = if steep { (m, t) } else { (t, m) };
        if x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < height {
            plot(y as usize * width + x as usize);
        }
        t += 1;
    }
}

/// Cohen-Sutherland clip then Bresenham integer line drawing.
/// Lines with one endpoint way off-screen (from behind-camera perspective blowup)
/// are clipped to the viewport before rasterisation so Bresenham never iterates
/// millions of steps.
#[allow(clippy::too_many_arguments)]
pub fn draw_line(
    buf: &mut [u32],
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
    let color = color | crate::gfx::UNLIT;
    walk_line(width, height, x0, y0, x1, y1, |idx| buf[idx] = color);
}

/// Fast saturating per-channel additive of `src` over `dst` (sRGB bytes).
#[inline]
pub fn add_sat(dst: u32, src: u32) -> u32 {
    let r = (((dst >> 16) & 0xFF) + ((src >> 16) & 0xFF)).min(255);
    let g = (((dst >> 8) & 0xFF) + ((src >> 8) & 0xFF)).min(255);
    let b = ((dst & 0xFF) + (src & 0xFF)).min(255);
    (r << 16) | (g << 8) | b
}

/// Pre-scale an sRGB colour by `a` (for the additive fast path).
#[inline]
fn scale_rgb(color: u32, a: f32) -> u32 {
    let r = (((color >> 16) & 0xFF) as f32 * a) as u32;
    let g = (((color >> 8) & 0xFF) as f32 * a) as u32;
    let b = ((color & 0xFF) as f32 * a) as u32;
    (r << 16) | (g << 8) | b
}

/// Blended Bresenham line (no AA â€” fast, matches the old `draw_line` speed).
/// Additive (mode 1) uses an integer saturating add; other modes/alpha fall
/// back to `composite_pixel`. Used for translucent 3-D FX lines (ring trails).
#[allow(clippy::too_many_arguments)]
pub fn draw_line_blend(
    buf: &mut [u32],
    width: usize,
    height: usize,
    color: u32,
    mode: u8,
    alpha: f32,
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
    let fast_add = mode == 1;
    let src = if fast_add && alpha < 0.999 { scale_rgb(color, alpha.clamp(0.0, 1.0)) } else { color };
    walk_line(width, height, x0, y0, x1, y1, |idx| {
        if fast_add {
            buf[idx] = add_sat(buf[idx], src) | crate::gfx::UNLIT;
        } else {
            let (x, y) = ((idx % width) as i32, (idx / width) as i32);
            composite_pixel(buf, width, height, x, y, color, alpha, mode, false);
            buf[idx] |= crate::gfx::UNLIT;
        }
    });
}


// â”€â”€ Anti-aliased primitives (used by the vector-font renderer) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Alpha-blend `color` over the pixel at (x,y) with `cov` âˆˆ [0,1].
/// `additive` matches `set_blend(1)`: source is added instead of mixed.
#[inline]
#[allow(clippy::too_many_arguments)]
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
    if !(cov > 0.0) {
        return; // also rejects NaN
    }
    // 8-bit fixed-point coverage; red+blue blend together in one u32 (lanes at
    // bits 0 and 16), green in another — two multiplies instead of six float
    // lerps, two clamps and three float→int conversions per pixel.
    let a = if cov >= 1.0 { 256 } else { (cov * 256.0) as u32 };
    if a == 0 {
        return;
    }
    let idx = y as usize * w + x as usize;
    let dst = buf[idx];
    let out = if additive {
        let src = ((((color & 0x00FF_00FF) * a) >> 8) & 0x00FF_00FF) | ((((color & 0x0000_FF00) * a) >> 8) & 0x0000_FF00);
        add_sat(dst & crate::gfx::RGB_MASK, src)
    } else {
        let ia = 256 - a;
        let rb = (((color & 0x00FF_00FF) * a + (dst & 0x00FF_00FF) * ia) >> 8) & 0x00FF_00FF;
        let g = (((color & 0x0000_FF00) * a + (dst & 0x0000_FF00) * ia) >> 8) & 0x0000_FF00;
        rb | g
    };
    buf[idx] = out | crate::gfx::UNLIT;
}

/// Xiaolin-Wu anti-aliased line. `additive` selects the blend mode.
///
/// Only the part of the line over the buffer is walked: the run along the
/// major axis is cut to the columns (rows, for steep lines) inside the buffer
/// and to where the minor coordinate is within it — so a line projected far
/// off-screen, or one crossing a thin raster strip, costs only its visible
/// pixels. The walk is cut, not the endpoints, so the visible pixels are the
/// same as drawing the whole line (no fake end caps at the cut).
#[allow(clippy::too_many_arguments)]
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
    // extent of the major / minor axis in buffer pixels
    let (major, minor) = if steep { (h as i32, w as f32) } else { (w as i32, h as f32) };

    let plot = |buf: &mut [u32], a: i32, b: i32, c: f32| {
        if steep {
            blend_pixel(buf, w, h, b, a, color, c, additive);
        } else {
            blend_pixel(buf, w, h, a, b, color, c, additive);
        }
    };

    // endpoints
    let inter0 = y0 + grad * (x0.round() - x0) + grad;
    let xend0 = x0.round();
    let xend1 = x1.round();

    plot(buf, xend0 as i32, y0.floor() as i32, 1.0);
    plot(buf, xend1 as i32, y1.floor() as i32, 1.0);

    let xpxl1 = xend0 as i32;
    let xpxl2 = xend1 as i32;
    let first = xpxl1.saturating_add(1);
    let mut lo = first.max(-1);
    let mut hi = xpxl2.saturating_sub(1).min(major);
    // where the minor coordinate inter(x) = inter0 + grad·(x − first) is on the buffer
    if grad.abs() > 1e-9 {
        let xa = first as f32 + (-2.0 - inter0) / grad;
        let xb = first as f32 + (minor + 1.0 - inter0) / grad;
        lo = lo.max((xa.min(xb).floor() as i32).saturating_sub(1));
        hi = hi.min((xa.max(xb).ceil() as i32).saturating_add(1));
    } else if inter0 < -2.0 || inter0 > minor + 1.0 {
        return;
    }
    let base = first as f32;
    let mut x = lo;
    while x <= hi {
        let iy = inter0 + grad * (x as f32 - base);
        let fy = iy.floor();
        let frac = iy - fy;
        plot(buf, x, fy as i32, 1.0 - frac);
        plot(buf, x, fy as i32 + 1, frac);
        x += 1;
    }
}

/// Draw a circular arc (or full circle) as a connected polyline from `a0` to
/// `a1` radians. `aa` selects Xiaolin-Wu anti-aliasing; otherwise crisp
/// (aliased) Bresenham. `additive` selects additive blending (glow); otherwise
/// the stroke is fully opaque. `segments` is the number of line segments.
#[allow(clippy::too_many_arguments)]
pub fn draw_arc(
    buf: &mut [u32],
    w: usize,
    h: usize,
    color: u32,
    aa: bool,
    additive: bool,
    cx: f32,
    cy: f32,
    r: f32,
    a0: f32,
    a1: f32,
    segments: u32,
) {
    if w == 0 || h == 0 || r <= 0.0 {
        return;
    }
    if !cx.is_finite() || !cy.is_finite() || !r.is_finite() || !a0.is_finite() || !a1.is_finite() {
        return;
    }
    let segs = segments.max(1);
    let step = (a1 - a0) / segs as f32;
    let mut px = cx + r * a0.cos();
    let mut py = cy + r * a0.sin();
    let mut i = 1u32;
    while i <= segs {
        let a = a0 + step * i as f32;
        let nx = cx + r * a.cos();
        let ny = cy + r * a.sin();
        if aa {
            draw_line_aa(buf, w, h, color, additive, px, py, nx, ny);
        } else if additive {
            draw_line_blend(buf, w, h, color, 1, 1.0, px, py, nx, ny);
        } else {
            draw_line(buf, w, h, color, px, py, nx, ny);
        }
        px = nx;
        py = ny;
        i += 1;
    }
}

/// Hard-edged scanline fill of closed `polylines` (screen space, y-down) using
/// the non-zero winding rule — same geometry as `fill_contours_aa` but a
/// single center-point sample per pixel row/column instead of 4× supersampled
/// coverage, so edges snap to whole pixels with no partial-alpha blending.
pub fn fill_contours(
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
    struct Edge {
        y_lo: f32,
        y_hi: f32,
        x_at_lo: f32,
        dxdy: f32,
        dir: i32,
    }
    let mut edges: Vec<Edge> = Vec::new();
    let (mut ymin, mut ymax) = (f32::MAX, f32::MIN);
    for pl in polylines {
        for seg in pl.windows(2) {
            let (a, b) = (seg[0], seg[1]);
            if !a[0].is_finite() || !a[1].is_finite() || !b[0].is_finite() || !b[1].is_finite() {
                continue;
            }
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
    let mut xs: Vec<(f32, i32)> = Vec::with_capacity(16);
    for py in y_start..y_end {
        let sy = py as f32 + 0.5;
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
                let xa = xs[i].0.round().max(0.0) as i32;
                let xb = (xs[i + 1].0.round() as i32).min(w as i32);
                for px in xa..xb {
                    blend_pixel(buf, w, h, px, py, color, 1.0, additive);
                }
            }
        }
    }
}

/// Coverage-based scanline fill of closed `polylines` (screen space, y-down)
/// using the non-zero winding rule, with 4Ã— vertical supersampling and analytic
/// horizontal span coverage â€” smooth filled glyph interiors without aliasing.
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

    // Active edge list: edges sorted by top y join when the scan reaches them
    // and leave past their bottom, so each sub-scanline looks at the handful of
    // edges crossing it instead of every edge of the path.
    edges.sort_by(|a, b| a.y_lo.partial_cmp(&b.y_lo).unwrap_or(std::cmp::Ordering::Equal));
    let mut next = 0usize;
    let mut active: Vec<usize> = Vec::with_capacity(16);
    for py in y_start..y_end {
        for c in cov.iter_mut() {
            *c = 0.0;
        }
        for s in 0..SS {
            let sy = py as f32 + (s as f32 + 0.5) * sub_w;
            while next < edges.len() && edges[next].y_lo <= sy {
                active.push(next);
                next += 1;
            }
            active.retain(|&i| sy < edges[i].y_hi);
            xs.clear();
            for &i in &active {
                let e = &edges[i];
                if sy >= e.y_lo {
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

/// Depth-of-field post pass (uses the z-buffer). Blurs the framebuffer by up to
/// `rad` px wherever the camera-space depth departs from `focus` by more than
/// `range` â€” sharp at the focal plane, progressively soft toward near/far.
/// Background pixels (no geometry, depth = +âˆž) are treated as fully far â†’ max
/// blur. Implementation: one separable box blur (sliding window, O(1)/px per
/// pass) at the max radius, then a per-pixel mix by circle-of-confusion â€” cheap
/// enough for a full-frame pass and the canonical z-buffer showcase effect.
#[allow(clippy::too_many_arguments)]
pub fn depth_of_field(
    buf: &mut [u32],
    zbuf: &[f32],
    width: usize,
    height: usize,
    focus: f32,
    range: f32,
    rad: usize,
    oil: f32,
) {
    let n = width * height;
    if width < 4 || height < 4 || rad == 0 || zbuf.len() != n || buf.len() != n {
        return;
    }
    let range = range.max(1e-3);
    let oil = oil.clamp(0.0, 1.0);

    // The blurred image is built at HALF resolution (the blur is low-frequency,
    // so this is visually identical and ~4× cheaper), then sampled bilinearly
    // during the full-res composite. R, G, B live interleaved in one plane taken
    // from the per-thread scratch pool, so a frame allocates nothing (this used
    // to be six fresh 0.5–2 MB planes per frame).
    let hw = width.div_ceil(2);
    let hh = height.div_ceil(2);
    let hn = hw * hh;
    let r_h = rad.div_ceil(2).max(1) as i32; // blur radius in half-res pixels
    let inv_win = 1.0 / (2 * r_h + 1) as f32;
    let hwi = hw as i32;
    let row3 = 3 * hw;

    let mut s = crate::gfx::scratch::take_f32(3 * hn);
    let mut tmp = crate::gfx::scratch::take_f32(3 * hn);

    // Downsample (2×2 average).
    let down_row = |hy: usize, row: &mut [f32]| {
        let y0 = (hy * 2).min(height - 1);
        let y1 = (hy * 2 + 1).min(height - 1);
        for hx in 0..hw {
            let x0 = (hx * 2).min(width - 1);
            let x1 = (hx * 2 + 1).min(width - 1);
            let (mut r, mut g, mut b) = (0f32, 0f32, 0f32);
            for &i in &[y0 * width + x0, y0 * width + x1, y1 * width + x0, y1 * width + x1] {
                let p = buf[i];
                r += ((p >> 16) & 0xFF) as f32;
                g += ((p >> 8) & 0xFF) as f32;
                b += (p & 0xFF) as f32;
            }
            row[3 * hx] = r * 0.25;
            row[3 * hx + 1] = g * 0.25;
            row[3 * hx + 2] = b * 0.25;
        }
    };
    // Separable box blur. H: sliding window per row; V: sum of the 2r+1 rows
    // above/below, read row-major.
    let hpass = |src: &[f32], y: usize, out: &mut [f32]| {
        let row = &src[y * row3..(y + 1) * row3];
        for c in 0..3 {
            let mut acc = 0.0f32;
            let mut k = -r_h;
            while k <= r_h {
                acc += row[3 * k.clamp(0, hwi - 1) as usize + c];
                k += 1;
            }
            for x in 0..hw {
                out[3 * x + c] = acc * inv_win;
                let lo = (x as i32 - r_h).clamp(0, hwi - 1) as usize;
                let hi2 = (x as i32 + r_h + 1).clamp(0, hwi - 1) as usize;
                acc += row[3 * hi2 + c] - row[3 * lo + c];
            }
        }
    };
    let vpass = |src: &[f32], y: usize, out: &mut [f32]| {
        out.fill(0.0);
        let mut k = -r_h;
        while k <= r_h {
            let yy = (y as i32 + k).clamp(0, hh as i32 - 1) as usize;
            for (o, v) in out.iter_mut().zip(&src[yy * row3..(yy + 1) * row3]) {
                *o += *v;
            }
            k += 1;
        }
        for o in out.iter_mut() {
            *o *= inv_win;
        }
    };

    // Composite: lerp sharp→blurred by circle-of-confusion, preserving any
    // tag bits (UNLIT ink survives the blur untagged-quantisation). With
    // `oil` > 0 the blurred zone gets an oil-slick treatment: the red/blue
    // blurred planes are sampled with opposite horizontal offsets (iridescent
    // chroma fringe) and the hue is swirled slightly — water / heat haze /
    // far-field shimmer.
    // per-column (left index, right index, weight) of the half-res plane
    let cols: Vec<(u32, u32, f32)> = (0..width)
        .map(|x| {
            let gx = (x as f32 * 0.5).min(hw as f32 - 1.0).max(0.0);
            let x0 = gx as usize;
            ((3 * x0) as u32, (3 * (x0 + 1).min(hw - 1)) as u32, gx - x0 as f32)
        })
        .collect();
    let comp_row = |s: &[f32], y: usize, row: &mut [u32], rowv: &mut Vec<f32>| {
        {
            let gy = (y as f32 * 0.5).min(hh as f32 - 1.0).max(0.0);
            let y0 = gy as usize;
            let y1 = (y0 + 1).min(hh - 1);
            let ty = gy - y0 as f32;
            rowv.clear();
            rowv.extend(s[y0 * row3..(y0 + 1) * row3].iter().zip(&s[y1 * row3..(y1 + 1) * row3]).map(|(a, b)| a * (1.0 - ty) + b * ty));
        }
        // Bilinear sample of channel `c` of the half-res plane at full-res coords.
        let samp = |c: usize, fx: f32, fy: f32| -> f32 {
            let gx = (fx * 0.5).min(hw as f32 - 1.0).max(0.0);
            let gy = (fy * 0.5).min(hh as f32 - 1.0).max(0.0);
            let x0 = gx as usize;
            let y0 = gy as usize;
            let x1 = (x0 + 1).min(hw - 1);
            let y1 = (y0 + 1).min(hh - 1);
            let tx = gx - x0 as f32;
            let ty = gy - y0 as f32;
            let a = s[y0 * row3 + 3 * x0 + c] * (1.0 - tx) + s[y0 * row3 + 3 * x1 + c] * tx;
            let b = s[y1 * row3 + 3 * x0 + c] * (1.0 - tx) + s[y1 * row3 + 3 * x1 + c] * tx;
            a * (1.0 - ty) + b * ty
        };
        let fy = y as f32;
        for (x, px) in row.iter_mut().enumerate() {
            let i = y * width + x;
            let z = zbuf[i];
            let coc = if z.is_finite() { ((z - focus).abs() / range).clamp(0.0, 1.0) } else { 1.0 };
            if coc <= 0.003 {
                continue; // in focus — untouched
            }
            let sh = 1.0 - coc;
            let p = *px;
            let tag = p & !crate::gfx::RGB_MASK;
            let pr = ((p >> 16) & 0xFF) as f32;
            let pg = ((p >> 8) & 0xFF) as f32;
            let pb = (p & 0xFF) as f32;
            let fx = x as f32;
            let (mut r, mut g, mut b);
            if oil > 0.0 {
                let off = coc * oil * 3.0;
                r = pr * sh + samp(0, fx - off, fy) * coc;
                g = pg * sh + samp(1, fx, fy) * coc;
                b = pb * sh + samp(2, fx + off, fy) * coc;
                // gentle hue swirl toward an iridescent film
                let k = oil * coc * 0.22;
                let (r0, g0, b0) = (r, g, b);
                r = r0 * (1.0 - k) + b0 * k;
                g = g0 * (1.0 - k) + r0 * k;
                b = b0 * (1.0 - k) + g0 * k;
            } else {
                // separable bilinear: the row was lerped vertically once above
                let (i0, i1, tx) = cols[x];
                let (i0, i1) = (i0 as usize, i1 as usize);
                r = pr * sh + (rowv[i0] * (1.0 - tx) + rowv[i1] * tx) * coc;
                g = pg * sh + (rowv[i0 + 1] * (1.0 - tx) + rowv[i1 + 1] * tx) * coc;
                b = pb * sh + (rowv[i0 + 2] * (1.0 - tx) + rowv[i1 + 2] * tx) * coc;
            }
            let r = r.round().clamp(0.0, 255.0) as u32;
            let g = g.round().clamp(0.0, 255.0) as u32;
            let b = b.round().clamp(0.0, 255.0) as u32;
            *px = (r << 16) | (g << 8) | b | tag;
        }
    };

    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        s.par_chunks_mut(row3).enumerate().for_each(|(y, r)| down_row(y, r));
        tmp.par_chunks_mut(row3).enumerate().for_each(|(y, r)| hpass(&s, y, r));
        s.par_chunks_mut(row3).enumerate().for_each(|(y, r)| vpass(&tmp, y, r));
        let sr = &s;
        buf[..n]
            .par_chunks_mut(width)
            .enumerate()
            .for_each_init(|| Vec::with_capacity(row3), |rowv, (y, row)| comp_row(sr, y, row, rowv));
    }
    #[cfg(target_arch = "wasm32")]
    {
        s.chunks_mut(row3).enumerate().for_each(|(y, r)| down_row(y, r));
        tmp.chunks_mut(row3).enumerate().for_each(|(y, r)| hpass(&s, y, r));
        s.chunks_mut(row3).enumerate().for_each(|(y, r)| vpass(&tmp, y, r));
        let mut rowv = Vec::with_capacity(row3);
        for (y, row) in buf[..n].chunks_mut(width).enumerate() {
            comp_row(&s, y, row, &mut rowv);
        }
    }
    crate::gfx::scratch::put_f32(s);
    crate::gfx::scratch::put_f32(tmp);
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
    fn grad_triangle_blends_between_vertex_colors() {
        let (w, h) = (40usize, 40usize);
        let mut buf = vec![0u32; w * h];
        // red apex top, blue base â€” midline should be a red/blue mix.
        fill_triangle_grad(
            &mut buf, w, h, 1.0, 0, false, false, 20.0, 2.0, 0xFF0000, 2.0, 38.0, 0x0000FF, 38.0,
            38.0, 0x0000FF,
        );
        let mid = buf[20 * w + 20];
        let (r, b) = ((mid >> 16) & 0xFF, mid & 0xFF);
        eprintln!("mid=0x{mid:06X}");
        assert!(r > 20 && b > 20, "interior should mix both vertex colours");
    }

    #[test]
    fn grad_rect_runs_dark_to_light() {
        let (w, h) = (32usize, 8usize);
        let mut buf = vec![0u32; w * h];
        // horizontal blackâ†’white gradient
        fill_rect_grad(
            &mut buf, w, h, 1.0, 0, false, false, 0.0, 0.0, 32.0, 8.0, 0x000000, 0xFFFFFF, 0,
        );
        let left = buf[4 * w + 1] & 0xFF;
        let right = buf[4 * w + 30] & 0xFF;
        eprintln!("left={left} right={right}");
        assert!(
            right > left + 100,
            "right edge must be much brighter than left"
        );
    }

    #[test]
    fn soft_disc_is_darkest_at_center() {
        let (w, h) = (40usize, 40usize);
        let mut buf = vec![0xFFFFFFu32; w * h]; // white bg
                                                // black shadow, alpha 0.8, soft edge
        fill_disc_soft(
            &mut buf, w, h, 20.0, 20.0, 12.0, 12.0, 0x000000, 0.8, 0.5, 0, false,
        );
        let center = buf[20 * w + 20] & 0xFF;
        let rim = buf[20 * w + 30] & 0xFF; // ~10px out, in the feathered region
        let corner = buf[0] & 0xFF; // untouched white
        eprintln!("center={center} rim={rim} corner={corner}");
        assert!(center < rim, "centre must be darker than the feathered rim");
        assert_eq!(corner, 0xFF, "outside the radius stays untouched");
    }

    #[test]
    fn zbuffer_keeps_nearest_regardless_of_order() {
        let (w, h) = (16usize, 16usize);
        let tri = |x: [f32; 3]| (x[0], x[1], x[2]);
        let _ = tri;
        // Cover the whole quad twice: a FAR red triangle drawn last must NOT
        // overwrite a NEAR blue triangle drawn first â€” the z-test rejects it.
        let mut buf = vec![0u32; w * h];
        let mut z = vec![f32::INFINITY; w * h];
        fill_triangle_z(
            &mut buf, &mut z, w, h, 0x0000FF, 0.0, 0.0, 1.0, 16.0, 0.0, 1.0, 0.0, 16.0, 1.0,
        ); // near, z=1
        fill_triangle_z(
            &mut buf, &mut z, w, h, 0xFF0000, 0.0, 0.0, 9.0, 16.0, 0.0, 9.0, 0.0, 16.0, 9.0,
        ); // far,  z=9
        assert_eq!(
            buf[2 * w + 2],
            0x0000FF,
            "nearer blue must survive a later far draw"
        );

        // And the near triangle wins even when drawn second.
        let mut buf2 = vec![0u32; w * h];
        let mut z2 = vec![f32::INFINITY; w * h];
        fill_triangle_z(
            &mut buf2, &mut z2, w, h, 0xFF0000, 0.0, 0.0, 9.0, 16.0, 0.0, 9.0, 0.0, 16.0, 9.0,
        ); // far
        fill_triangle_z(
            &mut buf2, &mut z2, w, h, 0x0000FF, 0.0, 0.0, 1.0, 16.0, 0.0, 1.0, 0.0, 16.0, 1.0,
        ); // near
        assert_eq!(
            buf2[2 * w + 2],
            0x0000FF,
            "nearer blue wins regardless of order"
        );
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
        fill_contours_aa(&mut buf, w, h, 0xFFFFFF, false, &[outer, inner]);
        let body = buf[10 * w + 20]; // in the ring body (top stroke)
        let hole = buf[20 * w + 20]; // center hole
        eprintln!("body=0x{body:06X} hole=0x{hole:06X}");
        assert!(body != 0, "ring body should be filled");
        assert!(hole == 0, "hole should be empty");
    }

    #[test]
    fn draw_line_sets_unlit_tag() {
        let (w, h) = (10usize, 10usize);
        let mut buf = vec![0u32; w * h];
        draw_line(&mut buf, w, h, 0x00FF00, 1.0, 5.0, 8.0, 5.0);
        let p = buf[5 * w + 5];
        assert_eq!(
            p & crate::gfx::UNLIT,
            crate::gfx::UNLIT,
            "line pixel must be tagged unlit"
        );
        assert_eq!(
            p & crate::gfx::RGB_MASK,
            0x00FF00,
            "line colour must be exact"
        );
    }

    #[test]
    fn draw_line_blend_additive_preserves_tag() {
        let (w, h) = (10usize, 10usize);
        let mut buf = vec![0u32; w * h];
        draw_line_blend(&mut buf, w, h, 0x00FF00, 1, 1.0, 1.0, 5.0, 8.0, 5.0);
        let p = buf[5 * w + 5];
        assert_eq!(
            p & crate::gfx::UNLIT,
            crate::gfx::UNLIT,
            "additive line pixel must be tagged unlit"
        );
    }

    #[test]
    fn draw_line_blend_composite_preserves_tag() {
        let (w, h) = (10usize, 10usize);
        let mut buf = vec![0u32; w * h];
        draw_line_blend(&mut buf, w, h, 0x00FF00, 0, 0.5, 1.0, 5.0, 8.0, 5.0);
        let p = buf[5 * w + 5];
        assert_eq!(
            p & crate::gfx::UNLIT,
            crate::gfx::UNLIT,
            "composite line pixel must be tagged unlit"
        );
    }

    #[test]
    fn fill_contours_aa_tags_covered_pixels() {
        let (w, h) = (20usize, 20usize);
        let mut buf = vec![0u32; w * h];
        let sq = vec![vec![
            [5.0, 5.0],
            [15.0, 5.0],
            [15.0, 15.0],
            [5.0, 15.0],
            [5.0, 5.0],
        ]];
        fill_contours_aa(&mut buf, w, h, 0xFFFFFF, false, &sq);
        let p = buf[10 * w + 10];
        assert_eq!(
            p & crate::gfx::UNLIT,
            crate::gfx::UNLIT,
            "text/glyph fill must be tagged unlit"
        );
    }

    #[test]
    fn fill_triangle_gouraud_tags_when_unlit() {
        // bands = 0: real callers always pass 0 for flat-shaded/unlit triangles
        // (see gfx::mod submit_triangle), so posterisation never runs here.
        let (w, h) = (20usize, 20usize);
        let mut buf = vec![0u32; w * h];
        fill_triangle_gouraud(
            &mut buf, w, h, 2.0, 2.0, 0x00FF00, 18.0, 2.0, 0x00FF00, 2.0, 18.0, 0x00FF00, 0, 0.0,
            1.0, 0, true,
        );
        let p = buf[10 * w + 5];
        assert_eq!(
            p & crate::gfx::UNLIT,
            crate::gfx::UNLIT,
            "flat-shaded triangle must tag unlit"
        );
        assert_eq!(p & crate::gfx::RGB_MASK, 0x00FF00);
    }

    #[test]
    fn fill_triangle_gouraud_no_tag_when_lit() {
        let (w, h) = (20usize, 20usize);
        let mut buf = vec![0u32; w * h];
        fill_triangle_gouraud(
            &mut buf, w, h, 2.0, 2.0, 0x00FF00, 18.0, 2.0, 0x00FF00, 2.0, 18.0, 0x00FF00, 3, 0.0,
            1.0, 0, false,
        );
        let p = buf[10 * w + 5];
        assert_eq!(
            p & crate::gfx::UNLIT,
            0,
            "lit triangle must not be tagged unlit"
        );
    }

    #[test]
    fn fill_triangle_gouraud_blend_mode_preserves_tag() {
        let (w, h) = (20usize, 20usize);
        let mut buf = vec![0u32; w * h];
        fill_triangle_gouraud(
            &mut buf, w, h, 2.0, 2.0, 0x00FF00, 18.0, 2.0, 0x00FF00, 2.0, 18.0, 0x00FF00, 3, 0.0,
            1.0, 1, true,
        );
        let p = buf[10 * w + 5];
        assert_eq!(
            p & crate::gfx::UNLIT,
            crate::gfx::UNLIT,
            "additive-blended flat triangle must tag unlit"
        );
    }

    #[test]
    fn fill_triangle_gouraud_z_tags_when_unlit() {
        let (w, h) = (20usize, 20usize);
        let mut buf = vec![0u32; w * h];
        let mut z = vec![f32::INFINITY; w * h];
        fill_triangle_gouraud_z(
            &mut buf, &mut z, w, h, 2.0, 2.0, 1.0, 0x00FF00, 18.0, 2.0, 1.0, 0x00FF00, 2.0, 18.0,
            1.0, 0x00FF00, 3, 0.0, 1.0, 0, true,
        );
        let p = buf[10 * w + 5];
        assert_eq!(
            p & crate::gfx::UNLIT,
            crate::gfx::UNLIT,
            "flat-shaded z-tested triangle must tag unlit"
        );
    }

    #[test]
    fn fill_triangle_over_line_clears_tag() {
        // Painter's-order guarantee: a triangle rasterized after a line must
        // fully overwrite the line's pixel, including the UNLIT tag.
        let (w, h) = (20usize, 20usize);
        let mut buf = vec![0u32; w * h];
        let mut z = vec![f32::INFINITY; w * h];
        draw_line(&mut buf, w, h, 0x00FF00, 0.0, 10.0, 19.0, 10.0);
        assert_eq!(buf[10 * w + 10] & crate::gfx::UNLIT, crate::gfx::UNLIT);
        // A triangle large enough to fully cover the small buffer.
        fill_triangle_z(
            &mut buf, &mut z, w, h, 0xFF0000, -100.0, -100.0, 1.0, 200.0, -100.0, 1.0, -100.0,
            200.0, 1.0,
        );
        let p = buf[10 * w + 10];
        assert_eq!(
            p & crate::gfx::UNLIT,
            0,
            "triangle overwrite must clear the unlit tag"
        );
        assert_eq!(p, 0xFF0000, "triangle colour must win");
    }

    // Count (pixels exactly == color, non-zero pixels that are NOT the color).
    // The second bucket represents partial-coverage / smoothed edge pixels.
    fn coverage_buckets(buf: &[u32], color: u32) -> (usize, usize) {
        let color = color & crate::gfx::RGB_MASK;
        let mut exact = 0;
        let mut partial = 0;
        for &p in buf {
            let p = p & crate::gfx::RGB_MASK;
            if p == color {
                exact += 1;
            } else if p != 0 {
                partial += 1;
            }
        }
        (exact, partial)
    }

    #[test]
    fn aliased_line_is_fully_opaque() {
        let (w, h) = (64usize, 64usize);
        let mut buf = vec![0u32; w * h];
        draw_line(&mut buf, w, h, 0xFFFFFF, 4.0, 4.0, 60.0, 40.0);
        let (exact, partial) = coverage_buckets(&buf, 0xFFFFFF);
        eprintln!("aliased line: exact={exact} partial={partial}");
        assert!(exact > 0, "aliased line must draw pixels");
        assert_eq!(
            partial, 0,
            "aliased line must be fully opaque (no smoothing)"
        );
    }

    #[test]
    fn aa_line_has_smoothed_edges() {
        let (w, h) = (64usize, 64usize);
        let mut buf = vec![0u32; w * h];
        draw_line_aa(&mut buf, w, h, 0xFFFFFF, false, 4.0, 4.0, 60.0, 40.0);
        let (exact, partial) = coverage_buckets(&buf, 0xFFFFFF);
        eprintln!("aa line: exact={exact} partial={partial}");
        assert!(exact > 0, "AA line keeps opaque full-coverage core pixels");
        assert!(
            partial > 0,
            "AA line must produce smoothed partial-coverage pixels"
        );
    }

    #[test]
    fn arc_aliased_opaque_but_aa_is_smooth() {
        let (w, h) = (64usize, 64usize);
        let color = 0xFFFFFFu32;
        let (cx, cy, r) = (32.0, 32.0, 20.0);
        let full = std::f32::consts::TAU;

        let mut aliased = vec![0u32; w * h];
        draw_arc(
            &mut aliased,
            w,
            h,
            color,
            false,
            false,
            cx,
            cy,
            r,
            0.0,
            full,
            96,
        );
        let (exact, partial) = coverage_buckets(&aliased, color);
        eprintln!("aliased arc: exact={exact} partial={partial}");
        assert!(exact > 0, "aliased arc must draw pixels");
        assert_eq!(partial, 0, "aliased arc must be fully opaque");

        let mut aa = vec![0u32; w * h];
        draw_arc(&mut aa, w, h, color, true, false, cx, cy, r, 0.0, full, 96);
        let (_exact, partial) = coverage_buckets(&aa, color);
        eprintln!("aa arc: partial={partial}");
        assert!(
            partial > 0,
            "AA arc must produce smoothed partial-coverage pixels"
        );
    }

    #[test]
    fn gouraud_posterize_bands_zero_is_smooth_passthrough() {
        // bands < 2 must not alter the interpolated colour at all — this is
        // the path every non-material triangle takes, and it must render
        // identically to before per-pixel posterisation existed.
        let (w, h) = (20usize, 20usize);
        let mut buf = vec![0u32; w * h];
        fill_triangle_gouraud(
            &mut buf, w, h, 2.0, 2.0, 0x00FF0000, 18.0, 2.0, 0x0000FF00, 2.0, 18.0, 0x00FF0000, 0,
            0.0, 1.0, 0, false,
        );
        let mut smooth = vec![0u32; w * h];
        // Same triangle, computed by hand: interior pixels should vary
        // continuously (multiple distinct colours), not collapse to a few
        // discrete bands.
        fill_triangle_gouraud(
            &mut smooth,
            w,
            h,
            2.0,
            2.0,
            0x00FF0000,
            18.0,
            2.0,
            0x0000FF00,
            2.0,
            18.0,
            0x00FF0000,
            0,
            0.0,
            1.0,
            0,
            false,
        );
        assert_eq!(buf, smooth, "bands=0 must be deterministic passthrough");
        let distinct: std::collections::HashSet<u32> =
            buf.iter().filter(|&&p| p != 0).copied().collect();
        assert!(
            distinct.len() > 4,
            "smooth Gouraud gradient must not collapse to a handful of bands"
        );
    }

    #[test]
    fn gouraud_posterize_bands_four_gives_few_distinct_levels() {
        // With bands >= 2 the interior must posterise to a small, bounded
        // number of luminance levels instead of a continuous gradient — the
        // per-pixel fix for tile-corner popping (band edges follow the pixel
        // gradient, not the triangle's vertices).
        let (w, h) = (60usize, 60usize);
        let mut buf = vec![0u32; w * h];
        fill_triangle_gouraud(
            &mut buf, w, h, 2.0, 2.0, 0x00202020, 58.0, 2.0, 0x00E0E0E0, 2.0, 58.0, 0x00202020, 4,
            0.0, 1.0, 0, false,
        );
        let distinct: std::collections::HashSet<u32> =
            buf.iter().filter(|&&p| p != 0).copied().collect();
        assert!(
            distinct.len() <= 4,
            "4-band posterisation must not produce more than 4 distinct levels, got {}",
            distinct.len()
        );
    }

    #[test]
    fn gouraud_posterize_softness_one_matches_unposterized() {
        // softness = 1.0 fully crossfades back to the raw interpolated colour
        // (see `ling_graphics::shading::posterize_soft`), so it must match the
        // bands=0 render within float/rounding tolerance.
        let (w, h) = (20usize, 20usize);
        let mut hard_off = vec![0u32; w * h];
        fill_triangle_gouraud(
            &mut hard_off,
            w,
            h,
            2.0,
            2.0,
            0x00FF0000,
            18.0,
            2.0,
            0x0000FF00,
            2.0,
            18.0,
            0x00FF0000,
            0,
            0.0,
            1.0,
            0,
            false,
        );
        let mut soft = vec![0u32; w * h];
        fill_triangle_gouraud(
            &mut soft, w, h, 2.0, 2.0, 0x00FF0000, 18.0, 2.0, 0x0000FF00, 2.0, 18.0, 0x00FF0000, 4,
            1.0, 1.0, 0, false,
        );
        for i in 0..hard_off.len() {
            let (a, b) = (hard_off[i], soft[i]);
            for shift in [16, 8, 0] {
                let ca = (a >> shift) & 0xFF;
                let cb = (b >> shift) & 0xFF;
                assert!(
                    (ca as i32 - cb as i32).abs() <= 1,
                    "softness=1.0 must crossfade back to smooth Gouraud (pixel {i}, channel shift {shift}: {ca} vs {cb})"
                );
            }
        }
    }
}

// ── perspective-correct depth ────────────────────────────────────────────────
// Camera depth z is not linear in screen space, 1/(z + zdist) is. The z-tested
// fills interpolate that and convert back, so long walls and big floor slabs
// resolve against small objects correctly. The flush sets the camera's zdist.
static PERSP_ZD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x7FC0_0000);
pub fn set_persp_zd(zd: f32) {
    PERSP_ZD.store(zd.to_bits(), std::sync::atomic::Ordering::Relaxed);
}
#[inline]
pub(crate) fn persp_zd() -> f32 {
    f32::from_bits(PERSP_ZD.load(std::sync::atomic::Ordering::Relaxed))
}
/// Per-vertex 1/w for the perspective depth, or None (fall back to linear) when
/// zdist is unset or a vertex sits behind the eye (2-D depth_triangle callers).
#[inline]
fn persp_iw(z0: f32, z1: f32, z2: f32) -> Option<(f32, f32, f32, f32)> {
    let zd = persp_zd();
    if !zd.is_finite() {
        return None;
    }
    let (a, b, c) = (z0 + zd, z1 + zd, z2 + zd);
    if a <= 1e-4 || b <= 1e-4 || c <= 1e-4 {
        return None;
    }
    Some((1.0 / a, 1.0 / b, 1.0 / c, zd))
}
#[inline]
fn persp_z(p: Option<(f32, f32, f32, f32)>, w0: f32, w1: f32, w2: f32, z0: f32, z1: f32, z2: f32) -> f32 {
    match p {
        Some((i0, i1, i2, zd)) => {
            let q = i0 * w0 + i1 * w1 + i2 * w2;
            if q > 1e-12 { 1.0 / q - zd } else { z0 * w0 + z1 * w1 + z2 * w2 }
        },
        None => z0 * w0 + z1 * w1 + z2 * w2,
    }
}

// ── textured triangles (game3d tm_* / pfx_* builtins) ───────────────────────

#[inline]
fn tex_fetch(t: &crate::gfx::depth::TexData, x: i32, y: i32) -> u32 {
    let xm = (t.w - 1) as i32;
    let ym = (t.h - 1) as i32;
    t.px[((y & ym) as u32 * t.w + (x & xm) as u32) as usize]
}

/// Bilinear, wrapping texel fetch at texture coordinates (u, v) in texels.
/// 8-bit fixed-point weights; the four channels blend two-at-a-time in packed
/// 16-bit lanes (RB / AG) instead of 16 float lerps through a loop.
#[inline]
fn tex_bilinear(t: &crate::gfx::depth::TexData, u: f32, v: f32) -> [f32; 4] {
    let u = u - 0.5;
    let v = v - 0.5;
    let x0 = u.floor();
    let y0 = v.floor();
    let fx = ((u - x0) * 256.0) as u32;
    let fy = ((v - y0) * 256.0) as u32;
    let xm = (t.w - 1) as i32;
    let ym = (t.h - 1) as i32;
    let (xi, yi) = (x0 as i32, y0 as i32);
    let xa = (xi & xm) as usize;
    let xb = ((xi + 1) & xm) as usize;
    let ra = ((yi & ym) as u32 * t.w) as usize;
    let rb = (((yi + 1) & ym) as u32 * t.w) as usize;
    let c00 = t.px[ra + xa];
    let c10 = t.px[ra + xb];
    let c01 = t.px[rb + xa];
    let c11 = t.px[rb + xb];
    const M: u32 = 0x00FF_00FF;
    #[inline(always)]
    fn lerp2(a: u32, b: u32, f: u32) -> u32 {
        ((a * (256 - f) + b * f) >> 8) & M
    }
    let rb_top = lerp2(c00 & M, c10 & M, fx);
    let rb_bot = lerp2(c01 & M, c11 & M, fx);
    let ag_top = lerp2((c00 >> 8) & M, (c10 >> 8) & M, fx);
    let ag_bot = lerp2((c01 >> 8) & M, (c11 >> 8) & M, fx);
    let rbv = lerp2(rb_top, rb_bot, fy);
    let agv = lerp2(ag_top, ag_bot, fy);
    [((rbv >> 16) & 255) as f32, (agv & 255) as f32, (rbv & 255) as f32, ((agv >> 16) & 255) as f32]
}

/// Perspective-correct textured triangle with optional z-test / z-write, pen
/// `alpha`, blend `mode` (0 normal, 1 additive, anything else normal), `tint`
/// (0xRRGGBB multiplier) and a soft see-through `hole` (screen x, y, radius,
/// depth): pixels nearer than `depth` fade to 12% inside the radius so whatever
/// stands behind a wall stays visible. `ysh` is the band's first row.
#[allow(clippy::too_many_arguments)]
pub fn fill_triangle_tex(
    buf: &mut [u32],
    zbuf: Option<&mut [f32]>,
    width: usize,
    height: usize,
    ysh: f32,
    v: &[crate::gfx::depth::TexVert; 3],
    tex: &crate::gfx::depth::TexData,
    tint: u32,
    hole: [f32; 4],
    zwrite: bool,
    mode: u8,
    alpha: f32,
) {
    fill_triangle_tex_shaded(buf, zbuf, width, height, ysh, v, tex, tint, hole, zwrite, mode, alpha, None)
}

/// [`fill_triangle_tex`] with optional per-pixel point lighting (from the
/// perspective-correct world position) and distance fog.
#[allow(clippy::too_many_arguments)]
pub fn fill_triangle_tex_shaded(
    buf: &mut [u32],
    zbuf: Option<&mut [f32]>,
    width: usize,
    height: usize,
    ysh: f32,
    v: &[crate::gfx::depth::TexVert; 3],
    tex: &crate::gfx::depth::TexData,
    tint: u32,
    hole: [f32; 4],
    zwrite: bool,
    mode: u8,
    alpha: f32,
    shade: Option<&crate::gfx::depth::TexShade>,
) {
    fill_triangle_tex_shaded_mode::<Z_LT>(buf, zbuf, width, height, ysh, v, tex, tint, hole, zwrite, mode, alpha, shade)
}

/// [`fill_triangle_tex_shaded`] in depth mode `M` (see [`Z_LE`]).
#[allow(clippy::too_many_arguments)]
pub fn fill_triangle_tex_shaded_mode<const M: u8>(
    buf: &mut [u32],
    mut zbuf: Option<&mut [f32]>,
    width: usize,
    height: usize,
    ysh: f32,
    v: &[crate::gfx::depth::TexVert; 3],
    tex: &crate::gfx::depth::TexData,
    tint: u32,
    hole: [f32; 4],
    zwrite: bool,
    mode: u8,
    alpha: f32,
    shade: Option<&crate::gfx::depth::TexShade>,
) {
    let lit = shade.map(|s| s.lit).unwrap_or(false);
    let (fog_s, fog_e, fog_c) = shade.map(|s| (s.fog_start, s.fog_end, s.fog_color)).unwrap_or((0.0, 0.0, 0));
    let fog_on = fog_e > fog_s && fog_e > 0.0;
    let fcr = ((fog_c >> 16) & 255) as f32;
    let fcg = ((fog_c >> 8) & 255) as f32;
    let fcb = (fog_c & 255) as f32;
    if width == 0 || height == 0 || tex.px.is_empty() {
        return;
    }
    let (x0, y0) = (v[0].x, v[0].y - ysh);
    let (x1, y1) = (v[1].x, v[1].y - ysh);
    let (x2, y2) = (v[2].x, v[2].y - ysh);
    if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite() && x2.is_finite() && y2.is_finite()) {
        return;
    }
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }
    let min_x = x0.min(x1).min(x2).max(0.0) as i32;
    let max_x = x0.max(x1).max(x2).min(width as f32 - 1.0) as i32;
    let min_y = y0.min(y1).min(y2).max(0.0) as i32;
    let max_y = y0.max(y1).max(y2).min(height as f32 - 1.0) as i32;
    if min_x > max_x || min_y > max_y {
        return;
    }
    let area = (x1 - x0) * (y2 - y0) - (y1 - y0) * (x2 - x0);
    if area.abs() < 1e-6 {
        return;
    }
    let tr = ((tint >> 16) & 255) as f32 / 255.0;
    let tg = ((tint >> 8) & 255) as f32 / 255.0;
    let tb = (tint & 255) as f32 / 255.0;
    let (tw, th) = (tex.w as f32, tex.h as f32);
    let hole_on = hole[2] > 0.0;
    let (hx, hy, hr, hd) = (hole[0], hole[1] - ysh, hole[2], hole[3]);
    let de0 = -(y1 - y0);
    let de1 = -(y2 - y1);
    let de2 = -(y0 - y2);
    // the edge sum is the same everywhere in the triangle: one reciprocal per triangle
    let fx0c = min_x as f32 + 0.5;
    let fy0c = min_y as f32 + 0.5;
    let tot0 = ((x1 - x0) * (fy0c - y0) - (y1 - y0) * (fx0c - x0))
        + ((x2 - x1) * (fy0c - y1) - (y2 - y1) * (fx0c - x1))
        + ((x0 - x2) * (fy0c - y2) - (y0 - y2) * (fx0c - x2));
    if tot0.abs() < 1e-9 {
        return;
    }
    let inv_tot = 1.0 / tot0;
    let zd = persp_zd();
    let persp_ok = zd.is_finite() && v[0].z + zd > 0.0;
    // equal 1/w on every vertex (sprites, billboards): affine, no per-pixel divide
    let affine = (v[0].iw - v[1].iw).abs() < 1e-9 && (v[0].iw - v[2].iw).abs() < 1e-9;
    let sgn = if tot0 > 0.0 { 1.0f32 } else { -1.0 };
    let de = [de0 * sgn, de1 * sgn, de2 * sgn];
    let ripples_on = shade.map(|s| !s.ripples.is_empty()).unwrap_or(false);
    for py in min_y..=max_y {
        let fy = py as f32 + 0.5;
        let fx0 = min_x as f32 + 0.5;
        let mut e0 = (x1 - x0) * (fy - y0) - (y1 - y0) * (fx0 - x0);
        let mut e1 = (x2 - x1) * (fy - y1) - (y2 - y1) * (fx0 - x1);
        let mut e2 = (x0 - x2) * (fy - y2) - (y0 - y2) * (fx0 - x2);
        // analytic span of this row: solve each edge function >= 0 (in the winding's
        // sign) for the pixel offset, widen by a pixel, and keep the exact per-pixel
        // inside test below, so coverage is unchanged while the empty bbox corners of
        // long thin triangles are skipped
        let mut lo = 0i32;
        let mut hi = max_x - min_x;
        let mut empty = false;
        for (ev, d) in [(e0, de[0]), (e1, de[1]), (e2, de[2])] {
            let a = ev * sgn;
            if d.abs() < 1e-12 {
                if a < 0.0 { empty = true; }
            } else {
                let t = -a / d;
                if d > 0.0 {
                    lo = lo.max((t.floor() as i32).saturating_sub(1));
                } else {
                    hi = hi.min((t.ceil() as i32).saturating_add(1));
                }
            }
        }
        if empty || lo > hi {
            continue;
        }
        let lof = lo as f32;
        e0 += de0 * lof;
        e1 += de1 * lof;
        e2 += de2 * lof;
        let row = py as usize * width;
        let mut in_span = false;
        for px in (min_x + lo)..=(min_x + hi) {
            let inside = (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0) || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0);
            if inside {
                in_span = true;
                {
                    let w2 = e0 * inv_tot;
                    let w0 = e1 * inv_tot;
                    let w1 = e2 * inv_tot;
                    let iw = if affine { v[0].iw } else { v[0].iw * w0 + v[1].iw * w1 + v[2].iw * w2 };
                    // one reciprocal serves both the depth and the perspective-correct uv
                    let rcp = if iw > 1e-12 { 1.0 / iw } else { 0.0 };
                    let z = if affine {
                        v[0].z
                    } else if persp_ok && iw > 1e-12 {
                        rcp - zd
                    } else {
                        v[0].z * w0 + v[1].z * w1 + v[2].z * w2
                    };
                    let idx = row + px as usize;
                    let pass = match zbuf.as_deref() {
                        Some(zb) => z_pass::<M>(z, zb[idx]),
                        None => true,
                    };
                    if M == Z_PREPASS {
                        // only called for opaque, non-holed, z-writing calls: every
                        // covered pixel with a valid 1/w writes its depth
                        if pass && iw > 1e-12 {
                            if let Some(zb) = zbuf.as_deref_mut() {
                                zb[idx] = z;
                            }
                        }
                    } else if pass {
                        if iw > 1e-12 {
                            let inv = rcp;
                            let mut u = (v[0].uw * w0 + v[1].uw * w1 + v[2].uw * w2) * inv;
                            let mut vv = (v[0].vw * w0 + v[1].vw * w1 + v[2].vw * w2) * inv;
                            // shockwave rings: the wave height h = env * sin(phase) is never
                            // drawn as geometry; its radial slope bends the texture lookup like
                            // light through a rippling surface, lights one flank and darkens the
                            // other, and the crests catch a soft highlight
                            let (mut bump, mut glint) = (1.0f32, 0.0f32);
                            if ripples_on {
                                let rp = &shade.unwrap().ripples;
                                let wx = (v[0].wx * w0 + v[1].wx * w1 + v[2].wx * w2) * inv;
                                let wz = (v[0].wz * w0 + v[1].wz * w1 + v[2].wz * w2) * inv;
                                let (mut gu, mut gv, mut sl) = (0.0f32, 0.0f32, 0.0f32);
                                for r in rp {
                                    let (dx, dz) = (wx - r[0], wz - r[1]);
                                    let d = (dx * dx + dz * dz).sqrt().max(1e-3);
                                    let x = d - r[2];
                                    if x.abs() < r[5] * 3.0 {
                                        let env = r[3] * (-(x / r[5]) * (x / r[5])).exp();
                                        let ph = x * std::f32::consts::TAU / r[4];
                                        let slope = env * ph.cos();
                                        gu += dx / d * slope;
                                        gv += dz / d * slope;
                                        sl += slope;
                                        glint += (env * ph.sin()).max(0.0);
                                    }
                                }
                                u += gu * 0.035;
                                vv += gv * 0.035;
                                bump = (1.0 + sl * 0.55).clamp(0.35, 1.9);
                                glint = (glint * glint * 90.0).min(110.0);
                            }
                            let s = tex_bilinear(tex, u * tw, vv * th);
                            let mut a = s[3] / 255.0 * alpha;
                            if hole_on && z < hd {
                                let dx = px as f32 + 0.5 - hx;
                                let dy = py as f32 + 0.5 - hy;
                                let d = (dx * dx + dy * dy).sqrt();
                                let t = ((d - hr * 0.55) / (hr * 0.45)).clamp(0.0, 1.0);
                                let t = t * t * (3.0 - 2.0 * t);
                                a *= 0.12 + 0.88 * t;
                            }
                            if a > 0.004 {
                                let mut sr = s[0] * tr * bump + glint;
                                let mut sg = s[1] * tg * bump + glint;
                                let mut sb = s[2] * tb * bump + glint;
                                if lit {
                                    if let Some(sh) = shade {
                                        let inv = if affine { 1.0 / v[0].iw } else { 1.0 / iw };
                                        let px_w = (v[0].wx * w0 + v[1].wx * w1 + v[2].wx * w2) * inv;
                                        let py_w = (v[0].wy * w0 + v[1].wy * w1 + v[2].wy * w2) * inv;
                                        let pz_w = (v[0].wz * w0 + v[1].wz * w1 + v[2].wz * w2) * inv;
                                        let (mut lr, mut lg, mut lb) = (sh.amb, sh.amb, sh.amb);
                                        for l in &sh.lights {
                                            let dx = px_w - l[0];
                                            let dy = py_w - l[1];
                                            let dz = pz_w - l[2];
                                            let d2 = dx * dx + dy * dy + dz * dz;
                                            let r2 = l[7] * l[7];
                                            if d2 < r2 {
                                                let k = 1.0 - (d2 / r2).sqrt();
                                                let e = k * k * l[6];
                                                lr += l[3] * e;
                                                lg += l[4] * e;
                                                lb += l[5] * e;
                                            }
                                        }
                                        sr = (sr * lr).min(255.0);
                                        sg = (sg * lg).min(255.0);
                                        sb = (sb * lb).min(255.0);
                                    }
                                }
                                if fog_on {
                                    let f = ((z - fog_s) / (fog_e - fog_s)).clamp(0.0, 1.0);
                                    sr += (fcr - sr) * f;
                                    sg += (fcg - sg) * f;
                                    sb += (fcb - sb) * f;
                                }
                                let dst = buf[idx];
                                let dr = ((dst >> 16) & 255) as f32;
                                let dg = ((dst >> 8) & 255) as f32;
                                let db = (dst & 255) as f32;
                                let (r, g, b) = if mode == 1 {
                                    ((dr + sr * a).min(255.0), (dg + sg * a).min(255.0), (db + sb * a).min(255.0))
                                } else {
                                    (dr + (sr - dr) * a, dg + (sg - dg) * a, db + (sb - db) * a)
                                };
                                buf[idx] = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                                if zwrite && a > 0.97 {
                                    if let Some(zb) = zbuf.as_deref_mut() {
                                        zb[idx] = z;
                                    }
                                }
                            }
                        }
                    }
                }
            } else if in_span {
                break;
            }
            e0 += de0;
            e1 += de1;
            e2 += de2;
        }
    }
}
