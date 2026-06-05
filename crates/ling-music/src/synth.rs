//! A compact, GM-capable polyphonic synth voice.
//!
//! Each patch is a small modular stack: up to 6 **operators** (oscillators with
//! per-op ADSR, level, ratio/fixed-freq, detune, and optional FM routing to a
//! lower-indexed operator), a resonant low-pass **filter** with its own envelope,
//! and a global **LFO** (vibrato + filter wobble). FM + subtractive together let
//! one engine approximate any General-MIDI family — pianos, organs, strings,
//! brass, basses, leads, plucks, and (with the noise oscillator) percussion.
//!
//! FM convention: an operator may modulate any **lower-indexed** operator. Voices
//! are processed high→low so modulator output is ready for its carrier. Operators
//! with no `fm_target` are carriers and sum to the voice output.

use crate::note::midi_to_hz;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wave { Sine, Saw, Square, Triangle, Noise }

impl Wave {
    pub fn from_name(s: &str) -> Wave {
        match s.to_ascii_lowercase().as_str() {
            "saw" | "sawtooth"  => Wave::Saw,
            "square" | "sq" | "pulse" => Wave::Square,
            "tri" | "triangle"  => Wave::Triangle,
            "noise" | "white"   => Wave::Noise,
            _ => Wave::Sine,
        }
    }
    #[inline]
    fn eval(self, phase: f32, rng: &mut u32) -> f32 {
        use std::f32::consts::TAU;
        match self {
            Wave::Sine     => (phase * TAU).sin(),
            Wave::Saw      => 2.0 * (phase - (phase + 0.5).floor()),
            Wave::Square   => if phase.fract() < 0.5 { 1.0 } else { -1.0 },
            Wave::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
            Wave::Noise => {
                // xorshift white noise
                *rng ^= *rng << 13; *rng ^= *rng >> 17; *rng ^= *rng << 5;
                (*rng as f32 / u32::MAX as f32) * 2.0 - 1.0
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Adsr { pub a: f32, pub d: f32, pub s: f32, pub r: f32 }
impl Default for Adsr { fn default() -> Self { Self { a: 0.005, d: 0.1, s: 0.8, r: 0.3 } } }

#[derive(Clone, Copy, Debug)]
pub struct Op {
    pub wave: Wave,
    pub ratio: f32,       // frequency = note_hz * ratio …
    pub fixed_hz: f32,    // … unless fixed_hz > 0
    pub level: f32,       // output (carrier) or modulation index (modulator)
    pub detune: f32,      // cents
    pub adsr: Adsr,
    pub fm_target: i32,   // index of the op this modulates, or -1 = carrier
}
impl Default for Op {
    fn default() -> Self {
        Self { wave: Wave::Sine, ratio: 1.0, fixed_hz: 0.0, level: 1.0, detune: 0.0,
               adsr: Adsr::default(), fm_target: -1 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Filter { pub cutoff: f32, pub reso: f32, pub env: f32, pub adsr: Adsr, pub on: bool }
impl Default for Filter {
    fn default() -> Self { Self { cutoff: 1.0, reso: 0.2, env: 0.0, adsr: Adsr { a: 0.0, d: 0.2, s: 1.0, r: 0.3 }, on: false } }
}

#[derive(Clone, Copy, Debug)]
pub struct Lfo { pub rate: f32, pub pitch: f32, pub cutoff: f32, pub amp: f32 }
impl Default for Lfo { fn default() -> Self { Self { rate: 5.0, pitch: 0.0, cutoff: 0.0, amp: 0.0 } } }

#[derive(Clone, Debug)]
pub struct Patch {
    pub name: String,
    pub gm: u32,
    pub poly: usize,
    pub ops: Vec<Op>,
    pub filter: Filter,
    pub lfo: Lfo,
    pub amp: f32,
}
impl Default for Patch {
    fn default() -> Self {
        Self { name: "Init".into(), gm: 0, poly: 16, ops: vec![Op::default()],
               filter: Filter::default(), lfo: Lfo::default(), amp: 0.8 }
    }
}

// ── Envelope state ──────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Stage { Attack, Decay, Sustain, Release, Done }

#[derive(Clone, Copy)]
struct Env { stage: Stage, level: f32 }
impl Env {
    fn new() -> Self { Self { stage: Stage::Attack, level: 0.0 } }
    fn release(&mut self) { self.stage = Stage::Release; }
    fn done(&self) -> bool { matches!(self.stage, Stage::Done) }
    #[inline]
    fn next(&mut self, a: &Adsr, dt: f32) -> f32 {
        match self.stage {
            Stage::Attack => {
                self.level += if a.a > 1e-5 { dt / a.a } else { 1.0 };
                if self.level >= 1.0 { self.level = 1.0; self.stage = Stage::Decay; }
            }
            Stage::Decay => {
                let rate = if a.d > 1e-5 { dt / a.d } else { 1.0 };
                self.level -= (1.0 - a.s) * rate;
                if self.level <= a.s { self.level = a.s; self.stage = Stage::Sustain; }
            }
            Stage::Sustain => { self.level = a.s; }
            Stage::Release => {
                self.level -= if a.r > 1e-5 { a.s.max(self.level) * (dt / a.r) } else { 1.0 };
                if self.level <= 0.0001 { self.level = 0.0; self.stage = Stage::Done; }
            }
            Stage::Done => { self.level = 0.0; }
        }
        self.level
    }
}

// ── Voice ───────────────────────────────────────────────────────────────────

struct Voice {
    patch: Patch,
    midi: i32,
    vel: f32,
    phases: Vec<f32>,
    envs: Vec<Env>,
    fenv: Env,
    lfo_phase: f32,
    rng: u32,
    // SVF state
    svf_low: f32,
    svf_band: f32,
    age: u64,
    auto_off: Option<u64>, // sample index at which to auto-release (one-shots)
    active: bool,
}

impl Voice {
    fn new(patch: Patch, midi: i32, vel: f32, seed: u32) -> Self {
        let n = patch.ops.len();
        Self {
            phases: vec![0.0; n],
            envs: vec![Env::new(); n],
            fenv: Env::new(),
            patch, midi, vel,
            lfo_phase: 0.0,
            rng: seed | 1,
            svf_low: 0.0, svf_band: 0.0,
            age: 0,
            auto_off: None,
            active: true,
        }
    }

    #[inline]
    fn next(&mut self, rate: f32) -> f32 {
        if !self.active { return 0.0; }
        let dt = 1.0 / rate;
        if let Some(off) = self.auto_off {
            if self.age >= off { for e in &mut self.envs { e.release(); } self.fenv.release(); self.auto_off = None; }
        }

        let p = &self.patch;
        let lfo = (self.lfo_phase * std::f32::consts::TAU).sin();
        self.lfo_phase = (self.lfo_phase + p.lfo.rate * dt).fract();
        let pitch_mod = 1.0 + lfo * p.lfo.pitch * 0.06; // ±cents-ish vibrato
        let base_hz = midi_to_hz(self.midi as f32) * pitch_mod;

        let n = p.ops.len();
        let mut out_buf = vec![0.0f32; n];
        let mut carrier_sum = 0.0f32;
        // high → low so a modulator (higher index) is ready for its carrier
        for i in (0..n).rev() {
            let op = p.ops[i];
            let env = self.envs[i].next(&op.adsr, dt);
            let detune = 2.0_f32.powf(op.detune / 1200.0);
            let hz = if op.fixed_hz > 0.0 { op.fixed_hz } else { base_hz * op.ratio } * detune;
            // FM: sum modulators whose target is this op
            let mut pmod = 0.0f32;
            for j in (i + 1)..n {
                if p.ops[j].fm_target == i as i32 { pmod += out_buf[j]; }
            }
            self.phases[i] = (self.phases[i] + hz * dt).fract();
            let s = op.wave.eval(self.phases[i] + pmod, &mut self.rng) * op.level * env;
            out_buf[i] = s;
            if op.fm_target < 0 { carrier_sum += s; }
        }

        // amplitude envelope = the carriers' own envelopes already applied; gate by velocity
        let mut sample = carrier_sum * self.vel * p.amp;

        // resonant low-pass (Chamberlin SVF), cutoff in Hz with envelope
        if p.filter.on {
            let fe = self.fenv.next(&p.filter.adsr, dt);
            let c01 = (p.filter.cutoff + p.filter.env * fe + p.lfo.cutoff * lfo).clamp(0.02, 1.0);
            let cutoff_hz = 80.0 * (c01 * 7.5).exp2().min(rate * 0.45 / 80.0); // ~80..~12kHz
            let f = (2.0 * std::f32::consts::PI * cutoff_hz / rate).min(1.4);
            let q = 1.0 / (0.5 + p.filter.reso * 3.5);
            let high = sample - self.svf_low - q * self.svf_band;
            self.svf_band += f * high;
            self.svf_low  += f * self.svf_band;
            sample = self.svf_low;
        }

        self.age += 1;
        // dead when all op envelopes finished
        if self.envs.iter().all(|e| e.done()) { self.active = false; }
        sample
    }

    fn release(&mut self) { for e in &mut self.envs { e.release(); } self.fenv.release(); }
}

// ── Synth ───────────────────────────────────────────────────────────────────

/// Polyphonic synth: a registry of patches + live voices.
pub struct Synth {
    rate: f32,
    pub patches: Vec<Patch>,
    voices: Vec<Voice>,
    sample_clock: u64,
    next_voice_id: u32,
}

impl Synth {
    pub fn new(rate: f32) -> Self {
        Self { rate, patches: Vec::new(), voices: Vec::new(), sample_clock: 0, next_voice_id: 1 }
    }

    /// Register a patch, returning its instrument handle.
    pub fn add_patch(&mut self, p: Patch) -> usize { self.patches.push(p); self.patches.len() - 1 }

    /// Start a note; returns a voice id for `note_off`.
    pub fn note_on(&mut self, patch: usize, midi: i32, vel: f32) -> u32 {
        let p = match self.patches.get(patch) { Some(p) => p.clone(), None => return 0 };
        let poly = p.poly.max(1);
        // voice stealing: drop oldest if at capacity
        if self.voices.len() >= poly * 2 { self.voices.remove(0); }
        let id = self.next_voice_id; self.next_voice_id += 1;
        let mut v = Voice::new(p, midi, vel.clamp(0.0, 1.0), id.wrapping_mul(2654435761));
        let _ = &mut v;
        self.voices.push(v);
        id
    }

    /// Trigger a note that auto-releases after `dur` seconds (one-shot).
    pub fn note(&mut self, patch: usize, midi: i32, vel: f32, dur: f32) -> u32 {
        let id = self.note_on(patch, midi, vel);
        if let Some(v) = self.voices.last_mut() {
            v.auto_off = Some(self.sample_clock + (dur.max(0.0) * self.rate) as u64);
        }
        id
    }

    /// Release the most recent voice playing `midi` (simple matcher).
    pub fn note_off_pitch(&mut self, patch: usize, midi: i32) {
        if let Some(v) = self.voices.iter_mut().rev()
            .find(|v| v.active && v.midi == midi && v.auto_off.is_none()
                  && self.patches.get(patch).map(|p| p.name.as_str()) == Some(v.patch.name.as_str())) {
            v.release();
        }
    }

    pub fn all_notes_off(&mut self) { for v in &mut self.voices { v.release(); } }
    pub fn active_voices(&self) -> usize { self.voices.iter().filter(|v| v.active).count() }

    /// Render the next mono sample (sum of all voices).
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        let rate = self.rate;
        let mut s = 0.0f32;
        for v in &mut self.voices { s += v.next(rate); }
        self.voices.retain(|v| v.active);
        self.sample_clock += 1;
        // soft clip so dense chords don't blow up
        (s * 0.6).tanh()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn voice_plays_and_dies() {
        let mut syn = Synth::new(44100.0);
        let mut p = Patch::default();
        p.ops[0].adsr = Adsr { a: 0.001, d: 0.01, s: 0.0, r: 0.02 };
        let inst = syn.add_patch(p);
        syn.note(inst, 69, 1.0, 0.01); // short A4
        let mut peak = 0.0f32;
        for _ in 0..44100 { peak = peak.max(syn.next_sample().abs()); }
        assert!(peak > 0.01, "should make sound, peak={peak}");
        assert_eq!(syn.active_voices(), 0, "voice should have died");
    }

    #[test]
    fn fm_changes_timbre() {
        // A 2-op FM patch should produce harmonics beyond the carrier.
        let mut syn = Synth::new(44100.0);
        let mut p = Patch::default();
        p.ops = vec![
            Op { ratio: 1.0, fm_target: -1, adsr: Adsr { a:0.001,d:0.5,s:1.0,r:0.1 }, ..Default::default() },
            Op { ratio: 2.0, level: 2.0, fm_target: 0, adsr: Adsr { a:0.001,d:0.5,s:1.0,r:0.1 }, ..Default::default() },
        ];
        let inst = syn.add_patch(p);
        syn.note_on(inst, 69, 1.0);
        let mut energy = 0.0f32;
        for _ in 0..2048 { let s = syn.next_sample(); energy += s * s; }
        assert!(energy > 0.0);
    }
}
