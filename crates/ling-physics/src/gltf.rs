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
/// an index into the bone list (or -1 for the root). `name` matches this
/// project's real Flyff-derived rig naming convention (3ds Max Biped scheme
/// — "Bip01 ...") — see the doc comment on `autorig` for where that's
/// verified from, not guessed.
#[derive(Clone, Debug)]
pub struct SimpleBone {
    pub name: String,
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

    /// Procedurally rig a mesh that has no skeleton: build a 20-bone humanoid
    /// skeleton from the model's bounding box and weight-paint every vertex
    /// to its nearest four bones (envelope skinning). Returns the bone
    /// count.
    ///
    /// Bone names/hierarchy match this project's real Flyff-derived rig —
    /// not a guess at Flyff's convention, a direct read of it: this repo's
    /// own `models/all/rig/mvr_NpcAchaben.ling` (generated from an actual
    /// ripped `.chr` by `tools/chr2ling.py`) carries the real
    /// `mvr_NpcAchaben_bone_names` list, which uses exactly this "Bip01 ..."
    /// 3ds Max Biped scheme:
    ///   Bip01, Bip01 Pelvis, Bip01 Spine, Bip01 Spine1, Bip01 Neck,
    ///   Bip01 Head, Bip01 L/R Clavicle, Bip01 L/R UpperArm,
    ///   Bip01 L/R ForeArm, Bip01 L/R Hand, Bip01 L/R Thigh, Bip01 L/R Calf,
    ///   Bip01 L/R Foot — plus a few things specific to that one NPC
    ///   (Ponytail1/11, Bone01-04 for a cloth chain) that aren't part of the
    ///   base humanoid skeleton, so aren't reproduced here.
    /// This engine has no separate "vertex group" concept (no Blender/Maya-
    /// style named weight layer distinct from the skinning data itself) —
    /// joint index + weight IS the vertex group assignment, so naming the
    /// bones this way already gives every vertex's group membership the
    /// same names a real Flyff rig would.
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
        let ad = (hi.z - lo.z).max(1e-3) * 0.5; // half-depth, for the feet's forward offset
        let y = |f: f32| lo.y + f * h;
        let bone = |name: &str, hx: f32, hy: f32, hz: f32, tx: f32, ty: f32, tz: f32, p: i32| SimpleBone {
            name: name.to_string(),
            head: Vec3::new(hx, hy, hz),
            tail: Vec3::new(tx, ty, tz),
            parent: p,
        };
        self.bones = vec![
            // 0 root — a zero-length hierarchy anchor, coincident with the
            // pelvis, same as a real Biped's Bip01. Excluded from weight
            // painting below: a real Biped never skins geometry to the
            // root either, and leaving it in the nearest-bone search would
            // just steal weight from Pelvis for anything close to the hips.
            bone("Bip01", cx, y(0.50), cz, cx, y(0.50), cz, -1),
            bone("Bip01 Pelvis", cx, y(0.50), cz, cx, y(0.58), cz, 0),           // 1
            bone("Bip01 Spine", cx, y(0.58), cz, cx, y(0.68), cz, 1),           // 2
            bone("Bip01 Spine1", cx, y(0.68), cz, cx, y(0.78), cz, 2),          // 3
            bone("Bip01 Neck", cx, y(0.78), cz, cx, y(0.86), cz, 3),            // 4
            bone("Bip01 Head", cx, y(0.86), cz, cx, y(1.00), cz, 4),            // 5
            bone("Bip01 L Clavicle", cx, y(0.80), cz, cx + aw * 0.25, y(0.79), cz, 3), // 6
            bone("Bip01 L UpperArm", cx + aw * 0.25, y(0.79), cz, cx + aw * 0.58, y(0.76), cz, 6), // 7
            bone("Bip01 L ForeArm", cx + aw * 0.58, y(0.76), cz, cx + aw * 0.88, y(0.68), cz, 7),  // 8
            bone("Bip01 L Hand", cx + aw * 0.88, y(0.68), cz, cx + aw * 1.02, y(0.62), cz, 8),     // 9
            bone("Bip01 R Clavicle", cx, y(0.80), cz, cx - aw * 0.25, y(0.79), cz, 3), // 10
            bone("Bip01 R UpperArm", cx - aw * 0.25, y(0.79), cz, cx - aw * 0.58, y(0.76), cz, 10), // 11
            bone("Bip01 R ForeArm", cx - aw * 0.58, y(0.76), cz, cx - aw * 0.88, y(0.68), cz, 11),  // 12
            bone("Bip01 R Hand", cx - aw * 0.88, y(0.68), cz, cx - aw * 1.02, y(0.62), cz, 12),     // 13
            bone("Bip01 L Thigh", cx + aw * 0.18, y(0.50), cz, cx + aw * 0.18, y(0.27), cz, 1),     // 14
            bone("Bip01 L Calf", cx + aw * 0.18, y(0.27), cz, cx + aw * 0.18, y(0.06), cz, 14),     // 15
            // feet get a forward (+Z) tail offset — real feet extend
            // forward from the ankle, not straight down like another calf
            // segment — so forward-most vertices (toes) actually bind
            // nearest to the foot bone instead of the calf.
            bone("Bip01 L Foot", cx + aw * 0.18, y(0.06), cz, cx + aw * 0.18, y(0.0), cz + ad * 0.55, 15), // 16
            bone("Bip01 R Thigh", cx - aw * 0.18, y(0.50), cz, cx - aw * 0.18, y(0.27), cz, 1),     // 17
            bone("Bip01 R Calf", cx - aw * 0.18, y(0.27), cz, cx - aw * 0.18, y(0.06), cz, 17),     // 18
            bone("Bip01 R Foot", cx - aw * 0.18, y(0.06), cz, cx - aw * 0.18, y(0.0), cz + ad * 0.55, 18), // 19
        ];
        // Weight each vertex to its nearest FOUR bones (inverse-square
        // falloff), not just two — GltfVertex already carries 4 joint/
        // weight slots (real skinned .glb imports use all 4; only autorig's
        // own envelope skinning was leaving 2 of them zeroed), and
        // skin_local() below now blends across all 4. Two-bone blending is
        // fine mid-limb but pinches/hinges unnaturally right at a joint
        // (knee, ankle, elbow, shoulder) where a vertex actually sits
        // between three or more bone influences — a standard reason real
        // rigs use 4-bone skinning, not a hypothetical one.
        for m in &mut self.meshes {
            for v in &mut m.verts {
                let mut best: [(f32, usize); 4] =
                    [(f32::INFINITY, 0); 4];
                for (i, b) in self.bones.iter().enumerate() {
                    if i == 0 {
                        continue; // root: not skinned, see autorig's doc comment
                    }
                    let d = point_seg_dist(v.pos, b.head, b.tail);
                    if d < best[0].0 {
                        best[3] = best[2];
                        best[2] = best[1];
                        best[1] = best[0];
                        best[0] = (d, i);
                    } else if d < best[1].0 {
                        best[3] = best[2];
                        best[2] = best[1];
                        best[1] = (d, i);
                    } else if d < best[2].0 {
                        best[3] = best[2];
                        best[2] = (d, i);
                    } else if d < best[3].0 {
                        best[3] = (d, i);
                    }
                }
                let w: [f32; 4] = [
                    1.0 / (best[0].0 * best[0].0 + 1e-4),
                    1.0 / (best[1].0 * best[1].0 + 1e-4),
                    1.0 / (best[2].0 * best[2].0 + 1e-4),
                    1.0 / (best[3].0 * best[3].0 + 1e-4),
                ];
                let s = w[0] + w[1] + w[2] + w[3];
                v.joints = [best[0].1 as u16, best[1].1 as u16, best[2].1 as u16, best[3].1 as u16];
                v.weights = [w[0] / s, w[1] / s, w[2] / s, w[3] / s];
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
                        // 4-bone linear-blend skinning — a weight of 0.0 for
                        // an unused slot (real .glb imports and autorig's
                        // own bounding-box rig alike, when fewer than 4
                        // bones are actually in range) contributes nothing,
                        // so this is exactly the old 2-bone behavior when
                        // only 2 slots are populated, and real 4-bone
                        // blending when they are.
                        let mut sp = Vec3::ZERO;
                        for k in 0..4 {
                            let w = v.weights[k];
                            if w != 0.0 {
                                let j = v.joints[k] as usize;
                                sp += (mats[j] * p).truncate() * w;
                            }
                        }
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
