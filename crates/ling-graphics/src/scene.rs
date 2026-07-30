use crate::geometry::Mesh;
use crate::material::Material;
use glam::{Mat4, Quat, Vec3};
use std::sync::Arc;

// ── Transform ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub fn from_translation(t: Vec3) -> Self {
        Self { translation: t, ..Self::IDENTITY }
    }

    pub fn from_rotation(r: Quat) -> Self {
        Self { rotation: r, ..Self::IDENTITY }
    }

    pub fn from_scale(s: Vec3) -> Self {
        Self { scale: s, ..Self::IDENTITY }
    }

    pub fn from_trs(translation: Vec3, rotation: Quat, scale: Vec3) -> Self {
        Self { translation, rotation, scale }
    }

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    pub fn global_matrix(&self, parent: &Mat4) -> Mat4 {
        *parent * self.matrix()
    }

    pub fn forward(&self) -> Vec3 {
        self.rotation * -Vec3::Z
    }

    pub fn right(&self) -> Vec3 {
        self.rotation * Vec3::X
    }

    pub fn up(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }

    pub fn look_at(&mut self, target: Vec3, world_up: Vec3) {
        let dir = (target - self.translation).normalize();
        if dir.length_squared() < 1e-8 {
            return;
        }
        let mat = glam::camera::rh::view::look_at_mat4(self.translation, target, world_up);
        let (_, rot, _) = mat.inverse().to_scale_rotation_translation();
        self.rotation = rot;
    }

    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        Self {
            translation: self.translation.lerp(other.translation, t),
            rotation: self.rotation.slerp(other.rotation, t),
            scale: self.scale.lerp(other.scale, t),
        }
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

// ── Scene graph ───────────────────────────────────────────────────────────────

pub type NodeId = usize;

#[derive(Debug, Clone)]
pub struct SceneNode {
    pub name: String,
    pub transform: Transform,
    pub mesh: Option<Arc<Mesh>>,
    pub material: Option<Arc<Material>>,
    pub visible: bool,
    pub children: Vec<NodeId>,
    pub parent: Option<NodeId>,
}

impl SceneNode {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            transform: Transform::IDENTITY,
            mesh: None,
            material: None,
            visible: true,
            children: Vec::new(),
            parent: None,
        }
    }

    pub fn with_mesh(mut self, mesh: Arc<Mesh>) -> Self {
        self.mesh = Some(mesh);
        self
    }

    pub fn with_material(mut self, mat: Arc<Material>) -> Self {
        self.material = Some(mat);
        self
    }

    pub fn with_transform(mut self, t: Transform) -> Self {
        self.transform = t;
        self
    }
}

#[derive(Debug, Default)]
pub struct Scene {
    nodes: Vec<SceneNode>,
    roots: Vec<NodeId>,
}

impl Scene {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node(&mut self, node: SceneNode) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(node);
        id
    }

    pub fn add_root(&mut self, id: NodeId) {
        self.roots.push(id);
    }

    pub fn add_child(&mut self, parent: NodeId, child: NodeId) {
        self.nodes[child].parent = Some(parent);
        self.nodes[parent].children.push(child);
    }

    pub fn node(&self, id: NodeId) -> &SceneNode {
        &self.nodes[id]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut SceneNode {
        &mut self.nodes[id]
    }

    pub fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    pub fn global_transform(&self, id: NodeId) -> Mat4 {
        let node = &self.nodes[id];
        match node.parent {
            None => node.transform.matrix(),
            Some(parent_id) => self.global_transform(parent_id) * node.transform.matrix(),
        }
    }

    /// Collect all visible (node, global_matrix) pairs in DFS order.
    pub fn collect_render_items(&self) -> Vec<(NodeId, Mat4)> {
        let mut out = Vec::new();
        for &root in &self.roots {
            self.collect_recursive(root, Mat4::IDENTITY, &mut out);
        }
        out
    }

    fn collect_recursive(&self, id: NodeId, parent_mat: Mat4, out: &mut Vec<(NodeId, Mat4)>) {
        let node = &self.nodes[id];
        if !node.visible {
            return;
        }
        let mat = parent_mat * node.transform.matrix();
        if node.mesh.is_some() {
            out.push((id, mat));
        }
        for &child in &node.children {
            self.collect_recursive(child, mat, out);
        }
    }

    /// Convenience: create a mesh node, add it as a root, and return its id.
    pub fn spawn_mesh(
        &mut self,
        name: impl Into<String>,
        mesh: Arc<Mesh>,
        material: Arc<Material>,
        transform: Transform,
    ) -> NodeId {
        let node = SceneNode::new(name)
            .with_mesh(mesh)
            .with_material(material)
            .with_transform(transform);
        let id = self.add_node(node);
        self.add_root(id);
        id
    }
}
