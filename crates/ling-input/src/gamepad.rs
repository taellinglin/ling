//! Vendor-neutral gamepad model — a standard layout plus a per-frame snapshot.
//!
//! Buttons use Xbox/standard *positional* names ([`GamepadButton::South`] etc.)
//! so logic never assumes a vendor; the [`Layout`] enum maps those positions to
//! the right printed glyph (A/B/X/Y, ✕/◯/▢/△, B/A/Y/X) for prompts and the
//! on-screen themer. Modern extras — back paddles, capture button, touchpad —
//! are first-class so 2030 controllers map cleanly.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use glam::Vec2;

use crate::button::Button;

/// A button position on the standard layout (vendor-neutral).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum GamepadButton {
    /// Bottom face (A / ✕ / B).
    South,
    /// Right face (B / ◯ / A).
    East,
    /// Left face (X / ▢ / Y).
    West,
    /// Top face (Y / △ / X).
    North,
    LeftShoulder,
    RightShoulder,
    /// Trigger pulled past its digital point (analog value lives in [`Gamepad`]).
    LeftTrigger,
    RightTrigger,
    /// Back / Select / Share / View.
    Select,
    /// Start / Options / Menu.
    Start,
    /// Guide / Home / PS / Xbox.
    Guide,
    /// Left stick click (L3).
    LeftStick,
    /// Right stick click (R3).
    RightStick,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    /// Rear paddles (Elite / DualSense Edge / pro controllers).
    LeftPaddle1,
    LeftPaddle2,
    RightPaddle1,
    RightPaddle2,
    /// Touchpad click (DualSense / DualShock 4).
    Touchpad,
    /// Capture / Share / Mic / misc.
    Misc,
}

impl GamepadButton {
    /// Every button in declaration (index) order.
    pub const ALL: [Self; Self::COUNT] = [
        Self::South,
        Self::East,
        Self::West,
        Self::North,
        Self::LeftShoulder,
        Self::RightShoulder,
        Self::LeftTrigger,
        Self::RightTrigger,
        Self::Select,
        Self::Start,
        Self::Guide,
        Self::LeftStick,
        Self::RightStick,
        Self::DpadUp,
        Self::DpadDown,
        Self::DpadLeft,
        Self::DpadRight,
        Self::LeftPaddle1,
        Self::LeftPaddle2,
        Self::RightPaddle1,
        Self::RightPaddle2,
        Self::Touchpad,
        Self::Misc,
    ];
    /// Number of distinct button positions.
    pub const COUNT: usize = 23;

    /// Dense index into a `[_; COUNT]` array.
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// An analog axis exposed by a standard gamepad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum GamepadAxis {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    LeftTrigger,
    RightTrigger,
}

/// Which physical stick, for binding helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum GamepadStick {
    Left,
    Right,
}

/// Vendor label set, for rendering the correct face-button glyphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Layout {
    Xbox,
    PlayStation,
    Nintendo,
    #[default]
    Generic,
}

impl Layout {
    /// The printed glyph for a button position on this layout.
    #[must_use]
    pub fn glyph(self, b: GamepadButton) -> &'static str {
        use GamepadButton as B;
        match (self, b) {
            (Self::Xbox, B::South) => "A",
            (Self::Xbox, B::East) => "B",
            (Self::Xbox, B::West) => "X",
            (Self::Xbox, B::North) => "Y",
            (Self::PlayStation, B::South) => "✕",
            (Self::PlayStation, B::East) => "◯",
            (Self::PlayStation, B::West) => "▢",
            (Self::PlayStation, B::North) => "△",
            (Self::Nintendo, B::South) => "B",
            (Self::Nintendo, B::East) => "A",
            (Self::Nintendo, B::West) => "Y",
            (Self::Nintendo, B::North) => "X",
            (Self::Generic, B::South) => "S",
            (Self::Generic, B::East) => "E",
            (Self::Generic, B::West) => "W",
            (Self::Generic, B::North) => "N",
            (_, B::DpadUp) => "▲",
            (_, B::DpadDown) => "▼",
            (_, B::DpadLeft) => "◀",
            (_, B::DpadRight) => "▶",
            (_, B::LeftShoulder) => "LB",
            (_, B::RightShoulder) => "RB",
            (_, B::LeftTrigger) => "LT",
            (_, B::RightTrigger) => "RT",
            (_, B::LeftStick) => "L3",
            (_, B::RightStick) => "R3",
            (_, B::Start) => "≡",
            (_, B::Select) => "⧉",
            (_, B::Guide) => "⌂",
            (_, B::Touchpad) => "▭",
            (_, B::Misc) => "●",
            (_, B::LeftPaddle1) => "P1",
            (_, B::LeftPaddle2) => "P2",
            (_, B::RightPaddle1) => "P3",
            (_, B::RightPaddle2) => "P4",
        }
    }
}

/// A full per-frame snapshot of one gamepad.
///
/// Sticks and triggers are already deadzoned/curved values (apply your
/// [`crate::axis::Stick`]/[`crate::axis::Axis`] when ingesting raw hardware).
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Gamepad {
    pub left_stick: Vec2,
    pub right_stick: Vec2,
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub layout: Layout,
    pub connected: bool,
    buttons: [Button; GamepadButton::COUNT],
    /// Live held state fed by events; advanced into `buttons` by [`Gamepad::tick`].
    raw: [bool; GamepadButton::COUNT],
}

impl Default for Gamepad {
    fn default() -> Self {
        Self {
            left_stick: Vec2::ZERO,
            right_stick: Vec2::ZERO,
            left_trigger: 0.0,
            right_trigger: 0.0,
            layout: Layout::Generic,
            connected: false,
            buttons: [Button::new(); GamepadButton::COUNT],
            raw: [false; GamepadButton::COUNT],
        }
    }
}

impl Gamepad {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Shared access to a button's edge state.
    #[must_use]
    pub fn button(&self, b: GamepadButton) -> &Button {
        &self.buttons[b.index()]
    }

    /// Mutable access to a button's edge state.
    pub fn button_mut(&mut self, b: GamepadButton) -> &mut Button {
        &mut self.buttons[b.index()]
    }

    /// Polling style: feed one button's live `down` state and advance its edge
    /// immediately. Use this when you read the whole pad every frame.
    pub fn set_button(&mut self, b: GamepadButton, down: bool, dt: f32) {
        self.raw[b.index()] = down;
        self.buttons[b.index()].update(down, dt);
    }

    /// Event style: record a button's held state without advancing its edge.
    /// Pair with [`Gamepad::tick`] once per frame.
    pub fn set_held(&mut self, b: GamepadButton, down: bool) {
        self.raw[b.index()] = down;
    }

    /// Advance every button's edge state from the recorded held bits. Call once
    /// per frame when driving the pad from [`crate::event::InputEvent`]s.
    pub fn tick(&mut self, dt: f32) {
        for (i, b) in self.buttons.iter_mut().enumerate() {
            b.update(self.raw[i], dt);
        }
    }

    #[must_use]
    pub fn is_down(&self, b: GamepadButton) -> bool {
        self.buttons[b.index()].is_down()
    }

    #[must_use]
    pub fn just_pressed(&self, b: GamepadButton) -> bool {
        self.buttons[b.index()].just_pressed()
    }

    #[must_use]
    pub fn just_released(&self, b: GamepadButton) -> bool {
        self.buttons[b.index()].just_released()
    }

    /// Write a named axis (event-driven ingest).
    pub fn set_axis(&mut self, a: GamepadAxis, v: f32) {
        match a {
            GamepadAxis::LeftStickX => self.left_stick.x = v,
            GamepadAxis::LeftStickY => self.left_stick.y = v,
            GamepadAxis::RightStickX => self.right_stick.x = v,
            GamepadAxis::RightStickY => self.right_stick.y = v,
            GamepadAxis::LeftTrigger => self.left_trigger = v,
            GamepadAxis::RightTrigger => self.right_trigger = v,
        }
    }

    /// Read a named axis.
    #[must_use]
    pub fn axis(&self, a: GamepadAxis) -> f32 {
        match a {
            GamepadAxis::LeftStickX => self.left_stick.x,
            GamepadAxis::LeftStickY => self.left_stick.y,
            GamepadAxis::RightStickX => self.right_stick.x,
            GamepadAxis::RightStickY => self.right_stick.y,
            GamepadAxis::LeftTrigger => self.left_trigger,
            GamepadAxis::RightTrigger => self.right_trigger,
        }
    }

    #[must_use]
    pub fn stick(&self, s: GamepadStick) -> Vec2 {
        match s {
            GamepadStick::Left => self.left_stick,
            GamepadStick::Right => self.right_stick,
        }
    }

    /// D-pad as a vector (`+y` down, screen convention).
    #[must_use]
    pub fn dpad(&self) -> Vec2 {
        let x = f32::from(self.is_down(GamepadButton::DpadRight))
            - f32::from(self.is_down(GamepadButton::DpadLeft));
        let y = f32::from(self.is_down(GamepadButton::DpadDown))
            - f32::from(self.is_down(GamepadButton::DpadUp));
        Vec2::new(x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_indices_are_dense_and_unique() {
        for (i, b) in GamepadButton::ALL.iter().enumerate() {
            assert_eq!(i, b.index());
        }
    }

    #[test]
    fn button_edges_round_trip() {
        let mut g = Gamepad::new();
        g.set_button(GamepadButton::South, true, 0.016);
        assert!(g.just_pressed(GamepadButton::South));
        g.set_button(GamepadButton::South, true, 0.016);
        assert!(g.is_down(GamepadButton::South) && !g.just_pressed(GamepadButton::South));
    }

    #[test]
    fn glyphs_differ_by_layout() {
        assert_eq!(Layout::Xbox.glyph(GamepadButton::South), "A");
        assert_eq!(Layout::Nintendo.glyph(GamepadButton::South), "B");
        assert_eq!(Layout::PlayStation.glyph(GamepadButton::East), "◯");
    }
}
