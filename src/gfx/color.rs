// src/gfx/color.rs — modern compositing & perceptual colour for the CPU raster.
//
// References (the "whitepaper" basis):
//   • Porter & Duff, "Compositing Digital Images", SIGGRAPH 1984
//       → the `over` operator and premultiplied alpha.
//   • Gamma-correct (linear-light) compositing: blend in linear RGB, store sRGB,
//       so alpha blends and gradients don't darken/shift hue.
//   • B. Ottosson, "A perceptual color space for image processing" (OkLab), 2020
//       → perceptually-uniform gradient interpolation and colour mixing.
//
// All framebuffer words are 0x00RRGGBB (opaque sRGB). Alpha is supplied per call.

// ── sRGB ⇄ linear ────────────────────────────────────────────────────────────

#[inline]
pub fn srgb_to_lin(c: u8) -> f32 {
    let s = c as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
pub fn lin_to_srgb(l: f32) -> f32 {
    let l = l.clamp(0.0, 1.0);
    let s = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5).clamp(0.0, 255.0)
}

#[inline]
pub fn unpack(c: u32) -> (u8, u8, u8) {
    (
        ((c >> 16) & 0xFF) as u8,
        ((c >> 8) & 0xFF) as u8,
        (c & 0xFF) as u8,
    )
}

#[inline]
pub fn pack(r: u32, g: u32, b: u32) -> u32 {
    ((r & 0xFF) << 16) | ((g & 0xFF) << 8) | (b & 0xFF)
}

// ── Blend modes (Porter-Duff `over` + photographic operators) ────────────────

/// Blend operator selected by `set_blend`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    Normal,
    Add,
    Multiply,
    Screen,
    Subtract,
    Overlay,
}

impl BlendMode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => BlendMode::Add,
            2 => BlendMode::Multiply,
            3 => BlendMode::Screen,
            4 => BlendMode::Subtract,
            5 => BlendMode::Overlay,
            _ => BlendMode::Normal,
        }
    }
}

#[inline]
fn blend_channel(mode: BlendMode, s: f32, d: f32) -> f32 {
    // s, d in [0,1]; returns the blended source colour (pre alpha-composite).
    match mode {
        BlendMode::Normal | BlendMode::Add | BlendMode::Subtract => s,
        BlendMode::Multiply => s * d,
        BlendMode::Screen => 1.0 - (1.0 - s) * (1.0 - d),
        BlendMode::Overlay => {
            if d < 0.5 {
                2.0 * s * d
            } else {
                1.0 - 2.0 * (1.0 - s) * (1.0 - d)
            }
        },
    }
}

/// Composite source colour `src` (0x00RRGGBB) at coverage·alpha `a` over the
/// destination word `dst`, using `mode`. When `linear` is true the maths is done
/// in linear light (gamma-correct) and converted back to sRGB; otherwise it runs
/// directly on sRGB bytes (the legacy/fast path).
#[inline]
pub fn composite(dst: u32, src: u32, a: f32, mode: BlendMode, linear: bool) -> u32 {
    let a = a.clamp(0.0, 1.0);
    if a <= 0.0 {
        return dst;
    }
    let (sr, sg, sb) = unpack(src);
    let (dr, dg, db) = unpack(dst);

    if linear {
        let s = [srgb_to_lin(sr), srgb_to_lin(sg), srgb_to_lin(sb)];
        let d = [srgb_to_lin(dr), srgb_to_lin(dg), srgb_to_lin(db)];
        let out = |i: usize| -> f32 {
            match mode {
                BlendMode::Add => (d[i] + s[i] * a).min(1.0),
                BlendMode::Subtract => (d[i] - s[i] * a).max(0.0),
                _ => {
                    let blended = blend_channel(mode, s[i], d[i]);
                    blended * a + d[i] * (1.0 - a) // premultiplied `over`
                },
            }
        };
        return pack(
            lin_to_srgb(out(0)) as u32,
            lin_to_srgb(out(1)) as u32,
            lin_to_srgb(out(2)) as u32,
        );
    }

    // Legacy sRGB-space compositing (matches the original blend_pixel maths).
    let s = [sr as f32, sg as f32, sb as f32];
    let d = [dr as f32, dg as f32, db as f32];
    let out = |i: usize| -> f32 {
        match mode {
            BlendMode::Add => (d[i] + s[i] * a).min(255.0),
            BlendMode::Subtract => (d[i] - s[i] * a).max(0.0),
            _ => {
                let sc = s[i] / 255.0;
                let dc = d[i] / 255.0;
                let blended = blend_channel(mode, sc, dc) * 255.0;
                blended * a + d[i] * (1.0 - a)
            },
        }
    };
    pack(out(0) as u32, out(1) as u32, out(2) as u32)
}

// ── OkLab perceptual colour (Ottosson 2020) ──────────────────────────────────

#[inline]
fn lin_rgb_to_oklab(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let l = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();
    (
        0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_,
        1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_,
        0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_,
    )
}

#[inline]
fn oklab_to_lin_rgb(l: f32, a: f32, b: f32) -> (f32, f32, f32) {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;
    (
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    )
}

/// Perceptually-uniform mix of two sRGB colours through OkLab. `t` ∈ [0,1].
#[inline]
pub fn mix_oklab(c0: u32, c1: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let (r0, g0, b0) = unpack(c0);
    let (r1, g1, b1) = unpack(c1);
    let a = lin_rgb_to_oklab(srgb_to_lin(r0), srgb_to_lin(g0), srgb_to_lin(b0));
    let b = lin_rgb_to_oklab(srgb_to_lin(r1), srgb_to_lin(g1), srgb_to_lin(b1));
    let l = a.0 + (b.0 - a.0) * t;
    let aa = a.1 + (b.1 - a.1) * t;
    let bb = a.2 + (b.2 - a.2) * t;
    let (lr, lg, lb) = oklab_to_lin_rgb(l, aa, bb);
    pack(
        lin_to_srgb(lr) as u32,
        lin_to_srgb(lg) as u32,
        lin_to_srgb(lb) as u32,
    )
}

/// Barycentric blend of three sRGB colours through OkLab (smooth gradient tris).
#[inline]
pub fn bary_oklab(c0: u32, c1: u32, c2: u32, w0: f32, w1: f32, w2: f32) -> u32 {
    let a = {
        let (r, g, b) = unpack(c0);
        lin_rgb_to_oklab(srgb_to_lin(r), srgb_to_lin(g), srgb_to_lin(b))
    };
    let b_ = {
        let (r, g, b) = unpack(c1);
        lin_rgb_to_oklab(srgb_to_lin(r), srgb_to_lin(g), srgb_to_lin(b))
    };
    let c = {
        let (r, g, b) = unpack(c2);
        lin_rgb_to_oklab(srgb_to_lin(r), srgb_to_lin(g), srgb_to_lin(b))
    };
    let l = a.0 * w0 + b_.0 * w1 + c.0 * w2;
    let aa = a.1 * w0 + b_.1 * w1 + c.1 * w2;
    let bb = a.2 * w0 + b_.2 * w1 + c.2 * w2;
    let (lr, lg, lb) = oklab_to_lin_rgb(l, aa, bb);
    pack(
        lin_to_srgb(lr) as u32,
        lin_to_srgb(lg) as u32,
        lin_to_srgb(lb) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_roundtrip() {
        for c in [0u8, 1, 64, 128, 200, 255] {
            let back = lin_to_srgb(srgb_to_lin(c)) as i32;
            assert!((back - c as i32).abs() <= 1, "sRGB roundtrip {c} -> {back}");
        }
    }

    #[test]
    fn oklab_midpoint_is_between() {
        // midpoint of black→white through OkLab should be a mid grey, and because
        // OkLab is perceptual it sits well above the naive sRGB 0.5*255≈128… no:
        // perceptual mid-grey of black/white is ~ L*0.5 → ~119–128. Just assert
        // it is a neutral grey strictly between the endpoints.
        let m = mix_oklab(0x000000, 0xFFFFFF, 0.5);
        let (r, g, b) = unpack(m);
        assert_eq!(r, g);
        assert_eq!(g, b);
        assert!(r > 30 && r < 230, "midpoint grey = {r}");
    }

    #[test]
    fn composite_over_opaque_replaces() {
        assert_eq!(
            composite(0x000000, 0xFF8040, 1.0, BlendMode::Normal, false),
            0xFF8040
        );
    }

    #[test]
    fn composite_zero_alpha_keeps_dst() {
        assert_eq!(
            composite(0x123456, 0xFFFFFF, 0.0, BlendMode::Normal, true),
            0x123456
        );
    }

    #[test]
    fn multiply_darkens() {
        // grey × grey should be darker than either in both pipelines.
        let out = composite(0x808080, 0x808080, 1.0, BlendMode::Multiply, false);
        let (r, _, _) = unpack(out);
        assert!(r < 0x80, "multiply should darken: {r:#x}");
    }
}
