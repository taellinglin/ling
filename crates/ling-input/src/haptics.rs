//! Force feedback — rumble, adaptive triggers, and time-sampled haptic effects.
//!
//! The 灵↔机 axis shows up here too: a [`Rumble`] command is the *mechanical*
//! end (raw motor amplitudes), while a [`HapticEffect`] is the *organic* end —
//! a little envelope of feeling you play back over time (a click, a heartbeat,
//! a ramp). [`HapticPlayer`] turns an effect into per-frame `Rumble` so a
//! backend only ever has to apply one simple command.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// An instantaneous force-feedback command, each channel `0..=1`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Rumble {
    /// Low-frequency / heavy motor (the deep "thud").
    pub low: f32,
    /// High-frequency / light motor (the fine "buzz").
    pub high: f32,
    /// Left trigger motor amplitude (controllers with trigger haptics).
    pub left_trigger: f32,
    /// Right trigger motor amplitude.
    pub right_trigger: f32,
}

impl Rumble {
    pub const OFF: Self = Self { low: 0.0, high: 0.0, left_trigger: 0.0, right_trigger: 0.0 };

    /// Both main motors at the same strength.
    #[must_use]
    pub const fn both(strength: f32) -> Self {
        Self {
            low: strength,
            high: strength,
            left_trigger: 0.0,
            right_trigger: 0.0,
        }
    }

    /// Scale every channel.
    #[must_use]
    pub fn scaled(self, k: f32) -> Self {
        Self {
            low: self.low * k,
            high: self.high * k,
            left_trigger: self.left_trigger * k,
            right_trigger: self.right_trigger * k,
        }
    }

    /// Channel-wise max — combine two simultaneous effects without clipping past 1.
    #[must_use]
    pub fn max(self, o: Self) -> Self {
        Self {
            low: self.low.max(o.low),
            high: self.high.max(o.high),
            left_trigger: self.left_trigger.max(o.left_trigger),
            right_trigger: self.right_trigger.max(o.right_trigger),
        }
    }

    #[must_use]
    pub fn is_silent(self) -> bool {
        self.low <= 0.0 && self.high <= 0.0 && self.left_trigger <= 0.0 && self.right_trigger <= 0.0
    }
}

/// One keyframe of a haptic envelope.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct HapticKey {
    /// Time in seconds from effect start.
    pub t: f32,
    pub low: f32,
    pub high: f32,
}

/// A reusable, serializable haptic envelope sampled over time.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct HapticEffect {
    pub keys: Vec<HapticKey>,
    pub looping: bool,
}

impl HapticEffect {
    /// Total duration (time of the last keyframe).
    #[must_use]
    pub fn duration(&self) -> f32 {
        self.keys.last().map_or(0.0, |k| k.t)
    }

    /// Linearly sample the envelope at `t` seconds.
    #[must_use]
    pub fn sample(&self, t: f32) -> Rumble {
        if self.keys.is_empty() {
            return Rumble::OFF;
        }
        let dur = self.duration();
        let t = if self.looping && dur > 0.0 {
            t.rem_euclid(dur)
        } else {
            t
        };
        if t <= self.keys[0].t {
            let k = self.keys[0];
            return Rumble { low: k.low, high: k.high, ..Rumble::OFF };
        }
        for pair in self.keys.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if t <= b.t {
                let span = (b.t - a.t).max(f32::EPSILON);
                let f = ((t - a.t) / span).clamp(0.0, 1.0);
                return Rumble {
                    low: a.low + (b.low - a.low) * f,
                    high: a.high + (b.high - a.high) * f,
                    ..Rumble::OFF
                };
            }
        }
        let k = *self.keys.last().unwrap();
        Rumble { low: k.low, high: k.high, ..Rumble::OFF }
    }

    /// A short sharp tap.
    #[must_use]
    pub fn click(strength: f32) -> Self {
        Self {
            keys: vec![
                HapticKey { t: 0.0, low: 0.0, high: strength },
                HapticKey { t: 0.04, low: 0.0, high: 0.0 },
            ],
            looping: false,
        }
    }

    /// A flat pulse of `strength` for `secs`.
    #[must_use]
    pub fn pulse(strength: f32, secs: f32) -> Self {
        Self {
            keys: vec![
                HapticKey { t: 0.0, low: strength, high: strength },
                HapticKey { t: secs, low: strength, high: strength },
                HapticKey { t: secs + 0.001, low: 0.0, high: 0.0 },
            ],
            looping: false,
        }
    }

    /// A two-thump heartbeat, optionally looping.
    #[must_use]
    pub fn heartbeat(looping: bool) -> Self {
        Self {
            keys: vec![
                HapticKey { t: 0.00, low: 0.9, high: 0.3 },
                HapticKey { t: 0.08, low: 0.0, high: 0.0 },
                HapticKey { t: 0.18, low: 0.6, high: 0.2 },
                HapticKey { t: 0.26, low: 0.0, high: 0.0 },
                HapticKey { t: 0.90, low: 0.0, high: 0.0 },
            ],
            looping,
        }
    }
}

/// Plays a [`HapticEffect`], advancing its clock and yielding a live [`Rumble`].
#[derive(Debug, Clone, Default)]
pub struct HapticPlayer {
    effect: HapticEffect,
    time: f32,
    playing: bool,
}

impl HapticPlayer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Start (or restart) an effect from the beginning.
    pub fn play(&mut self, effect: HapticEffect) {
        self.effect = effect;
        self.time = 0.0;
        self.playing = true;
    }

    pub fn stop(&mut self) {
        self.playing = false;
    }

    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Advance the clock and return the current command (`Rumble::OFF` when idle).
    pub fn tick(&mut self, dt: f32) -> Rumble {
        if !self.playing {
            return Rumble::OFF;
        }
        self.time += dt;
        if !self.effect.looping && self.time > self.effect.duration() {
            self.playing = false;
            return Rumble::OFF;
        }
        self.effect.sample(self.time)
    }
}

/// A DualSense-style adaptive-trigger profile (resistance the player *feels*).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum TriggerEffect {
    /// No added resistance.
    Off,
    /// Constant resistance once the trigger passes `start` (`0..1`).
    Resistance { start: f32, force: f32 },
    /// A "wall" between `start` and `end` (e.g. a gun's break point).
    Weapon { start: f32, end: f32, force: f32 },
    /// Vibrate the trigger at `freq` Hz with `amplitude` once past `start`.
    Vibration { start: f32, amplitude: f32, freq: f32 },
}

impl Default for TriggerEffect {
    fn default() -> Self {
        Self::Off
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_samples_and_completes() {
        let mut p = HapticPlayer::new();
        p.play(HapticEffect::pulse(0.5, 0.1));
        let r = p.tick(0.05);
        assert!(r.low > 0.0);
        // run past the end -> stops and goes silent
        for _ in 0..10 {
            p.tick(0.05);
        }
        assert!(!p.is_playing());
        assert!(p.tick(0.05).is_silent());
    }

    #[test]
    fn looping_wraps() {
        let e = HapticEffect::heartbeat(true);
        let a = e.sample(0.0);
        let b = e.sample(e.duration()); // wraps back to start
        assert_eq!(a, b);
    }
}
