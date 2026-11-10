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
    /// Pads already plugged in at start-up get registered on the first poll
    /// (gilrs only sends `Connected` for later hot-plugs on some platforms).
    scanned: bool,
    /// Last D-pad direction synthesized from hat/axis D-pads, per pad.
    dpad_axes: HashMap<GamepadId, (i8, i8)>,
    /// The playing rumble per pad: the gilrs effect (dropping it stops it), its
    /// low / high amplitudes and when it was (re)armed.
    rumble: HashMap<GamepadId, (gilrs::ff::Effect, f32, f32, std::time::Instant)>,
}

impl GilrsBackend {
    /// Create a backend, initializing the gilrs context.
    ///
    /// # Errors
    /// Returns the gilrs initialization error if no input subsystem is available.
    #[allow(clippy::result_large_err)]
    pub fn new() -> Result<Self, gilrs::Error> {
        Ok(Self { gilrs: Gilrs::new()?, ids: HashMap::new(), scanned: false, dpad_axes: HashMap::new(), rumble: HashMap::new() })
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
        if !self.scanned {
            self.scanned = true;
            let present: Vec<GamepadId> =
                self.gilrs.gamepads().filter(|(_, g)| g.is_connected()).map(|(id, _)| id).collect();
            for gid in present {
                let device = self.ensure_registered(gid, registry);
                if let Some(info) = registry.get(device).cloned() {
                    out.push(InputEvent::DeviceConnected(info));
                }
            }
        }
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
                EventType::AxisChanged(a @ (GAxis::DPadX | GAxis::DPadY), value, _) => {
                    // hat / axis D-pads (many DInput and generic pads): turn
                    // them into the same D-pad buttons XInput pads report
                    let v: i8 = if value > 0.5 { 1 } else if value < -0.5 { -1 } else { 0 };
                    let st = self.dpad_axes.entry(ev.id).or_insert((0, 0));
                    let (old, buttons) = if a == GAxis::DPadX {
                        (std::mem::replace(&mut st.0, v), [GamepadButton::DpadLeft, GamepadButton::DpadRight])
                    } else {
                        // gilrs: +y is up
                        (std::mem::replace(&mut st.1, v), [GamepadButton::DpadDown, GamepadButton::DpadUp])
                    };
                    if old != v {
                        if old != 0 {
                            let b = buttons[usize::from(old > 0)];
                            out.push(InputEvent::Button { device, button: b, pressed: false });
                        }
                        if v != 0 {
                            let b = buttons[usize::from(v > 0)];
                            out.push(InputEvent::Button { device, button: b, pressed: true });
                        }
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
    /// Rumble as a per-frame amplitude: the host calls this every frame it wants the
    /// motors on. Each call (re)arms a short gilrs force-feedback effect — strong
    /// motor = `low`, weak motor = `high` — lasting ~140 ms, so the motors stop on
    /// their own shortly after the host stops asking. A call with the same levels
    /// inside 80 ms is skipped (no effect churn); zero stops it at once.
    fn set_rumble(&mut self, device: DeviceId, rumble: Rumble) {
        use gilrs::ff::{BaseEffect, BaseEffectType, EffectBuilder, Repeat, Replay, Ticks};
        let Some(gid) = self.ids.iter().find(|(_, d)| **d == device).map(|(g, _)| *g) else { return };
        let lo = rumble.low.clamp(0.0, 1.0);
        let hi = rumble.high.clamp(0.0, 1.0);
        if lo <= 0.001 && hi <= 0.001 {
            self.rumble.remove(&gid);
            return;
        }
        if let Some((_, l, h, at)) = self.rumble.get(&gid) {
            if (l - lo).abs() < 0.04 && (h - hi).abs() < 0.04 && at.elapsed().as_millis() < 80 {
                return;
            }
        }
        if !self.gilrs.gamepad(gid).is_ff_supported() {
            return;
        }
        let play = Replay { after: Ticks::from_ms(0), play_for: Ticks::from_ms(140), with_delay: Ticks::from_ms(0) };
        let built = EffectBuilder::new()
            .add_effect(BaseEffect { kind: BaseEffectType::Strong { magnitude: (lo * 65535.0) as u16 }, scheduling: play, ..Default::default() })
            .add_effect(BaseEffect { kind: BaseEffectType::Weak { magnitude: (hi * 65535.0) as u16 }, scheduling: play, ..Default::default() })
            .gamepads(&[gid])
            .repeat(Repeat::For(Ticks::from_ms(140)))
            .finish(&mut self.gilrs);
        if let Ok(effect) = built {
            if effect.play().is_ok() {
                // replacing the entry drops (stops) the previous effect
                self.rumble.insert(gid, (effect, lo, hi, std::time::Instant::now()));
            }
        }
    }

    fn name(&self) -> &str {
        "gilrs"
    }
}
