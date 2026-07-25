//! Generic HID joystick / flight stick / arcade stick / racing wheel.
//!
//! Where [`crate::gamepad::Gamepad`] assumes a standard layout, real HID
//! devices expose an *arbitrary* set of axes, buttons, and POV hat switches —
//! a HOTAS throttle quadrant, a fight-stick, a racing wheel with pedals. This
//! model keeps them generic: index into [`Joystick::axes`] / `buttons` / `hats`.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::button::Button;

/// A point-of-view hat switch (8-way + centered).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Hat {
    #[default]
    Centered,
    Up,
    UpRight,
    Right,
    DownRight,
    Down,
    DownLeft,
    Left,
    UpLeft,
}

impl Hat {
    /// Direction as an `(x, y)` step, `+y` down (screen convention).
    #[must_use]
    pub const fn vector(self) -> (i8, i8) {
        match self {
            Self::Centered => (0, 0),
            Self::Up => (0, -1),
            Self::UpRight => (1, -1),
            Self::Right => (1, 0),
            Self::DownRight => (1, 1),
            Self::Down => (0, 1),
            Self::DownLeft => (-1, 1),
            Self::Left => (-1, 0),
            Self::UpLeft => (-1, -1),
        }
    }

    /// Quantize a clockwise-from-up degrees reading (HID convention) to 8-way.
    /// Pass `None` for centered.
    #[must_use]
    pub fn from_degrees(deg: Option<f32>) -> Self {
        let Some(d) = deg else { return Self::Centered };
        let sector = (((d.rem_euclid(360.0)) + 22.5) / 45.0) as i32 % 8;
        match sector {
            0 => Self::Up,
            1 => Self::UpRight,
            2 => Self::Right,
            3 => Self::DownRight,
            4 => Self::Down,
            5 => Self::DownLeft,
            6 => Self::Left,
            _ => Self::UpLeft,
        }
    }
}

/// A generic multi-axis HID controller.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Joystick {
    pub name: String,
    /// Calibrated axis values, each `-1..=1` (or `0..=1` for one-sided pedals).
    pub axes: Vec<f32>,
    pub buttons: Vec<Button>,
    pub hats: Vec<Hat>,
    pub connected: bool,
}

impl Joystick {
    /// Allocate a joystick with the given counts (all neutral).
    #[must_use]
    pub fn with_layout(name: impl Into<String>, axes: usize, buttons: usize, hats: usize) -> Self {
        Self {
            name: name.into(),
            axes: vec![0.0; axes],
            buttons: vec![Button::new(); buttons],
            hats: vec![Hat::Centered; hats],
            connected: true,
        }
    }

    #[must_use]
    pub fn axis(&self, i: usize) -> f32 {
        self.axes.get(i).copied().unwrap_or(0.0)
    }

    #[must_use]
    pub fn button(&self, i: usize) -> Option<&Button> {
        self.buttons.get(i)
    }

    #[must_use]
    pub fn hat(&self, i: usize) -> Hat {
        self.hats.get(i).copied().unwrap_or(Hat::Centered)
    }

    /// Advance every button's edge state for this frame from a `down` source.
    pub fn update_buttons(&mut self, down: impl Fn(usize) -> bool, dt: f32) {
        for (i, b) in self.buttons.iter_mut().enumerate() {
            b.update(down(i), dt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hat_quantization() {
        assert_eq!(Hat::from_degrees(None), Hat::Centered);
        assert_eq!(Hat::from_degrees(Some(0.0)), Hat::Up);
        assert_eq!(Hat::from_degrees(Some(90.0)), Hat::Right);
        assert_eq!(Hat::from_degrees(Some(225.0)), Hat::DownLeft);
        assert_eq!(Hat::from_degrees(Some(359.0)), Hat::Up);
    }

    #[test]
    fn layout_alloc() {
        let j = Joystick::with_layout("HOTAS", 6, 24, 1);
        assert_eq!(j.axes.len(), 6);
        assert_eq!(j.buttons.len(), 24);
        assert_eq!(j.hat(0), Hat::Centered);
    }
}
