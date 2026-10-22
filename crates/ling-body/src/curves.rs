//! Crease curves: iso-contours of a scalar field on a triangle mesh, chained into
//! polylines and fitted with cubic Béziers (Schneider's least-squares fit with
//! Newton-Raphson reparameterisation and recursive splitting at the worst point).

use crate::mesh::Mesh;
use std::collections::HashMap;
use std::fmt::Write as _;

pub type Bez = [[f32; 3]; 4];

#[derive(Clone, Debug)]
pub struct Curve {
    pub segs: Vec<Bez>,
    /// Field value the contour follows.
    pub level: f32,
    /// 0.5 for the weakest contour level up to 1.0 for the strongest; scales crease depth.
    pub weight: f32,
    pub closed: bool,
    /// Length of the polyline the curve was fitted to, in metres.
    pub length: f32,
}

type V = [f64; 3];

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: V, s: f64) -> V {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: V) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: V) -> V {
    let n = norm(a);
    if n < 1e-12 {
        [0.0, 0.0, 0.0]
    } else {
        mul(a, 1.0 / n)
    }
}

/// Contour vertices (one per crossed mesh edge) and their links.
#[derive(Default)]
struct Chain {
    pts: Vec<V>,
    adj: Vec<Vec<usize>>,
    index: HashMap<(u32, u32), usize>,
}

impl Chain {
    fn crossing(&mut self, mesh: &Mesh, s: &[f32], level: f32, a: u32, b: u32) -> usize {
        let key = (a.min(b), a.max(b));
        if let Some(&i) = self.index.get(&key) {
            return i;
        }
        let (sa, sb) = (s[a as usize], s[b as usize]);
        let t = ((level - sa) / (sb - sa)) as f64;
        let p = |i: u32| {
            let o = i as usize * 3;
            [mesh.pos[o] as f64, mesh.pos[o + 1] as f64, mesh.pos[o + 2] as f64]
        };
        let (pa, pb) = (p(a), p(b));
        self.pts.push(add(pa, mul(sub(pb, pa), t)));
        self.adj.push(Vec::new());
        self.index.insert(key, self.pts.len() - 1);
        self.pts.len() - 1
    }
}

/// Polylines (with a closed flag) where `s` crosses `level` on the mesh surface.
fn contour_lines(mesh: &Mesh, s: &[f32], level: f32) -> Vec<(Vec<V>, bool)> {
    let mut chain = Chain::default();
    for t in mesh.idx.as_chunks::<3>().0 {
        let above = [s[t[0] as usize] >= level, s[t[1] as usize] >= level, s[t[2] as usize] >= level];
        let n = above.iter().filter(|&&b| b).count();
        if n == 0 || n == 3 {
            continue;
        }
        let odd = (0..3).find(|&k| above[k] == (n == 1)).unwrap_or(0);
        let (o1, o2) = (t[(odd + 1) % 3], t[(odd + 2) % 3]);
        let p = chain.crossing(mesh, s, level, t[odd], o1);
        let q = chain.crossing(mesh, s, level, t[odd], o2);
        chain.adj[p].push(q);
        chain.adj[q].push(p);
    }
    let Chain { pts, adj, .. } = chain;

    let mut seen = vec![false; pts.len()];
    let mut lines = Vec::new();
    let walk = |start: usize, seen: &mut Vec<bool>| {
        let mut path = vec![start];
        seen[start] = true;
        let mut cur = start;
        while let Some(&next) = adj[cur].iter().find(|&&n| !seen[n]) {
            seen[next] = true;
            path.push(next);
            cur = next;
        }
        let closed = path.len() > 2 && adj[cur].contains(&start);
        (path, closed)
    };
    for start in 0..pts.len() {
        if !seen[start] && adj[start].len() == 1 {
            let (path, closed) = walk(start, &mut seen);
            lines.push((path.iter().map(|&i| pts[i]).collect(), closed));
        }
    }
    for start in 0..pts.len() {
        if !seen[start] {
            let (path, closed) = walk(start, &mut seen);
            lines.push((path.iter().map(|&i| pts[i]).collect(), closed));
        }
    }
    lines
}

fn smooth_line(p: &mut [V], closed: bool, passes: usize) {
    let n = p.len();
    if n < 3 {
        return;
    }
    for _ in 0..passes {
        let src = p.to_vec();
        for i in 0..n {
            let (prev, next) = if closed {
                (src[(i + n - 1) % n], src[(i + 1) % n])
            } else if i == 0 || i == n - 1 {
                continue;
            } else {
                (src[i - 1], src[i + 1])
            };
            p[i] = add(mul(src[i], 0.5), mul(add(prev, next), 0.25));
        }
    }
}

fn line_length(p: &[V]) -> f64 {
    p.windows(2).map(|w| norm(sub(w[1], w[0]))).sum()
}

fn bez_point(b: &[V; 4], t: f64) -> V {
    let m = 1.0 - t;
    let (c0, c1, c2, c3) = (m * m * m, 3.0 * m * m * t, 3.0 * m * t * t, t * t * t);
    [0, 1, 2].map(|k| b[0][k] * c0 + b[1][k] * c1 + b[2][k] * c2 + b[3][k] * c3)
}

fn bez_d1(b: &[V; 4], t: f64) -> V {
    let m = 1.0 - t;
    let q = [sub(b[1], b[0]), sub(b[2], b[1]), sub(b[3], b[2])];
    add(add(mul(q[0], 3.0 * m * m), mul(q[1], 6.0 * m * t)), mul(q[2], 3.0 * t * t))
}

fn bez_d2(b: &[V; 4], t: f64) -> V {
    let m = 1.0 - t;
    let q = [
        add(sub(b[2], mul(b[1], 2.0)), b[0]),
        add(sub(b[3], mul(b[2], 2.0)), b[1]),
    ];
    add(mul(q[0], 6.0 * m), mul(q[1], 6.0 * t))
}

fn chord_params(d: &[V]) -> Vec<f64> {
    let mut u = vec![0.0];
    for w in d.windows(2) {
        u.push(u.last().copied().unwrap_or(0.0) + norm(sub(w[1], w[0])));
    }
    let total = u.last().copied().unwrap_or(1.0).max(1e-12);
    u.iter().map(|v| v / total).collect()
}

fn generate(d: &[V], u: &[f64], t1: V, t2: V) -> [V; 4] {
    let (first, last) = (d[0], d[d.len() - 1]);
    let (mut c00, mut c01, mut c11, mut x0, mut x1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (p, &t) in d.iter().zip(u) {
        let m = 1.0 - t;
        let (b0, b1, b2, b3) = (m * m * m, 3.0 * t * m * m, 3.0 * t * t * m, t * t * t);
        let (a0, a1) = (mul(t1, b1), mul(t2, b2));
        c00 += dot(a0, a0);
        c01 += dot(a0, a1);
        c11 += dot(a1, a1);
        let tmp = sub(*p, add(mul(first, b0 + b1), mul(last, b2 + b3)));
        x0 += dot(a0, tmp);
        x1 += dot(a1, tmp);
    }
    let det = c00 * c11 - c01 * c01;
    let (al, ar) = if det.abs() > 1e-12 { ((x0 * c11 - x1 * c01) / det, (c00 * x1 - c01 * x0) / det) } else { (0.0, 0.0) };
    let seg = norm(sub(last, first));
    let eps = 1e-6 * seg;
    if al < eps || ar < eps {
        let third = seg / 3.0;
        [first, add(first, mul(t1, third)), add(last, mul(t2, third)), last]
    } else {
        [first, add(first, mul(t1, al)), add(last, mul(t2, ar)), last]
    }
}

fn reparameterize(d: &[V], u: &[f64], b: &[V; 4]) -> Vec<f64> {
    d.iter()
        .zip(u)
        .map(|(&p, &t)| {
            let q = sub(bez_point(b, t), p);
            let (d1, d2) = (bez_d1(b, t), bez_d2(b, t));
            let num = dot(q, d1);
            let den = dot(d1, d1) + dot(q, d2);
            if den.abs() < 1e-12 {
                t
            } else {
                (t - num / den).clamp(0.0, 1.0)
            }
        })
        .collect()
}

fn max_error(d: &[V], u: &[f64], b: &[V; 4]) -> (f64, usize) {
    let mut worst = (0.0, d.len() / 2);
    for i in 1..d.len() - 1 {
        let e = sub(bez_point(b, u[i]), d[i]);
        let e2 = dot(e, e);
        if e2 > worst.0 {
            worst = (e2, i);
        }
    }
    worst
}

fn fit_cubic(d: &[V], t1: V, t2: V, err2: f64, out: &mut Vec<[V; 4]>) {
    if d.len() == 2 {
        let third = norm(sub(d[1], d[0])) / 3.0;
        out.push([d[0], add(d[0], mul(t1, third)), add(d[1], mul(t2, third)), d[1]]);
        return;
    }
    let mut u = chord_params(d);
    let mut b = generate(d, &u, t1, t2);
    let (mut worst, mut split) = max_error(d, &u, &b);
    if worst < err2 {
        out.push(b);
        return;
    }
    if worst < err2 * 16.0 {
        for _ in 0..6 {
            u = reparameterize(d, &u, &b);
            b = generate(d, &u, t1, t2);
            (worst, split) = max_error(d, &u, &b);
            if worst < err2 {
                out.push(b);
                return;
            }
        }
    }
    let split = split.clamp(1, d.len() - 2);
    let center = unit(sub(d[split - 1], d[split + 1]));
    fit_cubic(&d[..=split], t1, center, err2, out);
    fit_cubic(&d[split..], mul(center, -1.0), t2, err2, out);
}

fn fit_line(p: &[V], tol: f64) -> Vec<[V; 4]> {
    let mut out = Vec::new();
    if p.len() < 2 {
        return out;
    }
    let t1 = unit(sub(p[1], p[0]));
    let t2 = unit(sub(p[p.len() - 2], p[p.len() - 1]));
    fit_cubic(p, t1, t2, tol * tol, &mut out);
    out
}

/// Settings for turning a scalar field into fitted crease curves.
pub struct CreaseCfg {
    /// Number of contour levels, spread over the upper quantiles of the field.
    pub levels: usize,
    /// Fitting tolerance in metres.
    pub tolerance: f32,
    /// Curves with a shorter polyline are dropped.
    pub min_length: f32,
}

/// Iso-contours of `s` on `mesh` at `levels` quantile-spaced values, or at zero when
/// `nodal` is set (pressure nodal lines), fitted as cubic Bézier chains.
pub fn detect(mesh: &Mesh, s: &[f32], cfg: &CreaseCfg, nodal: bool) -> Vec<Curve> {
    let mut sorted: Vec<f32> = s.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    if sorted.is_empty() {
        return Vec::new();
    }
    let levels: Vec<(f32, f32)> = if nodal {
        vec![(0.0, 1.0)]
    } else {
        let n = cfg.levels.max(1);
        (0..n)
            .map(|k| {
                let f = if n == 1 { 0.0 } else { k as f32 / (n - 1) as f32 };
                let q = 0.55 + 0.4 * f;
                (sorted[((sorted.len() - 1) as f32 * q) as usize], 0.5 + 0.5 * f)
            })
            .collect()
    };
    let mut curves = Vec::new();
    for (level, weight) in levels {
        for (mut line, closed) in contour_lines(mesh, s, level) {
            smooth_line(&mut line, closed, 3);
            if closed {
                line.push(line[0]);
            }
            line.dedup_by(|a, b| norm(sub(*a, *b)) < 1e-6);
            let length = line_length(&line);
            if line.len() < 3 || length < cfg.min_length as f64 {
                continue;
            }
            let segs: Vec<Bez> = fit_line(&line, cfg.tolerance as f64)
                .into_iter()
                .map(|b| b.map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]))
                .collect();
            curves.push(Curve { segs, level, weight, closed, length: length as f32 });
        }
    }
    curves
}

pub fn bez_at(b: &Bez, t: f32) -> [f32; 3] {
    let v = b.map(|p| [p[0] as f64, p[1] as f64, p[2] as f64]);
    let p = bez_point(&v, t as f64);
    [p[0] as f32, p[1] as f32, p[2] as f32]
}

/// Points along a curve, `per_seg` samples per Bézier segment plus the end point.
pub fn flatten(c: &Curve, per_seg: usize) -> Vec<[f32; 3]> {
    let n = per_seg.max(1);
    let mut out = Vec::with_capacity(c.segs.len() * n + 1);
    for seg in &c.segs {
        for k in 0..n {
            out.push(bez_at(seg, k as f32 / n as f32));
        }
    }
    if let Some(last) = c.segs.last() {
        out.push(last[3]);
    }
    out
}

pub fn to_json(curves: &[Curve]) -> String {
    let list: Vec<serde_json::Value> = curves
        .iter()
        .map(|c| {
            serde_json::json!({
                "level": c.level, "weight": c.weight, "closed": c.closed, "length": c.length,
                "segments": c.segs,
            })
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({
        "format": "ling-body/creases", "units": "m", "up": "y", "forward": "z", "curves": list,
    }))
    .unwrap_or_default()
}

/// A Blender script that recreates every crease as a 3D Bézier curve object.
pub fn to_blender_script(curves: &[Curve]) -> String {
    let conv = |p: [f32; 3]| format!("({:.5}, {:.5}, {:.5})", p[0], -p[2], p[1]);
    let mut s = String::from(
        "import bpy\n\n# Creases from ling-body. Y-up metres converted to Blender Z-up.\n\
         def crease(name, anchors, closed):\n\
         \x20   cu = bpy.data.curves.new(name, 'CURVE')\n\
         \x20   cu.dimensions = '3D'\n\
         \x20   cu.bevel_depth = 0.001\n\
         \x20   sp = cu.splines.new('BEZIER')\n\
         \x20   sp.bezier_points.add(len(anchors) - 1)\n\
         \x20   for bp, (co, hl, hr) in zip(sp.bezier_points, anchors):\n\
         \x20       bp.handle_left_type = bp.handle_right_type = 'FREE'\n\
         \x20       bp.co, bp.handle_left, bp.handle_right = co, hl, hr\n\
         \x20   sp.use_cyclic_u = closed\n\
         \x20   ob = bpy.data.objects.new(name, cu)\n\
         \x20   bpy.context.collection.objects.link(ob)\n\n",
    );
    for (i, c) in curves.iter().enumerate() {
        let n = c.segs.len();
        if n == 0 {
            continue;
        }
        let mut anchors: Vec<([f32; 3], [f32; 3], [f32; 3])> = Vec::new();
        for k in 0..=n {
            let (co, left, right) = if k == 0 {
                (c.segs[0][0], c.segs[0][0], c.segs[0][1])
            } else if k == n {
                (c.segs[n - 1][3], c.segs[n - 1][2], c.segs[n - 1][3])
            } else {
                (c.segs[k - 1][3], c.segs[k - 1][2], c.segs[k][1])
            };
            anchors.push((co, left, right));
        }
        if c.closed && anchors.len() > 2 {
            let last = anchors.pop().unwrap_or(anchors[0]);
            anchors[0].1 = last.1;
        }
        let _ = write!(s, "crease('crease_{i}', [");
        for (co, l, r) in &anchors {
            let _ = write!(s, "({}, {}, {}), ", conv(*co), conv(*l), conv(*r));
        }
        let _ = writeln!(s, "], {})", if c.closed { "True" } else { "False" });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesher;
    use crate::Grid;

    fn sphere_mesh() -> Mesh {
        let g = Grid { nx: 40, ny: 40, nz: 40, origin: [-0.4; 3], dx: 0.02 };
        let phi: Vec<f32> = (0..g.len())
            .map(|i| {
                let (x, y, z) = g.coords(i);
                let p = g.world(x, y, z);
                (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - 0.25
            })
            .collect();
        mesher::build(&phi, &g, 0.0, 1, 0.0)
    }

    #[test]
    fn latitude_contour_is_one_closed_curve_fitted_within_tolerance() {
        let m = sphere_mesh();
        let s: Vec<f32> = m.pos.as_chunks::<3>().0.iter().map(|p| p[1]).collect();
        let cfg = CreaseCfg { levels: 1, tolerance: 0.002, min_length: 0.1 };
        let level = 0.0;
        let lines = contour_lines(&m, &s, level);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].1, "equator must be closed");
        let curves = detect(&m, &s, &cfg, true);
        assert_eq!(curves.len(), 1);
        let c = &curves[0];
        assert!(c.closed);
        assert!((c.length - 2.0 * std::f32::consts::PI * 0.25).abs() < 0.03, "length {}", c.length);
        for p in flatten(c, 12) {
            let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((r - 0.25).abs() < 0.01 && p[1].abs() < 0.01, "off the equator: {p:?}");
        }
    }

    #[test]
    fn fewer_segments_than_samples_for_smooth_lines() {
        let line: Vec<V> = (0..200)
            .map(|i| {
                let a = i as f64 / 199.0 * std::f64::consts::PI;
                [a.cos() * 0.3, a.sin() * 0.3, 0.0]
            })
            .collect();
        let segs = fit_line(&line, 0.001);
        assert!(segs.len() <= 6, "{} segments", segs.len());
        let mid = bez_point(&segs[segs.len() / 2], 0.5);
        assert!((norm(mid) - 0.3).abs() < 0.002);
    }

    #[test]
    fn exports_name_every_curve() {
        let m = sphere_mesh();
        let s: Vec<f32> = m.pos.as_chunks::<3>().0.iter().map(|p| p[1]).collect();
        let curves = detect(&m, &s, &CreaseCfg { levels: 2, tolerance: 0.003, min_length: 0.1 }, false);
        assert!(!curves.is_empty());
        let json: serde_json::Value = serde_json::from_str(&to_json(&curves)).unwrap();
        assert_eq!(json["curves"].as_array().unwrap().len(), curves.len());
        let py = to_blender_script(&curves);
        assert_eq!(py.matches("crease('crease_").count(), curves.len());
    }
}
