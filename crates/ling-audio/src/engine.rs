// crates/ling-audio/src/engine.rs — 4D positional audio engine
//
// Each "tone" lives at a 3-D world position plus a 4th-dimension W value that
// cross-modulates the oscillator for a hyperdimensional shimmer.
//
// Spatial audio:
//   - Camera orientation (cry, sry, crx, srx) matches the Ling gfx Camera3D.
//   - World position → camera-space X drives equal-power L/R panning.
//   - Distance in camera space drives exponential attenuation.
//   - tanh soft-clips the final mix so nothing blows up.
//
// BGM: raw WAV loaded via hound, linearly-resampled to the device rate, looped.

use std::sync::{Arc, Mutex};
use std::f32::consts::{TAU, FRAC_PI_2};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

// ─── Public types ─────────────────────────────────────────────────────────────

/// Parameters for one positional tone.
#[derive(Clone, Debug)]
pub struct ToneParams {
    /// World-space position of the sound source.
    pub x: f32, pub y: f32, pub z: f32,
    /// 4th-dimension value: drives a sub-oscillator at `freq * w * 0.007 Hz`
    /// that cross-modulates the main carrier (0 → no 4D effect).
    pub w: f32,
    /// Carrier frequency in Hz.
    pub freq: f32,
    /// Linear amplitude (0..1 recommended).
    pub amp: f32,
    /// LFO rate in Hz (vibrato speed).
    pub lfo_rate: f32,
    /// LFO depth as a fraction of freq (0.03 = ±3 % pitch wobble).
    pub lfo_depth: f32,
}

impl Default for ToneParams {
    fn default() -> Self {
        Self {
            x: 0.0, y: 0.0, z: 0.0, w: 1.0,
            freq: 220.0, amp: 0.15,
            lfo_rate: 0.5, lfo_depth: 0.02,
        }
    }
}

// ─── Internal state ────────────────────────────────────────────────────────────

struct Tone {
    params:    ToneParams,
    phase:     f32,   // carrier oscillator phase [0, 1)
    lfo_phase: f32,   // LFO phase [0, 1)
    w_phase:   f32,   // 4D sub-oscillator phase [0, 1)
}

impl Tone {
    fn new(params: ToneParams) -> Self {
        Self { params, phase: 0.0, lfo_phase: 0.0, w_phase: 0.0 }
    }
}

struct BgmTrack {
    /// Interleaved stereo samples at `src_rate`.
    samples:  Vec<f32>,
    src_rate: u32,
    /// Fractional stereo-pair index (advances by `src_rate / device_rate` per sample).
    pos:      f64,
    volume:   f32,
}

struct AudioState {
    tones:         Vec<Option<Tone>>,
    bgm:           Option<BgmTrack>,
    master_volume: f32,
    // Camera orientation — mirrors Camera3D cry/sry/crx/srx.
    cry: f32, sry: f32,
    crx: f32, srx: f32,
    /// Half-width of the room (used to normalise the pan value).
    room_w: f32,
    sample_rate: u32,
}

impl AudioState {
    fn new(sample_rate: u32) -> Self {
        Self {
            tones:         (0..16).map(|_| None).collect(),
            bgm:           None,
            master_volume: 0.5,
            cry: 1.0, sry: 0.0,
            crx: 1.0, srx: 0.0,
            room_w: 9.0,
            sample_rate,
        }
    }

    /// Generate one stereo (L, R) sample pair.
    #[inline]
    fn next_sample(&mut self) -> (f32, f32) {
        // Copy scalars so borrowck doesn't complain about &mut self during tone loop.
        let cry   = self.cry;
        let sry   = self.sry;
        let crx   = self.crx;
        let srx   = self.srx;
        let room_w = self.room_w;
        let dt    = 1.0 / self.sample_rate as f32;

        let mut l = 0.0f32;
        let mut r = 0.0f32;

        for slot in &mut self.tones {
            let tone = match slot.as_mut() { Some(t) => t, None => continue };
            let p = &tone.params;

            // ── World → camera-space ─────────────────────────────────────────
            // Apply Y-rotation (yaw) then X-rotation (pitch) — same as Camera3D.
            let rz1   =  p.x * sry + p.z * cry;
            let cam_x =  p.x * cry - p.z * sry;
            let cam_y =  p.y * crx - rz1  * srx;
            let cam_z =  p.y * srx + rz1  * crx;

            // ── Spatial attenuation ──────────────────────────────────────────
            let dist  = (cam_x * cam_x + cam_y * cam_y + cam_z * cam_z).sqrt().max(0.5);
            let atten = (1.0 / (1.0 + dist * 0.18)).clamp(0.0, 1.0);

            // ── Equal-power panning ──────────────────────────────────────────
            let pan   = (cam_x / room_w.max(1.0)).clamp(-1.0, 1.0);
            let angle = (pan + 1.0) * 0.5 * FRAC_PI_2;
            let l_gain = angle.cos() * atten;
            let r_gain = angle.sin() * atten;

            // ── LFO (vibrato) ────────────────────────────────────────────────
            let lfo_mod = (tone.lfo_phase * TAU).sin() * p.lfo_depth;
            tone.lfo_phase = (tone.lfo_phase + p.lfo_rate * dt).fract();

            // ── 4D sub-oscillator ─────────────────────────────────────────────
            // W drives a slow cross-modulator; the phase drift creates
            // hyperdimensional beating that is unique per sound-source.
            let w_mod  = (tone.w_phase * TAU).sin() * 0.25;
            let w_freq = p.freq * p.w.abs() * 0.007;
            tone.w_phase = (tone.w_phase + w_freq * dt).fract();

            // ── Carrier oscillator ────────────────────────────────────────────
            let inst_freq = p.freq * (1.0 + lfo_mod) * (1.0 + w_mod * 0.08);
            let sample    = (tone.phase * TAU).sin() * p.amp;
            tone.phase    = (tone.phase + inst_freq * dt).fract();

            l += sample * l_gain;
            r += sample * r_gain;
        }

        // ── BGM ─────────────────────────────────────────────────────────────
        if let Some(bgm) = &mut self.bgm {
            let n_pairs = bgm.samples.len() / 2;
            if n_pairs >= 2 {
                let ratio = bgm.src_rate as f64 / self.sample_rate as f64;
                let idx   = bgm.pos as usize;
                let frac  = (bgm.pos - idx as f64) as f32;
                let nxt   = (idx + 1) % n_pairs;

                let bl = bgm.samples[idx * 2    ] + (bgm.samples[nxt * 2    ] - bgm.samples[idx * 2    ]) * frac;
                let br = bgm.samples[idx * 2 + 1] + (bgm.samples[nxt * 2 + 1] - bgm.samples[idx * 2 + 1]) * frac;

                l += bl * bgm.volume;
                r += br * bgm.volume;

                bgm.pos += ratio;
                if bgm.pos as usize >= n_pairs.saturating_sub(1) {
                    bgm.pos = 0.0;  // loop
                }
            }
        }

        let mv = self.master_volume;
        ((l * mv).tanh(), (r * mv).tanh())
    }
}

// ─── Public engine ─────────────────────────────────────────────────────────────

/// The live audio engine.  Create once at startup; keep alive for the program duration.
/// All methods take `&self` — mutation is routed through an interior `Arc<Mutex<>>`.
pub struct AudioEngine {
    state:    Arc<Mutex<AudioState>>,
    /// Kept alive to prevent cpal from stopping the stream when it's dropped.
    _stream:  cpal::Stream,
    /// Device sample rate (informational).
    pub out_rate: u32,
}

impl AudioEngine {
    /// Initialise cpal, open the default output device, start the audio thread.
    /// Returns `Err` if no audio device is available (e.g. headless CI).
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let host     = cpal::default_host();
        let device   = host.default_output_device()
            .ok_or("no default audio output device")?;
        let supported = device.default_output_config()?;

        let channels  = supported.channels() as usize;
        let out_rate  = supported.sample_rate().0;
        let fmt       = supported.sample_format();
        let config    = supported.config();

        let state  = Arc::new(Mutex::new(AudioState::new(out_rate)));
        let stream = build_stream(&device, &config, channels, Arc::clone(&state), fmt)?;
        stream.play()?;

        Ok(Self { state, _stream: stream, out_rate })
    }

    // ── Tone control ─────────────────────────────────────────────────────────

    /// Insert or update the tone at slot `idx`.  At most 64 slots; grows as needed.
    pub fn set_tone(&self, idx: usize, params: ToneParams) {
        if let Ok(mut s) = self.state.lock() {
            while s.tones.len() <= idx { s.tones.push(None); }
            match &mut s.tones[idx] {
                Some(t) => t.params = params,
                slot    => *slot = Some(Tone::new(params)),
            }
        }
    }

    /// Silence and remove the tone at `idx`.
    pub fn clear_tone(&self, idx: usize) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(slot) = s.tones.get_mut(idx) { *slot = None; }
        }
    }

    // ── Listener (camera) ────────────────────────────────────────────────────

    /// Update the listener orientation to match the Ling `set_camera` values.
    pub fn set_listener(&self, cry: f32, sry: f32, crx: f32, srx: f32) {
        if let Ok(mut s) = self.state.lock() {
            s.cry = cry; s.sry = sry;
            s.crx = crx; s.srx = srx;
        }
    }

    // ── BGM ─────────────────────────────────────────────────────────────────

    /// Load a WAV file and start looping it as background music.
    /// Silently ignores missing files so scenes still run in silent environments.
    pub fn load_bgm(&self, path: &str, vol: f32) {
        match load_wav(path) {
            Ok((samples, src_rate)) => {
                if let Ok(mut s) = self.state.lock() {
                    s.bgm = Some(BgmTrack { samples, src_rate, pos: 0.0, volume: vol });
                }
            }
            Err(e) => eprintln!("audio: bgm load failed ({path}): {e}"),
        }
    }

    /// Adjust BGM playback volume without reloading.
    pub fn set_bgm_volume(&self, vol: f32) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(bgm) = &mut s.bgm { bgm.volume = vol; }
        }
    }

    // ── Master ───────────────────────────────────────────────────────────────

    pub fn set_master_volume(&self, vol: f32) {
        if let Ok(mut s) = self.state.lock() { s.master_volume = vol; }
    }
}

// ─── WAV loader ───────────────────────────────────────────────────────────────

fn load_wav(path: &str) -> Result<(Vec<f32>, u32), Box<dyn std::error::Error>> {
    let mut reader   = hound::WavReader::open(path)?;
    let spec         = reader.spec();
    let channels     = spec.channels as usize;
    let src_rate     = spec.sample_rate;

    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => {
            reader.samples::<f32>().filter_map(|s| s.ok()).collect()
        }
        hound::SampleFormat::Int => {
            // Normalise to [-1, 1] regardless of bit depth.
            let max = (1i32 << spec.bits_per_sample.saturating_sub(1)) as f32;
            reader.samples::<i32>().filter_map(|s| s.ok())
                .map(|s| s as f32 / max)
                .collect()
        }
    };

    // Normalise to interleaved stereo.
    let stereo: Vec<f32> = match channels {
        1 => raw.iter().flat_map(|&s| [s, s]).collect(),
        2 => raw,
        n => raw.chunks(n)
                .flat_map(|c| [c[0], if c.len() > 1 { c[1] } else { c[0] }])
                .collect(),
    };

    Ok((stereo, src_rate))
}

// ─── cpal stream builder ──────────────────────────────────────────────────────

fn build_stream(
    device:   &cpal::Device,
    config:   &cpal::StreamConfig,
    channels: usize,
    state:    Arc<Mutex<AudioState>>,
    fmt:      cpal::SampleFormat,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    let err_fn = |e: cpal::StreamError| eprintln!("cpal stream error: {e}");

    Ok(match fmt {
        cpal::SampleFormat::F32 => {
            let st = Arc::clone(&state);
            device.build_output_stream(
                config,
                move |data: &mut [f32], _| fill_f32(data, channels, &st),
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let st = Arc::clone(&state);
            device.build_output_stream(
                config,
                move |data: &mut [i16], _| fill_i16(data, channels, &st),
                err_fn,
                None,
            )?
        }
        _ => {
            // Generic fallback: output as i16.
            let st = Arc::clone(&state);
            device.build_output_stream::<i16, _, _>(
                config,
                move |data: &mut [i16], _| fill_i16(data, channels, &st),
                err_fn,
                None,
            )?
        }
    })
}

/// Fill a `&mut [f32]` buffer (interleaved, `channels` wide).
fn fill_f32(data: &mut [f32], channels: usize, state: &Arc<Mutex<AudioState>>) {
    let ch = channels.max(1);
    if let Ok(mut s) = state.try_lock() {
        for frame in data.chunks_mut(ch) {
            let (l, r) = s.next_sample();
            frame[0] = l;
            if ch > 1 { frame[1] = r; }
            for extra in frame.iter_mut().skip(2) { *extra = 0.0; }
        }
    } else {
        for s in data.iter_mut() { *s = 0.0; }
    }
}

/// Fill a `&mut [i16]` buffer (interleaved, `channels` wide).
fn fill_i16(data: &mut [i16], channels: usize, state: &Arc<Mutex<AudioState>>) {
    let ch = channels.max(1);
    if let Ok(mut s) = state.try_lock() {
        for frame in data.chunks_mut(ch) {
            let (l, r) = s.next_sample();
            frame[0] = (l * 32_767.0) as i16;
            if ch > 1 { frame[1] = (r * 32_767.0) as i16; }
            for extra in frame.iter_mut().skip(2) { *extra = 0; }
        }
    } else {
        for s in data.iter_mut() { *s = 0; }
    }
}
