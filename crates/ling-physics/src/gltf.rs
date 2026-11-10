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
    /// Index into `GltfModel.textures` for this mesh's base-color texture, if any.
    pub base_tex: Option<usize>,
}

/// A decoded base-color texture: power-of-two-agnostic RGBA8, row-major, top-left origin.
#[derive(Clone, Debug)]
pub struct GltfImage {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
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
    /// Decoded base-color textures referenced by meshes (base_tex).
    pub textures: Vec<GltfImage>,
    /// Per-mesh hidden flags (index = mesh index; missing/short = visible).
    /// Hidden meshes are skipped by `skin_animated` (empty vertex list), so
    /// equipment variants baked into one GLB cost nothing while unworn.
    pub mesh_hidden: Vec<bool>,
    /// Per-mesh multiplicative 0xRRGGBB tint (missing/short = white); set with
    /// `gltf_mesh_tint`, e.g. to colourize a greyscale hair texture.
    pub mesh_tint: Vec<u32>,
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
    /// Load a .glb or .gltf file from the filesystem (native).
    #[cfg(all(feature = "with-gltf", not(target_arch = "wasm32")))]
    pub fn load(path: &str) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        Self::load_from_bytes(&bytes)
    }

    /// Load a .glb / .gltf from an in-memory buffer (works on wasm too).
    #[cfg(feature = "with-gltf")]
    pub fn load_from_bytes(data: &[u8]) -> Result<Self, String> {
        let (doc, buffers, images) = ::gltf::import_slice(data).map_err(|e| e.to_string())?;
        let mut model = GltfModel::default();

        // ── images: decode each glTF image to RGBA8 (deduped by source index) ──
        let img_rgba = |idx: usize| -> Option<GltfImage> {
            let d = images.get(idx)?;
            let (w, h) = (d.width, d.height);
            let n = (w * h) as usize;
            use ::gltf::image::Format;
            let rgba: Vec<u8> = match d.format {
                Format::R8G8B8A8 => d.pixels.clone(),
                Format::R8G8B8 => {
                    let mut o = Vec::with_capacity(n * 4);
                    for c in d.pixels.chunks_exact(3) {
                        o.extend_from_slice(&[c[0], c[1], c[2], 255]);
                    }
                    o
                },
                Format::R8G8 => {
                    let mut o = Vec::with_capacity(n * 4);
                    for c in d.pixels.chunks_exact(2) {
                        o.extend_from_slice(&[c[0], c[0], c[0], c[1]]);
                    }
                    o
                },
                Format::R8 => {
                    let mut o = Vec::with_capacity(n * 4);
                    for &c in &d.pixels {
                        o.extend_from_slice(&[c, c, c, 255]);
                    }
                    o
                },
                _ => return None, // 16-bit formats: skip (rare for base color)
            };
            Some(GltfImage { w, h, rgba })
        };
        // material index -> index into model.textures (lazily filled below)
        let mut mat_to_tex: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        let mut src_to_tex: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        for mat in doc.materials() {
            let Some(mi) = mat.index() else { continue };
            let Some(info) = mat.pbr_metallic_roughness().base_color_texture() else { continue };
            let src = info.texture().source().index();
            let ti = if let Some(&t) = src_to_tex.get(&src) {
                t
            } else if let Some(img) = img_rgba(src) {
                let t = model.textures.len();
                model.textures.push(img);
                src_to_tex.insert(src, t);
                t
            } else {
                continue;
            };
            mat_to_tex.insert(mi, ti);
        }

        // ── meshes: ONE GltfMesh per primitive (so each keeps its own texture) ──
        for mesh in doc.meshes() {
            let mname = mesh.name().unwrap_or("").to_string();
            for prim in mesh.primitives() {
                let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
                let positions: Vec<[f32; 3]> = match reader.read_positions() {
                    Some(p) => p.collect(),
                    None => continue,
                };
                let normals: Vec<[f32; 3]> = reader.read_normals().map(|n| n.collect()).unwrap_or_default();
                let uvs: Vec<[f32; 2]> = reader.read_tex_coords(0).map(|u| u.into_f32().collect()).unwrap_or_default();
                let joints: Vec<[u16; 4]> = reader.read_joints(0).map(|j| j.into_u16().collect()).unwrap_or_default();
                let weights: Vec<[f32; 4]> = reader.read_weights(0).map(|w| w.into_f32().collect()).unwrap_or_default();
                let mut verts: Vec<GltfVertex> = Vec::with_capacity(positions.len());
                for (i, p) in positions.iter().enumerate() {
                    let n = normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
                    let uv = uvs.get(i).copied().unwrap_or([0.0, 0.0]);
                    verts.push(GltfVertex {
                        pos: Vec3::new(p[0], p[1], p[2]),
                        normal: Vec3::new(n[0], n[1], n[2]),
                        uv: Vec2::new(uv[0], uv[1]),
                        joints: joints.get(i).copied().unwrap_or([0; 4]),
                        weights: weights.get(i).copied().unwrap_or([1.0, 0.0, 0.0, 0.0]),
                    });
                }
                let mut indices: Vec<u32> = Vec::new();
                match reader.read_indices() {
                    Some(idx) => indices.extend(idx.into_u32()),
                    None => indices.extend(0..positions.len() as u32),
                }
                let mat_idx = prim.material().index();
                let base_tex = mat_idx.and_then(|m| mat_to_tex.get(&m).copied());
                model.meshes.push(GltfMesh { name: mname.clone(), verts, indices, mat_idx, base_tex });
            }
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

        // ── skins: joint node indices + inverse-bind matrices ──
        for skin in doc.skins() {
            let reader = skin.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let ibms: Vec<Mat4> = reader
                .read_inverse_bind_matrices()
                .map(|it| it.map(|m| Mat4::from_cols_array_2d(&m)).collect())
                .unwrap_or_default();
            let joints: Vec<GltfJoint> = skin
                .joints()
                .enumerate()
                .map(|(i, node)| GltfJoint {
                    node_idx: node.index(),
                    inverse_bind: ibms.get(i).copied().unwrap_or(Mat4::IDENTITY),
                })
                .collect();
            model.skins.push(GltfSkin { name: skin.name().unwrap_or("").to_string(), joints });
        }

        // ── animations: per-channel keyframe tracks (T/R/S) ──
        for anim in doc.animations() {
            let mut channels: Vec<AnimChannel> = Vec::new();
            let mut duration = 0.0f32;
            for ch in anim.channels() {
                let reader = ch.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
                let times: Vec<f32> = reader.read_inputs().map(|it| it.collect()).unwrap_or_default();
                if let Some(&last) = times.last() {
                    if last > duration { duration = last; }
                }
                let (target, values): (AnimTarget, Vec<Vec4>) = match reader.read_outputs() {
                    Some(::gltf::animation::util::ReadOutputs::Translations(it)) =>
                        (AnimTarget::Translation, it.map(|t| Vec4::new(t[0], t[1], t[2], 0.0)).collect()),
                    Some(::gltf::animation::util::ReadOutputs::Rotations(r)) =>
                        (AnimTarget::Rotation, r.into_f32().map(|q| Vec4::new(q[0], q[1], q[2], q[3])).collect()),
                    Some(::gltf::animation::util::ReadOutputs::Scales(it)) =>
                        (AnimTarget::Scale, it.map(|sc| Vec4::new(sc[0], sc[1], sc[2], 0.0)).collect()),
                    _ => continue,
                };
                let interp = match ch.sampler().interpolation() {
                    ::gltf::animation::Interpolation::Step => Interpolation::Step,
                    ::gltf::animation::Interpolation::CubicSpline => Interpolation::CubicSpline,
                    _ => Interpolation::Linear,
                };
                channels.push(AnimChannel { node_idx: ch.target().node().index(), target, times, values, interp });
            }
            model.animations.push(GltfAnimation { name: anim.name().unwrap_or("").to_string(), channels, duration });
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
    /// Skin every mesh by the real skeleton posed by animation `anim_idx` at
    /// `time` seconds (glTF node TRS + channels + skin inverse-bind). Returns
    /// per-mesh vertex positions in model space (same shape as `skin_local`).
    pub fn skin_animated(&self, anim_idx: usize, time: f32) -> Vec<Vec<[f32; 3]>> {
        let n = self.nodes.len();
        if n == 0 || self.skins.is_empty() {
            return self.bind_positions();
        }
        let world = self.world_mats(anim_idx, time);
        let skin = &self.skins[0];
        let joint_mat: Vec<Mat4> = skin.joints.iter()
            .map(|j| if j.node_idx < n { world[j.node_idx] * j.inverse_bind } else { Mat4::IDENTITY })
            .collect();
        self.skin_with(&joint_mat)
    }

    /// Skin THIS model with `src`'s clip `anim_idx` at `time`, joints matched to
    /// `src` nodes by NAME. Equipment baked on the same skeleton but without
    /// animations follows the body exactly; joints `src` lacks keep bind pose.
    pub fn skin_animated_from(&self, src: &GltfModel, anim_idx: usize, time: f32) -> Vec<Vec<[f32; 3]>> {
        let n = self.nodes.len();
        if n == 0 || self.skins.is_empty() {
            return self.bind_positions();
        }
        let sw = src.world_mats(anim_idx, time);
        let own = self.world_mats(usize::MAX, 0.0);
        let joint_mat: Vec<Mat4> = self.skins[0].joints.iter()
            .map(|j| {
                if j.node_idx >= n { return Mat4::IDENTITY; }
                let nm = &self.nodes[j.node_idx].name;
                let w = if nm.is_empty() { None } else { src.nodes.iter().position(|x| x.name == *nm).map(|k| sw[k]) };
                w.unwrap_or(own[j.node_idx]) * j.inverse_bind
            })
            .collect();
        self.skin_with(&joint_mat)
    }

    /// Clip `anim_idx` at `time` (sampled from `src`'s animations when given,
    /// joints matched by name) with procedural pose layers — see world_mats_posed.
    pub fn skin_animated_posed(&self, src: Option<&GltfModel>, anim_idx: usize, time: f32, pose: &[(String, [f32; 3])]) -> Vec<Vec<[f32; 3]>> {
        let n = self.nodes.len();
        if n == 0 || self.skins.is_empty() {
            return self.bind_positions();
        }
        let joint_mat: Vec<Mat4> = match src {
            None => {
                let world = self.world_mats_posed(anim_idx, time, pose);
                self.skins[0].joints.iter()
                    .map(|j| if j.node_idx < n { world[j.node_idx] * j.inverse_bind } else { Mat4::IDENTITY })
                    .collect()
            }
            Some(sm) => {
                let sw = sm.world_mats_posed(anim_idx, time, pose);
                let own = self.world_mats(usize::MAX, 0.0);
                self.skins[0].joints.iter()
                    .map(|j| {
                        if j.node_idx >= n { return Mat4::IDENTITY; }
                        let nm = &self.nodes[j.node_idx].name;
                        let w = if nm.is_empty() { None } else { sm.nodes.iter().position(|x| x.name == *nm).map(|k| sw[k]) };
                        w.unwrap_or(own[j.node_idx]) * j.inverse_bind
                    })
                    .collect()
            }
        };
        self.skin_with(&joint_mat)
    }

    fn bind_positions(&self) -> Vec<Vec<[f32; 3]>> {
        self.meshes.iter().enumerate().map(|(mi, m)| {
            if self.mesh_hidden.get(mi).copied().unwrap_or(false) { return Vec::new(); }
            m.verts.iter().map(|v| [v.pos.x, v.pos.y, v.pos.z]).collect()
        }).collect()
    }

    /// World matrix of every node posed by clip `anim_idx` at `time`.
    fn world_mats(&self, anim_idx: usize, time: f32) -> Vec<Mat4> {
        self.world_mats_posed(anim_idx, time, &[])
    }

    /// `world_mats` plus procedural pose layers on top of the sampled clip:
    /// each `(bone name, euler XYZ radians)` post-multiplies that node's local
    /// rotation; the special name `*root*` rotates the whole skeleton in model
    /// space about the pivot given by a `*pivot*` entry (x, y, z; default origin).
    fn world_mats_posed(&self, anim_idx: usize, time: f32, pose: &[(String, [f32; 3])]) -> Vec<Mat4> {
        let n = self.nodes.len();
        // base TRS per node, then override with the animation's channels at `time`
        let mut trans = vec![Vec3::ZERO; n];
        let mut rot = vec![Quat::IDENTITY; n];
        let mut scale = vec![Vec3::ONE; n];
        for (i, node) in self.nodes.iter().enumerate() {
            let (s, r, t) = node.transform.to_scale_rotation_translation();
            scale[i] = s; rot[i] = r; trans[i] = t;
        }
        if let Some(anim) = self.animations.get(anim_idx) {
            for ch in &anim.channels {
                if ch.node_idx >= n { continue; }
                let v = ch.sample(time);
                match ch.target {
                    AnimTarget::Translation => trans[ch.node_idx] = Vec3::new(v.x, v.y, v.z),
                    AnimTarget::Rotation => rot[ch.node_idx] = Quat::from_xyzw(v.x, v.y, v.z, v.w).normalize(),
                    AnimTarget::Scale => scale[ch.node_idx] = Vec3::new(v.x, v.y, v.z),
                    AnimTarget::Weights => {},
                }
            }
        }
        let mut root_m = Mat4::IDENTITY;
        if !pose.is_empty() {
            let mut pivot = Vec3::ZERO;
            for (name, e) in pose {
                if name == "*pivot*" {
                    pivot = Vec3::new(e[0], e[1], e[2]);
                }
            }
            for (name, e) in pose {
                let q = Quat::from_euler(glam::EulerRot::XYZ, e[0], e[1], e[2]);
                if name == "*root*" {
                    root_m = Mat4::from_translation(pivot) * Mat4::from_quat(q) * Mat4::from_translation(-pivot);
                } else if name != "*pivot*" {
                    if let Some(i) = self.nodes.iter().position(|nd| nd.name == *name) {
                        rot[i] = (rot[i] * q).normalize();
                    }
                }
            }
        }
        let local: Vec<Mat4> = (0..n).map(|i| Mat4::from_scale_rotation_translation(scale[i], rot[i], trans[i])).collect();
        // world transforms via the node hierarchy (iterative; children lists give the tree)
        let mut parent = vec![usize::MAX; n];
        for (pi, node) in self.nodes.iter().enumerate() {
            for &c in &node.children {
                if c < n { parent[c] = pi; }
            }
        }
        let mut world = vec![Mat4::IDENTITY; n];
        let mut done = vec![false; n];
        // resolve in dependency order (parent before child); bounded passes
        for _ in 0..n {
            let mut progressed = false;
            for i in 0..n {
                if done[i] { continue; }
                let p = parent[i];
                if p == usize::MAX {
                    world[i] = root_m * local[i]; done[i] = true; progressed = true;
                } else if done[p] {
                    world[i] = world[p] * local[i]; done[i] = true; progressed = true;
                }
            }
            if !progressed { break; }
        }
        world
    }

    fn skin_with(&self, joint_mat: &[Mat4]) -> Vec<Vec<[f32; 3]>> {
        // skin each (visible) mesh
        self.meshes.iter().enumerate().map(|(mi, mesh)| {
            if self.mesh_hidden.get(mi).copied().unwrap_or(false) {
                return Vec::new();
            }
            mesh.verts.iter().map(|v| {
                let p = Vec4::new(v.pos.x, v.pos.y, v.pos.z, 1.0);
                let mut acc = Vec4::ZERO;
                let mut wsum = 0.0f32;
                for k in 0..4 {
                    let w = v.weights[k];
                    if w <= 0.0 { continue; }
                    let ji = v.joints[k] as usize;
                    if ji < joint_mat.len() {
                        acc += joint_mat[ji] * p * w;
                        wsum += w;
                    }
                }
                if wsum > 1e-4 { let o = acc / wsum; [o.x, o.y, o.z] } else { [v.pos.x, v.pos.y, v.pos.z] }
            }).collect()
        }).collect()
    }

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
        let mesh = GltfMesh { name: "Cube".into(), verts, indices, mat_idx: None, base_tex: None };
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
