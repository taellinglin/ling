//! Easing curves and a small `Lerp` trait.
//!
//! Standalone (no graphics dependency) so the animation core stays lean and
//! `wasm`-safe. The curve set mirrors the one in `ling-graphics::animation` so
//! the two can be bridged later without surprises.

use glam::{Quat, Vec2, Vec3, Vec4};

/// Anything that can be interpolated between two values.
pub trait Lerp: Clone {
    fn lerp_by(&self, other: &Self, t: f32) -> Self;
}

impl Lerp for f32 {
    fn lerp_by(&self, o: &Self, t: f32) -> Self {
        self + (o - self) * t
    }
}
impl Lerp for f64 {
    fn lerp_by(&self, o: &Self, t: f32) -> Self {
        self + (o - self) * t as f64
    }
}
impl Lerp for Vec2 {
    fn lerp_by(&self, o: &Self, t: f32) -> Self {
        self.lerp(*o, t)
    }
}
impl Lerp for Vec3 {
    fn lerp_by(&self, o: &Self, t: f32) -> Self {
        self.lerp(*o, t)
    }
}
impl Lerp for Vec4 {
    fn lerp_by(&self, o: &Self, t: f32) -> Self {
        self.lerp(*o, t)
    }
}
impl Lerp for Quat {
    fn lerp_by(&self, o: &Self, t: f32) -> Self {
        self.slerp(*o, t)
    }
}

/// Named easing functions covering the common in/out/in-out families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EaseFunction {
    Linear,
    Step,
    QuadIn,
    QuadOut,
    QuadInOut,
    CubicIn,
    CubicOut,
    CubicInOut,
    SineIn,
    SineOut,
    SineInOut,
    ExpoIn,
    ExpoOut,
    ExpoInOut,
    ElasticIn,
    ElasticOut,
    BackIn,
    BackOut,
    BackInOut,
    BounceIn,
    BounceOut,
}

impl EaseFunction {
    /// Parse a snake/kebab/space-separated name (case-insensitive). Unknown → Linear.
    pub fn from_name(name: &str) -> Self {
        let n: String = name
            .to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        match n.as_str() {
            "linear" => Self::Linear,
            "step" => Self::Step,
            "quadin" => Self::QuadIn,
            "quadout" => Self::QuadOut,
            "quadinout" => Self::QuadInOut,
            "cubicin" => Self::CubicIn,
            "cubicout" => Self::CubicOut,
            "cubicinout" | "smooth" => Self::CubicInOut,
            "sinein" => Self::SineIn,
            "sineout" => Self::SineOut,
            "sineinout" => Self::SineInOut,
            "expoin" => Self::ExpoIn,
            "expoout" => Self::ExpoOut,
            "expoinout" => Self::ExpoInOut,
            "elasticin" => Self::ElasticIn,
            "elasticout" | "elastic" => Self::ElasticOut,
            "backin" => Self::BackIn,
            "backout" | "overshoot" => Self::BackOut,
            "backinout" => Self::BackInOut,
            "bouncein" => Self::BounceIn,
            "bounceout" | "bounce" => Self::BounceOut,
            _ => Self::Linear,
        }
    }

    /// Evaluate the curve at `t`, clamped to `[0, 1]`.
    pub fn apply(self, t: f32) -> f32 {
        use std::f32::consts::{FRAC_PI_2, PI};
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::Step => {
                if t < 1.0 {
                    0.0
                } else {
                    1.0
                }
            },
            Self::QuadIn => t * t,
            Self::QuadOut => t * (2.0 - t),
            Self::QuadInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    -1.0 + (4.0 - 2.0 * t) * t
                }
            },
            Self::CubicIn => t * t * t,
            Self::CubicOut => {
                let s = t - 1.0;
                s * s * s + 1.0
            },
            Self::CubicInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    (t - 1.0) * (2.0 * t - 2.0) * (2.0 * t - 2.0) + 1.0
                }
            },
            Self::SineIn => 1.0 - (t * FRAC_PI_2).cos(),
            Self::SineOut => (t * FRAC_PI_2).sin(),
            Self::SineInOut => 0.5 * (1.0 - (PI * t).cos()),
            Self::ExpoIn => {
                if t == 0.0 {
                    0.0
                } else {
                    2.0_f32.powf(10.0 * t - 10.0)
                }
            },
            Self::ExpoOut => {
                if t == 1.0 {
                    1.0
                } else {
                    1.0 - 2.0_f32.powf(-10.0 * t)
                }
            },
            Self::ExpoInOut => {
                if t == 0.0 {
                    0.0
                } else if t == 1.0 {
                    1.0
                } else if t < 0.5 {
                    2.0_f32.powf(20.0 * t - 10.0) / 2.0
                } else {
                    (2.0 - 2.0_f32.powf(-20.0 * t + 10.0)) / 2.0
                }
            },
            Self::ElasticIn => {
                let c = 2.0 * PI / 3.0;
                if t == 0.0 {
                    0.0
                } else if t == 1.0 {
                    1.0
                } else {
                    -2.0_f32.powf(10.0 * t - 10.0) * ((10.0 * t - 10.75) * c).sin()
                }
            },
            Self::ElasticOut => {
                let c = 2.0 * PI / 3.0;
                if t == 0.0 {
                    0.0
                } else if t == 1.0 {
                    1.0
                } else {
                    2.0_f32.powf(-10.0 * t) * ((10.0 * t - 0.75) * c).sin() + 1.0
                }
            },
            Self::BackIn => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                c3 * t * t * t - c1 * t * t
            },
            Self::BackOut => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
            },
            Self::BackInOut => {
                let c2 = 1.70158 * 1.525;
                if t < 0.5 {
                    ((2.0 * t).powi(2) * ((c2 + 1.0) * 2.0 * t - c2)) / 2.0
                } else {
                    ((2.0 * t - 2.0).powi(2) * ((c2 + 1.0) * (2.0 * t - 2.0) + c2) + 2.0) / 2.0
                }
            },
            Self::BounceOut => bounce_out(t),
            Self::BounceIn => 1.0 - bounce_out(1.0 - t),
        }
    }
}

fn bounce_out(t: f32) -> f32 {
    let n1 = 7.5625;
    let d1 = 2.75;
    if t < 1.0 / d1 {
        n1 * t * t
    } else if t < 2.0 / d1 {
        let t = t - 1.5 / d1;
        n1 * t * t + 0.75
    } else if t < 2.5 / d1 {
        let t = t - 2.25 / d1;
        n1 * t * t + 0.9375
    } else {
        let t = t - 2.625 / d1;
        n1 * t * t + 0.984375
    }
}

/// Eased interpolation `a → b` by a named curve.
pub fn tween_ease<T: Lerp>(a: &T, b: &T, t: f32, ease: EaseFunction) -> T {
    a.lerp_by(b, ease.apply(t))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoints_are_fixed() {
        for f in [
            EaseFunction::CubicInOut,
            EaseFunction::BounceOut,
            EaseFunction::ElasticOut,
            EaseFunction::BackInOut,
            EaseFunction::ExpoInOut,
            EaseFunction::SineInOut,
        ] {
            assert!((f.apply(0.0)).abs() < 1e-4, "{f:?} f(0)");
            assert!((f.apply(1.0) - 1.0).abs() < 1e-4, "{f:?} f(1)");
        }
    }
    #[test]
    fn from_name_aliases() {
        assert_eq!(
            EaseFunction::from_name("cubic-in-out"),
            EaseFunction::CubicInOut
        );
        assert_eq!(EaseFunction::from_name("Bounce"), EaseFunction::BounceOut);
        assert_eq!(EaseFunction::from_name("???"), EaseFunction::Linear);
    }
}
