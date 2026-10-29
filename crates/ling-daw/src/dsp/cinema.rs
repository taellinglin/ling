//! Ling Cinema: sound effects for film, trailers, games and the transitions of electronic music, every one generated (no
//! recordings). Each note plays one effect from one of eleven engines, pitched by the key where a pitch makes sense:
//! * **riser** - a noise sweep (a band-pass rising from the lows to the air) under a pitched rise (detuned saws climbing `amount`
//!   semitones, or an endless Shepard tone), with a flutter that speeds up; it lasts `length` beats at the song's tempo, or as long
//!   as the note is held;
//! * **downlifter** - the same falling: bright and high to dark and low, fading out;
//! * **impact** - a trailer hit: a sub boom with a falling knock, a noise crack, a metallic (inharmonic) layer, a tail of dark noise
//!   into the hall, and optionally a reverse swell before the hit (`pre`: the hit lands that much after the note starts);
//! * **whoosh** - band-passed noise swelling past with a doppler pan across the stereo field and a pitch that drops as it passes;
//! * **sub drop** - a sine falling from the note (an 808-style sub drop) with a click and grit;
//! * **braam** - the cinematic brass: a stack of detuned saws (root, octaves, fifth) driven into a low-pass that swells open;
//! * **reverse** - a reverse cymbal / swell: metallic square partials and noise rising to a sudden stop;
//! * **laser** - a zap: a fast exponential pitch fall, optionally repeating (`motion`);
//! * **glitch** - a data burst: short random steps of beeps, buzzes and noise, crushed;
//! * **tape stop** - a chord that winds down as a tape (or a power-down) stops;
//! * **ambience** - an evolving soundscape held as long as the note: detuned oscillator layers drifting on slow random modulation,
//!   wind (noise through a wandering band-pass, in gusts) and a granular shimmer of octave and fifth grains.
//! On top, every engine shares the macros (length, rise / fall amount, curve, brightness, noise / tone, grit, width) and a hall
//! (an eight-line FDN, `space`). The presets are named sounds in eight categories.
use super::bassfx::{g_of, Tsvf};
use super::cinefx::Fdn;
use super::{midi_hz, Instrument, Rng, Smooth};
use std::f32::consts::{PI, TAU};

pub const ENGINES: [&str; 11] = ["riser", "downlifter", "impact", "whoosh", "sub drop", "braam", "reverse", "laser", "glitch", "tape stop", "ambience"];
const E_RISER: usize = 0;
const E_DOWN: usize = 1;
const E_IMPACT: usize = 2;
const E_WHOOSH: usize = 3;
const E_SUBDROP: usize = 4;
const E_BRAAM: usize = 5;
const E_REVERSE: usize = 6;
const E_LASER: usize = 7;
const E_GLITCH: usize = 8;
const E_TAPE: usize = 9;
const E_AMB: usize = 10;

/// (name, default): every parameter 0..1
pub const PARAMS: [(&str, f32); 26] = [
    ("engine", 0.0),
    ("length", 5.0 / 7.0),
    ("sync", 1.0),
    ("amount", 0.5),
    ("curve", 0.5),
    ("bright", 0.6),
    ("noise", 0.5),
    ("grit", 0.1),
    ("space", 0.35),
    ("width", 0.6),
    ("shepard", 0.0),
    ("motion", 0.4),
    ("sub", 0.6),
    ("crack", 0.5),
    ("metal", 0.3),
    ("pre", 0.0),
    ("tail", 0.5),
    ("detune", 0.4),
    ("doppler", 0.6),
    ("density", 0.5),
    ("shimmer", 0.3),
    ("wind", 0.4),
    ("attack", 0.0),
    ("release", 0.15),
    ("seed", 0.0),
    ("level", 0.7),
];
pub const NPARAM: usize = PARAMS.len();
const P_ENGINE: usize = 0;
const P_LENGTH: usize = 1;
const P_SYNC: usize = 2;
const P_AMOUNT: usize = 3;
const P_CURVE: usize = 4;
const P_BRIGHT: usize = 5;
const P_NOISE: usize = 6;
const P_GRIT: usize = 7;
const P_SPACE: usize = 8;
const P_WIDTH: usize = 9;
const P_SHEPARD: usize = 10;
const P_MOTION: usize = 11;
const P_SUB: usize = 12;
const P_CRACK: usize = 13;
const P_METAL: usize = 14;
const P_PRE: usize = 15;
const P_TAIL: usize = 16;
const P_DETUNE: usize = 17;
const P_DOPPLER: usize = 18;
const P_DENSITY: usize = 19;
const P_SHIMMER: usize = 20;
const P_WIND: usize = 21;
const P_ATTACK: usize = 22;
const P_RELEASE: usize = 23;
const P_SEED: usize = 24;
const P_LEVEL: usize = 25;

fn idx(name: &str) -> Option<usize> {
    PARAMS.iter().position(|p| p.0 == name)
}

/// the length knob in beats: a quarter beat to 32 beats, doubling every seventh
pub fn length_beats(v: f32) -> f32 {
    0.25 * 2f32.powf(v.clamp(0.0, 1.0) * 7.0)
}

#[inline]
fn blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

/// a band-limited saw (polyBLEP); `ph` 0..1 moves by `dt` = f / sr
#[inline]
fn saw(ph: &mut f32, dt: f32) -> f32 {
    let dt = dt.clamp(1e-6, 0.45);
    *ph += dt;
    if *ph >= 1.0 {
        *ph -= 1.0;
    }
    2.0 * *ph - 1.0 - blep(*ph, dt)
}

#[inline]
fn sine(ph: &mut f32, dt: f32) -> f32 {
    *ph = (*ph + dt.min(0.49)).fract();
    (*ph * TAU).sin()
}

/// a slow random walk: a new target every so often, glided to
#[derive(Clone, Copy, Default)]
struct Drift {
    cur: f32,
    tgt: f32,
    left: u32,
}
impl Drift {
    #[inline]
    fn run(&mut self, rng: &mut Rng, every: f32, k: f32) -> f32 {
        if self.left == 0 {
            self.tgt = rng.next();
            self.left = (every * (0.5 + 0.5 * (rng.next() + 1.0))).max(1.0) as u32;
        }
        self.left -= 1;
        self.cur += (self.tgt - self.cur) * k;
        self.cur
    }
}

#[derive(Clone, Copy, Default)]
struct Grain {
    on: bool,
    ph: f32,
    dt: f32,
    age: u32,
    len: u32,
    pan: f32,
}

#[derive(Clone)]
struct Voice {
    on: bool,
    key: u8,
    vel: f32,
    engine: usize,
    /// samples since the note began, the effect's length in samples
    t: u32,
    len: f32,
    held: bool,
    /// release gain (1 until the note ends) and its per-sample factor
    rel: f32,
    rel_k: f32,
    /// the attack ramp 0..1
    att: f32,
    pitch: Smooth,
    press: Smooth,
    ph: [f32; 16],
    f: [Tsvf; 6],
    lp: [f32; 4],
    rng: Rng,
    drift: [Drift; 6],
    grains: [Grain; 8],
    grain_next: f32,
    // glitch step state
    step_left: u32,
    step_dt: f32,
    step_wave: u8,
    step_amp: f32,
    step_bits: f32,
    hold: [f32; 2],
    hold_n: u32,
    // a quiet counter: the voice frees itself after this many silent samples
    quiet: u32,
    /// a DC blocker on the voice's output
    dc: [f32; 2],
}

impl Voice {
    fn new() -> Voice {
        Voice {
            on: false,
            key: 60,
            vel: 0.0,
            engine: 0,
            t: 0,
            len: 1.0,
            held: false,
            rel: 1.0,
            rel_k: 1.0,
            att: 0.0,
            pitch: Smooth::at(0.0),
            press: Smooth::at(0.0),
            ph: [0.0; 16],
            f: [Tsvf::default(); 6],
            lp: [0.0; 4],
            rng: Rng::new(1),
            drift: [Drift::default(); 6],
            grains: [Grain::default(); 8],
            grain_next: 0.0,
            step_left: 0,
            step_dt: 0.0,
            step_wave: 0,
            step_amp: 0.0,
            step_bits: 16.0,
            hold: [0.0; 2],
            hold_n: 0,
            quiet: 0,
            dc: [0.0; 2],
        }
    }
}

pub struct Cinema {
    sr: f32,
    v: [f32; NPARAM],
    voices: Vec<Voice>,
    bpm: f32,
    hall: Fdn,
    hall_set: (f32, f32),
    preset: String,
    notes: u32,
}

impl Cinema {
    pub fn new(sr: f32) -> Cinema {
        let mut c = Cinema { sr, v: [0.0; NPARAM], voices: (0..8).map(|_| Voice::new()).collect(), bpm: 120.0, hall: Fdn::new(sr), hall_set: (-1.0, -1.0), preset: String::new(), notes: 0 };
        c.v = defaults();
        c
    }
    pub fn engine(&self) -> usize {
        ((self.v[P_ENGINE] * (ENGINES.len() - 1) as f32).round() as usize).min(ENGINES.len() - 1)
    }
    /// the length in samples the knob asks for at the tempo
    fn knob_len(&self) -> f32 {
        length_beats(self.v[P_LENGTH]) * 60.0 / self.bpm.max(20.0) * self.sr
    }
    /// does the effect stop when the note is let go?
    fn gated(engine: usize) -> bool {
        matches!(engine, E_RISER | E_DOWN | E_BRAAM | E_AMB)
    }
    fn start(&mut self, key: u8, vel: f32, len_samples: f32) {
        let engine = self.engine();
        let held_len = self.v[P_SYNC] > 0.5 && len_samples > 16.0;
        let len = if held_len { len_samples } else { self.knob_len() };
        let len = match engine {
            // the impact's length is its tail; a laser and a glitch burst default short
            E_IMPACT => (0.3 + self.v[P_TAIL].powi(2) * 6.0) * self.sr,
            _ => len,
        }
        .max(self.sr * 0.01);
        // a free voice, else the oldest
        let i = self.voices.iter().position(|v| !v.on).unwrap_or_else(|| (0..self.voices.len()).max_by_key(|&i| self.voices[i].t).unwrap_or(0));
        self.notes = self.notes.wrapping_add(1);
        let seed = ((self.v[P_SEED] * 1000.0) as u32).wrapping_mul(7919).wrapping_add(key as u32 * 131 + 17);
        // a glitch burst is the same every time for a key and seed (renders sound like playback); the rest vary a little per note
        let seed = if engine == E_GLITCH { seed } else { seed.wrapping_add(self.notes.wrapping_mul(2654435761)) };
        let rel_s = 0.005 + self.v[P_RELEASE].powi(2) * 6.0;
        let v = &mut self.voices[i];
        *v = Voice::new();
        v.on = true;
        v.key = key;
        v.vel = vel.clamp(0.0, 1.0);
        v.engine = engine;
        v.len = len;
        v.held = true;
        v.rel_k = (-1.0 / (rel_s * self.sr)).exp();
        v.rng = Rng::new(seed | 1);
        for k in 0..16 {
            v.ph[k] = (v.rng.next() + 1.0) * 0.5;
        }
    }

    /// one sample of one voice: (left, right) before the level, the space and the width
    fn voice_sample(&self, v: &mut Voice) -> (f32, f32) {
        let sr = self.sr;
        let t = v.t as f32;
        let p = (t / v.len).min(1.0);
        let semis = v.pitch.tick();
        let press = v.press.tick();
        let f0 = midi_hz(v.key as f32 + semis);
        let amt = self.v[P_AMOUNT] * 48.0;
        let curve = self.v[P_CURVE];
        let bright = self.v[P_BRIGHT];
        let noise_mix = self.v[P_NOISE];
        let detune = self.v[P_DETUNE];
        let motion = self.v[P_MOTION];
        let grit = self.v[P_GRIT];
        let mut l: f32;
        let mut r: f32;
        match v.engine {
            E_RISER | E_DOWN => {
                let rise = v.engine == E_RISER;
                let e = 0.6 + curve * 2.4;
                // pitch progress: a riser speeds up at the end, a downlifter falls fastest at the start
                let pr = if rise { p.powf(e) } else { 1.0 - (1.0 - p).powf(e) };
                let st = if rise { amt * pr } else { -amt * pr };
                let tone = if self.v[P_SHEPARD] > 0.5 {
                    // an endless Shepard glissando: eight octaves of sines moving together under a bell over log-frequency
                    let pos = st / 12.0;
                    let lo = f0 / 16.0;
                    let mut s = 0.0;
                    for k in 0..8 {
                        let oct = (k as f32 + pos).rem_euclid(8.0);
                        let w = (-(oct - 4.0) * (oct - 4.0) / 3.2).exp();
                        s += sine(&mut v.ph[k], lo * 2f32.powf(oct) / sr) * w;
                    }
                    (s * 0.45, s * 0.45)
                } else {
                    let f = f0 * 2f32.powf(st / 12.0);
                    let d = 1.0 + detune * 0.012;
                    let a = saw(&mut v.ph[0], f / sr) + saw(&mut v.ph[1], f * d / sr) * 0.7;
                    let b = saw(&mut v.ph[2], f / sr / d) + saw(&mut v.ph[3], f * 2.0 / sr) * 0.35;
                    // a low-pass that follows the pitch
                    let g = g_of((f * (2.0 + bright * 14.0)).min(sr * 0.45), sr);
                    (v.f[0].run(a * 0.32, g, 1.0).0, v.f[1].run(b * 0.32, g, 1.0).0)
                };
                // the noise sweep: a band-pass from the lows to the air
                let sweep = if rise { pr } else { 1.0 - pr };
                let fc = 120.0 * 2f32.powf(sweep * (4.0 + bright * 6.5));
                let g = g_of(fc.min(sr * 0.45), sr);
                let (nl, nr) = (v.rng.next(), v.rng.next());
                let bl = v.f[2].run(nl, g, 0.7).1 + v.f[4].run(nl, g, 1.6).2 * 0.08 * sweep;
                let br = v.f[3].run(nr, g, 0.7).1 + v.f[5].run(nr, g, 1.6).2 * 0.08 * sweep;
                // the flutter speeds up through a rise (slows through a fall)
                let rate = motion * (1.5 + 18.0 * sweep * sweep);
                v.lp[0] = (v.lp[0] + rate / sr).fract();
                let am = 1.0 - motion * 0.45 * (0.5 - 0.5 * (v.lp[0] * TAU).cos());
                let amp = if rise { 0.06 + 0.94 * p.powf(1.0 + curve * 2.0) } else { (1.0 - p).powf(0.8 + curve * 2.0) * (t / (0.01 * sr)).min(1.0) };
                l = (tone.0 * (1.0 - noise_mix) + bl * noise_mix * 1.1) * amp * am;
                r = (tone.1 * (1.0 - noise_mix) + br * noise_mix * 1.1) * amp * am;
                if !rise && p >= 1.0 {
                    v.held = false;
                    v.rel = 0.0;
                }
            }
            E_IMPACT => {
                let pre = (self.v[P_PRE] * 60.0 / self.bpm.max(20.0) * sr).max(0.0);
                let tail = 0.3 + self.v[P_TAIL].powi(2) * 6.0;
                if t < pre {
                    // the reverse swell into the hit: noise and metal rising, ever brighter
                    let q = t / pre;
                    let g = g_of(300.0 * 2f32.powf(q * (3.0 + bright * 4.0)), sr);
                    let (nl, nr) = (v.rng.next(), v.rng.next());
                    let a = q.powi(3) * 0.9;
                    let m = (sine(&mut v.ph[10], f0 * 3.01 / sr) + sine(&mut v.ph[11], f0 * 4.37 / sr)) * 0.3 * self.v[P_METAL];
                    l = (v.f[2].run(nl, g, 0.9).1 + m) * a;
                    r = (v.f[3].run(nr, g, 0.9).1 + m) * a;
                } else {
                    let tau = (t - pre) / sr;
                    // the boom: the note folded below 90 Hz, falling from a knock
                    let mut fb = f0;
                    while fb > 90.0 {
                        fb *= 0.5;
                    }
                    let knock = 2f32.powf(amt * 0.5 * (-tau / 0.045).exp() / 12.0);
                    let boom = sine(&mut v.ph[0], fb * knock / sr) * (-tau / (tail * 0.3)).exp() * self.v[P_SUB] * 1.1;
                    let boom = (boom * (1.0 + grit * 4.0)).tanh();
                    // the crack: bright noise, a few tens of ms
                    let (nl, nr) = (v.rng.next(), v.rng.next());
                    let gh = g_of(1200.0 + bright * 5000.0, sr);
                    let ce = (-tau / 0.035).exp() * self.v[P_CRACK] * 1.3;
                    let cl = v.f[0].run(nl, gh, 0.8).2 * ce;
                    let cr = v.f[1].run(nr, gh, 0.8).2 * ce;
                    // the metal: inharmonic partials over the note, ring-modulated
                    let mut fm = f0 * 2.0;
                    while fm < 180.0 {
                        fm *= 2.0;
                    }
                    while fm > 1800.0 {
                        fm *= 0.5;
                    }
                    const RAT: [f32; 6] = [1.0, 1.47, 2.09, 2.56, 3.37, 4.18];
                    let mut m = 0.0;
                    for (k, rt) in RAT.iter().enumerate() {
                        m += sine(&mut v.ph[2 + k], fm * rt / sr) * (1.0 - k as f32 * 0.12);
                    }
                    let m = m * sine(&mut v.ph[9], fm * 0.71 / sr) * (-tau / (0.12 + tail * 0.25)).exp() * self.v[P_METAL] * 0.35;
                    // the tail: dark noise dying away under the hall
                    let gl = g_of((300.0 + bright * 2500.0) * (-tau / tail).exp().max(0.15), sr);
                    let te = (-tau / (tail * 0.45)).exp() * 0.45 * (tau / 0.01).min(1.0);
                    let tl = v.f[2].run(nl, gl, 1.2).0 * te;
                    let tr = v.f[3].run(nr, gl, 1.2).0 * te;
                    l = boom + cl + m + tl;
                    r = boom + cr + m * 0.9 + tr;
                    if tau > tail * 1.6 {
                        v.held = false;
                        v.rel = v.rel.min(0.999);
                    }
                }
            }
            E_WHOOSH => {
                // the passage: a bell of loudness and brightness, panned across, the pitch dropping as it goes by
                let pk = 0.35 + curve * 0.3;
                let q = if p < pk { p / pk } else { 1.0 - (p - pk) / (1.0 - pk) };
                let bell = (q.clamp(0.0, 1.0) * PI * 0.5).sin().powi(2);
                let fc = 250.0 * 2f32.powf(bell * (2.5 + bright * 4.5));
                let g = g_of(fc.min(sr * 0.45), sr);
                let n = v.rng.next();
                let n2 = v.rng.next();
                let air = v.f[0].run(n, g, 0.9).1 + v.f[1].run(n2, g_of((fc * 2.3).min(sr * 0.45), sr), 1.4).1 * 0.4;
                let dop = ((0.5 - p) * 6.0).tanh() * amt * 0.25;
                let ft = f0 * 2f32.powf(dop / 12.0);
                let tone = v.f[2].run(saw(&mut v.ph[0], ft / sr) + saw(&mut v.ph[1], ft * 1.007 / sr), g_of((ft * 4.0).min(sr * 0.45), sr), 0.8).0 * 0.25;
                let x = (air * noise_mix + tone * (1.0 - noise_mix)) * bell * 1.3;
                let pan = (2.0 * p - 1.0) * self.v[P_DOPPLER];
                let a = (pan + 1.0) * PI * 0.25;
                l = x * a.cos() * 1.414;
                r = x * a.sin() * 1.414;
                if p >= 1.0 {
                    v.held = false;
                    v.rel = 0.0;
                }
            }
            E_SUBDROP => {
                let mut fs = f0;
                while fs > 110.0 {
                    fs *= 0.5;
                }
                let pr = 1.0 - (1.0 - p).powf(1.0 + curve * 3.0);
                let f = fs * 2f32.powf(-amt * pr / 12.0);
                let s = sine(&mut v.ph[0], f / sr);
                let drive = 1.0 + grit * 6.0;
                let y = (s * drive).tanh() / drive.tanh();
                let click = if t < 0.004 * sr { v.rng.next() * (1.0 - t / (0.004 * sr)) * 0.4 * self.v[P_CRACK] } else { 0.0 };
                let amp = (1.0 - p).powf(0.6) * (t / (0.002 * sr)).min(1.0);
                l = (y * 0.95 + click) * amp;
                r = l;
                if p >= 1.0 {
                    v.held = false;
                    v.rel = 0.0;
                }
            }
            E_BRAAM => {
                // root, an octave and two below, the fifth and the octave: two detuned saws each, into a swelling low-pass
                const IV: [f32; 5] = [0.0, -12.0, 7.0, 12.0, -24.0];
                let d = detune * 0.018;
                let (mut a, mut b) = (0.0, 0.0);
                for (k, iv) in IV.iter().enumerate() {
                    let f = f0 * 2f32.powf(iv / 12.0);
                    let w = if k == 4 { 0.6 } else { 1.0 };
                    a += saw(&mut v.ph[k * 2], f * (1.0 + d) / sr) * w;
                    b += saw(&mut v.ph[k * 2 + 1], f * (1.0 - d) / sr) * w;
                }
                let n = v.rng.next() * noise_mix * 0.6;
                let drive = 1.0 + grit * 5.0;
                let (a, b) = (((a * 0.22 + n) * drive).tanh(), ((b * 0.22 + n) * drive).tanh());
                let swell = if p < 0.3 { (p / 0.3).powf(0.7) } else { 1.0 - 0.55 * ((p - 0.3) / 0.7) };
                let fc = 70.0 * 2f32.powf(swell * (3.0 + bright * 5.2) + press * 1.5);
                let g = g_of(fc.min(sr * 0.45), sr);
                let k = 0.55;
                let a1 = v.f[1].run(a, g, k).0;
                let b1 = v.f[3].run(b, g, k).0;
                let ya = v.f[0].run(a1, g, 1.2).0;
                let yb = v.f[2].run(b1, g, 1.2).0;
                let sub = sine(&mut v.ph[12], f0 * 0.5 / sr) * self.v[P_SUB] * 0.35;
                let amp = (t / (0.04 * sr)).min(1.0);
                l = (ya * 0.8 + sub) * amp;
                r = (yb * 0.8 + sub) * amp;
            }
            E_REVERSE => {
                // a reverse cymbal: six square partials (the 808 cymbal's) and noise, swelling to a cut
                const CYM: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];
                let sc = (f0 / 261.6).clamp(0.5, 3.0);
                let mut m = 0.0;
                for (k, fq) in CYM.iter().enumerate() {
                    v.ph[k] = (v.ph[k] + fq * sc * 4.0 / sr).fract();
                    m += if v.ph[k] < 0.5 { 1.0 } else { -1.0 };
                }
                let gh = g_of(3000.0 + bright * 5000.0, sr);
                let met = v.f[0].run(m * 0.15, gh, 0.9).2 * self.v[P_METAL] * 2.0;
                let (nl, nr) = (v.rng.next(), v.rng.next());
                let gl = g_of((1500.0 * 2f32.powf(p * (1.5 + bright * 2.5))).min(sr * 0.45), sr);
                let hl = v.f[2].run(nl, gh, 1.0).2;
                let hr = v.f[4].run(nr, gh, 1.0).2;
                let al = v.f[1].run(hl, gl, 0.8).0;
                let ar = v.f[3].run(hr, gl, 0.8).0;
                // the reversed pad under it (the tone side of the noise knob)
                let pad = (saw(&mut v.ph[8], f0 / sr) + saw(&mut v.ph[9], f0 * 1.005 / sr)) * 0.25;
                let pad = v.f[5].run(pad, g_of((f0 * (1.0 + p * 8.0)).min(sr * 0.45), sr), 1.0).0;
                let amp = p.powf(2.0 + curve * 4.0);
                let cut = if p >= 1.0 { 0.0 } else { 1.0 };
                l = ((al + met) * noise_mix + pad * (1.0 - noise_mix)) * amp * cut;
                r = ((ar + met) * noise_mix + pad * (1.0 - noise_mix)) * amp * cut;
                if p >= 1.0 {
                    v.held = false;
                    v.rel = 0.0;
                }
            }
            E_LASER => {
                // one zap, or a stream of them (motion = repeats per second)
                let per = if motion > 0.02 { sr * (0.035 + (1.0 - motion).powi(2) * 0.3) } else { v.len };
                let u = (t % per) / per;
                let z = amt.max(6.0) * (1.0 - u).powf(3.0 + curve * 8.0);
                let f = f0 * 2f32.powf(z / 12.0);
                let wob = sine(&mut v.ph[1], 37.0 / sr) * 0.004;
                let s = sine(&mut v.ph[0], f * (1.0 + wob) / sr);
                let sq = (s * (1.0 + grit * 12.0)).tanh();
                let g = g_of((800.0 + bright * 12000.0).min(sr * 0.45), sr);
                let y = v.f[0].run(sq, g, 1.0).0;
                let amp = (1.0 - u).powf(1.4) * (u * per / (0.002 * sr)).min(1.0);
                l = y * amp * 0.8;
                r = l;
                if p >= 1.0 {
                    v.held = false;
                    v.rel = 0.0;
                }
            }
            E_GLITCH => {
                if v.step_left == 0 {
                    // a new step: on or off, a waveform, a pitch, a bit depth
                    let step = sr * (0.004 + (1.0 - motion).powi(2) * 0.11);
                    v.step_left = (step * (0.5 + (v.rng.next() + 1.0) * 0.5)) as u32 + 1;
                    let on = (v.rng.next() + 1.0) * 0.5 < 0.25 + self.v[P_DENSITY] * 0.75;
                    v.step_amp = if on { 0.5 + (v.rng.next() + 1.0) * 0.25 } else { 0.0 };
                    v.step_wave = (((v.rng.next() + 1.0) * 2.0) as u8).min(3);
                    let semi = ((v.rng.next() + 1.0) * 15.0).round() - 6.0;
                    let hi = (v.rng.next() + 1.0) * 0.5 < 0.25;
                    v.step_dt = if hi { (1500.0 + (v.rng.next() + 1.0) * 2500.0) / sr } else { f0 * 2f32.powf(semi / 12.0) / sr };
                    v.step_bits = 3.0 + (1.0 - grit) * 10.0 + (v.rng.next() + 1.0) * 2.0;
                    v.hold_n = 1 + ((v.rng.next() + 1.0) * grit * 8.0) as u32;
                }
                v.step_left -= 1;
                let w = match v.step_wave {
                    0 => {
                        let s = saw(&mut v.ph[0], v.step_dt);
                        if s > 0.0 { 0.8 } else { -0.8 }
                    }
                    1 => saw(&mut v.ph[0], v.step_dt) * 0.8,
                    2 => v.rng.next() * 0.7,
                    _ => sine(&mut v.ph[0], v.step_dt * (1.0 + (v.t % 512) as f32 / 512.0)),
                };
                // crushed: held for a few samples, then to a few bits
                if v.t % v.hold_n.max(1) == 0 {
                    let q = 2f32.powf(v.step_bits - 1.0);
                    v.hold[0] = (w * q).round() / q;
                }
                let g = g_of((1500.0 + bright * 14000.0).min(sr * 0.45), sr);
                let y = v.f[0].run(v.hold[0], g, 1.2).0 * v.step_amp;
                v.lp[1] += (y - v.lp[1]) * 0.3;
                let pan = ((v.step_dt * 977.0).fract() - 0.5) * 1.2;
                l = v.lp[1] * (1.0 - pan) * 0.7;
                r = v.lp[1] * (1.0 + pan) * 0.7;
                if p >= 1.0 {
                    v.held = false;
                    v.rel = 0.0;
                }
            }
            E_TAPE => {
                // the speed falls from 1 to 0: pitch, tone and level with it
                let s = (1.0 - p).powf(0.4 + curve * 2.6);
                let d = 1.0 + detune * 0.01;
                let f = f0 * s;
                let a = saw(&mut v.ph[0], f / sr) + saw(&mut v.ph[1], f * 1.4983 * d / sr) * 0.6 + saw(&mut v.ph[2], f * 2.0 / d / sr) * 0.4;
                let b = saw(&mut v.ph[3], f * d / sr) + saw(&mut v.ph[4], f * 1.4983 / sr) * 0.6 + saw(&mut v.ph[5], f * 2.0 * d / sr) * 0.4;
                let n = v.rng.next() * noise_mix * 0.5;
                let g = g_of(((400.0 + bright * 9000.0) * s).max(40.0).min(sr * 0.45), sr);
                l = v.f[0].run(a * 0.3 + n, g, 0.9).0 * s.powf(0.3);
                r = v.f[1].run(b * 0.3 + n, g, 0.9).0 * s.powf(0.3);
                if p >= 1.0 {
                    v.held = false;
                    v.rel = 0.0;
                }
            }
            _ => {
                // the soundscape: drifting layers, wind and shimmer grains
                let every = sr * (0.4 + (1.0 - motion) * 3.5);
                let k = 1.0 / (sr * (0.2 + (1.0 - motion) * 1.5));
                let mut dr = [0.0f32; 6];
                for i in 0..6 {
                    dr[i] = v.drift[i].run(&mut v.rng, every, k);
                }
                const LAY: [f32; 6] = [0.0, 7.0, 12.0, 19.0, -12.0, 0.0];
                let (mut a, mut b) = (0.0, 0.0);
                for (i, iv) in LAY.iter().enumerate() {
                    let cents = (if i % 2 == 0 { 1.0 } else { -1.0 }) * detune * 9.0 + dr[i] * detune * 6.0;
                    let f = f0 * 2f32.powf((iv + cents / 100.0) / 12.0);
                    // the octave below is the sub knob's
                    let w = [1.0, 0.45, 0.6, 0.25, 0.9 * self.v[P_SUB], 0.8][i] * (0.75 + 0.25 * dr[(i + 3) % 6]);
                    let s = saw(&mut v.ph[i], f / sr) * 0.4 + sine(&mut v.ph[6 + i], f / sr) * 0.6;
                    if i % 2 == 0 {
                        a += s * w;
                    } else {
                        b += s * w;
                    }
                }
                let cut = (f0 * (1.5 + bright * 10.0) * (1.0 + 0.4 * dr[2])).min(sr * 0.45);
                let g = g_of(cut, sr);
                let la = v.f[0].run(a * 0.25, g, 1.1).0;
                let lb = v.f[1].run(b * 0.25, g, 1.1).0;
                let tone = 1.0 - noise_mix * 0.6;
                // the wind: noise through a wandering band-pass, in gusts
                let wf = 250.0 * 2f32.powf(1.8 + dr[4] * 1.8 + bright);
                let gw = g_of(wf.min(sr * 0.45), sr);
                let gust = (0.55 + 0.45 * dr[5]).max(0.0);
                let (nl, nr) = (v.rng.next(), v.rng.next());
                let wl = v.f[2].run(nl, gw, 0.35).1 * gust * self.v[P_WIND] * 0.5;
                let wr = v.f[3].run(nr, gw * 1.07, 0.35).1 * gust * self.v[P_WIND] * 0.5;
                // the shimmer: short sine grains an octave, a twelfth and two octaves up
                let (mut gl, mut gr) = (0.0, 0.0);
                let sh = self.v[P_SHIMMER];
                if sh > 0.0 {
                    v.grain_next -= 1.0;
                    if v.grain_next <= 0.0 {
                        let rate = 1.0 + self.v[P_DENSITY] * 14.0;
                        v.grain_next = sr / rate * (0.4 + (v.rng.next() + 1.0) * 0.6);
                        if let Some(gi) = v.grains.iter().position(|g| !g.on) {
                            const UP: [f32; 5] = [12.0, 19.0, 24.0, 31.0, 36.0];
                            let u = UP[(((v.rng.next() + 1.0) * 2.5) as usize).min(4)];
                            v.grains[gi] = Grain { on: true, ph: 0.0, dt: f0 * 2f32.powf(u / 12.0) / sr, age: 0, len: (sr * (0.08 + (v.rng.next() + 1.0) * 0.17)) as u32, pan: v.rng.next() };
                        }
                    }
                    for g in v.grains.iter_mut().filter(|g| g.on) {
                        let e = (PI * g.age as f32 / g.len as f32).sin().powi(2);
                        let s = sine(&mut g.ph, g.dt) * e * sh * 0.18;
                        gl += s * (1.0 - g.pan);
                        gr += s * (1.0 + g.pan);
                        g.age += 1;
                        if g.age >= g.len {
                            g.on = false;
                        }
                    }
                }
                let a_s = 0.02 + self.v[P_ATTACK].powi(2) * 6.0;
                let amp = (t / (a_s * sr)).min(1.0);
                l = (la * tone + wl + gl) * amp * 2.0;
                r = (lb * tone + wr + gr) * amp * 2.0;
            }
        }
        // grit: saturation, and past half way fewer bits
        if grit > 0.0 && !matches!(v.engine, E_GLITCH | E_SUBDROP | E_IMPACT) {
            let d = 1.0 + grit * 4.0;
            l = (l * d).tanh() / d.tanh().max(0.5);
            r = (r * d).tanh() / d.tanh().max(0.5);
            if grit > 0.5 {
                let q = 2f32.powf(16.0 - (grit - 0.5) * 2.0 * 11.0);
                l = (l * q).round() / q;
                r = (r * q).round() / q;
            }
        }
        // the shared attack (not on the hits: their attack is the point)
        if !matches!(v.engine, E_IMPACT | E_LASER | E_GLITCH | E_SUBDROP | E_AMB) {
            let a_s = 0.001 + self.v[P_ATTACK].powi(2) * 4.0;
            v.att = (v.att + 1.0 / (a_s * sr)).min(1.0);
            l *= v.att;
            r *= v.att;
        }
        // no DC out of the drive and the saw stacks
        v.dc[0] += (l - v.dc[0]) * 0.0007;
        v.dc[1] += (r - v.dc[1]) * 0.0007;
        l -= v.dc[0];
        r -= v.dc[1];
        // the end of the note
        if !v.held {
            v.rel *= v.rel_k;
        }
        let g = v.vel * v.rel * (1.0 + press * 0.5);
        (l * g, r * g)
    }
}

fn defaults() -> [f32; NPARAM] {
    let mut v = [0.0; NPARAM];
    for (i, p) in PARAMS.iter().enumerate() {
        v[i] = p.1;
    }
    v
}

impl Instrument for Cinema {
    fn note_on(&mut self, key: u8, vel: f32) {
        self.start(key, vel, 0.0);
    }
    fn note_on_lyric(&mut self, key: u8, vel: f32, _pan: f32, _cutoff: f32, _reso: f32, _lyric: &str, len_samples: f32) {
        self.start(key, vel, len_samples);
    }
    fn note_off(&mut self, key: u8) {
        for v in self.voices.iter_mut().filter(|v| v.on && v.key == key && v.held) {
            if Cinema::gated(v.engine) {
                v.held = false;
            }
        }
    }
    fn note_pitch(&mut self, key: u8, semis: f32) {
        for v in self.voices.iter_mut().filter(|v| v.on && v.key == key) {
            let fresh = v.t == 0;
            v.pitch.set(semis, fresh);
        }
    }
    fn note_pressure(&mut self, key: u8, x: f32) {
        for v in self.voices.iter_mut().filter(|v| v.on && v.key == key) {
            let fresh = v.t == 0;
            v.press.set(x, fresh);
        }
    }
    fn all_off(&mut self) {
        for v in self.voices.iter_mut() {
            *v = Voice::new();
        }
    }
    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 20.0 {
            self.bpm = bpm as f32;
        }
    }
    fn render(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        // the hall: its size and decay from the space knob
        let space = self.v[P_SPACE];
        let want = (space, self.v[P_TAIL]);
        if want != self.hall_set {
            self.hall_set = want;
            self.hall.set(0.45 + space * 0.55, 1.2 + space * 5.0 + self.v[P_TAIL] * 2.0, 0.35);
        }
        let any = self.voices.iter().any(|v| v.on);
        let width = self.v[P_WIDTH];
        let level = self.v[P_LEVEL];
        let gain = level * level * 0.9;
        let wet = space * 0.55;
        let mut voices = std::mem::take(&mut self.voices);
        for v in voices.iter_mut().filter(|v| v.on) {
            v.pitch.begin(frames);
            v.press.begin(frames);
        }
        for i in 0..frames {
            let (mut l, mut r) = (0.0f32, 0.0f32);
            if any {
                for v in voices.iter_mut().filter(|v| v.on) {
                    let (a, b) = self.voice_sample(v);
                    let (a, b) = if a.is_finite() && b.is_finite() { (a, b) } else { (0.0, 0.0) };
                    l += a;
                    r += b;
                    v.t = v.t.saturating_add(1);
                    // free the voice once it is done and quiet
                    if !v.held {
                        if v.rel < 1e-4 || (a.abs() + b.abs()) < 1e-6 {
                            v.quiet += 1;
                        } else {
                            v.quiet = 0;
                        }
                        if v.rel < 1e-4 || v.quiet > 4800 {
                            v.on = false;
                        }
                    }
                }
            }
            // width on the dry, then the hall
            let (m, s) = ((l + r) * 0.5, (l - r) * 0.5 * width * 1.6);
            let (dl, dr) = (m + s, m - s);
            let (hl, hr) = self.hall.run(dl, dr);
            let ol = (dl + hl * wet) * gain;
            let or = (dr + hr * wet) * gain;
            let (ol, or) = (ol.tanh(), or.tanh());
            out[i * 2] += if ol.is_finite() { ol } else { 0.0 };
            out[i * 2 + 1] += if or.is_finite() { or } else { 0.0 };
        }
        self.voices = voices;
    }
    fn can_sleep(&self) -> bool {
        // the hall's tail rings on after the last voice
        true
    }
    fn params(&self) -> Vec<(&'static str, f32)> {
        PARAMS.iter().zip(self.v.iter()).map(|(p, v)| (p.0, *v)).collect()
    }
    fn set_param(&mut self, name: &str, v: f32) {
        if !v.is_finite() {
            return;
        }
        if let Some(i) = idx(name) {
            self.v[i] = v.clamp(0.0, 1.0);
        }
    }
    fn name(&self) -> &'static str {
        "Ling Cinema"
    }
    fn presets(&self) -> &'static [&'static str] {
        static N: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
        N.get_or_init(|| PRESETS.iter().map(|p| p.0).collect())
    }
    fn preset_categories(&self) -> Vec<(String, String)> {
        PRESETS.iter().map(|p| (p.0.to_string(), p.1.to_string())).collect()
    }
    fn load_preset(&mut self, name: &str) {
        if let Some(p) = PRESETS.iter().find(|p| p.0.eq_ignore_ascii_case(name)) {
            self.v = defaults();
            for (k, x) in p.3 {
                self.set_param(k, *x);
            }
            self.preset = p.0.to_string();
        }
    }
    fn preset_info(&self, name: &str) -> String {
        PRESETS.iter().find(|p| p.0.eq_ignore_ascii_case(name)).map(|p| p.2.to_string()).unwrap_or_default()
    }
    fn info(&self, what: &str) -> String {
        match what {
            "preset" => PRESETS.iter().find(|p| p.0 == self.preset).map(|p| p.2.to_string()).unwrap_or_default(),
            "engine" => ENGINES[self.engine()].to_string(),
            "engines" => ENGINES.join("\n"),
            _ => String::new(),
        }
    }
    fn randomize(&mut self, seed: u32) -> bool {
        let mut r = Rng::new(seed.wrapping_mul(2654435761).wrapping_add(11));
        let p = &PRESETS[(((r.next() + 1.0) * 0.5 * PRESETS.len() as f32) as usize).min(PRESETS.len() - 1)];
        self.load_preset(p.0);
        for n in ["amount", "curve", "bright", "noise", "detune", "motion", "metal", "crack", "shimmer", "wind", "density", "seed"] {
            if let Some(i) = idx(n) {
                self.v[i] = (self.v[i] + r.next() * 0.2).clamp(0.0, 1.0);
            }
        }
        true
    }
    /// "env": the loudness over the effect's length; "pitch": the pitch over it (0.5 = the note, 0 / 1 = the amount down / up)
    fn plot(&self, what: &str, n: usize) -> Vec<f32> {
        let e = self.engine();
        let amt = self.v[P_AMOUNT];
        let curve = self.v[P_CURVE];
        let ex = 0.6 + curve * 2.4;
        (0..n)
            .map(|i| {
                let p = i as f32 / (n.max(2) - 1) as f32;
                match what {
                    "env" => match e {
                        E_RISER => 0.06 + 0.94 * p.powf(1.0 + curve * 2.0),
                        E_DOWN => (1.0 - p).powf(0.8 + curve * 2.0),
                        E_IMPACT => {
                            let pre = self.v[P_PRE] * 0.3;
                            if p < pre { (p / pre).powi(3) } else { (-(p - pre) * 6.0).exp() }
                        }
                        E_WHOOSH => {
                            let pk = 0.35 + curve * 0.3;
                            let q = if p < pk { p / pk } else { 1.0 - (p - pk) / (1.0 - pk) };
                            (q * PI * 0.5).sin().powi(2)
                        }
                        E_SUBDROP => (1.0 - p).powf(0.6),
                        E_BRAAM => if p < 0.3 { (p / 0.3).powf(0.7) } else { 1.0 - 0.55 * ((p - 0.3) / 0.7) },
                        E_REVERSE => p.powf(2.0 + curve * 4.0),
                        E_LASER => {
                            let m = self.v[P_MOTION];
                            let u = if m > 0.02 { (p * (3.0 + m * 12.0)).fract() } else { p };
                            (1.0 - u).powf(1.4)
                        }
                        E_GLITCH => if ((p * 37.0).sin() * 999.0).fract().abs() < 0.25 + self.v[P_DENSITY] * 0.75 { 0.8 } else { 0.1 },
                        E_TAPE => (1.0 - p).powf(0.4 + curve * 2.6).powf(0.3),
                        _ => (p * 4.0).min(1.0),
                    },
                    _ => match e {
                        E_RISER => 0.5 + 0.5 * amt * p.powf(ex),
                        E_DOWN => 0.5 - 0.5 * amt * (1.0 - (1.0 - p).powf(ex)),
                        E_IMPACT => 0.5 + 0.25 * amt * (-p * 30.0).exp(),
                        E_WHOOSH => 0.5 + ((0.5 - p) * 6.0).tanh() * amt * 0.125,
                        E_SUBDROP => 0.5 - 0.5 * amt * (1.0 - (1.0 - p).powf(1.0 + curve * 3.0)),
                        E_LASER => 0.5 + 0.5 * amt.max(0.125) * (1.0 - p).powf(3.0 + curve * 8.0),
                        E_TAPE => 0.5 * (1.0 - p).powf(0.4 + curve * 2.6),
                        _ => 0.5,
                    },
                }
            })
            .collect()
    }
}

type Pre = (&'static str, &'static str, &'static str, &'static [(&'static str, f32)]);

/// (name, category, description, settings over the defaults). engine = index / 10 (riser 0, downlifter .1, impact .2, whoosh .3,
/// sub drop .4, braam .5, reverse .6, laser .7, glitch .8, tape stop .9, ambience 1); length: 1 beat .286, 2 .429, 4 .571, 8 .714,
/// 16 .857, 32 1; amount x 48 semitones
pub const PRESETS: &[Pre] = &[
    // ── risers
    ("White Noise Riser", "Risers", "white noise sweeping up from the lows to the air, swelling into the drop - the classic EDM build", &[("engine", 0.0), ("noise", 1.0), ("bright", 0.7), ("curve", 0.6), ("motion", 0.0), ("space", 0.3), ("width", 0.7)]),
    ("Pitch Riser", "Risers", "detuned saws climbing two octaves with a little noise on top and a flutter that speeds up", &[("engine", 0.0), ("noise", 0.25), ("amount", 0.5), ("curve", 0.7), ("motion", 0.5), ("detune", 0.5), ("space", 0.35)]),
    ("Shepard Riser", "Risers", "an endless Shepard tone: it rises and rises and never arrives - tension without a top", &[("engine", 0.0), ("shepard", 1.0), ("noise", 0.3), ("amount", 0.75), ("curve", 0.15), ("motion", 0.2), ("space", 0.45)]),
    ("Tension Riser", "Risers", "a dark, slow noise and saw rise for film tension, the flutter tightening as it climbs", &[("engine", 0.0), ("noise", 0.55), ("amount", 0.25), ("bright", 0.35), ("curve", 0.85), ("motion", 0.75), ("grit", 0.25), ("space", 0.5)]),
    ("Hyper Riser", "Risers", "a bright, fast-fluttering saw rise for hard drops, three octaves in the length", &[("engine", 0.0), ("noise", 0.35), ("amount", 0.75), ("bright", 0.9), ("curve", 0.5), ("motion", 1.0), ("grit", 0.3), ("detune", 0.7)]),
    ("Reverse Cymbal", "Risers", "a cymbal played backwards: metal and noise swelling to a sudden stop on the downbeat", &[("engine", 0.6), ("noise", 1.0), ("metal", 0.6), ("bright", 0.7), ("curve", 0.3), ("length", 0.571), ("sync", 0.0), ("space", 0.25)]),
    ("Reverse Swell", "Risers", "a reversed pad: the note swelling up out of nothing into the next section", &[("engine", 0.6), ("noise", 0.25), ("metal", 0.2), ("bright", 0.5), ("curve", 0.2), ("space", 0.5)]),
    // ── downlifters
    ("Noise Downlifter", "Downlifters", "white noise falling from the air to the lows and fading out after the drop hits", &[("engine", 0.1), ("noise", 1.0), ("bright", 0.7), ("curve", 0.4), ("motion", 0.0), ("length", 0.571), ("sync", 0.0), ("space", 0.4)]),
    ("Pitch Fall", "Downlifters", "detuned saws dropping two octaves and dying away", &[("engine", 0.1), ("noise", 0.2), ("amount", 0.5), ("curve", 0.5), ("motion", 0.3), ("length", 0.571), ("sync", 0.0)]),
    ("Shepard Fall", "Downlifters", "an endless falling Shepard tone - the floor keeps going", &[("engine", 0.1), ("shepard", 1.0), ("noise", 0.25), ("amount", 0.75), ("curve", 0.2), ("length", 0.714), ("sync", 0.0), ("space", 0.5)]),
    ("Air Drop", "Downlifters", "a short breath of falling air after a hit", &[("engine", 0.1), ("noise", 0.9), ("bright", 0.85), ("curve", 0.9), ("length", 0.429), ("sync", 0.0), ("width", 0.9)]),
    // ── impacts
    ("Trailer Hit", "Impacts", "the trailer slam: a sub boom with a knock, a noise crack, a metal ring and a long dark tail into the hall", &[("engine", 0.2), ("sub", 0.9), ("crack", 0.6), ("metal", 0.45), ("tail", 0.6), ("amount", 0.5), ("space", 0.55), ("grit", 0.2)]),
    ("Sub Boom", "Impacts", "a deep sub boom with a soft knock and almost no top - the downbeat of a drop", &[("engine", 0.2), ("sub", 1.0), ("crack", 0.15), ("metal", 0.0), ("tail", 0.55), ("amount", 0.35), ("bright", 0.25), ("space", 0.3)]),
    ("Metal Slam", "Impacts", "a hit of inharmonic metal with a sharp crack, ringing out", &[("engine", 0.2), ("sub", 0.5), ("crack", 0.8), ("metal", 1.0), ("tail", 0.45), ("bright", 0.8), ("space", 0.45)]),
    ("Reverse Swell Impact", "Impacts", "a reversed swell for one beat, then the hit: place the note a beat before the downbeat", &[("engine", 0.2), ("pre", 1.0), ("sub", 0.85), ("crack", 0.5), ("metal", 0.4), ("tail", 0.6), ("space", 0.55)]),
    ("Thunder Hit", "Impacts", "a low boom and a long rumbling noise tail - distant thunder", &[("engine", 0.2), ("sub", 0.8), ("crack", 0.35), ("metal", 0.0), ("tail", 0.95), ("bright", 0.15), ("space", 0.7), ("grit", 0.3)]),
    ("Sub Drop", "Impacts", "a sine falling two octaves from the note with a click - the 808 sub drop", &[("engine", 0.4), ("amount", 0.5), ("curve", 0.4), ("grit", 0.2), ("crack", 0.4), ("length", 0.571), ("sync", 0.0), ("space", 0.1), ("width", 0.0)]),
    ("Deep Sub Drop", "Impacts", "a long, slow sub falling three octaves, a little driven", &[("engine", 0.4), ("amount", 0.75), ("curve", 0.2), ("grit", 0.45), ("crack", 0.2), ("length", 0.714), ("sync", 0.0), ("space", 0.15), ("width", 0.0)]),
    // ── whooshes
    ("Fast Whoosh", "Whooshes", "a quick swish of air passing from left to right", &[("engine", 0.3), ("noise", 0.95), ("length", 0.286), ("sync", 0.0), ("doppler", 0.8), ("bright", 0.7), ("curve", 0.4)]),
    ("Slow Swoosh", "Whooshes", "a long, airy swoosh that swells and passes", &[("engine", 0.3), ("noise", 0.9), ("length", 0.571), ("sync", 0.0), ("doppler", 0.6), ("bright", 0.5), ("curve", 0.6), ("space", 0.45)]),
    ("Doppler Pass", "Whooshes", "something flying past: a tone and air panning across, the pitch dropping as it goes by", &[("engine", 0.3), ("noise", 0.45), ("amount", 0.5), ("length", 0.429), ("sync", 0.0), ("doppler", 1.0), ("bright", 0.6), ("curve", 0.5)]),
    ("Air Swipe", "Whooshes", "a short, bright swipe for transitions and cuts", &[("engine", 0.3), ("noise", 1.0), ("length", 0.143), ("sync", 0.0), ("doppler", 0.5), ("bright", 0.95), ("curve", 0.25), ("width", 0.9)]),
    // ── braams
    ("Deep Braam", "Braams", "the cinematic brass: a stack of detuned saws an octave and two below, the filter swelling open - play low notes", &[("engine", 0.5), ("detune", 0.45), ("bright", 0.5), ("grit", 0.35), ("sub", 0.6), ("noise", 0.1), ("space", 0.55), ("length", 0.714), ("release", 0.35)]),
    ("Bright Braam", "Braams", "a brighter, more open brass stack for the big reveal", &[("engine", 0.5), ("detune", 0.35), ("bright", 0.85), ("grit", 0.3), ("sub", 0.4), ("space", 0.5), ("length", 0.571), ("release", 0.3)]),
    ("Distorted Braam", "Braams", "a driven, buzzing brass stack - the robot horn", &[("engine", 0.5), ("detune", 0.6), ("bright", 0.65), ("grit", 0.75), ("sub", 0.5), ("noise", 0.2), ("space", 0.4), ("release", 0.3)]),
    ("Dark Horn Stack", "Braams", "a slow, dark swell of horns that never quite opens", &[("engine", 0.5), ("detune", 0.3), ("bright", 0.2), ("grit", 0.15), ("sub", 0.7), ("attack", 0.3), ("space", 0.65), ("length", 0.857), ("release", 0.45)]),
    // ── glitch
    ("Data Burst", "Glitch", "a burst of random computer beeps, buzzes and noise, crushed - the data stream", &[("engine", 0.8), ("motion", 0.7), ("density", 0.7), ("grit", 0.5), ("bright", 0.7), ("length", 0.429), ("sync", 0.0), ("space", 0.2)]),
    ("Glitch Stream", "Glitch", "a long, dense stream of glitches for as long as the length", &[("engine", 0.8), ("motion", 0.9), ("density", 0.9), ("grit", 0.65), ("bright", 0.8), ("length", 0.714), ("sync", 1.0), ("seed", 0.42)]),
    ("Bit Spray", "Glitch", "short, sparse, very crushed bits", &[("engine", 0.8), ("motion", 1.0), ("density", 0.35), ("grit", 0.9), ("bright", 0.9), ("length", 0.286), ("sync", 0.0), ("seed", 0.77)]),
    ("Tape Stop", "Glitch", "a chord winding down to nothing as the tape stops", &[("engine", 0.9), ("curve", 0.4), ("bright", 0.6), ("detune", 0.3), ("noise", 0.15), ("length", 0.429), ("sync", 0.0)]),
    ("Power Down", "Glitch", "a slow power-down: the machine dies", &[("engine", 0.9), ("curve", 0.15), ("bright", 0.4), ("grit", 0.3), ("noise", 0.3), ("length", 0.571), ("sync", 0.0), ("space", 0.3)]),
    // ── ambience
    ("Ambient Drift", "Ambience", "a slowly evolving drone of drifting, detuned layers, soft wind and a shimmer of high grains - hold the note", &[("engine", 1.0), ("detune", 0.45), ("motion", 0.35), ("wind", 0.35), ("shimmer", 0.35), ("density", 0.4), ("bright", 0.45), ("attack", 0.55), ("release", 0.6), ("space", 0.7), ("width", 0.9)]),
    ("Dark Space", "Ambience", "a low, dark void: a deep drone, distant wind and no shimmer", &[("engine", 1.0), ("detune", 0.6), ("motion", 0.2), ("wind", 0.5), ("shimmer", 0.0), ("bright", 0.15), ("noise", 0.3), ("attack", 0.6), ("release", 0.65), ("space", 0.8), ("width", 0.9)]),
    ("Wind Field", "Ambience", "mostly wind: gusts of noise through a wandering band-pass over a faint drone", &[("engine", 1.0), ("noise", 0.9), ("wind", 1.0), ("shimmer", 0.1), ("motion", 0.6), ("bright", 0.5), ("attack", 0.5), ("release", 0.6), ("space", 0.5), ("width", 1.0)]),
    ("Glass Shimmer", "Ambience", "a bright cloud of glassy grains over a quiet pad", &[("engine", 1.0), ("shimmer", 1.0), ("density", 0.85), ("wind", 0.1), ("detune", 0.25), ("bright", 0.7), ("noise", 0.5), ("attack", 0.45), ("release", 0.6), ("space", 0.75), ("width", 1.0)]),
    ("Deep Drone", "Ambience", "a heavy, slowly beating drone for tension under a scene", &[("engine", 1.0), ("detune", 0.8), ("motion", 0.15), ("wind", 0.15), ("shimmer", 0.05), ("bright", 0.3), ("grit", 0.2), ("attack", 0.5), ("release", 0.55), ("space", 0.6)]),
    // ── sci-fi
    ("Laser Zap", "Sci-Fi", "a single laser shot: a fast pitch fall from far above the note", &[("engine", 0.7), ("amount", 0.75), ("curve", 0.4), ("motion", 0.0), ("grit", 0.2), ("bright", 0.7), ("length", 0.143), ("sync", 0.0), ("space", 0.25)]),
    ("Blaster Burst", "Sci-Fi", "a burst of rapid laser shots", &[("engine", 0.7), ("amount", 0.6), ("curve", 0.5), ("motion", 0.75), ("grit", 0.5), ("bright", 0.6), ("length", 0.429), ("sync", 0.0), ("space", 0.3)]),
    ("Tractor Beam", "Sci-Fi", "a humming Shepard rise with a fast flutter - something is pulling", &[("engine", 0.0), ("shepard", 1.0), ("noise", 0.1), ("amount", 0.5), ("motion", 0.9), ("curve", 0.0), ("grit", 0.35), ("space", 0.4)]),
    ("Warp Out", "Sci-Fi", "a falling zap into a fading tone - the ship jumps away", &[("engine", 0.1), ("noise", 0.15), ("amount", 1.0), ("curve", 0.8), ("motion", 0.6), ("grit", 0.25), ("length", 0.429), ("sync", 0.0), ("space", 0.5)]),
    ("Alien Signal", "Sci-Fi", "a slow, sparse glitch of tones from far away", &[("engine", 0.8), ("motion", 0.2), ("density", 0.5), ("grit", 0.1), ("bright", 0.5), ("length", 0.714), ("sync", 0.0), ("space", 0.75), ("seed", 0.13)]),
];

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 44100.0;

    pub(crate) fn play(c: &mut Cinema, key: u8, secs: f32, hold: f32) -> Vec<f32> {
        let n = (SR * secs) as usize;
        let mut out = vec![0f32; n * 2];
        c.note_on_lyric(key, 0.9, 0.0, 0.5, 0.5, "", hold * SR);
        let off = (hold * SR) as usize * 2;
        let mut i = 0;
        while i < out.len() {
            if i >= off && i < off + 512 {
                c.note_off(key);
            }
            let k = 512.min(out.len() - i);
            c.render(&mut out[i..i + k]);
            i += k;
        }
        out
    }
    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
    }

    #[test]
    fn every_engine_renders_finite_sane_audio() {
        for (e, name) in ENGINES.iter().enumerate() {
            let mut c = Cinema::new(SR);
            c.set_tempo(120.0);
            c.set_param("engine", e as f32 / 10.0);
            assert_eq!(c.engine(), e);
            c.set_param("length", 4.0 / 7.0);
            c.set_param("sync", 0.0);
            let o = play(&mut c, 48, 4.0, 2.5);
            let pk = o.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            let r = rms(&o);
            assert!(o.iter().all(|x| x.is_finite()), "{name}");
            assert!(pk > 0.02 && pk <= 1.0, "{name}: peak {pk}");
            assert!(r > 0.003, "{name}: rms {r}");
            let l: Vec<f32> = o.chunks(2).map(|c| c[0]).collect();
            let dc = l.iter().sum::<f32>() / l.len() as f32;
            assert!(dc.abs() < 0.03, "{name}: dc {dc}");
            // it ends: a while after the note and the effect, the voices are free
            let mut z = vec![0f32; (SR * 12.0) as usize * 2];
            for ch in z.chunks_mut(512) {
                c.render(ch);
            }
            assert!(!c.voices.iter().any(|v| v.on), "{name}: a voice hangs");
            let late = rms(&z[z.len() - 4410..]);
            assert!(late < 1e-3, "{name}: still sounding {late}");
        }
    }

    #[test]
    fn every_preset_sounds_and_stays_sane() {
        assert!(PRESETS.len() >= 30);
        let cats: Vec<&str> = PRESETS.iter().map(|p| p.1).collect();
        for c in ["Risers", "Downlifters", "Impacts", "Whooshes", "Braams", "Glitch", "Ambience", "Sci-Fi"] {
            assert!(cats.contains(&c), "{c}");
        }
        for p in PRESETS {
            let mut c = Cinema::new(SR);
            c.set_tempo(128.0);
            c.load_preset(p.0);
            let o = play(&mut c, 48, 3.0, 2.0);
            let pk = o.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(o.iter().all(|x| x.is_finite()), "{}", p.0);
            assert!(pk > 0.02 && pk <= 1.0, "{}: {pk}", p.0);
            assert!(!p.2.is_empty());
        }
    }

    #[test]
    fn a_riser_rises_follows_the_held_note_and_a_downlifter_falls() {
        let zc = |v: &[f32]| v.chunks(2).map(|c| c[0]).collect::<Vec<_>>().windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        let mut c = Cinema::new(SR);
        c.load_preset("Pitch Riser");
        c.set_param("noise", 0.0);
        c.set_param("motion", 0.0);
        c.set_param("space", 0.0);
        let o = play(&mut c, 48, 2.0, 2.0);
        let q = o.len() / 4;
        assert!(zc(&o[q * 3..]) > zc(&o[..q]) * 2, "the pitch climbs");
        assert!(rms(&o[q * 3..]) > 2.0 * rms(&o[..q]), "and it swells");
        // the length follows the held note: a 1 s note is at full height at 1 s
        let mut c = Cinema::new(SR);
        c.load_preset("White Noise Riser");
        c.set_param("space", 0.0);
        let short = play(&mut c, 60, 1.5, 1.0);
        let at = |v: &[f32], s: f32| rms(&v[((s - 0.1) * SR) as usize * 2..(s * SR) as usize * 2]);
        assert!(at(&short, 1.0) > at(&short, 0.5) * 1.5);
        assert!(at(&short, 1.5) < at(&short, 1.0) * 0.1, "it stops with the note");
        let mut c = Cinema::new(SR);
        c.load_preset("Pitch Fall");
        c.set_param("noise", 0.0);
        c.set_param("space", 0.0);
        let o = play(&mut c, 72, 2.0, 2.0);
        let q = (SR * 0.4) as usize * 2;
        assert!(zc(&o[..q]) > zc(&o[q..q * 2]), "the pitch falls");
    }

    #[test]
    fn the_ambience_holds_while_the_note_is_held() {
        let mut c = Cinema::new(SR);
        c.load_preset("Ambient Drift");
        let o = play(&mut c, 48, 9.0, 8.0);
        let at = |s: f32| rms(&o[(s * SR) as usize * 2..((s + 0.5) * SR) as usize * 2]);
        assert!(at(7.0) > 0.01 && at(4.0) > 0.01, "{} {}", at(4.0), at(7.0));
        assert!(at(7.0) > at(4.0) * 0.4, "it does not die while held");
    }

    #[test]
    fn the_plot_curves_have_the_asked_length() {
        let mut c = Cinema::new(SR);
        for e in 0..ENGINES.len() {
            c.set_param("engine", e as f32 / 10.0);
            for w in ["env", "pitch"] {
                let p = c.plot(w, 64);
                assert_eq!(p.len(), 64);
                assert!(p.iter().all(|x| x.is_finite() && *x >= -0.01 && *x <= 1.01), "{e} {w}");
            }
        }
    }

    /// not a test: writes one patch file per preset into $CINEMA_OUT (instruments/Cinema), so the song generator can pick them
    #[test]
    #[ignore]
    fn dump_preset_patches() {
        let Ok(dir) = std::env::var("CINEMA_OUT") else { return };
        std::fs::create_dir_all(&dir).unwrap();
        for p in PRESETS {
            let mut c = Cinema::new(44100.0);
            c.load_preset(p.0);
            let d = defaults();
            let mut body = String::new();
            for (i, (n, v)) in c.params().into_iter().enumerate() {
                if (v - d[i]).abs() > 1e-4 {
                    body.push_str(&format!("{}={:.4}\n", n.replace(' ', "_"), v));
                }
            }
            let text = format!("# Ling Cinema patch - edit freely; load from the Instrument tab\n# {}\nname = \"{}\"\ntype = cinema\n{}", p.2, p.0, body);
            std::fs::write(std::path::Path::new(&dir).join(format!("Cinema_{}.ling", p.0.replace(' ', "_"))), text).unwrap();
        }
    }
    /// not a test: the demo renders (`CINEMA_RENDERS=<dir>`): one WAV per engine, some presets, the effects, and the Digitize "Matrix
    /// Mirror" over a phrase the Ling Voice sings, its transform ramping from 0 to 1. Everything is synthesised here.
    #[test]
    #[ignore]
    fn render_demos() {
        use crate::dsp::fx::{make, presets, Effect};
        let Ok(dir) = std::env::var("CINEMA_RENDERS") else { return };
        std::fs::create_dir_all(&dir).unwrap();
        let sr = 48000.0f32;
        let save = |name: &str, data: &[f32]| {
            let pk = data.iter().fold(0.0f32, |m, x| m.max(x.abs())).max(1e-9);
            // never past -1 dBFS peak
            let g = if pk > 0.89 { 0.89 / pk } else { 1.0 };
            let v: Vec<f32> = data.iter().map(|x| x * g).collect();
            let path = std::path::Path::new(&dir).join(format!("{name}.wav"));
            crate::export::write_audio(&path.to_string_lossy(), &v, sr as u32, "wav", 16, 0.0).unwrap();
        };
        let play = |c: &mut Cinema, key: u8, secs: f32, hold: f32| -> Vec<f32> {
            let n = (sr * secs) as usize;
            let mut out = vec![0f32; n * 2];
            c.note_on_lyric(key, 0.9, 0.0, 0.5, 0.5, "", hold * sr);
            let off = (hold * sr) as usize * 2;
            let mut i = 0;
            while i < out.len() {
                if i >= off && i < off + 512 {
                    c.note_off(key);
                }
                let k = 512.min(out.len() - i);
                c.render(&mut out[i..i + k]);
                i += k;
            }
            out
        };
        // one per engine, on a sensible key, through the preset that shows it best
        let per_engine = [
            ("riser", "Pitch Riser", 48u8, 6.0f32, 4.0f32),
            ("downlifter", "Noise Downlifter", 60, 4.0, 3.0),
            ("impact", "Trailer Hit", 36, 6.0, 1.0),
            ("whoosh", "Doppler Pass", 60, 3.0, 2.0),
            ("sub_drop", "Sub Drop", 36, 3.5, 2.0),
            ("braam", "Deep Braam", 36, 7.0, 4.5),
            ("reverse", "Reverse Cymbal", 60, 3.5, 2.0),
            ("laser", "Blaster Burst", 72, 2.5, 1.0),
            ("glitch", "Data Burst", 60, 2.5, 1.0),
            ("tape_stop", "Tape Stop", 48, 3.0, 2.0),
            ("ambience", "Ambient Drift", 48, 14.0, 11.0),
        ];
        for (e, preset, key, secs, hold) in per_engine {
            let mut c = Cinema::new(sr);
            c.set_tempo(120.0);
            c.load_preset(preset);
            let o = play(&mut c, key, secs, hold);
            save(&format!("engine_{e}"), &o);
        }
        for preset in ["Shepard Riser", "White Noise Riser", "Sub Boom", "Reverse Swell Impact", "Metal Slam", "Thunder Hit", "Fast Whoosh", "Distorted Braam", "Laser Zap", "Glitch Stream", "Power Down", "Dark Space", "Glass Shimmer", "Wind Field", "Tractor Beam", "Warp Out"] {
            let mut c = Cinema::new(sr);
            c.set_tempo(120.0);
            c.load_preset(preset);
            let amb = preset == "Dark Space" || preset == "Glass Shimmer" || preset == "Wind Field";
            let (secs, hold) = if amb { (12.0, 9.0) } else { (6.0, 4.0) };
            let low = ["Boom", "Thunder", "Braam", "Impact", "Slam"].iter().any(|w| preset.contains(w));
            let o = play(&mut c, if low { 36 } else { 52 }, secs, hold);
            save(&format!("preset_{}", preset.replace(' ', "_")), &o);
        }
        // a transition: two risers into an impact, a downlifter after it
        {
            let n = (sr * 9.0) as usize;
            let mut mix = vec![0f32; n * 2];
            for (preset, key, at, hold, secs) in [("White Noise Riser", 60u8, 0.0f32, 3.75f32, 4.0f32), ("Shepard Riser", 48, 0.0, 3.75, 4.0), ("Trailer Hit", 36, 3.75, 1.0, 5.0), ("Noise Downlifter", 64, 3.75, 1.0, 3.0)] {
                let mut c = Cinema::new(sr);
                c.set_tempo(128.0);
                c.load_preset(preset);
                let o = play(&mut c, key, secs, hold);
                let s0 = (at * sr) as usize * 2;
                for (i, x) in o.iter().enumerate() {
                    if s0 + i < mix.len() {
                        mix[s0 + i] += x;
                    }
                }
            }
            save("transition_riser_impact_downlifter", &mix);
        }
        // the Matrix mirror: the Ling Voice sings a phrase and yells a long "no", the Digitize transform rising 0 -> 1 under it
        let mut v = crate::dsp::make_instrument("voice", sr).unwrap();
        v.load_preset("kai");
        let total = (sr * 6.5) as usize;
        let mut dry = vec![0f32; total * 2];
        // (start s, length s, key, syllable in IPA)
        let notes: [(f32, f32, u8, &str); 6] = [(0.2, 0.35, 62, "wʌt"), (0.6, 0.3, 64, "ɪz"), (0.95, 0.55, 65, "ðɪs"), (1.7, 0.4, 64, "aɪ"), (2.15, 0.45, 62, "kænt"), (2.8, 3.0, 69, "noʊ")];
        let block = 256usize;
        let mut active: Vec<(u8, usize)> = vec![];
        let mut pos = 0usize;
        while pos < total {
            let t = pos as f32 / sr;
            for (st, len, key, syl) in notes.iter() {
                let s0 = (st * sr) as usize;
                if s0 >= pos && s0 < pos + block {
                    v.note_on_lyric(*key, 0.95, 0.0, 0.5, 0.5, syl, len * sr);
                    active.push((*key, ((st + len) * sr) as usize));
                }
            }
            // the yell bends up a little and strains
            if t > 2.8 {
                let u = ((t - 2.8) / 3.0).min(1.0);
                v.note_pitch(69, 1.5 * u * u);
                v.note_pressure(69, 0.4 + 0.6 * u);
            }
            let mut ended = vec![];
            active.retain(|(k, end)| {
                if *end < pos + block {
                    ended.push(*k);
                    false
                } else {
                    true
                }
            });
            for k in ended {
                v.note_off(k);
            }
            let k = block.min(total - pos);
            v.render(&mut dry[pos * 2..(pos + k) * 2]);
            pos += k;
        }
        save("digitize_source_voice_dry", &dry);
        let run_fx = |name: &str, preset: &str, src: &[f32], ramp: Option<(f32, f32)>, stop: Option<(f32, f32)>| -> Vec<f32> {
            let mut e: Box<dyn Effect> = make(name, sr).unwrap();
            if let Some((_, set)) = presets(name).iter().find(|p| p.0 == preset) {
                for (k, x) in set.iter() {
                    e.set_param(k, *x);
                }
            }
            let mut b = src.to_vec();
            let n = b.len() / 2;
            let mut i = 0;
            while i < n {
                let k = 256.min(n - i);
                let t = i as f32 / sr;
                if let Some((t0, t1)) = ramp {
                    e.set_param("transform", ((t - t0) / (t1 - t0)).clamp(0.0, 1.0));
                }
                if let Some((t0, t1)) = stop {
                    e.set_param("stop", if t >= t0 && t < t1 { 1.0 } else { 0.0 });
                }
                e.process(&mut b[i * 2..(i + k) * 2]);
                i += k;
            }
            b
        };
        save("digitize_matrix_mirror_transform_0_to_1", &run_fx("Digitize", "Matrix Mirror", &dry, Some((0.8, 5.5)), None));
        for p in ["Telephone", "Walkie Talkie", "AM Radio", "Robot", "Data Corrupt", "Low Bitrate Codec", "Megaphone"] {
            save(&format!("digitize_{}", p.replace(' ', "_")), &run_fx("Digitize", p, &dry, None, None));
        }
        // the other effects on the same phrase (with room for the tails), the tape stop pressed from 2.5 to 4 s
        let mut with_tail = dry.clone();
        with_tail.extend(vec![0f32; (sr * 5.0) as usize * 2]);
        save("shimmer_verb_heaven", &run_fx("Shimmer Verb", "Heaven shimmer", &with_tail, None, None));
        save("granular_cloud_soft", &run_fx("Granular Cloud", "Soft cloud", &with_tail, None, None));
        save("granular_cloud_reverse_haze", &run_fx("Granular Cloud", "Reverse haze", &with_tail, None, None));
        save("tape_stop_turntable_brake", &run_fx("Tape Stop", "Turntable brake", &dry, None, Some((2.5, 4.0))));
    }
}
