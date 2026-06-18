//! Face & skull topology — a labelled vector map for facial-expression rigging.
//!
//! Two layers:
//!   • **Skull** anchors — rigid bony landmarks (cranium, brow ridge, zygomatic
//!     cheekbones, jaw hinge/angle, chin) the soft tissue rides on.
//!   • **Soft** landmarks — brows, lids, eye corners, iris, nose, cheeks, lips,
//!     chin — the points an expression actually moves.
//!
//! Coordinates are in a normalised, front-facing face space: `x` right, `y` up,
//! origin between the eyes, with the face roughly spanning `x ∈ [-1, 1]` and
//! `y ∈ [-1.3 (chin) … 1.55 (cranium)]`.
//!
//! Expressions are FACS-flavoured blendshapes: each contributes a per-landmark
//! offset, summed by weight, so any number can be layered (a half-blink over a
//! smile over a raised brow).

use glam::Vec2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Region {
    Cranium,
    Skull,
    Scalp,
    Brow,
    Eye,
    Lid,
    Iris,
    Nose,
    Cheek,
    Mouth,
    Jaw,
}

/// One labelled point in the face/skull vector map.
#[derive(Debug, Clone, Copy)]
pub struct FacePoint {
    pub name: &'static str,
    pub rest: Vec2,
    pub region: Region,
    /// Rigid (bone) landmark — expressions never move it.
    pub rigid: bool,
}

const fn p(name: &'static str, x: f32, y: f32, region: Region, rigid: bool) -> FacePoint {
    FacePoint { name, rest: Vec2::new(x, y), region, rigid }
}

/// The canonical face + skull topology (the "vector map").
pub fn topology() -> Vec<FacePoint> {
    use Region::*;
    vec![
        // ── Scalp / hairline (rigid hair-rig anchors the hair rides on) ──
        p("crown", 0.00, 1.40, Scalp, true),
        p("hairline_mid", 0.00, 0.95, Scalp, true),
        p("hairline_l", -0.55, 0.85, Scalp, true),
        p("hairline_r", 0.55, 0.85, Scalp, true),
        p("sideburn_l", -0.85, 0.35, Scalp, true),
        p("sideburn_r", 0.85, 0.35, Scalp, true),
        // ── Skull / bone (rigid anchors) ──
        p("cranium_top", 0.00, 1.55, Cranium, true),
        p("glabella", 0.00, 0.62, Skull, true),
        p("nasion", 0.00, 0.42, Skull, true),
        p("temple_l", -0.92, 0.70, Skull, true),
        p("temple_r", 0.92, 0.70, Skull, true),
        p("brow_ridge_l", -0.50, 0.62, Skull, true),
        p("brow_ridge_r", 0.50, 0.62, Skull, true),
        p("zygomatic_l", -0.78, -0.05, Skull, true),
        p("zygomatic_r", 0.78, -0.05, Skull, true),
        p("jaw_hinge_l", -0.85, -0.25, Jaw, true),
        p("jaw_hinge_r", 0.85, -0.25, Jaw, true),
        p("gonion_l", -0.60, -0.95, Jaw, true),
        p("gonion_r", 0.60, -0.95, Jaw, true),
        p("menton", 0.00, -1.30, Jaw, true),
        // ── Brows ──
        p("brow_inner_l", -0.18, 0.56, Brow, false),
        p("brow_inner_r", 0.18, 0.56, Brow, false),
        p("brow_mid_l", -0.42, 0.62, Brow, false),
        p("brow_mid_r", 0.42, 0.62, Brow, false),
        p("brow_outer_l", -0.62, 0.52, Brow, false),
        p("brow_outer_r", 0.62, 0.52, Brow, false),
        // ── Eyes / lids / iris ──
        p("eye_inner_l", -0.20, 0.30, Eye, false),
        p("eye_inner_r", 0.20, 0.30, Eye, false),
        p("eye_outer_l", -0.68, 0.30, Eye, false),
        p("eye_outer_r", 0.68, 0.30, Eye, false),
        p("lid_upper_l", -0.45, 0.42, Lid, false),
        p("lid_upper_r", 0.45, 0.42, Lid, false),
        p("lid_lower_l", -0.45, 0.18, Lid, false),
        p("lid_lower_r", 0.45, 0.18, Lid, false),
        p("iris_l", -0.45, 0.30, Iris, false),
        p("iris_r", 0.45, 0.30, Iris, false),
        // ── Nose ──
        p("nose_bridge", 0.00, 0.20, Nose, false),
        p("nose_tip", 0.00, -0.18, Nose, false),
        p("nostril_l", -0.18, -0.22, Nose, false),
        p("nostril_r", 0.18, -0.22, Nose, false),
        // ── Cheeks ──
        p("cheek_l", -0.60, -0.20, Cheek, false),
        p("cheek_r", 0.60, -0.20, Cheek, false),
        // ── Mouth / lips ──
        p("mouth_corner_l", -0.38, -0.62, Mouth, false),
        p("mouth_corner_r", 0.38, -0.62, Mouth, false),
        p("lip_top_mid", 0.00, -0.52, Mouth, false),
        p("lip_top_l", -0.18, -0.55, Mouth, false),
        p("lip_top_r", 0.18, -0.55, Mouth, false),
        p("lip_bot_mid", 0.00, -0.72, Mouth, false),
        p("lip_bot_l", -0.18, -0.69, Mouth, false),
        p("lip_bot_r", 0.18, -0.69, Mouth, false),
        p("philtrum", 0.00, -0.42, Mouth, false),
        p("chin", 0.00, -1.05, Jaw, false),
    ]
}

/// FACS-flavoured expression blendshapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Expression {
    Blink,
    Smile,
    Frown,
    Surprise,
    Squint,
    JawOpen,
    Anger,
    Sad,
}

impl Expression {
    /// Per-landmark displacement this expression applies at full weight.
    pub fn offset(self, name: &str) -> Vec2 {
        use Expression::*;
        let z = Vec2::ZERO;
        match self {
            Blink => match name {
                "lid_upper_l" | "lid_upper_r" => Vec2::new(0.0, -0.24), // upper lid drops to lower
                "iris_l" | "iris_r" => Vec2::new(0.0, -0.05),
                "lid_lower_l" | "lid_lower_r" => Vec2::new(0.0, 0.02),
                _ => z,
            },
            Smile => match name {
                "mouth_corner_l" => Vec2::new(-0.08, 0.14),
                "mouth_corner_r" => Vec2::new(0.08, 0.14),
                "cheek_l" => Vec2::new(-0.02, 0.12),
                "cheek_r" => Vec2::new(0.02, 0.12),
                "lid_lower_l" | "lid_lower_r" => Vec2::new(0.0, 0.05), // Duchenne squint
                "lip_top_mid" => Vec2::new(0.0, 0.03),
                _ => z,
            },
            Frown => match name {
                "mouth_corner_l" => Vec2::new(0.01, -0.13),
                "mouth_corner_r" => Vec2::new(-0.01, -0.13),
                "brow_inner_l" => Vec2::new(0.02, -0.05),
                "brow_inner_r" => Vec2::new(-0.02, -0.05),
                _ => z,
            },
            Surprise => match name {
                "brow_inner_l" | "brow_mid_l" | "brow_outer_l" => Vec2::new(0.0, 0.12),
                "brow_inner_r" | "brow_mid_r" | "brow_outer_r" => Vec2::new(0.0, 0.12),
                "lid_upper_l" | "lid_upper_r" => Vec2::new(0.0, 0.08),
                "lip_bot_mid" | "chin" => Vec2::new(0.0, -0.12),
                "lip_top_mid" => Vec2::new(0.0, 0.02),
                _ => z,
            },
            Squint => match name {
                "lid_upper_l" | "lid_upper_r" => Vec2::new(0.0, -0.08),
                "lid_lower_l" | "lid_lower_r" => Vec2::new(0.0, 0.08),
                _ => z,
            },
            JawOpen => match name {
                "chin" | "menton" => Vec2::new(0.0, -0.20),
                "lip_bot_mid" | "lip_bot_l" | "lip_bot_r" => Vec2::new(0.0, -0.16),
                "lip_top_mid" => Vec2::new(0.0, -0.03),
                _ => z,
            },
            Anger => match name {
                "brow_inner_l" => Vec2::new(0.04, -0.10),
                "brow_inner_r" => Vec2::new(-0.04, -0.10),
                "brow_outer_l" | "brow_outer_r" => Vec2::new(0.0, 0.03),
                "lid_upper_l" | "lid_upper_r" => Vec2::new(0.0, -0.04),
                "mouth_corner_l" => Vec2::new(0.02, -0.06),
                "mouth_corner_r" => Vec2::new(-0.02, -0.06),
                _ => z,
            },
            Sad => match name {
                "brow_inner_l" => Vec2::new(0.03, 0.08), // inner-brow raise
                "brow_inner_r" => Vec2::new(-0.03, 0.08),
                "mouth_corner_l" => Vec2::new(0.0, -0.08),
                "mouth_corner_r" => Vec2::new(0.0, -0.08),
                "lip_bot_mid" => Vec2::new(0.0, 0.04), // slight pout
                _ => z,
            },
        }
    }
}

/// A solvable face: the topology plus a way to deform it by weighted expressions.
#[derive(Debug, Clone)]
pub struct FaceRig {
    pub points: Vec<FacePoint>,
}

impl FaceRig {
    pub fn new() -> Self {
        Self { points: topology() }
    }

    pub fn rest(&self, name: &str) -> Option<Vec2> {
        self.points.iter().find(|p| p.name == name).map(|p| p.rest)
    }

    /// Deform a single landmark by a set of `(expression, weight)` controls.
    pub fn solve_point(&self, name: &str, weights: &[(Expression, f32)]) -> Option<Vec2> {
        let base = self.rest(name)?;
        let rigid = self
            .points
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.rigid)
            .unwrap_or(false);
        if rigid {
            return Some(base);
        }
        let mut off = Vec2::ZERO;
        for &(e, w) in weights {
            off += e.offset(name) * w;
        }
        Some(base + off)
    }

    /// Deform the whole topology, returning positions aligned with `self.points`.
    pub fn solve(&self, weights: &[(Expression, f32)]) -> Vec<Vec2> {
        self.points
            .iter()
            .map(|p| {
                if p.rigid {
                    return p.rest;
                }
                let mut off = Vec2::ZERO;
                for &(e, w) in weights {
                    off += e.offset(p.name) * w;
                }
                p.rest + off
            })
            .collect()
    }

    /// Vertical eye aperture (upper-lid → lower-lid gap) for the left eye —
    /// handy for driving a blink directly.
    pub fn eye_aperture_l(&self, weights: &[(Expression, f32)]) -> f32 {
        let u = self
            .solve_point("lid_upper_l", weights)
            .unwrap_or(Vec2::ZERO);
        let l = self
            .solve_point("lid_lower_l", weights)
            .unwrap_or(Vec2::ZERO);
        (u.y - l.y).max(0.0)
    }
}

impl Default for FaceRig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topology_is_symmetric_and_complete() {
        let t = topology();
        assert!(
            t.len() >= 40,
            "expected a rich landmark set, got {}",
            t.len()
        );
        // every "_l" landmark has a mirrored "_r" with negated x
        for fp in &t {
            if let Some(stem) = fp.name.strip_suffix("_l") {
                let r = t
                    .iter()
                    .find(|q| q.name == format!("{stem}_r"))
                    .expect("mirror");
                assert!(
                    (fp.rest.x + r.rest.x).abs() < 1e-4,
                    "{} not mirrored",
                    fp.name
                );
                assert!(
                    (fp.rest.y - r.rest.y).abs() < 1e-4,
                    "{} y mismatch",
                    fp.name
                );
            }
        }
    }

    #[test]
    fn blink_closes_the_eye() {
        let rig = FaceRig::new();
        let open = rig.eye_aperture_l(&[]);
        let shut = rig.eye_aperture_l(&[(Expression::Blink, 1.0)]);
        assert!(
            shut < open * 0.25,
            "blink should mostly close: {open} -> {shut}"
        );
    }

    #[test]
    fn smile_lifts_mouth_corners() {
        let rig = FaceRig::new();
        let rest = rig.rest("mouth_corner_l").unwrap();
        let smiled = rig
            .solve_point("mouth_corner_l", &[(Expression::Smile, 1.0)])
            .unwrap();
        assert!(smiled.y > rest.y + 0.1, "corner should rise");
    }

    #[test]
    fn rigid_skull_points_never_move() {
        let rig = FaceRig::new();
        let rest = rig.rest("zygomatic_l").unwrap();
        let moved = rig
            .solve_point(
                "zygomatic_l",
                &[(Expression::Smile, 1.0), (Expression::JawOpen, 1.0)],
            )
            .unwrap();
        assert!((rest - moved).length() < 1e-6);
    }

    #[test]
    fn weights_blend_linearly() {
        let rig = FaceRig::new();
        let full = rig
            .solve_point("mouth_corner_l", &[(Expression::Smile, 1.0)])
            .unwrap();
        let half = rig
            .solve_point("mouth_corner_l", &[(Expression::Smile, 0.5)])
            .unwrap();
        let rest = rig.rest("mouth_corner_l").unwrap();
        let mid = rest.lerp(full, 0.5);
        assert!((half - mid).length() < 1e-5);
    }
}
