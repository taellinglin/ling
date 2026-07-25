//! Native gamepad backend built on [`gilrs`](https://docs.rs/gilrs) (XInput /
//! DInput / evdev / HID). Enabled by the `gilrs` feature.
//!
//! It translates gilrs events into [`InputEvent`]s and keeps the
//! [`DeviceRegistry`] in sync on hotplug. Stick `y` is negated so it follows the
//! project's screen-down `+y` convention. Rumble wiring is left to the host (see
//! the note on [`GilrsBackend::set_rumble`]): gilrs force-feedback uses built
//! effect objects rather than a per-frame amplitude, so it belongs with the
//! application's effect lifetime, not this thin adapter.

use std::collections::HashMap;

use gilrs::{Axis as GAxis, Button as GButton, EventType, GamepadId, Gilrs};

use crate::device::{Capabilities, DeviceId, DeviceInfo, DeviceKind, DeviceRegistry};
use crate::event::InputEvent;
use crate::gamepad::{GamepadAxis, GamepadButton};
use crate::haptics::Rumble;

use super::InputBackend;

/// A [`InputBackend`] over a live `gilrs` context.
pub struct GilrsBackend {
    gilrs: Gilrs,
    /// gilrs gamepad handle -> our device id.
    ids: HashMap<GamepadId, DeviceId>,
}

impl GilrsBackend {
    /// Create a backend, initializing the gilrs context.
    ///
    /// # Errors
    /// Returns the gilrs initialization error if no input subsystem is available.
    #[allow(clippy::result_large_err)]
    pub fn new() -> Result<Self, gilrs::Error> {
        Ok(Self { gilrs: Gilrs::new()?, ids: HashMap::new() })
    }

    fn ensure_registered(&mut self, gid: GamepadId, registry: &mut DeviceRegistry) -> DeviceId {
        if let Some(id) = self.ids.get(&gid) {
            return *id;
        }
        let name = self.gilrs.gamepad(gid).name().to_string();
        let mut info = DeviceInfo::new(0, DeviceKind::Gamepad, name);
        info.caps = Capabilities { rumble: true, ..Capabilities::default() };
        let id = registry.add(info);
        registry.assign_player(id);
        self.ids.insert(gid, id);
        id
    }
}

fn map_button(b: GButton) -> Option<GamepadButton> {
    Some(match b {
        GButton::South => GamepadButton::South,
        GButton::East => GamepadButton::East,
        GButton::North => GamepadButton::North,
        GButton::West => GamepadButton::West,
        GButton::LeftTrigger => GamepadButton::LeftShoulder,
        GButton::RightTrigger => GamepadButton::RightShoulder,
        GButton::LeftTrigger2 => GamepadButton::LeftTrigger,
        GButton::RightTrigger2 => GamepadButton::RightTrigger,
        GButton::Select => GamepadButton::Select,
        GButton::Start => GamepadButton::Start,
        GButton::Mode => GamepadButton::Guide,
        GButton::LeftThumb => GamepadButton::LeftStick,
        GButton::RightThumb => GamepadButton::RightStick,
        GButton::DPadUp => GamepadButton::DpadUp,
        GButton::DPadDown => GamepadButton::DpadDown,
        GButton::DPadLeft => GamepadButton::DpadLeft,
        GButton::DPadRight => GamepadButton::DpadRight,
        _ => return None,
    })
}

impl InputBackend for GilrsBackend {
    fn poll(&mut self, registry: &mut DeviceRegistry, out: &mut Vec<InputEvent>) {
        while let Some(ev) = self.gilrs.next_event() {
            let device = self.ensure_registered(ev.id, registry);
            match ev.event {
                EventType::Connected => {
                    if let Some(info) = registry.get(device).cloned() {
                        out.push(InputEvent::DeviceConnected(info));
                    }
                },
                EventType::Disconnected | EventType::Dropped => {
                    registry.disconnect(device);
                    out.push(InputEvent::DeviceDisconnected(device));
                },
                EventType::ButtonPressed(b, _) => {
                    if let Some(button) = map_button(b) {
                        out.push(InputEvent::Button { device, button, pressed: true });
                    }
                },
                EventType::ButtonReleased(b, _) => {
                    if let Some(button) = map_button(b) {
                        out.push(InputEvent::Button { device, button, pressed: false });
                    }
                },
                EventType::ButtonChanged(b, value, _) => {
                    // Analog triggers arrive here on most pads.
                    let axis = match b {
                        GButton::LeftTrigger2 => Some(GamepadAxis::LeftTrigger),
                        GButton::RightTrigger2 => Some(GamepadAxis::RightTrigger),
                        _ => None,
                    };
                    if let Some(axis) = axis {
                        out.push(InputEvent::Axis { device, axis, value });
                    }
                },
                EventType::AxisChanged(a, value, _) => {
                    // gilrs reports +y up; negate to screen-down.
                    let mapped = match a {
                        GAxis::LeftStickX => Some((GamepadAxis::LeftStickX, value)),
                        GAxis::LeftStickY => Some((GamepadAxis::LeftStickY, -value)),
                        GAxis::RightStickX => Some((GamepadAxis::RightStickX, value)),
                        GAxis::RightStickY => Some((GamepadAxis::RightStickY, -value)),
                        GAxis::LeftZ => Some((GamepadAxis::LeftTrigger, value)),
                        GAxis::RightZ => Some((GamepadAxis::RightTrigger, value)),
                        _ => None,
                    };
                    if let Some((axis, value)) = mapped {
                        out.push(InputEvent::Axis { device, axis, value });
                    }
                },
                _ => {},
            }
        }
    }

    /// Rumble is intentionally a no-op in this adapter. gilrs force-feedback is
    /// effect-based (`gilrs::ff::EffectBuilder`) and an effect must outlive the
    /// call, so the host should own effects and apply [`Rumble`] there. The
    /// crate's [`crate::haptics`] model produces the amplitudes to feed it.
    fn set_rumble(&mut self, _device: DeviceId, _rumble: Rumble) {}

    fn name(&self) -> &str {
        "gilrs"
    }
}
