//! Animation toolkit — easing curves, springs, tweens and a tiny animator.
//!
//! Everything is `f32`, allocation-free, and `no_std`-friendly in spirit. Drive
//! it by feeding elapsed time (`t` in 0..1 for easings) or `dt` (seconds) for
//! springs/animators.

use core::f32::consts::PI;

/// Standard easing curves (Penner-style), all mapping `t∈[0,1] → [0,1]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Easing {
    Linear,
    QuadIn,
    QuadOut,
    QuadInOut,
    CubicIn,
    CubicOut,
    CubicInOut,
    QuartOut,
    SineInOut,
    ExpoOut,
    BackOut,
    ElasticOut,
    BounceOut,
}

impl Easing {
    /// Parse a case-insensitive name (e.g. `"ease_out"`, `"elastic"`, `"bounce"`).
    pub fn from_name(s: &str) -> Easing {
        match s.to_ascii_lowercase().replace('-', "_").as_str() {
            "linear" => Easing::Linear,
            "quad_in" | "ease_in" => Easing::QuadIn,
            "quad_out" | "ease_out" => Easing::QuadOut,
            "quad_inout" | "ease" | "smooth" => Easing::QuadInOut,
            "cubic_in" => Easing::CubicIn,
            "cubic_out" => Easing::CubicOut,
            "cubic_inout" => Easing::CubicInOut,
            "quart_out" | "quartout" => Easing::QuartOut,
            "sine" | "sine_inout" => Easing::SineInOut,
            "expo" | "expo_out" => Easing::ExpoOut,
            "back" | "back_out" => Easing::BackOut,
            "elastic" | "elastic_out" => Easing::ElasticOut,
            "bounce" | "bounce_out" => Easing::BounceOut,
            _ => Easing::QuadInOut,
        }
    }

    /// Evaluate the curve. `t` is clamped to `[0,1]`.
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::QuadIn => t * t,
            Easing::QuadOut => 1.0 - (1.0 - t) * (1.0 - t),
            Easing::QuadInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                }
            },
            Easing::CubicIn => t * t * t,
            Easing::CubicOut => 1.0 - (1.0 - t).powi(3),
            Easing::CubicInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            },
            Easing::QuartOut => 1.0 - (1.0 - t).powi(4),
            Easing::SineInOut => -(PI * t).cos() * 0.5 + 0.5,
            Easing::ExpoOut => {
                if t >= 1.0 {
                    1.0
                } else {
                    1.0 - 2.0_f32.powf(-10.0 * t)
                }
            },
            Easing::BackOut => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
            },
            Easing::ElasticOut => {
                if t == 0.0 || t == 1.0 {
                    return t;
                }
                let c4 = (2.0 * PI) / 3.0;
                2.0_f32.powf(-10.0 * t) * ((t * 10.0 - 0.75) * c4).sin() + 1.0
            },
            Easing::BounceOut => bounce_out(t),
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

/// Convenience: ease a value between `a` and `b` by curve at parameter `t`.
pub fn ease(curve: Easing, a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * curve.apply(t)
}

/// A critically-dampable spring (implicit-ish) — great for snappy UI motion.
///
/// `stiffness` ≈ how hard it pulls toward the target, `damping` ≈ how fast it
/// settles. Step with `update(dt)` each frame.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub value: f32,
    pub target: f32,
    pub velocity: f32,
    pub stiffness: f32,
    pub damping: f32,
}

impl Spring {
    pub fn new(value: f32, stiffness: f32, damping: f32) -> Self {
        Self { value, target: value, velocity: 0.0, stiffness, damping }
    }

    /// A pleasant default for UI (snappy, slightly springy).
    pub fn ui(value: f32) -> Self {
        Self::new(value, 180.0, 18.0)
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Advance by `dt` seconds (sub-stepped for stability).
    pub fn update(&mut self, dt: f32) -> f32 {
        let steps = (dt / 0.004).ceil().max(1.0) as u32;
        let h = dt / steps as f32;
        for _ in 0..steps {
            let force = (self.target - self.value) * self.stiffness - self.velocity * self.damping;
            self.velocity += force * h;
            self.value += self.velocity * h;
        }
        self.value
    }

    pub fn is_settled(&self) -> bool {
        (self.target - self.value).abs() < 1e-3 && self.velocity.abs() < 1e-3
    }
}

/// A one-shot tween from `from`→`to` over `duration` seconds along a curve.
#[derive(Clone, Copy, Debug)]
pub struct Tween {
    pub from: f32,
    pub to: f32,
    pub duration: f32,
    pub elapsed: f32,
    pub curve: Easing,
}

impl Tween {
    pub fn new(from: f32, to: f32, duration: f32, curve: Easing) -> Self {
        Self { from, to, duration: duration.max(1e-4), elapsed: 0.0, curve }
    }

    /// Advance and return the current value.
    pub fn update(&mut self, dt: f32) -> f32 {
        self.elapsed = (self.elapsed + dt).min(self.duration);
        self.value()
    }

    pub fn value(&self) -> f32 {
        let t = self.elapsed / self.duration;
        ease(self.curve, self.from, self.to, t)
    }

    pub fn done(&self) -> bool {
        self.elapsed >= self.duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easings_pin_endpoints() {
        for c in [
            Easing::Linear,
            Easing::QuadInOut,
            Easing::CubicOut,
            Easing::ExpoOut,
            Easing::BackOut,
            Easing::ElasticOut,
            Easing::BounceOut,
            Easing::SineInOut,
        ] {
            assert!((c.apply(0.0) - 0.0).abs() < 1e-3, "{c:?} f(0)≠0");
            assert!((c.apply(1.0) - 1.0).abs() < 1e-3, "{c:?} f(1)≠1");
        }
    }

    #[test]
    fn from_name_aliases() {
        assert_eq!(Easing::from_name("ease-out"), Easing::QuadOut);
        assert_eq!(Easing::from_name("ELASTIC"), Easing::ElasticOut);
        assert_eq!(Easing::from_name("nonsense"), Easing::QuadInOut);
    }

    #[test]
    fn spring_settles_at_target() {
        let mut s = Spring::ui(0.0);
        s.set_target(1.0);
        for _ in 0..600 {
            s.update(1.0 / 60.0);
        }
        assert!(s.is_settled(), "value={} vel={}", s.value, s.velocity);
        assert!((s.value - 1.0).abs() < 1e-2);
    }

    #[test]
    fn tween_runs_to_completion() {
        let mut tw = Tween::new(10.0, 20.0, 0.5, Easing::CubicOut);
        assert!(!tw.done());
        for _ in 0..60 {
            tw.update(1.0 / 60.0);
        }
        assert!(tw.done());
        assert!((tw.value() - 20.0).abs() < 1e-3);
    }
}
