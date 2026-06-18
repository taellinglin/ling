//! Scalar drivers — the math behind the `.ling` animation builtins.
//!
//! These are pure functions (no rig state) so they can be called frame-by-frame
//! from a script. They split cleanly into **organic (灵)** drivers — breath,
//! sway, gait, springy secondary motion, IK — and **mechanical (机)** drivers —
//! exact gear/cam/slider-crank/rack coupling.

use std::f32::consts::{PI, TAU};

// ── Organic (灵) ──────────────────────────────────────────────────────────────

/// Breathing multiplier centred on 1.0: `1 + depth·sin(2π·rate·t)`.
/// Drive a scale or a vtex parameter with it to give something "life".
pub fn breathe(t: f32, rate: f32, depth: f32) -> f32 {
    1.0 + depth * (TAU * rate * t).sin()
}

/// Secondary sway / jiggle: `amp·sin(2π·(freq·t + phase))`.
pub fn wobble(t: f32, freq: f32, amp: f32, phase: f32) -> f32 {
    amp * (TAU * (freq * t + phase)).sin()
}

/// Smooth, deterministic idle noise in `[-1, 1]` (value-noise of time).
pub fn idle_noise(t: f32, seed: f32) -> f32 {
    let x = t * 1.37 + seed * 13.0;
    let i = x.floor();
    let f = x - i;
    let u = f * f * (3.0 - 2.0 * f); // smoothstep
    let a = hash01(i + seed);
    let b = hash01(i + 1.0 + seed);
    (a + (b - a) * u) * 2.0 - 1.0
}

fn hash01(n: f32) -> f32 {
    let s = (n * 127.1 + 311.7).sin() * 43758.5453;
    s - s.floor()
}

/// Locomotion phase in `[0, 1)` from elapsed time and steps-per-second.
pub fn gait_phase(t: f32, speed: f32) -> f32 {
    (t * speed).rem_euclid(1.0)
}

/// Forward/back leg-swing offset for a gait phase (organic walk cycle).
pub fn gait_swing(t: f32, speed: f32, stride: f32) -> f32 {
    stride * (TAU * gait_phase(t, speed)).sin()
}

/// Foot-lift height for a gait phase — one-sided so the foot only rises during
/// the swing half of the cycle.
pub fn gait_lift(t: f32, speed: f32, height: f32) -> f32 {
    height * (TAU * gait_phase(t, speed)).sin().max(0.0)
}

/// Semi-implicit Euler spring step (organic secondary motion / spring-bone).
/// Returns `(new_pos, new_vel)`.
pub fn spring_step(
    pos: f32,
    vel: f32,
    target: f32,
    stiffness: f32,
    damping: f32,
    dt: f32,
) -> (f32, f32) {
    let accel = (target - pos) * stiffness - vel * damping;
    let new_vel = vel + accel * dt;
    let new_pos = pos + new_vel * dt;
    (new_pos, new_vel)
}

/// Planar two-bone IK. Given bone lengths and a 2-D target, returns
/// `(shoulder_angle, elbow_relative_angle)` in radians such that forward
/// kinematics lands the tip on (or as near as reach allows) the target.
pub fn two_bone_ik(l1: f32, l2: f32, tx: f32, ty: f32) -> (f32, f32) {
    let reach_min = (l1 - l2).abs() + 1e-4;
    let reach_max = l1 + l2 - 1e-4;
    let d = (tx * tx + ty * ty).sqrt().clamp(reach_min, reach_max);
    let cos_e = ((l1 * l1 + l2 * l2 - d * d) / (2.0 * l1 * l2)).clamp(-1.0, 1.0);
    let interior = cos_e.acos();
    let cos_a = ((d * d + l1 * l1 - l2 * l2) / (2.0 * d * l1)).clamp(-1.0, 1.0);
    let a = cos_a.acos();
    let base = ty.atan2(tx);
    (base + a, interior - PI)
}

/// Forward kinematics for the two-bone chain — the inverse of [`two_bone_ik`].
pub fn two_bone_fk(l1: f32, l2: f32, shoulder: f32, elbow: f32) -> (f32, f32) {
    let ex = l1 * shoulder.cos();
    let ey = l1 * shoulder.sin();
    let tip = shoulder + elbow;
    (ex + l2 * tip.cos(), ey + l2 * tip.sin())
}

// ── Mechanical (机) ───────────────────────────────────────────────────────────

/// Meshed gear coupling: output angle for a given input angle and tooth counts.
/// Meshed gears spin opposite, by the tooth ratio. Exact, deterministic.
pub fn gear(input_angle: f32, teeth_in: f32, teeth_out: f32) -> f32 {
    if teeth_out.abs() < 1e-6 {
        return 0.0;
    }
    -input_angle * teeth_in / teeth_out
}

/// Cam follower lift over a full rotation: smooth full rise-and-return,
/// `lift · ½(1 − cos θ)` (0 at θ=0, peak at θ=π).
pub fn cam_lift(angle: f32, lift: f32) -> f32 {
    lift * 0.5 * (1.0 - angle.cos())
}

/// Slider-crank piston pin position along the cylinder axis from crank angle.
pub fn piston(angle: f32, crank: f32, rod: f32) -> f32 {
    let s = crank * angle.sin();
    crank * angle.cos() + (rod * rod - s * s).max(0.0).sqrt()
}

/// Rack-and-pinion linear travel from pinion rotation: `θ · radius`.
pub fn rack(angle: f32, pinion_radius: f32) -> f32 {
    angle * pinion_radius
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn breathe_oscillates_around_one() {
        assert!((breathe(0.0, 1.0, 0.2) - 1.0).abs() < 1e-6);
        assert!((breathe(0.25, 1.0, 0.2) - 1.2).abs() < 1e-5);
    }
    #[test]
    fn ik_round_trips_to_fk() {
        let (l1, l2) = (1.0f32, 0.8f32);
        for &(tx, ty) in &[(1.2f32, 0.4f32), (0.5, 1.0), (1.0, -0.6), (-0.7, 0.9)] {
            // keep within reach for an exact round-trip
            let d = (tx * tx + ty * ty).sqrt();
            if d > l1 + l2 - 0.05 || d < (l1 - l2).abs() + 0.05 {
                continue;
            }
            let (sh, el) = two_bone_ik(l1, l2, tx, ty);
            let (fx, fy) = two_bone_fk(l1, l2, sh, el);
            assert!(
                (fx - tx).hypot(fy - ty) < 1e-3,
                "({tx},{ty}) -> ({fx},{fy})"
            );
        }
    }
    #[test]
    fn gears_are_exact_and_opposite() {
        // 12-tooth driving 36-tooth: output turns 1/3 as fast, opposite sign.
        assert!((gear(3.0, 12.0, 36.0) + 1.0).abs() < 1e-6);
    }
    #[test]
    fn spring_settles_toward_target() {
        let (mut p, mut v) = (0.0f32, 0.0f32);
        for _ in 0..2000 {
            let (np, nv) = spring_step(p, v, 1.0, 120.0, 14.0, 1.0 / 120.0);
            p = np;
            v = nv;
        }
        assert!((p - 1.0).abs() < 1e-2, "settled at {p}");
    }
}
