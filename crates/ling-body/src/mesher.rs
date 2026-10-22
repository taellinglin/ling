//! Zero level set extraction by marching tetrahedra, followed by Taubin smoothing and
//! vertex-cluster decimation. Marching tetrahedra needs no case tables and, unlike
//! marching cubes, has no ambiguous cases, so thin features cannot tear the surface.

use crate::grid::Grid;
use crate::mesh::Mesh;
use std::collections::HashMap;

const CORNER: [[usize; 3]; 8] =
    [[0, 0, 0], [1, 0, 0], [0, 1, 0], [1, 1, 0], [0, 0, 1], [1, 0, 1], [0, 1, 1], [1, 1, 1]];

/// Freudenthal split of a cube into six tetrahedra sharing the 0-7 diagonal. The split is
/// the same in every cell, so neighbouring cells agree on their shared faces.
const TETS: [[usize; 4]; 6] = [[0, 1, 3, 7], [0, 1, 5, 7], [0, 2, 3, 7], [0, 2, 6, 7], [0, 4, 5, 7], [0, 4, 6, 7]];

struct Welder<'a> {
    grid: &'a Grid,
    phi: &'a [f32],
    iso: f32,
    pos: Vec<f32>,
    cache: HashMap<(u32, u32), u32>,
}

impl Welder<'_> {
    fn value(&self, i: usize) -> f32 {
        let v = self.phi[i] - self.iso;
        if v.abs() < 1e-7 {
            1e-7
        } else {
            v
        }
    }

    /// Vertex on the lattice edge `a`-`b`, shared by every tetrahedron touching it.
    fn on_edge(&mut self, a: usize, b: usize) -> u32 {
        let key = (a.min(b) as u32, a.max(b) as u32);
        if let Some(&v) = self.cache.get(&key) {
            return v;
        }
        let (va, vb) = (self.value(a), self.value(b));
        let t = va / (va - vb);
        let (ax, ay, az) = self.grid.coords(a);
        let (bx, by, bz) = self.grid.coords(b);
        let pa = self.grid.world(ax, ay, az);
        let pb = self.grid.world(bx, by, bz);
        let id = (self.pos.len() / 3) as u32;
        for k in 0..3 {
            self.pos.push(pa[k] + (pb[k] - pa[k]) * t);
        }
        self.cache.insert(key, id);
        id
    }

    fn point(&self, i: usize) -> [f32; 3] {
        let (x, y, z) = self.grid.coords(i);
        self.grid.world(x, y, z)
    }
}

fn centroid(w: &Welder, ids: &[usize; 8], set: &[usize]) -> [f32; 3] {
    let mut c = [0.0f32; 3];
    for &k in set {
        let p = w.point(ids[k]);
        for a in 0..3 {
            c[a] += p[a] / set.len() as f32;
        }
    }
    c
}

fn emit(w: &Welder, idx: &mut Vec<u32>, tri: [u32; 3], outward: [f32; 3]) {
    let p = |i: u32| {
        let i = i as usize * 3;
        [w.pos[i], w.pos[i + 1], w.pos[i + 2]]
    };
    let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let dot = n[0] * outward[0] + n[1] * outward[1] + n[2] * outward[2];
    if dot >= 0.0 {
        idx.extend_from_slice(&tri);
    } else {
        idx.extend_from_slice(&[tri[0], tri[2], tri[1]]);
    }
}

/// Extracts `phi == iso` by marching tetrahedra: one welded vertex per crossed lattice
/// edge, outward-facing triangles, and a closed manifold wherever the surface stays
/// inside the grid, whatever the feature size.
pub fn marching_tets(phi: &[f32], g: &Grid, iso: f32) -> Mesh {
    let mut w = Welder { grid: g, phi, iso, pos: Vec::new(), cache: HashMap::new() };
    let mut idx: Vec<u32> = Vec::new();
    for z in 0..g.nz - 1 {
        for y in 0..g.ny - 1 {
            for x in 0..g.nx - 1 {
                let mut ids = [0usize; 8];
                let mut v = [0.0f32; 8];
                let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
                for (k, c) in CORNER.iter().enumerate() {
                    ids[k] = g.idx(x + c[0], y + c[1], z + c[2]);
                    v[k] = w.value(ids[k]);
                    lo = lo.min(v[k]);
                    hi = hi.max(v[k]);
                }
                if lo >= 0.0 || hi < 0.0 {
                    continue;
                }
                for tet in TETS {
                    let inside: Vec<usize> = tet.iter().copied().filter(|&k| v[k] < 0.0).collect();
                    let outside: Vec<usize> = tet.iter().copied().filter(|&k| v[k] >= 0.0).collect();
                    match (inside.len(), outside.len()) {
                        (1, 3) | (3, 1) => {
                            let (lone, rest) = if inside.len() == 1 { (inside[0], &outside) } else { (outside[0], &inside) };
                            let t = [
                                w.on_edge(ids[lone], ids[rest[0]]),
                                w.on_edge(ids[lone], ids[rest[1]]),
                                w.on_edge(ids[lone], ids[rest[2]]),
                            ];
                            let (ci, co) = if inside.len() == 1 {
                                (w.point(ids[lone]), centroid(&w, &ids, rest))
                            } else {
                                (centroid(&w, &ids, rest), w.point(ids[lone]))
                            };
                            emit(&w, &mut idx, t, [co[0] - ci[0], co[1] - ci[1], co[2] - ci[2]]);
                        },
                        (2, 2) => {
                            let (i, j, k, l) = (inside[0], inside[1], outside[0], outside[1]);
                            let q = [
                                w.on_edge(ids[i], ids[k]),
                                w.on_edge(ids[i], ids[l]),
                                w.on_edge(ids[j], ids[l]),
                                w.on_edge(ids[j], ids[k]),
                            ];
                            let (ci, co) = (centroid(&w, &ids, &inside), centroid(&w, &ids, &outside));
                            let dir = [co[0] - ci[0], co[1] - ci[1], co[2] - ci[2]];
                            emit(&w, &mut idx, [q[0], q[1], q[2]], dir);
                            emit(&w, &mut idx, [q[0], q[2], q[3]], dir);
                        },
                        _ => {},
                    }
                }
            }
        }
    }
    Mesh { pos: w.pos, nrm: Vec::new(), uv: Vec::new(), idx }
}

pub(crate) fn adjacency(vert_count: usize, idx: &[u32]) -> (Vec<usize>, Vec<u32>) {
    let mut pairs: Vec<(u32, u32)> = Vec::with_capacity(idx.len() * 2);
    for t in idx.as_chunks::<3>().0 {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            pairs.push((a, b));
            pairs.push((b, a));
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    let mut start = vec![0usize; vert_count + 1];
    for &(a, _) in &pairs {
        start[a as usize + 1] += 1;
    }
    for i in 0..vert_count {
        start[i + 1] += start[i];
    }
    (start, pairs.into_iter().map(|p| p.1).collect())
}

/// Taubin λ|μ smoothing: removes surface noise without shrinking the body.
pub fn smooth(mesh: &mut Mesh, iterations: usize) {
    if iterations == 0 || mesh.idx.is_empty() {
        return;
    }
    let n = mesh.vert_count();
    let (start, nbr) = adjacency(n, &mesh.idx);
    let (lambda, mu) = (0.5f32, -0.53f32);
    let mut tmp = vec![0.0f32; mesh.pos.len()];
    for _ in 0..iterations {
        for w in [lambda, mu] {
            for v in 0..n {
                let (a, b) = (start[v], start[v + 1]);
                if a == b {
                    tmp[v * 3..v * 3 + 3].copy_from_slice(&mesh.pos[v * 3..v * 3 + 3]);
                    continue;
                }
                let mut avg = [0.0f32; 3];
                for &u in &nbr[a..b] {
                    for (k, a) in avg.iter_mut().enumerate() {
                        *a += mesh.pos[u as usize * 3 + k];
                    }
                }
                let inv = 1.0 / (b - a) as f32;
                for k in 0..3 {
                    let p = mesh.pos[v * 3 + k];
                    tmp[v * 3 + k] = p + w * (avg[k] * inv - p);
                }
            }
            std::mem::swap(&mut mesh.pos, &mut tmp);
        }
    }
}

/// Merges vertices that fall in the same `cell`-sized bucket and drops collapsed triangles.
pub fn decimate(mesh: &Mesh, cell: f32) -> Mesh {
    if cell <= 0.0 || mesh.idx.is_empty() {
        return mesh.clone();
    }
    let mut bucket: HashMap<(i32, i32, i32), u32> = HashMap::new();
    let mut sums: Vec<[f32; 4]> = Vec::new();
    let mut remap = vec![0u32; mesh.vert_count()];
    for (v, p) in mesh.pos.as_chunks::<3>().0.iter().enumerate() {
        let key = ((p[0] / cell).floor() as i32, (p[1] / cell).floor() as i32, (p[2] / cell).floor() as i32);
        let id = *bucket.entry(key).or_insert_with(|| {
            sums.push([0.0; 4]);
            (sums.len() - 1) as u32
        });
        let s = &mut sums[id as usize];
        s[0] += p[0];
        s[1] += p[1];
        s[2] += p[2];
        s[3] += 1.0;
        remap[v] = id;
    }
    let pos: Vec<f32> = sums.iter().flat_map(|s| [s[0] / s[3], s[1] / s[3], s[2] / s[3]]).collect();
    let mut seen = std::collections::HashSet::new();
    let mut idx = Vec::new();
    for t in mesh.idx.as_chunks::<3>().0 {
        let (a, b, c) = (remap[t[0] as usize], remap[t[1] as usize], remap[t[2] as usize]);
        if a == b || b == c || a == c {
            continue;
        }
        let mut key = [a, b, c];
        key.sort_unstable();
        if seen.insert(key) {
            idx.extend_from_slice(&[a, b, c]);
        }
    }
    Mesh { pos, nrm: Vec::new(), uv: Vec::new(), idx }
}

/// Full post-process: extraction, smoothing, optional decimation, normals and UVs.
pub fn build(phi: &[f32], g: &Grid, iso: f32, smooth_iters: usize, decimate_cell: f32) -> Mesh {
    let mut m = marching_tets(phi, g, iso);
    smooth(&mut m, smooth_iters);
    let mut m = decimate(&m, decimate_cell);
    m.compute_normals();
    m.compute_uvs();
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sphere(grid: &Grid, r: f32) -> Vec<f32> {
        (0..grid.len())
            .map(|i| {
                let (x, y, z) = grid.coords(i);
                let p = grid.world(x, y, z);
                (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - r
            })
            .collect()
    }

    #[test]
    fn sphere_is_watertight_and_round() {
        let g = Grid { nx: 40, ny: 40, nz: 40, origin: [-0.4; 3], dx: 0.02 };
        let m = build(&sphere(&g, 0.25), &g, 0.0, 2, 0.0);
        assert!(m.tri_count() > 500);
        assert!(m.is_watertight(), "{} bad edges", m.non_manifold_edges());
        for p in m.pos.as_chunks::<3>().0 {
            let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((r - 0.25).abs() < 0.012, "radius {r}");
        }
        let n = &m.nrm;
        let p = &m.pos;
        let dot: f32 = (0..3).map(|k| n[k] * p[k]).sum();
        assert!(dot > 0.0, "normals must point outward");
    }

    #[test]
    fn decimation_reduces_triangles() {
        let g = Grid { nx: 40, ny: 40, nz: 40, origin: [-0.4; 3], dx: 0.02 };
        let m = marching_tets(&sphere(&g, 0.25), &g, 0.0);
        let d = decimate(&m, 0.05);
        assert!(d.tri_count() < m.tri_count() / 2);
    }
}
