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

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::f32::consts::{FRAC_PI_2, TAU};
use std::sync::{Arc, Mutex};

// ─── Public types ─────────────────────────────────────────────────────────────

/// Parameters for one positional tone.
#[derive(Clone, Debug)]
pub struct ToneParams {
    /// World-space position of the sound source.
    pub x: f32,
    pub y: f32,
    pub z: f32,
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
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
            freq: 220.0,
            amp: 0.15,
            lfo_rate: 0.5,
            lfo_depth: 0.02,
        }
    }
}

// ─── Internal state ────────────────────────────────────────────────────────────

struct Tone {
    params: ToneParams,
    phase: f32,     // carrier oscillator phase [0, 1)
    lfo_phase: f32, // LFO phase [0, 1)
    w_phase: f32,   // 4D sub-oscillator phase [0, 1)
    cur_amp: f32,   // smoothed amplitude (glides to params.amp — de-click)
    cur_freq: f32,  // smoothed frequency (glides to params.freq — de-zipper)
}

impl Tone {
    fn new(params: ToneParams) -> Self {
        let (a, f) = (params.amp, params.freq);
        Self {
            params,
            phase: 0.0,
            lfo_phase: 0.0,
            w_phase: 0.0,
            cur_amp: a,
            cur_freq: f,
        }
    }
}

/// Oscillator waveform for one-shot UI blips.
#[derive(Clone, Copy, Debug)]
pub enum Wave {
    Sine,
    Square,
    Saw,
    Triangle,
    Noise,
}

impl Wave {
    pub fn from_name(s: &str) -> Wave {
        match s.to_ascii_lowercase().as_str() {
            "square" | "sq" => Wave::Square,
            "saw" | "sawtooth" => Wave::Saw,
            "tri" | "triangle" => Wave::Triangle,
            "noise" | "wn" | "ns" => Wave::Noise,
            _ => Wave::Sine,
        }
    }

    #[inline]
    fn sample(self, phase: f32) -> f32 {
        match self {
            Wave::Sine => (phase * TAU).sin(),
            Wave::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            },
            Wave::Saw => phase * 2.0 - 1.0,
            Wave::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
            Wave::Noise => 0.0, // handled per-sample via LCG in voice render
        }
    }
}

/// A fire-and-forget interface sound with a fast attack + exponential decay.
struct Blip {
    freq: f32,
    amp: f32,
    wave: Wave,
    dur: f32, // seconds until it's removed
    age: f32, // seconds elapsed
    phase: f32,
    seed: u32, // LCG state for Wave::Noise
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
        let raw = if let Wave::Noise = self.wave {
            self.seed = self.seed.wrapping_mul(1664525).wrapping_add(1013904223);
            ((self.seed >> 8) as f32 / 8_388_608.0) - 1.0
        } else {
            self.wave.sample(self.phase)
        };
        let s = raw * self.amp * env;
        self.phase = (self.phase + self.freq * dt).fract();
        self.age += dt;
        s
    }

    fn done(&self) -> bool {
        self.age >= self.dur
    }
}

struct BgmTrack {
    /// Interleaved stereo samples at `src_rate`.
    samples: Vec<f32>,
    src_rate: u32,
    /// Fractional stereo-pair index (advances by `src_rate / device_rate` per sample).
    pos: f64,
    volume: f32,
}

/// Equal-power pan + distance attenuation gains for a world-space point, using
/// the current listener (camera) orientation. Shared by tones, sfx and samples.
#[allow(clippy::too_many_arguments)]
#[inline]
fn spatial_gains(
    cry: f32,
    sry: f32,
    crx: f32,
    srx: f32,
    lx: f32,
    ly: f32,
    lz: f32,
    room_w: f32,
    x: f32,
    y: f32,
    z: f32,
) -> (f32, f32) {
    let (x, y, z) = (x - lx, y - ly, z - lz); // make the sound relative to the listener (camera) position
    let rz1 = x * sry + z * cry;
    let cam_x = x * cry - z * sry;
    let cam_y = y * crx - rz1 * srx;
    let cam_z = y * srx + rz1 * crx;
    let dist = (cam_x * cam_x + cam_y * cam_y + cam_z * cam_z)
        .sqrt()
        .max(0.5);
    let atten = (1.0 / (1.0 + dist * 0.18)).clamp(0.0, 1.0);
    let pan = (cam_x / room_w.max(1.0)).clamp(-1.0, 1.0);
    let angle = (pan + 1.0) * 0.5 * FRAC_PI_2;
    (angle.cos() * atten, angle.sin() * atten)
}

/// A positional (2D/3D/4D) one-shot sound effect with a fast-attack/decay
/// envelope — like a [`Blip`] but spatialized at a world position.
struct SfxVoice {
    x: f32,
    y: f32,
    z: f32,
    w: f32,
    freq: f32,
    amp: f32,
    wave: Wave,
    dur: f32,
    age: f32,
    phase: f32,
    w_phase: f32,
    seed: u32,
}
impl SfxVoice {
    #[inline]
    fn next(&mut self, dt: f32) -> f32 {
        let atk = 0.005;
        let env = if self.age < atk {
            self.age / atk
        } else {
            (-(self.age - atk) / (self.dur * 0.4 + 1e-4)).exp()
        };
        let w_mod = (self.w_phase * TAU).sin() * 0.25;
        self.w_phase = (self.w_phase + self.freq * self.w.abs() * 0.007 * dt).fract();
        let f = self.freq * (1.0 + w_mod * 0.06);
        let raw = if let Wave::Noise = self.wave {
            self.seed = self.seed.wrapping_mul(1664525).wrapping_add(1013904223);
            ((self.seed >> 8) as f32 / 8_388_608.0) - 1.0
        } else {
            self.wave.sample(self.phase)
        };
        let s = raw * self.amp * env;
        self.phase = (self.phase + f * dt).fract();
        self.age += dt;
        s
    }

    fn done(&self) -> bool {
        self.age >= self.dur
    }
}

/// A playing instance of a loaded sample buffer at a world position (looping or one-shot).
#[allow(dead_code)] // `w` is reserved for spatial-audio weighting (not yet read)
struct SampleVoice {
    id: u32,
    sample: usize,
    pos: f64,
    x: f32,
    y: f32,
    z: f32,
    w: f32,
    vol: f32,
    looping: bool,
    active: bool,
}

/// YIN-YANG morph voice — one voice that spans the acoustic↔digital continuum.
/// "Light" core = a Karplus-Strong physical model (excited string/air/membrane →
/// organic plucked/bowed/blown/struck tones). "Dark" core = an FM operator pair
/// through a bit/sample-rate crusher (digital grit). The `morph` dial (0=light ..
/// 1=dark) crossfades the two AND tilts the shaping — more FM index, deeper crush
/// and more tanh drive toward dark. Material presets pick excitation + damping so
/// a single voice covers a wide palette. Self-contained DSP; existing voices are
/// untouched (fully backwards compatible).
#[allow(dead_code)] // `w` reserved for 4D spatial weighting (not yet read)
struct MorphVoice {
    freq: f32,
    ks: Vec<f32>, // Karplus-Strong delay line (the resonator)
    ks_len: usize,
    ks_idx: usize,
    ks_damp: f32, // brightness: high-freq retained per round-trip
    ks_fb: f32,   // string feedback (ring length)
    ks_lp: f32,   // 1-pole damping state
    excite: u8,   // 0 pluck · 1 bow · 2 blow · 3 strike
    seed: u32,
    car: f32, // FM carrier phase
    md: f32,  // FM modulator phase
    ratio: f32,
    index: f32,
    hold: f32,      // crusher sample-hold
    crush_acc: f32, // sample-rate-reduction accumulator
    morph: f32,
    amp: f32,
    dur: f32,
    age: f32,
    x: f32,
    y: f32,
    z: f32,
    w: f32,
}
impl MorphVoice {
    #[allow(clippy::too_many_arguments)]
    fn new(
        sr: f32,
        freq: f32,
        amp: f32,
        dur: f32,
        material: u8,
        morph: f32,
        x: f32,
        y: f32,
        z: f32,
        w: f32,
    ) -> Self {
        let freq = freq.clamp(20.0, sr * 0.45);
        let ks_len = ((sr / freq).round() as usize).clamp(2, 4096);
        // material → (excite, damp, feedback, fm ratio, fm index)
        let (excite, ks_damp, ks_fb, ratio, index) = match material {
            0 => (1u8, 0.55, 0.992, 2.0, 0.5), // Bowed Silk (sustained string)
            1 => (0u8, 0.80, 0.990, 3.0, 0.9), // Plucked Bamboo (bright pluck)
            2 => (2u8, 0.48, 0.991, 1.0, 0.4), // Blown Jade (air column)
            _ => (3u8, 0.90, 0.997, 1.414, 1.3), // Struck Bronze (long inharmonic metal)
        };
        let mut s = freq.to_bits().wrapping_mul(2654435761).wrapping_add(1);
        let mut ks = vec![0.0f32; ks_len];
        if excite == 0 || excite == 3 {
            // pluck/strike: prefill the line with a noise burst so it rings
            for v in ks.iter_mut() {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                *v = ((s >> 9) as f32 / 4_194_304.0) - 1.0;
            }
        }
        Self {
            freq,
            ks,
            ks_len,
            ks_idx: 0,
            ks_damp,
            ks_fb,
            ks_lp: 0.0,
            excite,
            seed: s | 1,
            car: 0.0,
            md: 0.0,
            ratio,
            index,
            hold: 0.0,
            crush_acc: 0.0,
            morph: morph.clamp(0.0, 1.0),
            amp,
            dur: dur.max(0.02),
            age: 0.0,
            x,
            y,
            z,
            w,
        }
    }

    #[inline]
    fn noise(&mut self) -> f32 {
        self.seed = self.seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((self.seed >> 9) as f32 / 4_194_304.0) - 1.0
    }

    #[inline]
    fn next(&mut self, dt: f32) -> f32 {
        let atk = 0.006;
        let env = if self.age < atk {
            self.age / atk
        } else {
            (-(self.age - atk) / (self.dur * 0.5 + 1e-4)).exp()
        };
        // ── excitation into the resonator ──
        let n = self.noise();
        let exc = match self.excite {
            1 => n * 0.5 * env,  // bow: sustained noise
            2 => n * 0.32 * env, // blow: breath
            3 => {
                if self.age < 0.0015 {
                    n * 1.5
                } else {
                    0.0
                }
            }, // strike: impulse (+prefill)
            _ => {
                if self.age < 0.004 {
                    n * 0.4
                } else {
                    0.0
                }
            }, // pluck: brief top-up (+prefill)
        };
        // ── organic core: Karplus-Strong ──
        let cur = self.ks[self.ks_idx];
        let nxt = self.ks[(self.ks_idx + 1) % self.ks_len];
        let avg = (cur + nxt) * 0.5;
        self.ks_lp += (avg - self.ks_lp) * self.ks_damp;
        self.ks[self.ks_idx] = self.ks_lp * self.ks_fb + exc;
        self.ks_idx = (self.ks_idx + 1) % self.ks_len;
        let organic = cur;
        // ── digital core: FM operator pair ──
        let idx = self.index * (0.3 + self.morph * 1.8);
        let modv = (self.md * TAU).sin();
        let fm = ((self.car + modv * idx) * TAU).sin();
        self.car = (self.car + self.freq * dt).fract();
        self.md = (self.md + self.freq * self.ratio * dt).fract();
        // ── bit + sample-rate crush (deeper toward dark) ──
        let step = 1.0 + self.morph * 10.0;
        self.crush_acc += 1.0;
        if self.crush_acc >= step {
            self.crush_acc -= step;
            let bits = 11.0 - self.morph * 8.0;
            let q = 2f32.powf(bits);
            self.hold = (fm * q).round() / q;
        }
        let digital = self.hold;
        // ── morph crossfade + dark drive ──
        let m = self.morph;
        let mut out = organic * (1.0 - m) + digital * m;
        out = (out * (1.0 + m * 3.5)).tanh();
        self.age += dt;
        out * self.amp * env
    }

    fn done(&self) -> bool {
        self.age >= self.dur
    }
}

/// Stereo feedback delay on the master bus.
struct Delay {
    bl: Vec<f32>,
    br: Vec<f32>,
    idx: usize,
    len: usize,
    fb: f32,
    mix: f32,
}
impl Delay {
    fn new(rate: u32) -> Self {
        let cap = (rate as usize * 2).max(1); // up to 2 s
        Self {
            bl: vec![0.0; cap],
            br: vec![0.0; cap],
            idx: 0,
            len: 0,
            fb: 0.0,
            mix: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.mix <= 0.0 || self.len == 0 {
            return (l, r);
        }
        let read = (self.idx + self.bl.len() - self.len) % self.bl.len();
        let dl = self.bl[read];
        let dr = self.br[read];
        self.bl[self.idx] = l + dl * self.fb;
        self.br[self.idx] = r + dr * self.fb;
        self.idx = (self.idx + 1) % self.bl.len();
        (l + dl * self.mix, r + dr * self.mix)
    }
}

/// A single Freeverb-style comb filter.
struct Comb {
    buf: Vec<f32>,
    idx: usize,
    fb: f32,
    store: f32,
    damp: f32,
}
impl Comb {
    fn new(n: usize, fb: f32) -> Self {
        Self { buf: vec![0.0; n.max(1)], idx: 0, fb, store: 0.0, damp: 0.2 }
    }

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
struct Allpass {
    buf: Vec<f32>,
    idx: usize,
}
impl Allpass {
    fn new(n: usize) -> Self {
        Self { buf: vec![0.0; n.max(1)], idx: 0 }
    }

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
struct Reverb {
    combs: Vec<Comb>,
    allpass: Vec<Allpass>,
    mix: f32,
}
impl Reverb {
    fn new(rate: u32) -> Self {
        let s = rate as f32 / 44100.0;
        let comb = |n: usize, fb: f32| Comb::new((n as f32 * s) as usize, fb);
        let ap = |n: usize| Allpass::new((n as f32 * s) as usize);
        Self {
            combs: vec![
                comb(1116, 0.84),
                comb(1188, 0.83),
                comb(1277, 0.82),
                comb(1356, 0.81),
            ],
            allpass: vec![ap(225), ap(556)],
            mix: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.mix <= 0.0 {
            return (l, r);
        }
        let x = (l + r) * 0.5;
        let mut y = 0.0;
        for c in &mut self.combs {
            y += c.process(x);
        }
        y *= 0.25;
        for a in &mut self.allpass {
            y = a.process(y);
        }
        (l + y * self.mix, r + y * self.mix)
    }
}

/// Master 2-pole low-pass (smoothed cutoff) for muffled / underwater scenes.
/// `cutoff` ∈ (0,1]; 1.0 ≈ bypass.
struct LowPass {
    yl: [f32; 2],
    yr: [f32; 2],
    cutoff: f32,
    target: f32,
}
impl LowPass {
    fn new() -> Self {
        Self { yl: [0.0; 2], yr: [0.0; 2], cutoff: 1.0, target: 1.0 }
    }

    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        // glide toward target to avoid zipper noise
        self.cutoff += (self.target - self.cutoff) * 0.001;
        if self.cutoff >= 0.999 {
            return (l, r);
        }
        // map cutoff01 → one-pole coefficient (exp so low values are very dark)
        let a = (self.cutoff * self.cutoff).clamp(0.0008, 1.0);
        self.yl[0] += a * (l - self.yl[0]);
        self.yl[1] += a * (self.yl[0] - self.yl[1]);
        self.yr[0] += a * (r - self.yr[0]);
        self.yr[1] += a * (self.yr[0] - self.yr[1]);
        (self.yl[1], self.yr[1])
    }
}

struct AudioState {
    tones: Vec<Option<Tone>>,
    blips: Vec<Blip>,
    sfx: Vec<SfxVoice>,
    morph_voices: Vec<MorphVoice>, // YIN-YANG morph synth voices
    samples: Vec<(std::sync::Arc<Vec<f32>>, u32)>, // (mono buffer, src rate)
    sample_voices: Vec<SampleVoice>,
    next_voice_id: u32,
    delay: Delay,
    reverb: Reverb,
    lowpass: LowPass,
    bgm: Option<BgmTrack>,
    master_volume: f32,
    // Camera orientation — mirrors Camera3D cry/sry/crx/srx.
    cry: f32,
    sry: f32,
    crx: f32,
    srx: f32,
    /// Half-width of the room (used to normalise the pan value).
    room_w: f32,
    /// Listener (camera) world position — sounds are spatialised relative to it.
    lx: f32,
    ly: f32,
    lz: f32,
    sample_rate: u32,
}

impl AudioState {
    fn new(sample_rate: u32) -> Self {
        Self {
            tones: (0..16).map(|_| None).collect(),
            blips: Vec::new(),
            sfx: Vec::new(),
            morph_voices: Vec::new(),
            samples: Vec::new(),
            sample_voices: Vec::new(),
            next_voice_id: 1,
            delay: Delay::new(sample_rate),
            reverb: Reverb::new(sample_rate),
            lowpass: LowPass::new(),
            bgm: None,
            master_volume: 0.5,
            cry: 1.0,
            sry: 0.0,
            crx: 1.0,
            srx: 0.0,
            room_w: 9.0,
            lx: 0.0,
            ly: 0.0,
            lz: 0.0,
            sample_rate,
        }
    }

    /// Generate one stereo (L, R) sample pair.
    #[inline]
    fn next_sample(&mut self) -> (f32, f32) {
        // Copy scalars so borrowck doesn't complain about &mut self during tone loop.
        let cry = self.cry;
        let sry = self.sry;
        let crx = self.crx;
        let srx = self.srx;
        let room_w = self.room_w;
        let lx = self.lx;
        let ly = self.ly;
        let lz = self.lz;
        let dt = 1.0 / self.sample_rate as f32;

        let mut l = 0.0f32;
        let mut r = 0.0f32;

        for slot in &mut self.tones {
            let tone = match slot.as_mut() {
                Some(t) => t,
                None => continue,
            };
            // Copy params to locals (no borrow held → free to mutate the tone below).
            let (px, py, pz, pfreq, pamp, plfo_depth, plfo_rate, pw) = {
                let p = &tone.params;
                (p.x, p.y, p.z, p.freq, p.amp, p.lfo_depth, p.lfo_rate, p.w)
            };
            // ── De-click / de-zipper: glide amp & freq toward their targets ──
            // The game re-sets tone params every frame; jumping amp/freq makes the
            // waveform discontinuous → audible snaps/pops. One-pole smoothing fixes it.
            tone.cur_amp += (pamp - tone.cur_amp) * 0.004;
            tone.cur_freq += (pfreq - tone.cur_freq) * 0.012;

            // ── World → camera-space ─────────────────────────────────────────
            // Apply Y-rotation (yaw) then X-rotation (pitch) — same as Camera3D.
            let rz1 = px * sry + pz * cry;
            let cam_x = px * cry - pz * sry;
            let cam_y = py * crx - rz1 * srx;
            let cam_z = py * srx + rz1 * crx;

            // ── Spatial attenuation ──────────────────────────────────────────
            let dist = (cam_x * cam_x + cam_y * cam_y + cam_z * cam_z)
                .sqrt()
                .max(0.5);
            let atten = (1.0 / (1.0 + dist * 0.18)).clamp(0.0, 1.0);

            // ── Equal-power panning ──────────────────────────────────────────
            let pan = (cam_x / room_w.max(1.0)).clamp(-1.0, 1.0);
            let angle = (pan + 1.0) * 0.5 * FRAC_PI_2;
            let l_gain = angle.cos() * atten;
            let r_gain = angle.sin() * atten;

            // ── LFO (vibrato) ────────────────────────────────────────────────
            let lfo_mod = (tone.lfo_phase * TAU).sin() * plfo_depth;
            tone.lfo_phase = (tone.lfo_phase + plfo_rate * dt).fract();

            // ── 4D sub-oscillator ─────────────────────────────────────────────
            // W drives a slow cross-modulator; the phase drift creates
            // hyperdimensional beating that is unique per sound-source.
            let w_mod = (tone.w_phase * TAU).sin() * 0.25;
            let w_freq = tone.cur_freq * pw.abs() * 0.007;
            tone.w_phase = (tone.w_phase + w_freq * dt).fract();

            // ── Carrier oscillator (smoothed amp & freq) ──────────────────────
            let inst_freq = tone.cur_freq * (1.0 + lfo_mod) * (1.0 + w_mod * 0.08);
            let sample = (tone.phase * TAU).sin() * tone.cur_amp;
            tone.phase = (tone.phase + inst_freq * dt).fract();

            l += sample * l_gain;
            r += sample * r_gain;
        }

        // ── One-shot UI blips (centred, no spatialisation) ───────────────────
        if !self.blips.is_empty() {
            let mut mono = 0.0f32;
            for b in &mut self.blips {
                mono += b.next(dt);
            }
            self.blips.retain(|b| !b.done());
            l += mono;
            r += mono;
        }

        // ── Positional one-shot SFX (2D/3D/4D) ───────────────────────────────
        if !self.sfx.is_empty() {
            for v in &mut self.sfx {
                let (lg, rg) = spatial_gains(cry, sry, crx, srx, lx, ly, lz, room_w, v.x, v.y, v.z);
                let s = v.next(dt);
                l += s * lg;
                r += s * rg;
            }
            self.sfx.retain(|v| !v.done());
        }

        // ── YIN-YANG morph synth voices (physical-model ↔ FM/crush) ──────────
        if !self.morph_voices.is_empty() {
            for v in &mut self.morph_voices {
                let (lg, rg) = spatial_gains(cry, sry, crx, srx, lx, ly, lz, room_w, v.x, v.y, v.z);
                let s = v.next(dt);
                l += s * lg;
                r += s * rg;
            }
            self.morph_voices.retain(|v| !v.done());
        }

        // ── Positional sample voices (one-shot or looping) ───────────────────
        if !self.sample_voices.is_empty() {
            let out_rate = self.sample_rate as f64;
            for v in &mut self.sample_voices {
                if !v.active {
                    continue;
                }
                let (buf, src_rate) = match self.samples.get(v.sample) {
                    Some(s) => s,
                    None => {
                        v.active = false;
                        continue;
                    },
                };
                let n = buf.len();
                if n < 2 {
                    v.active = false;
                    continue;
                }
                let idx = v.pos as usize;
                let frac = (v.pos - idx as f64) as f32;
                let s = if idx + 1 < n {
                    buf[idx] + (buf[idx + 1] - buf[idx]) * frac
                } else {
                    buf[idx.min(n - 1)]
                };
                let (lg, rg) = spatial_gains(cry, sry, crx, srx, lx, ly, lz, room_w, v.x, v.y, v.z);
                l += s * v.vol * lg;
                r += s * v.vol * rg;
                v.pos += *src_rate as f64 / out_rate;
                if v.pos as usize >= n - 1 {
                    if v.looping {
                        v.pos = 0.0;
                    } else {
                        v.active = false;
                    }
                }
            }
            self.sample_voices.retain(|v| v.active);
        }

        // ── BGM ─────────────────────────────────────────────────────────────
        if let Some(bgm) = &mut self.bgm {
            let n_pairs = bgm.samples.len() / 2;
            if n_pairs >= 2 {
                let ratio = bgm.src_rate as f64 / self.sample_rate as f64;
                let idx = bgm.pos as usize;
                let frac = (bgm.pos - idx as f64) as f32;
                let nxt = (idx + 1) % n_pairs;

                let bl =
                    bgm.samples[idx * 2] + (bgm.samples[nxt * 2] - bgm.samples[idx * 2]) * frac;
                let br = bgm.samples[idx * 2 + 1]
                    + (bgm.samples[nxt * 2 + 1] - bgm.samples[idx * 2 + 1]) * frac;

                l += bl * bgm.volume;
                r += br * bgm.volume;

                bgm.pos += ratio;
                if bgm.pos as usize >= n_pairs.saturating_sub(1) {
                    bgm.pos = 0.0; // loop
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
    state: Arc<Mutex<AudioState>>,
    /// Kept alive to prevent cpal from stopping the stream when it's dropped.
    _stream: cpal::Stream,
    /// Device sample rate (informational).
    pub out_rate: u32,
}

impl AudioEngine {
    /// Initialise cpal, open the default output device, start the audio thread.
    /// Returns `Err` if no audio device is available (e.g. headless CI).
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("no default audio output device")?;
        let supported = device.default_output_config()?;

        let channels = supported.channels() as usize;
        let out_rate = supported.sample_rate().0;
        let fmt = supported.sample_format();
        let config = supported.config();

        let state = Arc::new(Mutex::new(AudioState::new(out_rate)));
        let stream = build_stream(&device, &config, channels, Arc::clone(&state), fmt)?;
        stream.play()?;

        Ok(Self { state, _stream: stream, out_rate })
    }

    // ── Tone control ─────────────────────────────────────────────────────────

    /// Insert or update the tone at slot `idx`.  At most 64 slots; grows as needed.
    pub fn set_tone(&self, idx: usize, params: ToneParams) {
        if let Ok(mut s) = self.state.lock() {
            while s.tones.len() <= idx {
                s.tones.push(None);
            }
            match &mut s.tones[idx] {
                Some(t) => t.params = params,
                slot => *slot = Some(Tone::new(params)),
            }
        }
    }

    /// Fire a one-shot interface sound (fast attack, exponential decay).
    /// `dur` is in seconds; up to 32 blips overlap before the oldest is dropped.
    pub fn blip(&self, freq: f32, amp: f32, dur: f32, wave: Wave) {
        if let Ok(mut s) = self.state.lock() {
            if s.blips.len() >= 32 {
                s.blips.remove(0);
            }
            s.blips.push(Blip {
                freq,
                amp,
                wave,
                dur: dur.max(0.01),
                age: 0.0,
                phase: 0.0,
                seed: freq.to_bits().wrapping_mul(2654435761).wrapping_add(1),
            });
        }
    }

    /// Fire a positional (2D/3D/4D) one-shot sound effect at a world point.
    #[allow(clippy::too_many_arguments)]
    pub fn sfx(&self, x: f32, y: f32, z: f32, w: f32, freq: f32, amp: f32, dur: f32, wave: Wave) {
        if let Ok(mut s) = self.state.lock() {
            if s.sfx.len() >= 64 {
                s.sfx.remove(0);
            }
            s.sfx.push(SfxVoice {
                x,
                y,
                z,
                w,
                freq,
                amp,
                wave,
                dur: dur.max(0.01),
                age: 0.0,
                phase: 0.0,
                w_phase: 0.0,
                seed: freq.to_bits().wrapping_mul(2654435761).wrapping_add(1),
            });
        }
    }

    /// Stop every one-shot effect immediately — the SFX pool, the YIN-YANG morph
    /// voices, and sample voices — so an effect tail (slash / monster callout /
    /// explosion) can't bleed across a scene transition. Continuous tones and
    /// music are left alone (silence those via `tone(..,amp=0)` / `stop_music`).
    pub fn stop_all_sfx(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.sfx.clear();
            s.morph_voices.clear();
            s.sample_voices.clear();
        }
    }

    /// Fire a YIN-YANG morph synth note. `material`: 0 bowed-string · 1 plucked ·
    /// 2 blown · 3 struck-metal. `morph` 0=light/acoustic .. 1=dark/digital
    /// (FM + bit/sample crush + drive). Up to 32 morph voices; oldest dropped.
    #[allow(clippy::too_many_arguments)]
    pub fn morph_note(
        &self,
        x: f32,
        y: f32,
        z: f32,
        w: f32,
        freq: f32,
        amp: f32,
        dur: f32,
        material: u8,
        morph: f32,
    ) {
        if let Ok(mut s) = self.state.lock() {
            if s.morph_voices.len() >= 32 {
                s.morph_voices.remove(0);
            }
            let sr = s.sample_rate as f32;
            s.morph_voices.push(MorphVoice::new(
                sr, freq, amp, dur, material, morph, x, y, z, w,
            ));
        }
    }

    /// Register a mono sample buffer (decoded elsewhere), returning its id.
    pub fn add_sample(&self, mono: Vec<f32>, src_rate: u32) -> usize {
        if let Ok(mut s) = self.state.lock() {
            s.samples.push((std::sync::Arc::new(mono), src_rate.max(1)));
            s.samples.len() - 1
        } else {
            0
        }
    }

    /// Play a loaded sample at a world position (looping or one-shot). Returns a voice id.
    #[allow(clippy::too_many_arguments)]
    pub fn play_sample(
        &self,
        id: usize,
        x: f32,
        y: f32,
        z: f32,
        w: f32,
        vol: f32,
        looping: bool,
    ) -> u32 {
        if let Ok(mut s) = self.state.lock() {
            if id >= s.samples.len() {
                return 0;
            }
            let vid = s.next_voice_id;
            s.next_voice_id += 1;
            if s.sample_voices.len() >= 64 {
                s.sample_voices.remove(0);
            }
            s.sample_voices.push(SampleVoice {
                id: vid,
                sample: id,
                pos: 0.0,
                x,
                y,
                z,
                w,
                vol,
                looping,
                active: true,
            });
            vid
        } else {
            0
        }
    }

    /// Stop a sample voice by id.
    pub fn stop_sample(&self, voice: u32) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(v) = s.sample_voices.iter_mut().find(|v| v.id == voice) {
                v.active = false;
            }
        }
    }

    // ── master FX ─────────────────────────────────────────────────────────────
    pub fn fx_delay(&self, time_s: f32, feedback: f32, mix: f32) {
        if let Ok(mut s) = self.state.lock() {
            let cap = s.delay.bl.len();
            s.delay.len =
                ((time_s.max(0.0) * s.sample_rate as f32) as usize).min(cap.saturating_sub(1));
            s.delay.fb = feedback.clamp(0.0, 0.95);
            s.delay.mix = mix.clamp(0.0, 1.0);
        }
    }

    pub fn fx_reverb(&self, mix: f32) {
        if let Ok(mut s) = self.state.lock() {
            s.reverb.mix = mix.clamp(0.0, 1.0);
        }
    }

    /// Master low-pass cutoff ∈ (0,1]; 1.0 ≈ open, lower = muffled/underwater.
    pub fn fx_lowpass(&self, cutoff01: f32) {
        if let Ok(mut s) = self.state.lock() {
            s.lowpass.target = cutoff01.clamp(0.0, 1.0);
        }
    }

    /// Silence and remove the tone at `idx`.
    pub fn clear_tone(&self, idx: usize) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(slot) = s.tones.get_mut(idx) {
                *slot = None;
            }
        }
    }

    // ── Listener (camera) ────────────────────────────────────────────────────

    /// Update the listener orientation to match the Ling `set_camera` values.
    pub fn set_listener(&self, cry: f32, sry: f32, crx: f32, srx: f32) {
        if let Ok(mut s) = self.state.lock() {
            s.cry = cry;
            s.sry = sry;
            s.crx = crx;
            s.srx = srx;
        }
    }

    /// Update the listener (camera) world position so positional sounds pan and
    /// attenuate relative to where the camera actually is.
    pub fn set_listener_pos(&self, x: f32, y: f32, z: f32) {
        if let Ok(mut s) = self.state.lock() {
            s.lx = x;
            s.ly = y;
            s.lz = z;
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
            },
            Err(e) => eprintln!("audio: bgm load failed ({path}): {e}"),
        }
    }

    /// Adjust BGM playback volume without reloading.
    pub fn set_bgm_volume(&self, vol: f32) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(bgm) = &mut s.bgm {
                bgm.volume = vol;
            }
        }
    }

    // ── Master ───────────────────────────────────────────────────────────────

    pub fn set_master_volume(&self, vol: f32) {
        if let Ok(mut s) = self.state.lock() {
            s.master_volume = vol;
        }
    }
}

// ─── WAV loader ───────────────────────────────────────────────────────────────

fn load_wav(path: &str) -> Result<(Vec<f32>, u32), Box<dyn std::error::Error>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let src_rate = spec.sample_rate;

    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(|s| s.ok()).collect(),
        hound::SampleFormat::Int => {
            // Normalise to [-1, 1] regardless of bit depth.
            let max = (1i32 << spec.bits_per_sample.saturating_sub(1)) as f32;
            reader
                .samples::<i32>()
                .filter_map(|s| s.ok())
                .map(|s| s as f32 / max)
                .collect()
        },
    };

    // Normalise to interleaved stereo.
    let stereo: Vec<f32> = match channels {
        1 => raw.iter().flat_map(|&s| [s, s]).collect(),
        2 => raw,
        n => raw
            .chunks(n)
            .flat_map(|c| [c[0], if c.len() > 1 { c[1] } else { c[0] }])
            .collect(),
    };

    Ok((stereo, src_rate))
}

// ─── cpal stream builder ──────────────────────────────────────────────────────

fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    state: Arc<Mutex<AudioState>>,
    fmt: cpal::SampleFormat,
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
        },
        cpal::SampleFormat::I16 => {
            let st = Arc::clone(&state);
            device.build_output_stream(
                config,
                move |data: &mut [i16], _| fill_i16(data, channels, &st),
                err_fn,
                None,
            )?
        },
        _ => {
            // Generic fallback: output as i16.
            let st = Arc::clone(&state);
            device.build_output_stream::<i16, _, _>(
                config,
                move |data: &mut [i16], _| fill_i16(data, channels, &st),
                err_fn,
                None,
            )?
        },
    })
}

/// Fill a `&mut [f32]` buffer (interleaved, `channels` wide).
fn fill_f32(data: &mut [f32], channels: usize, state: &Arc<Mutex<AudioState>>) {
    let ch = channels.max(1);
    // `lock()`, NOT `try_lock()`: the game thread makes many short audio calls
    // per frame (4 hum tones + low-pass + SFX + monster callouts…), each briefly
    // holding this mutex. With `try_lock` the callback dropped to SILENCE on any
    // contention → choppy/stuttering continuous tones (the "ball whirr" hum).
    // The lock holders are all microsecond-short, so blocking here is negligible
    // and gives glitch-free audio. (A lock-free ring buffer would be the textbook
    // real-time fix; this is the pragmatic one for a game mixer.)
    let mut s = match state.lock() {
        Ok(s) => s,
        Err(p) => p.into_inner(), // poisoned (a panic mid-mix) → keep playing
    };
    for frame in data.chunks_mut(ch) {
        let (l, r) = s.next_sample();
        frame[0] = l;
        if ch > 1 {
            frame[1] = r;
        }
        for extra in frame.iter_mut().skip(2) {
            *extra = 0.0;
        }
    }
}

/// Fill a `&mut [i16]` buffer (interleaved, `channels` wide).
fn fill_i16(data: &mut [i16], channels: usize, state: &Arc<Mutex<AudioState>>) {
    let ch = channels.max(1);
    // `lock()` not `try_lock()` — see fill_f32: try_lock dropped to silence on
    // contention with the game thread's per-frame audio calls → choppy tones.
    let mut s = match state.lock() {
        Ok(s) => s,
        Err(p) => p.into_inner(),
    };
    for frame in data.chunks_mut(ch) {
        let (l, r) = s.next_sample();
        frame[0] = (l * 32_767.0) as i16;
        if ch > 1 {
            frame[1] = (r * 32_767.0) as i16;
        }
        for extra in frame.iter_mut().skip(2) {
            *extra = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sfx_voice_envelopes_and_ends() {
        let mut v = SfxVoice {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
            freq: 440.0,
            amp: 0.5,
            wave: Wave::Sine,
            dur: 0.02,
            age: 0.0,
            phase: 0.0,
            w_phase: 0.0,
            seed: 1,
        };
        let dt = 1.0 / 44100.0;
        let mut peak = 0.0f32;
        let mut steps = 0;
        while !v.done() && steps < 44100 {
            peak = peak.max(v.next(dt).abs());
            steps += 1;
        }
        assert!(peak > 0.01, "sfx should produce sound");
        assert!(v.done(), "sfx should finish after its duration");
    }

    #[test]
    fn sample_voice_loops_and_oneshot_stops() {
        let mut st = AudioState::new(44100);
        st.samples
            .push((std::sync::Arc::new(vec![0.5f32; 100]), 44100));
        // looping voice survives past the buffer end
        st.sample_voices.push(SampleVoice {
            id: 1,
            sample: 0,
            pos: 0.0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
            vol: 1.0,
            looping: true,
            active: true,
        });
        for _ in 0..500 {
            let _ = st.next_sample();
        }
        assert_eq!(
            st.sample_voices.len(),
            1,
            "looping voice should still be alive"
        );
        // one-shot voice deactivates after the buffer
        st.sample_voices.push(SampleVoice {
            id: 2,
            sample: 0,
            pos: 0.0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
            vol: 1.0,
            looping: false,
            active: true,
        });
        for _ in 0..500 {
            let _ = st.next_sample();
        }
        assert!(
            st.sample_voices.iter().all(|v| v.id != 2),
            "one-shot should have stopped"
        );
    }

    #[test]
    fn master_fx_stay_finite() {
        let mut st = AudioState::new(44100);
        st.delay.len = 2000;
        st.delay.fb = 0.6;
        st.delay.mix = 0.4;
        st.reverb.mix = 0.5;
        st.lowpass.target = 0.2;
        st.lowpass.cutoff = 0.2;
        // feed a tone and ensure the FX chain never blows up
        st.tones[0] = Some(Tone::new(ToneParams {
            freq: 220.0,
            amp: 0.8,
            ..Default::default()
        }));
        for _ in 0..44100 {
            let (l, r) = st.next_sample();
            assert!(l.is_finite() && r.is_finite() && l.abs() <= 1.0 && r.abs() <= 1.0);
        }
    }

    // ── Wave channel synthesis ────────────────────────────────────────────────
    #[test]
    fn wave_from_name_maps_and_samples_stay_bounded() {
        assert!(matches!(Wave::from_name("square"), Wave::Square));
        assert!(matches!(Wave::from_name("SAW"), Wave::Saw)); // case-insensitive
        assert!(matches!(Wave::from_name("triangle"), Wave::Triangle));
        assert!(matches!(Wave::from_name("noise"), Wave::Noise));
        assert!(matches!(Wave::from_name("whatever"), Wave::Sine)); // default
        for wave in [
            Wave::Sine,
            Wave::Square,
            Wave::Saw,
            Wave::Triangle,
            Wave::Noise,
        ] {
            for i in 0..256 {
                let v = wave.sample(i as f32 / 256.0);
                assert!(
                    v.is_finite() && v.abs() <= 1.0,
                    "{wave:?} out of range: {v}"
                );
            }
        }
    }

    // ── Mixer combines independent channels (tone + sfx + sample) ─────────────
    #[test]
    fn channels_mix_together() {
        let mut st = AudioState::new(44100);
        st.tones[0] = Some(Tone::new(ToneParams {
            freq: 330.0,
            amp: 0.4,
            ..Default::default()
        }));
        st.sfx.push(SfxVoice {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
            freq: 660.0,
            amp: 0.4,
            wave: Wave::Square,
            dur: 0.5,
            age: 0.0,
            phase: 0.0,
            w_phase: 0.0,
            seed: 7,
        });
        st.samples
            .push((std::sync::Arc::new(vec![0.3f32; 256]), 44100));
        st.sample_voices.push(SampleVoice {
            id: 1,
            sample: 0,
            pos: 0.0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
            vol: 0.6,
            looping: true,
            active: true,
        });
        let mut energy = 0.0f64;
        for _ in 0..2000 {
            let (l, r) = st.next_sample();
            assert!(l.is_finite() && r.is_finite());
            energy += (l * l + r * r) as f64;
        }
        assert!(
            energy > 1.0,
            "all three channels should contribute audible energy"
        );
    }

    // ── Many simultaneous loud channels must never exceed digital full-scale ──
    #[test]
    fn many_loud_channels_stay_within_headroom() {
        let mut st = AudioState::new(44100);
        st.master_volume = 1.0;
        for i in 0..st.tones.len() {
            st.tones[i] = Some(Tone::new(ToneParams {
                freq: 110.0 + i as f32 * 25.0,
                amp: 1.0,
                ..Default::default()
            }));
        }
        for i in 0..60 {
            st.sfx.push(SfxVoice {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
                freq: 200.0 + i as f32 * 30.0,
                amp: 1.0,
                wave: Wave::Saw,
                dur: 1.0,
                age: 0.0,
                phase: 0.0,
                w_phase: 0.0,
                seed: i + 1,
            });
        }
        for _ in 0..44100 {
            let (l, r) = st.next_sample();
            assert!(
                l.is_finite() && r.is_finite() && l.abs() <= 1.0 && r.abs() <= 1.0,
                "soft-clip must keep the mix bus within [-1, 1]: ({l}, {r})"
            );
        }
    }

    // ── Positional channels pan correctly (identity listener: world -x = left) ─
    #[test]
    fn positional_channel_pans_left_then_right() {
        let energy = |x: f32| -> (f64, f64) {
            let mut st = AudioState::new(44100);
            // Dry signal so panning isn't smeared by the master FX bus.
            st.delay.mix = 0.0;
            st.reverb.mix = 0.0;
            st.lowpass.target = 1.0;
            st.lowpass.cutoff = 1.0;
            st.tones[0] = Some(Tone::new(ToneParams {
                x,
                freq: 440.0,
                amp: 0.6,
                ..Default::default()
            }));
            let (mut el, mut er) = (0.0f64, 0.0f64);
            for _ in 0..4000 {
                let (l, r) = st.next_sample();
                el += (l * l) as f64;
                er += (r * r) as f64;
            }
            (el, er)
        };
        let (ll, lr) = energy(-8.0); // hard left
        assert!(
            ll > lr * 1.5,
            "left-positioned source should favour L ({ll} vs {lr})"
        );
        let (rl, rr) = energy(8.0); // hard right
        assert!(
            rr > rl * 1.5,
            "right-positioned source should favour R ({rl} vs {rr})"
        );
    }

    // ── THREADS: the cpal callback thread and the game thread share AudioState
    // through Arc<Mutex<>>. A producer (game: set_tone/sfx) and a consumer
    // (audio callback: next_sample) hammering it concurrently must not panic,
    // deadlock, or emit garbage. ───────────────────────────────────────────────
    #[test]
    fn audio_thread_shared_state_stays_safe_under_contention() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let state = Arc::new(Mutex::new(AudioState::new(44100)));
        let stop = Arc::new(AtomicBool::new(false));

        // Producer: mimics the game thread re-setting tones + firing sfx each frame.
        let producer = {
            let state = Arc::clone(&state);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut n: u32 = 0;
                while !stop.load(Ordering::Relaxed) {
                    if let Ok(mut s) = state.lock() {
                        let slot = (n as usize) % s.tones.len();
                        s.tones[slot] = Some(Tone::new(ToneParams {
                            freq: 200.0 + (n % 800) as f32,
                            amp: 0.5,
                            ..Default::default()
                        }));
                        if s.sfx.len() >= 64 {
                            s.sfx.remove(0);
                        } // same cap the engine enforces
                        s.sfx.push(SfxVoice {
                            x: ((n % 17) as f32) - 8.0,
                            y: 0.0,
                            z: 0.0,
                            w: 1.0,
                            freq: 440.0,
                            amp: 0.4,
                            wave: Wave::Triangle,
                            dur: 0.05,
                            age: 0.0,
                            phase: 0.0,
                            w_phase: 0.0,
                            seed: n.max(1),
                        });
                    }
                    n = n.wrapping_add(1);
                    std::thread::yield_now();
                }
            })
        };

        // Consumer: mimics the cpal callback pulling sample blocks under the lock.
        let consumer = {
            let state = Arc::clone(&state);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut produced = 0u64;
                let mut bad = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    if let Ok(mut s) = state.lock() {
                        for _ in 0..256 {
                            let (l, r) = s.next_sample();
                            if !(l.is_finite() && r.is_finite() && l.abs() <= 1.0 && r.abs() <= 1.0)
                            {
                                bad += 1;
                            }
                            produced += 1;
                        }
                    }
                    std::thread::yield_now();
                }
                (produced, bad)
            })
        };

        std::thread::sleep(std::time::Duration::from_millis(200));
        stop.store(true, Ordering::Relaxed);

        producer.join().expect("producer thread must not panic");
        let (produced, bad) = consumer.join().expect("consumer thread must not panic");
        assert!(produced > 0, "consumer should have mixed some samples");
        assert_eq!(
            bad, 0,
            "every concurrently-mixed sample must be finite and in range"
        );
    }

    // ── YIN-YANG morph voice: every material makes sound, stays bounded ───────
    #[test]
    fn morph_voice_each_material_is_audible_and_bounded() {
        for material in 0u8..4 {
            let mut v =
                MorphVoice::new(44100.0, 220.0, 0.7, 0.4, material, 0.5, 0.0, 0.0, 0.0, 1.0);
            let dt = 1.0 / 44100.0;
            let mut peak = 0.0f32;
            let mut steps = 0;
            while !v.done() && steps < 44100 {
                let s = v.next(dt);
                assert!(
                    s.is_finite() && s.abs() <= 1.0,
                    "material {material} out of range: {s}"
                );
                peak = peak.max(s.abs());
                steps += 1;
            }
            assert!(
                peak > 0.01,
                "material {material} should produce sound (peak {peak})"
            );
        }
    }

    // ── The morph dial actually changes the timbre (light ≠ dark) ─────────────
    #[test]
    fn morph_light_and_dark_differ() {
        let render = |morph: f32| -> f64 {
            let mut v = MorphVoice::new(44100.0, 330.0, 0.7, 0.3, 1, morph, 0.0, 0.0, 0.0, 1.0);
            let dt = 1.0 / 44100.0;
            let mut acc = 0.0f64;
            for _ in 0..6000 {
                let s = v.next(dt);
                acc += (s.abs()) as f64; // crude spectral-energy proxy
            }
            acc
        };
        let light = render(0.0);
        let dark = render(1.0);
        // Dark adds FM sidebands + crush + tanh drive → a materially different signal.
        assert!(
            (light - dark).abs() / (light + dark + 1e-6) > 0.05,
            "light {light} vs dark {dark} should differ"
        );
    }
}
