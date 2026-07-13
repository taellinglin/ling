//! Keyframe tracks and a playable timeline.

use crate::ease::{EaseFunction, Lerp};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Keyframe<T> {
    pub time: f32,
    pub value: T,
    pub ease: EaseFunction,
}

/// A sorted list of keyframes of one channel, sampleable at any time.
#[derive(Debug, Clone)]
pub struct Track<T: Lerp> {
    keyframes: Vec<Keyframe<T>>,
}

impl<T: Lerp> Track<T> {
    pub fn new() -> Self {
        Self { keyframes: Vec::new() }
    }

    /// Add a keyframe (kept time-sorted). Chainable.
    pub fn key(mut self, time: f32, value: T, ease: EaseFunction) -> Self {
        self.keyframes.push(Keyframe { time, value, ease });
        self.keyframes.sort_by(|a, b| {
            a.time
                .partial_cmp(&b.time)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self
    }

    pub fn is_empty(&self) -> bool {
        self.keyframes.is_empty()
    }

    pub fn duration(&self) -> f32 {
        self.keyframes.last().map(|k| k.time).unwrap_or(0.0)
    }

    /// Sample the track at `t`, easing into the next key. Clamps at the ends.
    pub fn sample(&self, t: f32) -> Option<T> {
        let ks = &self.keyframes;
        if ks.is_empty() {
            return None;
        }
        if t <= ks[0].time {
            return Some(ks[0].value.clone());
        }
        let last = ks.last().unwrap();
        if t >= last.time {
            return Some(last.value.clone());
        }
        for w in ks.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            if t >= a.time && t <= b.time {
                let local = (t - a.time) / (b.time - a.time);
                return Some(a.value.lerp_by(&b.value, a.ease.apply(local)));
            }
        }
        None
    }
}

impl<T: Lerp> Default for Track<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// A clock that advances time, loops, and reports normalized progress.
#[derive(Debug, Clone)]
pub struct Timeline {
    pub time: f32,
    pub speed: f32,
    pub playing: bool,
    pub looping: bool,
    pub duration: f32,
}

impl Timeline {
    pub fn new(duration: f32) -> Self {
        Self {
            time: 0.0,
            speed: 1.0,
            playing: true,
            looping: true,
            duration: duration.max(0.0),
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

    /// Advance by `dt` seconds, honoring speed/loop. Returns the new time.
    pub fn tick(&mut self, dt: f32) -> f32 {
        if self.playing {
            self.time += dt * self.speed;
            if self.duration > 0.0 {
                if self.looping {
                    self.time = self.time.rem_euclid(self.duration);
                } else if self.time >= self.duration {
                    self.time = self.duration;
                    self.playing = false;
                }
            }
        }
        self.time
    }

    pub fn normalized(&self) -> f32 {
        if self.duration <= 0.0 {
            0.0
        } else {
            (self.time / self.duration).clamp(0.0, 1.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn track_samples_and_clamps() {
        let t = Track::new().key(0.0, 0.0f32, EaseFunction::Linear).key(
            1.0,
            10.0,
            EaseFunction::Linear,
        );
        assert_eq!(t.sample(-1.0), Some(0.0));
        assert_eq!(t.sample(0.5), Some(5.0));
        assert_eq!(t.sample(2.0), Some(10.0));
        assert_eq!(t.duration(), 1.0);
    }
    #[test]
    fn timeline_loops() {
        let mut tl = Timeline::new(1.0);
        tl.tick(0.75);
        let w = tl.tick(0.5); // 1.25 -> wraps to 0.25
        assert!((w - 0.25).abs() < 1e-5);
    }
}
