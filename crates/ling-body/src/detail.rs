//! Detail synthesis. The mesh is subdivided, the new vertices are snapped back onto
//! the smooth level-set surface, and the detail the grid could not hold is added as
//! displacement along the normal: grooves with raised lips along the crease curves,
//! plus fractal noise whose strength follows the local acoustic field.

use crate::curves::{self, Curve};
use crate::grid::Grid;
use crate::mesh::Mesh;
use rayon::prelude::*;
use std::collections::HashMap;

pub struct DetailCfg {
    pub levels: usize,
    /// Groove depth, metres.
    pub depth: f32,
    /// Groove half width, metres.
    pub width: f32,
    /// Noise amplitude, metres.
    pub noise_amp: f32,
    /// Noise base frequency, cycles per metre.
    pub noise_freq: f32,
}

const MAX_TRIANGLES: usize = 1_600_000;

fn gradient(phi: &[f32], g: &Grid, p: [f32; 3]) -> ([f32; 3], f32) {
    let h = g.dx * 0.5;
    let s = |q: [f32; 3]| g.sample(phi, g.lattice(q));
    let d = [
        s([p[0] + h, p[1], p[2]]) - s([p[0] - h, p[1], p[2]]),
        s([p[0], p[1] + h, p[2]]) - s([p[0], p[1] - h, p[2]]),
        s([p[0], p[1], p[2] + h]) - s([p[0], p[1], p[2] - h]),
    ];
    ([d[0] / (2.0 * h), d[1] / (2.0 * h), d[2] / (2.0 * h)], s(p))
}

/// Moves `p` onto the zero level set along the gradient.
fn snap(phi: &[f32], g: &Grid, mut p: [f32; 3]) -> [f32; 3] {
    for _ in 0..3 {
        let (gr, v) = gradient(phi, g, p);
        let n2 = gr[0] * gr[0] + gr[1] * gr[1] + gr[2] * gr[2];
        if n2 < 1e-6 || v.abs() > 4.0 * g.dx {
            break;
        }
        for k in 0..3 {
            p[k] -= v * gr[k] / n2;
        }
    }
    p
}

/// One midpoint subdivision pass. `scalar` is interpolated to the new vertices.
fn subdivide(m: &Mesh, scalar: &[f32], phi: &[f32], g: &Grid) -> (Mesh, Vec<f32>) {
    let mut pos = m.pos.clone();
    let mut sc = scalar.to_vec();
    let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
    let mut idx = Vec::with_capacity(m.idx.len() * 4);
    for t in m.idx.as_chunks::<3>().0 {
        let mut m3 = [0u32; 3];
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let key = (a.min(b), a.max(b));
            m3[k] = *mid.entry(key).or_insert_with(|| {
                let (pa, pb) = (a as usize * 3, b as usize * 3);
                let p = [
                    (m.pos[pa] + m.pos[pb]) * 0.5,
                    (m.pos[pa + 1] + m.pos[pb + 1]) * 0.5,
                    (m.pos[pa + 2] + m.pos[pb + 2]) * 0.5,
                ];
                pos.extend_from_slice(&snap(phi, g, p));
                sc.push((scalar[a as usize] + scalar[b as usize]) * 0.5);
                (pos.len() / 3 - 1) as u32
            });
        }
        idx.extend_from_slice(&[t[0], m3[0], m3[2], m3[0], t[1], m3[1], m3[2], m3[1], t[2], m3[0], m3[1], m3[2]]);
    }
    (Mesh { pos, nrm: Vec::new(), uv: Vec::new(), idx }, sc)
}

fn hash3(x: i32, y: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA6B) ^ (z as u32).wrapping_mul(0xC2B2_AE35);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// Smooth value noise in [-1, 1].
fn value_noise(p: [f32; 3]) -> f32 {
    let (fx, fy, fz) = (p[0].floor(), p[1].floor(), p[2].floor());
    let (ix, iy, iz) = (fx as i32, fy as i32, fz as i32);
    let t = |v: f32| v * v * (3.0 - 2.0 * v);
    let (tx, ty, tz) = (t(p[0] - fx), t(p[1] - fy), t(p[2] - fz));
    let l = |a: f32, b: f32, w: f32| a + (b - a) * w;
    let c = |dx, dy, dz| hash3(ix + dx, iy + dy, iz + dz);
    let x00 = l(c(0, 0, 0), c(1, 0, 0), tx);
    let x10 = l(c(0, 1, 0), c(1, 1, 0), tx);
    let x01 = l(c(0, 0, 1), c(1, 0, 1), tx);
    let x11 = l(c(0, 1, 1), c(1, 1, 1), tx);
    l(l(x00, x10, ty), l(x01, x11, ty), tz)
}

fn fbm(p: [f32; 3], octaves: usize) -> f32 {
    let (mut sum, mut amp, mut norm, mut f) = (0.0, 1.0, 0.0, 1.0);
    for o in 0..octaves {
        let q = [p[0] * f + 17.0 * o as f32, p[1] * f, p[2] * f];
        sum += amp * value_noise(q);
        norm += amp;
        amp *= 0.5;
        f *= 2.0;
    }
    sum / norm
}

/// Nearest-crease lookup over densely sampled curve points.
type Cells = HashMap<(i32, i32, i32), Vec<([f32; 3], f32)>>;

struct CreaseField {
    cell: f32,
    cells: Cells,
}

impl CreaseField {
    fn new(curves: &[Curve], cell: f32) -> CreaseField {
        let mut cells = Cells::new();
        let step = cell / 3.0;
        for c in curves {
            let per_seg = ((c.length / c.segs.len().max(1) as f32) / step).ceil().clamp(2.0, 400.0) as usize;
            for p in curves::flatten(c, per_seg) {
                cells.entry(Self::key(p, cell)).or_default().push((p, c.weight));
            }
        }
        CreaseField { cell, cells }
    }

    fn key(p: [f32; 3], cell: f32) -> (i32, i32, i32) {
        ((p[0] / cell).floor() as i32, (p[1] / cell).floor() as i32, (p[2] / cell).floor() as i32)
    }

    /// Distance to the nearest crease sample within one cell, and that crease's weight.
    fn nearest(&self, p: [f32; 3]) -> Option<(f32, f32)> {
        let (cx, cy, cz) = Self::key(p, self.cell);
        let mut best: Option<(f32, f32)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(list) = self.cells.get(&(cx + dx, cy + dy, cz + dz)) {
                        for (q, w) in list {
                            let d = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt();
                            if best.is_none_or(|b| d < b.0) {
                                best = Some((d, *w));
                            }
                        }
                    }
                }
            }
        }
        best
    }
}

/// Groove profile: a Gaussian trench flanked by soft raised lips, in units of depth.
fn groove(d: f32, w: f32) -> f32 {
    let trench = (-(d / w).powi(2)).exp();
    let lip = (-((d - 2.2 * w) / (0.9 * w)).powi(2)).exp();
    trench - 0.35 * lip
}

pub fn build(base: &Mesh, scalar: &[f32], phi: &[f32], grid: &Grid, creases: &[Curve], cfg: &DetailCfg) -> Mesh {
    let mut mesh = base.clone();
    let mut sc = scalar.to_vec();
    for _ in 0..cfg.levels {
        if mesh.tri_count() * 4 > MAX_TRIANGLES {
            break;
        }
        (mesh, sc) = subdivide(&mesh, &sc, phi, grid);
    }
    mesh.compute_normals();
    let field = CreaseField::new(creases, (cfg.width * 3.0).max(1e-4));
    let offsets: Vec<f32> = mesh
        .pos
        .par_chunks_exact(3)
        .zip(sc.par_iter())
        .map(|(p, &s)| {
            let p = [p[0], p[1], p[2]];
            let mut d = 0.0;
            if let Some((dist, weight)) = field.nearest(p) {
                if dist < cfg.width * 3.5 {
                    d -= cfg.depth * weight * groove(dist, cfg.width);
                }
            }
            let strength = (s.abs() / (1.0 + s.abs())) * 2.0;
            let f = cfg.noise_freq;
            d + cfg.noise_amp * strength.min(1.0) * fbm([p[0] * f, p[1] * f, p[2] * f], 3)
        })
        .collect();
    for ((p, n), o) in mesh.pos.as_chunks_mut::<3>().0.iter_mut().zip(mesh.nrm.as_chunks::<3>().0).zip(&offsets) {
        for k in 0..3 {
            p[k] += n[k] * o;
        }
    }
    mesh.compute_normals();
    mesh.compute_uvs();
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curves::{detect, CreaseCfg};
    use crate::mesher;

    fn sphere() -> (Mesh, Vec<f32>, Grid) {
        let g = Grid { nx: 40, ny: 40, nz: 40, origin: [-0.4; 3], dx: 0.02 };
        let phi: Vec<f32> = (0..g.len())
            .map(|i| {
                let (x, y, z) = g.coords(i);
                let p = g.world(x, y, z);
                (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - 0.25
            })
            .collect();
        (mesher::build(&phi, &g, 0.0, 1, 0.0), phi, g)
    }

    #[test]
    fn subdivision_keeps_the_surface_watertight_and_on_the_sphere() {
        let (m, phi, g) = sphere();
        let scalar = vec![0.0; m.vert_count()];
        let (s, sc) = subdivide(&m, &scalar, &phi, &g);
        assert_eq!(s.tri_count(), m.tri_count() * 4);
        assert_eq!(sc.len(), s.vert_count());
        assert!(s.is_watertight());
        for p in s.pos.as_chunks::<3>().0 {
            let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((r - 0.25).abs() < 0.004, "radius {r}");
        }
    }

    #[test]
    fn grooves_cut_inward_along_the_crease_and_stay_manifold() {
        let (m, phi, g) = sphere();
        let height: Vec<f32> = m.pos.as_chunks::<3>().0.iter().map(|p| p[1]).collect();
        let creases = detect(&m, &height, &CreaseCfg { levels: 1, tolerance: 0.002, min_length: 0.1 }, true);
        assert_eq!(creases.len(), 1);
        let cfg = DetailCfg { levels: 1, depth: 0.004, width: 0.006, noise_amp: 0.0, noise_freq: 50.0 };
        let d = build(&m, &height, &phi, &g, &creases, &cfg);
        assert!(d.is_watertight());
        let radius = |p: &[f32]| (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        let on_equator: Vec<f32> = d.pos.as_chunks::<3>().0.iter().filter(|p| p[1].abs() < 0.002).map(|p| radius(p)).collect();
        let off: Vec<f32> = d.pos.as_chunks::<3>().0.iter().filter(|p| p[1].abs() > 0.1 && p[1].abs() < 0.12).map(|p| radius(p)).collect();
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        assert!(mean(&on_equator) < mean(&off) - 0.002, "equator {} vs elsewhere {}", mean(&on_equator), mean(&off));
    }

    #[test]
    fn noise_is_bounded_and_deterministic() {
        let a = fbm([1.3, 2.7, -0.4], 3);
        assert_eq!(a, fbm([1.3, 2.7, -0.4], 3));
        assert!(a.abs() <= 1.0);
    }
}
