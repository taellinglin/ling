//! Analog axes & sticks — deadzones, response curves, calibration.
//!
//! Raw analog hardware is noisy, off-center, and linear when players want
//! precision near the middle. [`Axis`] turns a raw 1-D reading into a clean,
//! calibrated, curved `-1..=1` value; [`Stick`] does the same for a 2-D stick
//! with a *radial* deadzone (the correct way — per-axis deadzones make
//! diagonals feel wrong).

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use glam::Vec2;

/// Response shaping applied after the deadzone, on the `0..=1` magnitude.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Curve {
    /// Output equals input.
    Linear,
    /// `m^exp`. `exp > 1` softens the center for fine aim; `exp < 1` sharpens it.
    Power(f32),
    /// Smoothstep — gentle ease-in/out.
    Smooth,
}

impl Curve {
    /// Shape a non-negative magnitude `m` (already in `0..=1`).
    #[must_use]
    pub fn apply(self, m: f32) -> f32 {
        let m = m.clamp(0.0, 1.0);
        match self {
            Self::Linear => m,
            Self::Power(e) => m.powf(e),
            Self::Smooth => m * m * (3.0 - 2.0 * m),
        }
    }
}

impl Default for Curve {
    fn default() -> Self {
        Self::Linear
    }
}

/// Configuration + transform for a single analog axis.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Axis {
    /// Inner deadzone as a fraction of full travel (`0..1`).
    pub deadzone: f32,
    /// Travel fraction that already maps to full output (anti-saturation, `<=1`).
    pub saturation: f32,
    /// Response curve applied to the post-deadzone magnitude.
    pub curve: Curve,
    /// Flip the sign of the output.
    pub invert: bool,
    /// Calibration: raw value at full negative.
    pub min: f32,
    /// Calibration: raw resting value.
    pub center: f32,
    /// Calibration: raw value at full positive.
    pub max: f32,
}

impl Default for Axis {
    fn default() -> Self {
        Self {
            deadzone: 0.08,
            saturation: 1.0,
            curve: Curve::Linear,
            invert: false,
            min: -1.0,
            center: 0.0,
            max: 1.0,
        }
    }
}

impl Axis {
    /// Trigger preset: one-sided `0..=1` (resting low, soft floor deadzone).
    #[must_use]
    pub fn trigger() -> Self {
        Self { deadzone: 0.04, min: 0.0, center: 0.0, max: 1.0, ..Self::default() }
    }

    /// Map a raw reading through calibration, deadzone, saturation and curve.
    #[must_use]
    pub fn value(&self, raw: f32) -> f32 {
        // 1. Calibrate raw -> normalized -1..=1 around the resting center.
        let n = if raw >= self.center {
            let span = self.max - self.center;
            if span > f32::EPSILON {
                (raw - self.center) / span
            } else {
                0.0
            }
        } else {
            let span = self.center - self.min;
            if span > f32::EPSILON {
                (raw - self.center) / span
            } else {
                0.0
            }
        }
        .clamp(-1.0, 1.0);

        // 2. Radial (here, 1-D) deadzone + saturation rescale on magnitude.
        let sign = if n < 0.0 { -1.0 } else { 1.0 };
        let mag = n.abs();
        let dz = self.deadzone.clamp(0.0, 0.999);
        let sat = self.saturation.clamp(dz + 1e-3, 1.0);
        let shaped = if mag <= dz { 0.0 } else { ((mag - dz) / (sat - dz)).min(1.0) };

        let out = self.curve.apply(shaped) * sign;
        if self.invert {
            -out
        } else {
            out
        }
    }
}

/// A 2-D analog stick with a single radial deadzone — diagonals stay round.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Stick {
    pub deadzone: f32,
    pub saturation: f32,
    pub curve: Curve,
    pub invert_x: bool,
    pub invert_y: bool,
}

impl Default for Stick {
    fn default() -> Self {
        Self {
            deadzone: 0.12,
            saturation: 0.95,
            curve: Curve::Linear,
            invert_x: false,
            invert_y: false,
        }
    }
}

impl Stick {
    /// Map a raw stick vector (roughly within the unit disc) to a clean vector.
    /// Direction is preserved; only the magnitude is deadzoned, saturated, and
    /// curved. (Screen-down convention: `+y` points down — see project notes.)
    #[must_use]
    pub fn vector(&self, raw: Vec2) -> Vec2 {
        let mut v = raw;
        if self.invert_x {
            v.x = -v.x;
        }
        if self.invert_y {
            v.y = -v.y;
        }
        let mag = v.length();
        if mag <= f32::EPSILON {
            return Vec2::ZERO;
        }
        let dz = self.deadzone.clamp(0.0, 0.999);
        let sat = self.saturation.clamp(dz + 1e-3, 1.0);
        if mag <= dz {
            return Vec2::ZERO;
        }
        let shaped = self.curve.apply(((mag - dz) / (sat - dz)).min(1.0));
        v / mag * shaped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadzone_kills_small_input_but_reaches_one() {
        let a = Axis::default();
        assert_eq!(a.value(0.0), 0.0);
        assert_eq!(a.value(0.05), 0.0); // inside deadzone
        assert!((a.value(1.0) - 1.0).abs() < 1e-5);
        assert!((a.value(-1.0) + 1.0).abs() < 1e-5);
    }

    #[test]
    fn stick_preserves_direction() {
        let s = Stick { deadzone: 0.1, saturation: 1.0, ..Stick::default() };
        let v = s.vector(Vec2::new(0.7, 0.7));
        // 45-degree input stays on the diagonal
        assert!((v.x - v.y).abs() < 1e-5);
        assert!(s.vector(Vec2::new(0.05, 0.0)) == Vec2::ZERO);
    }
}
