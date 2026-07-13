use crate::color::Color;
use crate::scene::Transform;
use glam::{Quat, Vec2, Vec3, Vec4};

// ── Lerp trait ────────────────────────────────────────────────────────────────

pub trait Lerp: Clone + Send + Sync + 'static {
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
impl Lerp for Color {
    fn lerp_by(&self, o: &Self, t: f32) -> Self {
        self.lerp(*o, t)
    }
}
impl Lerp for Transform {
    fn lerp_by(&self, o: &Self, t: f32) -> Self {
        self.lerp(o, t)
    }
}

// ── Ease functions ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
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
    BounceOut,
    BounceIn,
}

impl EaseFunction {
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            EaseFunction::Linear => t,
            EaseFunction::Step => {
                if t < 1.0 {
                    0.0
                } else {
                    1.0
                }
            },
            EaseFunction::QuadIn => t * t,
            EaseFunction::QuadOut => t * (2.0 - t),
            EaseFunction::QuadInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    -1.0 + (4.0 - 2.0 * t) * t
                }
            },
            EaseFunction::CubicIn => t * t * t,
            EaseFunction::CubicOut => {
                let s = t - 1.0;
                s * s * s + 1.0
            },
            EaseFunction::CubicInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    (t - 1.0) * (2.0 * t - 2.0) * (2.0 * t - 2.0) + 1.0
                }
            },
            EaseFunction::SineIn => 1.0 - ((t * std::f32::consts::FRAC_PI_2).cos()),
            EaseFunction::SineOut => (t * std::f32::consts::FRAC_PI_2).sin(),
            EaseFunction::SineInOut => 0.5 * (1.0 - (std::f32::consts::PI * t).cos()),
            EaseFunction::ExpoIn => {
                if t == 0.0 {
                    0.0
                } else {
                    (2.0_f32).powf(10.0 * t - 10.0)
                }
            },
            EaseFunction::ExpoOut => {
                if t == 1.0 {
                    1.0
                } else {
                    1.0 - (2.0_f32).powf(-10.0 * t)
                }
            },
            EaseFunction::ExpoInOut => {
                if t == 0.0 {
                    return 0.0;
                }
                if t == 1.0 {
                    return 1.0;
                }
                if t < 0.5 {
                    (2.0_f32).powf(20.0 * t - 10.0) / 2.0
                } else {
                    (2.0 - (2.0_f32).powf(-20.0 * t + 10.0)) / 2.0
                }
            },
            EaseFunction::ElasticIn => {
                let c = 2.0 * std::f32::consts::PI / 3.0;
                if t == 0.0 {
                    0.0
                } else if t == 1.0 {
                    1.0
                } else {
                    -(2.0_f32).powf(10.0 * t - 10.0) * ((10.0 * t - 10.75) * c).sin()
                }
            },
            EaseFunction::ElasticOut => {
                let c = 2.0 * std::f32::consts::PI / 3.0;
                if t == 0.0 {
                    0.0
                } else if t == 1.0 {
                    1.0
                } else {
                    (2.0_f32).powf(-10.0 * t) * ((10.0 * t - 0.75) * c).sin() + 1.0
                }
            },
            EaseFunction::BackIn => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                c3 * t * t * t - c1 * t * t
            },
            EaseFunction::BackOut => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
            },
            EaseFunction::BackInOut => {
                let c2 = 1.70158 * 1.525;
                if t < 0.5 {
                    ((2.0 * t).powi(2) * ((c2 + 1.0) * 2.0 * t - c2)) / 2.0
                } else {
                    ((2.0 * t - 2.0).powi(2) * ((c2 + 1.0) * (2.0 * t - 2.0) + c2) + 2.0) / 2.0
                }
            },
            EaseFunction::BounceOut => bounce_out(t),
            EaseFunction::BounceIn => 1.0 - bounce_out(1.0 - t),
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

// ── Keyframe & Track ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Keyframe<T: Lerp> {
    pub time: f32,
    pub value: T,
    pub ease: EaseFunction,
}

pub struct Track<T: Lerp> {
    pub keyframes: Vec<Keyframe<T>>,
}

impl<T: Lerp> Track<T> {
    pub fn new() -> Self {
        Self { keyframes: Vec::new() }
    }

    pub fn add(mut self, time: f32, value: T, ease: EaseFunction) -> Self {
        self.keyframes.push(Keyframe { time, value, ease });
        self.keyframes
            .sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap());
        self
    }

    pub fn sample(&self, t: f32) -> Option<T> {
        if self.keyframes.is_empty() {
            return None;
        }
        if t <= self.keyframes[0].time {
            return Some(self.keyframes[0].value.clone());
        }
        let last = self.keyframes.last().unwrap();
        if t >= last.time {
            return Some(last.value.clone());
        }
        for i in 0..self.keyframes.len() - 1 {
            let kf0 = &self.keyframes[i];
            let kf1 = &self.keyframes[i + 1];
            if t >= kf0.time && t <= kf1.time {
                let local = (t - kf0.time) / (kf1.time - kf0.time);
                let eased = kf0.ease.apply(local);
                return Some(kf0.value.lerp_by(&kf1.value, eased));
            }
        }
        None
    }

    pub fn duration(&self) -> f32 {
        self.keyframes.last().map(|k| k.time).unwrap_or(0.0)
    }
}

impl<T: Lerp> Default for Track<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ── Timeline ──────────────────────────────────────────────────────────────────

/// Named animation clips, played by name.
#[derive(Debug, Default)]
pub struct Timeline {
    pub time: f32,
    pub playing: bool,
    pub looping: bool,
    pub speed: f32,
    duration: f32,
}

impl Timeline {
    pub fn new(duration: f32) -> Self {
        Self {
            time: 0.0,
            playing: false,
            looping: false,
            speed: 1.0,
            duration,
        }
    }

    pub fn play(&mut self) {
        self.playing = true;
    }

    pub fn pause(&mut self) {
        self.playing = false;
    }

    pub fn stop(&mut self) {
        self.playing = false;
        self.time = 0.0;
    }

    pub fn tick(&mut self, dt: f32) {
        if !self.playing {
            return;
        }
        self.time += dt * self.speed;
        if self.time >= self.duration {
            if self.looping {
                self.time = self.time % self.duration;
            } else {
                self.time = self.duration;
                self.playing = false;
            }
        }
    }

    pub fn normalized_time(&self) -> f32 {
        if self.duration <= 0.0 {
            return 0.0;
        }
        (self.time / self.duration).clamp(0.0, 1.0)
    }
}
