//! Picture splitting: turns one flat picture (a generated layer, a pasted PNG) into editable layers.
//!
//! * **Components**: lineart, flat colours (optionally one layer per colour), shadows (Multiply) and
//!   highlights (Screen). The layers are built so that stacked together they give back the original picture:
//!   shadows / highlights are the per-channel ratio between the picture and its flat colour, and the lineart
//!   colour is "un-blended" against what lies under it.
//! * **Objects**: the picture is cut into a background and foreground objects (from its transparency, from an
//!   AI matte, or from colour regions that do not touch the top / sides). The background gets a *clean plate*:
//!   what was behind the objects is filled in (push-pull inpainting), so objects can move in 2.5D parallax
//!   without leaving holes. Object edges are un-blended from the plate, so object-over-plate gives back the
//!   original picture.
//! * Both: every object and the background are split into components.
//!
//! Everything runs on a worker thread over a copy of the layer, reporting progress in permille.

#[allow(unused_imports)]
use crate::prelude::*;
use crate::px::*;
use core::sync::atomic::{AtomicU32, Ordering};
#[cfg(feature = "std")]
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct SplitOpts {
    pub objects: bool,
    pub components: bool,
    pub per_color: bool,
    pub colors: usize,
    /// 0..1, higher finds fainter lines
    pub line_sens: f32,
    /// thickest line to treat as lineart, px
    pub line_width: f32,
    pub max_objects: usize,
    /// 0 auto, 1 transparency, 2 colour regions, 3 AI matte (rembg on the SD server)
    pub method: u8,
    pub sd_url: String,
}

pub struct OutLayer {
    pub name: String,
    pub blend: u8,
    /// position inside the source region
    pub x0: i32,
    pub y0: i32,
    pub w: usize,
    pub h: usize,
    pub px: Vec<Px>,
    /// 0 = background, 1.. = objects front-most last; -1 = whole picture
    pub group: i32,
}

pub struct PartOut {
    pub name: String,
    pub x0: i32,
    pub y0: i32,
    pub w: usize,
    pub h: usize,
    /// straight RGBA8
    pub rgba: Vec<u8>,
    /// 0 = background (furthest), larger = nearer
    pub depth: usize,
}

pub struct SplitResult {
    pub layers: Vec<OutLayer>,
    pub parts: Vec<PartOut>,
    /// where the background was filled in (region coords, 0..1), for an AI touch-up
    pub hole: Option<Vec<f32>>,
    pub region_w: usize,
    pub region_h: usize,
    pub note: String,
}

const MULTIPLY: u8 = 1;
const SCREEN: u8 = 2;

fn luma(c: [f32; 3]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

fn oklab(c: [f32; 3]) -> [f32; 3] {
    let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    let (r, g, b) = (lin(c[0]), lin(c[1]), lin(c[2]));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}
/// feature space for colour clustering: lightness counts less than hue / chroma, so a lit and a shaded part
/// of the same material land in the same flat
fn feat(c: [f32; 3]) -> [f32; 3] {
    let o = oklab(c);
    [o[0] * 0.45, o[1], o[2]]
}
fn d2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// separable max filter, radius r
fn max_filter(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 {
        return v.to_vec();
    }
    let mut t = vec![0.0f32; w * h];
    t.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            row[x] = v[y * w + a..=y * w + b].iter().cloned().fold(f32::MIN, f32::max);
        }
    });
    let mut o = vec![0.0f32; w * h];
    o.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            let mut m = f32::MIN;
            for yy in a..=b {
                m = m.max(t[yy * w + x]);
            }
            row[x] = m;
        }
    });
    o
}

/// box blur of a single channel, radius r (running sums, clamped edges)
fn box_blur(v: &mut [f32], w: usize, h: usize, r: usize) {
    if r == 0 || w == 0 || h == 0 {
        return;
    }
    let k = 1.0 / (2 * r + 1) as f32;
    v.par_chunks_mut(w).for_each(|row| {
        let src = row.to_vec();
        let g = |x: isize| src[x.clamp(0, w as isize - 1) as usize];
        let mut acc: f32 = (-(r as isize)..=r as isize).map(g).sum();
        for x in 0..w {
            row[x] = acc * k;
            acc += g(x as isize + r as isize + 1) - g(x as isize - r as isize);
        }
    });
    let src = v.to_vec();
    let mut cols = vec![0.0f32; w * h];
    // transpose-free vertical pass, a column at a time in parallel over column chunks
    cols.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        let g = |y: isize| src[y.clamp(0, h as isize - 1) as usize * w + x];
        let mut acc: f32 = (-(r as isize)..=r as isize).map(g).sum();
        for y in 0..h {
            col[y] = acc * k;
            acc += g(y as isize + r as isize + 1) - g(y as isize - r as isize);
        }
    });
    for x in 0..w {
        for y in 0..h {
            v[y * w + x] = cols[x * h + y];
        }
    }
}

/// jump flooding: for every pixel the index of the nearest seed (u32::MAX when there are no seeds)
fn nearest_seed(seed: &[bool], w: usize, h: usize) -> Vec<u32> {
    let mut near: Vec<u32> = seed.iter().enumerate().map(|(i, &s)| if s { i as u32 } else { u32::MAX }).collect();
    if !seed.iter().any(|&s| s) {
        return near;
    }
    let mut step = (w.max(h).next_power_of_two() / 2).max(1);
    let mut steps = vec![];
    while step >= 1 {
        steps.push(step);
        step /= 2;
    }
    steps.push(1);
    for &s in &steps {
        let s = s as isize;
        let prev = near.clone();
        near.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                let mut best = prev[y * w + x];
                let mut bd = if best == u32::MAX { f32::MAX } else { dist(best, x, y, w) };
                for dy in [-s, 0, s] {
                    let yy = y as isize + dy;
                    if yy < 0 || yy >= h as isize {
                        continue;
                    }
                    for dx in [-s, 0, s] {
                        let xx = x as isize + dx;
                        if xx < 0 || xx >= w as isize {
                            continue;
                        }
                        let c = prev[yy as usize * w + xx as usize];
                        if c != u32::MAX {
                            let d = dist(c, x, y, w);
                            if d < bd {
                                bd = d;
                                best = c;
                            }
                        }
                    }
                }
                row[x] = best;
            }
        });
    }
    near
}
#[inline]
fn dist(i: u32, x: usize, y: usize, w: usize) -> f32 {
    let (sx, sy) = ((i as usize % w) as f32, (i as usize / w) as f32);
    (sx - x as f32).powi(2) + (sy - y as f32).powi(2)
}

/// push-pull inpainting of premultiplied RGBA: pixels with weight 1 stay, weight 0 get a smooth fill from around
fn push_pull(px: &mut [[f32; 4]], wt: &[f32], w: usize, h: usize) {
    // pyramid of (average colour, coverage)
    let mut levels: Vec<(usize, usize, Vec<[f32; 4]>, Vec<f32>)> = vec![(w, h, px.to_vec(), wt.to_vec())];
    loop {
        let (lw, lh, c, k) = levels.last().unwrap();
        if *lw <= 1 && *lh <= 1 {
            break;
        }
        let (nw, nh) = ((lw + 1) / 2, (lh + 1) / 2);
        let mut nc = vec![[0.0f32; 4]; nw * nh];
        let mut nk = vec![0.0f32; nw * nh];
        for y in 0..nh {
            for x in 0..nw {
                let mut acc = [0.0f32; 4];
                let mut ks = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = (x * 2 + dx, y * 2 + dy);
                    if sx < *lw && sy < *lh {
                        let i = sy * lw + sx;
                        for ch in 0..4 {
                            acc[ch] += c[i][ch] * k[i];
                        }
                        ks += k[i];
                    }
                }
                if ks > 0.0 {
                    for ch in 0..4 {
                        acc[ch] /= ks;
                    }
                }
                nc[y * nw + x] = acc;
                nk[y * nw + x] = ks.min(1.0);
            }
        }
        levels.push((nw, nh, nc, nk));
    }
    // pull: fill each level's gaps from the (bilinearly sampled) coarser level
    for l in (0..levels.len() - 1).rev() {
        let (cw, ch_, cc, _) = {
            let c = &levels[l + 1];
            (c.0, c.1, c.2.clone(), ())
        };
        let (lw, lh, c, k) = &mut levels[l];
        let sample = |fx: f32, fy: f32| -> [f32; 4] {
            let fx = fx.clamp(0.0, (cw - 1) as f32);
            let fy = fy.clamp(0.0, (ch_ - 1) as f32);
            let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(cw - 1), (y0 + 1).min(ch_ - 1));
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let mut o = [0.0f32; 4];
            for ch in 0..4 {
                let a = cc[y0 * cw + x0][ch] * (1.0 - tx) + cc[y0 * cw + x1][ch] * tx;
                let b = cc[y1 * cw + x0][ch] * (1.0 - tx) + cc[y1 * cw + x1][ch] * tx;
                o[ch] = a * (1.0 - ty) + b * ty;
            }
            o
        };
        let lw = *lw;
        c.par_chunks_mut(lw).zip(k.par_chunks_mut(lw)).enumerate().for_each(|(y, (crow, krow))| {
            for x in 0..lw {
                let kk = krow[x];
                if kk >= 1.0 {
                    continue;
                }
                let up = sample((x as f32 - 0.5) * 0.5, (y as f32 - 0.5) * 0.5);
                for ch in 0..4 {
                    crow[x][ch] = crow[x][ch] * kk + up[ch] * (1.0 - kk);
                }
                krow[x] = 1.0;
            }
        });
        let _ = lh;
    }
    let base = &levels[0].2;
    px.par_iter_mut().zip(wt.par_iter()).zip(base.par_iter()).for_each(|((p, &k), b)| {
        if k < 1.0 {
            for ch in 0..4 {
                p[ch] = p[ch] * k + b[ch] * (1.0 - k);
            }
        }
    });
}

/// vertical box blur (two passes) of premultiplied RGBA
fn gauss_rows(px: &mut [[f32; 4]], w: usize, h: usize, r: usize) {
    for ch in 0..4 {
        let mut v: Vec<f32> = px.iter().map(|p| p[ch]).collect();
        for _ in 0..2 {
            let src = v.clone();
            for x in 0..w {
                let g = |y: isize| src[y.clamp(0, h as isize - 1) as usize * w + x];
                let mut acc: f32 = (-(r as isize)..=r as isize).map(g).sum();
                for y in 0..h {
                    v[y * w + x] = acc / (2 * r + 1) as f32;
                    acc += g(y as isize + r as isize + 1) - g(y as isize - r as isize);
                }
            }
        }
        for (p, val) in px.iter_mut().zip(v) {
            p[ch] = val;
        }
    }
}

/// connected components (8-neighbour) of a boolean map: labels (-1 outside) and the pixel count of each
fn components(m: &[bool], w: usize, h: usize) -> (Vec<i32>, Vec<usize>) {
    let mut lab = vec![-1i32; w * h];
    let mut sizes = vec![];
    let mut stack = vec![];
    for s in 0..w * h {
        if !m[s] || lab[s] >= 0 {
            continue;
        }
        let id = sizes.len() as i32;
        let mut n = 0usize;
        lab[s] = id;
        stack.push(s);
        while let Some(i) = stack.pop() {
            n += 1;
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    let (xx, yy) = (x + dx, y + dy);
                    if xx < 0 || yy < 0 || xx >= w as isize || yy >= h as isize {
                        continue;
                    }
                    let j = yy as usize * w + xx as usize;
                    if m[j] && lab[j] < 0 {
                        lab[j] = id;
                        stack.push(j);
                    }
                }
            }
        }
        sizes.push(n);
    }
    (lab, sizes)
}

fn kmeans(samples: &[[f32; 3]], k: usize, rng: &mut Rng) -> Vec<[f32; 3]> {
    if samples.is_empty() {
        return vec![];
    }
    let k = k.min(samples.len()).max(1);
    // k-means++ seeding
    let mut cent = vec![samples[(rng.next() * samples.len() as f32) as usize % samples.len()]];
    let mut dmin: Vec<f32> = samples.iter().map(|s| d2(*s, cent[0])).collect();
    while cent.len() < k {
        let tot: f32 = dmin.iter().sum();
        if tot <= 1e-12 {
            break;
        }
        let mut r = rng.next() * tot;
        let mut pick = samples.len() - 1;
        for (i, d) in dmin.iter().enumerate() {
            r -= d;
            if r <= 0.0 {
                pick = i;
                break;
            }
        }
        let c = samples[pick];
        cent.push(c);
        for (i, s) in samples.iter().enumerate() {
            dmin[i] = dmin[i].min(d2(*s, c));
        }
    }
    for _ in 0..14 {
        let assign: Vec<usize> = samples.par_iter().map(|s| nearest(&cent, *s)).collect();
        let mut sum = vec![[0.0f64; 3]; cent.len()];
        let mut cnt = vec![0usize; cent.len()];
        for (s, &a) in samples.iter().zip(&assign) {
            for ch in 0..3 {
                sum[a][ch] += s[ch] as f64;
            }
            cnt[a] += 1;
        }
        for i in 0..cent.len() {
            if cnt[i] > 0 {
                cent[i] = [(sum[i][0] / cnt[i] as f64) as f32, (sum[i][1] / cnt[i] as f64) as f32, (sum[i][2] / cnt[i] as f64) as f32];
            }
        }
    }
    cent
}
fn nearest(cent: &[[f32; 3]], s: [f32; 3]) -> usize {
    let mut b = 0;
    let mut bd = f32::MAX;
    for (i, c) in cent.iter().enumerate() {
        let d = d2(*c, s);
        if d < bd {
            bd = d;
            b = i;
        }
    }
    b
}

/// 3x3 majority filter of a label map (labels >= `skip` are left alone)
fn mode_filter(l: &[u16], w: usize, h: usize, skip: u16) -> Vec<u16> {
    let mut o = l.to_vec();
    o.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            if l[y * w + x] >= skip {
                continue;
            }
            let mut v = [0u16; 9];
            let mut n = 0;
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    let (xx, yy) = (x as isize + dx, y as isize + dy);
                    if xx >= 0 && yy >= 0 && xx < w as isize && yy < h as isize {
                        let q = l[yy as usize * w + xx as usize];
                        if q < skip {
                            v[n] = q;
                            n += 1;
                        }
                    }
                }
            }
            let mut best = row[x];
            let mut bc = 0;
            for i in 0..n {
                let c = v[..n].iter().filter(|&&q| q == v[i]).count();
                if c > bc {
                    bc = c;
                    best = v[i];
                }
            }
            row[x] = best;
        }
    });
    o
}

/// what lineart / flats / shading need to know about a picture
struct Analysis {
    /// lineart coverage 0..1
    line: Vec<f32>,
    /// the picture with the lines painted out (nearest non-line colour)
    fill: Vec<[f32; 3]>,
    /// flat group per pixel (NONE where transparent): a line-bounded region, merged with its neighbours of the
    /// same hue / chroma so lit and shaded parts of one material share a flat
    label: Vec<u32>,
    /// flat colour of each group
    rep: Vec<[f32; 3]>,
}
const NONE: u32 = u32::MAX;

fn min_filter(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let neg: Vec<f32> = v.iter().map(|x| -x).collect();
    max_filter(&neg, w, h, r).into_iter().map(|x| -x).collect()
}

/// regions of equal label (4-neighbour)
fn label_regions(l: &[u16], w: usize, h: usize) -> (Vec<u32>, usize) {
    let mut reg = vec![u32::MAX; w * h];
    let mut n = 0u32;
    let mut stack = vec![];
    for s in 0..w * h {
        if reg[s] != u32::MAX {
            continue;
        }
        reg[s] = n;
        stack.push(s);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            for j in [if x > 0 { Some(i - 1) } else { None }, if x + 1 < w { Some(i + 1) } else { None }, if y > 0 { Some(i - w) } else { None }, if y + 1 < h { Some(i + w) } else { None }].into_iter().flatten() {
                if reg[j] == u32::MAX && l[j] == l[i] {
                    reg[j] = n;
                    stack.push(j);
                }
            }
        }
        n += 1;
    }
    (reg, n as usize)
}

fn uf_find(p: &mut [u32], mut x: u32) -> u32 {
    while p[x as usize] != x {
        p[x as usize] = p[p[x as usize] as usize];
        x = p[x as usize];
    }
    x
}

fn analyse(c: &[[f32; 3]], a: &[f32], w: usize, h: usize, o: &SplitOpts) -> Analysis {
    let n = w * h;
    // lines: black top-hat (closing minus picture) finds dark strokes thinner than the line width, and not
    // the border between a dark area and a light one
    let y: Vec<f32> = (0..n).map(|i| if a[i] > 0.02 { luma(c[i]) } else { 1.0 }).collect();
    let r = o.line_width.round().clamp(1.0, 24.0) as usize;
    let closed = min_filter(&max_filter(&y, w, h, r), w, h, r);
    let t0 = 0.45 - 0.37 * o.line_sens.clamp(0.0, 1.0);
    let line: Vec<f32> = (0..n)
        .into_par_iter()
        .map(|i| {
            if a[i] <= 0.02 {
                return 0.0;
            }
            let d = (closed[i] - y[i]) / closed[i].max(0.04);
            smoothstep(t0, t0 + 0.22, d)
        })
        .collect();
    // paint the lines out with the nearest clean colour
    let known: Vec<bool> = (0..n).map(|i| line[i] < 0.3 && a[i] > 0.02).collect();
    let near = nearest_seed(&known, w, h);
    let fill: Vec<[f32; 3]> = (0..n).map(|i| if known[i] || near[i] == u32::MAX { c[i] } else { c[near[i] as usize] }).collect();
    // colour clusters -> line-bounded regions
    let idx: Vec<usize> = (0..n).filter(|&i| known[i] && a[i] > 0.5).collect();
    let stride = (idx.len() / 60_000).max(1);
    let samples: Vec<[f32; 3]> = idx.iter().step_by(stride).map(|&i| feat(c[i])).collect();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let cent = kmeans(&samples, o.colors.clamp(2, 64).max(8), &mut rng);
    const T: u16 = u16::MAX;
    let mut cl: Vec<u16> = (0..n).into_par_iter().map(|i| if a[i] > 0.02 && !cent.is_empty() { nearest(&cent, feat(fill[i])) as u16 } else { T }).collect();
    for _ in 0..2 {
        cl = mode_filter(&cl, w, h, T);
    }
    let (reg, nreg) = label_regions(&cl, w, h);
    // region colour as oklab chroma relative to lightness: a pure shade / tint of a colour keeps a/L, b/L
    let mut sum = vec![[0.0f64; 3]; nreg];
    let mut cnt = vec![0usize; nreg];
    for i in 0..n {
        if cl[i] == T || !known[i] {
            continue;
        }
        let o = oklab(fill[i]);
        let r = reg[i] as usize;
        for ch in 0..3 {
            sum[r][ch] += o[ch] as f64;
        }
        cnt[r] += 1;
    }
    let chroma: Vec<[f32; 2]> = (0..nreg)
        .map(|r| {
            let k = cnt[r].max(1) as f64;
            let l = (sum[r][0] / k).max(0.05);
            [(sum[r][1] / k / l) as f32, (sum[r][2] / k / l) as f32]
        })
        .collect();
    // merge touching regions (not across a line) of the same relative chroma
    let mut par: Vec<u32> = (0..nreg as u32).collect();
    for i in 0..n {
        if cl[i] == T || line[i] >= 0.3 {
            continue;
        }
        let (x, yy) = (i % w, i / w);
        for j in [if x + 1 < w { Some(i + 1) } else { None }, if yy + 1 < h { Some(i + w) } else { None }].into_iter().flatten() {
            if cl[j] == T || line[j] >= 0.3 || reg[j] == reg[i] {
                continue;
            }
            let (p, q) = (reg[i] as usize, reg[j] as usize);
            if cnt[p] == 0 || cnt[q] == 0 {
                continue;
            }
            let d = ((chroma[p][0] - chroma[q][0]).powi(2) + (chroma[p][1] - chroma[q][1]).powi(2)).sqrt();
            if d < 0.07 {
                let (rp, rq) = (uf_find(&mut par, p as u32), uf_find(&mut par, q as u32));
                if rp != rq {
                    par[rp as usize] = rq;
                }
            }
        }
    }
    // group ids
    let mut gid = vec![NONE; nreg];
    let mut ng = 0u32;
    let mut label = vec![NONE; n];
    for i in 0..n {
        if cl[i] == T {
            continue;
        }
        let root = uf_find(&mut par, reg[i]) as usize;
        if gid[root] == NONE {
            gid[root] = ng;
            ng += 1;
        }
        label[i] = gid[root];
    }
    // a flat is the lit half of its group: shading then reads as shadow (multiply) more than highlight
    let k = ng as usize;
    let mut lums: Vec<Vec<f32>> = vec![vec![]; k];
    for i in 0..n {
        if label[i] != NONE && known[i] {
            lums[label[i] as usize].push(luma(fill[i]));
        }
    }
    let med: Vec<f32> = lums
        .iter_mut()
        .map(|v| {
            if v.is_empty() {
                return 0.0;
            }
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[v.len() / 2]
        })
        .collect();
    let mut sum = vec![[0.0f64; 3]; k];
    let mut cnt = vec![0usize; k];
    let mut sum_all = vec![[0.0f64; 3]; k];
    let mut cnt_all = vec![0usize; k];
    for i in 0..n {
        let l = label[i];
        if l == NONE {
            continue;
        }
        let l = l as usize;
        let f = fill[i];
        for ch in 0..3 {
            sum_all[l][ch] += f[ch] as f64;
        }
        cnt_all[l] += 1;
        if known[i] && luma(f) >= med[l] {
            for ch in 0..3 {
                sum[l][ch] += f[ch] as f64;
            }
            cnt[l] += 1;
        }
    }
    let rep = (0..k)
        .map(|l| {
            let (s, c) = if cnt[l] > 0 { (sum[l], cnt[l]) } else { (sum_all[l], cnt_all[l].max(1)) };
            [(s[0] / c as f64) as f32, (s[1] / c as f64) as f32, (s[2] / c as f64) as f32]
        })
        .collect();
    Analysis { line, fill, label, rep }
}

fn hex(c: [f32; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", (c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8)
}

/// lineart / flats / shadows / highlights of one (cropped) picture, bottom to top
fn decompose(c: &[[f32; 3]], a: &[f32], w: usize, h: usize, x0: i32, y0: i32, o: &SplitOpts, prefix: &str, group: i32) -> Vec<OutLayer> {
    let n = w * h;
    let an = analyse(c, a, w, h, o);
    let mk = |name: String, blend: u8, px: Vec<Px>| OutLayer { name: format!("{prefix}{name}"), blend, x0, y0, w, h, px, group };
    let mut out = vec![];
    // flats
    if o.per_color {
        // group the flats into at most `colors` layers by colour
        let feats: Vec<[f32; 3]> = an.rep.iter().map(|c| feat(*c)).collect();
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        let cent = kmeans(&feats, o.colors.clamp(2, 64), &mut rng);
        let bucket: Vec<usize> = feats.iter().map(|f| nearest(&cent, *f)).collect();
        let mut area = vec![0usize; cent.len()];
        let mut col = vec![[0.0f64; 3]; cent.len()];
        for i in 0..n {
            if an.label[i] != NONE {
                let b = bucket[an.label[i] as usize];
                area[b] += 1;
                let rc = an.rep[an.label[i] as usize];
                for ch in 0..3 {
                    col[b][ch] += rc[ch] as f64;
                }
            }
        }
        let mut used: Vec<usize> = (0..cent.len()).filter(|&b| area[b] > 0).collect();
        used.sort_by(|x, y| area[*y].cmp(&area[*x]));
        for b in used {
            let k = area[b] as f64;
            let avg = [(col[b][0] / k) as f32, (col[b][1] / k) as f32, (col[b][2] / k) as f32];
            let px: Vec<Px> = (0..n)
                .map(|i| {
                    if an.label[i] == NONE || bucket[an.label[i] as usize] != b {
                        return [0; 4];
                    }
                    let rc = an.rep[an.label[i] as usize];
                    fromf([rc[0] * a[i], rc[1] * a[i], rc[2] * a[i], a[i]])
                })
                .collect();
            out.push(mk(format!("Colour {}", hex(avg)), 0, px));
        }
    } else {
        let px: Vec<Px> = (0..n)
            .map(|i| {
                if an.label[i] == NONE {
                    return [0; 4];
                }
                let rc = an.rep[an.label[i] as usize];
                fromf([rc[0] * a[i], rc[1] * a[i], rc[2] * a[i], a[i]])
            })
            .collect();
        out.push(mk("Flats".into(), 0, px));
    }
    // shading: per channel, multiply where darker than the flat, screen where lighter
    let mut shade = vec![[0u16; 4]; n];
    let mut light = vec![[0u16; 4]; n];
    shade.par_iter_mut().zip(light.par_iter_mut()).enumerate().for_each(|(i, (sp, lp))| {
        if an.label[i] == NONE {
            return;
        }
        let b = an.rep[an.label[i] as usize];
        let s = an.fill[i];
        let mut m = [1.0f32; 3];
        let mut hl = [0.0f32; 3];
        for ch in 0..3 {
            if s[ch] <= b[ch] {
                m[ch] = if b[ch] > 0.002 { (s[ch] / b[ch]).clamp(0.0, 1.0) } else { 1.0 };
            } else {
                hl[ch] = if b[ch] < 0.998 { (1.0 - (1.0 - s[ch]) / (1.0 - b[ch])).clamp(0.0, 1.0) } else { 0.0 };
            }
        }
        let am = 1.0 - m[0].min(m[1]).min(m[2]);
        if am > 0.004 {
            let col: Vec<f32> = (0..3).map(|ch| (1.0 - (1.0 - m[ch]) / am).clamp(0.0, 1.0)).collect();
            let al = am * a[i];
            *sp = fromf([col[0] * al, col[1] * al, col[2] * al, al]);
        }
        let ah = hl[0].max(hl[1]).max(hl[2]);
        if ah > 0.004 {
            let al = ah * a[i];
            *lp = fromf([hl[0] / ah * al, hl[1] / ah * al, hl[2] / ah * al, al]);
        }
    });
    out.push(mk("Shadows".into(), MULTIPLY, shade));
    out.push(mk("Highlights".into(), SCREEN, light));
    // lineart, un-blended against the clean colour under it
    let lines: Vec<Px> = (0..n)
        .into_par_iter()
        .map(|i| {
            let l = an.line[i];
            if l < 0.02 {
                return [0; 4];
            }
            let u = an.fill[i];
            let col: Vec<f32> = (0..3).map(|ch| ((c[i][ch] - (1.0 - l) * u[ch]) / l).clamp(0.0, 1.0)).collect();
            let al = l * a[i];
            fromf([col[0] * al, col[1] * al, col[2] * al, al])
        })
        .collect();
    out.push(mk("Lineart".into(), 0, lines));
    out
}

/// LingOS: no SD server, so no AI matte
#[cfg(not(feature = "std"))]
fn ai_matte(_url: &str, _c: &[[f32; 3]], _a: &[f32], _w: usize, _h: usize) -> Result<Vec<f32>, String> {
    Err(String::from("AI matte: Stable Diffusion is not available on LingOS - use the colour-region method"))
}

/// AI foreground matte from the SD server's rembg extension (isnet-anime model), 0..1 per pixel
#[cfg(feature = "std")]
fn ai_matte(url: &str, c: &[[f32; 3]], a: &[f32], w: usize, h: usize) -> Result<Vec<f32>, String> {
    use base64::Engine;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for i in 0..w * h {
        rgba.extend_from_slice(&[(c[i][0] * 255.0) as u8, (c[i][1] * 255.0) as u8, (c[i][2] * 255.0) as u8, (a[i] * 255.0) as u8]);
    }
    let mut png = std::io::Cursor::new(Vec::new());
    image::write_buffer_with_format(&mut png, &rgba, w as u32, h as u32, image::ExtendedColorType::Rgba8, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
    let body = serde_json::json!({ "input_image": b64, "model": "isnet-anime", "return_mask": true, "alpha_matting": false }).to_string();
    let reply = crate::sd::http(url, "POST", "/rembg", Some(&body), 300).map_err(|e| format!("AI matte: {e} (needs the rembg extension on the SD server)"))?;
    let j: serde_json::Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
    let s = j.get("image").and_then(|v| v.as_str()).ok_or("AI matte: no image in the reply")?;
    let s = s.split(',').next_back().unwrap_or(s);
    let bytes = base64::engine::general_purpose::STANDARD.decode(s).map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?.to_rgba8();
    let img = if img.width() as usize != w || img.height() as usize != h { image::imageops::resize(&img, w as u32, h as u32, image::imageops::FilterType::Triangle) } else { img };
    Ok(img.pixels().map(|p| if p[3] < 255 { p[3] as f32 / 255.0 } else { luma([p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]) }).collect())
}

/// foreground by flooding the background in from the top and sides: the flood steps between similar
/// neighbouring pixels (so sky gradients pass) and never across lineart (so outlined objects stop it)
fn fg_from_regions(c: &[[f32; 3]], a: &[f32], w: usize, h: usize, o: &SplitOpts) -> Vec<f32> {
    let n = w * h;
    let an = analyse(c, a, w, h, o);
    let tol = 0.035 + 0.06 * (1.0 - o.line_sens.clamp(0.0, 1.0));
    let mut bgm = vec![false; n];
    let mut stack: Vec<usize> = vec![];
    let seed = |i: usize, bgm: &mut Vec<bool>, stack: &mut Vec<usize>| {
        if !bgm[i] && an.line[i] < 0.35 {
            bgm[i] = true;
            stack.push(i);
        }
    };
    for x in 0..w {
        seed(x, &mut bgm, &mut stack);
    }
    for y in 0..h {
        seed(y * w, &mut bgm, &mut stack);
        seed(y * w + w - 1, &mut bgm, &mut stack);
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let ci = c[i];
        let mut go = |j: usize| {
            if !bgm[j] && an.line[j] < 0.35 {
                let cj = c[j];
                let d = (ci[0] - cj[0]).abs().max((ci[1] - cj[1]).abs()).max((ci[2] - cj[2]).abs());
                if d < tol || a[j] <= 0.02 {
                    bgm[j] = true;
                    stack.push(j);
                }
            }
        };
        if x > 0 {
            go(i - 1);
        }
        if x + 1 < w {
            go(i + 1);
        }
        if y > 0 {
            go(i - w);
        }
        if y + 1 < h {
            go(i + w);
        }
    }
    // pockets: background showing through enclosed gaps (between an arm and the body, inside a hair loop)
    // match the flooded background's colour - take those back out of the foreground
    let flooded: Vec<usize> = (0..n).filter(|&i| bgm[i] && a[i] > 0.5).collect();
    if flooded.len() > n / 10 {
        let step = (flooded.len() / 50_000).max(1);
        let mut acc = [0.0f64; 3];
        let mut k = 0usize;
        for &i in flooded.iter().step_by(step) {
            for ch in 0..3 {
                acc[ch] += c[i][ch] as f64;
            }
            k += 1;
        }
        let bgc = [(acc[0] / k as f64) as f32, (acc[1] / k as f64) as f32, (acc[2] / k as f64) as f32];
        let dist = |p: [f32; 3]| (p[0] - bgc[0]).abs().max((p[1] - bgc[1]).abs()).max((p[2] - bgc[2]).abs());
        let cand: Vec<bool> = (0..n).map(|i| !bgm[i] && an.line[i] < 0.3 && dist(c[i]) < 0.06).collect();
        let (pl, psz) = components(&cand, w, h);
        let mut sum = vec![0.0f32; psz.len()];
        for i in 0..n {
            if pl[i] >= 0 {
                sum[pl[i] as usize] += dist(c[i]);
            }
        }
        let min = (n / 4000).max(24);
        for i in 0..n {
            if pl[i] >= 0 {
                let b = pl[i] as usize;
                if psz[b] >= min && sum[b] / (psz[b] as f32) < 0.04 {
                    bgm[i] = true;
                }
            }
        }
    }
    let mut fg: Vec<bool> = (0..n).map(|i| !bgm[i] && a[i] > 0.02).collect();
    // an object's outline belongs to the object
    let fgf: Vec<f32> = fg.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect();
    let grow = max_filter(&fgf, w, h, o.line_width.round().clamp(1.0, 24.0) as usize + 1);
    for i in 0..n {
        if !fg[i] && grow[i] > 0.5 && an.line[i] > 0.25 {
            fg[i] = true;
        }
    }
    // specks are background
    let (cl, sizes) = components(&fg, w, h);
    let min = (n as f32 * 0.0015) as usize;
    for i in 0..n {
        if cl[i] >= 0 && sizes[cl[i] as usize] < min {
            fg[i] = false;
        }
    }
    let mut f: Vec<f32> = fg.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect();
    box_blur(&mut f, w, h, 1);
    f
}

pub fn split(src: &[[f32; 4]], w: usize, h: usize, o: &SplitOpts, prog: &AtomicU32) -> Result<SplitResult, String> {
    let n = w * h;
    if n == 0 {
        return Err("the layer is empty".into());
    }
    let set = |p: u32| prog.store(p, Ordering::Relaxed);
    set(20);
    let st: Vec<[f32; 4]> = src.par_iter().map(|&p| unpre(p)).collect();
    let c: Vec<[f32; 3]> = st.iter().map(|p| [p[0], p[1], p[2]]).collect();
    let a: Vec<f32> = st.iter().map(|p| p[3]).collect();
    let mut note = String::new();
    let mut res = SplitResult { layers: vec![], parts: vec![], hole: None, region_w: w, region_h: h, note: String::new() };

    if !o.objects {
        set(100);
        res.layers = decompose(&c, &a, w, h, 0, 0, o, "", -1);
        set(1000);
        res.note = "split into lineart, flats, shadows and highlights".into();
        return Ok(res);
    }

    // ── foreground
    let clear = a.iter().filter(|&&v| v < 0.5).count() as f32 / n as f32;
    let mut method = o.method;
    if method == 0 {
        method = if clear > 0.02 { 1 } else { 2 };
    }
    let fg: Vec<f32> = match method {
        1 => a.clone(),
        3 => match ai_matte(&o.sd_url, &c, &a, w, h) {
            Ok(m) => {
                note.push_str("AI matte; ");
                m
            }
            Err(e) => {
                note.push_str(&format!("{e} - used colour regions instead; "));
                fg_from_regions(&c, &a, w, h, o)
            }
        },
        _ => fg_from_regions(&c, &a, w, h, o),
    };
    set(250);
    let has_plate = method != 1;

    // ── objects: connected blobs of the foreground, soft edges go to the nearest blob
    let core: Vec<bool> = fg.iter().map(|&f| f > 0.5).collect();
    let (cl, sizes) = components(&core, w, h);
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|a, b| sizes[*b].cmp(&sizes[*a]));
    let keep = o.max_objects.max(1);
    let min_obj = (n as f32 * 0.0005).max(16.0) as usize;
    // blob -> object id (the small ones share a "Details" object)
    let mut obj_of = vec![usize::MAX; sizes.len()];
    let mut nobj = 0;
    let mut details = usize::MAX;
    for &b in &order {
        if sizes[b] >= min_obj && (nobj < keep - 1 || (nobj < keep && order.len() <= keep)) {
            obj_of[b] = nobj;
            nobj += 1;
        } else {
            if details == usize::MAX {
                details = nobj;
                nobj += 1;
            }
            obj_of[b] = details;
        }
    }
    let seed: Vec<bool> = (0..n).map(|i| cl[i] >= 0 && obj_of[cl[i] as usize] != usize::MAX).collect();
    let near = nearest_seed(&seed, w, h);
    let obj: Vec<usize> = (0..n)
        .map(|i| {
            if fg[i] <= 0.02 || near[i] == u32::MAX {
                return usize::MAX;
            }
            let b = cl[near[i] as usize];
            if b < 0 {
                usize::MAX
            } else {
                obj_of[b as usize]
            }
        })
        .collect();
    set(350);

    // ── clean plate behind the objects
    let mut plate: Vec<[f32; 4]> = src.to_vec();
    if has_plate {
        let fgk: Vec<f32> = (0..n).map(|i| if obj[i] != usize::MAX { fg[i] } else { 0.0 }).collect();
        let hole = max_filter(&fgk, w, h, 2);
        let wt: Vec<f32> = hole.iter().map(|&hv| if hv > 0.02 { 0.0 } else { 1.0 }).collect();
        let smooth = {
            let mut p = plate.clone();
            push_pull(&mut p, &wt, w, h);
            p
        };
        // across each row, interpolate between the known pixels either side of the hole (keeps horizons and
        // ground lines continuous), mixed with the smooth fill so it does not streak
        plate.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let wrow = &wt[y * w..y * w + w];
            let mut x = 0;
            while x < w {
                if wrow[x] >= 1.0 {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < w && wrow[x] < 1.0 {
                    x += 1;
                }
                let (l, r) = (start.checked_sub(1), if x < w { Some(x) } else { None });
                for k in start..x {
                    let sm = smooth[y * w + k];
                    let lin = match (l, r) {
                        (Some(l), Some(r)) => {
                            let t = (k - l) as f32 / (r - l) as f32;
                            let (a, b) = (row[l], row[r]);
                            [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
                        }
                        (Some(l), None) => row[l],
                        (None, Some(r)) => row[r],
                        _ => sm,
                    };
                    let span = (x - start) as f32 / w as f32;
                    let m = if span < 0.6 { 0.7 } else { 0.3 };
                    row[k] = [lin[0] * m + sm[0] * (1.0 - m), lin[1] * m + sm[1] * (1.0 - m), lin[2] * m + sm[2] * (1.0 - m), lin[3] * m + sm[3] * (1.0 - m)];
                }
            }
        });
        // soften the rows vertically inside the fill only
        let mut col: Vec<[f32; 4]> = plate.clone();
        gauss_rows(&mut col, w, h, 3);
        for i in 0..n {
            if wt[i] < 1.0 {
                plate[i] = col[i];
            }
        }
        // a little grain-free softening inside the fill so the seams do not show as hard steps
        res.hole = Some(hole.iter().map(|&v| if v > 0.02 { 1.0 } else { 0.0 }).collect());
        note.push_str("background filled in behind the objects; ");
    }
    set(500);

    // ── object layers: un-blend the soft edge from the plate so object over plate == the original
    struct Obj {
        name: String,
        b: [usize; 4],
        bottom: usize,
    }
    let mut objs: Vec<Obj> = (0..nobj).map(|k| Obj { name: if k == details { "Details".into() } else { format!("Object {}", k + 1) }, b: [usize::MAX, usize::MAX, 0, 0], bottom: 0 }).collect();
    for i in 0..n {
        if obj[i] != usize::MAX {
            let (x, y) = (i % w, i / w);
            let ob = &mut objs[obj[i]];
            ob.b = [ob.b[0].min(x), ob.b[1].min(y), ob.b[2].max(x + 1), ob.b[3].max(y + 1)];
        }
    }
    for ob in objs.iter_mut() {
        ob.bottom = ob.b[3];
    }
    // back to front: what reaches lower in the frame is nearer
    let mut depth_order: Vec<usize> = (0..nobj).filter(|&k| objs[k].b[2] > 0).collect();
    depth_order.sort_by_key(|&k| objs[k].bottom);
    let obj_px = |k: usize| -> (Vec<[f32; 4]>, [usize; 4]) {
        let b = objs[k].b;
        let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
        let mut out = vec![[0.0f32; 4]; bw * bh];
        for y in 0..bh {
            for x in 0..bw {
                let i = (b[1] + y) * w + b[0] + x;
                if obj[i] != k {
                    continue;
                }
                let p = src[i];
                if !has_plate {
                    out[y * bw + x] = p;
                    continue;
                }
                let f = fg[i].clamp(0.0, 1.0);
                let q = plate[i];
                let al = (p[3] - (1.0 - f) * q[3]).clamp(0.0, 1.0);
                if al <= 0.002 {
                    continue;
                }
                out[y * bw + x] = [(p[0] - (1.0 - f) * q[0]).clamp(0.0, al), (p[1] - (1.0 - f) * q[1]).clamp(0.0, al), (p[2] - (1.0 - f) * q[2]).clamp(0.0, al), al];
            }
        }
        (out, b)
    };
    let to8 = |px: &[[f32; 4]]| -> Vec<u8> {
        let mut v = Vec::with_capacity(px.len() * 4);
        for p in px {
            let s = unpre(*p);
            v.extend_from_slice(&[(s[0] * 255.0 + 0.5) as u8, (s[1] * 255.0 + 0.5) as u8, (s[2] * 255.0 + 0.5) as u8, (s[3] * 255.0 + 0.5) as u8]);
        }
        v
    };

    if has_plate {
        if o.components {
            let ps: Vec<[f32; 4]> = plate.iter().map(|p| unpre(*p)).collect();
            let pc: Vec<[f32; 3]> = ps.iter().map(|p| [p[0], p[1], p[2]]).collect();
            let pa: Vec<f32> = ps.iter().map(|p| p[3]).collect();
            res.layers.extend(decompose(&pc, &pa, w, h, 0, 0, o, "Background · ", 0));
        } else {
            res.layers.push(OutLayer { name: "Background (filled)".into(), blend: 0, x0: 0, y0: 0, w, h, px: plate.iter().map(|p| fromf(*p)).collect(), group: 0 });
        }
        res.parts.push(PartOut { name: "Background".into(), x0: 0, y0: 0, w, h, rgba: to8(&plate), depth: 0 });
    }
    let total = depth_order.len().max(1);
    for (d, &k) in depth_order.iter().enumerate() {
        set(500 + (450 * d / total) as u32);
        let (px, b) = obj_px(k);
        let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
        let name = objs[k].name.clone();
        if o.components {
            let st: Vec<[f32; 4]> = px.iter().map(|p| unpre(*p)).collect();
            let oc: Vec<[f32; 3]> = st.iter().map(|p| [p[0], p[1], p[2]]).collect();
            let oa: Vec<f32> = st.iter().map(|p| p[3]).collect();
            res.layers.extend(decompose(&oc, &oa, bw, bh, b[0] as i32, b[1] as i32, o, &format!("{name} · "), d as i32 + 1));
        } else {
            res.layers.push(OutLayer { name: name.clone(), blend: 0, x0: b[0] as i32, y0: b[1] as i32, w: bw, h: bh, px: px.iter().map(|p| fromf(*p)).collect(), group: d as i32 + 1 });
        }
        res.parts.push(PartOut { name, x0: b[0] as i32, y0: b[1] as i32, w: bw, h: bh, rgba: to8(&px), depth: d + 1 });
    }
    note.push_str(&format!("{} object{}", depth_order.len(), if depth_order.len() == 1 { "" } else { "s" }));
    res.note = note;
    set(1000);
    Ok(res)
}
