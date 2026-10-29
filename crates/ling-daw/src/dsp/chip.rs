//! Ling Chip: the sound chips of 8- and 16-bit consoles and home computers - NES (2A03), Commodore 64 (SID), Game Boy and SNES
//! (sample playback) - as one instrument for chip music and game sound effects.
//!
//! A voice is one chip channel: a pulse (duty 12.5 / 25 / 50 / 75 % on the NES and Game Boy, any width with PWM on the SID), a
//! triangle (32 coarse steps on the NES, smooth on the SID), a saw, LFSR noise (long, or the short metallic loop) or a sample (the
//! SNES: 4-point Gaussian interpolation, an optional loop, your own WAV or one of the bundled ones). On top:
//! * the arpeggiator - up, down, up-down, random or the held notes in order, through a chord shape (major, minor, power, octave,
//!   sus4, maj7, min7, dim, add9) when one note is held, at chip ticks (50 / 60 Hz) or synced to the tempo, over 1..3 octaves;
//! * four 16-step sequences (FamiTracker-style macros): volume, arpeggio (semitones), duty and fine pitch, each with its length and
//!   loop point, stepped every `m rate` ticks;
//! * a pitch sweep (the NES sweep unit: up for power-ups and lasers, down for drops and explosions) that can run for a set time,
//!   vibrato with a delay, glide;
//! * the SID filter (low / band / high pass), ring modulation, noise mixed in, bit and sample-rate reduction, and the SNES echo.
//! Every chip has its own colour: the NES and Game Boy quantize volume to 16 steps and duty to four widths and run their output
//! through the console's filters (90 Hz high-pass, 14 kHz low-pass).
use super::{midi_hz, Instrument, Rng, Smooth};
use std::f32::consts::TAU;
use std::sync::Arc;

pub const CHIPS: [&str; 5] = ["nes", "sid", "gb", "snes", "free"];
pub const WAVES: [&str; 6] = ["pulse", "tri", "saw", "noise", "metal", "sample"];
pub const ARPS: [&str; 6] = ["off", "up", "down", "updown", "random", "held"];
pub const SHAPES: [&str; 9] = ["maj", "min", "power", "oct", "sus4", "maj7", "min7", "dim", "add9"];
const SHAPE_NOTES: [&[i32]; 9] = [&[0, 4, 7], &[0, 3, 7], &[0, 7, 12], &[0, 12], &[0, 5, 7], &[0, 4, 7, 11], &[0, 3, 7, 10], &[0, 3, 6, 9], &[0, 4, 7, 14]];
/// arp speeds: chip ticks per step, or (synced) steps per beat
const ARP_TICKS: [f32; 6] = [1.0, 2.0, 3.0, 4.0, 6.0, 8.0];
const ARP_SYNC: [f32; 6] = [16.0, 12.0, 8.0, 6.0, 4.0, 2.0];
const NSTEP: usize = 16;
const MACROS: [&str; 4] = ["v", "a", "d", "p"];

const CORE: [&str; 39] = [
    "chip", "wave", "duty", "pwm", "pwm rate", "amp a", "amp d", "amp s", "amp r", "arp", "arp shape", "arp speed", "arp oct", "sync", "sweep", "sweep time", "vib", "vib rate",
    "vib delay", "glide", "cutoff", "res", "filter", "ring", "noise", "bits", "rate", "echo", "echo time", "echo fb", "poly", "level", "m rate", "sample", "sloop", "sroot", "width",
    "tempo", "detune",
];
pub const NPARAM: usize = CORE.len() + MACROS.len() * (NSTEP + 2);

pub fn param_names() -> &'static [&'static str] {
    static N: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    N.get_or_init(|| {
        let mut v: Vec<&'static str> = CORE.to_vec();
        for m in MACROS {
            v.push(Box::leak(format!("{m} len").into_boxed_str()));
            v.push(Box::leak(format!("{m} loop").into_boxed_str()));
            for k in 0..NSTEP {
                v.push(Box::leak(format!("{m} s{k}").into_boxed_str()));
            }
        }
        v
    })
}

fn idx(name: &str) -> usize {
    param_names().iter().position(|n| *n == name).unwrap_or(0)
}

fn defaults() -> Vec<f32> {
    let mut v = vec![0.0f32; NPARAM];
    let mut set = |n: &str, x: f32| v[idx(n)] = x;
    set("wave", 0.0);
    set("duty", 0.5);
    set("pwm rate", 0.3);
    set("amp a", 0.0);
    set("amp d", 0.3);
    set("amp s", 0.8);
    set("amp r", 0.15);
    set("arp speed", 0.2);
    set("arp oct", 0.0);
    set("sweep", 0.5);
    set("sweep time", 1.0);
    set("vib rate", 0.5);
    set("vib delay", 0.2);
    set("cutoff", 1.0);
    set("res", 0.1);
    set("bits", 0.0);
    set("rate", 0.0);
    set("echo time", 0.4);
    set("echo fb", 0.35);
    set("poly", 1.0);
    set("level", 0.7);
    set("m rate", 0.0);
    set("sroot", (60.0 - 24.0) / 72.0);
    set("width", 0.3);
    set("tempo", (120.0 - 60.0) / 140.0);
    set("detune", 0.0);
    for m in MACROS {
        let base = idx(&format!("{m} s0"));
        // neutral steps: full volume, no transposition, the duty knob's own width, no pitch change
        let neutral = if m == "v" { 1.0 } else { 0.5 };
        for k in 0..NSTEP {
            v[base + k] = neutral;
        }
    }
    v
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Idle,
    Att,
    Dec,
    Sus,
    Rel,
}

#[derive(Clone)]
struct Voice {
    on: bool,
    key: u8,
    vel: f32,
    gate: bool,
    stage: Stage,
    env: f32,
    pitch: f32,
    target: f32,
    ph: f32,
    ph2: f32,
    lfsr: u16,
    noise_ph: f32,
    noise_out: f32,
    t: f32,
    ticks: f32,
    arp_i: usize,
    arp_dir: i32,
    arp_ticks: f32,
    step_ticks: f32,
    mstep: [usize; 4],
    sweep: f32,
    spos: f64,
    svf: (f32, f32),
    age: u64,
    /// per-note expression: pitch gesture (semitones), pressure 0..1, timbre 0..1 (0.5 neutral); `fresh` = taken at once
    pb: Smooth,
    pr: Smooth,
    tm: Smooth,
    fresh: bool,
}

impl Voice {
    fn new() -> Voice {
        Voice { on: false, key: 60, vel: 1.0, gate: false, stage: Stage::Idle, env: 0.0, pitch: 60.0, target: 60.0, ph: 0.0, ph2: 0.0, lfsr: 1, noise_ph: 0.0, noise_out: 0.0, t: 0.0, ticks: 0.0, arp_i: 0, arp_dir: 1, arp_ticks: 0.0, step_ticks: 0.0, mstep: [0; 4], sweep: 0.0, spos: 0.0, svf: (0.0, 0.0), age: 0, pb: Smooth::at(0.0), pr: Smooth::at(0.0), tm: Smooth::at(0.5), fresh: false }
    }
}

/// a loaded sample: mono, its rate, where it comes from
#[derive(Clone)]
struct Smp {
    data: Arc<Vec<f32>>,
    rate: f32,
    path: String,
}

pub struct Chip {
    sr: f32,
    v: Vec<f32>,
    voices: Vec<Voice>,
    held: Vec<u8>,
    rng: Rng,
    clock: u64,
    smp: Option<Smp>,
    /// bundled samples: (name, path)
    bundled: Vec<(String, String)>,
    custom: String,
    crush_hold: (f32, f32),
    crush_t: f32,
    hp: [f32; 2],
    lp: [f32; 2],
    echo: Vec<f32>,
    echo_w: usize,
    echo_lp: [f32; 2],
    preset: String,
}

/// where the bundled samples live (the patches folder of LingStationX)
fn bundled_dir() -> std::path::PathBuf {
    for d in ["instruments/Chip/samples", "../instruments/Chip/samples"] {
        let p = std::path::PathBuf::from(d);
        if p.is_dir() {
            return p;
        }
    }
    std::path::PathBuf::from("instruments/Chip/samples")
}

impl Chip {
    pub fn new(sr: f32) -> Chip {
        let mut bundled: Vec<(String, String)> = std::fs::read_dir(bundled_dir())
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().extension().map(|x| x.eq_ignore_ascii_case("wav")).unwrap_or(false))
                    .map(|e| (e.path().file_stem().map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_default(), e.path().to_string_lossy().into_owned()))
                    .collect()
            })
            .unwrap_or_default();
        bundled.sort();
        let mut c = Chip {
            sr,
            v: defaults(),
            voices: (0..8).map(|_| Voice::new()).collect(),
            held: vec![],
            rng: Rng::new(0x51d),
            clock: 0,
            smp: None,
            bundled,
            custom: String::new(),
            crush_hold: (0.0, 0.0),
            crush_t: 0.0,
            hp: [0.0; 2],
            lp: [0.0; 2],
            echo: vec![0.0; (sr * 1.2) as usize * 2],
            echo_w: 0,
            echo_lp: [0.0; 2],
            preset: String::new(),
        };
        c.pick_bundled();
        c
    }

    fn p(&self, n: &str) -> f32 {
        self.v[idx(n)]
    }
    fn sel(&self, n: &str, count: usize) -> usize {
        ((self.p(n) * (count - 1) as f32).round() as usize).min(count - 1)
    }

    /// the bundled sample the `sample` knob points at (when no file of your own is loaded)
    fn pick_bundled(&mut self) {
        if !self.custom.is_empty() || self.bundled.is_empty() {
            return;
        }
        let i = ((self.p("sample") * (self.bundled.len() - 1) as f32).round() as usize).min(self.bundled.len() - 1);
        let path = self.bundled[i].1.clone();
        if self.smp.as_ref().map(|s| s.path == path).unwrap_or(false) {
            return;
        }
        self.smp = load_mono(&path);
    }

    /// the notes a voice steps through: the held notes (mode "held", mono), else the chord shape over the arp octaves
    fn arp_notes(&self, key: u8) -> Vec<i32> {
        let mode = self.sel("arp", 6);
        let octs = 1 + self.sel("arp oct", 3);
        let base: Vec<i32> = if mode == 5 && self.held.len() > 1 {
            let mut h: Vec<i32> = self.held.iter().map(|k| *k as i32).collect();
            h.sort();
            let lo = h[0];
            h.iter().map(|k| k - lo).collect()
        } else {
            SHAPE_NOTES[self.sel("arp shape", 9)].to_vec()
        };
        let mut out = vec![];
        for o in 0..octs {
            for b in &base {
                out.push(key as i32 + b + 12 * o as i32);
            }
        }
        if mode == 5 && self.held.len() > 1 {
            // the held notes keep their own pitches (the lowest is the voice's key)
            let lo = *self.held.iter().min().unwrap() as i32;
            return out.iter().map(|k| k - key as i32 + lo).collect();
        }
        out
    }

    fn free_voice(&mut self) -> usize {
        if self.p("poly") < 0.5 {
            return 0;
        }
        if let Some(i) = self.voices.iter().position(|v| v.stage == Stage::Idle) {
            return i;
        }
        // the oldest
        (0..self.voices.len()).min_by_key(|i| self.voices[*i].age).unwrap_or(0)
    }

    /// a sample's tempo for synced steps
    fn bpm(&self) -> f32 {
        60.0 + self.p("tempo") * 140.0
    }
}

/// a WAV as mono samples
fn load_mono(path: &str) -> Option<Smp> {
    let s = crate::model::load_sample(path).ok()?;
    let data: Vec<f32> = s.stereo.chunks(2).map(|c| (c[0] + c.get(1).copied().unwrap_or(c[0])) * 0.5).collect();
    if data.is_empty() {
        return None;
    }
    Some(Smp { data: Arc::new(data), rate: s.rate as f32, path: path.to_string() })
}

/// SNES-style 4-point interpolation (a Gaussian-ish cubic: soft, a little dull, like the console)
#[inline]
fn gauss(d: &[f32], pos: f64, looped: bool) -> f32 {
    let n = d.len();
    let i = pos.floor() as isize;
    let f = (pos - pos.floor()) as f32;
    let at = |k: isize| -> f32 {
        if looped {
            d[k.rem_euclid(n as isize) as usize]
        } else if k < 0 || k as usize >= n {
            0.0
        } else {
            d[k as usize]
        }
    };
    let (a, b, c, e) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    // B-spline weights: smooth, slightly low-passed
    let w0 = (1.0 - f).powi(3) / 6.0;
    let w1 = (3.0 * f * f * f - 6.0 * f * f + 4.0) / 6.0;
    let w2 = (-3.0 * f * f * f + 3.0 * f * f + 3.0 * f + 1.0) / 6.0;
    let w3 = f * f * f / 6.0;
    a * w0 + b * w1 + c * w2 + e * w3
}

impl Instrument for Chip {
    fn note_on(&mut self, key: u8, vel: f32) {
        self.held.retain(|k| *k != key);
        self.held.push(key);
        let mono = self.p("poly") < 0.5;
        let i = self.free_voice();
        self.clock += 1;
        let glide = self.p("glide") > 0.01;
        let vkey = if mono && self.sel("arp", 6) == 5 { *self.held.iter().min().unwrap() } else { key };
        let v = &mut self.voices[i];
        let legato = mono && v.gate && glide;
        // a legato slide starts where the voice is heard (its pitch gesture included); the new note brings its own gesture
        if legato && v.pb.cur != 0.0 {
            v.pitch += v.pb.cur;
        }
        v.pb.reset(0.0);
        v.pr.reset(0.0);
        v.tm.reset(0.5);
        v.fresh = true;
        v.on = true;
        v.key = vkey;
        v.vel = vel.clamp(0.0, 1.0);
        v.target = v.key as f32;
        if !legato {
            v.pitch = v.target;
            v.stage = Stage::Att;
            v.t = 0.0;
            v.ticks = 0.0;
            v.arp_i = 0;
            v.arp_dir = 1;
            v.arp_ticks = 0.0;
            v.step_ticks = 0.0;
            v.mstep = [0; 4];
            v.sweep = 0.0;
            v.spos = 0.0;
            v.ph = 0.0;
            v.lfsr = 1;
        }
        v.gate = true;
        v.age = self.clock;
    }
    fn note_off(&mut self, key: u8) {
        self.held.retain(|k| *k != key);
        let mono = self.p("poly") < 0.5;
        if mono {
            if let Some(&k) = self.held.last() {
                // back to the last held note (legato)
                let vkey = if self.sel("arp", 6) == 5 { *self.held.iter().min().unwrap() } else { k };
                let jump = self.p("glide") < 0.01;
                let v = &mut self.voices[0];
                v.key = vkey;
                v.target = v.key as f32;
                // the returning note's own gesture (if any) arrives with the next block
                if !v.pb.idle(0.0) {
                    v.pitch += v.pb.cur;
                    v.pb.reset(0.0);
                }
                v.fresh = true;
                if jump {
                    v.pitch = v.target;
                }
                return;
            }
            let v = &mut self.voices[0];
            v.gate = false;
            v.stage = Stage::Rel;
            return;
        }
        for v in self.voices.iter_mut().filter(|v| v.gate && v.key == key) {
            v.gate = false;
            v.stage = Stage::Rel;
        }
    }
    fn all_off(&mut self) {
        self.held.clear();
        for v in self.voices.iter_mut() {
            v.gate = false;
            v.stage = Stage::Idle;
            v.env = 0.0;
            v.on = false;
        }
    }
    fn note_pitch(&mut self, key: u8, semis: f32) {
        for v in self.voices.iter_mut().filter(|v| v.gate && v.key == key) {
            v.pb.set(semis, v.fresh);
        }
    }
    fn note_pressure(&mut self, key: u8, x: f32) {
        for v in self.voices.iter_mut().filter(|v| v.gate && v.key == key) {
            v.pr.set(x, v.fresh);
        }
    }
    fn note_timbre(&mut self, key: u8, x: f32) {
        for v in self.voices.iter_mut().filter(|v| v.gate && v.key == key) {
            v.tm.set(x, v.fresh);
        }
    }
    fn set_tempo(&mut self, bpm: f64) {
        let i = idx("tempo");
        self.v[i] = ((bpm as f32 - 60.0) / 140.0).clamp(0.0, 1.0);
    }

    fn render(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        if frames == 0 {
            return;
        }
        self.pick_bundled();
        let sr = self.sr;
        let dt = 1.0 / sr;
        let chip = self.sel("chip", 5);
        let wave = self.sel("wave", 6);
        let quant = chip == 0 || chip == 2;
        let tick_hz = if chip == 1 { 50.0 } else { 60.0 };
        let sync = self.p("sync") > 0.5;
        let bpm = self.bpm();
        let arp_mode = self.sel("arp", 6);
        let arp_step_s = if sync { 60.0 / bpm / ARP_SYNC[self.sel("arp speed", 6)] } else { ARP_TICKS[self.sel("arp speed", 6)] / tick_hz };
        let m_step_s = (1.0 + (self.p("m rate") * 7.0).round()) / tick_hz;
        let (aa, ad, as_, ar) = (0.0005 + self.p("amp a").powi(2) * 2.0, 0.005 + self.p("amp d").powi(2) * 2.5, self.p("amp s"), 0.005 + self.p("amp r").powi(2) * 3.0);
        let sweep_rate = {
            let x = (self.p("sweep") - 0.5) * 2.0;
            x.signum() * x * x * 96.0 // semitones per second, up to 8 octaves
        };
        let sweep_len = if self.p("sweep time") > 0.99 { f32::MAX } else { 0.01 + self.p("sweep time").powi(2) * 3.0 };
        let (vib, vib_hz, vib_delay) = (self.p("vib") * 1.0, 0.5 + self.p("vib rate") * 11.5, self.p("vib delay") * 1.5);
        let glide_k = if self.p("glide") > 0.01 { 1.0 - (-dt / (0.003 + self.p("glide").powi(2) * 0.6)).exp() } else { 1.0 };
        let duty0 = self.p("duty");
        let (pwm, pwm_hz) = (self.p("pwm"), 0.05 + self.p("pwm rate") * 6.0);
        let filt = self.sel("filter", 4);
        let cut_hz = 40.0 * 2f32.powf(self.p("cutoff") * 9.0);
        let g = (std::f32::consts::PI * cut_hz.min(sr * 0.45) / sr).tan();
        let k = 2.0 - self.p("res") * 1.9;
        let ring = self.p("ring");
        let noise_mix = self.p("noise");
        let detune = self.p("detune") * 0.3;
        let level = self.p("level").powi(2);
        let looped = self.p("sloop") > 0.5;
        let sroot = 24.0 + self.p("sroot") * 72.0;
        // the macros: lengths and loop points
        let mlen: [usize; 4] = std::array::from_fn(|m| (self.v[idx(&format!("{} len", MACROS[m]))] * 16.0).round() as usize);
        let mloop: [usize; 4] = std::array::from_fn(|m| ((self.v[idx(&format!("{} loop", MACROS[m]))] * 15.0).round() as usize).min(mlen[m].saturating_sub(1)));
        let mbase: [usize; 4] = std::array::from_fn(|m| idx(&format!("{} s0", MACROS[m])));
        let notes_cache: Vec<Vec<i32>> = self.voices.iter().map(|v| if v.stage != Stage::Idle { self.arp_notes(v.key) } else { vec![] }).collect();
        let width = self.p("width");
        let smp = self.smp.clone();
        for v in self.voices.iter_mut() {
            v.pb.begin(frames);
            v.pr.begin(frames);
            v.tm.begin(frames);
            v.fresh = false;
        }
        for i in 0..frames {
            let (mut sl, mut sr_out) = (0.0f32, 0.0f32);
            for (vi, v) in self.voices.iter_mut().enumerate() {
                if v.stage == Stage::Idle {
                    continue;
                }
                // envelope (16 steps on the NES / Game Boy)
                match v.stage {
                    Stage::Att => {
                        v.env += dt / aa;
                        if v.env >= 1.0 {
                            v.env = 1.0;
                            v.stage = Stage::Dec;
                        }
                    }
                    Stage::Dec => {
                        v.env += (as_ - v.env) * (1.0 - (-dt * 4.0 / ad).exp());
                        if (v.env - as_).abs() < 0.001 {
                            v.stage = Stage::Sus;
                        }
                    }
                    Stage::Sus => v.env = as_,
                    Stage::Rel => {
                        v.env -= v.env * (1.0 - (-dt * 4.0 / ar).exp());
                        if v.env < 0.0005 {
                            v.env = 0.0;
                            v.stage = Stage::Idle;
                            v.on = false;
                            continue;
                        }
                    }
                    Stage::Idle => {}
                }
                v.t += dt;
                // the macros step on chip ticks
                v.step_ticks += dt;
                if v.step_ticks >= m_step_s {
                    v.step_ticks -= m_step_s;
                    for m in 0..4 {
                        if mlen[m] > 0 {
                            v.mstep[m] += 1;
                            if v.mstep[m] >= mlen[m] {
                                v.mstep[m] = if m == 0 && !v.gate { mlen[m] - 1 } else { mloop[m] };
                            }
                        }
                    }
                }
                let mv = |m: usize, v: &Voice, vals: &Vec<f32>| if mlen[m] > 0 { vals[mbase[m] + v.mstep[m].min(15)] } else if m == 0 { 1.0 } else { 0.5 };
                let m_vol = mv(0, v, &self.v);
                let m_arp = ((mv(1, v, &self.v) - 0.5) * 48.0).round();
                let m_duty = mv(2, v, &self.v);
                let m_pitch = (mv(3, v, &self.v) - 0.5) * 2.0;
                // the arpeggiator
                let notes = &notes_cache[vi];
                let mut key = v.pitch;
                if arp_mode > 0 && !notes.is_empty() {
                    v.arp_ticks += dt;
                    if v.arp_ticks >= arp_step_s {
                        v.arp_ticks -= arp_step_s;
                        let n = notes.len();
                        v.arp_i = match arp_mode {
                            2 => (v.arp_i + n - 1) % n,
                            3 => {
                                let ni = v.arp_i as i32 + v.arp_dir;
                                if ni < 0 || ni >= n as i32 {
                                    v.arp_dir = -v.arp_dir;
                                }
                                (v.arp_i as i32 + v.arp_dir).clamp(0, n as i32 - 1) as usize
                            }
                            4 => (((self.rng.next() + 1.0) * 0.5 * n as f32) as usize).min(n - 1),
                            _ => (v.arp_i + 1) % n,
                        };
                    }
                    key = notes[v.arp_i.min(notes.len() - 1)] as f32 + (v.pitch - v.key as f32);
                }
                v.pitch += (v.target - v.pitch) * glide_k;
                // sweep, vibrato, the pitch macro
                if v.t < sweep_len {
                    v.sweep += sweep_rate * dt;
                }
                let vibv = if v.t > vib_delay { (TAU * vib_hz * v.t).sin() * vib * ((v.t - vib_delay) * 4.0).min(1.0) } else { 0.0 };
                let gest = if v.pb.idle(0.0) { 0.0 } else { v.pb.tick() };
                let (press, timbre) = (if v.pr.idle(0.0) { 0.0 } else { v.pr.tick() }, if v.tm.idle(0.5) { 0.5 } else { v.tm.tick() });
                let semis = if gest != 0.0 { (key + m_arp + v.sweep + vibv + m_pitch + gest).clamp(-24.0, 140.0) } else { (key + m_arp + v.sweep + vibv + m_pitch).clamp(-24.0, 140.0) };
                let f = midi_hz(semis);
                let dph = f / sr;
                // the oscillator
                let duty = {
                    let d = if mlen[2] > 0 { m_duty } else { duty0 };
                    let d = if timbre != 0.5 { d + (timbre - 0.5) * 0.8 } else { d };
                    let d = (d + pwm * 0.45 * (TAU * pwm_hz * v.t).sin()).clamp(0.02, 0.98);
                    if quant {
                        [0.125, 0.25, 0.5, 0.75][((d * 3.0).round() as usize).min(3)]
                    } else {
                        0.02 + d * 0.48 * 2.0 * 0.5 + d * 0.0
                    }
                };
                v.ph += dph;
                if v.ph >= 1.0 {
                    v.ph -= v.ph.floor();
                }
                let mut s = match wave {
                    0 => {
                        if v.ph < duty {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                    1 => {
                        let t = 1.0 - 4.0 * (v.ph - 0.5).abs();
                        if chip == 0 {
                            // the 2A03 triangle: 32 steps
                            ((t * 7.5).round()) / 7.5
                        } else {
                            t
                        }
                    }
                    2 => {
                        let s = 2.0 * v.ph - 1.0;
                        if quant {
                            (s * 7.5).round() / 7.5
                        } else {
                            s
                        }
                    }
                    3 | 4 => {
                        // LFSR clocked from the pitch (the NES noise period table, roughly)
                        v.noise_ph += f * 8.0 / sr;
                        while v.noise_ph >= 1.0 {
                            v.noise_ph -= 1.0;
                            let tap = if wave == 4 { 6 } else { 1 };
                            let bit = (v.lfsr ^ (v.lfsr >> tap)) & 1;
                            v.lfsr = (v.lfsr >> 1) | (bit << 14);
                            v.noise_out = if v.lfsr & 1 == 1 { 1.0 } else { -1.0 };
                        }
                        v.noise_out
                    }
                    _ => match &smp {
                        Some(sm) if !sm.data.is_empty() => {
                            let step = (sm.rate / sr) as f64 * 2f64.powf(((semis - sroot) / 12.0) as f64);
                            v.spos += step;
                            if looped && v.spos >= sm.data.len() as f64 {
                                v.spos %= sm.data.len() as f64;
                            }
                            if !looped && v.spos >= sm.data.len() as f64 {
                                0.0
                            } else {
                                gauss(&sm.data, v.spos, looped)
                            }
                        }
                        _ => 0.0,
                    },
                };
                if ring > 0.001 {
                    v.ph2 += dph * 1.4983;
                    v.ph2 -= v.ph2.floor();
                    let r = if v.ph2 < 0.5 { 1.0 } else { -1.0 };
                    s = s * (1.0 - ring) + s * r * ring;
                }
                if noise_mix > 0.001 && wave < 3 {
                    s = s * (1.0 - noise_mix * 0.6) + self.rng.next() * noise_mix * 0.6;
                }
                // the SID filter
                if filt > 0 {
                    let (ic1, ic2) = v.svf;
                    let a1 = 1.0 / (1.0 + g * (g + k));
                    let v3 = s - ic2;
                    let v1 = a1 * ic1 + g * a1 * v3;
                    let v2 = ic2 + g * v1;
                    v.svf = (2.0 * v1 - ic1, 2.0 * v2 - ic2);
                    s = match filt {
                        1 => v2,
                        2 => v1 * k,
                        _ => s - k * v1 - v2,
                    };
                }
                // volume: the macro times the envelope, 16 steps on the NES / Game Boy
                let mut vol = v.env * m_vol * (0.35 + 0.65 * v.vel);
                if press != 0.0 {
                    vol *= 1.0 + press * 0.5;
                }
                if quant {
                    vol = (vol * 15.0).round() / 15.0;
                }
                let x = s * vol * 0.32;
                // a little stereo: alternate voices left / right by `width`, the detune doubles a voice slightly
                let pan = if vi % 2 == 0 { -width } else { width } * 0.5;
                sl += x * (1.0 - pan);
                sr_out += x * (1.0 + pan);
                if detune > 0.001 {
                    v.ph2 += dph * (1.0 + detune * 0.01);
                    v.ph2 -= v.ph2.floor();
                }
            }
            // bit and rate reduction
            let bits = self.p("bits");
            if self.p("rate") > 0.005 {
                let hold_hz = 44100.0 * (1.0 - self.p("rate")).powi(2) + 2000.0;
                self.crush_t += hold_hz / sr;
                if self.crush_t >= 1.0 {
                    self.crush_t -= self.crush_t.floor();
                    self.crush_hold = (sl, sr_out);
                }
                sl = self.crush_hold.0;
                sr_out = self.crush_hold.1;
            }
            if bits > 0.005 {
                let q = 2f32.powf(16.0 - bits * 13.0);
                sl = (sl * q).round() / q;
                sr_out = (sr_out * q).round() / q;
            }
            // the console's own output filters (NES / Game Boy): 90 Hz high-pass, 14 kHz low-pass
            if quant {
                let ah = 1.0 - (-TAU * 90.0 / sr).exp();
                let al = 1.0 - (-TAU * 14000.0f32.min(sr * 0.45) / sr).exp();
                self.hp[0] += (sl - self.hp[0]) * ah;
                self.hp[1] += (sr_out - self.hp[1]) * ah;
                sl -= self.hp[0];
                sr_out -= self.hp[1];
                self.lp[0] += (sl - self.lp[0]) * al;
                self.lp[1] += (sr_out - self.lp[1]) * al;
                sl = self.lp[0];
                sr_out = self.lp[1];
            }
            // the SNES echo
            let em = self.p("echo");
            if em > 0.002 {
                let n = self.echo.len() / 2;
                let d = ((0.04 + self.p("echo time") * 0.9) * sr) as usize % n.max(1);
                let ri = (self.echo_w + n - d.max(1)) % n;
                let (el, er) = (self.echo[ri * 2], self.echo[ri * 2 + 1]);
                self.echo_lp[0] += (el - self.echo_lp[0]) * 0.35;
                self.echo_lp[1] += (er - self.echo_lp[1]) * 0.35;
                let fb = self.p("echo fb") * 0.85;
                // ping-pong
                self.echo[self.echo_w * 2] = sl + self.echo_lp[1] * fb;
                self.echo[self.echo_w * 2 + 1] = sr_out + self.echo_lp[0] * fb;
                self.echo_w = (self.echo_w + 1) % n;
                sl += self.echo_lp[0] * em;
                sr_out += self.echo_lp[1] * em;
            }
            let (l, r) = ((sl * level).tanh(), (sr_out * level).tanh());
            out[i * 2] += if l.is_finite() { l } else { 0.0 };
            out[i * 2 + 1] += if r.is_finite() { r } else { 0.0 };
        }
    }

    fn params(&self) -> Vec<(&'static str, f32)> {
        param_names().iter().zip(self.v.iter()).map(|(n, v)| (*n, *v)).collect()
    }
    fn set_param(&mut self, name: &str, v: f32) {
        if !v.is_finite() {
            return;
        }
        if let Some(i) = param_names().iter().position(|n| *n == name) {
            self.v[i] = v.clamp(0.0, 1.0);
        }
    }
    fn name(&self) -> &'static str {
        "Ling Chip"
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
            // a preset that plays a bundled sample picks it by name
            if let Some(sname) = p.3.iter().find(|x| x.0 == "sample").map(|_| ()).and(PRESET_SAMPLES.iter().find(|s| s.0 == p.0)) {
                if let Some(bi) = self.bundled.iter().position(|b| b.0.eq_ignore_ascii_case(sname.1)) {
                    let n = self.bundled.len().max(2) - 1;
                    let i = idx("sample");
                    self.v[i] = bi as f32 / n as f32;
                }
            }
            self.custom.clear();
            self.smp = None;
            self.pick_bundled();
        }
    }
    fn randomize(&mut self, seed: u32) -> bool {
        let mut r = Rng::new(seed.wrapping_mul(2654435761).wrapping_add(7));
        let p = &PRESETS[(((r.next() + 1.0) * 0.5 * PRESETS.len() as f32) as usize).min(PRESETS.len() - 1)];
        self.load_preset(p.0);
        for n in ["duty", "pwm", "arp speed", "vib", "cutoff", "res", "echo", "echo time", "amp d", "amp s"] {
            let i = idx(n);
            self.v[i] = (self.v[i] + r.next() * 0.2).clamp(0.0, 1.0);
        }
        true
    }
    fn command(&mut self, cmd: &str, _args: &[f64], text: &str) -> Vec<f64> {
        match cmd {
            // load a WAV of your own into the sample channel
            "load" => match load_mono(text) {
                Some(s) => {
                    self.custom = text.to_string();
                    self.smp = Some(s);
                    let i = idx("wave");
                    self.v[i] = 1.0;
                    vec![1.0]
                }
                None => vec![0.0],
            },
            // back to the bundled samples
            "unload" => {
                self.custom.clear();
                self.smp = None;
                self.pick_bundled();
                vec![1.0]
            }
            _ => vec![],
        }
    }
    fn info(&self, what: &str) -> String {
        match what {
            "file" | "config" => self.custom.clone(),
            "sample" => self.smp.as_ref().map(|s| std::path::Path::new(&s.path).file_stem().map(|x| x.to_string_lossy().replace('_', " ")).unwrap_or_default()).unwrap_or_default(),
            "samples" => self.bundled.iter().map(|b| b.0.clone()).collect::<Vec<_>>().join("\n"),
            "preset" => PRESETS.iter().find(|p| p.0 == self.preset).map(|p| p.2.to_string()).unwrap_or_default(),
            _ => String::new(),
        }
    }
    fn preset_info(&self, name: &str) -> String {
        PRESETS.iter().find(|p| p.0.eq_ignore_ascii_case(name)).map(|p| p.2.to_string()).unwrap_or_default()
    }
    fn plot(&self, what: &str, n: usize) -> Vec<f32> {
        if what != "wave" {
            return vec![];
        }
        let wave = self.sel("wave", 6);
        let duty = self.p("duty");
        (0..n)
            .map(|i| {
                let ph = i as f32 / n as f32;
                match wave {
                    0 => {
                        if ph < (0.05 + duty * 0.9) {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                    1 => 1.0 - 4.0 * (ph - 0.5).abs(),
                    2 => 2.0 * ph - 1.0,
                    3 | 4 => {
                        if ((ph * 37.0).sin() * 999.0).fract().abs() > 0.5 {
                            0.8
                        } else {
                            -0.8
                        }
                    }
                    _ => match &self.smp {
                        Some(s) if !s.data.is_empty() => s.data[((ph * s.data.len().min(4096) as f32) as usize).min(s.data.len() - 1)],
                        _ => 0.0,
                    },
                }
            })
            .collect()
    }
}

/// the bundled sample a preset plays
const PRESET_SAMPLES: &[(&str, &str)] = &[
    ("SNES Strings", "snes strings"),
    ("SNES Choir", "snes choir"),
    ("SNES Orchestra Hit", "orchestra hit"),
    ("SNES Slap Bass", "slap bass"),
    ("SNES Steel Drum", "steel drum"),
    ("SNES Brass", "snes brass"),
    ("SNES Flute", "snes flute"),
    ("Voice Ah", "voice ah"),
];

type Pre = (&'static str, &'static str, &'static str, &'static [(&'static str, f32)]);

/// (name, category, description, settings over the defaults). chip 0 nes, .25 sid, .5 gb, .75 snes, 1 free; wave 0 pulse, .2 tri, .4 saw,
/// .6 noise, .8 metal, 1 sample; arp 0 off, .2 up, .4 down, .6 updown, .8 random, 1 held; arp shape /8; arp speed /5; filter 0 off,
/// .333 lp, .667 bp, 1 hp
pub const PRESETS: &[Pre] = &[
    // ── sound effects
    ("Coin", "SFX", "the classic pickup: a square blip jumping up a fourth, short and bright", &[("chip", 0.0), ("wave", 0.0), ("duty", 0.33), ("amp d", 0.32), ("amp s", 0.0), ("amp r", 0.1), ("a len", 0.125), ("a loop", 0.07), ("a s0", 0.5), ("a s1", 0.604), ("m rate", 0.43), ("poly", 0.0)]),
    ("Jump", "SFX", "a rising square sweep - a quick upward chirp that dies away", &[("chip", 0.0), ("wave", 0.0), ("duty", 0.66), ("sweep", 0.86), ("sweep time", 0.2), ("amp d", 0.3), ("amp s", 0.0), ("amp r", 0.08), ("poly", 0.0)]),
    ("Power Up", "SFX", "an arpeggio climbing octaves while the pitch sweeps up - collect a mushroom", &[("chip", 0.0), ("wave", 0.0), ("duty", 0.33), ("arp", 0.2), ("arp shape", 0.375), ("arp oct", 1.0), ("arp speed", 0.2), ("sweep", 0.72), ("sweep time", 0.45), ("amp d", 0.6), ("amp s", 0.0), ("amp r", 0.1), ("poly", 0.0)]),
    ("1-Up", "SFX", "six bright notes in a row up the major chord - an extra life", &[("chip", 0.0), ("wave", 0.0), ("duty", 0.33), ("arp", 0.2), ("arp shape", 0.0), ("arp oct", 1.0), ("arp speed", 0.6), ("amp d", 0.7), ("amp s", 0.0), ("amp r", 0.1), ("poly", 0.0)]),
    ("Laser", "SFX", "a fast falling square sweep with a ring-modulated edge - pew", &[("chip", 0.0), ("wave", 0.0), ("duty", 0.0), ("sweep", 0.04), ("sweep time", 0.3), ("ring", 0.4), ("amp d", 0.25), ("amp s", 0.0), ("amp r", 0.05), ("poly", 0.0)]),
    ("Explosion", "SFX", "long noise falling in pitch with a low rumble - boom", &[("chip", 0.0), ("wave", 0.6), ("sweep", 0.18), ("sweep time", 0.7), ("amp d", 0.75), ("amp s", 0.0), ("amp r", 0.4), ("rate", 0.2), ("poly", 0.0)]),
    ("Hit", "SFX", "a short burst of metallic noise and a pitch drop - damage taken", &[("chip", 0.0), ("wave", 0.8), ("sweep", 0.12), ("sweep time", 0.2), ("amp d", 0.25), ("amp s", 0.0), ("amp r", 0.05), ("poly", 0.0)]),
    ("Menu Blip", "SFX", "a tiny high square tick for cursors and menus", &[("chip", 0.0), ("wave", 0.0), ("duty", 0.33), ("amp d", 0.15), ("amp s", 0.0), ("amp r", 0.03), ("poly", 0.0)]),
    ("Select", "SFX", "two quick rising notes - confirm", &[("chip", 0.5), ("wave", 0.0), ("duty", 0.66), ("a len", 0.125), ("a loop", 0.07), ("a s0", 0.5), ("a s1", 0.75), ("m rate", 0.29), ("amp d", 0.3), ("amp s", 0.0), ("amp r", 0.06), ("poly", 0.0)]),
    ("Warp", "SFX", "a wobbling upward sweep with vibrato and echo - teleport", &[("chip", 0.25), ("wave", 0.2), ("sweep", 0.82), ("sweep time", 0.6), ("vib", 0.6), ("vib rate", 0.8), ("vib delay", 0.0), ("echo", 0.35), ("amp d", 0.7), ("amp s", 0.0), ("amp r", 0.3), ("poly", 0.0)]),
    ("Fireball", "SFX", "noise with a quick downward sweep and a soft filter - whoosh", &[("chip", 0.25), ("wave", 0.6), ("sweep", 0.3), ("sweep time", 0.3), ("filter", 0.333), ("cutoff", 0.75), ("amp d", 0.4), ("amp s", 0.0), ("amp r", 0.1), ("poly", 0.0)]),
    ("Spring", "SFX", "a boing: a triangle bouncing up and down with fast vibrato", &[("chip", 0.0), ("wave", 0.2), ("sweep", 0.74), ("sweep time", 0.12), ("vib", 1.0), ("vib rate", 0.95), ("vib delay", 0.05), ("amp d", 0.5), ("amp s", 0.0), ("amp r", 0.1), ("poly", 0.0)]),
    ("Alarm", "SFX", "two alternating tones, looping - danger", &[("chip", 0.0), ("wave", 0.0), ("duty", 0.33), ("a len", 0.125), ("a loop", 0.0), ("a s0", 0.5), ("a s1", 0.5625), ("m rate", 1.0), ("amp s", 1.0), ("poly", 0.0)]),
    ("Rocket", "SFX", "a long rising noise with a sweeping filter - lift-off", &[("chip", 0.25), ("wave", 0.6), ("sweep", 0.7), ("sweep time", 0.85), ("filter", 0.667), ("cutoff", 0.5), ("res", 0.6), ("amp a", 0.3), ("amp s", 1.0), ("amp r", 0.4), ("poly", 0.0)]),
    ("Pickup Gem", "SFX", "a sparkling arpeggio up a major seventh chord with echo", &[("chip", 0.5), ("wave", 0.0), ("duty", 0.33), ("arp", 0.2), ("arp shape", 0.625), ("arp speed", 0.0), ("arp oct", 0.5), ("echo", 0.3), ("echo time", 0.2), ("amp d", 0.5), ("amp s", 0.0), ("amp r", 0.2), ("poly", 0.0)]),
    ("Game Over", "SFX", "a slow descending minor arpeggio with a falling pitch - the end", &[("chip", 0.0), ("wave", 0.0), ("duty", 0.66), ("arp", 0.4), ("arp shape", 0.125), ("arp speed", 1.0), ("sweep", 0.4), ("sweep time", 1.0), ("amp d", 0.9), ("amp s", 0.0), ("amp r", 0.3), ("poly", 0.0)]),
    // ── NES
    ("NES Square Lead", "NES", "the 2A03 square at 50 % with a delayed vibrato - the classic lead", &[("chip", 0.0), ("wave", 0.0), ("duty", 0.66), ("vib", 0.25), ("vib rate", 0.5), ("vib delay", 0.25), ("amp d", 0.4), ("amp s", 0.7)]),
    ("NES Duty Lead", "NES", "a square whose duty steps 12.5 -> 25 -> 50 % on every note: a bright, moving attack", &[("chip", 0.0), ("wave", 0.0), ("d len", 0.25), ("d loop", 0.2), ("d s0", 0.0), ("d s1", 0.33), ("d s2", 0.66), ("d s3", 0.66), ("m rate", 0.29), ("vib", 0.2), ("vib delay", 0.3), ("amp s", 0.7)]),
    ("NES Triangle Bass", "NES", "the stepped 2A03 triangle - round, buzzy bass", &[("chip", 0.0), ("wave", 0.2), ("amp d", 0.3), ("amp s", 1.0), ("amp r", 0.05)]),
    ("NES Kick", "NES", "a triangle pitch drop - the 8-bit kick drum", &[("chip", 0.0), ("wave", 0.2), ("sweep", 0.0), ("sweep time", 0.2), ("amp d", 0.32), ("amp s", 0.0), ("amp r", 0.05)]),
    ("NES Snare", "NES", "noise with a fast decay and a short square body", &[("chip", 0.0), ("wave", 0.6), ("amp d", 0.28), ("amp s", 0.0), ("amp r", 0.05), ("v len", 0.25), ("v s0", 1.0), ("v s1", 0.6), ("v s2", 0.3), ("v s3", 0.0), ("m rate", 0.14)]),
    ("NES Hi-Hat", "NES", "short metallic noise - a closed hat", &[("chip", 0.0), ("wave", 0.8), ("amp d", 0.12), ("amp s", 0.0), ("amp r", 0.02)]),
    ("NES Echo Lead", "NES", "a 25 % square played twice: the trick of a quieter copy a few ticks late (via the volume sequence)", &[("chip", 0.0), ("wave", 0.0), ("duty", 0.33), ("v len", 0.5), ("v loop", 0.47), ("v s0", 1.0), ("v s1", 1.0), ("v s2", 0.9), ("v s3", 0.8), ("v s4", 0.35), ("v s5", 0.45), ("v s6", 0.45), ("v s7", 0.4), ("m rate", 0.43), ("amp s", 0.9)]),
    // ── C64 SID
    ("SID Saw Lead", "SID", "the SID sawtooth through the low-pass filter with vibrato - soaring", &[("chip", 0.25), ("wave", 0.4), ("filter", 0.333), ("cutoff", 0.72), ("res", 0.35), ("vib", 0.3), ("vib delay", 0.25), ("amp s", 0.8)]),
    ("SID PWM Pad", "SID", "a pulse with slow pulse-width modulation and a soft attack - lush 8-bit pad", &[("chip", 0.25), ("wave", 0.0), ("duty", 0.4), ("pwm", 0.75), ("pwm rate", 0.15), ("amp a", 0.45), ("amp s", 0.85), ("amp r", 0.5), ("filter", 0.333), ("cutoff", 0.7), ("echo", 0.25)]),
    ("SID Filter Bass", "SID", "a pulse bass with a resonant low-pass - the C64 squelch", &[("chip", 0.25), ("wave", 0.0), ("duty", 0.3), ("filter", 0.333), ("cutoff", 0.45), ("res", 0.7), ("amp d", 0.35), ("amp s", 0.5), ("poly", 0.0), ("glide", 0.15)]),
    ("SID Ring Bell", "SID", "a triangle ring-modulated into a metallic bell", &[("chip", 0.25), ("wave", 0.2), ("ring", 0.8), ("amp d", 0.6), ("amp s", 0.0), ("amp r", 0.5), ("echo", 0.25)]),
    ("C64 Arp Chords", "SID", "chords as one fast arpeggio of the held notes - the C64 trick for harmony on one voice", &[("chip", 0.25), ("wave", 0.0), ("duty", 0.4), ("pwm", 0.4), ("arp", 1.0), ("arp speed", 0.0), ("poly", 0.0), ("amp s", 0.8)]),
    ("SID Drum Kick", "SID", "a fast falling triangle with a click of noise", &[("chip", 0.25), ("wave", 0.2), ("noise", 0.2), ("sweep", 0.02), ("sweep time", 0.18), ("amp d", 0.3), ("amp s", 0.0)]),
    // ── Game Boy
    ("GB Pulse Lead", "Game Boy", "the Game Boy square at 25 % - nasal, bright, pocket-sized", &[("chip", 0.5), ("wave", 0.0), ("duty", 0.33), ("vib", 0.15), ("vib delay", 0.3), ("amp s", 0.75)]),
    ("GB Wave Bass", "Game Boy", "the wave channel's coarse saw - a fat little bass", &[("chip", 0.5), ("wave", 0.4), ("amp d", 0.3), ("amp s", 0.9), ("poly", 0.0)]),
    ("GB Arp Shimmer", "Game Boy", "a fast major arpeggio over two octaves with echo - sparkle", &[("chip", 0.5), ("wave", 0.0), ("duty", 0.33), ("arp", 0.2), ("arp shape", 0.0), ("arp oct", 0.5), ("arp speed", 0.0), ("echo", 0.35), ("echo time", 0.3), ("amp s", 0.7)]),
    // ── SNES (samples)
    ("SNES Strings", "SNES", "a looped string sample with soft interpolation and the SNES echo - 16-bit RPG strings", &[("chip", 0.75), ("wave", 1.0), ("sample", 0.0), ("sloop", 1.0), ("amp a", 0.35), ("amp s", 0.9), ("amp r", 0.5), ("echo", 0.35), ("echo time", 0.45), ("echo fb", 0.4)]),
    ("SNES Choir", "SNES", "an 'aah' choir sample, looped, with echo", &[("chip", 0.75), ("wave", 1.0), ("sample", 0.0), ("sloop", 1.0), ("amp a", 0.4), ("amp s", 0.9), ("amp r", 0.6), ("echo", 0.4)]),
    ("SNES Orchestra Hit", "SNES", "the orchestra stab every 16-bit game had", &[("chip", 0.75), ("wave", 1.0), ("sample", 0.0), ("amp d", 0.7), ("amp s", 0.0), ("amp r", 0.3), ("echo", 0.2)]),
    ("SNES Slap Bass", "SNES", "a funky slap bass sample (recorded at C2)", &[("chip", 0.75), ("wave", 1.0), ("sample", 0.0), ("sroot", 0.1667), ("amp d", 0.55), ("amp s", 0.3), ("amp r", 0.15)]),
    ("SNES Steel Drum", "SNES", "a bright steel pan for island levels", &[("chip", 0.75), ("wave", 1.0), ("sample", 0.0), ("amp d", 0.6), ("amp s", 0.0), ("amp r", 0.3), ("echo", 0.25)]),
    ("SNES Brass", "SNES", "a brass section sample, looped - heroic fanfares", &[("chip", 0.75), ("wave", 1.0), ("sample", 0.0), ("sloop", 1.0), ("amp a", 0.15), ("amp s", 0.85), ("amp r", 0.3), ("echo", 0.2), ("vib", 0.15), ("vib delay", 0.35)]),
    ("SNES Flute", "SNES", "a breathy flute sample with delayed vibrato", &[("chip", 0.75), ("wave", 1.0), ("sample", 0.0), ("sloop", 1.0), ("amp a", 0.2), ("amp s", 0.85), ("amp r", 0.3), ("vib", 0.2), ("vib delay", 0.4), ("echo", 0.3)]),
    ("Voice Ah", "SNES", "a sung 'ah' sample for chip-tune vocals", &[("chip", 0.75), ("wave", 1.0), ("sample", 0.0), ("sloop", 1.0), ("amp a", 0.25), ("amp s", 0.9), ("amp r", 0.4), ("vib", 0.25), ("vib delay", 0.3)]),
    // ── shimmer and arps
    ("Shimmer Arp", "Arp", "an add9 arpeggio over three octaves synced to the tempo, with a long ping-pong echo - glittering", &[("chip", 1.0), ("wave", 0.0), ("duty", 0.35), ("pwm", 0.3), ("arp", 0.6), ("arp shape", 1.0), ("arp oct", 1.0), ("arp speed", 0.4), ("sync", 1.0), ("echo", 0.45), ("echo time", 0.5), ("echo fb", 0.5), ("amp s", 0.7), ("width", 0.8)]),
    ("Hyper Arp", "Arp", "a very fast octave arpeggio - the chiptune shred", &[("chip", 0.0), ("wave", 0.0), ("duty", 0.33), ("arp", 0.2), ("arp shape", 0.375), ("arp oct", 1.0), ("arp speed", 0.0), ("amp s", 0.8)]),
    ("Held-Note Arp", "Arp", "hold a chord, hear it as a tempo-synced arpeggio of exactly those notes", &[("chip", 1.0), ("wave", 0.0), ("duty", 0.5), ("arp", 1.0), ("arp speed", 0.4), ("sync", 1.0), ("poly", 0.0), ("echo", 0.25), ("amp s", 0.75)]),
    ("Sparkle Sweep", "Arp", "a random arpeggio over a maj7 chord with an upward sweep on every note and echo - fairy dust", &[("chip", 0.5), ("wave", 0.0), ("duty", 0.0), ("arp", 0.8), ("arp shape", 0.625), ("arp oct", 1.0), ("arp speed", 0.2), ("sweep", 0.62), ("sweep time", 0.1), ("echo", 0.5), ("echo fb", 0.55), ("amp d", 0.35), ("amp s", 0.3), ("width", 0.9)]),
    ("Echo Pluck", "Arp", "a short triangle pluck with a tempo echo - soft rain", &[("chip", 1.0), ("wave", 0.2), ("amp d", 0.3), ("amp s", 0.0), ("amp r", 0.2), ("echo", 0.45), ("echo time", 0.35), ("echo fb", 0.5)]),
    ("Chip Bell", "Arp", "a pulse with ring modulation and a fast decay - a little bell", &[("chip", 1.0), ("wave", 0.0), ("duty", 0.2), ("ring", 0.55), ("amp d", 0.5), ("amp s", 0.0), ("amp r", 0.35), ("echo", 0.3)]),
    ("Lo-Fi Square", "Arp", "a square crushed to 4 bits at a low rate - gritty handheld lead", &[("chip", 1.0), ("wave", 0.0), ("duty", 0.5), ("bits", 0.85), ("rate", 0.55), ("vib", 0.2), ("vib delay", 0.3), ("amp s", 0.75)]),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn render(c: &mut Chip, secs: f32) -> Vec<f32> {
        let mut out = vec![0f32; (44100.0 * secs) as usize * 2];
        for ch in out.chunks_mut(512) {
            c.render(ch);
        }
        out
    }

    #[test]
    fn every_preset_sounds_and_stays_sane() {
        for p in PRESETS {
            let mut c = Chip::new(44100.0);
            c.load_preset(p.0);
            c.note_on(60, 0.9);
            if p.0.contains("Held") || p.0.contains("C64 Arp") {
                c.note_on(64, 0.9);
                c.note_on(67, 0.9);
            }
            let o = render(&mut c, 0.6);
            c.note_off(60);
            let tail = render(&mut c, 0.3);
            let pk = o.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(o.iter().chain(tail.iter()).all(|x| x.is_finite()), "{}", p.0);
            // samples are optional (the bundled folder may be missing in a bare checkout)
            if !(p.1 == "SNES" && c.bundled.is_empty()) {
                assert!(pk > 0.01, "{} is silent", p.0);
            }
            assert!(pk <= 1.0, "{}: {pk}", p.0);
            assert!(!p.2.is_empty(), "{} needs a description", p.0);
        }
    }

    #[test]
    fn the_arpeggiator_plays_the_chord_and_the_sweep_moves_the_pitch() {
        // zero crossings per second ~ pitch
        let zc = |v: &[f32]| v.chunks(2).map(|c| c[0]).collect::<Vec<_>>().windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        let mut c = Chip::new(44100.0);
        c.set_param("chip", 1.0);
        c.set_param("wave", 0.2);
        c.set_param("amp s", 1.0);
        c.note_on(48, 1.0);
        let plain = render(&mut c, 0.5);
        let mut c2 = Chip::new(44100.0);
        c2.set_param("chip", 1.0);
        c2.set_param("wave", 0.2);
        c2.set_param("amp s", 1.0);
        c2.set_param("arp", 0.2);
        c2.set_param("arp shape", 0.0);
        c2.note_on(48, 1.0);
        let arp = render(&mut c2, 0.5);
        // a major arpeggio averages higher than its root
        assert!(zc(&arp) > zc(&plain) + 5, "{} vs {}", zc(&arp), zc(&plain));
        let mut c3 = Chip::new(44100.0);
        c3.set_param("chip", 1.0);
        c3.set_param("wave", 0.2);
        c3.set_param("amp s", 1.0);
        c3.set_param("sweep", 0.9);
        c3.note_on(48, 1.0);
        let sw = render(&mut c3, 0.5);
        let half = sw.len() / 2;
        assert!(zc(&sw[half..]) > zc(&sw[..half]) * 2, "the sweep rises");
    }

    /// not a test: writes one patch file per preset into $CHIP_OUT (instruments/Chip), so the song generator can pick them
    #[test]
    #[ignore]
    fn dump_preset_patches() {
        let Ok(dir) = std::env::var("CHIP_OUT") else { return };
        std::fs::create_dir_all(&dir).unwrap();
        // run from LingStationX, so the bundled samples (and each preset's choice among them) are found
        if let Some(root) = std::path::Path::new(&dir).parent().and_then(|p| p.parent()) {
            let _ = std::env::set_current_dir(root);
        }
        for p in PRESETS {
            let mut c = Chip::new(44100.0);
            c.load_preset(p.0);
            let d = defaults();
            let mut body = String::new();
            for (i, (n, v)) in c.params().into_iter().enumerate() {
                if (v - d[i]).abs() > 1e-4 {
                    body.push_str(&format!("{}={:.4}
", n.replace(' ', "_"), v));
                }
            }
            let text = format!("# Ling Chip patch - edit freely; load from the Instrument tab
# {}
name = \"{}\"
type = chip
{}", p.2, p.0, body);
            std::fs::write(std::path::Path::new(&dir).join(format!("Chip_{}.ling", p.0.replace(' ', "_"))), text).unwrap();
        }
    }

    #[test]
    fn held_notes_arpeggiate_in_mono() {
        let mut c = Chip::new(44100.0);
        c.load_preset("Held-Note Arp");
        c.note_on(60, 1.0);
        c.note_on(64, 1.0);
        c.note_on(67, 1.0);
        assert_eq!(c.arp_notes(60), vec![60, 64, 67]);
        assert_eq!(c.voices.iter().filter(|v| v.stage != Stage::Idle).count(), 1, "one chip channel");
    }
}
