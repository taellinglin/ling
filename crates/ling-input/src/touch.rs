//! Raw multi-touch — a pool of tracked touch points with phase and pressure.
//!
//! This is the substrate for everything mobile: [`gesture`](crate::gesture)
//! recognition and [`onscreen`](crate::onscreen) virtual controls both read a
//! [`TouchPool`]. Feed it `begin` / `move` / `end` events from the platform;
//! call [`TouchPool::begin_frame`] each tick to retire finished touches and age
//! `Moved` into `Stationary`.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use glam::Vec2;

/// Stable per-finger identifier supplied by the platform.
pub type TouchId = u64;

/// Lifecycle phase of a touch within the current frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum TouchPhase {
    Began,
    Moved,
    Stationary,
    Ended,
    Cancelled,
}

/// A single tracked contact point (coordinates in pixels, `+y` down).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Touch {
    pub id: TouchId,
    pub pos: Vec2,
    /// Position when the touch began.
    pub start: Vec2,
    /// Position on the previous frame.
    pub prev: Vec2,
    /// Normalized pressure `0..=1` (`1` if the device has no force sensor).
    pub pressure: f32,
    pub phase: TouchPhase,
}

impl Touch {
    /// Movement since last frame.
    #[must_use]
    pub fn delta(&self) -> Vec2 {
        self.pos - self.prev
    }
    /// Total movement since the touch began.
    #[must_use]
    pub fn drift(&self) -> Vec2 {
        self.pos - self.start
    }
    #[must_use]
    pub fn is_active(&self) -> bool {
        !matches!(self.phase, TouchPhase::Ended | TouchPhase::Cancelled)
    }
}

/// The set of touches currently known to the system.
#[derive(Debug, Clone, Default)]
pub struct TouchPool {
    touches: Vec<Touch>,
}

impl TouchPool {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Retire finished touches and reset per-frame phases. Call once per tick
    /// *before* feeding the frame's events.
    pub fn begin_frame(&mut self) {
        self.touches.retain(Touch::is_active);
        for t in &mut self.touches {
            t.prev = t.pos;
            t.phase = TouchPhase::Stationary;
        }
    }

    /// A finger touched down.
    pub fn begin(&mut self, id: TouchId, pos: Vec2, pressure: f32) {
        // Replace any stale entry with the same id.
        self.touches.retain(|t| t.id != id);
        self.touches.push(Touch {
            id,
            pos,
            start: pos,
            prev: pos,
            pressure,
            phase: TouchPhase::Began,
        });
    }

    /// A finger moved.
    pub fn moved(&mut self, id: TouchId, pos: Vec2, pressure: f32) {
        if let Some(t) = self.get_mut(id) {
            t.pos = pos;
            t.pressure = pressure;
            if t.phase != TouchPhase::Began {
                t.phase = TouchPhase::Moved;
            }
        }
    }

    /// A finger lifted.
    pub fn end(&mut self, id: TouchId) {
        if let Some(t) = self.get_mut(id) {
            t.phase = TouchPhase::Ended;
        }
    }

    /// A touch was cancelled (e.g. a system gesture took over).
    pub fn cancel(&mut self, id: TouchId) {
        if let Some(t) = self.get_mut(id) {
            t.phase = TouchPhase::Cancelled;
        }
    }

    #[must_use]
    pub fn get(&self, id: TouchId) -> Option<&Touch> {
        self.touches.iter().find(|t| t.id == id)
    }
    pub fn get_mut(&mut self, id: TouchId) -> Option<&mut Touch> {
        self.touches.iter_mut().find(|t| t.id == id)
    }

    /// All touches (including those that ended this frame).
    pub fn iter(&self) -> impl Iterator<Item = &Touch> {
        self.touches.iter()
    }

    /// Touches that are still down.
    pub fn active(&self) -> impl Iterator<Item = &Touch> {
        self.touches.iter().filter(|t| t.is_active())
    }

    #[must_use]
    pub fn active_count(&self) -> usize {
        self.active().count()
    }

    /// Nearest still-active touch to `p` within `radius`, if any.
    #[must_use]
    pub fn pick(&self, p: Vec2, radius: f32) -> Option<&Touch> {
        self.active()
            .filter(|t| t.pos.distance(p) <= radius)
            .min_by(|a, b| {
                a.pos
                    .distance_squared(p)
                    .total_cmp(&b.pos.distance_squared(p))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle() {
        let mut p = TouchPool::new();
        p.begin(1, Vec2::new(10.0, 10.0), 1.0);
        assert_eq!(p.active_count(), 1);
        p.begin_frame();
        p.moved(1, Vec2::new(20.0, 10.0), 1.0);
        assert_eq!(p.get(1).unwrap().delta(), Vec2::new(10.0, 0.0));
        p.end(1);
        // ended touch still visible this frame, gone after next begin_frame
        assert_eq!(p.active_count(), 0);
        p.begin_frame();
        assert!(p.get(1).is_none());
    }
}
