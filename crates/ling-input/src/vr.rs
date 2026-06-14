//! XR — 6-DoF poses, headset, controllers, skeletal hands, and eye gaze.
//!
//! Built for the 2030 mixed-reality stack: every tracked thing is a [`Pose`]
//! (position + orientation + velocities). Controllers carry the standard
//! action set plus per-finger [`HandSkeleton`] joints (OpenXR's 26-joint hand),
//! and the headset carries an eye-[`Gaze`] ray — the gaze-and-pinch interaction
//! model. [`PlaySpace`] holds the guardian/chaperone bounds.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use glam::{Quat, Vec2, Vec3};

use crate::button::Button;
use crate::haptics::Rumble;

/// A tracked 6-DoF pose in world (play-space) coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Pose {
    pub position: Vec3,
    pub orientation: Quat,
    pub linear_velocity: Vec3,
    pub angular_velocity: Vec3,
    /// `false` when tracking is lost (occluded / out of range).
    pub tracked: bool,
}

impl Pose {
    pub const IDENTITY: Self = Self {
        position: Vec3::ZERO,
        orientation: Quat::IDENTITY,
        linear_velocity: Vec3::ZERO,
        angular_velocity: Vec3::ZERO,
        tracked: false,
    };

    /// Forward (-Z), the OpenXR/“look” direction.
    #[must_use]
    pub fn forward(&self) -> Vec3 {
        self.orientation * Vec3::NEG_Z
    }
    #[must_use]
    pub fn right(&self) -> Vec3 {
        self.orientation * Vec3::X
    }
    #[must_use]
    pub fn up(&self) -> Vec3 {
        self.orientation * Vec3::Y
    }
    /// Transform a local-space point into world space.
    #[must_use]
    pub fn transform_point(&self, local: Vec3) -> Vec3 {
        self.position + self.orientation * local
    }
}

impl Default for Pose {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// Which hand a controller / hand belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Hand {
    Left,
    Right,
}

/// Buttons on a standard XR controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum XrButton {
    /// Lower face button (A / X).
    PrimaryFace,
    /// Upper face button (B / Y).
    SecondaryFace,
    /// Thumbstick / trackpad click.
    Thumbstick,
    /// Trigger fully pressed (digital).
    Trigger,
    /// Grip fully squeezed (digital).
    Grip,
    /// System / menu.
    Menu,
}

impl XrButton {
    pub const COUNT: usize = 6;
    pub const ALL: [Self; Self::COUNT] = [
        Self::PrimaryFace,
        Self::SecondaryFace,
        Self::Thumbstick,
        Self::Trigger,
        Self::Grip,
        Self::Menu,
    ];
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// The 26 OpenXR hand joints (wrist + palm + 5×fingers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum HandJoint {
    Palm,
    Wrist,
    ThumbMetacarpal,
    ThumbProximal,
    ThumbDistal,
    ThumbTip,
    IndexMetacarpal,
    IndexProximal,
    IndexIntermediate,
    IndexDistal,
    IndexTip,
    MiddleMetacarpal,
    MiddleProximal,
    MiddleIntermediate,
    MiddleDistal,
    MiddleTip,
    RingMetacarpal,
    RingProximal,
    RingIntermediate,
    RingDistal,
    RingTip,
    LittleMetacarpal,
    LittleProximal,
    LittleIntermediate,
    LittleDistal,
    LittleTip,
}

impl HandJoint {
    pub const COUNT: usize = 26;
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// Per-finger skeletal tracking, plus convenience grip/pinch strengths.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct HandSkeleton {
    /// World poses for each [`HandJoint`] (index by `joint.index()`).
    pub joints: [Pose; HandJoint::COUNT],
    /// Per-joint radius in metres (for collision / rendering).
    pub radii: [f32; HandJoint::COUNT],
    /// `0` open .. `1` thumb-tip touching index-tip.
    pub pinch: f32,
    /// `0` open .. `1` full fist.
    pub grab: f32,
    /// Whether camera/optical hand tracking is currently active.
    pub active: bool,
}

impl Default for HandSkeleton {
    fn default() -> Self {
        Self {
            joints: [Pose::IDENTITY; HandJoint::COUNT],
            radii: [0.01; HandJoint::COUNT],
            pinch: 0.0,
            grab: 0.0,
            active: false,
        }
    }
}

impl HandSkeleton {
    #[must_use]
    pub fn joint(&self, j: HandJoint) -> Pose {
        self.joints[j.index()]
    }
}

/// One XR controller / tracked hand.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct XrController {
    pub hand: Hand,
    /// Grip pose — where the hand physically is.
    pub grip_pose: Pose,
    /// Aim pose — the pointer ray for far interaction.
    pub aim_pose: Pose,
    pub thumbstick: Vec2,
    pub trigger: f32,
    pub grip: f32,
    pub skeleton: HandSkeleton,
    /// Outbound haptic command applied by the backend.
    pub haptic: Rumble,
    /// Battery level `0..=1` (`<0` = unknown).
    pub battery: f32,
    buttons: [Button; XrButton::COUNT],
}

impl XrController {
    #[must_use]
    pub fn new(hand: Hand) -> Self {
        Self {
            hand,
            grip_pose: Pose::IDENTITY,
            aim_pose: Pose::IDENTITY,
            thumbstick: Vec2::ZERO,
            trigger: 0.0,
            grip: 0.0,
            skeleton: HandSkeleton::default(),
            haptic: Rumble::OFF,
            battery: -1.0,
            buttons: [Button::new(); XrButton::COUNT],
        }
    }

    #[must_use]
    pub fn button(&self, b: XrButton) -> &Button {
        &self.buttons[b.index()]
    }
    pub fn set_button(&mut self, b: XrButton, down: bool, dt: f32) {
        self.buttons[b.index()].update(down, dt);
    }
    /// The far-interaction pointer ray `(origin, direction)`.
    #[must_use]
    pub fn ray(&self) -> (Vec3, Vec3) {
        (self.aim_pose.position, self.aim_pose.forward())
    }
}

/// Eye-tracking gaze ray (Vision Pro / Quest Pro style).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Gaze {
    pub origin: Vec3,
    pub direction: Vec3,
    /// `0` closed .. `1` fully open (blink detection).
    pub openness: f32,
    pub valid: bool,
}

/// The head-mounted display.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Headset {
    pub pose: Pose,
    pub left_eye: Pose,
    pub right_eye: Pose,
    pub gaze: Gaze,
    /// Inter-pupillary distance in metres.
    pub ipd: f32,
    /// `true` while the HMD is on the user's head.
    pub worn: bool,
}

impl Default for Headset {
    fn default() -> Self {
        Self {
            pose: Pose::IDENTITY,
            left_eye: Pose::IDENTITY,
            right_eye: Pose::IDENTITY,
            gaze: Gaze::default(),
            ipd: 0.063,
            worn: false,
        }
    }
}

/// How the play-space boundary was set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum BoundaryKind {
    /// Seated / standing, no room boundary.
    Stationary,
    /// A room-scale polygon the player can walk inside.
    RoomScale,
}

/// Guardian / chaperone bounds in the floor (XZ) plane.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct PlaySpace {
    /// Floor-plane polygon (metres). Empty for [`BoundaryKind::Stationary`].
    pub bounds: Vec<Vec2>,
    pub kind: Option<BoundaryKind>,
}

impl PlaySpace {
    /// Is a world point inside the boundary polygon? (XZ projection, ray-cast.)
    #[must_use]
    pub fn contains_xz(&self, p: Vec3) -> bool {
        let n = self.bounds.len();
        if n < 3 {
            return true; // no boundary => unbounded
        }
        let (x, z) = (p.x, p.z);
        let mut inside = false;
        let mut j = n - 1;
        for i in 0..n {
            let (xi, zi) = (self.bounds[i].x, self.bounds[i].y);
            let (xj, zj) = (self.bounds[j].x, self.bounds[j].y);
            let intersects = ((zi > z) != (zj > z))
                && (x < (xj - xi) * (z - zi) / (zj - zi) + xi);
            if intersects {
                inside = !inside;
            }
            j = i;
        }
        inside
    }
}

/// Aggregate XR session state — one HMD, two hands, the play-space.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Xr {
    pub headset: Headset,
    pub left: XrController,
    pub right: XrController,
    pub play_space: PlaySpace,
    /// Whether an XR session is currently active.
    pub active: bool,
}

impl Default for Xr {
    fn default() -> Self {
        Self {
            headset: Headset::default(),
            left: XrController::new(Hand::Left),
            right: XrController::new(Hand::Right),
            play_space: PlaySpace::default(),
            active: false,
        }
    }
}

impl Xr {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    #[must_use]
    pub fn hand(&self, hand: Hand) -> &XrController {
        match hand {
            Hand::Left => &self.left,
            Hand::Right => &self.right,
        }
    }
    pub fn hand_mut(&mut self, hand: Hand) -> &mut XrController {
        match hand {
            Hand::Left => &mut self.left,
            Hand::Right => &mut self.right,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pose_basis_is_right_handed() {
        let p = Pose { orientation: Quat::IDENTITY, ..Pose::IDENTITY };
        assert!((p.forward() - Vec3::NEG_Z).length() < 1e-6);
        assert!((p.right() - Vec3::X).length() < 1e-6);
    }

    #[test]
    fn playspace_polygon_containment() {
        let ps = PlaySpace {
            bounds: vec![
                Vec2::new(-1.0, -1.0),
                Vec2::new(1.0, -1.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(-1.0, 1.0),
            ],
            kind: Some(BoundaryKind::RoomScale),
        };
        assert!(ps.contains_xz(Vec3::ZERO));
        assert!(!ps.contains_xz(Vec3::new(2.0, 0.0, 0.0)));
    }
}
