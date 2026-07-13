//! On-screen touch controls — virtual sticks, buttons, and d-pads.
//!
//! These widgets consume a [`TouchPool`] and synthesize a standard
//! [`Gamepad`], so the *rest of the game never knows the difference* between a
//! thumb on glass and a real controller. Each widget carries geometry; a
//! [`ControlTheme`] supplies the look (this crate emits data, not pixels).
//! Sticks support **fixed** and **floating** placement — the modern mobile feel
//! where the stick spawns under your thumb.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use glam::Vec2;

use crate::axis::Stick;
use crate::button::Button;
use crate::gamepad::{Gamepad, GamepadButton, GamepadStick, Layout};
use crate::theme::ControlTheme;
use crate::touch::{TouchId, TouchPool};

/// An axis-aligned rectangle in screen pixels (`+y` down).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Rect {
    pub min: Vec2,
    pub max: Vec2,
}

impl Rect {
    #[must_use]
    pub fn from_center_size(center: Vec2, size: Vec2) -> Self {
        let h = size * 0.5;
        Self { min: center - h, max: center + h }
    }

    #[must_use]
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    #[must_use]
    pub fn center(&self) -> Vec2 {
        (self.min + self.max) * 0.5
    }
}

/// Placement behaviour of a [`VirtualStick`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum StickMode {
    /// Base stays at `anchor`; the touch must start inside `zone`.
    Fixed,
    /// Base spawns wherever the touch lands inside `zone` ("floating" stick).
    Floating,
}

/// Live, non-serialized runtime of a stick.
#[derive(Debug, Clone, Copy, Default)]
pub struct StickRuntime {
    pub touch: Option<TouchId>,
    /// Where the base currently sits (center of travel).
    pub base: Vec2,
    /// Where the knob currently sits.
    pub knob: Vec2,
    /// Clean output vector (`-1..=1`, `+y` down).
    pub value: Vec2,
    pub active: bool,
    /// `0` idle .. `1` active, eased for theme fades.
    pub blend: f32,
}

/// A virtual thumb-stick that fills one [`GamepadStick`].
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VirtualStick {
    pub bind: GamepadStick,
    pub mode: StickMode,
    /// Resting base (also fallback render position for `Floating`).
    pub anchor: Vec2,
    /// Region a touch must begin within to grab this stick.
    pub zone: Rect,
    /// Travel radius in pixels.
    pub radius: f32,
    /// Visual knob radius in pixels.
    pub knob_radius: f32,
    /// Shaping applied to the normalized vector.
    pub shaping: Stick,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub rt: StickRuntime,
}

impl VirtualStick {
    /// A stick anchored at `anchor` with the given travel `radius`.
    #[must_use]
    pub fn new(bind: GamepadStick, anchor: Vec2, radius: f32) -> Self {
        let zone = Rect::from_center_size(anchor, Vec2::splat(radius * 3.0));
        Self {
            bind,
            mode: StickMode::Fixed,
            anchor,
            zone,
            radius,
            knob_radius: radius * 0.4,
            shaping: Stick::default(),
            rt: StickRuntime { base: anchor, knob: anchor, ..StickRuntime::default() },
        }
    }

    #[must_use]
    pub fn floating(mut self) -> Self {
        self.mode = StickMode::Floating;
        self
    }

    /// Recompute from the touch pool.
    pub fn update(&mut self, pool: &TouchPool, dt: f32) {
        // Drop a lost touch.
        if let Some(id) = self.rt.touch {
            if pool.get(id).is_none_or(|t| !t.is_active()) {
                self.rt.touch = None;
            }
        }

        // Acquire a new touch starting in the zone.
        if self.rt.touch.is_none() {
            if let Some(t) = pool.active().find(|t| self.zone.contains(t.start)) {
                self.rt.touch = Some(t.id);
                self.rt.base = match self.mode {
                    StickMode::Fixed => self.anchor,
                    StickMode::Floating => t.start,
                };
            }
        }

        if let Some(id) = self.rt.touch {
            if let Some(t) = pool.get(id) {
                let raw = (t.pos - self.rt.base) / self.radius.max(1.0);
                self.rt.value = self.shaping.vector(raw);
                let clamped = if raw.length() > 1.0 {
                    raw.normalize()
                } else {
                    raw
                };
                self.rt.knob = self.rt.base + clamped * self.radius;
                self.rt.active = true;
            }
        } else {
            self.rt.value = Vec2::ZERO;
            self.rt.active = false;
            if self.mode == StickMode::Fixed {
                self.rt.knob = self.anchor;
                self.rt.base = self.anchor;
            }
        }

        let target = if self.rt.active { 1.0 } else { 0.0 };
        let step = if dt > 0.0 { (dt / 0.2).min(1.0) } else { 1.0 };
        self.rt.blend += (target - self.rt.blend) * step;
    }
}

/// A virtual button mapped to a [`GamepadButton`].
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VirtualButton {
    pub bind: GamepadButton,
    pub label: String,
    pub center: Vec2,
    pub radius: f32,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub button: Button,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub touch: Option<TouchId>,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub blend: f32,
}

impl VirtualButton {
    #[must_use]
    pub fn new(bind: GamepadButton, center: Vec2, radius: f32, label: impl Into<String>) -> Self {
        Self {
            bind,
            label: label.into(),
            center,
            radius,
            button: Button::new(),
            touch: None,
            blend: 0.0,
        }
    }

    fn update(&mut self, pool: &TouchPool, dt: f32) {
        // A button is pressed if any active touch sits within its disc.
        let held = pool.pick(self.center, self.radius).is_some();
        self.button.update(held, dt);
        let target = if held { 1.0 } else { 0.0 };
        let step = if dt > 0.0 { (dt / 0.12).min(1.0) } else { 1.0 };
        self.blend += (target - self.blend) * step;
    }
}

/// A four-way virtual d-pad mapped to the four `Dpad*` buttons.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct VirtualDpad {
    pub center: Vec2,
    pub radius: f32,
    /// Inner dead radius (no direction registered).
    pub dead: f32,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub up: Button,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub down: Button,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub left: Button,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub right: Button,
}

impl VirtualDpad {
    #[must_use]
    pub fn new(center: Vec2, radius: f32) -> Self {
        Self {
            center,
            radius,
            dead: radius * 0.25,
            up: Button::new(),
            down: Button::new(),
            left: Button::new(),
            right: Button::new(),
        }
    }

    fn update(&mut self, pool: &TouchPool, dt: f32) {
        let mut u = false;
        let mut d = false;
        let mut l = false;
        let mut r = false;
        if let Some(t) = pool.pick(self.center, self.radius) {
            let v = t.pos - self.center;
            if v.length() >= self.dead {
                // 8-way: activate any axis beyond ~22 degrees of the diagonal.
                if v.x.abs() > self.dead * 0.4 {
                    r = v.x > 0.0;
                    l = v.x < 0.0;
                }
                if v.y.abs() > self.dead * 0.4 {
                    d = v.y > 0.0;
                    u = v.y < 0.0;
                }
            }
        }
        self.up.update(u, dt);
        self.down.update(d, dt);
        self.left.update(l, dt);
        self.right.update(r, dt);
    }
}

/// A complete on-screen control surface that produces a [`Gamepad`].
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct OnScreenControls {
    pub sticks: Vec<VirtualStick>,
    pub buttons: Vec<VirtualButton>,
    pub dpads: Vec<VirtualDpad>,
    pub theme: ControlTheme,
    /// Glyph set for button labels.
    pub layout: Layout,
    /// Whether the surface is shown at all (auto-hidden when a real pad is used).
    pub enabled: bool,
    #[cfg_attr(feature = "serde", serde(skip))]
    pad: Gamepad,
}

impl OnScreenControls {
    #[must_use]
    pub fn new() -> Self {
        Self { enabled: true, ..Self::default() }
    }

    /// A sensible twin-stick layout for a screen of `size` pixels.
    #[must_use]
    pub fn twin_stick(size: Vec2) -> Self {
        let m = size.y * 0.18; // margin
        let r = size.y * 0.13; // stick radius
        let mut s = Self::new();
        s.sticks.push(
            VirtualStick::new(GamepadStick::Left, Vec2::new(m + r, size.y - m - r), r).floating(),
        );
        s.sticks.push(VirtualStick::new(
            GamepadStick::Right,
            Vec2::new(size.x - m - r, size.y - m - r),
            r,
        ));
        let br = size.y * 0.07;
        let bx = size.x - m;
        let by = size.y * 0.45;
        s.buttons.push(VirtualButton::new(
            GamepadButton::South,
            Vec2::new(bx - br * 2.2, by + br * 2.2),
            br,
            "A",
        ));
        s.buttons.push(VirtualButton::new(
            GamepadButton::East,
            Vec2::new(bx, by),
            br,
            "B",
        ));
        s.buttons.push(VirtualButton::new(
            GamepadButton::West,
            Vec2::new(bx - br * 4.4, by),
            br,
            "X",
        ));
        s.buttons.push(VirtualButton::new(
            GamepadButton::North,
            Vec2::new(bx - br * 2.2, by - br * 2.2),
            br,
            "Y",
        ));
        s
    }

    /// Update every widget against the touch pool and refresh the gamepad.
    pub fn update(&mut self, pool: &TouchPool, dt: f32) {
        if !self.enabled {
            return;
        }
        self.pad.connected = true;
        self.pad.layout = self.layout;

        for s in &mut self.sticks {
            s.update(pool, dt);
            match s.bind {
                GamepadStick::Left => self.pad.left_stick = s.rt.value,
                GamepadStick::Right => self.pad.right_stick = s.rt.value,
            }
        }
        for b in &mut self.buttons {
            b.update(pool, dt);
            self.pad.set_button(b.bind, b.button.is_down(), dt);
            if b.bind == GamepadButton::LeftTrigger {
                self.pad.left_trigger = f32::from(b.button.is_down());
            } else if b.bind == GamepadButton::RightTrigger {
                self.pad.right_trigger = f32::from(b.button.is_down());
            }
        }
        for d in &mut self.dpads {
            d.update(pool, dt);
            self.pad
                .set_button(GamepadButton::DpadUp, d.up.is_down(), dt);
            self.pad
                .set_button(GamepadButton::DpadDown, d.down.is_down(), dt);
            self.pad
                .set_button(GamepadButton::DpadLeft, d.left.is_down(), dt);
            self.pad
                .set_button(GamepadButton::DpadRight, d.right.is_down(), dt);
        }
    }

    /// The synthesized gamepad — drive game logic from this.
    #[must_use]
    pub fn gamepad(&self) -> &Gamepad {
        &self.pad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floating_stick_spawns_under_thumb_and_reports_vector() {
        let mut pool = TouchPool::new();
        let mut s =
            VirtualStick::new(GamepadStick::Left, Vec2::new(200.0, 600.0), 100.0).floating();
        // Touch starts inside the zone, off the anchor.
        let start = Vec2::new(150.0, 620.0);
        pool.begin(1, start, 1.0);
        s.update(&pool, 0.016);
        assert_eq!(s.rt.base, start); // floating base moved to the thumb
                                      // Drag right -> +x output.
        pool.begin_frame();
        pool.moved(1, start + Vec2::new(80.0, 0.0), 1.0);
        s.update(&pool, 0.016);
        assert!(s.rt.value.x > 0.3 && s.rt.value.y.abs() < 0.2);
    }

    #[test]
    fn buttons_drive_gamepad() {
        let mut osc = OnScreenControls::twin_stick(Vec2::new(1280.0, 720.0));
        let b = osc.buttons[1].center; // "B"
        let mut pool = TouchPool::new();
        pool.begin(9, b, 1.0);
        osc.update(&pool, 0.016);
        assert!(osc.gamepad().is_down(GamepadButton::East));
    }
}
