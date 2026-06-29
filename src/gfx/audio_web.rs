// src/gfx/audio_web.rs — Web Audio API backend for wasm32 targets.
//
// Provides:
//  • Up to 16 spatially positioned sine-tone oscillators (LFO freq mod)
//  • BGM playback (load_bgm / set_bgm_volume)
//  • Music track playback with position tracking (play_music / music_position)
//  • Sample buffer pool (add_sample / play_sample)
//
// Signal graph per tone:
//   lfo_osc → lfo_gain → osc.frequency   (LFO → freq mod)
//   osc → amp_gain → panner → master_gain → destination

use js_sys::{Array, Function, Reflect, Uint8Array};
use std::cell::RefCell;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{AudioContext, AudioBuffer, AudioBufferSourceNode, GainNode, OscillatorNode, OscillatorType, PannerNode};

struct Tone {
    osc: OscillatorNode,
    lfo: OscillatorNode,
    lfo_gain: GainNode,
    amp: GainNode,
    panner: PannerNode,
}

struct BgmState {
    #[allow(dead_code)]
    buffer: AudioBuffer,
    source: Option<AudioBufferSourceNode>,
    gain: GainNode,
    volume: f32,
}

/// A decoded music track loaded via `play_music` and playing via Web Audio.
struct MusicSlot {
    #[allow(dead_code)]
    buffer: AudioBuffer,
    source: Option<AudioBufferSourceNode>,
    gain: GainNode,
    /// AudioContext.currentTime recorded when `start()` was called.
    start_ctx_time: f64,
    /// Track duration in seconds (for position wrap-around on loop).
    duration: f64,
}

struct AudioState {
    ctx: AudioContext,
    tones: Vec<Option<Tone>>,
    master: GainNode,
    bgm: Option<BgmState>,
    /// Per-track music playback slots (index = music track id).
    music_slots: Vec<Option<MusicSlot>>,
    /// Loaded sample AudioBuffers (index = sample handle).
    sample_bufs: Vec<Option<AudioBuffer>>,
    /// Track id of the most-recently started music (for `music_pos()`).
    current_music: usize,
}

thread_local! {
    static AUDIO: RefCell<Option<AudioState>> = RefCell::new(None);
    // When Web Audio is unavailable (e.g. worker scope without AudioContext),
    // permanently disable the backend so we don't throw every frame.
    static AUDIO_DISABLED: RefCell<bool> = RefCell::new(false);
}

fn has_audio_context_ctor() -> bool {
    let g = js_sys::global();
    Reflect::has(&g, &JsValue::from_str("AudioContext")).unwrap_or(false)
        || Reflect::has(&g, &JsValue::from_str("webkitAudioContext")).unwrap_or(false)
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
    if AUDIO_DISABLED.with(|d| *d.borrow()) {
        return false;
    }
    AUDIO.with(|a| {
        if a.borrow().is_some() {
            return true;
        }
        if !has_audio_context_ctor() {
            AUDIO_DISABLED.with(|d| *d.borrow_mut() = true);
            return false;
        }
        match AudioContext::new() {
            Ok(ctx) => {
                let master = match ctx.create_gain() {
                    Ok(g) => g,
                    Err(_) => return false,
                };
                master.gain().set_value(1.0);
                master.connect_with_audio_node(&ctx.destination()).ok();
                *a.borrow_mut() = Some(AudioState {
                    ctx,
                    tones: (0..16).map(|_| None).collect(),
                    master,
                    bgm: None,
                    music_slots: Vec::new(),
                    sample_bufs: Vec::new(),
                    current_music: 0,
                });
                true
            },
            Err(e) => {
                web_sys::console::warn_1(&e);
                AUDIO_DISABLED.with(|d| *d.borrow_mut() = true);
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

/// Load and play BGM from a URL. Uses synchronous XHR + decodeAudioData.
/// The file is fetched, decoded, and looped automatically.
pub fn load_bgm(path: &str, vol: f32) {
    if !ensure_init() {
        return;
    }

    // Fetch audio file synchronously
    let bytes = match fetch_audio_sync(path) {
        Ok(b) => b,
        Err(e) => {
            web_sys::console::warn_1(&format!("Failed to fetch BGM {}: {}", path, e).into());
            return;
        }
    };

    AUDIO.with(|a| {
        let mut opt = a.borrow_mut();
        if let Some(state) = opt.as_mut() {
            let ctx = state.ctx.clone();

            // Create gain node for BGM
            let gain = match ctx.create_gain() {
                Ok(g) => g,
                Err(_) => return,
            };
            gain.gain().set_value(vol);
            if gain.connect_with_audio_node(&state.master).is_err() {
                return;
            }

            // Decode audio data synchronously using a Promise-based approach
            // Note: We use a blocking approach here since Ling uses sync APIs
            match decode_audio_sync(&ctx, &bytes) {
                Ok(buffer) => {
                    // Stop any existing BGM
                    if let Some(bgm) = &state.bgm {
                        if let Some(src) = &bgm.source {
                            #[allow(deprecated)]
                            src.stop().ok();
                        }
                    }

                    // Create and start new source
                    let source = match ctx.create_buffer_source() {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                    source.set_buffer(Some(&buffer));
                    source.set_loop(true);
                    if source.connect_with_audio_node(&gain).is_err() {
                        return;
                    }
                    ctx.resume().ok();
                    if source.start().is_err() {
                        return;
                    }

                    state.bgm = Some(BgmState {
                        buffer,
                        source: Some(source),
                        gain,
                        volume: vol,
                    });
                },
                Err(e) => {
                    web_sys::console::warn_1(&format!("Failed to decode audio: {}", e).into());
                }
            }
        }
    });
}

// ─── PCM helpers ─────────────────────────────────────────────────────────────

/// Convert interleaved PCM (`stereo`, channel-count `channels`) into a Web
/// Audio `AudioBuffer`. Supports mono and stereo; silently clamps to 2 ch.
fn pcm_to_audio_buffer(ctx: &AudioContext, stereo: &[f32], channels: usize, rate: u32) -> Option<AudioBuffer> {
    if channels == 0 || stereo.is_empty() || rate == 0 {
        return None;
    }
    let ch = channels.min(2) as u32;
    let frames = stereo.len() / channels;
    let buf = ctx.create_buffer(ch, frames as u32, rate as f32).ok()?;

    // Left / mono channel — web-sys copy_to_channel takes &[f32] directly.
    let left: Vec<f32> = stereo.iter().step_by(channels).copied().collect();
    buf.copy_to_channel(&left, 0).ok()?;

    // Right channel (stereo only)
    if channels > 1 {
        let right: Vec<f32> = stereo[1..].iter().step_by(channels).copied().collect();
        buf.copy_to_channel(&right, 1).ok()?;
    }

    Some(buf)
}

// ─── Music track playback ─────────────────────────────────────────────────────

/// Start playing a decoded PCM track identified by `slot_id`.
/// If a track is already playing in that slot it is stopped first.
/// The track loops indefinitely.
pub fn play_music(slot_id: usize, stereo: &[f32], channels: usize, rate: u32, vol: f32) {
    if !ensure_init() {
        return;
    }
    AUDIO.with(|a| {
        let mut opt = a.borrow_mut();
        let state = match opt.as_mut() {
            Some(s) => s,
            None => return,
        };

        // Stop any existing source in this slot
        if let Some(Some(old)) = state.music_slots.get_mut(slot_id) {
            if let Some(src) = old.source.take() {
                #[allow(deprecated)]
                src.stop().ok();
            }
        }

        let duration = stereo.len() as f64 / channels.max(1) as f64 / rate.max(1) as f64;

        let buf = match pcm_to_audio_buffer(&state.ctx, stereo, channels, rate) {
            Some(b) => b,
            None => {
                web_sys::console::warn_1(&"play_music: pcm_to_audio_buffer failed".into());
                return;
            },
        };

        let gain = match state.ctx.create_gain() {
            Ok(g) => g,
            Err(_) => return,
        };
        gain.gain().set_value(vol);
        if gain.connect_with_audio_node(&state.master).is_err() {
            return;
        }

        let source = match state.ctx.create_buffer_source() {
            Ok(s) => s,
            Err(_) => return,
        };
        source.set_buffer(Some(&buf));
        source.set_loop(true);
        if source.connect_with_audio_node(&gain).is_err() {
            return;
        }
        // Resume the AudioContext before starting — browsers start it suspended
        // until a user gesture; this call is a no-op when already running.
        state.ctx.resume().ok();
        if source.start().is_err() {
            return;
        }

        let start = state.ctx.current_time();

        while state.music_slots.len() <= slot_id {
            state.music_slots.push(None);
        }
        state.music_slots[slot_id] = Some(MusicSlot {
            buffer: buf,
            source: Some(source),
            gain,
            start_ctx_time: start,
            duration,
        });
        state.current_music = slot_id;
    });
}

/// Return playback position of a music slot in seconds (wraps at duration).
pub fn music_position(slot_id: usize) -> f64 {
    AUDIO.with(|a| {
        let opt = a.borrow();
        if let Some(state) = opt.as_ref() {
            if let Some(Some(slot)) = state.music_slots.get(slot_id) {
                let elapsed = state.ctx.current_time() - slot.start_ctx_time;
                if slot.duration > 0.0 {
                    return elapsed % slot.duration;
                }
                return elapsed.max(0.0);
            }
        }
        0.0
    })
}

/// Playback position of the most-recently started track (for `music_pos()`).
pub fn current_music_position() -> f64 {
    AUDIO.with(|a| {
        let opt = a.borrow();
        if let Some(state) = opt.as_ref() {
            music_position_inner(state, state.current_music)
        } else {
            0.0
        }
    })
}

fn music_position_inner(state: &AudioState, slot_id: usize) -> f64 {
    if let Some(Some(slot)) = state.music_slots.get(slot_id) {
        let elapsed = state.ctx.current_time() - slot.start_ctx_time;
        if slot.duration > 0.0 {
            return elapsed % slot.duration;
        }
        return elapsed.max(0.0);
    }
    0.0
}

/// Set volume for a music slot.
pub fn set_music_volume(slot_id: usize, vol: f32) {
    AUDIO.with(|a| {
        let opt = a.borrow();
        if let Some(state) = opt.as_ref() {
            if let Some(Some(slot)) = state.music_slots.get(slot_id) {
                slot.gain.gain().set_value(vol);
            }
        }
    });
}

/// Stop a music slot (irreversible — call `play_music` to restart).
pub fn stop_music(slot_id: usize) {
    AUDIO.with(|a| {
        let mut opt = a.borrow_mut();
        if let Some(state) = opt.as_mut() {
            if let Some(Some(slot)) = state.music_slots.get_mut(slot_id) {
                if let Some(src) = slot.source.take() {
                    #[allow(deprecated)]
                    src.stop().ok();
                }
            }
        }
    });
}

// ─── Sample buffer pool ───────────────────────────────────────────────────────

/// Load a decoded PCM buffer into the pool.  Returns the new sample handle
/// (index into `sample_bufs`), or -1 on failure.
pub fn add_sample(stereo: &[f32], channels: usize, rate: u32) -> i32 {
    if !ensure_init() {
        return -1;
    }
    AUDIO.with(|a| {
        let mut opt = a.borrow_mut();
        if let Some(state) = opt.as_mut() {
            if let Some(buf) = pcm_to_audio_buffer(&state.ctx, stereo, channels, rate) {
                let id = state.sample_bufs.len();
                state.sample_bufs.push(Some(buf));
                return id as i32;
            }
        }
        -1
    })
}

/// Play sample `id` at world position (x, y, z) with the given volume.
/// If `looping` is true the source will loop until explicitly stopped.
/// Returns immediately (fire-and-forget for one-shot; ignores return voice id).
pub fn play_sample(id: usize, x: f32, y: f32, z: f32, vol: f32, looping: bool) {
    if !ensure_init() {
        return;
    }
    AUDIO.with(|a| {
        let opt = a.borrow();
        if let Some(state) = opt.as_ref() {
            let buf = match state.sample_bufs.get(id).and_then(|b| b.as_ref()) {
                Some(b) => b,
                None => return,
            };

            let gain = match state.ctx.create_gain() {
                Ok(g) => g,
                Err(_) => return,
            };
            gain.gain().set_value(vol);

            let panner = match state.ctx.create_panner() {
                Ok(p) => p,
                Err(_) => return,
            };
            js_call(panner.as_ref(), "setPosition", &[x as f64, y as f64, z as f64]);

            if gain.connect_with_audio_node(&panner).is_err() {
                return;
            }
            if panner.connect_with_audio_node(&state.master).is_err() {
                return;
            }

            let source = match state.ctx.create_buffer_source() {
                Ok(s) => s,
                Err(_) => return,
            };
            source.set_buffer(Some(buf));
            source.set_loop(looping);
            if source.connect_with_audio_node(&gain).is_err() {
                return;
            }
            state.ctx.resume().ok();
            source.start().ok();
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────────

/// Resume the AudioContext. Call this after a user gesture (click / keydown)
/// to satisfy the browser's autoplay policy. Safe to call multiple times.
pub fn resume() {
    AUDIO.with(|a| {
        if let Some(state) = a.borrow().as_ref() {
            state.ctx.resume().ok();
        }
    });
}

/// Set BGM volume.
pub fn set_bgm_volume(vol: f32) {
    AUDIO.with(|a| {
        if let Some(state) = a.borrow_mut().as_mut() {
            if let Some(bgm) = &mut state.bgm {
                bgm.volume = vol;
                bgm.gain.gain().set_value(vol);
            }
        }
    });
}

/// Fetch audio file synchronously using XMLHttpRequest
fn fetch_audio_sync(path: &str) -> Result<Vec<u8>, String> {
    let script = format!(
        r#"(function() {{
            var xhr = new XMLHttpRequest();
            xhr.open('GET', '{}', false);
            xhr.responseType = 'arraybuffer';
            xhr.send(null);
            if (xhr.status !== 200 && xhr.status !== 0) {{
                throw new Error('HTTP ' + xhr.status);
            }}
            return new Uint8Array(xhr.response || new ArrayBuffer(0));
        }})()"#,
        path.replace('\\', "\\\\").replace('\'', "\\'")
    );

    let result = js_sys::eval(&script)
        .map_err(|e| format!("XHR failed: {:?}", e))?;

    let arr = Uint8Array::new(&result);
    let mut bytes = vec![0u8; arr.length() as usize];
    arr.copy_to(&mut bytes);
    Ok(bytes)
}

/// Decode audio data synchronously by spinning on the Promise
fn decode_audio_sync(ctx: &AudioContext, data: &[u8]) -> Result<AudioBuffer, String> {
    // Create a Uint8Array from the data
    let array = Uint8Array::new_with_length(data.len() as u32);
    array.copy_from(data);

    // Call decodeAudioData and wait for it synchronously
    let promise = ctx
        .decode_audio_data(&array.buffer())
        .map_err(|e| format!("decode_audio_data failed: {:?}", e))?;

    // Spin-wait for the promise to resolve (blocks the thread)
    // This works in web workers and matches Ling's synchronous API design
    let script = format!(
        r#"(function(promise) {{
            var result = null;
            var error = null;
            var done = false;
            promise.then(function(r) {{ result = r; done = true; }})
                   .catch(function(e) {{ error = e; done = true; }});
            var start = Date.now();
            while (!done && Date.now() - start < 10000) {{
                // Spin-wait up to 10 seconds
            }}
            if (error) throw error;
            if (!done) throw new Error('Timeout decoding audio');
            return result;
        }})(arguments[0])"#
    );

    let func = js_sys::Function::new_with_args("promise", &script);
    let args = Array::new();
    args.push(&promise);

    let result = func
        .apply(&JsValue::NULL, &args)
        .map_err(|e| format!("Audio decode failed: {:?}", e))?;

    result
        .dyn_into::<AudioBuffer>()
        .map_err(|_| "Result is not an AudioBuffer".to_string())
}
