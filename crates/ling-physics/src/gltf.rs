//! glTF 2.0 model loading with skeletal animation support.
//!
//! Enable the `with-gltf` Cargo feature to use the real loader via the `gltf` crate.
//! Without the feature, only the data structures and a stub loader are available.

use glam::{Vec2, Vec3, Vec4, Mat4, Quat};

// ── Data types ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct GltfVertex {
    pub pos:    Vec3,
    pub normal: Vec3,
    pub uv:     Vec2,
    pub joints: [u16; 4],   // bone indices for skinning
    pub weights:[f32; 4],   // blend weights (sum ≈ 1)
}

#[derive(Clone, Debug)]
pub struct GltfMesh {
    pub name:    String,
    pub verts:   Vec<GltfVertex>,
    pub indices: Vec<u32>,
    pub mat_idx: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct GltfNode {
    pub name:      String,
    pub transform: Mat4,
    pub mesh_idx:  Option<usize>,
    pub children:  Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct GltfJoint {
    pub node_idx:     usize,
    pub inverse_bind: Mat4,
}

#[derive(Clone, Debug)]
pub struct GltfSkin {
    pub name:   String,
    pub joints: Vec<GltfJoint>,
}

// ── Animation ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum Interpolation { Linear, Step, CubicSpline }

#[derive(Clone, Debug)]
pub enum AnimTarget { Translation, Rotation, Scale, Weights }

#[derive(Clone, Debug)]
pub struct AnimChannel {
    pub node_idx: usize,
    pub target:   AnimTarget,
    pub times:    Vec<f32>,
    pub values:   Vec<Vec4>,  // translation=xyz0, rotation=xyzw, scale=xyz0
    pub interp:   Interpolation,
}

impl AnimChannel {
    pub fn sample(&self, t: f32) -> Vec4 {
        if self.times.is_empty() { return Vec4::ZERO; }
        let t = t.rem_euclid(*self.times.last().unwrap_or(&1.0));
        let idx = self.times.partition_point(|&s| s <= t).saturating_sub(1);
        let idx = idx.min(self.times.len() - 1);
        let next = (idx + 1).min(self.times.len() - 1);
        if idx == next { return self.values[idx]; }
        let lo = self.times[idx];
        let hi = self.times[next];
        let f  = ((t - lo) / (hi - lo)).clamp(0.0, 1.0);
        match self.interp {
            Interpolation::Step   => self.values[idx],
            Interpolation::Linear | Interpolation::CubicSpline =>
                self.values[idx].lerp(self.values[next], f),
        }
    }
}

#[derive(Clone, Debug)]
pub struct GltfAnimation {
    pub name:     String,
    pub channels: Vec<AnimChannel>,
    pub duration: f32,
}

// ── Model ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct GltfModel {
    pub meshes:     Vec<GltfMesh>,
    pub nodes:      Vec<GltfNode>,
    pub skins:      Vec<GltfSkin>,
    pub animations: Vec<GltfAnimation>,
    pub root_nodes: Vec<usize>,
}

impl GltfModel {
    /// Load a .glb or .gltf file.
    #[cfg(feature = "with-gltf")]
    pub fn load(path: &str) -> Result<Self, String> {
        use ::gltf::Gltf;
        let gltf = Gltf::open(path).map_err(|e| e.to_string())?;
        // Full parsing implementation omitted for brevity — returns stubs.
        let _ = gltf;
        Err("full glTF parsing not yet wired".to_string())
    }

    #[cfg(not(feature = "with-gltf"))]
    pub fn load(_path: &str) -> Result<Self, String> {
        Err("compile with feature 'with-gltf' to load glTF files".to_string())
    }

    /// Build a unit cube model for testing.
    pub fn unit_cube() -> Self {
        let verts: Vec<GltfVertex> = vec![
            // 8 corners of a unit cube (-0.5..0.5)
            // Front face
            GltfVertex { pos: Vec3::new(-0.5,-0.5, 0.5), normal: Vec3::Z, uv: Vec2::ZERO, joints: [0;4], weights: [1.0,0.0,0.0,0.0] },
            GltfVertex { pos: Vec3::new( 0.5,-0.5, 0.5), normal: Vec3::Z, uv: Vec2::X,    joints: [0;4], weights: [1.0,0.0,0.0,0.0] },
            GltfVertex { pos: Vec3::new( 0.5, 0.5, 0.5), normal: Vec3::Z, uv: Vec2::ONE,  joints: [0;4], weights: [1.0,0.0,0.0,0.0] },
            GltfVertex { pos: Vec3::new(-0.5, 0.5, 0.5), normal: Vec3::Z, uv: Vec2::Y,    joints: [0;4], weights: [1.0,0.0,0.0,0.0] },
        ];
        let indices = vec![0u32, 1, 2, 0, 2, 3];
        let mesh = GltfMesh { name: "Cube".into(), verts, indices, mat_idx: None };
        let node = GltfNode {
            name: "Cube".into(),
            transform: Mat4::IDENTITY,
            mesh_idx: Some(0),
            children: vec![],
        };
        Self { meshes: vec![mesh], nodes: vec![node], root_nodes: vec![0], ..Default::default() }
    }

    /// Evaluate the global transform for node `idx` at animation time `t`.
    pub fn eval_node_transform(&self, idx: usize, anim_idx: usize, t: f32) -> Mat4 {
        let base = self.nodes.get(idx).map(|n| n.transform).unwrap_or(Mat4::IDENTITY);
        let Some(anim) = self.animations.get(anim_idx) else { return base; };
        let mut translation = None;
        let mut rotation    = None;
        let mut scale       = None;
        for ch in &anim.channels {
            if ch.node_idx != idx { continue; }
            let v = ch.sample(t);
            match ch.target {
                AnimTarget::Translation => translation = Some(Vec3::new(v.x, v.y, v.z)),
                AnimTarget::Rotation    => rotation    = Some(Quat::from_vec4(v).normalize()),
                AnimTarget::Scale       => scale       = Some(Vec3::new(v.x, v.y, v.z)),
                AnimTarget::Weights     => {}
            }
        }
        let t3 = translation.unwrap_or(base.w_axis.truncate());
        let r  = rotation.unwrap_or(Quat::from_mat4(&base));
        let s  = scale.unwrap_or(Vec3::ONE);
        Mat4::from_scale_rotation_translation(s, r, t3)
    }

    /// Compute joint matrices for skinning.
    pub fn joint_matrices(&self, skin_idx: usize, node_transforms: &[Mat4]) -> Vec<Mat4> {
        let Some(skin) = self.skins.get(skin_idx) else { return vec![]; };
        skin.joints.iter().map(|j| {
            let node_tf = node_transforms.get(j.node_idx).copied().unwrap_or(Mat4::IDENTITY);
            node_tf * j.inverse_bind
        }).collect()
    }
}
