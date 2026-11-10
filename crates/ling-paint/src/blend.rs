//! Layer blend modes (W3C compositing formulas, premultiplied in / out).

#[allow(unused_imports)]
use crate::prelude::*;

pub const BLEND_NAMES: [&str; 27] = [
    "Normal", "Multiply", "Screen", "Overlay", "Darken", "Lighten", "Color Dodge", "Color Burn", "Linear Burn", "Add (Linear Dodge)",
    "Subtract", "Hard Light", "Soft Light", "Vivid Light", "Linear Light", "Pin Light", "Hard Mix", "Difference", "Exclusion", "Divide",
    "Hue", "Saturation", "Color", "Luminosity", "Darker Color", "Lighter Color", "Glow (Add, soft)",
];

#[inline(always)]
fn dodge(b: f32, s: f32) -> f32 {
    if b <= 0.0 {
        0.0
    } else if s >= 1.0 {
        1.0
    } else {
        (b / (1.0 - s)).min(1.0)
    }
}
#[inline(always)]
fn burn(b: f32, s: f32) -> f32 {
    if b >= 1.0 {
        1.0
    } else if s <= 0.0 {
        0.0
    } else {
        1.0 - ((1.0 - b) / s).min(1.0)
    }
}
#[inline(always)]
fn hard(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        2.0 * b * s
    } else {
        let s2 = 2.0 * s - 1.0;
        b + s2 - b * s2
    }
}
#[inline(always)]
fn soft(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        b - (1.0 - 2.0 * s) * b * (1.0 - b)
    } else {
        let d = if b <= 0.25 { ((16.0 * b - 12.0) * b + 4.0) * b } else { b.sqrt() };
        b + (2.0 * s - 1.0) * (d - b)
    }
}
#[inline(always)]
fn vivid(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        burn(b, 2.0 * s)
    } else {
        dodge(b, 2.0 * s - 1.0)
    }
}

#[inline(always)]
fn sep(mode: u8, b: f32, s: f32) -> f32 {
    match mode {
        1 => b * s,
        2 => b + s - b * s,
        3 => hard(s, b),
        4 => b.min(s),
        5 => b.max(s),
        6 => dodge(b, s),
        7 => burn(b, s),
        8 => (b + s - 1.0).max(0.0),
        9 | 26 => (b + s).min(1.0),
        10 => (b - s).max(0.0),
        11 => hard(b, s),
        12 => soft(b, s),
        13 => vivid(b, s),
        14 => (b + 2.0 * s - 1.0).clamp(0.0, 1.0),
        15 => {
            if s <= 0.5 {
                b.min(2.0 * s)
            } else {
                b.max(2.0 * s - 1.0)
            }
        }
        16 => {
            if vivid(b, s) < 0.5 {
                0.0
            } else {
                1.0
            }
        }
        17 => (b - s).abs(),
        18 => b + s - 2.0 * b * s,
        19 => {
            if s <= 0.0 {
                1.0
            } else {
                (b / s).min(1.0)
            }
        }
        _ => s,
    }
}

#[inline(always)]
fn lumv(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}
fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lumv(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut o = c;
    if n < 0.0 {
        for v in o.iter_mut() {
            *v = l + (*v - l) * l / (l - n).max(1e-6);
        }
    }
    if x > 1.0 {
        for v in o.iter_mut() {
            *v = l + (*v - l) * (1.0 - l) / (x - l).max(1e-6);
        }
    }
    o
}
fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lumv(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}
fn satv(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}
fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    if mx - mn <= 1e-6 {
        return [0.0; 3];
    }
    let k = s / (mx - mn);
    [(c[0] - mn) * k, (c[1] - mn) * k, (c[2] - mn) * k]
}

/// blend premultiplied `src` onto premultiplied `dst` with mode index `mode`
#[inline]
pub fn blend(dst: [f32; 4], src: [f32; 4], mode: u8) -> [f32; 4] {
    let sa = src[3];
    if sa <= 0.0 {
        return dst;
    }
    let da = dst[3];
    if mode == 0 || da <= 0.0 {
        let k = 1.0 - sa;
        return [src[0] + dst[0] * k, src[1] + dst[1] * k, src[2] + dst[2] * k, sa + da * k];
    }
    let s = [src[0] / sa, src[1] / sa, src[2] / sa];
    let b = [dst[0] / da, dst[1] / da, dst[2] / da];
    let m = if mode >= 20 && mode <= 25 {
        match mode {
            20 => set_lum(set_sat(s, satv(b)), lumv(b)),
            21 => set_lum(set_sat(b, satv(s)), lumv(b)),
            22 => set_lum(s, lumv(b)),
            23 => set_lum(b, lumv(s)),
            24 => {
                if lumv(s) < lumv(b) {
                    s
                } else {
                    b
                }
            }
            _ => {
                if lumv(s) > lumv(b) {
                    s
                } else {
                    b
                }
            }
        }
    } else {
        [sep(mode, b[0], s[0]), sep(mode, b[1], s[1]), sep(mode, b[2], s[2])]
    };
    let ra = sa + da * (1.0 - sa);
    let mut out = [0.0; 4];
    for k in 0..3 {
        out[k] = src[k] * (1.0 - da) + dst[k] * (1.0 - sa) + sa * da * m[k];
    }
    out[3] = ra;
    if mode == 26 {
        // glow: light added to what is below without darkening the edges (alpha does not stack)
        for k in 0..3 {
            out[k] = (dst[k] + src[k]).min(ra);
        }
    }
    out
}
