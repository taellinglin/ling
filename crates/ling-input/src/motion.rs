//! Inertial motion — gyroscope + accelerometer fusion and gyro-aiming.
//!
//! Modern controllers (Switch Joy-Con, DualSense, Steam Deck) and every phone
//! ship an IMU. [`MotionState`] runs a small complementary filter to fuse the
//! gyro (precise but drifting) with the accelerometer (noisy but absolute "up")
//! into a stable [`glam::Quat`] orientation, and exposes [`MotionState::gyro_aim`]
//! for the now-standard gyro-as-mouse aiming.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use glam::{Quat, Vec2, Vec3};

/// One raw IMU reading.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct MotionSample {
    /// Angular velocity in radians/second (x=pitch, y=yaw, z=roll).
    pub gyro: Vec3,
    /// Proper acceleration in g (≈ `Vec3::Y` when resting, +Y up).
    pub accel: Vec3,
    /// Seconds since the previous sample.
    pub dt: f32,
}

/// Which frame gyro deltas are interpreted in for aiming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum GyroSpace {
    /// Raw device axes (yaw = turn the controller flat).
    Local,
    /// "Player space" — combine yaw + roll by gravity so tilting the pad also
    /// turns the camera; the comfortable default for handhelds.
    Player,
}

/// Fused orientation + derived gravity/user-acceleration state.
#[derive(Debug, Clone, Copy)]
pub struct MotionState {
    /// Fused world orientation of the device.
    pub orientation: Quat,
    /// Latest angular velocity (rad/s).
    pub angular_velocity: Vec3,
    /// Low-pass estimate of the gravity direction in the device frame (g).
    pub gravity: Vec3,
    /// Acceleration with gravity removed, in the device frame (g).
    pub user_accel: Vec3,
    /// Complementary blend toward the accelerometer per second (`0..1`).
    pub correction: f32,
}

impl Default for MotionState {
    fn default() -> Self {
        Self {
            orientation: Quat::IDENTITY,
            angular_velocity: Vec3::ZERO,
            gravity: Vec3::Y,
            user_accel: Vec3::ZERO,
            correction: 2.0,
        }
    }
}

impl MotionState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fuse one sample into the running orientation estimate.
    pub fn update(&mut self, s: MotionSample) {
        self.angular_velocity = s.gyro;
        let dt = s.dt.max(0.0);

        // 1. Integrate the gyro (the precise short-term signal).
        if dt > 0.0 {
            let dq = Quat::from_scaled_axis(s.gyro * dt);
            self.orientation = (self.orientation * dq).normalize();
        }

        // 2. Track gravity (low-passed accel) and split off user acceleration.
        let a = s.accel;
        if a.length_squared() > 1e-6 {
            let alpha = (self.correction * dt).clamp(0.0, 1.0);
            self.gravity = (self.gravity * (1.0 - alpha) + a * alpha).normalize_or(self.gravity);
            self.user_accel = a - self.gravity * a.dot(self.gravity);

            // 3. Nudge orientation so its "up" matches measured gravity (kills
            //    yaw-free drift on pitch/roll). World up is +Y.
            let world_up = self.orientation * self.gravity.normalize_or(Vec3::Y);
            let correction = Quat::from_rotation_arc(world_up, Vec3::Y);
            let gentle = Quat::IDENTITY.slerp(correction, alpha.min(0.5));
            self.orientation = (gentle * self.orientation).normalize();
        }
    }

    /// Camera delta (yaw, pitch) in radians for gyro aiming this frame.
    /// `sensitivity` scales raw angular travel; flip via negative sensitivity.
    #[must_use]
    pub fn gyro_aim(&self, sensitivity: f32, space: GyroSpace, dt: f32) -> Vec2 {
        let g = self.angular_velocity;
        let (yaw, pitch) = match space {
            GyroSpace::Local => (g.y, g.x),
            GyroSpace::Player => {
                // Blend yaw + roll by how upright the device is, so a tilted
                // controller still turns the camera left/right naturally.
                let up = self.gravity.normalize_or(Vec3::Y);
                let yaw = -(g.y * up.y + g.z * up.z);
                (yaw, g.x)
            },
        };
        Vec2::new(yaw, pitch) * sensitivity * dt
    }

    /// Reset accumulated orientation (e.g. on a "recenter" gesture).
    pub fn recenter(&mut self) {
        self.orientation = Quat::IDENTITY;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resting_keeps_identity_ish() {
        let mut m = MotionState::new();
        for _ in 0..120 {
            m.update(MotionSample { gyro: Vec3::ZERO, accel: Vec3::Y, dt: 1.0 / 60.0 });
        }
        // No rotation input, gravity straight up -> stays near identity.
        assert!(m.orientation.angle_between(Quat::IDENTITY) < 0.05);
        assert!(m.user_accel.length() < 0.05);
    }

    #[test]
    fn gyro_aim_scales_with_velocity() {
        let mut m = MotionState::new();
        m.update(MotionSample { gyro: Vec3::new(0.0, 1.0, 0.0), accel: Vec3::Y, dt: 0.016 });
        let aim = m.gyro_aim(1.0, GyroSpace::Local, 0.016);
        assert!(aim.x.abs() > 0.0);
    }
}
