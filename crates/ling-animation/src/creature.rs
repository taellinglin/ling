//! Organic (灵) presets — procedural locomotion for creatures.
//!
//! Each leg reports `(swing, lift)`: a forward/back offset and an upward foot
//! lift, phased so the gait reads as a natural walk/trot.

use crate::scalar::{gait_swing, gait_lift, breathe};

fn leg(t: f32, speed: f32, stride: f32, height: f32, phase: f32) -> (f32, f32) {
    let pt = t + phase / speed.max(1e-3);
    (gait_swing(pt, speed, stride), gait_lift(pt, speed, height))
}

/// Two legs in anti-phase (a biped walk). Returns `[left, right]`.
pub fn biped_legs(t: f32, speed: f32, stride: f32, height: f32) -> [(f32, f32); 2] {
    [
        leg(t, speed, stride, height, 0.0),
        leg(t, speed, stride, height, 0.5),
    ]
}

/// Four legs in a diagonal trot (FL+BR together, FR+BL together).
/// Returns `[front_left, front_right, back_left, back_right]`.
pub fn quadruped_legs(t: f32, speed: f32, stride: f32, height: f32) -> [(f32, f32); 4] {
    [
        leg(t, speed, stride, height, 0.0),  // FL
        leg(t, speed, stride, height, 0.5),  // FR
        leg(t, speed, stride, height, 0.5),  // BL
        leg(t, speed, stride, height, 0.0),  // BR
    ]
}

/// Chest/belly breathing scale for an idle creature (1.0-centred).
pub fn breathing_scale(t: f32, rate: f32, depth: f32) -> f32 {
    breathe(t, rate, depth)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn biped_legs_are_antiphase() {
        let [l, r] = biped_legs(0.25, 1.0, 1.0, 0.3);
        // a quarter-cycle apart from the half-cycle offset → opposite swing signs
        assert!(l.0 * r.0 <= 1e-3);
    }
}
