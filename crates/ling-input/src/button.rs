//! Digital input — edge-detected button state with key-repeat.
//!
//! A [`Button`] is fed a raw `down` flag each tick and remembers the previous
//! frame so it can report *edges* (`just_pressed` / `just_released`), how long
//! it has been held, and a menu-friendly auto-repeat. Every higher-level
//! control (gamepad faces, on-screen buttons, VR triggers, keyboard keys)
//! reuses this one primitive so edge semantics are identical everywhere.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// One logical button tracked across two frames.
///
/// Construct with [`Button::new`], optionally [`Button::with_repeat`], then call
/// [`Button::update`] once per frame with the live `down` state and frame `dt`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Button {
    down: bool,
    prev: bool,
    held_for: f32,
    repeat_delay: f32,
    repeat_interval: f32,
    repeat_acc: f32,
    repeat_fired: bool,
}

impl Button {
    /// A released button with auto-repeat disabled.
    pub const fn new() -> Self {
        Self {
            down: false,
            prev: false,
            held_for: 0.0,
            repeat_delay: 0.0,
            repeat_interval: 0.0,
            repeat_acc: 0.0,
            repeat_fired: false,
        }
    }

    /// Enable auto-repeat: after `delay` seconds held, [`repeated`](Self::repeated)
    /// fires every `interval` seconds (and once on the initial press). Great for
    /// menu navigation and text cursors.
    pub const fn with_repeat(mut self, delay: f32, interval: f32) -> Self {
        self.repeat_delay = delay;
        self.repeat_interval = interval;
        self
    }

    /// Advance one frame from the live `down` state and the elapsed `dt`.
    pub fn update(&mut self, down: bool, dt: f32) {
        self.prev = self.down;
        self.down = down;
        self.repeat_fired = false;

        if down {
            if self.prev {
                self.held_for += dt;
                if self.repeat_delay > 0.0 && self.held_for >= self.repeat_delay {
                    self.repeat_acc += dt;
                    if self.repeat_interval <= 0.0 {
                        self.repeat_fired = true;
                    } else {
                        while self.repeat_acc >= self.repeat_interval {
                            self.repeat_acc -= self.repeat_interval;
                            self.repeat_fired = true;
                        }
                    }
                }
            } else {
                // rising edge
                self.held_for = 0.0;
                self.repeat_acc = 0.0;
                self.repeat_fired = self.repeat_delay > 0.0;
            }
        } else {
            self.held_for = 0.0;
            self.repeat_acc = 0.0;
        }
    }

    /// Convenience: treat an analog reading as a button via a threshold.
    pub fn update_analog(&mut self, value: f32, threshold: f32, dt: f32) {
        self.update(value >= threshold, dt);
    }

    /// Currently held.
    pub const fn is_down(&self) -> bool {
        self.down
    }
    /// Currently released.
    pub const fn is_up(&self) -> bool {
        !self.down
    }
    /// Went down this frame (rising edge).
    pub const fn just_pressed(&self) -> bool {
        self.down && !self.prev
    }
    /// Went up this frame (falling edge).
    pub const fn just_released(&self) -> bool {
        !self.down && self.prev
    }
    /// Seconds the button has been continuously held (`0` while up).
    pub const fn held_for(&self) -> f32 {
        self.held_for
    }
    /// `true` on the initial press and on each auto-repeat tick thereafter.
    /// Always `false` unless [`with_repeat`](Self::with_repeat) was set.
    pub const fn repeated(&self) -> bool {
        self.repeat_fired
    }
}

impl Default for Button {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges() {
        let mut b = Button::new();
        b.update(true, 0.016);
        assert!(b.just_pressed() && b.is_down() && !b.just_released());
        b.update(true, 0.016);
        assert!(!b.just_pressed() && b.is_down());
        b.update(false, 0.016);
        assert!(b.just_released() && b.is_up());
    }

    #[test]
    fn repeat_fires_on_press_then_interval() {
        let mut b = Button::new().with_repeat(0.30, 0.10);
        b.update(true, 0.016); // initial press
        assert!(b.repeated());
        // hold until past the delay, then accumulate one interval
        let mut fires = 0;
        for _ in 0..40 {
            b.update(true, 0.016);
            if b.repeated() {
                fires += 1;
            }
        }
        assert!(fires >= 2, "expected several repeats, got {fires}");
    }
}
