//! glTF 2.0 model loading with skeletal animation support.
//!
//! Enable the `with-gltf` Cargo feature to use the real loader via the `gltf` crate.
//! Without the feature, only the data structures and a stub loader are available.

use glam::{Mat4, Quat, Vec2, Vec3, Vec4};

// ── Data types ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct GltfVertex {
    pub pos: Vec3,
    pub normal: Vec3,
    pub uv: Vec2,
    pub joints: [u16; 4],  // bone indices for skinning
    pub weights: [f32; 4], // blend weights (sum ≈ 1)
}

#[derive(Clone, Debug)]
pub struct GltfMesh {
    pub name: String,
    pub verts: Vec<GltfVertex>,
    pub indices: Vec<u32>,
    pub mat_idx: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct GltfNode {
    pub name: String,
    pub transform: Mat4,
    pub mesh_idx: Option<usize>,
    pub children: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct GltfJoint {
    pub node_idx: usize,
    pub inverse_bind: Mat4,
}

#[derive(Clone, Debug)]
pub struct GltfSkin {
    pub name: String,
    pub joints: Vec<GltfJoint>,
}

// ── Animation ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum Interpolation {
    Linear,
    Step,
    CubicSpline,
}

#[derive(Clone, Debug)]
pub enum AnimTarget {
    Translation,
    Rotation,
    Scale,
    Weights,
}

#[derive(Clone, Debug)]
pub struct AnimChannel {
    pub node_idx: usize,
    pub target: AnimTarget,
    pub times: Vec<f32>,
    pub values: Vec<Vec4>, // translation=xyz0, rotation=xyzw, scale=xyz0
    pub interp: Interpolation,
}

impl AnimChannel {
    pub fn sample(&self, t: f32) -> Vec4 {
        if self.times.is_empty() {
            return Vec4::ZERO;
        }
        let t = t.rem_euclid(*self.times.last().unwrap_or(&1.0));
        let idx = self.times.partition_point(|&s| s <= t).saturating_sub(1);
        let idx = idx.min(self.times.len() - 1);
        let next = (idx + 1).min(self.times.len() - 1);
        if idx == next {
            return self.values[idx];
        }
        let lo = self.times[idx];
        let hi = self.times[next];
        let f = ((t - lo) / (hi - lo)).clamp(0.0, 1.0);
        match self.interp {
            Interpolation::Step => self.values[idx],
            Interpolation::Linear | Interpolation::CubicSpline => {
                self.values[idx].lerp(self.values[next], f)
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct GltfAnimation {
    pub name: String,
    pub channels: Vec<AnimChannel>,
    pub duration: f32,
}

// ── Model ─────────────────────────────────────────────────────────────────────

/// A procedurally-generated bone for auto-rigging a mesh that shipped with no
/// skeleton. `head`/`tail` are rest positions in mesh-local space; `parent` is
/// an index into the bone list (or -1 for the root).
#[derive(Clone, Debug)]
pub struct SimpleBone {
    pub head: Vec3,
    pub tail: Vec3,
    pub parent: i32,
}

#[derive(Clone, Debug, Default)]
pub struct GltfModel {
    pub meshes: Vec<GltfMesh>,
    pub nodes: Vec<GltfNode>,
    pub skins: Vec<GltfSkin>,
    pub animations: Vec<GltfAnimation>,
    pub root_nodes: Vec<usize>,
    /// Procedural humanoid skeleton (empty until `autorig()` is called).
    pub bones: Vec<SimpleBone>,
}

/// Shortest distance from point `p` to segment `a`→`b`.
fn point_seg_dist(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let l2 = ab.length_squared();
    let t = if l2 > 1e-9 {
        ((p - a).dot(ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + ab * t)).length()
}

impl GltfModel {
    /// Load a .glb or .gltf file.
    #[cfg(feature = "with-gltf")]
    pub fn load(path: &str) -> Result<Self, String> {
        let (doc, buffers, _images) = ::gltf::import(path).map_err(|e| e.to_string())?;
        let mut model = GltfModel::default();

        // ── meshes: positions / normals / uvs / indices (per primitive, concatenated) ──
        for mesh in doc.meshes() {
            let mut verts: Vec<GltfVertex> = Vec::new();
            let mut indices: Vec<u32> = Vec::new();
            let mut mat_idx: Option<usize> = None;
            for prim in mesh.primitives() {
                let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
                let positions: Vec<[f32; 3]> = match reader.read_positions() {
                    Some(p) => p.collect(),
                    None => continue,
                };
                let base = verts.len() as u32;
                let normals: Vec<[f32; 3]> = reader
                    .read_normals()
                    .map(|n| n.collect())
                    .unwrap_or_default();
                let uvs: Vec<[f32; 2]> = reader
                    .read_tex_coords(0)
                    .map(|u| u.into_f32().collect())
                    .unwrap_or_default();
                for (i, p) in positions.iter().enumerate() {
                    let n = normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
                    let uv = uvs.get(i).copied().unwrap_or([0.0, 0.0]);
                    verts.push(GltfVertex {
                        pos: Vec3::new(p[0], p[1], p[2]),
                        normal: Vec3::new(n[0], n[1], n[2]),
                        uv: Vec2::new(uv[0], uv[1]),
                        joints: [0; 4],
                        weights: [1.0, 0.0, 0.0, 0.0],
                    });
                }
                match reader.read_indices() {
                    Some(idx) => {
                        for i in idx.into_u32() {
                            indices.push(base + i);
                        }
                    },
                    None => {
                        for i in 0..positions.len() as u32 {
                            indices.push(base + i);
                        }
                    },
                }
                if mat_idx.is_none() {
                    mat_idx = prim.material().index();
                }
            }
            model.meshes.push(GltfMesh {
                name: mesh.name().unwrap_or("").to_string(),
                verts,
                indices,
                mat_idx,
            });
        }

        // ── node hierarchy (name, transform, mesh ref, children) ──
        for node in doc.nodes() {
            let transform = Mat4::from_cols_array_2d(&node.transform().matrix());
            model.nodes.push(GltfNode {
                name: node.name().unwrap_or("").to_string(),
                transform,
                mesh_idx: node.mesh().map(|m| m.index()),
                children: node.children().map(|c| c.index()).collect(),
            });
        }
        if let Some(scene) = doc.default_scene().or_else(|| doc.scenes().next()) {
            for n in scene.nodes() {
                model.root_nodes.push(n.index());
            }
        }

        Ok(model)
    }

    #[cfg(not(feature = "with-gltf"))]
    pub fn load(_path: &str) -> Result<Self, String> {
        Err("compile with feature 'with-gltf' to load glTF files".to_string())
    }

    /// Procedurally rig a mesh that has no skeleton: build a 12-bone humanoid
    /// skeleton from the model's bounding box and weight-paint every vertex to
    /// its nearest two bones (envelope skinning). Returns the bone count.
    pub fn autorig(&mut self) -> usize {
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for m in &self.meshes {
            for v in &m.verts {
                lo = lo.min(v.pos);
                hi = hi.max(v.pos);
            }
        }
        if !lo.is_finite() || !hi.is_finite() {
            return 0;
        }
        let h = (hi.y - lo.y).max(1e-3);
        let cx = (lo.x + hi.x) * 0.5;
        let cz = (lo.z + hi.z) * 0.5;
        let aw = (hi.x - lo.x).max(1e-3) * 0.5; // half-width, for arm/leg spread
        let y = |f: f32| lo.y + f * h;
        let bone = |hx: f32, hy: f32, tx: f32, ty: f32, p: i32| SimpleBone {
            head: Vec3::new(hx, hy, cz),
            tail: Vec3::new(tx, ty, cz),
            parent: p,
        };
        self.bones = vec![
            bone(cx, y(0.50), cx, y(0.62), -1), // 0 hips
            bone(cx, y(0.62), cx, y(0.74), 0),  // 1 spine
            bone(cx, y(0.74), cx, y(0.84), 1),  // 2 chest
            bone(cx, y(0.86), cx, y(1.00), 2),  // 3 head
            bone(cx + aw * 0.28, y(0.80), cx + aw * 0.60, y(0.78), 2), // 4 L upper arm
            bone(cx + aw * 0.60, y(0.78), cx + aw * 0.95, y(0.70), 4), // 5 L forearm
            bone(cx - aw * 0.28, y(0.80), cx - aw * 0.60, y(0.78), 2), // 6 R upper arm
            bone(cx - aw * 0.60, y(0.78), cx - aw * 0.95, y(0.70), 6), // 7 R forearm
            bone(cx + aw * 0.18, y(0.50), cx + aw * 0.18, y(0.26), 0), // 8 L thigh
            bone(cx + aw * 0.18, y(0.26), cx + aw * 0.18, y(0.02), 8), // 9 L shin
            bone(cx - aw * 0.18, y(0.50), cx - aw * 0.18, y(0.26), 0), // 10 R thigh
            bone(cx - aw * 0.18, y(0.26), cx - aw * 0.18, y(0.02), 10), // 11 R shin
        ];
        // weight each vertex to its nearest two bones (inverse-square falloff)
        for m in &mut self.meshes {
            for v in &mut m.verts {
                let mut best0 = (f32::INFINITY, 0usize);
                let mut best1 = (f32::INFINITY, 0usize);
                for (i, b) in self.bones.iter().enumerate() {
                    let d = point_seg_dist(v.pos, b.head, b.tail);
                    if d < best0.0 {
                        best1 = best0;
                        best0 = (d, i);
                    } else if d < best1.0 {
                        best1 = (d, i);
                    }
                }
                let w0 = 1.0 / (best0.0 * best0.0 + 1e-4);
                let w1 = 1.0 / (best1.0 * best1.0 + 1e-4);
                let s = w0 + w1;
                v.joints = [best0.1 as u16, best1.1 as u16, 0, 0];
                v.weights = [w0 / s, w1 / s, 0.0, 0.0];
            }
        }
        self.bones.len()
    }

    /// Forward-kinematics: turn a per-bone local rotation pose (flat XYZ-euler
    /// radians, 3 per bone) into per-bone linear-blend skinning matrices.
    pub fn skinning_mats(&self, euler: &[f32]) -> Vec<Mat4> {
        let n = self.bones.len();
        let mut world = vec![Mat4::IDENTITY; n];
        for i in 0..n {
            let b = &self.bones[i];
            let ex = euler.get(i * 3).copied().unwrap_or(0.0);
            let ey = euler.get(i * 3 + 1).copied().unwrap_or(0.0);
            let ez = euler.get(i * 3 + 2).copied().unwrap_or(0.0);
            let r = Quat::from_euler(glam::EulerRot::XYZ, ex, ey, ez);
            let parent_head = if b.parent < 0 {
                Vec3::ZERO
            } else {
                self.bones[b.parent as usize].head
            };
            let local = Mat4::from_translation(b.head - parent_head) * Mat4::from_quat(r);
            world[i] = if b.parent < 0 {
                local
            } else {
                world[b.parent as usize] * local
            };
        }
        // skinning = posed_world * rest_world⁻¹, and rest_world = translate(head)
        for (i, w) in world.iter_mut().enumerate() {
            *w *= Mat4::from_translation(-self.bones[i].head);
        }
        world
    }

    /// Skin every mesh with the given pose; returns per-mesh, per-vertex
    /// deformed positions in mesh-local space. Falls back to the rest positions
    /// when the model has not been auto-rigged.
    pub fn skin_local(&self, euler: &[f32]) -> Vec<Vec<[f32; 3]>> {
        if self.bones.is_empty() {
            return self
                .meshes
                .iter()
                .map(|m| {
                    m.verts
                        .iter()
                        .map(|v| [v.pos.x, v.pos.y, v.pos.z])
                        .collect()
                })
                .collect();
        }
        let mats = self.skinning_mats(euler);
        self.meshes
            .iter()
            .map(|m| {
                m.verts
                    .iter()
                    .map(|v| {
                        let p = v.pos.extend(1.0);
                        let j0 = v.joints[0] as usize;
                        let j1 = v.joints[1] as usize;
                        let sp = (mats[j0] * p).truncate() * v.weights[0]
                            + (mats[j1] * p).truncate() * v.weights[1];
                        [sp.x, sp.y, sp.z]
                    })
                    .collect()
            })
            .collect()
    }

    /// Build a unit cube model for testing.
    pub fn unit_cube() -> Self {
        let verts: Vec<GltfVertex> = vec![
            // 8 corners of a unit cube (-0.5..0.5)
            // Front face
            GltfVertex {
                pos: Vec3::new(-0.5, -0.5, 0.5),
                normal: Vec3::Z,
                uv: Vec2::ZERO,
                joints: [0; 4],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            GltfVertex {
                pos: Vec3::new(0.5, -0.5, 0.5),
                normal: Vec3::Z,
                uv: Vec2::X,
                joints: [0; 4],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            GltfVertex {
                pos: Vec3::new(0.5, 0.5, 0.5),
                normal: Vec3::Z,
                uv: Vec2::ONE,
                joints: [0; 4],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            GltfVertex {
                pos: Vec3::new(-0.5, 0.5, 0.5),
                normal: Vec3::Z,
                uv: Vec2::Y,
                joints: [0; 4],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
        ];
        let indices = vec![0u32, 1, 2, 0, 2, 3];
        let mesh = GltfMesh { name: "Cube".into(), verts, indices, mat_idx: None };
        let node = GltfNode {
            name: "Cube".into(),
            transform: Mat4::IDENTITY,
            mesh_idx: Some(0),
            children: vec![],
        };
        Self {
            meshes: vec![mesh],
            nodes: vec![node],
            root_nodes: vec![0],
            ..Default::default()
        }
    }

    /// Evaluate the global transform for node `idx` at animation time `t`.
    pub fn eval_node_transform(&self, idx: usize, anim_idx: usize, t: f32) -> Mat4 {
        let base = self
            .nodes
            .get(idx)
            .map(|n| n.transform)
            .unwrap_or(Mat4::IDENTITY);
        let Some(anim) = self.animations.get(anim_idx) else {
            return base;
        };
        let mut translation = None;
        let mut rotation = None;
        let mut scale = None;
        for ch in &anim.channels {
            if ch.node_idx != idx {
                continue;
            }
            let v = ch.sample(t);
            match ch.target {
                AnimTarget::Translation => translation = Some(Vec3::new(v.x, v.y, v.z)),
                AnimTarget::Rotation => rotation = Some(Quat::from_vec4(v).normalize()),
                AnimTarget::Scale => scale = Some(Vec3::new(v.x, v.y, v.z)),
                AnimTarget::Weights => {},
            }
        }
        let t3 = translation.unwrap_or(base.w_axis.truncate());
        let r = rotation.unwrap_or(Quat::from_mat4(&base));
        let s = scale.unwrap_or(Vec3::ONE);
        Mat4::from_scale_rotation_translation(s, r, t3)
    }

    /// Compute joint matrices for skinning.
    pub fn joint_matrices(&self, skin_idx: usize, node_transforms: &[Mat4]) -> Vec<Mat4> {
        let Some(skin) = self.skins.get(skin_idx) else {
            return vec![];
        };
        skin.joints
            .iter()
            .map(|j| {
                let node_tf = node_transforms
                    .get(j.node_idx)
                    .copied()
                    .unwrap_or(Mat4::IDENTITY);
                node_tf * j.inverse_bind
            })
            .collect()
    }
}
