// src/runtime/gamepad.rs — native gamepad input + rumble via gilrs (XInput /
// DInput / evdev). Single-threaded: the game loop pumps `poll()` each frame,
// then queries `button()` / `axis()`. `rumble()` builds a force-feedback effect
// whose lifetime we own until it expires.
//
//   gamepad_poll()                 — pump events (call once per frame)
//   gamepad_button(name) → 0/1     — a b x y l1 r1 l2 r2 l3 r3 start select
//                                     dup ddown dleft dright
//   gamepad_axis(name)   → -1..1   — lx ly rx ry l2 r2
//   gamepad_rumble(low, high, ms)  — low/high motor 0..1 for ms milliseconds

use std::cell::RefCell;
use std::time::{Duration, Instant};

use gilrs::ff::{BaseEffect, BaseEffectType, Effect, EffectBuilder, Replay, Ticks};
use gilrs::{Axis as GAxis, Button as GButton, GamepadId, Gilrs};

struct Pad {
    gilrs: Gilrs,
    active: Option<GamepadId>,
    effects: Vec<(Effect, Instant)>, // keep effects alive until they expire
}

thread_local! {
    static PAD: RefCell<Option<Pad>> = const { RefCell::new(None) };
}

fn ensure(p: &mut Option<Pad>) {
    if p.is_none() {
        if let Ok(gilrs) = Gilrs::new() {
            let active = gilrs.gamepads().next().map(|(id, _)| id);
            *p = Some(Pad { gilrs, active, effects: Vec::new() });
        }
    }
}

pub fn poll() {
    PAD.with(|cell| {
        let mut g = cell.borrow_mut();
        ensure(&mut g);
        let Some(pad) = g.as_mut() else { return };
        while let Some(ev) = pad.gilrs.next_event() {
            pad.gilrs.update(&ev);
            // latch onto the most recently active gamepad
            pad.active = Some(ev.id);
        }
        if pad.active.is_none() {
            pad.active = pad.gilrs.gamepads().next().map(|(id, _)| id);
        }
        // drop finished rumble effects
        let now = Instant::now();
        pad.effects.retain(|(_, exp)| *exp > now);
    });
}

fn btn(name: &str) -> Option<GButton> {
    Some(match name {
        "a" => GButton::South,
        "b" => GButton::East,
        "x" => GButton::West,
        "y" => GButton::North,
        "l1" => GButton::LeftTrigger,
        "r1" => GButton::RightTrigger,
        "l2" => GButton::LeftTrigger2,
        "r2" => GButton::RightTrigger2,
        "l3" => GButton::LeftThumb,
        "r3" => GButton::RightThumb,
        "start" => GButton::Start,
        "select" => GButton::Select,
        "dup" => GButton::DPadUp,
        "ddown" => GButton::DPadDown,
        "dleft" => GButton::DPadLeft,
        "dright" => GButton::DPadRight,
        _ => return None,
    })
}

pub fn button(name: &str) -> bool {
    let Some(b) = btn(name) else { return false };
    PAD.with(|cell| {
        let g = cell.borrow();
        let Some(pad) = g.as_ref() else { return false };
        let Some(id) = pad.active else { return false };
        pad.gilrs.gamepad(id).is_pressed(b)
    })
}

pub fn axis(name: &str) -> f32 {
    let a = match name {
        "lx" => GAxis::LeftStickX,
        "ly" => GAxis::LeftStickY,
        "rx" => GAxis::RightStickX,
        "ry" => GAxis::RightStickY,
        "l2" => GAxis::LeftZ,
        "r2" => GAxis::RightZ,
        _ => return 0.0,
    };
    PAD.with(|cell| {
        let g = cell.borrow();
        let Some(pad) = g.as_ref() else { return 0.0 };
        let Some(id) = pad.active else { return 0.0 };
        pad.gilrs.gamepad(id).value(a)
    })
}

pub fn rumble(low: f32, high: f32, ms: u32) {
    let lo = (low.clamp(0.0, 1.0) * 65535.0) as u16;
    let hi = (high.clamp(0.0, 1.0) * 65535.0) as u16;
    PAD.with(|cell| {
        let mut g = cell.borrow_mut();
        ensure(&mut g);
        let Some(pad) = g.as_mut() else { return };
        let Some(id) = pad.active else { return };
        if !pad.gilrs.gamepad(id).is_ff_supported() { return; }
        let play = Replay { play_for: Ticks::from_ms(ms), ..Default::default() };
        let built = EffectBuilder::new()
            .add_effect(BaseEffect { kind: BaseEffectType::Strong { magnitude: hi }, scheduling: play, ..Default::default() })
            .add_effect(BaseEffect { kind: BaseEffectType::Weak { magnitude: lo }, scheduling: play, ..Default::default() })
            .gamepads(&[id])
            .finish(&mut pad.gilrs);
        if let Ok(effect) = built {
            let _ = effect.play();
            pad.effects.push((effect, Instant::now() + Duration::from_millis(ms as u64 + 50)));
        }
    });
}
