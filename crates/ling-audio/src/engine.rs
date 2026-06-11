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

/// Oscillator waveform for one-shot UI blips.
#[derive(Clone, Copy, Debug)]
pub enum Wave { Sine, Square, Saw, Triangle }

impl Wave {
    pub fn from_name(s: &str) -> Wave {
        match s.to_ascii_lowercase().as_str() {
            "square" | "sq"   => Wave::Square,
            "saw" | "sawtooth" => Wave::Saw,
            "tri" | "triangle" => Wave::Triangle,
            _ => Wave::Sine,
        }
    }
    #[inline]
    fn sample(self, phase: f32) -> f32 {
        match self {
            Wave::Sine     => (phase * TAU).sin(),
            Wave::Square   => if phase < 0.5 { 1.0 } else { -1.0 },
            Wave::Saw      => phase * 2.0 - 1.0,
            Wave::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
        }
    }
}

/// A fire-and-forget interface sound with a fast attack + exponential decay.
struct Blip {
    freq:  f32,
    amp:   f32,
    wave:  Wave,
    dur:   f32,   // seconds until it's removed
    age:   f32,   // seconds elapsed
    phase: f32,
}

impl Blip {
    /// Advance one sample, returning the (mono) value; envelope folds in here.
    #[inline]
    fn next(&mut self, dt: f32) -> f32 {
        // 5 ms attack ramp, then exponential decay across the remaining duration.
        let atk = 0.005;
        let env = if self.age < atk {
            self.age / atk
        } else {
            (-(self.age - atk) / (self.dur * 0.4 + 1e-4)).exp()
        };
        let s = self.wave.sample(self.phase) * self.amp * env;
        self.phase = (self.phase + self.freq * dt).fract();
        self.age += dt;
        s
    }
    fn done(&self) -> bool { self.age >= self.dur }
}

struct BgmTrack {
    /// Interleaved stereo samples at `src_rate`.
    samples:  Vec<f32>,
    src_rate: u32,
    /// Fractional stereo-pair index (advances by `src_rate / device_rate` per sample).
    pos:      f64,
    volume:   f32,
}

/// Equal-power pan + distance attenuation gains for a world-space point, using
/// the current listener (camera) orientation. Shared by tones, sfx and samples.
#[inline]
fn spatial_gains(cry: f32, sry: f32, crx: f32, srx: f32, room_w: f32, x: f32, y: f32, z: f32) -> (f32, f32) {
    let rz1   = x * sry + z * cry;
    let cam_x = x * cry - z * sry;
    let cam_y = y * crx - rz1 * srx;
    let cam_z = y * srx + rz1 * crx;
    let dist  = (cam_x * cam_x + cam_y * cam_y + cam_z * cam_z).sqrt().max(0.5);
    let atten = (1.0 / (1.0 + dist * 0.18)).clamp(0.0, 1.0);
    let pan   = (cam_x / room_w.max(1.0)).clamp(-1.0, 1.0);
    let angle = (pan + 1.0) * 0.5 * FRAC_PI_2;
    (angle.cos() * atten, angle.sin() * atten)
}

/// A positional (2D/3D/4D) one-shot sound effect with a fast-attack/decay
/// envelope — like a [`Blip`] but spatialized at a world position.
struct SfxVoice {
    x: f32, y: f32, z: f32, w: f32,
    freq: f32, amp: f32, wave: Wave, dur: f32, age: f32, phase: f32, w_phase: f32,
}
impl SfxVoice {
    #[inline]
    fn next(&mut self, dt: f32) -> f32 {
        let atk = 0.005;
        let env = if self.age < atk { self.age / atk }
                  else { (-(self.age - atk) / (self.dur * 0.4 + 1e-4)).exp() };
        let w_mod = (self.w_phase * TAU).sin() * 0.25;
        self.w_phase = (self.w_phase + self.freq * self.w.abs() * 0.007 * dt).fract();
        let f = self.freq * (1.0 + w_mod * 0.06);
        let s = self.wave.sample(self.phase) * self.amp * env;
        self.phase = (self.phase + f * dt).fract();
        self.age += dt;
        s
    }
    fn done(&self) -> bool { self.age >= self.dur }
}

/// A playing instance of a loaded sample buffer at a world position (looping or one-shot).
#[allow(dead_code)] // `w` is reserved for spatial-audio weighting (not yet read)
struct SampleVoice {
    id: u32,
    sample: usize,
    pos: f64,
    x: f32, y: f32, z: f32, w: f32,
    vol: f32, looping: bool, active: bool,
}

/// Stereo feedback delay on the master bus.
struct Delay {
    bl: Vec<f32>, br: Vec<f32>, idx: usize, len: usize, fb: f32, mix: f32,
}
impl Delay {
    fn new(rate: u32) -> Self {
        let cap = (rate as usize * 2).max(1); // up to 2 s
        Self { bl: vec![0.0; cap], br: vec![0.0; cap], idx: 0, len: 0, fb: 0.0, mix: 0.0 }
    }
    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.mix <= 0.0 || self.len == 0 { return (l, r); }
        let read = (self.idx + self.bl.len() - self.len) % self.bl.len();
        let dl = self.bl[read]; let dr = self.br[read];
        self.bl[self.idx] = l + dl * self.fb;
        self.br[self.idx] = r + dr * self.fb;
        self.idx = (self.idx + 1) % self.bl.len();
        (l + dl * self.mix, r + dr * self.mix)
    }
}

/// A single Freeverb-style comb filter.
struct Comb { buf: Vec<f32>, idx: usize, fb: f32, store: f32, damp: f32 }
impl Comb {
    fn new(n: usize, fb: f32) -> Self { Self { buf: vec![0.0; n.max(1)], idx: 0, fb, store: 0.0, damp: 0.2 } }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.buf[self.idx];
        self.store = y * (1.0 - self.damp) + self.store * self.damp;
        self.buf[self.idx] = x + self.store * self.fb;
        self.idx = (self.idx + 1) % self.buf.len();
        y
    }
}
/// A single allpass filter.
struct Allpass { buf: Vec<f32>, idx: usize }
impl Allpass {
    fn new(n: usize) -> Self { Self { buf: vec![0.0; n.max(1)], idx: 0 } }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let buf = self.buf[self.idx];
        let y = -x + buf;
        self.buf[self.idx] = x + buf * 0.5;
        self.idx = (self.idx + 1) % self.buf.len();
        y
    }
}
/// Cheap mono Schroeder reverb mixed back into stereo.
struct Reverb { combs: Vec<Comb>, allpass: Vec<Allpass>, mix: f32 }
impl Reverb {
    fn new(rate: u32) -> Self {
        let s = rate as f32 / 44100.0;
        let comb = |n: usize, fb: f32| Comb::new((n as f32 * s) as usize, fb);
        let ap = |n: usize| Allpass::new((n as f32 * s) as usize);
        Self {
            combs:   vec![comb(1116, 0.84), comb(1188, 0.83), comb(1277, 0.82), comb(1356, 0.81)],
            allpass: vec![ap(225), ap(556)],
            mix: 0.0,
        }
    }
    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.mix <= 0.0 { return (l, r); }
        let x = (l + r) * 0.5;
        let mut y = 0.0;
        for c in &mut self.combs { y += c.process(x); }
        y *= 0.25;
        for a in &mut self.allpass { y = a.process(y); }
        (l + y * self.mix, r + y * self.mix)
    }
}

/// Master 2-pole low-pass (smoothed cutoff) for muffled / underwater scenes.
/// `cutoff` ∈ (0,1]; 1.0 ≈ bypass.
struct LowPass { yl: [f32; 2], yr: [f32; 2], cutoff: f32, target: f32 }
impl LowPass {
    fn new() -> Self { Self { yl: [0.0; 2], yr: [0.0; 2], cutoff: 1.0, target: 1.0 } }
    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        // glide toward target to avoid zipper noise
        self.cutoff += (self.target - self.cutoff) * 0.001;
        if self.cutoff >= 0.999 { return (l, r); }
        // map cutoff01 → one-pole coefficient (exp so low values are very dark)
        let a = (self.cutoff * self.cutoff).clamp(0.0008, 1.0);
        self.yl[0] += a * (l - self.yl[0]); self.yl[1] += a * (self.yl[0] - self.yl[1]);
        self.yr[0] += a * (r - self.yr[0]); self.yr[1] += a * (self.yr[0] - self.yr[1]);
        (self.yl[1], self.yr[1])
    }
}

struct AudioState {
    tones:         Vec<Option<Tone>>,
    blips:         Vec<Blip>,
    sfx:           Vec<SfxVoice>,
    samples:       Vec<(std::sync::Arc<Vec<f32>>, u32)>, // (mono buffer, src rate)
    sample_voices: Vec<SampleVoice>,
    next_voice_id: u32,
    delay:         Delay,
    reverb:        Reverb,
    lowpass:       LowPass,
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
            blips:         Vec::new(),
            sfx:           Vec::new(),
            samples:       Vec::new(),
            sample_voices: Vec::new(),
            next_voice_id: 1,
            delay:         Delay::new(sample_rate),
            reverb:        Reverb::new(sample_rate),
            lowpass:       LowPass::new(),
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

        // ── One-shot UI blips (centred, no spatialisation) ───────────────────
        if !self.blips.is_empty() {
            let mut mono = 0.0f32;
            for b in &mut self.blips { mono += b.next(dt); }
            self.blips.retain(|b| !b.done());
            l += mono;
            r += mono;
        }

        // ── Positional one-shot SFX (2D/3D/4D) ───────────────────────────────
        if !self.sfx.is_empty() {
            for v in &mut self.sfx {
                let (lg, rg) = spatial_gains(cry, sry, crx, srx, room_w, v.x, v.y, v.z);
                let s = v.next(dt);
                l += s * lg;
                r += s * rg;
            }
            self.sfx.retain(|v| !v.done());
        }

        // ── Positional sample voices (one-shot or looping) ───────────────────
        if !self.sample_voices.is_empty() {
            let out_rate = self.sample_rate as f64;
            for v in &mut self.sample_voices {
                if !v.active { continue; }
                let (buf, src_rate) = match self.samples.get(v.sample) { Some(s) => s, None => { v.active = false; continue; } };
                let n = buf.len();
                if n < 2 { v.active = false; continue; }
                let idx = v.pos as usize;
                let frac = (v.pos - idx as f64) as f32;
                let s = if idx + 1 < n { buf[idx] + (buf[idx + 1] - buf[idx]) * frac } else { buf[idx.min(n - 1)] };
                let (lg, rg) = spatial_gains(cry, sry, crx, srx, room_w, v.x, v.y, v.z);
                l += s * v.vol * lg;
                r += s * v.vol * rg;
                v.pos += *src_rate as f64 / out_rate;
                if v.pos as usize >= n - 1 {
                    if v.looping { v.pos = 0.0; } else { v.active = false; }
                }
            }
            self.sample_voices.retain(|v| v.active);
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

        // ── Master FX chain: delay → reverb → low-pass → volume ──────────────
        let (l, r) = self.delay.process(l, r);
        let (l, r) = self.reverb.process(l, r);
        let (l, r) = self.lowpass.process(l, r);
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

    /// Fire a one-shot interface sound (fast attack, exponential decay).
    /// `dur` is in seconds; up to 32 blips overlap before the oldest is dropped.
    pub fn blip(&self, freq: f32, amp: f32, dur: f32, wave: Wave) {
        if let Ok(mut s) = self.state.lock() {
            if s.blips.len() >= 32 { s.blips.remove(0); }
            s.blips.push(Blip { freq, amp, wave, dur: dur.max(0.01), age: 0.0, phase: 0.0 });
        }
    }

    /// Fire a positional (2D/3D/4D) one-shot sound effect at a world point.
    pub fn sfx(&self, x: f32, y: f32, z: f32, w: f32, freq: f32, amp: f32, dur: f32, wave: Wave) {
        if let Ok(mut s) = self.state.lock() {
            if s.sfx.len() >= 64 { s.sfx.remove(0); }
            s.sfx.push(SfxVoice { x, y, z, w, freq, amp, wave, dur: dur.max(0.01), age: 0.0, phase: 0.0, w_phase: 0.0 });
        }
    }

    /// Register a mono sample buffer (decoded elsewhere), returning its id.
    pub fn add_sample(&self, mono: Vec<f32>, src_rate: u32) -> usize {
        if let Ok(mut s) = self.state.lock() {
            s.samples.push((std::sync::Arc::new(mono), src_rate.max(1)));
            s.samples.len() - 1
        } else { 0 }
    }

    /// Play a loaded sample at a world position (looping or one-shot). Returns a voice id.
    pub fn play_sample(&self, id: usize, x: f32, y: f32, z: f32, w: f32, vol: f32, looping: bool) -> u32 {
        if let Ok(mut s) = self.state.lock() {
            if id >= s.samples.len() { return 0; }
            let vid = s.next_voice_id; s.next_voice_id += 1;
            if s.sample_voices.len() >= 64 { s.sample_voices.remove(0); }
            s.sample_voices.push(SampleVoice { id: vid, sample: id, pos: 0.0, x, y, z, w, vol, looping, active: true });
            vid
        } else { 0 }
    }

    /// Stop a sample voice by id.
    pub fn stop_sample(&self, voice: u32) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(v) = s.sample_voices.iter_mut().find(|v| v.id == voice) { v.active = false; }
        }
    }

    // ── master FX ─────────────────────────────────────────────────────────────
    pub fn fx_delay(&self, time_s: f32, feedback: f32, mix: f32) {
        if let Ok(mut s) = self.state.lock() {
            let cap = s.delay.bl.len();
            s.delay.len = ((time_s.max(0.0) * s.sample_rate as f32) as usize).min(cap.saturating_sub(1));
            s.delay.fb = feedback.clamp(0.0, 0.95);
            s.delay.mix = mix.clamp(0.0, 1.0);
        }
    }
    pub fn fx_reverb(&self, mix: f32) {
        if let Ok(mut s) = self.state.lock() { s.reverb.mix = mix.clamp(0.0, 1.0); }
    }
    /// Master low-pass cutoff ∈ (0,1]; 1.0 ≈ open, lower = muffled/underwater.
    pub fn fx_lowpass(&self, cutoff01: f32) {
        if let Ok(mut s) = self.state.lock() { s.lowpass.target = cutoff01.clamp(0.0, 1.0); }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sfx_voice_envelopes_and_ends() {
        let mut v = SfxVoice { x: 0.0, y: 0.0, z: 0.0, w: 1.0, freq: 440.0, amp: 0.5,
                               wave: Wave::Sine, dur: 0.02, age: 0.0, phase: 0.0, w_phase: 0.0 };
        let dt = 1.0 / 44100.0;
        let mut peak = 0.0f32;
        let mut steps = 0;
        while !v.done() && steps < 44100 { peak = peak.max(v.next(dt).abs()); steps += 1; }
        assert!(peak > 0.01, "sfx should produce sound");
        assert!(v.done(), "sfx should finish after its duration");
    }

    #[test]
    fn sample_voice_loops_and_oneshot_stops() {
        let mut st = AudioState::new(44100);
        st.samples.push((std::sync::Arc::new(vec![0.5f32; 100]), 44100));
        // looping voice survives past the buffer end
        st.sample_voices.push(SampleVoice { id: 1, sample: 0, pos: 0.0, x: 0.0, y: 0.0, z: 0.0, w: 1.0, vol: 1.0, looping: true, active: true });
        for _ in 0..500 { let _ = st.next_sample(); }
        assert_eq!(st.sample_voices.len(), 1, "looping voice should still be alive");
        // one-shot voice deactivates after the buffer
        st.sample_voices.push(SampleVoice { id: 2, sample: 0, pos: 0.0, x: 0.0, y: 0.0, z: 0.0, w: 1.0, vol: 1.0, looping: false, active: true });
        for _ in 0..500 { let _ = st.next_sample(); }
        assert!(st.sample_voices.iter().all(|v| v.id != 2), "one-shot should have stopped");
    }

    #[test]
    fn master_fx_stay_finite() {
        let mut st = AudioState::new(44100);
        st.delay.len = 2000; st.delay.fb = 0.6; st.delay.mix = 0.4;
        st.reverb.mix = 0.5;
        st.lowpass.target = 0.2; st.lowpass.cutoff = 0.2;
        // feed a tone and ensure the FX chain never blows up
        st.tones[0] = Some(Tone::new(ToneParams { freq: 220.0, amp: 0.8, ..Default::default() }));
        for _ in 0..44100 {
            let (l, r) = st.next_sample();
            assert!(l.is_finite() && r.is_finite() && l.abs() <= 1.0 && r.abs() <= 1.0);
        }
    }
}
