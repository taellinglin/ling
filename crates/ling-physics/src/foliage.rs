//! Procedural foliage — trees, grass, wind deformation.

use glam::Vec3;

// ── LCG RNG for deterministic generation ─────────────────────────────────────

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0xdeadbeef_cafebabe)
    }

    fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 as u32) as f32 / u32::MAX as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next_f32() * (hi - lo)
    }
}

// ── Tree generation ───────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct TreeParams {
    pub seed: u64,
    pub trunk_height: f32,
    pub trunk_radius: f32,
    pub branch_count: u32, // branches per split
    pub max_depth: u32,
    pub taper: f32,  // radius reduction per level
    pub spread: f32, // angle spread from parent axis (radians)
    pub leaf_size: f32,
    pub wind_response: f32, // 0..1
}

impl Default for TreeParams {
    fn default() -> Self {
        Self {
            seed: 42,
            trunk_height: 4.0,
            trunk_radius: 0.15,
            branch_count: 3,
            max_depth: 4,
            taper: 0.62,
            spread: 0.55,
            leaf_size: 0.6,
            wind_response: 0.3,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TreeMesh {
    /// Trunk + branch vertices (position).
    pub bark_verts: Vec<[f32; 3]>,
    pub bark_normals: Vec<[f32; 3]>,
    pub bark_indices: Vec<u32>,
    /// Leaf quad vertices.
    pub leaf_verts: Vec<[f32; 3]>,
    pub leaf_normals: Vec<[f32; 3]>,
    pub leaf_indices: Vec<u32>,
    /// Per-vertex wind weight (0 = root, 1 = tip).
    pub wind_weights: Vec<f32>,
}

struct Branch {
    start: Vec3,
    end: Vec3,
    radius: f32,
    depth: u32,
    wind_w: f32,
}

fn gen_branches(
    p: &TreeParams,
    start: Vec3,
    dir: Vec3,
    radius: f32,
    depth: u32,
    wind_w: f32,
    rng: &mut Rng,
) -> Vec<Branch> {
    if depth >= p.max_depth {
        return vec![];
    }
    let length = p.trunk_height * 0.6f32.powi(depth as i32);
    let end = start + dir * length;
    let mut out = vec![Branch { start, end, radius, depth, wind_w }];
    if depth + 1 < p.max_depth {
        for i in 0..p.branch_count {
            let base_angle = (i as f32 / p.branch_count as f32) * std::f32::consts::TAU;
            let jitter_az = rng.range(-0.3, 0.3);
            let jitter_el = rng.range(-p.spread * 0.2, p.spread * 0.2);
            let elev = p.spread + jitter_el;
            let azim = base_angle + jitter_az;
            let child_dir = spread_dir(dir, azim, elev);
            let child_r = radius * p.taper;
            let child_w = wind_w + (1.0 - wind_w) * 0.5;
            out.extend(gen_branches(
                p,
                end,
                child_dir,
                child_r,
                depth + 1,
                child_w,
                rng,
            ));
        }
    }
    out
}

fn spread_dir(axis: Vec3, azimuth: f32, elevation: f32) -> Vec3 {
    let right = if axis.abs().dot(Vec3::X) < 0.9 {
        axis.cross(Vec3::X)
    } else {
        axis.cross(Vec3::Z)
    };
    let right = right.normalize_or(Vec3::X);
    let fwd = right.cross(axis).normalize_or(Vec3::Z);
    let local = Vec3::new(
        elevation.sin() * azimuth.cos(),
        elevation.cos(),
        elevation.sin() * azimuth.sin(),
    );
    (axis * local.y + right * local.x + fwd * local.z).normalize_or(Vec3::Y)
}

fn cylinder_mesh(
    start: Vec3,
    end: Vec3,
    r: f32,
    sides: u32,
    wind_w: f32,
    verts: &mut Vec<[f32; 3]>,
    norms: &mut Vec<[f32; 3]>,
    idxs: &mut Vec<u32>,
    weights: &mut Vec<f32>,
) {
    let axis = (end - start).normalize_or(Vec3::Y);
    let right = if axis.abs().dot(Vec3::X) < 0.9 {
        axis.cross(Vec3::X)
    } else {
        axis.cross(Vec3::Z)
    };
    let right = right.normalize_or(Vec3::X);
    let fwd = right.cross(axis).normalize_or(Vec3::Z);
    let base = verts.len() as u32;
    for ring in 0..2u32 {
        let centre = if ring == 0 { start } else { end };
        let ww = if ring == 0 { wind_w * 0.3 } else { wind_w };
        for i in 0..sides {
            let a = i as f32 / sides as f32 * std::f32::consts::TAU;
            let n = right * a.cos() + fwd * a.sin();
            verts.push((centre + n * r).to_array());
            norms.push(n.to_array());
            weights.push(ww);
        }
    }
    for i in 0..sides {
        let a = i;
        let b = (i + 1) % sides;
        let c = a + sides;
        let d = b + sides;
        idxs.extend_from_slice(&[base + a, base + c, base + b, base + b, base + c, base + d]);
    }
}

impl TreeParams {
    pub fn generate(&self) -> TreeMesh {
        let mut rng = Rng::new(self.seed);
        let branches = gen_branches(
            self,
            Vec3::ZERO,
            Vec3::Y,
            self.trunk_radius,
            0,
            0.0,
            &mut rng,
        );
        let mut bv = Vec::new();
        let mut bn = Vec::new();
        let mut bi = Vec::new();
        let mut bw = Vec::new();
        let mut lv = Vec::new();
        let mut ln = Vec::new();
        let mut li = Vec::new();
        let sides = 6u32;
        for b in &branches {
            cylinder_mesh(
                b.start, b.end, b.radius, sides, b.wind_w, &mut bv, &mut bn, &mut bi, &mut bw,
            );
            if b.depth + 1 >= self.max_depth {
                // Add leaf quads at branch tips.
                let s = self.leaf_size;
                let base = lv.len() as u32;
                let n = (b.end - b.start).normalize_or(Vec3::Y);
                let right = if n.abs().dot(Vec3::X) < 0.9 {
                    n.cross(Vec3::X)
                } else {
                    n.cross(Vec3::Z)
                };
                let right = right.normalize_or(Vec3::X) * s * 0.5;
                let up = n * s * 0.5;
                lv.push((b.end - right).to_array());
                lv.push((b.end + right).to_array());
                lv.push((b.end + right + up).to_array());
                lv.push((b.end - right + up).to_array());
                for _ in 0..4 {
                    ln.push(n.to_array());
                }
                li.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
        TreeMesh {
            bark_verts: bv,
            bark_normals: bn,
            bark_indices: bi,
            wind_weights: bw,
            leaf_verts: lv,
            leaf_normals: ln,
            leaf_indices: li,
        }
    }

    /// Deform bark vertices by wind.  Call each frame before uploading to GPU.
    pub fn wind_deform(&self, mesh: &mut TreeMesh, wind_dir: [f32; 2], strength: f32, time: f32) {
        for (i, v) in mesh.bark_verts.iter_mut().enumerate() {
            let w = mesh.wind_weights[i] * self.wind_response;
            let wave = ((time * 2.1 + v[1] * 0.5).sin() * 0.6
                + (time * 3.7 + v[1] * 0.9).sin() * 0.4)
                * strength
                * w;
            v[0] += wind_dir[0] * wave;
            v[2] += wind_dir[1] * wave;
        }
        for v in mesh.leaf_verts.iter_mut() {
            let h = v[1];
            let wave = ((time * 2.5 + h * 0.6).sin()) * strength * self.wind_response;
            v[0] += wind_dir[0] * wave;
            v[2] += wind_dir[1] * wave;
        }
    }
}

// ── Grass patch ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct GrassBlade {
    pub pos: [f32; 3],
    pub height: f32,
    pub tilt: f32,
    pub dir: f32, // azimuth in radians
}

/// Generate a patch of grass blades around `centre` within `radius`.
pub fn gen_grass(centre: Vec3, radius: f32, count: u32, seed: u64) -> Vec<GrassBlade> {
    let mut rng = Rng::new(seed);
    (0..count)
        .map(|_| {
            let a = rng.range(0.0, std::f32::consts::TAU);
            let r = rng.range(0.0, radius);
            GrassBlade {
                pos: [centre.x + a.cos() * r, centre.y, centre.z + a.sin() * r],
                height: rng.range(0.15, 0.5),
                tilt: rng.range(0.0, 0.3),
                dir: rng.range(0.0, std::f32::consts::TAU),
            }
        })
        .collect()
}
