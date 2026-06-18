//! # ling-animation — "Anima"
//!
//! Ling's unified animation system. The thesis: **one rig, any target, any
//! temperament.** Every animated thing — a 2-D vtexture, a 3-D mesh, a particle
//! field, or a bag of scalar parameters — is a [`rig::Rig`] of abstract joints,
//! driven by composable drivers, and deformed onto its target by a binding.
//!
//! What sets it apart is the **temperament axis** ([`temperament::Temperament`]):
//! a per-chain scalar from **organic (灵)** to **mechanical (机)** that selects
//! and cross-fades the solver — muscle/jiggle/breath/IK on one end, exact
//! gear/cam/linkage coupling on the other. A robot can have organic hydraulic
//! sag; a creature can have a mechanical jaw.
//!
//! ## Layers
//! - [`ease`] — easing curves + a `Lerp` trait (standalone, wasm-safe).
//! - [`track`] — keyframe tracks + a playable [`track::Timeline`].
//! - [`rig`] — joints, hierarchy, world-pose + skinning matrices.
//! - [`temperament`] — the 灵 ↔ 机 axis.
//! - [`scalar`] — per-frame organic/mechanical driver math (powers the builtins).
//! - [`mechanism`] — exact linkages (gear trains, four-bar).
//! - [`creature`] — procedural biped/quadruped gaits + breathing.

pub mod creature;
pub mod ease;
pub mod face;
pub mod mechanism;
pub mod rig;
pub mod scalar;
pub mod temperament;
pub mod track;

pub use ease::{tween_ease, EaseFunction, Lerp};
pub use face::{Expression, FacePoint, FaceRig, Region};
pub use rig::{Joint, JointId, Rig, Transform};
pub use temperament::Temperament;
pub use track::{Keyframe, Timeline, Track};

/// A small owner of animation state, indexed by handle. Hosts (like the Ling
/// runtime) can park rigs and timelines here and drive them by integer handle.
#[derive(Debug, Default)]
pub struct Animator {
    rigs: Vec<Rig>,
    timelines: Vec<Timeline>,
}

impl Animator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_rig(&mut self, rig: Rig) -> usize {
        self.rigs.push(rig);
        self.rigs.len() - 1
    }
    pub fn rig(&self, h: usize) -> Option<&Rig> {
        self.rigs.get(h)
    }
    pub fn rig_mut(&mut self, h: usize) -> Option<&mut Rig> {
        self.rigs.get_mut(h)
    }

    /// Create a looping timeline of `duration` seconds; returns its handle.
    pub fn add_timeline(&mut self, duration: f32) -> usize {
        self.timelines.push(Timeline::new(duration));
        self.timelines.len() - 1
    }
    pub fn timeline(&self, h: usize) -> Option<&Timeline> {
        self.timelines.get(h)
    }

    /// Advance one timeline by `dt`; returns its new normalized progress `[0,1]`.
    pub fn tick(&mut self, h: usize, dt: f32) -> f32 {
        match self.timelines.get_mut(h) {
            Some(tl) => {
                tl.tick(dt);
                tl.normalized()
            },
            None => 0.0,
        }
    }

    /// Advance every timeline by `dt`.
    pub fn tick_all(&mut self, dt: f32) {
        for tl in &mut self.timelines {
            tl.tick(dt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn animator_ticks_timeline() {
        let mut a = Animator::new();
        let h = a.add_timeline(2.0);
        assert!((a.tick(h, 1.0) - 0.5).abs() < 1e-5);
    }
}
