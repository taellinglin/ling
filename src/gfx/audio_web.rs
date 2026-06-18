// src/gfx/audio_web.rs — Web Audio API backend for wasm32 targets.
//
// Provides up to 16 spatially positioned sine-tone oscillators, each with
// LFO frequency modulation.  State is lazily initialised on the first
// audio_tone call.
//
// Signal graph per tone:
//   lfo_osc → lfo_gain → osc.frequency   (LFO → freq mod)
//   osc → amp_gain → panner → master_gain → destination

use js_sys::{Array, Function, Reflect};
use std::cell::RefCell;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{AudioContext, GainNode, OscillatorNode, OscillatorType, PannerNode};

struct Tone {
    osc: OscillatorNode,
    lfo: OscillatorNode,
    lfo_gain: GainNode,
    amp: GainNode,
    panner: PannerNode,
}

struct AudioState {
    ctx: AudioContext,
    tones: Vec<Option<Tone>>,
    master: GainNode,
}

thread_local! {
    static AUDIO: RefCell<Option<AudioState>> = RefCell::new(None);
}

/// Call a named method on a JS object with f64 arguments.
/// Uses Reflect so it works regardless of which web-sys bindings are enabled.
fn js_call(obj: &JsValue, method: &str, args: &[f64]) {
    if let Ok(func_val) = Reflect::get(obj, &JsValue::from_str(method)) {
        if let Some(func) = func_val.dyn_ref::<Function>() {
            let arr = Array::new();
            for &a in args {
                arr.push(&JsValue::from_f64(a));
            }
            let _ = func.apply(obj, &arr);
        }
    }
}

/// Lazily create the AudioContext and master gain on first use.
fn ensure_init() -> bool {
    AUDIO.with(|a| {
        if a.borrow().is_some() {
            return true;
        }
        match AudioContext::new() {
            Ok(ctx) => {
                let master = match ctx.create_gain() {
                    Ok(g) => g,
                    Err(_) => return false,
                };
                master.gain().set_value(1.0);
                master.connect_with_audio_node(&ctx.destination()).ok();
                *a.borrow_mut() =
                    Some(AudioState { ctx, tones: (0..16).map(|_| None).collect(), master });
                true
            },
            Err(e) => {
                web_sys::console::warn_1(&e);
                false
            },
        }
    })
}

/// Create or update tone channel `idx`.
///
/// `x, y, z` — world-space emitter position
/// `freq`     — base frequency Hz
/// `amp`      — amplitude [0..1]
/// `lfo_rate` — LFO Hz
/// `lfo_depth`— depth as fraction of base frequency
pub fn set_tone(
    idx: usize,
    x: f32,
    y: f32,
    z: f32,
    _w: f32,
    freq: f32,
    amp: f32,
    lfo_rate: f32,
    lfo_depth: f32,
) {
    if !ensure_init() {
        return;
    }
    AUDIO.with(|a| {
        let mut opt = a.borrow_mut();
        if let Some(state) = opt.as_mut() {
            if idx >= state.tones.len() {
                return;
            }

            // Create channel on first use
            if state.tones[idx].is_none() {
                let ctx = &state.ctx;
                let result = (|| -> Option<Tone> {
                    let osc = ctx.create_oscillator().ok()?;
                    let lfo = ctx.create_oscillator().ok()?;
                    let lfo_gain = ctx.create_gain().ok()?;
                    let amp_node = ctx.create_gain().ok()?;
                    let panner = ctx.create_panner().ok()?;

                    osc.set_type(OscillatorType::Sine);
                    lfo.set_type(OscillatorType::Sine);

                    // LFO → lfo_gain → osc.frequency
                    lfo.connect_with_audio_node(&lfo_gain).ok()?;
                    lfo_gain.connect_with_audio_param(&osc.frequency()).ok()?;

                    // osc → amp → panner → master
                    osc.connect_with_audio_node(&amp_node).ok()?;
                    amp_node.connect_with_audio_node(&panner).ok()?;
                    panner.connect_with_audio_node(&state.master).ok()?;

                    osc.start().ok()?;
                    lfo.start().ok()?;
                    Some(Tone { osc, lfo, lfo_gain, amp: amp_node, panner })
                })();

                state.tones[idx] = result;
            }

            // Update parameters
            if let Some(tone) = &state.tones[idx] {
                tone.osc.frequency().set_value(freq);
                tone.amp.gain().set_value(amp);
                tone.lfo.frequency().set_value(lfo_rate);
                tone.lfo_gain.gain().set_value(freq * lfo_depth);

                // Set 3-D position via deprecated but universally supported setPosition.
                // PannerNode.positionX AudioParams are not consistently available in
                // web-sys 0.3, so we call via Reflect to avoid feature-flag issues.
                js_call(
                    tone.panner.as_ref(),
                    "setPosition",
                    &[x as f64, y as f64, z as f64],
                );
            }
        }
    });
}

/// Update the listener orientation to match the camera rotation.
///
/// `cry/sry` = cos/sin of horizontal yaw, `crx/srx` = cos/sin of pitch.
pub fn set_listener(cry: f32, sry: f32, crx: f32, srx: f32) {
    AUDIO.with(|a| {
        let opt = a.borrow();
        if let Some(state) = opt.as_ref() {
            let listener = state.ctx.listener();
            // Forward vector: camera looks along -Z rotated by yaw then pitch
            let fx = sry;
            let fy = -srx * cry;
            let fz = -crx * cry;
            // setOrientation(forwardX, forwardY, forwardZ, upX, upY, upZ)
            js_call(
                listener.as_ref(),
                "setOrientation",
                &[fx as f64, fy as f64, fz as f64, 0.0, 1.0, 0.0],
            );
        }
    });
}

/// Set master output gain.
pub fn set_master_volume(vol: f32) {
    AUDIO.with(|a| {
        if let Some(state) = a.borrow().as_ref() {
            state.master.gain().set_value(vol);
        }
    });
}

/// BGM playback is not yet implemented in WASM (requires async fetch + decodeAudioData).
pub fn load_bgm(_path: &str, _vol: f32) {}

/// BGM volume no-op.
pub fn set_bgm_volume(_vol: f32) {}
