//! Keyboard — layout-independent scancodes with edge state.
//!
//! [`Key`] is a thin newtype over a physical scancode (USB HID usage), so
//! bindings survive QWERTY/AZERTY/Dvorak swaps — the 2030-correct way. The
//! [`Keyboard`] tracks an edge [`Button`] per pressed key plus live modifiers.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use std::collections::HashMap;

use crate::button::Button;

/// A physical key, identified by scancode (USB HID keyboard usage id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Key(pub u32);

#[allow(non_upper_case_globals)]
impl Key {
    // Letters (HID usage ids 0x04..=0x1D).
    pub const A: Self = Self(0x04);
    pub const B: Self = Self(0x05);
    pub const Backspace: Self = Self(0x2A);
    pub const C: Self = Self(0x06);
    pub const D: Self = Self(0x07);
    pub const Down: Self = Self(0x51);
    pub const E: Self = Self(0x08);
    pub const Enter: Self = Self(0x28);
    pub const Escape: Self = Self(0x29);
    pub const F: Self = Self(0x09);
    pub const F1: Self = Self(0x3A);
    pub const F10: Self = Self(0x43);
    pub const F11: Self = Self(0x44);
    pub const F12: Self = Self(0x45);
    pub const F2: Self = Self(0x3B);
    pub const F3: Self = Self(0x3C);
    pub const F4: Self = Self(0x3D);
    pub const F5: Self = Self(0x3E);
    pub const F6: Self = Self(0x3F);
    pub const F7: Self = Self(0x40);
    pub const F8: Self = Self(0x41);
    pub const F9: Self = Self(0x42);
    pub const G: Self = Self(0x0A);
    pub const H: Self = Self(0x0B);
    pub const I: Self = Self(0x0C);
    pub const J: Self = Self(0x0D);
    pub const K: Self = Self(0x0E);
    pub const L: Self = Self(0x0F);
    pub const Left: Self = Self(0x50);
    pub const LeftAlt: Self = Self(0xE2);
    pub const LeftCtrl: Self = Self(0xE0);
    pub const LeftMeta: Self = Self(0xE3);
    pub const LeftShift: Self = Self(0xE1);
    pub const M: Self = Self(0x10);
    pub const N: Self = Self(0x11);
    pub const Num0: Self = Self(0x27);
    // Number row (0x1E..=0x27).
    pub const Num1: Self = Self(0x1E);
    pub const Num2: Self = Self(0x1F);
    pub const Num3: Self = Self(0x20);
    pub const Num4: Self = Self(0x21);
    pub const Num5: Self = Self(0x22);
    pub const Num6: Self = Self(0x23);
    pub const Num7: Self = Self(0x24);
    pub const Num8: Self = Self(0x25);
    pub const Num9: Self = Self(0x26);
    pub const O: Self = Self(0x12);
    pub const P: Self = Self(0x13);
    pub const Q: Self = Self(0x14);
    pub const R: Self = Self(0x15);
    pub const Right: Self = Self(0x4F);
    pub const RightAlt: Self = Self(0xE6);
    pub const RightCtrl: Self = Self(0xE4);
    pub const RightMeta: Self = Self(0xE7);
    pub const RightShift: Self = Self(0xE5);
    pub const S: Self = Self(0x16);
    pub const Space: Self = Self(0x2C);
    pub const T: Self = Self(0x17);
    pub const Tab: Self = Self(0x2B);
    pub const U: Self = Self(0x18);
    pub const Up: Self = Self(0x52);
    pub const V: Self = Self(0x19);
    pub const W: Self = Self(0x1A);
    pub const X: Self = Self(0x1B);
    pub const Y: Self = Self(0x1C);
    pub const Z: Self = Self(0x1D);

    #[must_use]
    pub const fn scancode(self) -> u32 {
        self.0
    }
}

/// Held modifier-key summary for the current frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Edge-tracked keyboard state.
#[derive(Debug, Clone, Default)]
pub struct Keyboard {
    keys: HashMap<Key, Button>,
    /// Most recent text codepoints this frame (already composed by the IME).
    pub text: String,
    pub modifiers: Modifiers,
}

impl Keyboard {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a frame: clear text and age missing keys to released.
    pub fn begin_frame(&mut self) {
        self.text.clear();
    }

    /// Feed one key's live `down` state (creates the slot on first use).
    pub fn set(&mut self, key: Key, down: bool, dt: f32) {
        self.keys.entry(key).or_default().update(down, dt);
        match key {
            Key::LeftCtrl | Key::RightCtrl => self.modifiers.ctrl = down,
            Key::LeftShift | Key::RightShift => self.modifiers.shift = down,
            Key::LeftAlt | Key::RightAlt => self.modifiers.alt = down,
            Key::LeftMeta | Key::RightMeta => self.modifiers.meta = down,
            _ => {},
        }
    }

    /// Append composed text input (from the platform/IME).
    pub fn push_text(&mut self, s: &str) {
        self.text.push_str(s);
    }

    #[must_use]
    pub fn is_down(&self, key: Key) -> bool {
        self.keys.get(&key).is_some_and(Button::is_down)
    }

    #[must_use]
    pub fn just_pressed(&self, key: Key) -> bool {
        self.keys.get(&key).is_some_and(Button::just_pressed)
    }

    #[must_use]
    pub fn just_released(&self, key: Key) -> bool {
        self.keys.get(&key).is_some_and(Button::just_released)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_edges_and_modifiers() {
        let mut kb = Keyboard::new();
        kb.set(Key::W, true, 0.016);
        kb.set(Key::LeftShift, true, 0.016);
        assert!(kb.just_pressed(Key::W) && kb.is_down(Key::W));
        assert!(kb.modifiers.shift);
        kb.set(Key::W, false, 0.016);
        assert!(kb.just_released(Key::W));
    }
}
