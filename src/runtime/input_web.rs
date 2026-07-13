// WASM gamepad + touch polling using the Web Gamepad API.
//
// `poll()` must be called each frame (pad_poll builtin). It snapshots the live
// `navigator.getGamepads()` array into a thread-local so the other pad_* builtins
// can read it cheaply without re-querying the DOM.

use std::cell::RefCell;

/// Snapshot of one connected gamepad's state.
#[derive(Clone, Default)]
pub struct WasmPad {
    pub connected: bool,
    /// Raw button pressed flags (standard mapping, up to 17).
    pub buttons: [bool; 17],
    /// Raw analog button values (for triggers).
    pub button_values: [f32; 17],
    /// Axes: [lx, ly, rx, ry].
    pub axes: [f32; 4],
}

thread_local! {
    static PADS: RefCell<Vec<WasmPad>> = const { RefCell::new(Vec::new()) };
}

/// Poll `navigator.getGamepads()` and update the thread-local snapshot.
/// Returns the number of connected gamepads.
pub fn poll() -> usize {
    let result = try_poll();
    result.unwrap_or(0)
}

fn try_poll() -> Option<usize> {
    let window = web_sys::window()?;
    let navigator = window.navigator();
    let raw = navigator.get_gamepads().ok()?;

    Some(PADS.with(|cell| {
        let mut pads = cell.borrow_mut();
        let len = raw.length() as usize;
        if pads.len() < len {
            pads.resize_with(len, WasmPad::default);
        }
        let mut connected = 0usize;
        for i in 0..len {
            let val = raw.get(i as u32);
            if val.is_null() || val.is_undefined() {
                if i < pads.len() {
                    pads[i].connected = false;
                }
                continue;
            }
            let gp: web_sys::Gamepad = val.into();
            if !gp.connected() {
                if i < pads.len() {
                    pads[i].connected = false;
                }
                continue;
            }

            if i >= pads.len() {
                pads.resize_with(i + 1, WasmPad::default);
            }
            let pad = &mut pads[i];
            pad.connected = true;
            connected += 1;

            let btns = gp.buttons();
            let blen = btns.length().min(17) as usize;
            for b in 0..blen {
                let btn: web_sys::GamepadButton = btns.get(b as u32).into();
                pad.buttons[b] = btn.pressed();
                pad.button_values[b] = btn.value() as f32;
            }

            let axes = gp.axes();
            let alen = axes.length().min(4) as usize;
            for a in 0..alen {
                pad.axes[a] = js_sys::Array::from(&axes)
                    .get(a as u32)
                    .as_f64()
                    .unwrap_or(0.0) as f32;
            }
        }
        connected
    }))
}

/// Map a button name to the W3C standard-mapping index.
pub fn button_index(name: &str) -> Option<usize> {
    Some(match name.to_ascii_lowercase().as_str() {
        "a" | "south" | "cross" => 0,
        "b" | "east" | "circle" => 1,
        "x" | "west" | "square" => 2,
        "y" | "north" | "triangle" => 3,
        "lb" | "l1" | "left_shoulder" => 4,
        "rb" | "r1" | "right_shoulder" => 5,
        "lt" | "l2" | "left_trigger" => 6,
        "rt" | "r2" | "right_trigger" => 7,
        "select" | "back" | "share" | "view" => 8,
        "start" | "menu" | "options" => 9,
        "l3" | "left_stick" => 10,
        "r3" | "right_stick" => 11,
        "up" | "dpad_up" => 12,
        "down" | "dpad_down" => 13,
        "left" | "dpad_left" => 14,
        "right" | "dpad_right" => 15,
        "guide" | "home" => 16,
        _ => return None,
    })
}

pub fn is_connected(i: usize) -> bool {
    PADS.with(|c| c.borrow().get(i).is_some_and(|p| p.connected))
}

pub fn count() -> usize {
    PADS.with(|c| c.borrow().iter().filter(|p| p.connected).count())
}

pub fn button_down(pad: usize, name: &str) -> bool {
    let idx = match button_index(name) {
        Some(i) => i,
        None => return false,
    };
    PADS.with(|c| {
        c.borrow()
            .get(pad)
            .is_some_and(|p| p.connected && p.buttons.get(idx).copied().unwrap_or(false))
    })
}

pub fn axis_lx(pad: usize) -> f32 {
    PADS.with(|c| c.borrow().get(pad).map_or(0.0, |p| p.axes[0]))
}
pub fn axis_ly(pad: usize) -> f32 {
    PADS.with(|c| c.borrow().get(pad).map_or(0.0, |p| p.axes[1]))
}
pub fn axis_rx(pad: usize) -> f32 {
    PADS.with(|c| c.borrow().get(pad).map_or(0.0, |p| p.axes[2]))
}
pub fn axis_ry(pad: usize) -> f32 {
    PADS.with(|c| c.borrow().get(pad).map_or(0.0, |p| p.axes[3]))
}
pub fn trigger_lt(pad: usize) -> f32 {
    PADS.with(|c| c.borrow().get(pad).map_or(0.0, |p| p.button_values[6]))
}
pub fn trigger_rt(pad: usize) -> f32 {
    PADS.with(|c| c.borrow().get(pad).map_or(0.0, |p| p.button_values[7]))
}
