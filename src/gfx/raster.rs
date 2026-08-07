// src/gfx/raster.rs â€” software 2-D pixel rasteriser
// All buffers are row-major Vec<u32> with pixels as 0x00RRGGBB.

/// Fill a triangle using incremental edge evaluation (Pineda 1988).
/// For convex triangles we break early once we exit the span â€” roughly 2Ã— faster
/// than scanning the full bounding box.
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

    // x-step deltas: adding 1 to fx changes each edge by -(Î”y)
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
                break; // convex â€” span finished for this scanline
            }
            e0 += de0;
            e1 += de1;
            e2 += de2;
        }
    }
}

/// Posterise one interpolated Gouraud pixel (0..255 channels) into `bands`
/// discrete luminance levels, chroma preserved, softened by `softness`. Runs
/// once per pixel on the smoothly interpolated colour, so band edges follow
/// the shaded surface instead of being pinned to triangle/mesh vertices —
/// see `ling_graphics::shading` for the underlying posterisation model.
/// `bands < 2` skips the work entirely (smooth Gouraud, unchanged).
#[inline]
fn posterize_span(r: f32, g: f32, b: f32, bands: u32, softness: f32) -> (f32, f32, f32) {
    if bands < 2 {
        return (r, g, b);
    }
    let linear = [r / 255.0, g / 255.0, b / 255.0];
    let out = ling_graphics::shading::posterize_soft(linear, bands, softness);
    (out[0] * 255.0, out[1] * 255.0, out[2] * 255.0)
}

/// Gouraud-interpolated triangle with per-pixel cel-shade posterisation.
///
/// Vertex colours are linearly interpolated with barycentric weights; the
/// resulting per-pixel luminance is snapped to 3 discrete toon bands (shadow /
/// mid / lit) matching `cel_quantize` â€” giving a smooth gradient of bands across
/// the face rather than a per-vertex flat-shade seam.
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
    let blended = alpha < 0.999 || mode != 0;

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
                    let w2 = e0 / tot;
                    let w0 = e1 / tot;
                    let w1 = e2 / tot;
                    let r = r0 * w0 + r1 * w1 + r2 * w2;
                    let g = g0 * w0 + g1 * w1 + g2 * w2;
                    let b = b0 * w0 + b1 * w1 + b2 * w2;
                    let (r, g, b) = posterize_span(r, g, b, bands, softness);
                    let packed = ((r.clamp(0.0, 255.0) as u32) << 16)
                        | ((g.clamp(0.0, 255.0) as u32) << 8)
                        | (b.clamp(0.0, 255.0) as u32);
                    if blended {
                        composite_pixel(buf, width, height, px, py, packed, alpha, mode, false);
                        if tag_unlit {
                            buf[row + px as usize] |= crate::gfx::UNLIT;
                        }
                    } else {
                        buf[row + px as usize] = if tag_unlit {
                            packed | crate::gfx::UNLIT
                        } else {
                            packed
                        };
                    }
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
        let row = py as usize * width;
        for px in min_x..=max_x {
            let inside =
                (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0) || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0);
            if inside {
                let tot = e0 + e1 + e2;
                if tot.abs() > 1e-6 {
                    let w2 = e0 / tot;
                    let w0 = e1 / tot;
                    let w1 = e2 / tot;
                    let z = z0 * w0 + z1 * w1 + z2 * w2;
                    let idx = row + px as usize;
                    if z < zbuf[idx] {
                        zbuf[idx] = z;
                        buf[idx] = color;
                    }
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
    let fast_add = mode == 1; // integer saturating-add fast path
    let src = if fast_add && alpha < 0.999 {
        scale_rgb(color, alpha)
    } else {
        color
    };

    let min_x = x0.min(x1).min(x2).max(0.0) as i32;
    let max_x = x0.max(x1).max(x2).min(width as f32 - 1.0) as i32;
    let min_y = y0.min(y1).min(y2).max(0.0) as i32;
    let max_y = y0.max(y1).max(y2).min(height as f32 - 1.0) as i32;
    if min_x > max_x || min_y > max_y {
        return;
    }

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
                if fast_add {
                    let idx = py as usize * width + px as usize;
                    buf[idx] = add_sat(buf[idx], src);
                } else {
                    composite_pixel(buf, width, height, px, py, color, alpha, mode, false);
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
    let fast_add = mode == 1; // integer saturating-add fast path
    let src = if fast_add && alpha < 0.999 {
        scale_rgb(color, alpha)
    } else {
        color
    };

    let min_x = x0.min(x1).min(x2).max(0.0) as i32;
    let max_x = x0.max(x1).max(x2).min(width as f32 - 1.0) as i32;
    let min_y = y0.min(y1).min(y2).max(0.0) as i32;
    let max_y = y0.max(y1).max(y2).min(height as f32 - 1.0) as i32;
    if min_x > max_x || min_y > max_y {
        return;
    }

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
        let row = py as usize * width;
        for px in min_x..=max_x {
            let inside =
                (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0) || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0);
            if inside {
                let tot = e0 + e1 + e2;
                if tot.abs() > 1e-6 {
                    let w2 = e0 / tot;
                    let w0 = e1 / tot;
                    let w1 = e2 / tot;
                    let z = z0 * w0 + z1 * w1 + z2 * w2;
                    if z < zbuf[row + px as usize] {
                        if fast_add {
                            let idx = row + px as usize;
                            buf[idx] = add_sat(buf[idx], src);
                        } else {
                            composite_pixel(buf, width, height, px, py, color, alpha, mode, false);
                        }
                    }
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
    let blended = alpha < 0.999 || mode != 0;

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
                    let w2 = e0 / tot;
                    let w0 = e1 / tot;
                    let w1 = e2 / tot;
                    let z = z0 * w0 + z1 * w1 + z2 * w2;
                    let idx = row + px as usize;
                    if z < zbuf[idx] {
                        let r = r0 * w0 + r1 * w1 + r2 * w2;
                        let g = g0 * w0 + g1 * w1 + g2 * w2;
                        let b = b0 * w0 + b1 * w1 + b2 * w2;
                        let (r, g, b) = posterize_span(r, g, b, bands, softness);
                        let packed = ((r.clamp(0.0, 255.0) as u32) << 16)
                            | ((g.clamp(0.0, 255.0) as u32) << 8)
                            | (b.clamp(0.0, 255.0) as u32);
                        if blended {
                            // Translucent: test depth but don't write it so layers stack correctly.
                            composite_pixel(buf, width, height, px, py, packed, alpha, mode, false);
                            if tag_unlit {
                                buf[idx] |= crate::gfx::UNLIT;
                            }
                        } else {
                            zbuf[idx] = z;
                            buf[idx] = if tag_unlit {
                                packed | crate::gfx::UNLIT
                            } else {
                                packed
                            };
                        }
                    }
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

    let xmax = (width - 1) as f32;
    let ymax = (height - 1) as f32;

    let (mut ax, mut ay, mut bx, mut by) = (x0, y0, x1, y1);
    if !cs_clip(&mut ax, &mut ay, &mut bx, &mut by, xmax, ymax) {
        return;
    }

    let color = color | crate::gfx::UNLIT;

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
    let xmax = (width - 1) as f32;
    let ymax = (height - 1) as f32;
    let (mut ax, mut ay, mut bx, mut by) = (x0, y0, x1, y1);
    if !cs_clip(&mut ax, &mut ay, &mut bx, &mut by, xmax, ymax) {
        return;
    }
    let fast_add = mode == 1;
    let src = if fast_add && alpha < 0.999 {
        scale_rgb(color, alpha.clamp(0.0, 1.0))
    } else {
        color
    };
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
            if fast_add {
                let idx = y as usize * width + x as usize;
                buf[idx] = add_sat(buf[idx], src) | crate::gfx::UNLIT;
            } else {
                composite_pixel(buf, width, height, x, y, color, alpha, mode, false);
                let idx = y as usize * width + x as usize;
                buf[idx] |= crate::gfx::UNLIT;
            }
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

// â”€â”€ Cohen-Sutherland helpers â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

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

/// Returns true if the segment (ax,ay)â†’(bx,by) has any part inside the viewport;
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
    buf[idx] = ((nr as u32) << 16) | ((ng as u32) << 8) | (nb as u32) | crate::gfx::UNLIT;
}

/// Xiaolin-Wu anti-aliased line. `additive` selects the blend mode.
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
    // during the full-res composite.
    let hw = width.div_ceil(2);
    let hh = height.div_ceil(2);
    let hn = hw * hh;
    let r_h = rad.div_ceil(2).max(1) as i32; // blur radius in half-res pixels
    let inv_win = 1.0 / (2 * r_h + 1) as f32;

    // Downsample (2×2 average) into half-res f32 planes.
    let mut rc = vec![0f32; hn];
    let mut gc = vec![0f32; hn];
    let mut bc = vec![0f32; hn];
    {
        let down_row = |hy: usize, rows: (&mut [f32], &mut [f32], &mut [f32])| {
            let y0 = (hy * 2).min(height - 1);
            let y1 = (hy * 2 + 1).min(height - 1);
            for hx in 0..hw {
                let x0 = (hx * 2).min(width - 1);
                let x1 = (hx * 2 + 1).min(width - 1);
                let (mut r, mut g, mut b) = (0f32, 0f32, 0f32);
                for &i in &[
                    y0 * width + x0,
                    y0 * width + x1,
                    y1 * width + x0,
                    y1 * width + x1,
                ] {
                    let p = buf[i];
                    r += ((p >> 16) & 0xFF) as f32;
                    g += ((p >> 8) & 0xFF) as f32;
                    b += (p & 0xFF) as f32;
                }
                rows.0[hx] = r * 0.25;
                rows.1[hx] = g * 0.25;
                rows.2[hx] = b * 0.25;
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            use rayon::prelude::*;
            rc.par_chunks_mut(hw)
                .zip(gc.par_chunks_mut(hw))
                .zip(bc.par_chunks_mut(hw))
                .enumerate()
                .for_each(|(hy, ((r, g), b))| down_row(hy, (r, g, b)));
        }
        #[cfg(target_arch = "wasm32")]
        for (hy, ((r, g), b)) in rc
            .chunks_mut(hw)
            .zip(gc.chunks_mut(hw))
            .zip(bc.chunks_mut(hw))
            .enumerate()
        {
            down_row(hy, (r, g, b));
        }
    }

    // Separable box blur on the half-res planes (H: sliding window; V: direct
    // small-window sum — both row-parallel on native).
    let hwi = hw as i32;
    let blur = |src: &[f32]| -> Vec<f32> {
        let mut tmp = vec![0f32; hn];
        let hpass = |y: usize, out_row: &mut [f32]| {
            let row = y * hw;
            let mut acc = 0.0f32;
            let mut k = -r_h;
            while k <= r_h {
                acc += src[row + k.clamp(0, hwi - 1) as usize];
                k += 1;
            }
            for (x, out) in out_row.iter_mut().enumerate() {
                *out = acc * inv_win;
                let lo = (x as i32 - r_h).clamp(0, hwi - 1) as usize;
                let hi2 = (x as i32 + r_h + 1).clamp(0, hwi - 1) as usize;
                acc += src[row + hi2] - src[row + lo];
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            use rayon::prelude::*;
            tmp.par_chunks_mut(hw)
                .enumerate()
                .for_each(|(y, row)| hpass(y, row));
        }
        #[cfg(target_arch = "wasm32")]
        for (y, row) in tmp.chunks_mut(hw).enumerate() {
            hpass(y, row);
        }

        let mut out = vec![0f32; hn];
        let vpass = |y: usize, out_row: &mut [f32]| {
            for (x, o) in out_row.iter_mut().enumerate() {
                let mut acc = 0.0f32;
                let mut k = -r_h;
                while k <= r_h {
                    let yy = (y as i32 + k).clamp(0, hh as i32 - 1) as usize;
                    acc += tmp[yy * hw + x];
                    k += 1;
                }
                *o = acc * inv_win;
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            use rayon::prelude::*;
            out.par_chunks_mut(hw)
                .enumerate()
                .for_each(|(y, row)| vpass(y, row));
        }
        #[cfg(target_arch = "wasm32")]
        for (y, row) in out.chunks_mut(hw).enumerate() {
            vpass(y, row);
        }
        out
    };

    let rb = blur(&rc);
    let gb = blur(&gc);
    let bb = blur(&bc);
    drop(rc);
    drop(gc);
    drop(bc);

    // Bilinear sample of a half-res plane at full-res pixel coords.
    let samp = |s: &[f32], fx: f32, fy: f32| -> f32 {
        let gx = (fx * 0.5).min(hw as f32 - 1.0).max(0.0);
        let gy = (fy * 0.5).min(hh as f32 - 1.0).max(0.0);
        let x0 = gx as usize;
        let y0 = gy as usize;
        let x1 = (x0 + 1).min(hw - 1);
        let y1 = (y0 + 1).min(hh - 1);
        let tx = gx - x0 as f32;
        let ty = gy - y0 as f32;
        let a = s[y0 * hw + x0] * (1.0 - tx) + s[y0 * hw + x1] * tx;
        let b = s[y1 * hw + x0] * (1.0 - tx) + s[y1 * hw + x1] * tx;
        a * (1.0 - ty) + b * ty
    };

    // Composite: lerp sharp→blurred by circle-of-confusion, preserving any
    // tag bits (UNLIT ink survives the blur untagged-quantisation). With
    // `oil` > 0 the blurred zone gets an oil-slick treatment: the red/blue
    // blurred planes are sampled with opposite horizontal offsets (iridescent
    // chroma fringe) and the hue is swirled slightly — water / heat haze /
    // far-field shimmer.
    let comp_row = |y: usize, row: &mut [u32]| {
        let fy = y as f32;
        for (x, px) in row.iter_mut().enumerate() {
            let i = y * width + x;
            let z = zbuf[i];
            let coc = if z.is_finite() {
                ((z - focus).abs() / range).clamp(0.0, 1.0)
            } else {
                1.0
            };
            if coc <= 0.003 {
                continue; // in focus — untouched
            }
            let s = 1.0 - coc;
            let p = *px;
            let tag = p & !crate::gfx::RGB_MASK;
            let pr = ((p >> 16) & 0xFF) as f32;
            let pg = ((p >> 8) & 0xFF) as f32;
            let pb = (p & 0xFF) as f32;
            let fx = x as f32;
            let (mut r, mut g, mut b);
            if oil > 0.0 {
                let off = coc * oil * 3.0;
                r = pr * s + samp(&rb, fx - off, fy) * coc;
                g = pg * s + samp(&gb, fx, fy) * coc;
                b = pb * s + samp(&bb, fx + off, fy) * coc;
                // gentle hue swirl toward an iridescent film
                let k = oil * coc * 0.22;
                let (r0, g0, b0) = (r, g, b);
                r = r0 * (1.0 - k) + b0 * k;
                g = g0 * (1.0 - k) + r0 * k;
                b = b0 * (1.0 - k) + g0 * k;
            } else {
                r = pr * s + samp(&rb, fx, fy) * coc;
                g = pg * s + samp(&gb, fx, fy) * coc;
                b = pb * s + samp(&bb, fx, fy) * coc;
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
        buf[..n]
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(y, row)| comp_row(y, row));
    }
    #[cfg(target_arch = "wasm32")]
    for (y, row) in buf[..n].chunks_mut(width).enumerate() {
        comp_row(y, row);
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
            &mut smooth, w, h, 2.0, 2.0, 0x00FF0000, 18.0, 2.0, 0x0000FF00, 2.0, 18.0, 0x00FF0000,
            0, 0.0, 1.0, 0, false,
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
            &mut hard_off, w, h, 2.0, 2.0, 0x00FF0000, 18.0, 2.0, 0x0000FF00, 2.0, 18.0,
            0x00FF0000, 0, 0.0, 1.0, 0, false,
        );
        let mut soft = vec![0u32; w * h];
        fill_triangle_gouraud(
            &mut soft, w, h, 2.0, 2.0, 0x00FF0000, 18.0, 2.0, 0x0000FF00, 2.0, 18.0, 0x00FF0000,
            4, 1.0, 1.0, 0, false,
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
