//! Mouse & unified pointer — including pen pressure/tilt.
//!
//! [`Mouse`] is the classic relative+absolute pointer with a wheel and five
//! buttons. [`Pointer`] is the 2030 unification: mouse, pen, and touch all
//! arrive as the same struct (with pressure/tilt for styli) so UI code handles
//! one type.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use glam::Vec2;

use crate::button::Button;

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    /// Back / thumb 1.
    Back,
    /// Forward / thumb 2.
    Forward,
}

impl MouseButton {
    pub const COUNT: usize = 5;
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// Mouse state for the frame.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Mouse {
    /// Absolute position in pixels (`+y` down).
    pub pos: Vec2,
    /// Movement since last frame (raw, for camera look).
    pub delta: Vec2,
    /// Scroll this frame (`y` = vertical, `x` = horizontal).
    pub wheel: Vec2,
    /// Whether the OS cursor is captured/locked (relative mode).
    pub locked: bool,
    buttons: [Button; MouseButton::COUNT],
}

impl Default for Mouse {
    fn default() -> Self {
        Self {
            pos: Vec2::ZERO,
            delta: Vec2::ZERO,
            wheel: Vec2::ZERO,
            locked: false,
            buttons: [Button::new(); MouseButton::COUNT],
        }
    }
}

impl Mouse {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset per-frame deltas (call before feeding the frame's motion/wheel).
    pub fn begin_frame(&mut self) {
        self.delta = Vec2::ZERO;
        self.wheel = Vec2::ZERO;
    }

    pub fn move_to(&mut self, pos: Vec2) {
        self.delta += pos - self.pos;
        self.pos = pos;
    }
    /// Feed relative motion directly (locked / raw-input mode).
    pub fn move_by(&mut self, delta: Vec2) {
        self.delta += delta;
        self.pos += delta;
    }
    pub fn scroll(&mut self, by: Vec2) {
        self.wheel += by;
    }
    pub fn set_button(&mut self, b: MouseButton, down: bool, dt: f32) {
        self.buttons[b.index()].update(down, dt);
    }

    #[must_use]
    pub fn is_down(&self, b: MouseButton) -> bool {
        self.buttons[b.index()].is_down()
    }
    #[must_use]
    pub fn just_pressed(&self, b: MouseButton) -> bool {
        self.buttons[b.index()].just_pressed()
    }
    #[must_use]
    pub fn just_released(&self, b: MouseButton) -> bool {
        self.buttons[b.index()].just_released()
    }
}

/// What kind of device produced a [`Pointer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum PointerKind {
    Mouse,
    Pen,
    Touch,
    Eraser,
}

/// A unified pointer sample (mouse / pen / touch share this).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Pointer {
    pub kind: PointerKind,
    pub pos: Vec2,
    /// `0..=1`; `1` for mouse, real force for pen/touch.
    pub pressure: f32,
    /// Stylus tilt in radians `(x, y)` from vertical (`0` for mouse/touch).
    pub tilt: Vec2,
    /// `true` for the primary contact (the one that emulates the mouse).
    pub primary: bool,
}

impl Pointer {
    /// A plain mouse pointer at `pos`.
    #[must_use]
    pub fn mouse(pos: Vec2) -> Self {
        Self {
            kind: PointerKind::Mouse,
            pos,
            pressure: 1.0,
            tilt: Vec2::ZERO,
            primary: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_accumulates_delta() {
        let mut m = Mouse::new();
        m.begin_frame();
        m.move_to(Vec2::new(10.0, 0.0));
        m.move_to(Vec2::new(15.0, 0.0));
        assert_eq!(m.delta, Vec2::new(15.0, 0.0));
        m.set_button(MouseButton::Left, true, 0.016);
        assert!(m.just_pressed(MouseButton::Left));
    }
}
