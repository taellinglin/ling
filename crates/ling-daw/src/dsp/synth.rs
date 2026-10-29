//! "Ling Synth X": one voice architecture combining
//!  * subtractive: osc (sine..tri..saw..pulse morph, pulse width) + noise -> SVF with envelope
//!  * additive: up to 16 partials with rolloff, odd/even tilt, inharmonicity and per-partial decay
//!  * FM: sine modulator (ratio, index, envelope-following) on a sine carrier
//!  * string: Karplus-Strong plucked / struck string (decay, brightness, pluck position)
//! plus the things that make General MIDI instruments believable: pitch envelope, key tracking,
//! velocity-to-brightness, hammer/pick click, breath noise, a vowel formant filter, delayed vibrato
//! and tremolo. The sources are layered by `sub` / `add` / `fm` / `pluck` levels.
use crate::dsp::fx::{FilterMode, Svf};
use crate::dsp::{midi_hz, Instrument, Rng, Smooth, TAU};

#[derive(Clone, Copy)]
pub struct Adsr {
    pub a: f32,
    pub d: f32,
    pub s: f32,
    pub r: f32,
}
#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Attack,
    Decay,
    Sustain,
    Release,
    Off,
}
#[derive(Clone)]
struct Env {
    st: Stage,
    v: f32,
}
impl Env {
    fn new() -> Self {
        Env { st: Stage::Off, v: 0.0 }
    }
    fn on(&mut self) {
        self.st = Stage::Attack;
        self.v = 0.0;
    }
    fn off(&mut self) {
        if self.st != Stage::Off {
            self.st = Stage::Release;
        }
    }
    #[inline]
    fn tick(&mut self, p: &Adsr, dt: f32) -> f32 {
        match self.st {
            Stage::Attack => {
                self.v += dt / p.a.max(1e-4);
                if self.v >= 1.0 {
                    self.v = 1.0;
                    self.st = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.v -= dt / p.d.max(1e-4) * (1.0 - p.s).max(0.01);
                if self.v <= p.s {
                    self.v = p.s;
                    self.st = Stage::Sustain;
                }
            }
            Stage::Sustain => self.v = p.s,
            Stage::Release => {
                // -60 dB over the release time
                self.v *= (-6.9 * dt / p.r.max(1e-3)).exp();
                if self.v <= 0.0005 {
                    self.v = 0.0;
                    self.st = Stage::Off;
                }
            }
            Stage::Off => {}
        }
        self.v
    }
}

#[derive(Clone)]
pub struct SynthParams {
    pub wave: f32,
    pub pulse: f32,
    pub mix_sub: f32,
    pub mix_add: f32,
    pub mix_fm: f32,
    pub mix_str: f32,
    pub cutoff: f32,
    pub reso: f32,
    pub filt_env: f32,
    pub filt_mode: FilterMode,
    pub key_track: f32,
    pub vel_cut: f32,
    pub partials: f32,
    pub rolloff: f32,
    pub odd_even: f32,
    pub inharm: f32,
    pub damp: f32,
    pub fm_ratio: f32,
    pub fm_index: f32,
    pub fm_env: f32,
    pub str_decay: f32,
    pub str_bright: f32,
    pub str_pos: f32,
    pub click: f32,
    pub pitch_env: f32,
    pub pitch_time: f32,
    pub detune: f32,
    pub noise: f32,
    pub breath: f32,
    pub formant: f32,
    pub vowel: f32,
    pub gain: f32,
    pub amp: Adsr,
    pub fenv: Adsr,
    pub lfo_rate: f32,
    pub lfo_pitch: f32,
    pub lfo_cut: f32,
    pub lfo_amp: f32,
    pub vib_delay: f32,
    /// pitch wheel, 0.5 = centre
    pub bend: f32,
    /// wheel range: semitones = 1 + v * 23
    pub bend_range: f32,
    /// legato portamento: 0 = off (every note its own voice); above 0 a note that starts while another is held takes that voice over
    /// without retriggering it and slides from its pitch (time constant 0.002 + v^2 * 0.8 s), as a mono synth's slide
    pub glide: f32,
}
impl Default for SynthParams {
    fn default() -> Self {
        SynthParams {
            wave: 0.66,
            pulse: 0.5,
            mix_sub: 1.0,
            mix_add: 0.0,
            mix_fm: 0.0,
            mix_str: 0.0,
            cutoff: 0.6,
            reso: 0.2,
            filt_env: 0.4,
            filt_mode: FilterMode::Low,
            key_track: 0.0,
            vel_cut: 0.0,
            partials: 0.5,
            rolloff: 0.5,
            odd_even: 0.5,
            inharm: 0.0,
            damp: 0.0,
            fm_ratio: 0.25,
            fm_index: 0.3,
            fm_env: 0.5,
            str_decay: 0.5,
            str_bright: 0.5,
            str_pos: 0.0,
            click: 0.0,
            pitch_env: 0.5,
            pitch_time: 0.3,
            detune: 0.15,
            noise: 0.0,
            breath: 0.0,
            formant: 0.0,
            vowel: 0.0,
            gain: 0.5,
            amp: Adsr { a: 0.005, d: 0.25, s: 0.7, r: 0.25 },
            fenv: Adsr { a: 0.005, d: 0.3, s: 0.3, r: 0.25 },
            lfo_rate: 0.3,
            lfo_pitch: 0.0,
            lfo_cut: 0.0,
            lfo_amp: 0.0,
            vib_delay: 0.0,
            bend: 0.5,
            bend_range: 1.0 / 23.0,
            glide: 0.0,
        }
    }
}

const NPART: usize = 16;

struct Voice {
    key: u8,
    vel: f32,
    freq: f32,
    /// the sliding pitch while a legato glide runs (Hz), 0 = none (the voice plays `freq`)
    slide: f32,
    on: bool,
    ph: [f32; 2],
    hph: [f32; NPART],
    mod_ph: f32,
    car_ph: f32,
    aenv: Env,
    fenv: Env,
    fl: Svf,
    fr: Svf,
    /// two formant band-pass filters per channel
    fm_l: [Svf; 2],
    fm_r: [Svf; 2],
    breath: Svf,
    age: u64,
    pan: f32,
    cut_mod: f32,
    reso_mod: f32,
    /// seconds since note-on
    t: f32,
    /// pitch envelope state 1 -> 0
    pe: f32,
    /// click envelope 1 -> 0
    ck: f32,
    /// Karplus-Strong delay line
    ks: Vec<f32>,
    ks_w: usize,
    ks_lp: f32,
    ks_live: bool,
    /// per-note expression: pitch gesture (semitones), pressure 0..1, timbre 0..1 (0.5 = neutral), smoothed over each block
    pb: Smooth,
    pr: Smooth,
    tm: Smooth,
    /// started (or taken over) since the last block: the first expression value is taken at once
    fresh: bool,
}

pub struct LingSynth {
    pub p: SynthParams,
    voices: Vec<Voice>,
    sr: f32,
    rng: Rng,
    lfo: f32,
    clock: u64,
    last: usize,
}

/// cheap sine, phase in cycles (any range); max error ~0.001
#[inline]
fn fsin(x: f32) -> f32 {
    let t = x - (x + 0.5).floor();
    let p = 8.0 * t * (1.0 - 2.0 * t.abs());
    p * (0.225 * (p.abs() - 1.0) + 1.0)
}

/// (F1, F2) in Hz for vowel 0..1 = a e i o u
fn formants(v: f32) -> (f32, f32) {
    const T: [(f32, f32); 5] = [(730.0, 1090.0), (530.0, 1840.0), (270.0, 2290.0), (570.0, 840.0), (300.0, 870.0)];
    let x = v.clamp(0.0, 1.0) * 4.0;
    let i = (x as usize).min(3);
    let f = x - i as f32;
    (T[i].0 + (T[i + 1].0 - T[i].0) * f, T[i].1 + (T[i + 1].1 - T[i].1) * f)
}

impl LingSynth {
    pub fn new(sr: f32) -> Self {
        let cap = (sr / 10.0) as usize + 8; // lowest string ~10 Hz
        let voices = (0..16)
            .map(|_| Voice {
                key: 0,
                vel: 0.0,
                freq: 440.0,
                slide: 0.0,
                on: false,
                ph: [0.0; 2],
                hph: [0.0; NPART],
                mod_ph: 0.0,
                car_ph: 0.0,
                aenv: Env::new(),
                fenv: Env::new(),
                fl: Svf::new(FilterMode::Low),
                fr: Svf::new(FilterMode::Low),
                fm_l: [Svf::new(FilterMode::Band), Svf::new(FilterMode::Band)],
                fm_r: [Svf::new(FilterMode::Band), Svf::new(FilterMode::Band)],
                breath: Svf::new(FilterMode::Band),
                age: 0,
                pan: 0.0,
                cut_mod: 0.5,
                reso_mod: 0.5,
                t: 0.0,
                pe: 0.0,
                ck: 0.0,
                ks: vec![0.0; cap],
                ks_w: 0,
                ks_lp: 0.0,
                ks_live: false,
                pb: Smooth::at(0.0),
                pr: Smooth::at(0.0),
                tm: Smooth::at(0.5),
                fresh: false,
            })
            .collect();
        LingSynth { p: SynthParams::default(), voices, sr, rng: Rng::new(7), lfo: 0.0, clock: 0, last: 0 }
    }

    pub const PRESETS: [&'static str; 5] = ["Init Saw", "Sub Bass", "Additive Organ", "FM Bell", "Hybrid Pad"];

    pub fn preset(&mut self, name: &str) {
        let mut p = SynthParams::default();
        match name {
            "Sub Bass" => {
                p.cutoff = 0.3;
                p.reso = 0.4;
                p.detune = 0.1;
                p.amp.r = 0.1;
            }
            "Additive Organ" => {
                p.mix_sub = 0.0;
                p.mix_add = 1.0;
                p.partials = 0.5;
                p.rolloff = 0.3;
                p.odd_even = 0.7;
                p.cutoff = 1.0;
                p.filt_env = 0.0;
                p.amp = Adsr { a: 0.01, d: 0.1, s: 0.9, r: 0.1 };
            }
            "FM Bell" => {
                p.mix_sub = 0.0;
                p.mix_fm = 1.0;
                p.fm_ratio = 0.58;
                p.fm_index = 0.5;
                p.fm_env = 0.8;
                p.cutoff = 1.0;
                p.filt_env = 0.0;
                p.amp = Adsr { a: 0.002, d: 1.6, s: 0.0, r: 0.8 };
            }
            "Hybrid Pad" => {
                p.mix_sub = 0.6;
                p.mix_add = 0.5;
                p.mix_fm = 0.2;
                p.cutoff = 0.45;
                p.filt_env = 0.2;
                p.detune = 0.4;
                p.lfo_cut = 0.2;
                p.amp = Adsr { a: 0.5, d: 0.5, s: 0.8, r: 1.2 };
            }
            _ => {}
        }
        self.p = p;
    }

    /// Fill the string's delay line with a (filtered, comb-shaped) noise burst.
    fn excite(&mut self, idx: usize) {
        let p = self.p.clone();
        let sr = self.sr;
        let v = &mut self.voices[idx];
        let n = ((sr / v.freq) as usize).clamp(2, v.ks.len() - 1);
        v.ks.iter_mut().for_each(|x| *x = 0.0);
        // brighter strings get a brighter excitation; harder hits (velocity) add more top end
        let k = (0.12 + 0.88 * (p.str_bright * 0.7 + v.vel * 0.3)).clamp(0.05, 1.0);
        let mut lp = 0.0;
        for i in 0..n {
            let x = self.rng.next();
            lp += (x - lp) * k;
            v.ks[i] = lp;
        }
        // pluck position: subtract a delayed copy (comb) => hollow tone near the bridge
        let pos = (p.str_pos * 0.5 * n as f32) as usize;
        if pos > 0 && pos < n {
            for i in (pos..n).rev() {
                v.ks[i] -= v.ks[i - pos];
            }
        }
        // normalise the burst
        let pk = v.ks[..n].iter().fold(1e-6f32, |m, x| m.max(x.abs()));
        let g = 0.9 / pk;
        v.ks[..n].iter_mut().for_each(|x| *x *= g);
        v.ks_w = n;
        v.ks_lp = 0.0;
        v.ks_live = true;
    }
}

#[inline]
fn osc(ph: f32, wave: f32, pw: f32) -> f32 {
    let sine = fsin(ph);
    let tri = 4.0 * (ph - 0.5).abs() - 1.0;
    let saw = 2.0 * ph - 1.0;
    let sq = if ph < pw { 0.7 } else { -0.7 } - (2.0 * pw - 1.0) * 0.7;
    let w = wave * 3.0;
    if w < 1.0 {
        sine + (tri - sine) * w
    } else if w < 2.0 {
        tri + (saw - tri) * (w - 1.0)
    } else {
        saw + (sq - saw) * (w - 2.0)
    }
}

impl Instrument for LingSynth {
    fn note_on(&mut self, key: u8, vel: f32) {
        self.clock += 1;
        // legato glide: a held voice (the newest) is taken over and slides to the new note, its envelopes keep going
        if self.p.glide > 0.0 {
            if let Some(i) = self.voices.iter().enumerate().filter(|(_, v)| v.on && v.aenv.st != Stage::Off).max_by_key(|(_, v)| v.age).map(|(i, _)| i) {
                let v = &mut self.voices[i];
                let mut from = if v.slide > 0.0 { v.slide } else { v.freq };
                // the slide starts where the voice is heard, its pitch gesture included; the new note brings its own
                if v.pb.cur != 0.0 {
                    from *= (v.pb.cur / 12.0).exp2();
                }
                v.pb.reset(0.0);
                v.pr.reset(0.0);
                v.tm.reset(0.5);
                v.fresh = true;
                v.key = key;
                v.vel = vel;
                v.freq = midi_hz(key as f32);
                v.slide = from;
                v.age = self.clock;
                self.last = i;
                return;
            }
        }
        let idx = self
            .voices
            .iter()
            .position(|v| v.aenv.st == Stage::Off)
            .or_else(|| self.voices.iter().enumerate().min_by_key(|(_, v)| v.age).map(|(i, _)| i))
            .unwrap();
        let mode = self.p.filt_mode;
        self.last = idx;
        let string = self.p.mix_str > 0.0;
        {
            let v = &mut self.voices[idx];
            v.pan = 0.0;
            v.cut_mod = 0.5;
            v.reso_mod = 0.5;
            v.key = key;
            v.vel = vel;
            v.freq = midi_hz(key as f32);
            v.slide = 0.0;
            v.on = true;
            v.age = self.clock;
            v.aenv.on();
            v.fenv.on();
            v.fl = Svf::new(mode);
            v.fr = Svf::new(mode);
            v.fm_l = [Svf::new(FilterMode::Band), Svf::new(FilterMode::Band)];
            v.fm_r = [Svf::new(FilterMode::Band), Svf::new(FilterMode::Band)];
            v.breath = Svf::new(FilterMode::Band);
            v.t = 0.0;
            v.pe = 1.0;
            v.ck = 1.0;
            v.hph = [0.0; NPART];
            v.ks_live = false;
            v.pb.reset(0.0);
            v.pr.reset(0.0);
            v.tm.reset(0.5);
            v.fresh = true;
        }
        if string {
            self.excite(idx);
        }
    }
    fn note_on_ex(&mut self, key: u8, vel: f32, pan: f32, cutoff: f32, reso: f32) {
        self.note_on(key, vel);
        let v = &mut self.voices[self.last];
        v.pan = pan.clamp(-1.0, 1.0);
        v.cut_mod = cutoff;
        v.reso_mod = reso;
    }
    fn note_off(&mut self, key: u8) {
        for v in self.voices.iter_mut().filter(|v| v.on && v.key == key) {
            v.on = false;
            v.aenv.off();
            v.fenv.off();
        }
    }
    fn all_off(&mut self) {
        for v in self.voices.iter_mut() {
            v.on = false;
            v.aenv.off();
            v.fenv.off();
        }
    }
    fn note_pitch(&mut self, key: u8, semis: f32) {
        for v in self.voices.iter_mut().filter(|v| v.on && v.key == key) {
            v.pb.set(semis, v.fresh);
        }
    }
    fn note_pressure(&mut self, key: u8, x: f32) {
        for v in self.voices.iter_mut().filter(|v| v.on && v.key == key) {
            v.pr.set(x, v.fresh);
        }
    }
    fn note_timbre(&mut self, key: u8, x: f32) {
        for v in self.voices.iter_mut().filter(|v| v.on && v.key == key) {
            v.tm.set(x, v.fresh);
        }
    }

    fn render(&mut self, out: &mut [f32]) {
        let p = self.p.clone();
        let sr = self.sr;
        let dt = 1.0 / sr;
        let nparts = 1 + (p.partials * 15.0) as usize;
        let ratio = 0.5 + (p.fm_ratio * 7.0 * 2.0).round() * 0.25;
        let vib_delay = p.vib_delay * p.vib_delay * 1.5;
        let pe_semi = (p.pitch_env - 0.5) * 2.0 * 24.0; // +-2 octaves
        let pe_tau = 0.004 + p.pitch_time * p.pitch_time * 0.6;
        let pe_decay = (-dt / pe_tau).exp();
        let ck_decay = (-dt / 0.008).exp();
        let bend = pe_semi != 0.0;
        let inh = p.inharm * p.inharm * p.inharm * 0.25;
        let damp_rate = p.damp * 8.0;
        // string loop: T60 from 0.15 s to ~18 s
        let t60 = 0.15 * 120.0f32.powf(p.str_decay);
        let ks_k = (0.25 + 0.75 * p.str_bright).clamp(0.1, 1.0);
        let n_vowel = formants(p.vowel);
        let glide_k = if p.glide > 0.0 { 1.0 - (-dt / (0.002 + p.glide * p.glide * 0.8)).exp() } else { 1.0 };
        let bend_mul = if p.bend != 0.5 { ((p.bend - 0.5) * 2.0 * (1.0 + p.bend_range * 23.0) / 12.0).exp2() } else { 1.0 };
        let frames = out.len() / 2;
        for v in self.voices.iter_mut() {
            v.pb.begin(frames);
            v.pr.begin(frames);
            v.tm.begin(frames);
            v.fresh = false;
        }
        for fr in out.chunks_exact_mut(2) {
            self.lfo = (self.lfo + dt * (0.1 + p.lfo_rate * 12.0)).fract();
            let lfo = fsin(self.lfo);
            let (mut l, mut r) = (0.0, 0.0);
            for v in self.voices.iter_mut() {
                if v.aenv.st == Stage::Off {
                    continue;
                }
                v.t += dt;
                let ae = v.aenv.tick(&p.amp, dt);
                let fe = v.fenv.tick(&p.fenv, dt);
                let vg = if vib_delay > 0.0 { (v.t / vib_delay).min(1.0) } else { 1.0 };
                let base = if v.slide > 0.0 {
                    v.slide += (v.freq - v.slide) * glide_k;
                    if (v.slide - v.freq).abs() < v.freq * 1e-4 {
                        v.slide = 0.0;
                    }
                    if v.slide > 0.0 { v.slide } else { v.freq }
                } else {
                    v.freq
                };
                let mut f0 = base * bend_mul * (1.0 + lfo * p.lfo_pitch * 0.05 * vg);
                if !v.pb.idle(0.0) {
                    f0 *= (v.pb.tick() / 12.0).exp2();
                }
                // pressure: louder and brighter; timbre: the cutoff either way (as the note's own cutoff)
                let (press, timbre) = (if v.pr.idle(0.0) { 0.0 } else { v.pr.tick() }, if v.tm.idle(0.5) { 0.5 } else { v.tm.tick() });
                if bend {
                    v.pe *= pe_decay;
                    f0 *= (pe_semi * v.pe / 12.0).exp2();
                }
                let (mut sl, mut sr_) = (0.0, 0.0);
                if p.mix_sub > 0.0 {
                    let det = 1.0 + p.detune * 0.012;
                    v.ph[0] = (v.ph[0] + f0 * dt).fract();
                    v.ph[1] = (v.ph[1] + f0 * det * dt).fract();
                    let n = self.rng.next() * p.noise;
                    sl += (osc(v.ph[0], p.wave, p.pulse) + n) * p.mix_sub;
                    sr_ += (osc(v.ph[1], p.wave, p.pulse) + n) * p.mix_sub;
                }
                if p.mix_add > 0.0 {
                    let bright = if damp_rate > 0.0 { (-v.t * damp_rate).exp() } else { 1.0 };
                    let (mut s, mut norm) = (0.0, 0.0);
                    let mut g = 1.0f32;
                    for h in 1..=nparts {
                        let hf = h as f32;
                        let ratio_h = if inh > 0.0 { hf * (1.0 + inh * hf * hf).sqrt() } else { hf };
                        if f0 * ratio_h > sr * 0.45 {
                            break;
                        }
                        let tilt = if h % 2 == 1 { p.odd_even * 2.0 } else { (1.0 - p.odd_even) * 2.0 };
                        let tilt = tilt.clamp(0.02, 1.0);
                        let a = (1.0 / hf.powf(0.2 + p.rolloff * 2.0)) * if h == 1 { 1.0 } else { tilt };
                        let ph = &mut v.hph[h - 1];
                        *ph = (*ph + f0 * ratio_h * dt).fract();
                        s += a * g * fsin(*ph);
                        norm += a;
                        g *= bright; // each higher partial fades faster
                    }
                    let s = s / norm.max(1.0) * p.mix_add * 1.4;
                    sl += s;
                    sr_ += s;
                }
                if p.mix_fm > 0.0 {
                    v.mod_ph = (v.mod_ph + f0 * ratio * dt).fract();
                    v.car_ph = (v.car_ph + f0 * dt).fract();
                    let idx = p.fm_index * 8.0 * (1.0 - p.fm_env + p.fm_env * ae);
                    let s = fsin(v.car_ph + fsin(v.mod_ph) * idx / TAU) * p.mix_fm;
                    sl += s;
                    sr_ += s;
                }
                if p.mix_str > 0.0 && v.ks_live {
                    let cap = v.ks.len();
                    let d = (sr / f0 - (1.0 - ks_k) / ks_k).clamp(2.0, cap as f32 - 2.0);
                    let mut rp = v.ks_w as f32 - d;
                    if rp < 0.0 {
                        rp += cap as f32;
                    }
                    let i0 = rp as usize;
                    let fr_ = rp - i0 as f32;
                    let y = v.ks[i0 % cap] * (1.0 - fr_) + v.ks[(i0 + 1) % cap] * fr_;
                    v.ks_lp += (y - v.ks_lp) * ks_k;
                    let fb = (-6.9 / (t60 * f0)).exp();
                    v.ks[v.ks_w] = v.ks_lp * fb;
                    v.ks_w = (v.ks_w + 1) % cap;
                    let s = y * p.mix_str * 1.6;
                    sl += s;
                    sr_ += s;
                }
                if p.click > 0.0 && v.ck > 0.001 {
                    v.ck *= ck_decay;
                    let c = self.rng.next() * v.ck * p.click * (0.3 + v.vel);
                    sl += c;
                    sr_ += c;
                }
                if p.formant > 0.0 {
                    let (f1, f2) = n_vowel;
                    let (a1, a2) = (v.fm_l[0].tick(sl, f1, 0.86, sr), v.fm_l[1].tick(sl, f2, 0.86, sr));
                    let (b1, b2) = (v.fm_r[0].tick(sr_, f1, 0.86, sr), v.fm_r[1].tick(sr_, f2, 0.86, sr));
                    let m = p.formant;
                    sl = sl * (1.0 - m) + (a1 * 1.6 + a2 * 1.0) * m;
                    sr_ = sr_ * (1.0 - m) + (b1 * 1.6 + b2 * 1.0) * m;
                }
                let cut = (p.cutoff
                    + (v.cut_mod - 0.5) * 0.8
                    + p.filt_env * fe
                    + lfo * p.lfo_cut * 0.3
                    + p.key_track * (v.key as f32 - 60.0) * 0.0088
                    + p.vel_cut * (v.vel - 0.7) * 0.5
                    + if press != 0.0 || timbre != 0.5 { press * 0.25 + (timbre - 0.5) * 0.8 } else { 0.0 })
                    .clamp(0.0, 1.0);
                let hz = 30.0 * (20000.0f32 / 30.0).powf(cut);
                let trem = if p.lfo_amp > 0.0 { 1.0 - p.lfo_amp * 0.5 * (1.0 + lfo) } else { 1.0 };
                let g = ae * v.vel * p.gain * trem * if press != 0.0 { 1.0 + press * 0.5 } else { 1.0 };
                let reso = (p.reso + (v.reso_mod - 0.5) * 0.8).clamp(0.0, 0.95);
                let ang = (v.pan + 1.0) * std::f32::consts::FRAC_PI_4;
                let (pl, pr) = (ang.cos() * std::f32::consts::SQRT_2, ang.sin() * std::f32::consts::SQRT_2);
                let mut ol = v.fl.tick(sl, hz, reso, sr) * g;
                let mut or = v.fr.tick(sr_, hz, reso, sr) * g;
                if p.breath > 0.0 {
                    // noise band around the played pitch, after the main filter: the air in flutes, pipes, shakuhachi
                    let bn = v.breath.tick(self.rng.next(), (f0 * 2.5).min(sr * 0.4), 0.6, sr) * p.breath * 2.0 * g;
                    ol += bn;
                    or += bn;
                }
                l += ol * pl;
                r += or * pr;
            }
            fr[0] += l;
            fr[1] += r;
        }
    }

    fn params(&self) -> Vec<(&'static str, f32)> {
        let p = &self.p;
        vec![
            ("wave", p.wave),
            ("sub", p.mix_sub),
            ("add", p.mix_add),
            ("fm", p.mix_fm),
            ("cutoff", p.cutoff),
            ("reso", p.reso),
            ("filt env", p.filt_env),
            ("partials", p.partials),
            ("rolloff", p.rolloff),
            ("odd/even", p.odd_even),
            ("fm ratio", p.fm_ratio),
            ("fm index", p.fm_index),
            ("detune", p.detune),
            ("noise", p.noise),
            ("attack", (p.amp.a / 2.0).sqrt()),
            ("decay", (p.amp.d / 3.0).sqrt()),
            ("sustain", p.amp.s),
            ("release", (p.amp.r / 4.0).sqrt()),
            ("lfo rate", p.lfo_rate),
            ("lfo cut", p.lfo_cut),
            ("lfo pitch", p.lfo_pitch),
            ("fm env", p.fm_env),
            ("gain", p.gain),
            ("filt mode", match p.filt_mode {
                FilterMode::Low => 0.0,
                FilterMode::Band => 0.5,
                FilterMode::High => 1.0,
            }),
            ("pulse", p.pulse),
            ("string", p.mix_str),
            ("str decay", p.str_decay),
            ("str bright", p.str_bright),
            ("str pos", p.str_pos),
            ("click", p.click),
            ("inharm", p.inharm),
            ("damp", p.damp),
            ("pitch env", p.pitch_env),
            ("pitch time", p.pitch_time),
            ("key track", p.key_track),
            ("vel cut", p.vel_cut),
            ("breath", p.breath),
            ("formant", p.formant),
            ("vowel", p.vowel),
            ("lfo amp", p.lfo_amp),
            ("vib delay", p.vib_delay),
            ("bend", p.bend),
            ("bend range", p.bend_range),
            ("fenv attack", (p.fenv.a / 2.0).sqrt()),
            ("fenv decay", (p.fenv.d / 3.0).sqrt()),
            ("fenv sustain", p.fenv.s),
            ("glide", p.glide),
        ]
    }
    fn set_param(&mut self, n: &str, v: f32) {
        if !v.is_finite() {
            return;
        }
        let v = v.clamp(0.0, 1.0);
        let p = &mut self.p;
        match n {
            "wave" => p.wave = v,
            "sub" => p.mix_sub = v,
            "add" => p.mix_add = v,
            "fm" => p.mix_fm = v,
            "cutoff" => p.cutoff = v,
            "reso" => p.reso = v,
            "filt env" => p.filt_env = v,
            "partials" => p.partials = v,
            "rolloff" => p.rolloff = v,
            "odd/even" => p.odd_even = v,
            "fm ratio" => p.fm_ratio = v,
            "fm index" => p.fm_index = v,
            "detune" => p.detune = v,
            "noise" => p.noise = v,
            "attack" => p.amp.a = v * v * 2.0,
            "decay" => p.amp.d = v * v * 3.0,
            "sustain" => p.amp.s = v,
            "release" => p.amp.r = v * v * 4.0,
            "lfo rate" => p.lfo_rate = v,
            "lfo cut" => p.lfo_cut = v,
            "lfo pitch" => p.lfo_pitch = v,
            "fm env" => p.fm_env = v,
            "gain" => p.gain = v,
            "filt mode" => p.filt_mode = if v < 0.33 { FilterMode::Low } else if v < 0.66 { FilterMode::Band } else { FilterMode::High },
            "pulse" => p.pulse = v.clamp(0.05, 0.95),
            "string" => p.mix_str = v,
            "str decay" => p.str_decay = v,
            "str bright" => p.str_bright = v,
            "str pos" => p.str_pos = v,
            "click" => p.click = v,
            "inharm" => p.inharm = v,
            "damp" => p.damp = v,
            "pitch env" => p.pitch_env = v,
            "pitch time" => p.pitch_time = v,
            "key track" => p.key_track = v,
            "vel cut" => p.vel_cut = v,
            "breath" => p.breath = v,
            "formant" => p.formant = v,
            "vowel" => p.vowel = v,
            "lfo amp" => p.lfo_amp = v,
            "vib delay" => p.vib_delay = v,
            "bend" => p.bend = v,
            "bend range" => p.bend_range = v,
            "fenv attack" => p.fenv.a = v * v * 2.0,
            "fenv decay" => p.fenv.d = v * v * 3.0,
            "fenv sustain" => p.fenv.s = v,
            "glide" => p.glide = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Ling Synth X"
    }
    fn presets(&self) -> &'static [&'static str] {
        &Self::PRESETS
    }
    fn load_preset(&mut self, name: &str) {
        self.preset(name);
    }
}
