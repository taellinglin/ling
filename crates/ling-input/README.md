# ling-input — "Sensorium"

Ling's unified input system. **Every controller, touch, limb, and gaze becomes
one stream of intent.** Game logic reads remappable *actions* and never names a
device — a gamepad, a flight stick, an on-screen thumbstick, a VR hand, or an
eye-gaze ray all flow through the same primitives.

Like [`ling-animation`](../ling-animation), it carries the **organic 灵 ↔
mechanical 机** motif: input ranges from the mechanical end (crisp buttons,
gear-precise sticks, key chords) to the organic end (motion, skeletal hand
tracking, touch gestures, the soft envelope of a haptic effect).

## What's in the box

| Area | Module | Highlights |
|------|--------|-----------|
| Digital | `button` | edge detection (`just_pressed`/`released`), held time, auto-repeat |
| Analog | `axis` | radial deadzones, response curves, calibration, 2-D sticks |
| Gamepads | `gamepad` | vendor-neutral layout, per-vendor glyphs, paddles/touchpad/capture |
| HID | `joystick` | arbitrary axes/buttons + 8-way POV hats (HOTAS, wheels, fight sticks) |
| Motion | `motion` | gyro+accel complementary fusion, **gyro-aiming** |
| Haptics | `haptics` | dual-motor rumble, **adaptive triggers**, time-sampled effects (click/heartbeat/pulse) |
| XR | `vr` | 6-DoF poses, headset, controllers, 26-joint **skeletal hands**, **eye gaze**, guardian bounds |
| Touch | `touch` | multi-touch pool with phase + pressure |
| Gestures | `gesture` | tap / double-tap / long-press / swipe / pinch / rotate |
| On-screen | `onscreen` | **fixed & floating** virtual sticks, buttons, d-pads → synthesize a `Gamepad` |
| Theming | `theme` | serializable skins, Ling palette by default, accessibility presets |
| KB/Mouse | `keyboard`, `pointer` | scancode keys (layout-independent), mouse, unified **pen pointer** (pressure/tilt) |
| Actions | `mapping` | bindings, 2-D composites, push/pop **action sets** (contexts) |
| Devices | `device` | registry, capability flags, hotplug, player assignment |
| Events | `event` | one serializable stream — record for **replays / rollback netcode** |
| Backends | `backend` | `InputBackend` trait, `ManualBackend`, native `gilrs` (feature) |

## Quick start

```rust
use ling_input::{Sensorium, ManualBackend, Action, ActionMap, ActionSet, Binding, GamepadButton, Key};

let mut s = Sensorium::new(4);          // 4 player slots
let mut backend = ManualBackend::new(); // swap for GilrsBackend on native

let mut gameplay = ActionMap::new();
gameplay.set("jump", Action::new()
    .bind(Binding::Pad(GamepadButton::South))
    .bind(Binding::Key(Key::Space)));
s.actions.define("gameplay", ActionSet { map: gameplay, blocking: false });
s.actions.push("gameplay");

// per frame:
s.begin_frame();
s.pump(&mut backend);     // drain events from a backend
s.update(1.0 / 60.0);     // advance edges, gestures, on-screen pad, actions
if s.actions.map("gameplay").unwrap().just_pressed("jump") { /* ... */ }
```

## Features

- `serde` *(default)* — serialize themes, layouts, action maps, and input
  snapshots (replays / netcode). Also pulls glam's serde impls.
- `gilrs` — native gamepad backend (XInput / DInput / evdev / HID) via
  [`gilrs`](https://docs.rs/gilrs). Reads buttons/axes/hotplug; sticks are
  negated to the project's screen-down `+y`. Rumble is host-owned (see
  `GilrsBackend::set_rumble`).

## Conventions

- **Screen-down `+y`** throughout (matches the rest of Ling): pushing a stick or
  thumb "down" yields `+y`.
- Math is [`glam`]. The crate is `wasm`-safe by default (no platform deps unless
  a backend feature is enabled), so the same code runs in the browser, on
  mobile, and on native.
