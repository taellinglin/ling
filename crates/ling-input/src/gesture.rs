//! Touch gestures — tap, double-tap, long-press, swipe, pinch, rotate.
//!
//! A [`GestureRecognizer`] watches a [`TouchPool`] across frames and emits
//! discrete [`Gesture`] events. Single-finger gestures (tap / long-press /
//! swipe) are tracked per touch; two-finger gestures (pinch / rotate) report
//! *incremental* change each frame so they compose smoothly with a camera.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use std::collections::HashMap;

use glam::Vec2;

use crate::touch::{TouchId, TouchPool};

/// A cardinal swipe direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum SwipeDir {
    Up,
    Down,
    Left,
    Right,
}

/// A recognized gesture (positions in pixels, `+y` down).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Gesture {
    Tap { pos: Vec2 },
    DoubleTap { pos: Vec2 },
    LongPress { pos: Vec2 },
    Swipe { start: Vec2, end: Vec2, dir: SwipeDir },
    /// `scale` is the multiplicative change *this frame* (≈1.0 = no change).
    Pinch { center: Vec2, scale: f32 },
    /// `radians` is the rotation *this frame* (signed, screen CW positive).
    Rotate { center: Vec2, radians: f32 },
}

#[derive(Debug, Clone, Copy)]
struct Track {
    start: Vec2,
    start_t: f32,
    long_fired: bool,
}

#[derive(Debug, Clone, Copy)]
struct TwoFinger {
    a: TouchId,
    b: TouchId,
    dist: f32,
    angle: f32,
}

/// Stateful recognizer. Tune the thresholds, then call [`update`](Self::update)
/// each frame and read [`events`](Self::events).
#[derive(Debug, Clone)]
pub struct GestureRecognizer {
    /// Max seconds for a press to count as a tap.
    pub tap_time: f32,
    /// Max pixel drift for a tap / long-press.
    pub tap_dist: f32,
    /// Seconds held (without moving) to fire a long-press.
    pub long_time: f32,
    /// Min pixel travel to count as a swipe.
    pub swipe_dist: f32,
    /// Max seconds between taps to chain a double-tap.
    pub double_time: f32,
    clock: f32,
    tracks: HashMap<TouchId, Track>,
    two: Option<TwoFinger>,
    last_tap: Option<(f32, Vec2)>,
    out: Vec<Gesture>,
}

impl Default for GestureRecognizer {
    fn default() -> Self {
        Self {
            tap_time: 0.30,
            tap_dist: 16.0,
            long_time: 0.55,
            swipe_dist: 60.0,
            double_time: 0.30,
            clock: 0.0,
            tracks: HashMap::new(),
            two: None,
            last_tap: None,
            out: Vec::new(),
        }
    }
}

impl GestureRecognizer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Gestures recognized on the most recent [`update`](Self::update).
    #[must_use]
    pub fn events(&self) -> &[Gesture] {
        &self.out
    }

    /// Advance one frame against the live touch pool.
    pub fn update(&mut self, pool: &TouchPool, dt: f32) {
        self.clock += dt;
        self.out.clear();

        self.update_single(pool);
        self.update_two_finger(pool);
    }

    fn update_single(&mut self, pool: &TouchPool) {
        // Register new touches.
        for t in pool.active() {
            self.tracks.entry(t.id).or_insert(Track {
                start: t.start,
                start_t: self.clock,
                long_fired: false,
            });
        }

        // Long-press: held in place beyond the threshold (only when alone).
        if pool.active_count() == 1 {
            if let Some(t) = pool.active().next() {
                if let Some(tr) = self.tracks.get_mut(&t.id) {
                    let held = self.clock - tr.start_t;
                    if !tr.long_fired
                        && held >= self.long_time
                        && t.drift().length() <= self.tap_dist
                    {
                        tr.long_fired = true;
                        self.out.push(Gesture::LongPress { pos: t.pos });
                    }
                }
            }
        }

        // Finalize touches that ended/cancelled (or vanished) this frame.
        let ended: Vec<TouchId> = self
            .tracks
            .keys()
            .copied()
            .filter(|id| pool.get(*id).is_none_or(|t| !t.is_active()))
            .collect();

        for id in ended {
            let tr = self.tracks.remove(&id).unwrap();
            let Some(t) = pool.get(id) else { continue };
            let held = self.clock - tr.start_t;
            let drift = t.drift();

            if drift.length() >= self.swipe_dist {
                self.out.push(Gesture::Swipe {
                    start: tr.start,
                    end: t.pos,
                    dir: swipe_dir(drift),
                });
            } else if held <= self.tap_time && drift.length() <= self.tap_dist && !tr.long_fired {
                let is_double = self
                    .last_tap
                    .is_some_and(|(lt, lp)| self.clock - lt <= self.double_time && lp.distance(t.pos) <= self.tap_dist * 2.0);
                if is_double {
                    self.last_tap = None;
                    self.out.push(Gesture::DoubleTap { pos: t.pos });
                } else {
                    self.last_tap = Some((self.clock, t.pos));
                    self.out.push(Gesture::Tap { pos: t.pos });
                }
            }
        }
    }

    fn update_two_finger(&mut self, pool: &TouchPool) {
        let pts: Vec<_> = pool.active().take(3).collect();
        if pts.len() != 2 {
            self.two = None;
            return;
        }
        let (p0, p1) = (pts[0], pts[1]);
        let d = p1.pos - p0.pos;
        let dist = d.length().max(1e-3);
        let angle = d.y.atan2(d.x);
        let center = (p0.pos + p1.pos) * 0.5;

        match self.two {
            Some(prev) if prev.a == p0.id && prev.b == p1.id => {
                let scale = dist / prev.dist.max(1e-3);
                if (scale - 1.0).abs() > 1e-3 {
                    self.out.push(Gesture::Pinch { center, scale });
                }
                let mut dr = angle - prev.angle;
                // wrap to [-pi, pi]
                if dr > std::f32::consts::PI {
                    dr -= std::f32::consts::TAU;
                } else if dr < -std::f32::consts::PI {
                    dr += std::f32::consts::TAU;
                }
                if dr.abs() > 1e-4 {
                    self.out.push(Gesture::Rotate { center, radians: dr });
                }
            },
            _ => {},
        }
        self.two = Some(TwoFinger { a: p0.id, b: p1.id, dist, angle });
    }
}

fn swipe_dir(d: Vec2) -> SwipeDir {
    if d.x.abs() >= d.y.abs() {
        if d.x >= 0.0 {
            SwipeDir::Right
        } else {
            SwipeDir::Left
        }
    } else if d.y >= 0.0 {
        SwipeDir::Down
    } else {
        SwipeDir::Up
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_press_is_a_tap() {
        let mut pool = TouchPool::new();
        let mut g = GestureRecognizer::new();
        pool.begin(1, Vec2::new(100.0, 100.0), 1.0);
        g.update(&pool, 0.016);
        pool.begin_frame();
        pool.end(1);
        g.update(&pool, 0.10);
        assert!(matches!(g.events().first(), Some(Gesture::Tap { .. })));
    }

    #[test]
    fn long_hold_is_a_long_press() {
        let mut pool = TouchPool::new();
        let mut g = GestureRecognizer::new();
        pool.begin(1, Vec2::new(50.0, 50.0), 1.0);
        for _ in 0..60 {
            g.update(&pool, 0.016);
            pool.begin_frame();
        }
        assert!(g.events().is_empty() || true); // long-press fired during the loop
        // Re-run to confirm a long press was emitted at some point.
        let mut pool2 = TouchPool::new();
        let mut g2 = GestureRecognizer::new();
        pool2.begin(7, Vec2::new(50.0, 50.0), 1.0);
        let mut saw = false;
        for _ in 0..60 {
            g2.update(&pool2, 0.016);
            saw |= g2.events().iter().any(|e| matches!(e, Gesture::LongPress { .. }));
            pool2.begin_frame();
        }
        assert!(saw);
    }
}
