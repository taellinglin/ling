//! Effects: SVF filter, delay, Schroeder reverb, compressor, 3-band EQ, drive.
use crate::dsp::TAU;

pub trait Effect: Send {
    fn process(&mut self, buf: &mut [f32]); // interleaved stereo, in place
    fn set_param(&mut self, name: &str, v: f32);
    fn name(&self) -> &'static str;
    /// Level (linear) of an external sidechain key for this block; None = no key.
    fn set_sidechain(&mut self, _level: Option<f32>) {}
    /// Trigger events for this block: (frame in the block, velocity 0..1). The Ducker starts its envelope on each one.
    fn set_triggers(&mut self, _events: &[(u32, f32)]) {}
    /// Live values for the effect's display (0..1 each), empty when it has none.
    fn view(&self) -> Vec<f32> {
        vec![]
    }
    /// Load a file the effect works from (a WAV for the growl's spectral source); false when it has none or cannot read it.
    fn load_source(&mut self, _path: &str) -> bool {
        false
    }
    /// The audio of the tracks routed to this track's modulator input for this block (interleaved stereo, same length as the block); None = none.
    fn set_aux(&mut self, _aux: Option<&[f32]>) {}
    /// Effect-specific edit commands (the Ringstorm's pedals): `cmd` with numeric `args` and some `text`; the reply is a list of numbers.
    fn command(&mut self, _cmd: &str, _args: &[f64], _text: &str) -> Vec<f64> {
        vec![]
    }
    /// Effect-specific text ("config" = the whole state as JSON for saving).
    fn info(&self, _what: &str) -> String {
        String::new()
    }
    /// The file loaded with `load_source`, empty for none / the built-in.
    fn source(&self) -> String {
        String::new()
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum FilterMode {
    Low,
    High,
    Band,
}

/// Chamberlin-style state-variable filter (2x-oversampled step for stability).
#[derive(Clone)]
pub struct Svf {
    lp: f32,
    bp: f32,
    pub mode: FilterMode,
}
impl Svf {
    pub fn new(mode: FilterMode) -> Self {
        Svf { lp: 0.0, bp: 0.0, mode }
    }
    #[inline]
    pub fn tick(&mut self, x: f32, cutoff_hz: f32, reso: f32, sr: f32) -> f32 {
        // 4 sub-steps per sample => coefficient computed at 4*sr; clamp keeps
        // the Chamberlin form stable at high cutoff / low damping.
        let f = (2.0 * (std::f32::consts::PI * cutoff_hz.clamp(20.0, sr * 0.45) / (4.0 * sr)).sin()).min(0.6);
        let q = 1.0 - reso.clamp(0.0, 0.95);
        let mut out = 0.0;
        for _ in 0..4 {
            let hp = x - self.lp - q * 2.0 * self.bp;
            self.bp += f * hp;
            self.lp += f * self.bp;
            out = match self.mode {
                FilterMode::Low => self.lp,
                FilterMode::High => hp,
                FilterMode::Band => self.bp,
            };
        }
        out
    }
}

pub struct Delay {
    buf: [Vec<f32>; 2],
    pos: usize,
    pub time: f32,
    pub feedback: f32,
    pub mix: f32,
    sr: f32,
}
impl Delay {
    pub fn new(sr: f32) -> Self {
        let n = (sr * 2.0) as usize;
        Delay { buf: [vec![0.0; n], vec![0.0; n]], pos: 0, time: 0.3, feedback: 0.4, mix: 0.3, sr }
    }
}
impl Effect for Delay {
    fn process(&mut self, b: &mut [f32]) {
        let n = self.buf[0].len();
        let d = ((self.time * self.sr) as usize).clamp(1, n - 1);
        for fr in b.chunks_exact_mut(2) {
            let rp = (self.pos + n - d) % n;
            for c in 0..2 {
                let wet = self.buf[c][rp];
                self.buf[c][self.pos] = fr[c] + wet * self.feedback;
                fr[c] += wet * self.mix;
            }
            self.pos = (self.pos + 1) % n;
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        match n {
            "time" => self.time = 0.02 + v * 1.5,
            "feedback" => self.feedback = v * 0.95,
            "mix" => self.mix = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Delay"
    }
}

struct Comb {
    buf: Vec<f32>,
    i: usize,
    lp: f32,
}
struct Allpass {
    buf: Vec<f32>,
    i: usize,
}
pub struct Reverb {
    combs: [Vec<Comb>; 2],
    aps: [Vec<Allpass>; 2],
    pub mix: f32,
    pub size: f32,
    pub damp: f32,
}
impl Reverb {
    pub fn new(sr: f32) -> Self {
        let k = sr / 44100.0;
        let ct = [1116usize, 1188, 1277, 1356, 1422, 1491];
        let at = [556usize, 441, 341];
        let mk = |off: usize| -> (Vec<Comb>, Vec<Allpass>) {
            (
                ct.iter().map(|&t| Comb { buf: vec![0.0; ((t + off) as f32 * k) as usize], i: 0, lp: 0.0 }).collect(),
                at.iter().map(|&t| Allpass { buf: vec![0.0; ((t + off) as f32 * k) as usize], i: 0 }).collect(),
            )
        };
        let (c0, a0) = mk(0);
        let (c1, a1) = mk(23);
        Reverb { combs: [c0, c1], aps: [a0, a1], mix: 0.25, size: 0.8, damp: 0.4 }
    }
}
impl Effect for Reverb {
    fn process(&mut self, b: &mut [f32]) {
        for fr in b.chunks_exact_mut(2) {
            let inp = (fr[0] + fr[1]) * 0.5 * 0.15;
            for c in 0..2 {
                let mut acc = 0.0;
                for cb in self.combs[c].iter_mut() {
                    let y = cb.buf[cb.i];
                    cb.lp = y * (1.0 - self.damp) + cb.lp * self.damp;
                    cb.buf[cb.i] = inp + cb.lp * self.size;
                    cb.i = (cb.i + 1) % cb.buf.len();
                    acc += y;
                }
                for ap in self.aps[c].iter_mut() {
                    let y = ap.buf[ap.i];
                    let z = acc + y * 0.5;
                    ap.buf[ap.i] = z;
                    acc = y - z * 0.5;
                    ap.i = (ap.i + 1) % ap.buf.len();
                }
                fr[c] += acc * self.mix;
            }
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        match n {
            "mix" => self.mix = v,
            "size" => self.size = 0.5 + v * 0.48,
            "damp" => self.damp = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Reverb"
    }
}

pub struct Compressor {
    env: f32,
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack: f32,
    pub release: f32,
    pub makeup_db: f32,
    sr: f32,
    sc: Option<f32>,
}
impl Compressor {
    pub fn new(sr: f32) -> Self {
        Compressor { env: 0.0, threshold_db: -18.0, ratio: 4.0, attack: 0.005, release: 0.12, makeup_db: 3.0, sr, sc: None }
    }
}
impl Effect for Compressor {
    fn process(&mut self, b: &mut [f32]) {
        let ka = (-1.0 / (self.attack * self.sr)).exp();
        let kr = (-1.0 / (self.release * self.sr)).exp();
        let mk = 10f32.powf(self.makeup_db / 20.0);
        for fr in b.chunks_exact_mut(2) {
            let lvl = self.sc.unwrap_or_else(|| fr[0].abs().max(fr[1].abs()));
            let k = if lvl > self.env { ka } else { kr };
            self.env = lvl + (self.env - lvl) * k;
            let db = 20.0 * self.env.max(1e-6).log10();
            let over = (db - self.threshold_db).max(0.0);
            let g = 10f32.powf(-over * (1.0 - 1.0 / self.ratio) / 20.0) * mk;
            fr[0] *= g;
            fr[1] *= g;
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        match n {
            "threshold" => self.threshold_db = -60.0 + v * 60.0,
            "ratio" => self.ratio = 1.0 + v * 19.0,
            "attack" => self.attack = (1.0 + v * 199.0) / 1000.0,
            "release" => self.release = (10.0 + v * 990.0) / 1000.0,
            "makeup" => self.makeup_db = -12.0 + v * 24.0,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Compressor"
    }
    fn set_sidechain(&mut self, level: Option<f32>) {
        self.sc = level;
    }
}

// ── Ducker: a sidechain-style volume dip triggered by the notes of another track (or by a sidechain audio key) ─────────────────────

/// The volume dips by `depth` dB the moment a trigger arrives, holds, and comes back along a curve. Triggers are the notes of the tracks routed
/// into it with a Duck route (sample-accurate, and only the notes the route lets through, such as the kick of a drum kit) or the rising edges
/// of an audio sidechain key. A new trigger in the middle of a recovery starts from where the volume is, so retriggers never jump.
pub struct Ducker {
    sr: f32,
    pub depth_db: f32,
    pub attack: f32,
    pub hold: f32,
    pub release: f32,
    pub curve: f32,
    pub thresh: f32,
    pub mix: f32,
    /// seconds since the last trigger
    t: f32,
    g_from: f32,
    g_min: f32,
    active: bool,
    events: Vec<(u32, f32)>,
    sc_over: bool,
    sc_edge: bool,
    gain: f32,
}

impl Ducker {
    pub fn new(sr: f32) -> Self {
        Ducker { sr, depth_db: 13.5, attack: 0.004, hold: 0.03, release: 0.26, curve: 1.6, thresh: 0.1, mix: 1.0, t: 0.0, g_from: 1.0, g_min: 1.0, active: false, events: vec![], sc_over: false, sc_edge: false, gain: 1.0 }
    }

    fn trigger(&mut self, vel: f32) {
        // a harder note ducks a little deeper
        let d = self.depth_db * (0.55 + 0.45 * vel.clamp(0.0, 1.0));
        self.g_from = self.gain;
        self.g_min = 10f32.powf(-d / 20.0).min(self.g_from);
        self.t = 0.0;
        self.active = true;
    }

    /// the gain `t` seconds after a trigger
    fn gain_at(&self, t: f32) -> f32 {
        if t < self.attack {
            let u = (t / self.attack.max(1e-4)).clamp(0.0, 1.0);
            let e = u * u * (3.0 - 2.0 * u);
            return self.g_from + (self.g_min - self.g_from) * e;
        }
        let t2 = t - self.attack;
        if t2 < self.hold {
            return self.g_min;
        }
        let u = ((t2 - self.hold) / self.release.max(1e-3)).clamp(0.0, 1.0);
        self.g_min + (1.0 - self.g_min) * u.powf(self.curve)
    }

    /// the whole envelope of one trigger from silence: (seconds, gain) points for the display
    pub fn shape(&self, n: usize) -> Vec<f32> {
        let span = self.attack + self.hold + self.release;
        let mut d = Ducker { gain: 1.0, g_from: 1.0, ..Ducker::new(self.sr) };
        d.depth_db = self.depth_db;
        d.attack = self.attack;
        d.hold = self.hold;
        d.release = self.release;
        d.curve = self.curve;
        d.g_min = 10f32.powf(-self.depth_db / 20.0);
        (0..n.max(2)).map(|i| d.gain_at(span * 1.15 * i as f32 / (n.max(2) - 1) as f32)).collect()
    }
}

impl Effect for Ducker {
    fn process(&mut self, b: &mut [f32]) {
        let frames = b.len() / 2;
        if self.events.is_empty() && !self.sc_edge && !self.active {
            self.gain = 1.0;
            return;
        }
        let mut events = std::mem::take(&mut self.events);
        events.sort_by_key(|e| e.0);
        let mut ei = 0;
        let dt = 1.0 / self.sr;
        if self.sc_edge {
            self.trigger(1.0);
            self.sc_edge = false;
        }
        for f in 0..frames {
            while ei < events.len() && events[ei].0 as usize <= f {
                self.trigger(events[ei].1);
                ei += 1;
            }
            let g = if self.active {
                let g = self.gain_at(self.t);
                self.t += dt;
                if self.t > self.attack + self.hold + self.release {
                    self.active = false;
                }
                g
            } else {
                1.0
            };
            self.gain = g;
            let gm = 1.0 + (g - 1.0) * self.mix;
            b[f * 2] *= gm;
            b[f * 2 + 1] *= gm;
        }
        events.clear();
        self.events = events;
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "depth" => self.depth_db = v * 30.0,
            "attack" => self.attack = (0.0005 + v * v * 0.04).max(0.0005),
            "hold" => self.hold = v * 0.2,
            "release" => self.release = 0.03 + v * v * 1.17,
            "curve" => self.curve = 0.4 + v * 3.6,
            "thresh" => self.thresh = 10f32.powf((-50.0 + v * 44.0) / 20.0),
            "mix" => self.mix = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Ducker"
    }
    fn set_sidechain(&mut self, level: Option<f32>) {
        if let Some(l) = level {
            let over = l > self.thresh;
            if over && !self.sc_over {
                self.sc_edge = true;
            }
            self.sc_over = over;
        } else {
            self.sc_over = false;
        }
    }
    fn set_triggers(&mut self, events: &[(u32, f32)]) {
        self.events.clear();
        self.events.extend_from_slice(events);
    }
    fn view(&self) -> Vec<f32> {
        // the dip, drawn as 0..1 (1 = full volume)
        self.shape(64)
    }
}

/// 3-band EQ via one-pole crossovers at 250 Hz / 3.5 kHz.
pub struct Eq3 {
    lo: [f32; 2],
    hi: [f32; 2],
    pub low_db: f32,
    pub mid_db: f32,
    pub high_db: f32,
    a_lo: f32,
    a_hi: f32,
}
impl Eq3 {
    pub fn new(sr: f32) -> Self {
        let a = |f: f32| 1.0 - (-TAU * f / sr).exp();
        Eq3 { lo: [0.0; 2], hi: [0.0; 2], low_db: 0.0, mid_db: 0.0, high_db: 0.0, a_lo: a(250.0), a_hi: a(3500.0) }
    }
}
impl Effect for Eq3 {
    fn process(&mut self, b: &mut [f32]) {
        let g = |db: f32| 10f32.powf(db / 20.0);
        let (gl, gm, gh) = (g(self.low_db), g(self.mid_db), g(self.high_db));
        for fr in b.chunks_exact_mut(2) {
            for c in 0..2 {
                let x = fr[c];
                self.lo[c] += self.a_lo * (x - self.lo[c]);
                self.hi[c] += self.a_hi * (x - self.hi[c]);
                let low = self.lo[c];
                let high = x - self.hi[c];
                let mid = x - low - high;
                fr[c] = low * gl + mid * gm + high * gh;
            }
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let db = (v - 0.5) * 24.0;
        match n {
            "low" => self.low_db = db,
            "mid" => self.mid_db = db,
            "high" => self.high_db = db,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "EQ"
    }
}

pub struct Drive {
    pub amount: f32,
    pub mix: f32,
}
impl Drive {
    pub fn new() -> Self {
        Drive { amount: 0.3, mix: 1.0 }
    }
}
impl Default for Drive {
    fn default() -> Self {
        Self::new()
    }
}
impl Effect for Drive {
    fn process(&mut self, b: &mut [f32]) {
        let g = 1.0 + self.amount * 20.0;
        for s in b.iter_mut() {
            *s = *s * (1.0 - self.mix) + (*s * g).tanh() / g.sqrt() * self.mix;
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        match n {
            "amount" => self.amount = v,
            "mix" => self.mix = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Drive"
    }
}

/// Two-voice modulated-delay chorus (ensemble strings, organs, clean guitars, pads).
pub struct Chorus {
    buf: [Vec<f32>; 2],
    pos: usize,
    ph: f32,
    rate: f32,
    depth: f32,
    mix: f32,
    sr: f32,
}
impl Chorus {
    pub fn new(sr: f32) -> Self {
        let n = (sr * 0.06) as usize + 8;
        Chorus { buf: [vec![0.0; n], vec![0.0; n]], pos: 0, ph: 0.0, rate: 0.8, depth: 0.5, mix: 0.5, sr }
    }
}
impl Effect for Chorus {
    fn process(&mut self, b: &mut [f32]) {
        let n = self.buf[0].len();
        let dt = self.rate / self.sr;
        for fr in b.chunks_exact_mut(2) {
            self.ph = (self.ph + dt).fract();
            for c in 0..2 {
                self.buf[c][self.pos] = fr[c];
                // the two channels use opposite LFO phases for stereo width
                let ph = if c == 0 { self.ph } else { (self.ph + 0.5).fract() };
                let lfo = (ph * TAU).sin();
                let d = (0.012 + lfo * self.depth * 0.006) * self.sr; // 12 ms +- 6 ms
                let rp = self.pos as f32 + n as f32 - d;
                let i0 = rp.floor();
                let f = rp - i0;
                let i0 = i0 as usize % n;
                let i1 = (i0 + 1) % n;
                let wet = self.buf[c][i0] * (1.0 - f) + self.buf[c][i1] * f;
                fr[c] = fr[c] * (1.0 - self.mix * 0.5) + wet * self.mix * 0.8;
            }
            self.pos = (self.pos + 1) % n;
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        match n {
            "rate" => self.rate = 0.1 + v * 4.0,
            "depth" => self.depth = v,
            "mix" => self.mix = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Chorus"
    }
}

pub const EFFECT_NAMES: [&str; 13] = ["EQ", "Compressor", "Drive", "Delay", "Reverb", "Chorus", "Vox Filter", "Vox Reverb", "Vocal Halo", "Rainbow Hypergrowl", "Ducker", "Vocoder", "Ringstorm"];

/// Parameter names + default normalised values shown in the UI.
pub fn defaults(name: &str) -> &'static [(&'static str, f32)] {
    match name {
        "EQ" => &[("low", 0.5), ("mid", 0.5), ("high", 0.5)],
        "Compressor" => &[("threshold", 0.7), ("ratio", 0.2), ("attack", 0.02), ("release", 0.11), ("makeup", 0.625)],
        "Drive" => &[("amount", 0.3), ("mix", 1.0)],
        "Delay" => &[("time", 0.2), ("feedback", 0.4), ("mix", 0.3)],
        "Reverb" => &[("mix", 0.25), ("size", 0.6), ("damp", 0.4)],
        "Chorus" => &[("rate", 0.25), ("depth", 0.5), ("mix", 0.5)],
        "Vox Filter" => &[("hp", 0.4), ("slope", 0.5), ("lp", 1.0), ("res", 0.1), ("tilt", 0.5), ("dyn", 0.3), ("mix", 1.0)],
        "Vox Reverb" => &[("mix", 0.25), ("decay", 0.4), ("predelay", 0.1), ("size", 0.5), ("damp", 0.5), ("lowcut", 0.45), ("highcut", 0.75), ("duck", 0.4), ("width", 0.8), ("early", 0.4), ("mod", 0.3)],
        "Vocal Halo" => &[("width", 0.55), ("halo", 0.4), ("double", 0.3), ("presence", 0.35), ("air", 0.3), ("deess", 0.3), ("punch", 0.25), ("bloom", 0.2), ("focus", 0.5), ("mix", 1.0)],
        "Rainbow Hypergrowl" => &super::hypergrowl::PARAMS,
        "Vocoder" => &super::vocoder::PARAMS,
        "Ringstorm" => &super::guitar::PARAMS,
        "Ducker" => &[("depth", 0.45), ("attack", 0.08), ("hold", 0.15), ("release", 0.2), ("curve", 0.35), ("thresh", 0.5), ("mix", 1.0)],
        _ => &[],
    }
}

pub fn make(name: &str, sr: f32) -> Option<Box<dyn Effect>> {
    let mut e: Box<dyn Effect> = match name {
        "Delay" => Box::new(Delay::new(sr)),
        "Reverb" => Box::new(Reverb::new(sr)),
        "Compressor" => Box::new(Compressor::new(sr)),
        "EQ" => Box::new(Eq3::new(sr)),
        "Drive" => Box::new(Drive::new()),
        "Chorus" => Box::new(Chorus::new(sr)),
        "Vox Filter" => Box::new(super::vox::VoxFilter::new(sr)),
        "Vox Reverb" => Box::new(super::vox::VoxReverb::new(sr)),
        "Vocal Halo" => Box::new(super::vox::VocalHalo::new(sr)),
        "Rainbow Hypergrowl" => Box::new(super::hypergrowl::Hypergrowl::new(sr)),
        "Ducker" => Box::new(Ducker::new(sr)),
        "Vocoder" => Box::new(super::vocoder::Vocoder::new(sr)),
        "Ringstorm" => Box::new(super::guitar::Rig::new(sr)),
        _ => return None,
    };
    for (n, v) in defaults(name) {
        e.set_param(n, *v);
    }
    Some(e)
}

/// Ready-made settings of an effect: (name, overrides on top of the defaults). Only some effects have any.
pub fn presets(name: &str) -> &'static [(&'static str, &'static [(&'static str, f32)])] {
    match name {
        "Rainbow Hypergrowl" => &super::hypergrowl::PRESETS,
        "Vocoder" => &super::vocoder::PRESETS,
        "Ringstorm" => &super::guitar::PRESETS,
        "Ducker" => &[
            ("Kick duck", &[("depth", 0.5), ("attack", 0.06), ("hold", 0.12), ("release", 0.2), ("curve", 0.35)]),
            ("Hard pump", &[("depth", 0.85), ("attack", 0.04), ("hold", 0.2), ("release", 0.32), ("curve", 0.2)]),
            ("Gentle breath", &[("depth", 0.25), ("attack", 0.2), ("hold", 0.0), ("release", 0.45), ("curve", 0.5)]),
            ("Tight gate", &[("depth", 1.0), ("attack", 0.02), ("hold", 0.3), ("release", 0.12), ("curve", 0.7)]),
        ],
        _ => &[],
    }
}

#[cfg(test)]
mod ducker_tests {
    use super::*;

    const SR: f32 = 48000.0;

    fn run(d: &mut Ducker, frames: usize, events: &[(u32, f32)]) -> Vec<f32> {
        let mut b = vec![1.0f32; frames * 2];
        d.set_triggers(events);
        d.process(&mut b);
        b.chunks(2).map(|f| f[0]).collect()
    }

    #[test]
    fn without_triggers_the_signal_is_untouched() {
        let mut d = Ducker::new(SR);
        let mut b: Vec<f32> = (0..2000).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        let orig = b.clone();
        d.process(&mut b);
        assert_eq!(b, orig);
    }

    #[test]
    fn a_trigger_dips_holds_and_comes_back() {
        let mut d = Ducker::new(SR);
        for (n, v) in defaults("Ducker") {
            d.set_param(n, *v);
        }
        d.set_param("depth", 0.5); // 15 dB
        let g = run(&mut d, SR as usize, &[(1000, 1.0)]);
        assert!((g[999] - 1.0).abs() < 1e-6, "nothing before the trigger");
        let (a, h, r) = (d.attack, d.hold, d.release);
        let at = |t: f32| g[1000 + (t * SR) as usize];
        let floor = 10f32.powf(-15.0 / 20.0);
        assert!((at(a + 0.002) - floor).abs() < 0.01, "the dip reaches 15 dB: {} vs {}", at(a + 0.002), floor);
        assert!((at(a + h * 0.5) - floor).abs() < 0.01, "and holds");
        assert!(at(a + h + r * 0.5) > floor + 0.1 && at(a + h + r * 0.5) < 0.95, "mid-recovery is between");
        assert!(at(a + h + r + 0.01) > 0.999, "and it is fully back");
        // the recovery only rises
        let mut last = 0.0;
        for k in 0..200 {
            let v = at(a + h + r * k as f32 / 200.0);
            assert!(v >= last - 1e-5);
            last = v;
        }
    }

    #[test]
    fn a_harder_note_ducks_deeper_and_depth_runs_to_30_db() {
        let dip = |vel: f32, depth: f32| {
            let mut d = Ducker::new(SR);
            d.set_param("depth", depth);
            let g = run(&mut d, 4800, &[(10, vel)]);
            g.iter().cloned().fold(1.0f32, f32::min)
        };
        assert!(dip(1.0, 0.5) < dip(0.3, 0.5));
        let deepest = dip(1.0, 1.0);
        assert!((deepest - 10f32.powf(-30.0 / 20.0)).abs() < 0.005, "{deepest}");
    }

    #[test]
    fn a_retrigger_during_the_recovery_starts_from_where_the_volume_is() {
        let mut d = Ducker::new(SR);
        let g = run(&mut d, 20000, &[(0, 1.0), (7000, 1.0)]);
        for w in g.windows(2) {
            assert!((w[1] - w[0]).abs() < 0.02, "a jump of {} in the volume", (w[1] - w[0]).abs());
        }
        // the second dip starts from a partly recovered volume, not from 1.0
        assert!(g[7000] < 0.95);
    }

    #[test]
    fn the_dip_carries_over_into_the_next_block() {
        let mut one = Ducker::new(SR);
        let whole = run(&mut one, 6000, &[(100, 1.0)]);
        let mut two = Ducker::new(SR);
        let mut parts = run(&mut two, 2000, &[(100, 1.0)]);
        parts.extend(run(&mut two, 4000, &[]));
        for (a, b) in whole.iter().zip(&parts) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn an_audio_key_triggers_on_the_rising_edge_only() {
        let mut d = Ducker::new(SR);
        d.set_param("thresh", 0.5);
        d.set_sidechain(Some(0.5));
        let g1 = run(&mut d, 3000, &[]);
        assert!(g1[2000] < 0.9, "the key above the threshold ducks");
        // still above: no second trigger, the volume keeps recovering
        d.set_sidechain(Some(0.5));
        let g2 = run(&mut d, 20000, &[]);
        assert!(g2[19999] > 0.99);
        // below, then above again: ducks again
        d.set_sidechain(Some(0.0));
        run(&mut d, 100, &[]);
        d.set_sidechain(Some(0.5));
        let g3 = run(&mut d, 3000, &[]);
        assert!(g3[2000] < 0.9);
    }

    #[test]
    fn the_shape_for_the_display_matches_the_envelope() {
        let mut d = Ducker::new(SR);
        d.set_param("depth", 0.4);
        let s = d.shape(64);
        assert_eq!(s.len(), 64);
        assert!(s[0] > 0.99 && s.iter().cloned().fold(1.0, f32::min) < 0.3 && s[63] > 0.999);
        assert_eq!(d.view().len(), 64);
        assert_eq!(super::defaults("Ducker").len(), 7);
        assert!(super::make("Ducker", SR).is_some());
    }
}
