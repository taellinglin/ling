//! Vector paths: cubic Bezier anchors with a width per anchor (pressure), optional fill (solid or gradient),
//! rasterised anti-aliased into the vector layer's tile cache.

use crate::gradient::{param, Gradient};
#[allow(unused_imports)]
use crate::prelude::*;
use crate::px::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Serialize, Deserialize, Debug, Default)]
pub struct Anchor {
    pub x: f32,
    pub y: f32,
    /// incoming / outgoing handles, relative to the anchor
    pub ix: f32,
    pub iy: f32,
    pub ox: f32,
    pub oy: f32,
    /// width multiplier (pressure) 0..1
    pub w: f32,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct GradFill {
    pub g: Gradient,
    pub kind: u8,
    pub rep: u8,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct VPath {
    pub a: Vec<Anchor>,
    pub closed: bool,
    pub stroke: bool,
    /// stroke diameter in pixels at w = 1
    pub width: f32,
    pub color: [f32; 4],
    pub fill: Option<[f32; 4]>,
    pub gfill: Option<GradFill>,
    pub taper_in: f32,
    pub taper_out: f32,
    pub opacity: f32,
}
impl Default for VPath {
    fn default() -> Self {
        VPath { a: vec![], closed: false, stroke: true, width: 4.0, color: [0.0, 0.0, 0.0, 1.0], fill: None, gfill: None, taper_in: 0.0, taper_out: 0.0, opacity: 1.0 }
    }
}

fn cubic(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32), t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    let a = u * u * u;
    let b = 3.0 * u * u * t;
    let c = 3.0 * u * t * t;
    let d = t * t * t;
    (a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0, a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1)
}

impl VPath {
    /// flatten to (x, y, radius) points
    pub fn flatten(&self) -> Vec<(f32, f32, f32)> {
        let n = self.a.len();
        let mut out = Vec::new();
        if n == 0 {
            return out;
        }
        let segs = if self.closed { n } else { n - 1 };
        for i in 0..segs {
            let a = self.a[i];
            let b = self.a[(i + 1) % n];
            let p0 = (a.x, a.y);
            let p1 = (a.x + a.ox, a.y + a.oy);
            let p2 = (b.x + b.ix, b.y + b.iy);
            let p3 = (b.x, b.y);
            let est = ((p1.0 - p0.0).hypot(p1.1 - p0.1) + (p2.0 - p1.0).hypot(p2.1 - p1.1) + (p3.0 - p2.0).hypot(p3.1 - p2.1)).max(1.0);
            let steps = (est / 2.5).ceil().clamp(1.0, 400.0) as usize;
            for s in 0..steps {
                let t = s as f32 / steps as f32;
                let (x, y) = cubic(p0, p1, p2, p3, t);
                out.push((x, y, a.w + (b.w - a.w) * t));
            }
        }
        let l = if self.closed { self.a[0] } else { self.a[n - 1] };
        out.push((l.x, l.y, l.w));
        // tapers along the arc length
        if (self.taper_in > 0.0 || self.taper_out > 0.0) && out.len() > 1 {
            let mut cum = vec![0.0f32; out.len()];
            for i in 1..out.len() {
                cum[i] = cum[i - 1] + (out[i].0 - out[i - 1].0).hypot(out[i].1 - out[i - 1].1);
            }
            let total = *cum.last().unwrap();
            for i in 0..out.len() {
                let mut k = 1.0f32;
                if self.taper_in > 0.0 {
                    k = k.min(cum[i] / self.taper_in);
                }
                if self.taper_out > 0.0 {
                    k = k.min((total - cum[i]) / self.taper_out);
                }
                out[i].2 *= smooth01(k).max(0.04);
            }
        }
        for p in out.iter_mut() {
            p.2 = p.2.max(0.0) * self.width * 0.5;
        }
        out
    }

    pub fn bbox(&self) -> [f32; 4] {
        let pts = self.flatten();
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for (x, y, r) in pts {
            b[0] = b[0].min(x - r);
            b[1] = b[1].min(y - r);
            b[2] = b[2].max(x + r);
            b[3] = b[3].max(y + r);
        }
        b
    }

    /// distance from (x, y) to the path's outline (negative inside a filled shape)
    pub fn dist(&self, x: f32, y: f32) -> f32 {
        let pts = self.flatten();
        let mut best = f32::MAX;
        for i in 0..pts.len().saturating_sub(1) {
            let (ax, ay, ar) = pts[i];
            let (bx, by, br) = pts[i + 1];
            let (dx, dy) = (bx - ax, by - ay);
            let l2 = (dx * dx + dy * dy).max(1e-6);
            let t = (((x - ax) * dx + (y - ay) * dy) / l2).clamp(0.0, 1.0);
            let d = (x - ax - dx * t).hypot(y - ay - dy * t) - (ar + (br - ar) * t);
            best = best.min(d);
        }
        if pts.len() == 1 {
            best = (x - pts[0].0).hypot(y - pts[0].1) - pts[0].2;
        }
        if (self.fill.is_some() || self.gfill.is_some()) && self.closed && inside(&pts, x, y) {
            return best.min(-1.0);
        }
        best
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        for a in self.a.iter_mut() {
            a.x += dx;
            a.y += dy;
        }
        if let Some(g) = self.gfill.as_mut() {
            g.x0 += dx;
            g.x1 += dx;
            g.y0 += dy;
            g.y1 += dy;
        }
    }

    /// paint the path into a layer
    pub fn raster(&self, tiles: &mut Tiles) {
        let pts = self.flatten();
        if pts.is_empty() {
            return;
        }
        let (w, h) = (tiles.w as i32, tiles.h as i32);
        let bb = self.bbox();
        let x0 = (bb[0].floor() as i32 - 2).max(0);
        let y0 = (bb[1].floor() as i32 - 2).max(0);
        let x1 = (bb[2].ceil() as i32 + 2).min(w);
        let y1 = (bb[3].ceil() as i32 + 2).min(h);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let bw = (x1 - x0) as usize;
        let bh = (y1 - y0) as usize;
        // fill coverage
        if self.closed && (self.fill.is_some() || self.gfill.is_some()) && pts.len() > 2 {
            let cov = fill_cov(&pts, x0, y0, bw, bh);
            let solid = self.fill.unwrap_or([0.0, 0.0, 0.0, 1.0]);
            let gf = self.gfill.clone();
            let op = self.opacity;
            tiles.for_rect(x0, y0, x1, y1, |x, y, p| {
                let c = cov[(y - y0) as usize * bw + (x - x0) as usize];
                if c <= 0.0 {
                    return;
                }
                let col = match &gf {
                    Some(g) => g.g.eval(param(g.kind, g.rep, g.x0, g.y0, g.x1, g.y1, x as f32 + 0.5, y as f32 + 0.5)),
                    None => solid,
                };
                let a = col[3] * c * op;
                *p = fromf(over(tof(*p), [col[0] * a, col[1] * a, col[2] * a, a]));
            });
        }
        if self.stroke && self.width > 0.0 {
            let mut cov = vec![0.0f32; bw * bh];
            let seg = |cov: &mut Vec<f32>, ax: f32, ay: f32, ar: f32, bx: f32, by: f32, br: f32| {
                let rmax = ar.max(br) + 1.5;
                let sx0 = ((ax.min(bx) - rmax).floor() as i32).max(x0);
                let sy0 = ((ay.min(by) - rmax).floor() as i32).max(y0);
                let sx1 = ((ax.max(bx) + rmax).ceil() as i32).min(x1);
                let sy1 = ((ay.max(by) + rmax).ceil() as i32).min(y1);
                let (dx, dy) = (bx - ax, by - ay);
                let l2 = (dx * dx + dy * dy).max(1e-6);
                for y in sy0..sy1 {
                    for x in sx0..sx1 {
                        let px = x as f32 + 0.5;
                        let py = y as f32 + 0.5;
                        let t = (((px - ax) * dx + (py - ay) * dy) / l2).clamp(0.0, 1.0);
                        let r = ar + (br - ar) * t;
                        let d = (px - ax - dx * t).hypot(py - ay - dy * t);
                        // thin lines keep their darkness by area instead of vanishing
                        let c = if r < 0.5 { (0.5 + r - d).clamp(0.0, 1.0) * (r * 2.0).max(0.15) } else { (0.5 + r - d).clamp(0.0, 1.0) };
                        let i = (y - y0) as usize * bw + (x - x0) as usize;
                        if c > cov[i] {
                            cov[i] = c;
                        }
                    }
                }
            };
            if pts.len() == 1 {
                let (x, y, r) = pts[0];
                seg(&mut cov, x, y, r, x, y, r);
            }
            for i in 0..pts.len().saturating_sub(1) {
                let (ax, ay, ar) = pts[i];
                let (bx, by, br) = pts[i + 1];
                seg(&mut cov, ax, ay, ar, bx, by, br);
            }
            let col = self.color;
            let op = self.opacity;
            tiles.for_rect(x0, y0, x1, y1, |x, y, p| {
                let c = cov[(y - y0) as usize * bw + (x - x0) as usize];
                if c <= 0.0 {
                    return;
                }
                let a = col[3] * c * op;
                *p = fromf(over(tof(*p), [col[0] * a, col[1] * a, col[2] * a, a]));
            });
        }
    }
}

fn inside(pts: &[(f32, f32, f32)], x: f32, y: f32) -> bool {
    let mut wind = 0;
    let n = pts.len();
    for i in 0..n {
        let (ax, ay, _) = pts[i];
        let (bx, by, _) = pts[(i + 1) % n];
        if ay <= y {
            if by > y && (bx - ax) * (y - ay) - (x - ax) * (by - ay) > 0.0 {
                wind += 1;
            }
        } else if by <= y && (bx - ax) * (y - ay) - (x - ax) * (by - ay) < 0.0 {
            wind -= 1;
        }
    }
    wind != 0
}

/// nonzero-winding polygon coverage with 4 sub-scanlines per pixel and exact horizontal span ends
fn fill_cov(pts: &[(f32, f32, f32)], x0: i32, y0: i32, bw: usize, bh: usize) -> Vec<f32> {
    let mut cov = vec![0.0f32; bw * bh];
    let n = pts.len();
    const SUB: usize = 4;
    let mut xs: Vec<(f32, i32)> = Vec::new();
    for row in 0..bh {
        for s in 0..SUB {
            let sy = y0 as f32 + row as f32 + (s as f32 + 0.5) / SUB as f32;
            xs.clear();
            for i in 0..n {
                let (ax, ay, _) = pts[i];
                let (bx, by, _) = pts[(i + 1) % n];
                if (ay <= sy && by > sy) || (by <= sy && ay > sy) {
                    let t = (sy - ay) / (by - ay);
                    xs.push((ax + (bx - ax) * t, if by > ay { 1 } else { -1 }));
                }
            }
            xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
            let mut wind = 0;
            for k in 0..xs.len() {
                let prev = wind;
                wind += xs[k].1;
                if prev == 0 && wind != 0 {
                    // span start
                    let start = xs[k].0;
                    let mut j = k + 1;
                    let mut w2 = wind;
                    while j < xs.len() {
                        w2 += xs[j].1;
                        if w2 == 0 {
                            break;
                        }
                        j += 1;
                    }
                    if j < xs.len() {
                        let end = xs[j].0;
                        let a = start - x0 as f32;
                        let b = end - x0 as f32;
                        let ia = a.floor().max(0.0) as usize;
                        let ib = (b.ceil() as usize).min(bw);
                        let base = row * bw;
                        for px in ia..ib {
                            let l = (px as f32).max(a);
                            let r = (px as f32 + 1.0).min(b);
                            if r > l {
                                cov[base + px] += (r - l) / SUB as f32;
                            }
                        }
                    }
                }
            }
        }
    }
    for c in cov.iter_mut() {
        *c = c.min(1.0);
    }
    cov
}

/// Ramer-Douglas-Peucker on (x, y, w)
fn rdp(pts: &[(f32, f32, f32)], eps: f32, out: &mut Vec<(f32, f32, f32)>) {
    if pts.len() < 3 {
        out.extend_from_slice(pts);
        return;
    }
    let (ax, ay, _) = pts[0];
    let (bx, by, _) = pts[pts.len() - 1];
    let (dx, dy) = (bx - ax, by - ay);
    let l = (dx * dx + dy * dy).sqrt().max(1e-6);
    let mut idx = 0;
    let mut dmax = 0.0;
    for (i, p) in pts.iter().enumerate().skip(1).take(pts.len() - 2) {
        let d = ((p.0 - ax) * dy - (p.1 - ay) * dx).abs() / l;
        // keep width changes too
        let wd = (p.2 - (pts[0].2 + pts[pts.len() - 1].2) * 0.5).abs() * 4.0;
        let d = d.max(wd);
        if d > dmax {
            dmax = d;
            idx = i;
        }
    }
    if dmax > eps {
        let mut left = Vec::new();
        rdp(&pts[..=idx], eps, &mut left);
        left.pop();
        out.extend(left);
        rdp(&pts[idx..], eps, out);
    } else {
        out.push(pts[0]);
        out.push(pts[pts.len() - 1]);
    }
}

/// turn a dense freehand polyline into a smooth Bezier path (Catmull-Rom handles)
pub fn fit(raw: &[(f32, f32, f32)], eps: f32) -> Vec<Anchor> {
    if raw.is_empty() {
        return vec![];
    }
    let mut s = Vec::new();
    rdp(raw, eps, &mut s);
    s.dedup_by(|a, b| (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01);
    let n = s.len();
    let mut out: Vec<Anchor> = s.iter().map(|&(x, y, w)| Anchor { x, y, w, ..Default::default() }).collect();
    for i in 0..n {
        let p = if i > 0 { s[i - 1] } else { s[i] };
        let q = if i + 1 < n { s[i + 1] } else { s[i] };
        let tx = (q.0 - p.0) / 6.0;
        let ty = (q.1 - p.1) / 6.0;
        if i > 0 {
            out[i].ix = -tx;
            out[i].iy = -ty;
        }
        if i + 1 < n {
            out[i].ox = tx;
            out[i].oy = ty;
        }
    }
    out
}

/// shape paths: kind 0 rect, 1 ellipse, 2 line, 3 polygon, 4 star
pub fn shape(kind: u8, x0: f32, y0: f32, x1: f32, y1: f32, sides: u32) -> (Vec<Anchor>, bool) {
    let a = |x: f32, y: f32| Anchor { x, y, w: 1.0, ..Default::default() };
    match kind {
        0 => (vec![a(x0, y0), a(x1, y0), a(x1, y1), a(x0, y1)], true),
        1 => {
            let cx = (x0 + x1) * 0.5;
            let cy = (y0 + y1) * 0.5;
            let rx = (x1 - x0).abs() * 0.5;
            let ry = (y1 - y0).abs() * 0.5;
            let k = 0.552_284_8;
            let mut v = vec![a(cx + rx, cy), a(cx, cy + ry), a(cx - rx, cy), a(cx, cy - ry)];
            v[0].iy = -ry * k;
            v[0].oy = ry * k;
            v[1].ix = rx * k;
            v[1].ox = -rx * k;
            v[2].iy = ry * k;
            v[2].oy = -ry * k;
            v[3].ix = -rx * k;
            v[3].ox = rx * k;
            (v, true)
        }
        2 => (vec![a(x0, y0), a(x1, y1)], false),
        _ => {
            let n = sides.clamp(3, 64) as usize;
            let cx = (x0 + x1) * 0.5;
            let cy = (y0 + y1) * 0.5;
            let rx = (x1 - x0).abs() * 0.5;
            let ry = (y1 - y0).abs() * 0.5;
            let mut v = Vec::new();
            let pts = if kind == 4 { n * 2 } else { n };
            for i in 0..pts {
                let ang = -core::f32::consts::FRAC_PI_2 + i as f32 / pts as f32 * core::f32::consts::TAU;
                let k = if kind == 4 && i % 2 == 1 { 0.45 } else { 1.0 };
                v.push(a(cx + ang.cos() * rx * k, cy + ang.sin() * ry * k));
            }
            (v, true)
        }
    }
}
