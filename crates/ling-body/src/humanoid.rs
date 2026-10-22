//! Parametric T-pose humanoid: tapered capsules blended with a smooth union,
//! plus the joint hierarchy used for source placement and rig export.
//! Y is up, the body faces +Z, the character's left side is +X, feet rest on y = 0.

use crate::grid::Grid;
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct Capsule {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub ra: f32,
    pub rb: f32,
    /// Anisotropic stretch about the capsule centre.
    pub scale: [f32; 3],
}

#[derive(Clone, Debug)]
pub struct Joint {
    pub name: String,
    pub parent: Option<usize>,
    pub pos: [f32; 3],
}

#[derive(Clone, Debug)]
pub struct Humanoid {
    pub height: f32,
    pub capsules: Vec<Capsule>,
    pub joints: Vec<Joint>,
    blend: f32,
}

const SIDES: [(&str, f32); 2] = [("l", 1.0), ("r", -1.0)];

impl Humanoid {
    pub fn new(height: f32, build: f32, arm_span: f32, head_scale: f32) -> Humanoid {
        let h = height;
        let b = build;
        let mut caps: Vec<Capsule> = Vec::new();
        let mut joints: Vec<Joint> = Vec::new();
        let p = |x: f32, y: f32, z: f32| [x * h, y * h, z * h];
        let mut cap = |a: [f32; 3], bb: [f32; 3], ra: f32, rb: f32, s: [f32; 3]| {
            caps.push(Capsule { a, b: bb, ra: ra * h, rb: rb * h, scale: s })
        };
        let mut joint = |name: &str, parent: Option<usize>, pos: [f32; 3]| -> usize {
            joints.push(Joint { name: name.to_string(), parent, pos });
            joints.len() - 1
        };

        let hips = joint("hips", None, p(0.0, 0.53, 0.0));
        let spine = joint("spine", Some(hips), p(0.0, 0.62, 0.0));
        let chest = joint("chest", Some(spine), p(0.0, 0.72, 0.0));
        let neck = joint("neck", Some(chest), p(0.0, 0.82, 0.0));
        let head = joint("head", Some(neck), p(0.0, 0.90, 0.0));

        cap(p(0.0, 0.54, 0.0), p(0.0, 0.76, 0.0), 0.082 * b, 0.088 * b, [1.0, 1.0, 0.72]);
        cap(p(0.0, 0.76, 0.0), p(0.0, 0.80, 0.0), 0.085 * b, 0.05 * b, [1.0, 1.0, 0.72]);
        cap(p(0.0, 0.80, 0.0), p(0.0, 0.855, 0.0), 0.033 * b, 0.031 * b, [1.0; 3]);
        let hr = 0.062 * head_scale;
        cap(p(0.0, 0.905, 0.0), p(0.0, 0.925, 0.0), hr, hr, [1.0, 1.1, 1.0]);
        cap(p(-0.055, 0.52, 0.0), p(0.055, 0.52, 0.0), 0.075 * b, 0.075 * b, [1.0, 1.0, 0.8]);
        cap(p(-0.115, 0.795, 0.0), p(0.115, 0.795, 0.0), 0.04 * b, 0.04 * b, [1.0; 3]);

        for (tag, s) in SIDES {
            let sh = p(0.115 * s, 0.80, 0.0);
            let el = p((0.115 + 0.165 * arm_span) * s, 0.80, 0.0);
            let wr = p((0.115 + 0.31 * arm_span) * s, 0.80, 0.0);
            let tip = p((0.115 + 0.395 * arm_span) * s, 0.80, 0.0);
            let hip = p(0.055 * s, 0.52, 0.0);
            let kn = p(0.058 * s, 0.285, 0.004);
            let an = p(0.060 * s, 0.045, 0.0);
            let toe = p(0.062 * s, 0.02, 0.11);

            let jsh = joint(&format!("{tag}_shoulder"), Some(chest), sh);
            let jel = joint(&format!("{tag}_elbow"), Some(jsh), el);
            let jwr = joint(&format!("{tag}_wrist"), Some(jel), wr);
            joint(&format!("{tag}_hand"), Some(jwr), tip);
            let jhp = joint(&format!("{tag}_hip"), Some(hips), hip);
            let jkn = joint(&format!("{tag}_knee"), Some(jhp), kn);
            let jan = joint(&format!("{tag}_ankle"), Some(jkn), an);
            joint(&format!("{tag}_toe"), Some(jan), toe);

            cap(sh, el, 0.038 * b, 0.030 * b, [1.0; 3]);
            cap(el, wr, 0.030 * b, 0.022 * b, [1.0; 3]);
            cap(wr, tip, 0.022 * b, 0.012 * b, [1.0, 0.5, 1.0]);
            cap(hip, kn, 0.062 * b, 0.042 * b, [1.0; 3]);
            cap(kn, an, 0.042 * b, 0.028 * b, [1.0; 3]);
            cap(an, toe, 0.030 * b, 0.022 * b, [1.0; 3]);
        }
        let _ = head;
        Humanoid { height, capsules: caps, joints, blend: 0.03 * h }
    }

    /// Scaffold whose height and arm span match a measured bounding box.
    pub fn fit(lo: [f32; 3], hi: [f32; 3]) -> Humanoid {
        let h = (hi[1] - lo[1]).max(0.1);
        let span = (hi[0] - lo[0]) / h;
        let mut s = Humanoid::new(h, 1.0, (span / 1.02).clamp(0.6, 1.4), 1.0);
        s.translate([(lo[0] + hi[0]) * 0.5, lo[1], (lo[2] + hi[2]) * 0.5]);
        s
    }

    pub fn translate(&mut self, d: [f32; 3]) {
        for c in &mut self.capsules {
            for (k, &off) in d.iter().enumerate() {
                c.a[k] += off;
                c.b[k] += off;
            }
        }
        for j in &mut self.joints {
            for (p, &off) in j.pos.iter_mut().zip(&d) {
                *p += off;
            }
        }
    }

    pub fn joint_index(&self, name: &str) -> Option<usize> {
        self.joints.iter().position(|j| j.name == name)
    }

    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for c in &self.capsules {
            let r = c.ra.max(c.rb);
            for k in 0..3 {
                let e = r * c.scale[k].max(1.0);
                lo[k] = lo[k].min(c.a[k].min(c.b[k]) - e);
                hi[k] = hi[k].max(c.a[k].max(c.b[k]) + e);
            }
        }
        (lo, hi)
    }

    fn capsule_dist(c: &Capsule, p: [f32; 3]) -> f32 {
        let m = [(c.a[0] + c.b[0]) * 0.5, (c.a[1] + c.b[1]) * 0.5, (c.a[2] + c.b[2]) * 0.5];
        let q = [
            m[0] + (p[0] - m[0]) / c.scale[0],
            m[1] + (p[1] - m[1]) / c.scale[1],
            m[2] + (p[2] - m[2]) / c.scale[2],
        ];
        let ab = [c.b[0] - c.a[0], c.b[1] - c.a[1], c.b[2] - c.a[2]];
        let ap = [q[0] - c.a[0], q[1] - c.a[1], q[2] - c.a[2]];
        let l2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
        let t = if l2 > 1e-12 { ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / l2).clamp(0.0, 1.0) } else { 0.0 };
        let d = [ap[0] - ab[0] * t, ap[1] - ab[1] * t, ap[2] - ab[2] * t];
        let r = c.ra + (c.rb - c.ra) * t;
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() - r;
        len * c.scale[0].min(c.scale[1]).min(c.scale[2])
    }

    /// Signed distance, negative inside; capsules fused with a polynomial smooth minimum.
    pub fn sdf(&self, p: [f32; 3]) -> f32 {
        let k = self.blend.max(1e-6);
        let mut d = f32::INFINITY;
        for c in &self.capsules {
            let e = Self::capsule_dist(c, p);
            d = if d.is_infinite() {
                e
            } else {
                let h = (0.5 + 0.5 * (e - d) / k).clamp(0.0, 1.0);
                e * (1.0 - h) + d * h - k * h * (1.0 - h)
            };
        }
        d
    }

    pub fn fill(&self, grid: &Grid) -> Vec<f32> {
        let mut out = vec![0.0f32; grid.len()];
        out.par_chunks_mut(grid.nx * grid.ny).enumerate().for_each(|(z, slab)| {
            for y in 0..grid.ny {
                for x in 0..grid.nx {
                    slab[x + grid.nx * y] = self.sdf(grid.world(x, y, z));
                }
            }
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_pose_proportions() {
        let h = Humanoid::new(1.8, 1.0, 1.0, 1.0);
        let (lo, hi) = h.bounds();
        assert!(lo[1] > -0.1 && lo[1] < 0.0);
        assert!(hi[1] > 1.6 && hi[1] < 1.95);
        assert!(hi[0] > 0.7 && lo[0] < -0.7);
        let chest = h.joints[h.joint_index("chest").unwrap()].pos;
        assert!(h.sdf(chest) < 0.0);
        assert!(h.sdf([2.0, 1.0, 0.0]) > 0.5);
    }

    #[test]
    fn fit_matches_box() {
        let h = Humanoid::fit([-0.6, 0.0, -0.1], [0.6, 1.5, 0.1]);
        let (lo, hi) = h.bounds();
        assert!((hi[1] - lo[1] - 1.5).abs() < 0.2);
        assert!(h.sdf([0.0, 0.8, 0.0]) < 0.0);
    }
}
