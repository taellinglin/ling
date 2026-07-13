//! The dimension-agnostic rig: a hierarchy of joints in an abstract pose space.
//!
//! A `Rig` knows nothing about meshes or pixels — a [`crate::binding`] maps joint
//! world-transforms onto a concrete target (3-D skin, 2-D vtex lattice, params…).

use crate::temperament::Temperament;
use glam::{Mat4, Quat, Vec3};

/// Local translate / rotate / scale, matching `ling-graphics::scene::Transform`.
#[derive(Debug, Clone, Copy, PartialEq)]
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

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    pub fn lerp(&self, o: &Self, t: f32) -> Self {
        Self {
            translation: self.translation.lerp(o.translation, t),
            rotation: self.rotation.slerp(o.rotation, t),
            scale: self.scale.lerp(o.scale, t),
        }
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

pub type JointId = usize;

/// One bone/control. Its `temperament` selects which solver family animates the
/// chain it leads (organic muscle/jiggle vs. mechanical exact coupling).
#[derive(Debug, Clone)]
pub struct Joint {
    pub name: String,
    pub parent: Option<JointId>,
    /// Rest (bind) local transform.
    pub rest: Transform,
    /// Current animated local transform (defaults to `rest`).
    pub local: Transform,
    pub temperament: Temperament,
    /// Optional joint angle limits in radians (min, max) around the local axis.
    pub limits: Option<(f32, f32)>,
}

impl Joint {
    pub fn new(name: impl Into<String>, parent: Option<JointId>, rest: Transform) -> Self {
        Self {
            name: name.into(),
            parent,
            rest,
            local: rest,
            temperament: Temperament::ORGANIC,
            limits: None,
        }
    }
}

/// A skeleton: joints plus a cached set of world matrices after [`Rig::resolve`].
#[derive(Debug, Clone, Default)]
pub struct Rig {
    pub joints: Vec<Joint>,
    world: Vec<Mat4>,
}

impl Rig {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a joint, returning its id. Parents must be added before children.
    pub fn add(&mut self, joint: Joint) -> JointId {
        let id = self.joints.len();
        self.joints.push(joint);
        self.world.push(Mat4::IDENTITY);
        id
    }

    pub fn len(&self) -> usize {
        self.joints.len()
    }

    pub fn is_empty(&self) -> bool {
        self.joints.is_empty()
    }

    pub fn find(&self, name: &str) -> Option<JointId> {
        self.joints.iter().position(|j| j.name == name)
    }

    pub fn set_local(&mut self, id: JointId, local: Transform) {
        if let Some(j) = self.joints.get_mut(id) {
            j.local = local;
        }
    }

    /// Reset every joint to its rest pose.
    pub fn reset(&mut self) {
        for j in &mut self.joints {
            j.local = j.rest;
        }
    }

    /// Recompute world matrices from local transforms (parents resolved first,
    /// which holds because children are always added after their parent).
    pub fn resolve(&mut self) {
        for i in 0..self.joints.len() {
            let local = self.joints[i].local.matrix();
            self.world[i] = match self.joints[i].parent {
                Some(p) => self.world[p] * local,
                None => local,
            };
        }
    }

    pub fn world(&self, id: JointId) -> Mat4 {
        self.world.get(id).copied().unwrap_or(Mat4::IDENTITY)
    }

    /// World-space position of a joint's origin (after [`resolve`]).
    pub fn world_pos(&self, id: JointId) -> Vec3 {
        self.world(id).transform_point3(Vec3::ZERO)
    }

    /// Skinning matrices = world(joint) * inverse(world_rest(joint)). Useful for
    /// linear-blend skinning once a [`crate::binding::SkinBinding`] is attached.
    pub fn skinning_matrices(&self) -> Vec<Mat4> {
        // Rebuild rest world matrices on the fly.
        let mut rest_world = vec![Mat4::IDENTITY; self.joints.len()];
        for i in 0..self.joints.len() {
            let local = self.joints[i].rest.matrix();
            rest_world[i] = match self.joints[i].parent {
                Some(p) => rest_world[p] * local,
                None => local,
            };
        }
        (0..self.joints.len())
            .map(|i| self.world[i] * rest_world[i].inverse())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hierarchy_accumulates() {
        let mut r = Rig::new();
        let root = r.add(Joint::new(
            "root",
            None,
            Transform::from_translation(Vec3::new(1.0, 0.0, 0.0)),
        ));
        let _tip = r.add(Joint::new(
            "tip",
            Some(root),
            Transform::from_translation(Vec3::new(0.0, 2.0, 0.0)),
        ));
        r.resolve();
        let p = r.world_pos(1);
        assert!((p - Vec3::new(1.0, 2.0, 0.0)).length() < 1e-5, "got {p:?}");
    }
}
