//! Action mapping — bind devices to *intent*, not buttons.
//!
//! The 2030-standard input layer (Steam Input / Enhanced Input / Unity's new
//! system): game logic asks "is `Jump` pressed?" / "what's the `Move` vector?"
//! and never names a device. Bindings are data — serializable, remappable at
//! runtime, and grouped into [`ActionSet`]s (a.k.a. contexts) you push/pop as
//! the game switches between gameplay, menus, vehicles, etc.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use std::collections::HashMap;

use glam::Vec2;

use crate::button::Button;
use crate::gamepad::{Gamepad, GamepadAxis, GamepadButton, GamepadStick};
use crate::keyboard::{Key, Keyboard};
use crate::pointer::{Mouse, MouseButton};

/// Borrowed references to the devices an [`ActionMap`] resolves against.
#[derive(Default, Clone, Copy)]
pub struct Devices<'a> {
    pub gamepad: Option<&'a Gamepad>,
    pub keyboard: Option<&'a Keyboard>,
    pub mouse: Option<&'a Mouse>,
}

impl<'a> Devices<'a> {
    #[must_use]
    pub fn gamepad(g: &'a Gamepad) -> Self {
        Self { gamepad: Some(g), ..Self::default() }
    }

    #[must_use]
    pub fn with_keyboard(mut self, k: &'a Keyboard) -> Self {
        self.keyboard = Some(k);
        self
    }

    #[must_use]
    pub fn with_mouse(mut self, m: &'a Mouse) -> Self {
        self.mouse = Some(m);
        self
    }
}

/// A single physical source for a digital/analog action.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Binding {
    Pad(GamepadButton),
    /// One side of a gamepad axis past `threshold` (`dir` = +1 / -1).
    PadAxis {
        axis: GamepadAxis,
        dir: f32,
        threshold: f32,
    },
    Key(Key),
    Mouse(MouseButton),
}

impl Binding {
    /// Resolve to a digital `down` and an analog `0..=1` value.
    fn resolve(self, dev: &Devices) -> (bool, f32) {
        match self {
            Self::Pad(b) => dev.gamepad.map_or((false, 0.0), |g| {
                let down = g.is_down(b);
                (down, f32::from(down))
            }),
            Self::PadAxis { axis, dir, threshold } => dev.gamepad.map_or((false, 0.0), |g| {
                let v = g.axis(axis) * dir.signum();
                (v >= threshold, v.clamp(0.0, 1.0))
            }),
            Self::Key(k) => dev.keyboard.map_or((false, 0.0), |kb| {
                let down = kb.is_down(k);
                (down, f32::from(down))
            }),
            Self::Mouse(b) => dev.mouse.map_or((false, 0.0), |m| {
                let down = m.is_down(b);
                (down, f32::from(down))
            }),
        }
    }
}

/// A source for a 2-D (vector) action.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum VectorBinding {
    /// A whole gamepad stick.
    Stick(GamepadStick),
    /// The gamepad d-pad.
    Dpad,
    /// Four keys composited into a vector (`+y` down).
    Keys { up: Key, down: Key, left: Key, right: Key },
}

impl VectorBinding {
    fn resolve(self, dev: &Devices) -> Vec2 {
        match self {
            Self::Stick(s) => dev.gamepad.map_or(Vec2::ZERO, |g| g.stick(s)),
            Self::Dpad => dev.gamepad.map_or(Vec2::ZERO, Gamepad::dpad),
            Self::Keys { up, down, left, right } => dev.keyboard.map_or(Vec2::ZERO, |kb| {
                let x = f32::from(kb.is_down(right)) - f32::from(kb.is_down(left));
                let y = f32::from(kb.is_down(down)) - f32::from(kb.is_down(up));
                let v = Vec2::new(x, y);
                if v.length() > 1.0 {
                    v.normalize()
                } else {
                    v
                }
            }),
        }
    }
}

/// One logical action: any number of digital/analog bindings (OR'd) plus an
/// optional vector binding. Resolved state is read after [`ActionMap::update`].
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Action {
    pub bindings: Vec<Binding>,
    pub vector: Option<VectorBinding>,
    #[cfg_attr(feature = "serde", serde(skip))]
    button: Button,
    #[cfg_attr(feature = "serde", serde(skip))]
    value: f32,
    #[cfg_attr(feature = "serde", serde(skip))]
    vec: Vec2,
}

impl Action {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: add a digital/analog source.
    #[must_use]
    pub fn bind(mut self, b: Binding) -> Self {
        self.bindings.push(b);
        self
    }

    /// Builder: set the 2-D source.
    #[must_use]
    pub fn vector(mut self, v: VectorBinding) -> Self {
        self.vector = Some(v);
        self
    }

    fn update(&mut self, dev: &Devices, dt: f32) {
        let mut down = false;
        let mut value = 0.0_f32;
        for b in &self.bindings {
            let (d, v) = b.resolve(dev);
            down |= d;
            value = value.max(v);
        }
        self.button.update(down, dt);
        self.value = value;
        self.vec = self.vector.map_or(Vec2::ZERO, |v| v.resolve(dev));
    }

    #[must_use]
    pub fn pressed(&self) -> bool {
        self.button.is_down()
    }

    #[must_use]
    pub fn just_pressed(&self) -> bool {
        self.button.just_pressed()
    }

    #[must_use]
    pub fn just_released(&self) -> bool {
        self.button.just_released()
    }

    #[must_use]
    pub fn analog(&self) -> f32 {
        self.value
    }

    #[must_use]
    pub fn axis2d(&self) -> Vec2 {
        self.vec
    }
}

/// A named collection of actions resolved together (one "context").
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ActionMap {
    actions: HashMap<String, Action>,
}

impl ActionMap {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Define or replace an action.
    pub fn set(&mut self, name: impl Into<String>, action: Action) {
        self.actions.insert(name.into(), action);
    }

    /// Resolve every action against the current device state.
    pub fn update(&mut self, dev: &Devices, dt: f32) {
        for a in self.actions.values_mut() {
            a.update(dev, dt);
        }
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Action> {
        self.actions.get(name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Action> {
        self.actions.get_mut(name)
    }

    #[must_use]
    pub fn pressed(&self, name: &str) -> bool {
        self.get(name).is_some_and(Action::pressed)
    }

    #[must_use]
    pub fn just_pressed(&self, name: &str) -> bool {
        self.get(name).is_some_and(Action::just_pressed)
    }

    #[must_use]
    pub fn analog(&self, name: &str) -> f32 {
        self.get(name).map_or(0.0, Action::analog)
    }

    #[must_use]
    pub fn axis2d(&self, name: &str) -> Vec2 {
        self.get(name).map_or(Vec2::ZERO, Action::axis2d)
    }
}

/// A named action-map plus whether it currently blocks lower sets.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ActionSet {
    pub map: ActionMap,
    /// If `true`, this set consumes input so sets below it on the stack don't
    /// also fire (e.g. a modal menu over gameplay).
    pub blocking: bool,
}

/// A stack of contexts. Only active sets update; the top blocking set wins.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ActionSets {
    sets: HashMap<String, ActionSet>,
    /// Active context names, lowest-priority first.
    stack: Vec<String>,
}

impl ActionSets {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn define(&mut self, name: impl Into<String>, set: ActionSet) {
        self.sets.insert(name.into(), set);
    }

    /// Push a context to the top of the active stack.
    pub fn push(&mut self, name: impl Into<String>) {
        let name = name.into();
        self.stack.retain(|n| n != &name);
        self.stack.push(name);
    }

    /// Remove a context from the active stack.
    pub fn pop(&mut self, name: &str) {
        self.stack.retain(|n| n != name);
    }

    /// The active context names, top last.
    #[must_use]
    pub fn active(&self) -> &[String] {
        &self.stack
    }

    /// Update active sets from the top down, stopping after the first blocking
    /// set so shadowed contexts go quiet.
    pub fn update(&mut self, dev: &Devices, dt: f32) {
        let order: Vec<String> = self.stack.iter().rev().cloned().collect();
        let mut blocked = false;
        for name in order {
            if let Some(set) = self.sets.get_mut(&name) {
                if blocked {
                    // Keep edges fresh as "released" while shadowed.
                    set.map.update(&Devices::default(), dt);
                } else {
                    set.map.update(dev, dt);
                    if set.blocking {
                        blocked = true;
                    }
                }
            }
        }
    }

    #[must_use]
    pub fn map(&self, name: &str) -> Option<&ActionMap> {
        self.sets.get(name).map(|s| &s.map)
    }

    pub fn map_mut(&mut self, name: &str) -> Option<&mut ActionMap> {
        self.sets.get_mut(name).map(|s| &mut s.map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_or_of_sources() {
        let mut kb = Keyboard::new();
        let mut map = ActionMap::new();
        map.set(
            "jump",
            Action::new()
                .bind(Binding::Key(Key::Space))
                .bind(Binding::Pad(GamepadButton::South)),
        );
        kb.set(Key::Space, true, 0.016);
        map.update(&Devices::default().with_keyboard(&kb), 0.016);
        assert!(map.just_pressed("jump"));
    }

    #[test]
    fn wasd_vector() {
        let mut kb = Keyboard::new();
        let mut map = ActionMap::new();
        map.set(
            "move",
            Action::new().vector(VectorBinding::Keys {
                up: Key::W,
                down: Key::S,
                left: Key::A,
                right: Key::D,
            }),
        );
        kb.set(Key::D, true, 0.016);
        map.update(&Devices::default().with_keyboard(&kb), 0.016);
        assert!(map.axis2d("move").x > 0.5);
    }

    #[test]
    fn blocking_set_shadows_lower() {
        let mut kb = Keyboard::new();
        let mut sets = ActionSets::new();

        let mut gameplay = ActionMap::new();
        gameplay.set("fire", Action::new().bind(Binding::Key(Key::Space)));
        sets.define("gameplay", ActionSet { map: gameplay, blocking: false });

        let mut menu = ActionMap::new();
        menu.set("confirm", Action::new().bind(Binding::Key(Key::Space)));
        sets.define("menu", ActionSet { map: menu, blocking: true });

        sets.push("gameplay");
        sets.push("menu"); // on top, blocking

        kb.set(Key::Space, true, 0.016);
        sets.update(&Devices::default().with_keyboard(&kb), 0.016);

        assert!(sets.map("menu").unwrap().pressed("confirm"));
        assert!(!sets.map("gameplay").unwrap().pressed("fire")); // shadowed
    }
}
