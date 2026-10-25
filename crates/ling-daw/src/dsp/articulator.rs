//! **Articulator** - a filter, a gate and an articulation shaper in one box.
//!
//! * the **filter**: a state-variable filter (low / high / band / notch / 24 dB low-pass) with drive in front of it. Its cutoff is moved by
//!   an envelope follower (auto-wah up or down), a tempo-synced LFO, and the gate steps themselves (each step "plucks" the filter open).
//! * the **gate**: a 16-step pattern (trance gate, offbeats, gallop, stutter ...) read in steps of 1/4 .. 1/32 or triplets, with swing,
//!   per-step accents, a length (staccato to legato) and a smoothing time so it never clicks. It follows the transport, so it stays
//!   locked to the song; stopped, it keeps its own time at the project tempo.
//! * the **articulation**: every step opens with a short "punch" (a quick gain and brightness pop that decays) - the pick, the bow,
//!   the tongue - so pads and chords can be played staccato, pumped or plucked without touching the notes.
//!
//! Parameters are 0..1 like every effect's. The effect-level dry/wet that every effect has (`fx<i>.wet`) is the mix, so there is no knob
//! for it here. Output is finite for any input and settings (tests below).
use super::bassfx::Tsvf;
use super::fx::Effect;
use crate::dsp::TAU;

pub const PARAMS: [(&str, f32); 18] = [
    ("mode", 0.0),
    ("cutoff", 0.62),
    ("res", 0.3),
    ("drive", 0.12),
    ("env", 0.5),
    ("follow", 0.4),
    ("lfo", 0.0),
    ("rate", 0.55),
    ("shape", 0.0),
    ("gate", 0.8),
    ("pattern", 0.2727),
    ("steps", 0.75),
    ("length", 0.6),
    ("smooth", 0.25),
    ("swing", 0.0),
    ("accent", 0.5),
    ("punch", 0.3),
    ("pluck", 0.0),
];

/// the 16-step patterns: 0 = closed, 1 = open at full level, in between = an accented step
pub const PATTERNS: [(&str, [f32; 16]); 12] = [
    ("Chop 16ths", [1.0; 16]),
    ("Eighths", [1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0]),
    ("Offbeat pump", [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
    ("Trance 3-3-2", [1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0]),
    ("Gallop", [1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, 1.0]),
    ("Stutter", [1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0]),
    ("Syncopated", [1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]),
    ("Accents", [1.0, 0.45, 0.7, 0.45, 1.0, 0.45, 0.7, 0.45, 1.0, 0.45, 0.7, 0.45, 1.0, 0.45, 0.7, 0.45]),
    ("Triplet feel", [1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0]),
    ("Build", [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0]),
    ("Sparse", [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]),
    ("Half time", [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
];

/// LFO / step lengths in beats: 1/4, 1/8, 1/8 triplet, 1/16, 1/32
const STEP_BEATS: [f64; 5] = [1.0, 0.5, 1.0 / 3.0, 0.25, 0.125];
/// LFO periods in beats: 4 bars, 2 bars, 1 bar, 1/2 bar, 1/4, 1/8, 1/16, 1/32 note
const LFO_BEATS: [f64; 8] = [16.0, 8.0, 4.0, 2.0, 1.0, 0.5, 0.25, 0.125];

#[inline]
fn fin(x: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

pub struct Articulator {
    sr: f32,
    // parameters (already mapped)
    mode: usize,
    cutoff: f32,
    res: f32,
    drive: f32,
    env_amt: f32,
    follow: f32,
    lfo_amt: f32,
    lfo_idx: usize,
    shape: usize,
    gate: f32,
    pattern: usize,
    steps: usize,
    length: f32,
    smooth: f32,
    swing: f32,
    accent: f32,
    punch: f32,
    pluck: f32,
    // state
    f1: [Tsvf; 2],
    f2: [Tsvf; 2],
    env: f32,
    gate_env: f32,
    pop: f32,
    pluck_env: f32,
    last_step: i64,
    beat: f64,
    last_base: f64,
    bpm: f64,
    rng: u32,
    sh: f32,
    last_lfo_cycle: i64,
    // what the display shows
    shown: [f32; 5],
    shown_step: usize,
}

impl Articulator {
    pub fn new(sr: f32) -> Articulator {
        Articulator {
            sr,
            mode: 0,
            cutoff: 0.62,
            res: 0.3,
            drive: 0.12,
            env_amt: 0.0,
            follow: 0.4,
            lfo_amt: 0.0,
            lfo_idx: 4,
            shape: 0,
            gate: 0.8,
            pattern: 3,
            steps: 3,
            length: 0.6,
            smooth: 0.25,
            swing: 0.0,
            accent: 0.5,
            punch: 0.3,
            pluck: 0.0,
            f1: [Tsvf::default(); 2],
            f2: [Tsvf::default(); 2],
            env: 0.0,
            gate_env: 1.0,
            pop: 0.0,
            pluck_env: 0.0,
            last_step: i64::MIN,
            beat: 0.0,
            last_base: f64::NAN,
            bpm: 120.0,
            rng: 0x1234_5678,
            sh: 0.0,
            last_lfo_cycle: i64::MIN,
            shown: [0.0; 5],
            shown_step: 0,
        }
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// the LFO at a phase 0..1
    fn lfo(&mut self, ph: f64, cycle: i64) -> f32 {
        let p = ph as f32;
        match self.shape {
            0 => (p * TAU).sin(),
            1 => 1.0 - 4.0 * (p - 0.5).abs(),
            2 => 1.0 - 2.0 * p,
            3 => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            _ => {
                if cycle != self.last_lfo_cycle {
                    self.last_lfo_cycle = cycle;
                    self.sh = self.rand();
                }
                self.sh
            }
        }
    }
}

impl Effect for Articulator {
    fn set_transport(&mut self, bpm: f64, beat: f64) {
        if bpm.is_finite() && bpm > 20.0 {
            self.bpm = bpm;
        }
        if !beat.is_finite() {
            return;
        }
        // stopped (the position does not move between blocks): keep time on our own so the effect still works on a live input
        let moved = self.last_base.is_nan() || (beat - self.last_base).abs() > 1e-9;
        self.last_base = beat;
        if moved {
            self.beat = beat;
        }
    }

    fn process(&mut self, buf: &mut [f32]) {
        let sr = self.sr;
        let bps = self.bpm / 60.0 / sr as f64;
        let step_beats = STEP_BEATS[self.steps.min(4)];
        let pat = &PATTERNS[self.pattern.min(PATTERNS.len() - 1)].1;
        let len = 0.08 + self.length * 0.92;
        // gate smoothing: opening a little quicker than closing
        let t_open = 0.0008 + self.smooth * 0.03;
        let t_close = 0.0015 + self.smooth * 0.09;
        let k_open = (-1.0 / (t_open * sr)).exp();
        let k_close = (-1.0 / (t_close * sr)).exp();
        let k_pop = (-1.0 / (0.06 * sr)).exp();
        let k_pluck = (-1.0 / ((0.04 + 0.4 * len) * sr)).exp();
        let t_env = 0.002 + (1.0 - self.follow) * 0.25;
        let k_env = (-1.0 / (t_env * sr)).exp();
        let outg = 1.0;
        let drive = 1.0 + self.drive * 9.0;
        let dcomp = 1.0 / drive.tanh().max(0.2);
        let q = 1.0 - self.res * 0.97;
        let k = (2.0 * q).max(0.04);
        let lfo_period = LFO_BEATS[self.lfo_idx.min(7)];
        for fr in buf.chunks_exact_mut(2) {
            let b = self.beat;
            self.beat += bps;
            // which step, and where in it (with swing: the odd steps start later)
            let sf = b / step_beats;
            let pair = sf / 2.0;
            let pk = pair.floor();
            let pf = pair - pk;
            let boundary = 0.5 + 0.25 * self.swing as f64;
            let (step, frac) = if pf < boundary { ((pk as i64) * 2, pf / boundary) } else { ((pk as i64) * 2 + 1, (pf - boundary) / (1.0 - boundary)) };
            let lvl = pat[(step.rem_euclid(16)) as usize];
            let open = lvl > 0.0 && (frac as f32) < len;
            // a new step that opens: the pick (punch + pluck) fires
            if step != self.last_step {
                self.last_step = step;
                if lvl > 0.0 {
                    self.pop = lvl;
                    self.pluck_env = lvl;
                }
                self.shown_step = (step.rem_euclid(16)) as usize;
            }
            let accent_gain = (1.0 - self.accent) + self.accent * lvl;
            let target = if open { accent_gain } else { 0.0 };
            let kk = if target > self.gate_env { k_open } else { k_close };
            self.gate_env = target + (self.gate_env - target) * kk;
            // the gate's depth: 0 = open all the time
            let g = 1.0 - self.gate * (1.0 - self.gate_env.clamp(0.0, 1.0));
            self.pop *= k_pop;
            self.pluck_env *= k_pluck;
            let punch_gain = 1.0 + self.punch * 0.9 * self.pop;

            let l = fin(fr[0]);
            let r = fin(fr[1]);
            // envelope follower of the input
            let lvl_in = l.abs().max(r.abs());
            self.env = lvl_in + (self.env - lvl_in) * k_env;
            // cutoff in octaves around the knob: 30 Hz .. ~21 kHz
            let base = 30.0 * (2.0f32).powf(self.cutoff * 9.5);
            let cyc = (b / lfo_period).floor();
            let ph = b / lfo_period - cyc;
            let lfo = self.lfo(ph, cyc as i64);
            let mut oct = self.env_amt * (self.env * 2.5).min(1.0) * 5.0 + self.lfo_amt * lfo * 3.0 + self.pluck * self.pluck_env * 4.0 + self.punch * self.pop * 1.5;
            if !oct.is_finite() {
                oct = 0.0;
            }
            let fc = (base * (2.0f32).powf(oct)).clamp(20.0, sr * 0.45);
            let gg = (std::f32::consts::PI * fc / sr).tan();
            let mut out = [0.0f32; 2];
            for (c, x0) in [l, r].into_iter().enumerate() {
                let x = (x0 * drive).tanh() * dcomp;
                let (lo, bp, hi) = self.f1[c].run(x, gg, k);
                let y = match self.mode {
                    0 => lo,
                    1 => hi,
                    2 => bp * (k.max(0.3)),
                    3 => lo + hi,
                    _ => {
                        let (lo2, _, _) = self.f2[c].run(lo, gg, k);
                        lo2
                    }
                };
                out[c] = fin(y) * g * punch_gain * outg;
            }
            fr[0] = fin(out[0]);
            fr[1] = fin(out[1]);
            self.shown = [fc.max(20.0).log2().min(15.0) / 15.0, self.gate_env.clamp(0.0, 1.0), (lfo * 0.5 + 0.5).clamp(0.0, 1.0), (self.env * 2.0).min(1.0), self.pop.min(1.0)];
        }
    }

    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "mode" => self.mode = ((v * 4.0).round() as usize).min(4),
            "cutoff" => self.cutoff = v,
            "res" => self.res = v,
            "drive" => self.drive = v,
            "env" => self.env_amt = (v - 0.5) * 2.0,
            "follow" => self.follow = v,
            "lfo" => self.lfo_amt = v,
            "rate" => self.lfo_idx = ((v * 7.0).round() as usize).min(7),
            "shape" => self.shape = ((v * 4.0).round() as usize).min(4),
            "gate" => self.gate = v,
            "pattern" => self.pattern = ((v * (PATTERNS.len() - 1) as f32).round() as usize).min(PATTERNS.len() - 1),
            "steps" => self.steps = ((v * 4.0).round() as usize).min(4),
            "length" => self.length = v,
            "smooth" => self.smooth = v,
            "swing" => self.swing = v,
            "accent" => self.accent = v,
            "punch" => self.punch = v,
            "pluck" => self.pluck = v,
            _ => {}
        }
    }

    fn name(&self) -> &'static str {
        "Articulator"
    }

    /// [cutoff 0..1 (log), gate open 0..1, LFO 0..1, input envelope 0..1, punch 0..1, current step 0..15 / 16, then the 16 steps of the pattern]
    fn view(&self) -> Vec<f32> {
        let mut v = self.shown.to_vec();
        v.push(self.shown_step as f32 / 16.0);
        v.extend(PATTERNS[self.pattern.min(PATTERNS.len() - 1)].1.iter().copied());
        v
    }

    fn info(&self, what: &str) -> String {
        match what {
            "pattern" => PATTERNS[self.pattern.min(PATTERNS.len() - 1)].0.to_string(),
            "patterns" => PATTERNS.iter().map(|p| p.0).collect::<Vec<_>>().join("|"),
            _ => String::new(),
        }
    }
}

type Pre = (&'static str, &'static [(&'static str, f32)]);
pub const PRESETS: [Pre; 8] = [
    ("Trance gate", &[("mode", 0.0), ("cutoff", 0.8), ("gate", 0.95), ("pattern", 0.2727), ("steps", 0.75), ("length", 0.55), ("smooth", 0.15), ("punch", 0.4)]),
    ("Pluck every step", &[("mode", 0.0), ("cutoff", 0.35), ("res", 0.45), ("gate", 0.0), ("pattern", 0.0), ("steps", 0.25), ("pluck", 0.8), ("punch", 0.2)]),
    ("Offbeat pump", &[("mode", 0.0), ("cutoff", 0.9), ("gate", 0.85), ("pattern", 0.1818), ("steps", 0.75), ("length", 0.45), ("smooth", 0.3), ("punch", 0.5)]),
    ("Auto-wah", &[("mode", 0.5), ("cutoff", 0.4), ("res", 0.55), ("env", 0.9), ("follow", 0.55), ("gate", 0.0), ("drive", 0.25)]),
    ("Slow sweep", &[("mode", 0.25), ("cutoff", 0.55), ("res", 0.4), ("lfo", 0.8), ("rate", 0.0), ("shape", 0.25), ("gate", 0.0)]),
    ("Staccato pads", &[("mode", 0.0), ("cutoff", 0.9), ("gate", 1.0), ("pattern", 0.0), ("steps", 0.25), ("length", 0.28), ("smooth", 0.2), ("punch", 0.5)]),
    ("Gallop gate", &[("mode", 0.0), ("cutoff", 0.85), ("gate", 0.9), ("pattern", 0.3636), ("steps", 0.75), ("length", 0.6), ("swing", 0.15)]),
    ("Telephone", &[("mode", 0.5), ("cutoff", 0.58), ("res", 0.5), ("drive", 0.6), ("gate", 0.0)]),
];

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;

    fn run(a: &mut Articulator, input: impl Fn(usize) -> f32, frames: usize, beat0: f64) -> Vec<f32> {
        let mut buf = vec![0.0f32; frames * 2];
        for i in 0..frames {
            let x = input(i);
            buf[i * 2] = x;
            buf[i * 2 + 1] = x;
        }
        a.set_transport(120.0, beat0);
        a.process(&mut buf);
        buf
    }

    fn set(a: &mut Articulator, kv: &[(&str, f32)]) {
        for (n, v) in kv {
            a.set_param(n, *v);
        }
    }

    #[test]
    fn finite_and_bounded_for_any_setting() {
        for seed in 0..40u32 {
            let mut a = Articulator::new(SR);
            let mut x = seed.wrapping_mul(2654435761) | 1;
            for (n, _) in PARAMS {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                a.set_param(n, (x % 1001) as f32 / 1000.0);
            }
            let y = run(&mut a, |i| ((i as f32 * 0.05).sin() * 0.9) + if i % 997 == 0 { 3.0 } else { 0.0 }, 20000, seed as f64 * 0.37);
            assert!(y.iter().all(|v| v.is_finite() && v.abs() < 60.0), "seed {seed}");
        }
        let mut a = Articulator::new(SR);
        let y = run(&mut a, |_| f32::NAN, 2000, 0.0);
        assert!(y.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn the_gate_follows_its_pattern() {
        // quarter-note steps at 120 bpm = 24000 samples each; "Half time" opens step 0 and 8 of 16 only
        let mut a = Articulator::new(SR);
        set(&mut a, &[("mode", 0.25), ("cutoff", 0.0), ("gate", 1.0), ("pattern", 1.0), ("steps", 0.0), ("length", 1.0), ("smooth", 0.0), ("drive", 0.0), ("punch", 0.0), ("res", 0.0)]);
        // a high-pass at 30 Hz lets a 1 kHz tone through; on a closed step the gate shuts it
        let tone = |i: usize| (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / SR).sin() * 0.5;
        let y = run(&mut a, tone, 24000 * 3, 0.0);
        let rms = |lo: usize, hi: usize| (y[lo * 2..hi * 2].iter().map(|v| v * v).sum::<f32>() / (2 * (hi - lo)) as f32).sqrt();
        assert!(rms(3000, 20000) > 0.2, "step 0 is open: {}", rms(3000, 20000));
        assert!(rms(30000, 46000) < 0.01, "step 1 is closed: {}", rms(30000, 46000));
    }

    #[test]
    fn the_filter_removes_the_highs() {
        let mut a = Articulator::new(SR);
        set(&mut a, &[("mode", 0.0), ("cutoff", 0.25), ("gate", 0.0), ("drive", 0.0), ("punch", 0.0), ("res", 0.0)]);
        let hi = |i: usize| (i as f32 * 2.0 * std::f32::consts::PI * 8000.0 / SR).sin() * 0.5;
        let lo = |i: usize| (i as f32 * 2.0 * std::f32::consts::PI * 80.0 / SR).sin() * 0.5;
        let yh = run(&mut a, hi, 9600, 0.0);
        let mut b = Articulator::new(SR);
        set(&mut b, &[("mode", 0.0), ("cutoff", 0.25), ("gate", 0.0), ("drive", 0.0), ("punch", 0.0), ("res", 0.0)]);
        let yl = run(&mut b, lo, 9600, 0.0);
        let rms = |y: &[f32]| (y[9600..].iter().map(|v| v * v).sum::<f32>() / (y.len() - 9600) as f32).sqrt();
        assert!(rms(&yh) < rms(&yl) * 0.2, "8 kHz {} vs 80 Hz {}", rms(&yh), rms(&yl));
    }

    #[test]
    fn it_keeps_time_when_stopped() {
        let mut a = Articulator::new(SR);
        set(&mut a, &[("gate", 1.0), ("pattern", 0.0909), ("steps", 0.5), ("length", 0.5)]);
        let tone = |i: usize| (i as f32 * 0.07).sin() * 0.5;
        // the same beat position block after block (a stopped transport): the gate still opens and closes
        let mut energy = vec![];
        for _ in 0..8 {
            let y = run(&mut a, tone, 6000, 4.0);
            energy.push(y.iter().map(|v| v * v).sum::<f32>());
        }
        let (mn, mx) = (energy.iter().cloned().fold(f32::MAX, f32::min), energy.iter().cloned().fold(0.0f32, f32::max));
        assert!(mx > mn * 3.0 || mn < 1e-3, "{energy:?}");
    }

    #[test]
    fn presets_and_names_are_valid() {
        let names: Vec<&str> = PARAMS.iter().map(|p| p.0).collect();
        for (pn, kv) in PRESETS {
            for (n, v) in kv {
                assert!(names.contains(n), "{pn}: {n}");
                assert!((0.0..=1.0).contains(v));
            }
        }
    }

    #[test]
    fn the_slot_blends_dry_and_wet() {
        use crate::model::EffectSlot;
        // a gate that is shut on the second step: wet 1 = silence, wet 0.5 = half the input, wet 0 = the input
        let mut slot = EffectSlot::new("Articulator", SR).unwrap();
        for (n, v) in [("mode", 0.25f32), ("cutoff", 0.0), ("gate", 1.0), ("pattern", 1.0), ("steps", 0.0), ("length", 1.0), ("smooth", 0.0), ("drive", 0.0), ("punch", 0.0), ("res", 0.0)] {
            let i = slot.params.iter().position(|p| p.0 == n).unwrap();
            slot.set(i, v);
        }
        let mk = || (0..48000usize * 2).map(|i| ((i / 2) as f32 * 0.14).sin() * 0.5).collect::<Vec<f32>>();
        let rms = |y: &[f32]| (y[60000..90000].iter().map(|v| v * v).sum::<f32>() / 30000.0).sqrt();
        let input = rms(&mk());
        slot.fx.set_transport(120.0, 0.0);
        let mut a = mk();
        slot.wet = 1.0;
        slot.process(&mut a);
        assert!(rms(&a) < 0.01, "wet 1: {}", rms(&a));
        let mut slot2 = EffectSlot::new("Articulator", SR).unwrap();
        for (n, v) in [("mode", 0.25f32), ("cutoff", 0.0), ("gate", 1.0), ("pattern", 1.0), ("steps", 0.0), ("length", 1.0), ("smooth", 0.0), ("drive", 0.0), ("punch", 0.0), ("res", 0.0)] {
            let i = slot2.params.iter().position(|p| p.0 == n).unwrap();
            slot2.set(i, v);
        }
        slot2.fx.set_transport(120.0, 0.0);
        slot2.wet = 0.5;
        let mut b = mk();
        slot2.process(&mut b);
        assert!((rms(&b) - input * 0.5).abs() < input * 0.05, "wet 0.5: {} of {}", rms(&b), input);
        let mut slot3 = EffectSlot::new("Articulator", SR).unwrap();
        slot3.wet = 0.0;
        let mut c = mk();
        slot3.process(&mut c);
        assert_eq!(c, mk());
    }
}
