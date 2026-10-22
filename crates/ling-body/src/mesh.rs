//! Triangle containers: raw triangle soup (input) and indexed meshes (output).

#[derive(Clone, Debug, Default)]
pub struct Tris {
    /// Nine floats per triangle.
    pub v: Vec<f32>,
}

impl Tris {
    pub fn len(&self) -> usize {
        self.v.len() / 9
    }

    pub fn is_empty(&self) -> bool {
        self.v.is_empty()
    }

    pub fn push(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3]) {
        self.v.extend_from_slice(&a);
        self.v.extend_from_slice(&b);
        self.v.extend_from_slice(&c);
    }

    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        bounds_of(&self.v)
    }

    pub fn extend(&mut self, other: &Tris) {
        self.v.extend_from_slice(&other.v);
    }

    /// Uniform scale then translation applied to every vertex.
    pub fn transform(&mut self, scale: f32, offset: [f32; 3]) {
        for p in self.v.as_chunks_mut::<3>().0 {
            for a in 0..3 {
                p[a] = p[a] * scale + offset[a];
            }
        }
    }
}

pub(crate) fn bounds_of(flat: &[f32]) -> Option<([f32; 3], [f32; 3])> {
    if flat.is_empty() {
        return None;
    }
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in flat.as_chunks::<3>().0 {
        for a in 0..3 {
            lo[a] = lo[a].min(p[a]);
            hi[a] = hi[a].max(p[a]);
        }
    }
    Some((lo, hi))
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub pos: Vec<f32>,
    pub nrm: Vec<f32>,
    pub uv: Vec<f32>,
    pub idx: Vec<u32>,
}

impl Mesh {
    pub fn vert_count(&self) -> usize {
        self.pos.len() / 3
    }

    pub fn tri_count(&self) -> usize {
        self.idx.len() / 3
    }

    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        bounds_of(&self.pos)
    }

    /// Area-weighted vertex normals.
    pub fn compute_normals(&mut self) {
        let mut n = vec![0.0f32; self.pos.len()];
        for t in self.idx.as_chunks::<3>().0 {
            let p = |i: u32| {
                let i = i as usize * 3;
                [self.pos[i], self.pos[i + 1], self.pos[i + 2]]
            };
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let f = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            for &i in t {
                for k in 0..3 {
                    n[i as usize * 3 + k] += f[k];
                }
            }
        }
        for c in n.as_chunks_mut::<3>().0 {
            let l = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
            if l > 1e-20 {
                c.iter_mut().for_each(|x| *x /= l);
            } else {
                c.copy_from_slice(&[0.0, 1.0, 0.0]);
            }
        }
        self.nrm = n;
    }

    /// Planar front projection normalised to the bounding box.
    pub fn compute_uvs(&mut self) {
        let Some((lo, hi)) = self.bounds() else { return };
        let sx = (hi[0] - lo[0]).max(1e-6);
        let sy = (hi[1] - lo[1]).max(1e-6);
        self.uv =
            self.pos.as_chunks::<3>().0.iter().flat_map(|p| [(p[0] - lo[0]) / sx, (p[1] - lo[1]) / sy]).collect();
    }

    /// Number of edges not shared by exactly two triangles.
    pub fn non_manifold_edges(&self) -> usize {
        let mut e: Vec<(u32, u32)> = Vec::with_capacity(self.idx.len());
        for t in self.idx.as_chunks::<3>().0 {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                e.push((a.min(b), a.max(b)));
            }
        }
        e.sort_unstable();
        let mut bad = 0;
        let mut i = 0;
        while i < e.len() {
            let mut j = i;
            while j < e.len() && e[j] == e[i] {
                j += 1;
            }
            if j - i != 2 {
                bad += 1;
            }
            i = j;
        }
        bad
    }

    pub fn is_watertight(&self) -> bool {
        !self.idx.is_empty() && self.non_manifold_edges() == 0
    }
}
