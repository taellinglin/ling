//! # ling-input — "Sensorium"
//!
//! Ling's unified input system: **every controller, touch, limb, and gaze
//! becomes one stream of intent.** A gamepad, a flight stick, a phone's
//! on-screen thumbstick, a VR hand, an eye-gaze ray — they all flow through the
//! same primitives and end up as remappable *actions* your game reads without
//! ever naming a device.
//!
//! It carries the same **organic 灵 ↔ mechanical 机** motif as
//! [`ling-animation`]: input ranges from the mechanical end (crisp buttons,
//! gear-precise sticks, exact key chords) to the organic end (motion, skeletal
//! hand tracking, touch gestures, the soft envelope of a haptic effect).
//!
//! ## Layers
//! - [`button`] — edge-detected digital state with auto-repeat (the shared primitive).
//! - [`axis`] — analog axes & sticks: deadzones, response curves, calibration.
//! - [`gamepad`] — vendor-neutral standard gamepad + per-frame snapshot.
//! - [`joystick`] — generic HID flight/arcade sticks & wheels (N axes, hats).
//! - [`motion`] — gyro + accelerometer fusion and gyro-aiming.
//! - [`haptics`] — rumble, adaptive triggers, time-sampled haptic effects.
//! - [`vr`] — XR poses, headset, controllers, skeletal hands, eye gaze.
//! - [`touch`] — raw multi-touch pool.
//! - [`gesture`] — tap / long-press / swipe / pinch / rotate recognition.
//! - [`onscreen`] — virtual sticks/buttons/d-pads that synthesize a gamepad.
//! - [`theme`] — themer for on-screen controls (Ling palette by default).
//! - [`keyboard`] / [`pointer`] — scancode keyboard, mouse, unified pen pointer.
//! - [`mapping`] — actions, bindings, and push/pop action sets (contexts).
//! - [`device`] — registry, capabilities, hotplug, player assignment.
//! - [`event`] — one serializable event stream (replays / rollback netcode).
//! - [`backend`] — pluggable sources ([`backend::ManualBackend`], `gilrs`, …).
//!
//! ## The hub
//! [`Sensorium`] ties it together: pump a [`backend::InputBackend`] (or feed
//! events by hand), call [`Sensorium::begin_frame`] / [`Sensorium::update`],
//! then read gamepads, gestures, the on-screen pad, motion, XR, and resolved
//! actions — all by handle, the way the Ling runtime drives its subsystems.
//!
//! [`ling-animation`]: https://docs.rs/ling-animation

pub mod axis;
pub mod backend;
pub mod button;
pub mod device;
pub mod event;
pub mod gamepad;
pub mod gesture;
pub mod haptics;
pub mod joystick;
pub mod keyboard;
pub mod mapping;
pub mod motion;
pub mod onscreen;
pub mod pointer;
pub mod theme;
pub mod touch;
pub mod vr;

pub use axis::{Axis, Curve, Stick};
pub use backend::{InputBackend, ManualBackend};
pub use button::Button;
pub use device::{Capabilities, DeviceId, DeviceInfo, DeviceKind, DeviceRegistry};
pub use event::{ActionPhase, InputEvent};
pub use gamepad::{Gamepad, GamepadAxis, GamepadButton, GamepadStick, Layout};
pub use gesture::{Gesture, GestureRecognizer, SwipeDir};
pub use haptics::{HapticEffect, HapticPlayer, Rumble, TriggerEffect};
pub use joystick::{Hat, Joystick};
pub use keyboard::{Key, Keyboard, Modifiers};
pub use mapping::{Action, ActionMap, ActionSet, ActionSets, Binding, Devices, VectorBinding};
pub use motion::{GyroSpace, MotionSample, MotionState};
pub use onscreen::{OnScreenControls, Rect, StickMode, VirtualButton, VirtualDpad, VirtualStick};
pub use pointer::{Mouse, MouseButton, Pointer, PointerKind};
pub use theme::{Color, ControlShape, ControlTheme};
pub use touch::{Touch, TouchId, TouchPhase, TouchPool};
pub use vr::{
    Gaze, Hand, HandJoint, HandSkeleton, Headset, PlaySpace, Pose, Xr, XrButton, XrController,
};

/// This crate's semantic version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The central input hub — one per application.
///
/// It owns every device's state and the per-frame plumbing. Typical loop:
/// ```no_run
/// use ling_input::{Sensorium, ManualBackend};
/// let mut sensorium = Sensorium::new(4);
/// let mut backend = ManualBackend::new();
/// # let dt = 1.0 / 60.0;
/// loop {
///     sensorium.begin_frame();
///     sensorium.pump(&mut backend);   // drain a backend's events
///     sensorium.update(dt);           // advance edges, gestures, actions
///     if sensorium.actions.map("gameplay").is_some_and(|m| m.just_pressed("jump")) {
///         // ...
///     }
/// #   break;
/// }
/// ```
#[derive(Debug)]
pub struct Sensorium {
    pub devices: DeviceRegistry,
    /// One gamepad per player slot (`0..max_players`).
    pub pads: Vec<Gamepad>,
    pub keyboard: Keyboard,
    pub mouse: Mouse,
    pub touch: TouchPool,
    pub gestures: GestureRecognizer,
    pub onscreen: OnScreenControls,
    pub motion: MotionState,
    pub xr: Xr,
    pub actions: ActionSets,
    /// Events ingested this frame (for inspection / recording).
    pub events: Vec<InputEvent>,
    /// Seconds since the Sensorium was created.
    pub time: f32,
    scratch: Vec<InputEvent>,
}

impl Sensorium {
    /// Create a hub with `max_players` gamepad slots.
    #[must_use]
    pub fn new(max_players: usize) -> Self {
        Self {
            devices: DeviceRegistry::new(),
            pads: (0..max_players.max(1)).map(|_| Gamepad::new()).collect(),
            keyboard: Keyboard::new(),
            mouse: Mouse::new(),
            touch: TouchPool::new(),
            gestures: GestureRecognizer::new(),
            onscreen: OnScreenControls::new(),
            motion: MotionState::new(),
            xr: Xr::new(),
            actions: ActionSets::new(),
            events: Vec::new(),
            time: 0.0,
            scratch: Vec::new(),
        }
    }

    /// Read a player's gamepad snapshot.
    #[must_use]
    pub fn player(&self, slot: usize) -> Option<&Gamepad> {
        self.pads.get(slot)
    }

    /// Begin a frame: reset per-frame device buffers. Call first.
    pub fn begin_frame(&mut self) {
        self.events.clear();
        self.touch.begin_frame();
        self.keyboard.begin_frame();
        self.mouse.begin_frame();
    }

    /// Drain a backend's events and ingest them.
    pub fn pump<B: InputBackend>(&mut self, backend: &mut B) {
        self.scratch.clear();
        backend.poll(&mut self.devices, &mut self.scratch);
        let drained = std::mem::take(&mut self.scratch);
        for ev in drained {
            self.ingest(ev);
        }
        self.scratch = Vec::new();
    }

    /// Apply one event to the appropriate device state and record it.
    pub fn ingest(&mut self, ev: InputEvent) {
        match &ev {
            InputEvent::Button { device, button, pressed } => {
                if let Some(pad) = self.pad_for(*device) {
                    pad.set_held(*button, *pressed);
                }
            },
            InputEvent::Axis { device, axis, value } => {
                if let Some(pad) = self.pad_for(*device) {
                    pad.set_axis(*axis, *value);
                }
            },
            InputEvent::Touch(t) => match t.phase {
                TouchPhase::Began => self.touch.begin(t.id, t.pos, t.pressure),
                TouchPhase::Moved | TouchPhase::Stationary => {
                    self.touch.moved(t.id, t.pos, t.pressure);
                },
                TouchPhase::Ended => self.touch.end(t.id),
                TouchPhase::Cancelled => self.touch.cancel(t.id),
            },
            InputEvent::Motion { sample, .. } => self.motion.update(*sample),
            _ => {},
        }
        self.events.push(ev);
    }

    /// Advance edges, gestures, on-screen controls, and actions for the frame.
    pub fn update(&mut self, dt: f32) {
        self.time += dt;
        for pad in &mut self.pads {
            pad.tick(dt);
        }
        self.gestures.update(&self.touch, dt);
        self.onscreen.update(&self.touch, dt);

        let dev = Devices {
            gamepad: self.pads.first(),
            keyboard: Some(&self.keyboard),
            mouse: Some(&self.mouse),
        };
        self.actions.update(&dev, dt);
    }

    /// Which player's pad does this device drive? (`player` slot, else `0`).
    fn pad_for(&mut self, device: DeviceId) -> Option<&mut Gamepad> {
        let slot = self
            .devices
            .get(device)
            .and_then(|d| d.player)
            .map_or(0, usize::from);
        self.pads.get_mut(slot)
    }
}

impl Default for Sensorium {
    fn default() -> Self {
        Self::new(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn end_to_end_button_through_action() {
        let mut s = Sensorium::new(2);
        let id = s
            .devices
            .add(DeviceInfo::new(0, DeviceKind::Gamepad, "Pad"));
        s.devices.assign_player(id);

        let mut gameplay = ActionMap::new();
        gameplay.set(
            "jump",
            Action::new().bind(Binding::Pad(GamepadButton::South)),
        );
        s.actions
            .define("gameplay", ActionSet { map: gameplay, blocking: false });
        s.actions.push("gameplay");

        s.begin_frame();
        s.ingest(InputEvent::Button { device: id, button: GamepadButton::South, pressed: true });
        s.update(1.0 / 60.0);

        assert!(s.actions.map("gameplay").unwrap().just_pressed("jump"));
        assert!(s.player(0).unwrap().is_down(GamepadButton::South));
    }

    #[test]
    fn touch_events_reach_onscreen_and_gestures() {
        let mut s = Sensorium::new(1);
        s.onscreen = OnScreenControls::twin_stick(glam::Vec2::new(1280.0, 720.0));
        let b = s.onscreen.buttons[1].center;
        s.begin_frame();
        s.ingest(InputEvent::Touch(Touch {
            id: 1,
            pos: b,
            start: b,
            prev: b,
            pressure: 1.0,
            phase: TouchPhase::Began,
        }));
        s.update(1.0 / 60.0);
        assert!(s.onscreen.gamepad().is_down(GamepadButton::East));
    }
}
