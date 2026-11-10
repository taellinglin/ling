//! Filters that run on one layer inside the selection: halftone (mono / CMYK, seven dot shapes), dithering
//! (ordered Bayer 2/4/8, Floyd-Steinberg, Atkinson, blue noise, white noise, clustered dot, at any pixel scale),
//! screentone spot functions (shared with tone layers), posterize, threshold, blur, glow, sharpen, outline,
//! lineart extraction, gradient map and colour adjustments.

use crate::gradient::Gradient;
#[cfg(not(feature = "std"))]
use crate::os::sync::OnceLock;
#[allow(unused_imports)]
use crate::prelude::*;
use crate::px::*;
#[cfg(feature = "std")]
use std::sync::OnceLock;

pub const SPOT_NAMES: [&str; 7] = ["Dot (euclidean)", "Round", "Line", "Cross", "Square", "Diamond", "Ellipse (chain)"];
pub const DITHER_NAMES: [&str; 8] = ["Bayer 2x2", "Bayer 4x4", "Bayer 8x8", "Floyd-Steinberg", "Atkinson", "Blue noise", "White noise", "Clustered dot"];

/// (id, menu label, [(param label, min, max, default)])
pub type FilterDef = (&'static str, &'static str, &'static [(&'static str, f32, f32, f32)]);
pub const FILTERS: &[FilterDef] = &[
    ("halftone", "Halftone...", &[("Cell size (px)", 2.0, 200.0, 8.0), ("Angle", -90.0, 90.0, 45.0), ("Shape", 0.0, 6.0, 0.0), ("Mode: ink / CMYK / ink on white", 0.0, 2.0, 0.0), ("Contrast", 0.2, 3.0, 1.0)]),
    ("dither", "Dither...", &[("Algorithm", 0.0, 7.0, 3.0), ("Levels", 2.0, 16.0, 2.0), ("Palette: levels / grey / FG-BG / swatches", 0.0, 3.0, 0.0), ("Strength", 0.0, 1.0, 1.0), ("1-bit alpha", 0.0, 1.0, 0.0), ("Pixel size", 1.0, 16.0, 1.0)]),
    ("posterize", "Posterize...", &[("Levels", 2.0, 32.0, 5.0)]),
    ("threshold", "Threshold...", &[("Level", 0.0, 1.0, 0.5), ("Use FG / BG", 0.0, 1.0, 1.0)]),
    ("blur", "Gaussian Blur...", &[("Radius", 0.5, 120.0, 4.0)]),
    ("glow", "Anime Glow (bloom)...", &[("Radius", 1.0, 120.0, 14.0), ("Strength", 0.0, 3.0, 1.0), ("Threshold", 0.0, 1.0, 0.55)]),
    ("sharpen", "Sharpen...", &[("Amount", 0.0, 3.0, 1.0), ("Radius", 0.5, 10.0, 1.5)]),
    ("outline", "Outline / Sticker Border...", &[("Width (px)", 1.0, 80.0, 4.0), ("Softness", 0.0, 1.0, 0.2)]),
    ("lineart", "Extract Lineart (white to alpha)...", &[("White point", 0.5, 1.0, 0.95), ("Black point", 0.0, 0.5, 0.08), ("Recolour with FG", 0.0, 1.0, 0.0)]),
    ("gradmap", "Gradient Map...", &[("Opacity", 0.0, 1.0, 1.0)]),
    ("hsl", "Hue / Saturation / Lightness...", &[("Hue", -180.0, 180.0, 0.0), ("Saturation", -1.0, 1.0, 0.0), ("Lightness", -1.0, 1.0, 0.0)]),
    ("bc", "Brightness / Contrast...", &[("Brightness", -1.0, 1.0, 0.0), ("Contrast", -1.0, 1.0, 0.0)]),
    ("noise", "Add Noise...", &[("Amount", 0.0, 1.0, 0.1), ("Monochrome", 0.0, 1.0, 1.0)]),
    ("invert", "Invert", &[]),
    ("desaturate", "Desaturate", &[]),
];

// ── spot functions (halftone and screentone) ─────────────────────────────────
#[inline(always)]
fn spot_raw(shape: u8, fx: f32, fy: f32) -> f32 {
    use core::f32::consts::TAU;
    match shape {
        1 => fx * fx + fy * fy,
        2 => fy.abs(),
        3 => fx.abs().min(fy.abs()),
        4 => fx.abs().max(fy.abs()),
        5 => fx.abs() + fy.abs(),
        6 => 0.5 - (0.3 * (TAU * fx).cos() + 0.2 * (TAU * fy).cos()),
        _ => 0.5 - 0.25 * ((TAU * fx).cos() + (TAU * fy).cos()),
    }
}
const LUTN: usize = 2048;
struct SpotLut {
    lo: f32,
    hi: f32,
    cdf: Vec<f32>,
}
fn luts() -> &'static Vec<SpotLut> {
    static L: OnceLock<Vec<SpotLut>> = OnceLock::new();
    L.get_or_init(|| {
        (0..7u8)
            .map(|s| {
                let n = 192;
                let mut v = Vec::with_capacity(n * n);
                for j in 0..n {
                    for i in 0..n {
                        let fx = (i as f32 + 0.5) / n as f32 - 0.5;
                        let fy = (j as f32 + 0.5) / n as f32 - 0.5;
                        v.push(spot_raw(s, fx, fy));
                    }
                }
                let lo = v.iter().cloned().fold(f32::MAX, f32::min);
                let hi = v.iter().cloned().fold(f32::MIN, f32::max) + 1e-6;
                let mut hist = vec![0u32; LUTN];
                for x in &v {
                    let b = (((x - lo) / (hi - lo)) * LUTN as f32) as usize;
                    hist[b.min(LUTN - 1)] += 1;
                }
                let mut cdf = vec![0.0f32; LUTN];
                let mut acc = 0u32;
                for b in 0..LUTN {
                    acc += hist[b];
                    // centre of the bin's mass
                    cdf[b] = (acc as f32 - hist[b] as f32 * 0.5) / v.len() as f32;
                }
                SpotLut { lo, hi, cdf }
            })
            .collect()
    })
}
/// area-ordered threshold 0..1 of a point inside a halftone cell (fx, fy in -0.5..0.5): a cell inks every point
/// whose threshold is below the ink amount, so the inked area equals the amount
#[inline(always)]
pub fn spot_t(shape: u8, fx: f32, fy: f32) -> f32 {
    let l = &luts()[shape.min(6) as usize];
    let r = spot_raw(shape, fx, fy);
    let b = (((r - l.lo) / (l.hi - l.lo)) * LUTN as f32) as isize;
    l.cdf[b.clamp(0, LUTN as isize - 1) as usize]
}
/// screentone coverage at canvas pixel (x, y) for ink amount k (0..1), cell size in px, angle in radians
#[inline(always)]
pub fn tone_cov(shape: u8, cell: f32, sa: f32, ca: f32, x: f32, y: f32, k: f32) -> f32 {
    if k <= 0.002 {
        return 0.0;
    }
    if k >= 0.998 {
        return 1.0;
    }
    let u = (x * ca + y * sa) / cell;
    let v = (-x * sa + y * ca) / cell;
    let fx = u - u.floor() - 0.5;
    let fy = v - v.floor() - 0.5;
    let t = spot_t(shape, fx, fy);
    // analytic anti-aliasing: the threshold changes by about 2 / cell per pixel
    let w = (2.2 / cell).min(0.5);
    ((k - t) / w + 0.5).clamp(0.0, 1.0)
}

// ── regions ──────────────────────────────────────────────────────────────────
pub struct Region {
    pub x0: i32,
    pub y0: i32,
    pub w: usize,
    pub h: usize,
    /// premultiplied
    pub px: Vec<[f32; 4]>,
}
impl Region {
    pub fn read(t: &Tiles, r: [i32; 4]) -> Region {
        let w = (r[2] - r[0]).max(0) as usize;
        let h = (r[3] - r[1]).max(0) as usize;
        let mut px = vec![[0.0f32; 4]; w * h];
        for y in 0..h {
            for x in 0..w {
                px[y * w + x] = t.getf(r[0] + x as i32, r[1] + y as i32);
            }
        }
        Region { x0: r[0], y0: r[1], w, h, px }
    }
    /// write back, blended by the selection (soft edges stay soft)
    pub fn write(&self, t: &mut Tiles, sel: Option<&Mask>, clip: [i32; 4]) {
        let (x0, y0, w) = (self.x0, self.y0, self.w);
        let px = &self.px;
        t.for_rect(clip[0].max(x0), clip[1].max(y0), clip[2].min(x0 + w as i32), clip[3].min(y0 + self.h as i32), |x, y, p| {
            let n = px[(y - y0) as usize * w + (x - x0) as usize];
            let k = match sel {
                Some(s) => s.at(x, y),
                None => 1.0,
            };
            if k <= 0.0 {
                return;
            }
            *p = fromf(if k >= 1.0 { n } else { lerp4(tof(*p), n, k) });
        });
        t.prune();
    }
}

/// box blur of an f32 channel image, radius r (separable running sums)
fn box1(v: &mut [f32], w: usize, h: usize, r: usize, stride_ch: usize, ch: usize) {
    if r == 0 {
        return;
    }
    let mut tmp = vec![0.0f32; w.max(h)];
    let norm = 1.0 / (2 * r + 1) as f32;
    for y in 0..h {
        let mut acc = 0.0;
        let get = |v: &[f32], x: isize| v[(y * w + x.clamp(0, w as isize - 1) as usize) * stride_ch + ch];
        for x in -(r as isize)..=(r as isize) {
            acc += get(v, x);
        }
        for x in 0..w {
            tmp[x] = acc * norm;
            acc += get(v, x as isize + r as isize + 1) - get(v, x as isize - r as isize);
        }
        for x in 0..w {
            v[(y * w + x) * stride_ch + ch] = tmp[x];
        }
    }
    for x in 0..w {
        let mut acc = 0.0;
        let get = |v: &[f32], y: isize| v[(y.clamp(0, h as isize - 1) as usize * w + x) * stride_ch + ch];
        for y in -(r as isize)..=(r as isize) {
            acc += get(v, y);
        }
        for y in 0..h {
            tmp[y] = acc * norm;
            acc += get(v, y as isize + r as isize + 1) - get(v, y as isize - r as isize);
        }
        for y in 0..h {
            v[(y * w + x) * stride_ch + ch] = tmp[y];
        }
    }
}
/// near-gaussian blur (three box passes) of a premultiplied RGBA image
pub fn gauss(px: &mut [[f32; 4]], w: usize, h: usize, radius: f32) {
    if radius < 0.3 || w == 0 || h == 0 {
        return;
    }
    // three boxes approximating sigma = radius / 2
    let sigma = radius * 0.5;
    let wi = ((12.0 * sigma * sigma / 3.0 + 1.0).sqrt()).floor().max(1.0);
    let r = ((wi - 1.0) / 2.0).max(0.0) as usize;
    let r = r.max(1);
    let flat: &mut [f32] = unsafe { core::slice::from_raw_parts_mut(px.as_mut_ptr() as *mut f32, px.len() * 4) };
    for _ in 0..3 {
        for ch in 0..4 {
            box1(flat, w, h, r, 4, ch);
        }
    }
}

/// exact euclidean distance transform (Felzenszwalb) of a boolean "inside" map: distance to the nearest inside pixel
pub fn edt(inside: &[bool], w: usize, h: usize) -> Vec<f32> {
    let inf = 1e20f32;
    let mut g: Vec<f32> = inside.iter().map(|&b| if b { 0.0 } else { inf }).collect();
    let n = w.max(h);
    let mut f = vec![0.0f32; n];
    let mut d = vec![0.0f32; n];
    let mut v = vec![0usize; n];
    let mut z = vec![0.0f32; n + 1];
    let pass = |f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32], len: usize| {
        let mut k = 0usize;
        v[0] = 0;
        z[0] = -inf;
        z[1] = inf;
        let isect = |q: usize, p: usize| ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * q as f32 - 2.0 * p as f32);
        for q in 1..len {
            let mut s = isect(q, v[k]);
            while s <= z[k] && k > 0 {
                k -= 1;
                s = isect(q, v[k]);
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = inf;
        }
        k = 0;
        for q in 0..len {
            while z[k + 1] < q as f32 {
                k += 1;
            }
            let p = v[k];
            d[q] = (q as f32 - p as f32).powi(2) + f[p];
        }
    };
    for x in 0..w {
        for y in 0..h {
            f[y] = g[y * w + x];
        }
        pass(&f, &mut d, &mut v, &mut z, h);
        for y in 0..h {
            g[y * w + x] = d[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&g[y * w..(y + 1) * w]);
        pass(&f, &mut d, &mut v, &mut z, w);
        g[y * w..(y + 1) * w].copy_from_slice(&d[..w]);
    }
    g.iter().map(|&x| x.sqrt()).collect()
}

const BAYER8: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];
const CLUSTER4: [[u8; 4]; 4] = [[12, 5, 6, 13], [4, 0, 1, 7], [11, 3, 2, 8], [15, 10, 9, 14]];

fn ordered(alg: u8, x: i64, y: i64) -> f32 {
    match alg {
        0 => {
            let m = [[0.0, 2.0], [3.0, 1.0]];
            (m[(y & 1) as usize][(x & 1) as usize] + 0.5) / 4.0
        }
        1 => (BAYER8[(y & 3) as usize * 2 % 8][(x & 3) as usize * 2 % 8] as f32 / 4.0 + 0.5) / 16.0,
        2 => (BAYER8[(y & 7) as usize][(x & 7) as usize] as f32 + 0.5) / 64.0,
        5 => {
            let v = 52.982_918 * (0.067_110_56 * x as f32 + 0.005_837_15 * y as f32).fract();
            v.fract()
        }
        6 => {
            let mut h = (x as u64).wrapping_mul(0x9E3779B97F4A7C15) ^ (y as u64).wrapping_mul(0xC2B2AE3D27D4EB4F);
            h ^= h >> 29;
            h = h.wrapping_mul(0xBF58476D1CE4E5B9);
            ((h >> 40) as f32) / 16777216.0
        }
        _ => (CLUSTER4[(y & 3) as usize][(x & 3) as usize] as f32 + 0.5) / 16.0,
    }
}

pub struct FilterEnv<'a> {
    pub fg: [f32; 3],
    pub bg: [f32; 3],
    pub grad: &'a Gradient,
    pub palette: &'a [[f32; 3]],
}

fn quantize(c: [f32; 3], pal: u8, levels: f32, env: &FilterEnv) -> [f32; 3] {
    match pal {
        1 => {
            let l = lum(c[0], c[1], c[2]);
            let q = (l * (levels - 1.0)).round() / (levels - 1.0);
            [q, q, q]
        }
        2 => {
            if lum(c[0], c[1], c[2]) < 0.5 {
                env.fg
            } else {
                env.bg
            }
        }
        3 if !env.palette.is_empty() => {
            let mut best = env.palette[0];
            let mut bd = f32::MAX;
            for p in env.palette {
                // weighted rgb distance
                let d = 0.3 * (p[0] - c[0]).powi(2) + 0.59 * (p[1] - c[1]).powi(2) + 0.11 * (p[2] - c[2]).powi(2);
                if d < bd {
                    bd = d;
                    best = *p;
                }
            }
            best
        }
        _ => {
            let q = |v: f32| (v.clamp(0.0, 1.0) * (levels - 1.0)).round() / (levels - 1.0);
            [q(c[0]), q(c[1]), q(c[2])]
        }
    }
}

/// run filter `name` with `p` on the layer pixels of `t`, inside the rectangle `rect` and the selection
pub fn run(name: &str, p: &[f32], t: &mut Tiles, sel: Option<&Mask>, rect: [i32; 4], env: &FilterEnv) {
    let pa = |i: usize, d: f32| p.get(i).copied().unwrap_or(d);
    let canvas = [0, 0, t.w as i32, t.h as i32];
    let clipr = |r: [i32; 4]| [r[0].max(0), r[1].max(0), r[2].min(t.w as i32), r[3].min(t.h as i32)];
    match name {
        "invert" | "desaturate" | "posterize" | "threshold" | "hsl" | "bc" | "gradmap" | "lineart" | "noise" => {
            let mut reg = Region::read(t, rect);
            for (i, c) in reg.px.iter_mut().enumerate() {
                if c[3] <= 0.0 && name != "noise" {
                    continue;
                }
                let u = unpre(*c);
                let (x, y) = ((i % reg.w) as i64 + reg.x0 as i64, (i / reg.w) as i64 + reg.y0 as i64);
                let mut rgb = [u[0], u[1], u[2]];
                let mut a = u[3];
                match name {
                    "invert" => rgb = [1.0 - rgb[0], 1.0 - rgb[1], 1.0 - rgb[2]],
                    "desaturate" => {
                        let l = lum(rgb[0], rgb[1], rgb[2]);
                        rgb = [l, l, l];
                    }
                    "posterize" => {
                        let lv = pa(0, 5.0).max(2.0);
                        for v in rgb.iter_mut() {
                            *v = (*v * (lv - 1.0)).round() / (lv - 1.0);
                        }
                    }
                    "threshold" => {
                        let on = lum(rgb[0], rgb[1], rgb[2]) >= pa(0, 0.5);
                        rgb = if pa(1, 1.0) > 0.5 {
                            if on {
                                env.bg
                            } else {
                                env.fg
                            }
                        } else if on {
                            [1.0; 3]
                        } else {
                            [0.0; 3]
                        };
                    }
                    "hsl" => {
                        let mut hsv = rgb2hsv(rgb);
                        hsv[0] += pa(0, 0.0) / 360.0;
                        hsv[1] = (hsv[1] * (1.0 + pa(1, 0.0))).clamp(0.0, 1.0);
                        rgb = hsv2rgb(hsv);
                        let l = pa(2, 0.0);
                        for v in rgb.iter_mut() {
                            *v = if l > 0.0 { *v + (1.0 - *v) * l } else { *v * (1.0 + l) };
                        }
                    }
                    "bc" => {
                        let b = pa(0, 0.0);
                        let c2 = pa(1, 0.0);
                        let f = if c2 > 0.0 { 1.0 / (1.0 - c2 * 0.99) } else { 1.0 + c2 };
                        for v in rgb.iter_mut() {
                            *v = ((*v - 0.5) * f + 0.5 + b).clamp(0.0, 1.0);
                        }
                    }
                    "gradmap" => {
                        let g = env.grad.eval(lum(rgb[0], rgb[1], rgb[2]));
                        let k = pa(0, 1.0) * g[3];
                        for j in 0..3 {
                            rgb[j] += (g[j] - rgb[j]) * k;
                        }
                    }
                    "lineart" => {
                        let l = lum(rgb[0], rgb[1], rgb[2]);
                        let ink = 1.0 - smoothstep(pa(1, 0.08), pa(0, 0.95), l);
                        let na = a * ink;
                        if pa(2, 0.0) > 0.5 {
                            rgb = env.fg;
                        } else if ink > 1e-3 {
                            for v in rgb.iter_mut() {
                                *v = ((*v - (1.0 - ink)) / ink).clamp(0.0, 1.0);
                            }
                        }
                        a = na;
                    }
                    "noise" => {
                        if a <= 0.0 {
                            continue;
                        }
                        let amt = pa(0, 0.1);
                        let n0 = ordered(6, x, y) - 0.5;
                        if pa(1, 1.0) > 0.5 {
                            for v in rgb.iter_mut() {
                                *v = (*v + n0 * amt).clamp(0.0, 1.0);
                            }
                        } else {
                            rgb[0] = (rgb[0] + n0 * amt).clamp(0.0, 1.0);
                            rgb[1] = (rgb[1] + (ordered(6, x + 7919, y) - 0.5) * amt).clamp(0.0, 1.0);
                            rgb[2] = (rgb[2] + (ordered(6, x, y + 104729) - 0.5) * amt).clamp(0.0, 1.0);
                        }
                    }
                    _ => {}
                }
                *c = pre([rgb[0], rgb[1], rgb[2], a]);
            }
            reg.write(t, sel, canvas);
        }
        "blur" | "glow" | "sharpen" => {
            let rad = if name == "sharpen" { pa(1, 1.5) } else { pa(0, 4.0) };
            let pad = (rad * 2.0).ceil() as i32 + 2;
            let big = clipr([rect[0] - pad, rect[1] - pad, rect[2] + pad, rect[3] + pad]);
            let mut reg = Region::read(t, big);
            let orig = reg.px.clone();
            if name == "glow" {
                let th = pa(2, 0.55);
                for c in reg.px.iter_mut() {
                    let u = unpre(*c);
                    let l = lum(u[0], u[1], u[2]);
                    let k = smoothstep(th, (th + 0.25).min(1.0), l) * u[3];
                    *c = [u[0] * k, u[1] * k, u[2] * k, k];
                }
            }
            gauss(&mut reg.px, reg.w, reg.h, rad);
            match name {
                "glow" => {
                    let s = pa(1, 1.0);
                    for (i, c) in reg.px.iter_mut().enumerate() {
                        let o = orig[i];
                        // screen the bloom onto the picture
                        let mut out = o;
                        for k in 0..3 {
                            let g = (c[k] * s).min(1.0);
                            out[k] = o[k] + g - o[k] * g;
                        }
                        out[3] = (o[3] + c[3] * s - o[3] * c[3] * s).clamp(0.0, 1.0);
                        for k in 0..3 {
                            out[k] = out[k].min(out[3]);
                        }
                        *c = out;
                    }
                }
                "sharpen" => {
                    let amt = pa(0, 1.0);
                    for (i, c) in reg.px.iter_mut().enumerate() {
                        let o = orig[i];
                        let mut out = o;
                        for k in 0..3 {
                            out[k] = (o[k] + (o[k] - c[k]) * amt).clamp(0.0, o[3]);
                        }
                        *c = out;
                    }
                }
                _ => {}
            }
            reg.write(t, sel, rect);
        }
        "outline" => {
            let wpx = pa(0, 4.0);
            let soft = pa(1, 0.2);
            let pad = wpx.ceil() as i32 + 3;
            let big = clipr([rect[0] - pad, rect[1] - pad, rect[2] + pad, rect[3] + pad]);
            let mut reg = Region::read(t, big);
            let inside: Vec<bool> = reg.px.iter().map(|c| c[3] >= 0.5).collect();
            let d = edt(&inside, reg.w, reg.h);
            let col = env.fg;
            for (i, c) in reg.px.iter_mut().enumerate() {
                let e = (soft * wpx).max(0.7);
                let oa = ((wpx - d[i]) / e + 0.5).clamp(0.0, 1.0);
                let ring = [col[0] * oa, col[1] * oa, col[2] * oa, oa];
                *c = over(ring, *c);
            }
            reg.write(t, sel, big);
        }
        "halftone" => {
            let cell = pa(0, 8.0).max(2.0);
            let ang = pa(1, 45.0).to_radians();
            let shape = pa(2, 0.0) as u8;
            let mode = pa(3, 0.0) as u8;
            let con = pa(4, 1.0);
            let mut reg = Region::read(t, rect);
            let (w, h) = (reg.w, reg.h);
            if w == 0 || h == 0 {
                return;
            }
            // ink amounts per channel (mono: one; CMYK: four), over white paper
            let nch = if mode == 1 { 4 } else { 1 };
            let mut ink = vec![0.0f32; w * h * nch];
            for (i, c) in reg.px.iter().enumerate() {
                let r = 1.0 - c[3] + c[0];
                let g = 1.0 - c[3] + c[1];
                let b = 1.0 - c[3] + c[2];
                let adj = |v: f32| ((v - 0.5) * con + 0.5).clamp(0.0, 1.0);
                if nch == 1 {
                    ink[i] = adj(1.0 - lum(r, g, b));
                } else {
                    let k = 1.0 - r.max(g).max(b);
                    let d = (1.0 - k).max(1e-4);
                    ink[i * 4] = adj((1.0 - r - k) / d);
                    ink[i * 4 + 1] = adj((1.0 - g - k) / d);
                    ink[i * 4 + 2] = adj((1.0 - b - k) / d);
                    ink[i * 4 + 3] = adj(k);
                }
            }
            let br = (cell * 0.5) as usize;
            for ch in 0..nch {
                box1(&mut ink, w, h, br, nch, ch);
                box1(&mut ink, w, h, br / 2 + 1, nch, ch);
            }
            let angles = [ang + 15f32.to_radians(), ang + 75f32.to_radians(), ang, ang + 45f32.to_radians()];
            let sub = [(0.25f32, 0.25f32), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];
            for y in 0..h {
                for x in 0..w {
                    let gx = (reg.x0 + x as i32) as f32;
                    let gy = (reg.y0 + y as i32) as f32;
                    let mut covs = [0.0f32; 4];
                    for ch in 0..nch {
                        let a = if nch == 1 { ang } else { angles[ch] };
                        let (sa, ca) = a.sin_cos();
                        let mut c = 0.0;
                        for (ox, oy) in sub {
                            let px = gx + ox;
                            let py = gy + oy;
                            let u = (px * ca + py * sa) / cell;
                            let v = (-px * sa + py * ca) / cell;
                            // ink sampled at the cell centre so every dot in a flat area is the same size
                            let cu = u.floor() + 0.5;
                            let cv = v.floor() + 0.5;
                            let cx = (cu * ca - cv * sa) * cell - reg.x0 as f32;
                            let cy = (cu * sa + cv * ca) * cell - reg.y0 as f32;
                            let ix = (cx as isize).clamp(0, w as isize - 1) as usize;
                            let iy = (cy as isize).clamp(0, h as isize - 1) as usize;
                            let k = ink[(iy * w + ix) * nch + ch];
                            if spot_t(shape, u - cu, v - cv) < k {
                                c += 0.25;
                            }
                        }
                        covs[ch] = c;
                    }
                    let i = y * w + x;
                    reg.px[i] = match mode {
                        1 => {
                            let k = covs[3];
                            pre([(1.0 - covs[0]) * (1.0 - k), (1.0 - covs[1]) * (1.0 - k), (1.0 - covs[2]) * (1.0 - k), 1.0])
                        }
                        2 => {
                            let c = covs[0];
                            [1.0 + (env.fg[0] - 1.0) * c, 1.0 + (env.fg[1] - 1.0) * c, 1.0 + (env.fg[2] - 1.0) * c, 1.0]
                        }
                        _ => {
                            let c = covs[0];
                            [env.fg[0] * c, env.fg[1] * c, env.fg[2] * c, c]
                        }
                    };
                }
            }
            reg.write(t, sel, canvas);
        }
        "dither" => {
            let alg = pa(0, 3.0) as u8;
            let levels = pa(1, 2.0).max(2.0);
            let pal = pa(2, 0.0) as u8;
            let strength = pa(3, 1.0);
            let abit = pa(4, 0.0) > 0.5;
            let s = pa(5, 1.0).max(1.0) as usize;
            let mut reg = Region::read(t, rect);
            let (w, h) = (reg.w, reg.h);
            if w == 0 || h == 0 {
                return;
            }
            let gw = w.div_ceil(s);
            let gh = h.div_ceil(s);
            // block-averaged straight colour grid
            let mut g = vec![[0.0f32; 4]; gw * gh];
            for gy in 0..gh {
                for gx in 0..gw {
                    let mut acc = [0.0f32; 4];
                    let mut n = 0.0;
                    for yy in gy * s..((gy + 1) * s).min(h) {
                        for xx in gx * s..((gx + 1) * s).min(w) {
                            let c = reg.px[yy * w + xx];
                            for k in 0..4 {
                                acc[k] += c[k];
                            }
                            n += 1.0;
                        }
                    }
                    for v in acc.iter_mut() {
                        *v /= n;
                    }
                    g[gy * gw + gx] = unpre(acc);
                }
            }
            let spread = if pal >= 2 { 0.5 } else { 1.0 / (levels - 1.0) } * strength;
            let diffuse = alg == 3 || alg == 4;
            let mut err = vec![[0.0f32; 3]; gw * gh];
            let mut out = vec![[0.0f32; 4]; gw * gh];
            for gy in 0..gh {
                for gx in 0..gw {
                    let i = gy * gw + gx;
                    let c = g[i];
                    let wx = (reg.x0 as i64) / s as i64 + gx as i64;
                    let wy = (reg.y0 as i64) / s as i64 + gy as i64;
                    let mut rgb = [c[0], c[1], c[2]];
                    if diffuse {
                        for k in 0..3 {
                            rgb[k] += err[i][k] * strength;
                        }
                    } else {
                        let th = ordered(alg, wx, wy) - 0.5;
                        for v in rgb.iter_mut() {
                            *v += th * spread;
                        }
                    }
                    let q = quantize(rgb, pal, levels, env);
                    if diffuse && c[3] > 0.0 {
                        let e = [rgb[0] - q[0], rgb[1] - q[1], rgb[2] - q[2]];
                        let mut push = |dx: isize, dy: isize, k: f32| {
                            let nx = gx as isize + dx;
                            let ny = gy as isize + dy;
                            if nx >= 0 && ny >= 0 && (nx as usize) < gw && (ny as usize) < gh {
                                let j = ny as usize * gw + nx as usize;
                                for c3 in 0..3 {
                                    err[j][c3] += e[c3] * k;
                                }
                            }
                        };
                        if alg == 3 {
                            push(1, 0, 7.0 / 16.0);
                            push(-1, 1, 3.0 / 16.0);
                            push(0, 1, 5.0 / 16.0);
                            push(1, 1, 1.0 / 16.0);
                        } else {
                            for (dx, dy) in [(1, 0), (2, 0), (-1, 1), (0, 1), (1, 1), (0, 2)] {
                                push(dx, dy, 1.0 / 8.0);
                            }
                        }
                    }
                    let mut a = c[3];
                    if abit {
                        let th = if diffuse { 0.5 } else { ordered(alg, wx, wy) };
                        a = if a > th { 1.0 } else { 0.0 };
                    }
                    out[i] = pre([q[0], q[1], q[2], a]);
                }
            }
            for y in 0..h {
                for x in 0..w {
                    reg.px[y * w + x] = out[(y / s) * gw + x / s];
                }
            }
            reg.write(t, sel, canvas);
        }
        _ => {}
    }
}
