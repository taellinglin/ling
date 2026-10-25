//! Ling Prism — a polyphonic analog-style synth for big electronic sounds.
//!
//! Voice: two morphing wavetable oscillators (A, B) with unison, phase warp and an *interaction* between them
//! (add, ring, phase-mod, hard sync, xor, min), plus sub, noise and an analog "hum" drone, into a stereo multimode
//! filter and a note-tuned comb resonator, an amp envelope, and analog drift. Eight multi-point envelopes (MSEG) and two
//! LFOs reach almost every parameter through a modulation matrix. A harmonizer, phaser, ensemble chorus, ping-pong
//! delay and reverb finish the sound. Any scale can be played: equal divisions of the octave, just intonation,
//! gamelan, Bohlen-Pierce, or twelve hand-tuned notes.
//!
//! Like the other Ling instruments the whole state is flat, named, 0..1 parameters (selectors are `index / (n-1)`),
//! so patches, project saves and automation need nothing special.
use super::fx::{Effect, Reverb};
use super::{midi_hz, Instrument, Rng, TAU};
use std::collections::HashMap;
use std::sync::OnceLock;

macro_rules! core_params {
    ($($id:ident $name:literal $def:literal;)*) => {
        #[allow(non_camel_case_types, dead_code)]
        #[derive(Clone, Copy)]
        #[repr(usize)]
        enum P { $($id),*, COUNT }
        const CORE: &[(&str, f32)] = &[$(($name, $def)),*];
    };
}

core_params! {
    A_WAVE "a wave" 0.3;
    A_WARP "a warp" 0.0;
    A_UNI "a uni" 0.34;
    A_DET "a det" 0.25;
    A_SPRD "a sprd" 0.6;
    A_OCT "a oct" 0.5;
    A_FINE "a fine" 0.5;
    A_LVL "a lvl" 0.8;
    B_WAVE "b wave" 0.5;
    B_WARP "b warp" 0.0;
    B_UNI "b uni" 0.17;
    B_DET "b det" 0.3;
    B_SPRD "b sprd" 0.5;
    B_OCT "b oct" 0.5;
    B_SEMI "b semi" 0.5;
    B_FINE "b fine" 0.5;
    B_LVL "b lvl" 0.0;
    MODE "mode" 0.0;
    AMT "amt" 0.5;
    SUB "sub" 0.2;
    SUBW "sub w" 0.0;
    NOISE "noise" 0.0;
    NOISEC "noise c" 0.3;
    HUM "hum" 0.0;
    HUMD "hum d" 0.4;
    FTYPE "ftype" 0.25;
    CUT "cut" 0.7;
    RES "res" 0.2;
    DRV "drive" 0.1;
    KEYTR "keytr" 0.5;
    FENV "fenv" 0.62;
    COMB "comb" 0.0;
    COMBT "comb t" 0.5;
    COMBFB "comb fb" 0.5;
    AMP_A "amp a" 0.02;
    AMP_D "amp d" 0.4;
    AMP_S "amp s" 0.8;
    AMP_R "amp r" 0.3;
    FLT_A "flt a" 0.02;
    FLT_D "flt d" 0.4;
    FLT_S "flt s" 0.3;
    FLT_R "flt r" 0.3;
    L1RATE "l1 rate" 0.4;
    L1SHP "l1 shp" 0.0;
    L2RATE "l2 rate" 0.3;
    L2SHP "l2 shp" 0.25;
    GLIDE "glide" 0.0;
    BEND "bend" 0.5;
    WHL "whl" 0.0;
    DRIFT "drift" 0.3;
    VEL "vel" 0.5;
    OUT "out" 0.7;
    TEMPO "tempo" 0.57;
    SCALE "scale" 0.0;
    ROOT "root" 0.0;
    STRETCH "stretch" 0.5;
    T0 "t0" 0.5;
    T1 "t1" 0.5;
    T2 "t2" 0.5;
    T3 "t3" 0.5;
    T4 "t4" 0.5;
    T5 "t5" 0.5;
    T6 "t6" 0.5;
    T7 "t7" 0.5;
    T8 "t8" 0.5;
    T9 "t9" 0.5;
    T10 "t10" 0.5;
    T11 "t11" 0.5;
    HZMIX "hz mix" 0.0;
    HZ1 "hz 1" 0.75;
    HZ2 "hz 2" 0.6;
    PHMIX "ph mix" 0.0;
    PHRATE "ph rate" 0.3;
    PHDEP "ph dep" 0.6;
    PHFB "ph fb" 0.4;
    PHST "ph st" 0.67;
    CHMIX "ch mix" 0.0;
    CHRATE "ch rate" 0.3;
    DLMIX "dl mix" 0.0;
    DLTIME "dl time" 0.57;
    DLFB "dl fb" 0.4;
    RVMIX "rv mix" 0.15;
    RVSIZE "rv size" 0.6;
}

pub const NMSEG: usize = 8;
pub const MSEG_PTS: usize = 16;
/// per MSEG: len, time, loop, int, then 16 x (time) and 16 y (level)
pub const MSEG_STRIDE: usize = 4 + 2 * MSEG_PTS;
pub const NSRC: usize = NMSEG + 5;
pub const NDST: usize = 20;
const NCORE: usize = P::COUNT as usize;
pub const MSEG_BASE: usize = NCORE;
pub const MAT_BASE: usize = MSEG_BASE + NMSEG * MSEG_STRIDE;
pub const NPARAM: usize = MAT_BASE + NSRC * NDST;
pub const SRC_NAMES: [&str; NSRC] = ["m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "l1", "l2", "vel", "key", "whl"];
pub const DST_NAMES: [&str; NDST] = ["pit", "awv", "awp", "alv", "bwv", "bwp", "blv", "amt", "cut", "res", "drv", "cmb", "sub", "nos", "pan", "det", "bpt", "amp", "hum", "spr"];

struct Names {
    list: Vec<&'static str>,
    index: HashMap<&'static str, usize>,
}

fn names() -> &'static Names {
    static N: OnceLock<Names> = OnceLock::new();
    N.get_or_init(|| {
        let mut list: Vec<&'static str> = CORE.iter().map(|c| c.0).collect();
        for m in 0..NMSEG {
            for s in ["len", "time", "loop", "int"] {
                list.push(Box::leak(format!("m{} {}", m + 1, s).into_boxed_str()));
            }
            for k in 0..MSEG_PTS {
                list.push(Box::leak(format!("m{} x{}", m + 1, k).into_boxed_str()));
            }
            for k in 0..MSEG_PTS {
                list.push(Box::leak(format!("m{} y{}", m + 1, k).into_boxed_str()));
            }
        }
        for s in SRC_NAMES {
            for d in DST_NAMES {
                list.push(Box::leak(format!("x {s}{d}").into_boxed_str()));
            }
        }
        let index = list.iter().enumerate().map(|(i, n)| (*n, i)).collect();
        Names { list, index }
    })
}

pub fn param_names() -> &'static [&'static str] {
    &names().list
}

fn defaults() -> Vec<f32> {
    let mut v = vec![0.5f32; NPARAM];
    for (i, c) in CORE.iter().enumerate() {
        v[i] = c.1;
    }
    for m in 0..NMSEG {
        let b = MSEG_BASE + m * MSEG_STRIDE;
        v[b] = 3.0 / 14.0; // 5 points
        v[b + 1] = 0.45;
        v[b + 2] = 0.0;
        v[b + 3] = 0.5;
        for k in 0..MSEG_PTS {
            v[b + 4 + k] = k as f32 / 15.0;
            v[b + 4 + MSEG_PTS + k] = 0.0;
        }
    }
    v
}

// ── tuning ────────────────────────────────────────────────────────────────────

pub const SCALE_NAMES: [&str; 16] = [
    "12 tet", "19 edo", "22 edo", "24 edo", "31 edo", "17 edo", "53 edo", "just 5-limit", "pythagorean", "meantone", "bohlen-pierce", "slendro", "pelog", "harmonic 8-16", "custom 12", "whole tone",
];

/// (steps in cents within one period, period in cents) for scale `i`; `cust` are the 12 hand-tuned offsets
fn scale_steps(i: usize, cust: &[f32; 12]) -> (Vec<f32>, f32) {
    let edo = |n: usize| ((0..n).map(|k| 1200.0 * k as f32 / n as f32).collect::<Vec<f32>>(), 1200.0);
    match i {
        1 => edo(19),
        2 => edo(22),
        3 => edo(24),
        4 => edo(31),
        5 => edo(17),
        6 => edo(53),
        7 => (vec![0.0, 111.73, 203.91, 315.64, 386.31, 498.04, 590.22, 701.96, 813.69, 884.36, 1017.60, 1088.27], 1200.0),
        8 => (vec![0.0, 90.22, 203.91, 294.13, 407.82, 498.04, 611.73, 701.96, 792.18, 905.87, 996.09, 1109.78], 1200.0),
        9 => (vec![0.0, 76.05, 193.16, 310.26, 386.31, 503.42, 579.47, 696.58, 772.63, 889.74, 1006.84, 1082.89], 1200.0),
        10 => ((0..13).map(|k| 1901.955 * k as f32 / 13.0).collect(), 1901.955),
        11 => ((0..5).map(|k| 240.0 * k as f32).collect(), 1200.0),
        12 => (vec![0.0, 120.0, 270.0, 540.0, 670.0, 800.0, 950.0], 1200.0),
        13 => ((8..16).map(|r| 1200.0 * (r as f32 / 8.0).log2()).collect(), 1200.0),
        14 => ((0..12).map(|k| k as f32 * 100.0 + cust[k]).collect(), 1200.0),
        15 => ((0..6).map(|k| 200.0 * k as f32).collect(), 1200.0),
        _ => edo(12),
    }
}

#[derive(Clone)]
struct Tuning {
    steps: Vec<f32>,
    period: f32,
    root: usize,
}
impl Tuning {
    /// frequency of (fractional) MIDI key `k`
    fn freq(&self, k: f32) -> f32 {
        let n = self.steps.len() as f32;
        let d = k - (60 + self.root) as f32;
        let oct = (d / n).floor();
        let r = d - oct * n;
        let i = (r.floor() as usize).min(self.steps.len() - 1);
        let frac = r - i as f32;
        let next = if i + 1 < self.steps.len() { self.steps[i + 1] } else { self.period };
        let cents = oct * self.period + self.steps[i] + frac * (next - self.steps[i]);
        midi_hz((60 + self.root) as f32) * 2f32.powf(cents / 1200.0)
    }
}

// ── wavetables ────────────────────────────────────────────────────────────────

const TN: usize = 2048;
const LEVELS: [usize; 7] = [64, 32, 16, 8, 4, 2, 1];
const NTABLE: usize = 8;

/// harmonic amplitudes (1-based) of table `t`
fn table_harmonics(t: usize) -> Vec<f32> {
    let mut a = vec![0.0f32; 65];
    for n in 1..=64usize {
        let nf = n as f32;
        a[n] = match t {
            0 => {
                if n == 1 {
                    1.0
                } else {
                    0.0
                }
            }
            1 => {
                // triangle
                if n % 2 == 1 {
                    (if (n / 2) % 2 == 0 { 1.0 } else { -1.0 }) / (nf * nf)
                } else {
                    0.0
                }
            }
            2 => 1.0 / nf,
            3 => {
                if n % 2 == 1 {
                    1.0 / nf
                } else {
                    0.0
                }
            }
            4 => {
                // narrow pulse (25%): hollow, nasal
                (std::f32::consts::PI * nf * 0.25).sin() / nf
            }
            5 => {
                // organ drawbars 1 2 3 4 6 8
                match n {
                    1 => 1.0,
                    2 => 0.7,
                    3 => 0.5,
                    4 => 0.45,
                    6 => 0.3,
                    8 => 0.25,
                    _ => 0.0,
                }
            }
            6 => {
                // bell: prime-ish partials
                match n {
                    1 => 1.0,
                    2 => 0.55,
                    3 => 0.7,
                    5 => 0.5,
                    7 => 0.4,
                    11 => 0.3,
                    13 => 0.22,
                    17 => 0.15,
                    _ => 0.0,
                }
            }
            _ => {
                // spectral comb: strong fundamental, a cluster around 8..12 (a vocal-ish formant), air above
                let base = if n == 1 { 1.0 } else { 0.0 };
                base + (-(((nf - 10.0) / 2.5).powi(2))).exp() * 0.9 / nf.sqrt() + (-(((nf - 3.0) / 1.0).powi(2))).exp() * 0.35
            }
        };
    }
    a
}

struct Tables {
    /// [table][level] -> one cycle
    w: Vec<Vec<Vec<f32>>>,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let sin_t: Vec<f32> = (0..TN).map(|i| (TAU * i as f32 / TN as f32).sin()).collect();
        let mut w = vec![];
        for t in 0..NTABLE {
            let h = table_harmonics(t);
            let build = |hmax: usize| -> Vec<f32> { (0..TN).map(|i| (1..=hmax).map(|n| h[n] * sin_t[(n * i) % TN]).sum()).collect() };
            let peak = build(64).iter().fold(1e-6f32, |m, x| m.max(x.abs()));
            w.push(LEVELS.iter().map(|&hm| build(hm).into_iter().map(|x| x / peak).collect()).collect());
        }
        Tables { w }
    })
}

#[inline]
fn rd(tab: &[f32], ph: f32) -> f32 {
    let x = ph * TN as f32;
    let fl = x.floor();
    let i = fl as usize & (TN - 1);
    let f = x - fl;
    let (a, b) = (tab[i], tab[(i + 1) & (TN - 1)]);
    a + (b - a) * f
}

/// phase warp: bends the cycle so the same table gets pulse-width / skew colours
#[inline]
fn warp(ph: f32, w: f32) -> f32 {
    if w < 0.001 {
        return ph;
    }
    let s = 0.5 - 0.45 * w; // where the first half of the cycle ends
    if ph < s {
        ph * 0.5 / s
    } else {
        0.5 + (ph - s) * 0.5 / (1.0 - s)
    }
}

// ── envelope, filters, delay lines ─────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Idle,
    Att,
    Dec,
    Sus,
    Rel,
}
#[derive(Clone)]
struct Env {
    stage: Stage,
    level: f32,
}
impl Env {
    fn new() -> Self {
        Env { stage: Stage::Idle, level: 0.0 }
    }
    fn on(&mut self) {
        self.stage = Stage::Att;
    }
    fn off(&mut self) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Rel;
        }
    }
    fn tick(&mut self, a: f32, d: f32, s: f32, r: f32, dt: f32) -> f32 {
        match self.stage {
            Stage::Idle => {}
            Stage::Att => {
                self.level += dt / a.max(1e-4);
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Dec;
                }
            }
            Stage::Dec => {
                self.level += (s - self.level) * (1.0 - (-dt * 4.0 / d.max(1e-3)).exp());
                if (self.level - s).abs() < 0.0005 {
                    self.level = s;
                    self.stage = Stage::Sus;
                }
            }
            Stage::Sus => self.level = s,
            Stage::Rel => {
                self.level -= self.level * (1.0 - (-dt * 4.0 / r.max(1e-3)).exp());
                if self.level < 0.0002 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }
}

#[derive(Clone, Copy, Default)]
struct Svf {
    ic1: f32,
    ic2: f32,
}
impl Svf {
    /// (lp, bp, hp)
    #[inline]
    fn run(&mut self, x: f32, g: f32, k: f32) -> (f32, f32, f32) {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, v1, x - k * v1 - v2)
    }
}

#[derive(Clone)]
struct Delay {
    buf: Vec<f32>,
    w: usize,
}
impl Delay {
    fn new(n: usize) -> Self {
        Delay { buf: vec![0.0; n.max(16)], w: 0 }
    }
    #[inline]
    fn push(&mut self, x: f32) {
        self.buf[self.w] = x;
        self.w = (self.w + 1) % self.buf.len();
    }
    #[inline]
    fn read(&self, d: f32) -> f32 {
        let n = self.buf.len();
        let d = d.clamp(1.0, (n - 2) as f32);
        let p = self.w as f32 - d;
        let p = if p < 0.0 { p + n as f32 } else { p };
        let i = p as usize % n;
        let f = p - p.floor();
        let (a, b) = (self.buf[i], self.buf[(i + 1) % n]);
        a + (b - a) * f
    }
    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|x| *x = 0.0);
    }
}

// ── voice ────────────────────────────────────────────────────────────────────

const MAXVOICES: usize = 16;
const MAXUNI: usize = 7;
const CTRL: usize = 8;

#[derive(Clone)]
struct Voice {
    active: bool,
    gate: bool,
    key: u8,
    age: u64,
    vel: f32,
    note: f32,
    target: f32,
    ph_a: [f32; MAXUNI],
    ph_b: [f32; MAXUNI],
    ph_sub: f32,
    ph_hum: [f32; 4],
    t: f32,
    amp: Env,
    flt: Env,
    f1: [Svf; 2],
    f1b: [Svf; 2],
    comb: [Delay; 2],
    noise_lp: f32,
    noise_lp2: f32,
    drift: f32,
    drift_c: f32,
    // control-rate state
    m: [f32; NDST],
    gains_a: [(f32, f32); MAXUNI],
    gains_b: [(f32, f32); MAXUNI],
    det_a: [f32; MAXUNI],
    det_b: [f32; MAXUNI],
    g: f32,
    k: f32,
    amp_v: f32,
}

impl Voice {
    fn new(sr: f32) -> Voice {
        Voice {
            active: false,
            gate: false,
            key: 0,
            age: 0,
            vel: 0.8,
            note: 60.0,
            target: 60.0,
            ph_a: [0.0; MAXUNI],
            ph_b: [0.0; MAXUNI],
            ph_sub: 0.0,
            ph_hum: [0.0; 4],
            t: 0.0,
            amp: Env::new(),
            flt: Env::new(),
            f1: [Svf::default(); 2],
            f1b: [Svf::default(); 2],
            comb: [Delay::new((sr / 12.0) as usize + 8), Delay::new((sr / 12.0) as usize + 8)],
            noise_lp: 0.0,
            noise_lp2: 0.0,
            drift: 0.0,
            drift_c: 0.0,
            m: [0.0; NDST],
            gains_a: [(0.0, 0.0); MAXUNI],
            gains_b: [(0.0, 0.0); MAXUNI],
            det_a: [0.0; MAXUNI],
            det_b: [0.0; MAXUNI],
            g: 0.1,
            k: 1.0,
            amp_v: 0.0,
        }
    }
}

fn uni_count(v: f32) -> usize {
    ((v * 6.0).round() as usize).min(6) + 1
}

/// symmetric positions -1..1 for `n` layers
fn spread_pos(i: usize, n: usize) -> f32 {
    if n <= 1 {
        0.0
    } else {
        (i as f32 / (n - 1) as f32) * 2.0 - 1.0
    }
}

pub struct Prism {
    sr: f32,
    v: Vec<f32>,
    tuning: Tuning,
    tuning_dirty: bool,
    /// non-zero matrix cells: (source, destination, amount)
    cells: Vec<(usize, usize, f32)>,
    cells_dirty: bool,
    voices: Vec<Voice>,
    clock: u64,
    last_note: f32,
    rng: Rng,
    lfo_ph: [f32; 2],
    lfo_hold: [f32; 2],
    lfo_val: [f32; 2],
    bend_s: f32,
    // effects
    hz: [[PitchShift; 2]; 2],
    ph_state: [[f32; 8]; 2],
    ph_fb: [f32; 2],
    ph_lfo: f32,
    ch: [Delay; 2],
    ch_lfo: f32,
    dl: [Delay; 2],
    dl_lp: [f32; 2],
    verb: Reverb,
    scr: Vec<f32>,
    scr2: Vec<f32>,
    silent: u32,
}

#[derive(Clone)]
struct PitchShift {
    buf: Vec<f32>,
    w: usize,
    ph: f32,
}
impl PitchShift {
    fn new(sr: f32) -> Self {
        PitchShift { buf: vec![0.0; (sr * 0.12) as usize + 8], w: 0, ph: 0.0 }
    }
    #[inline]
    fn process(&mut self, x: f32, ratio: f32, win: f32) -> f32 {
        let n = self.buf.len();
        self.buf[self.w] = x;
        self.w = (self.w + 1) % n;
        self.ph += (1.0 - ratio) / win;
        self.ph -= self.ph.floor();
        let tap = |ph: f32, me: &PitchShift| -> f32 {
            let d = ph * win + 4.0;
            let p = me.w as f32 - d;
            let p = if p < 0.0 { p + n as f32 } else { p };
            let i = p as usize % n;
            let f = p - p.floor();
            let (a, b) = (me.buf[i], me.buf[(i + 1) % n]);
            a + (b - a) * f
        };
        let p2 = (self.ph + 0.5).fract();
        let g1 = 0.5 - 0.5 * (TAU * self.ph).cos();
        let g2 = 0.5 - 0.5 * (TAU * p2).cos();
        tap(self.ph, self) * g1 + tap(p2, self) * g2
    }
}

impl Prism {
    pub const PRESETS: [&'static str; 385] = [
        "Supersaw Lead", "Rolling Bass", "Trance Pluck", "Uplift Pad", "Gated Pad", "Offbeat Bass", "Deep Stab", "Warm Sub", "Dream Keys", "Dub Chord", "Deep Pluck", "Disco Strings",
        "Octave Bass", "Funk Pluck", "Phaser Pad", "Brass Stab", "Witch Drag", "Ritual Bell", "Dark Choir", "Void Sub", "Cursed Arp", "Black Mass", "Hyper Lead", "Glitch Pluck",
        "Bubble Bass", "Sugar Stack", "Chipmunk Arp", "Candy Pad", "19 EDO Glass", "Slendro Pad", "Bohlen Drone", "Quarter Lead", "Analog Hum", "Tape Dream", "Radio Ghost", "Prism Bloom",
        "Classic Trance Pad", "Classic Trance Pad Two", "Dark Pad One", "Dark Pad Two", "Dark Pad Three", "Dreamer Pad", "Dreamer Pad Wide", "Earth Pad", "Ether Pad", "Tender Gated Pad", "Pedro Gated Pad", "Choir Pad Classic",
        "Oldskool Choir Pad", "Polar Pad", "Nebula Pad", "Rainy Day Pad", "Spacey Pad", "Soft Pad", "Strange Pad", "Netherworld Pad", "Monk Pad", "Fire Pad", "Heavy Pad", "Pad of Many",
        "Electric Pad", "Organ Pad", "Hardstyle Lead", "Hardstyle Lead Tuned", "Hardstyle Lead Two", "Simple Trance Lead", "Hoover Dark", "Hoover Heaven", "Hoover Jump", "Hoover Epic", "Glass Lead With Bell", "Dance Lead Phobia",
        "Retro Lead", "Spacer Lead", "Funky Lead", "Shoop Lead", "Dubble Lead", "Happy Hardcore Lead", "Panama Synth", "Panama Synth Four", "Super Synth", "Super Synth Hard", "Perfecto Synth", "Heard It Synth",
        "Hard Synth", "Dirty Lead", "Screaming Electro", "Bass Donk", "Bass Donk Hard", "Offbeat Bass Two", "Offbeat Bass Three", "Heavy Saw Bass", "Fuzzy Synth Bass", "Sine Bass", "Mega Bass", "Fallout Bass",
        "Punch Bass", "Electric Bass", "Bass Guitar Pluck", "Fretless Bass", "Flute Bass", "Circulating Bass", "Nasty Bass", "Knarzy Bass", "Dub Steppy Bass", "Dub Steppy Bass Two", "Greasy Wobble", "Basic Wobble",
        "Neuro Bass", "Tunnel Bass", "Space Bass", "Bajo Growl One", "Bajo Growl Laser", "Bajo Psy", "Bajo Melbourne", "Bajo Trance", "Bajo Techno", "Bajo Sweat", "Analog Bass One", "Seq Or Die Bass",
        "Face Eater", "Knarky Arp One", "Knarky Arp Two", "Knarky Arp Three", "Anero Synth Arp", "Slow Lizzy Arp", "Peace Synth Arp", "Poltergeist Arp", "Special Arp", "Tiny Arp", "Dreamers Synth Arp", "Tronic Arp",
        "Cubist Arp", "Zombitronic Arp", "Machine Wriggle Arp", "Nyan Sequence", "Plankton Sequence", "Filter Sequence", "Bass Machine Sequence", "Devils Piano", "Devils Piano Low", "Heavens Piano", "Real Piano", "Broken Piano",
        "Wannabe Piano", "Moonlight EP", "Rhodes Dream", "Hammond Drawbar", "Hammond Two", "Dirty Organ", "Cathedral Orgel", "Church Organ", "Steampunk Organ", "Accordiola", "Clav", "Harp of Illusion",
        "Church Bell Toll", "Dance Bell", "Bells From Distance", "Fairy Bells", "Ice Bells", "Gong Hit", "Crystal Sphere", "Labyrinth Bell", "String Orkestro", "Symphonic Strings", "Lazy String", "Dramatic Strings",
        "Basson Reed", "Flutes Breath", "Simple Harmonica", "Train Horn Mess", "Stab Laser Sword", "Stab Fireworks", "Stab Dark Noise", "Stab Dinosaur Walk", "Stab Unlocked", "Stab Broken TV", "Stab Buggy", "Stab Eye",
        "Kick Basic", "Kick Building Site", "Kick Nu Style", "Kick 808 Long", "Kick Trance", "Kick Psy", "Hardcore Kick", "Kick Machine", "Hihat Closed", "Hihat Open", "Perc Metal", "Perc Snap",
        "Snare Thin", "Clap Classic", "Moon Drum", "Helicopter", "Motor Going Up", "Motor Going Down", "Rain Drops", "Broken TV FX", "Rise of Terror", "Drop FX", "Power Up", "Blue Screen",
        "Haunted Place", "Ice Rain", "Lynchian Texture", "Magic Swamp", "Steam Starship", "Snake Stop", "Space Distortion", "Atlantis Pad", "Supersaw Pad", "Trance Pad Wash", "Starlight Pad", "Aquarius Pad",
        "Sunroom Pad", "Sherpad", "Oblivion Pad", "Dreamland Pad", "Gotham Pad", "Amber Pad", "Mirage Pad", "Equinox Pad", "Voice of Mist", "Moving Choir", "Old Vox", "Ahh Choir",
        "Chorus Ensemble", "Pluck Mau Style", "Pluck Rhodes", "Pluck Piano E", "Pluck Tropical", "Pluck Strobe", "Pluck Vocal", "Pluck Metallic", "Pluck Detuned", "Pluck Warm", "Pluck Sweet", "Pluck Heaven",
        "Pluck Neutron", "Pluck Space", "Strum Chord Pluck", "Supersaw Lead Two", "Hoover Lead Harmor", "Dubstep Lead", "Future Bass Lead", "Wah Lead", "Brass Lead", "Gamer Lead", "Arcade Lead", "Robot Lead",
        "Waspinator Lead", "Monolith Lead", "Classic Rave Stab", "Viking Stab", "Shorty Stab", "Ploc Stab", "Stab Fall", "Growler Synth", "Grunge Synth", "Filthy Synth", "Rachel Synth", "Samsara Synth",
        "Tranquility Pad", "Butterfly Pad", "Space Dust Pad", "8th Note Chords", "Ridge Groove", "Intro Sapphire", "Intro Quartz", "Intro Jade", "Intro Gold Pluck", "Intro Friendly Drip", "Intro Smoothy", "Intro Water",
        "Intro Freakenstein", "Pad of Many 31 EDO", "Bell 22 Shruti", "Pluck Just Intonation", "Pelog Dreamers", "Hyper Glitch Lead", "Pixel Pop Lead", "Bubble Pluck Pop", "Sugar Rush Arp", "Autotune Stab", "Gum Bass", "Pitched Vox Lead",
        "Nightcore Saw", "Crash Pad Hyper", "Glitter Bell", "Hyper 808", "Rainbow Chord Pop", "Deep House Chord", "Warm Organ Stab", "Rolling Deep Bass", "Sub Pulse Deep", "Dub Stab Deep", "Velvet Pad", "Rhodes House",
        "Pluck Chord Deep", "Late Night Pad", "Piano Stab Italo", "Chord Smear House", "Deep Lead Whisper", "Disco Strings Lush", "Funky Clav Stab", "Octave Disco Bass", "Philly Strings", "Guitar Chop Disco", "Brass Section Disco", "Disco Lead Whistle",
        "Moroder Arp", "Nu Disco Pad", "Slap Bass Synth", "Glitter Keys Disco", "Disco Chord Stab", "Eurodance Lead", "Rave Piano Stab", "Dance Pad Anthem", "Piano House Chord", "Dance Bass Pump", "Rave Organ Stab", "Club Pluck",
        "Euro Arp", "Diva Lead Synth", "Hands Up Lead", "Bounce Bass", "Dance Whistle", "Big Room Lead", "Festival Supersaw", "Future Bass Chord", "Drop Growl Bass", "Riser Sweep EDM", "Pluck Lead Drop", "Progressive Pad",
        "Anthem Stab", "Sidechain Pad", "Chain Bass EDM", "Brostep Lead", "EDM Pluck Arp", "Industrial Bass", "Metal Clang Lead", "Machine Drone", "Rust Pad", "Factory Stab", "Noise Seq Industrial", "Distorted Kick Tone",
        "Gnash Lead", "Grinder Arp", "Concrete Pad", "Ring Mod Machine", "Cold Steel Bell", "Deep Space Drone", "Nebula Wash", "Orbit Pad", "Cosmic Choir", "Event Horizon", "Aurora Pad", "Solar Wind",
        "Zero Gravity", "Crystal Cave", "Satellite Ping", "Interstellar Bell", "Dark Matter", "Stellar Swell", "Lunar Surface", "Neon Lead", "Outrun Bass", "Retrowave Pad", "Sunset Arp", "Eighties Brass Stab",
        "Night Drive Lead", "Chrome Pad", "Synthwave Pluck", "Arcade Bass", "Miami Chord", "Dusty Rhodes", "Vinyl Pad", "Chill Keys", "Tape Pluck", "Lo-Fi Bell", "Mellow Sub", "Warm Wobble Lead",
        "Late Study Chord",
    ];

    pub fn new(sr: f32) -> Self {
        let mut verb = Reverb::new(sr);
        verb.set_param("mix", 1.0);
        verb.set_param("damp", 0.4);
        let mk2 = || [PitchShift::new(sr), PitchShift::new(sr)];
        let mut s = Prism {
            sr,
            v: defaults(),
            tuning: Tuning { steps: vec![0.0], period: 1200.0, root: 0 },
            tuning_dirty: true,
            cells: vec![],
            cells_dirty: true,
            voices: (0..MAXVOICES).map(|_| Voice::new(sr)).collect(),
            clock: 0,
            last_note: 60.0,
            rng: Rng::new(0x1234abcd),
            lfo_ph: [0.0, 0.31],
            lfo_hold: [0.0; 2],
            lfo_val: [0.0; 2],
            bend_s: 0.0,
            hz: [mk2(), mk2()],
            ph_state: [[0.0; 8]; 2],
            ph_fb: [0.0; 2],
            ph_lfo: 0.0,
            ch: [Delay::new((sr * 0.06) as usize), Delay::new((sr * 0.06) as usize)],
            ch_lfo: 0.0,
            dl: [Delay::new((sr * 2.2) as usize), Delay::new((sr * 2.2) as usize)],
            dl_lp: [0.0; 2],
            verb,
            scr: vec![],
            scr2: vec![],
            silent: 0,
        };
        s.refresh();
        s
    }

    fn sel(&self, k: usize, n: usize) -> usize {
        ((self.v[k] * (n - 1) as f32).round() as usize).min(n - 1)
    }

    fn refresh(&mut self) {
        if self.tuning_dirty {
            let mut cust = [0.0f32; 12];
            for (i, c) in cust.iter_mut().enumerate() {
                *c = (self.v[P::T0 as usize + i] - 0.5) * 100.0;
            }
            let (steps, period) = scale_steps(self.sel(P::SCALE as usize, SCALE_NAMES.len()), &cust);
            let stretch = (self.v[P::STRETCH as usize] - 0.5) * 60.0;
            let n = steps.len() as f32;
            // a stretched period spreads its extra cents evenly over the steps
            let steps: Vec<f32> = steps.iter().enumerate().map(|(i, s)| s + stretch * i as f32 / n).collect();
            self.tuning = Tuning { steps, period: period + stretch, root: self.sel(P::ROOT as usize, 12) };
            self.tuning_dirty = false;
        }
        if self.cells_dirty {
            self.cells.clear();
            for s in 0..NSRC {
                for d in 0..NDST {
                    let a = (self.v[MAT_BASE + s * NDST + d] - 0.5) * 2.0;
                    if a.abs() > 0.004 {
                        self.cells.push((s, d, a));
                    }
                }
            }
            self.cells_dirty = false;
        }
    }

    fn mseg_value(&self, m: usize, t: f32, tempo_sec: f32) -> f32 {
        let _ = tempo_sec;
        let b = MSEG_BASE + m * MSEG_STRIDE;
        let n = ((self.v[b] * 14.0).round() as usize + 2).min(MSEG_PTS);
        let total = 0.05 * 2f32.powf(self.v[b + 1] * 8.3);
        let mode = ((self.v[b + 2] * 2.0).round() as usize).min(2);
        let int = ((self.v[b + 3] * 2.0).round() as usize).min(2);
        let mut u = t / total;
        u = match mode {
            1 => u - u.floor(),
            2 => {
                let f = u % 2.0;
                if f < 1.0 {
                    f
                } else {
                    2.0 - f
                }
            }
            _ => u.min(1.0),
        };
        let xs = &self.v[b + 4..b + 4 + n];
        let ys = &self.v[b + 4 + MSEG_PTS..b + 4 + MSEG_PTS + n];
        let xmax = xs[n - 1].max(1e-3);
        let mut i = 0;
        while i + 2 < n && u >= xs[i + 1] / xmax {
            i += 1;
        }
        let (x0, x1) = (xs[i] / xmax, xs[i + 1] / xmax);
        let f = if x1 > x0 { ((u - x0) / (x1 - x0)).clamp(0.0, 1.0) } else { 1.0 };
        match int {
            0 => ys[i],
            1 => ys[i] + (ys[i + 1] - ys[i]) * f,
            _ => {
                let f = (1.0 - (std::f32::consts::PI * f).cos()) * 0.5;
                ys[i] + (ys[i + 1] - ys[i]) * f
            }
        }
    }

    fn lfo_shape(shape: usize, ph: f32, hold: f32) -> f32 {
        match shape {
            0 => (TAU * ph).sin(),
            1 => 1.0 - 4.0 * (ph - 0.5).abs(),
            2 => 1.0 - 2.0 * ph,
            3 => {
                if ph < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            _ => hold,
        }
    }

    fn alloc_voice(&mut self) -> usize {
        if let Some(i) = self.voices.iter().position(|v| !v.active) {
            return i;
        }
        // steal: a releasing voice first (the quietest), else the oldest
        let rel = self.voices.iter().enumerate().filter(|(_, v)| !v.gate).min_by(|a, b| a.1.amp.level.partial_cmp(&b.1.amp.level).unwrap_or(std::cmp::Ordering::Equal)).map(|(i, _)| i);
        rel.unwrap_or_else(|| self.voices.iter().enumerate().min_by_key(|(_, v)| v.age).map(|(i, _)| i).unwrap_or(0))
    }

    pub fn preset_state(name: &str) -> Option<Vec<(String, f32)>> {
        static P: OnceLock<Vec<(String, Vec<(String, f32)>)>> = OnceLock::new();
        let all = P.get_or_init(|| {
            let mut out = vec![];
            let mut cur_name = String::new();
            let mut cur = String::new();
            let flush = |out: &mut Vec<(String, Vec<(String, f32)>)>, n: &str, t: &str| {
                if !n.is_empty() {
                    out.push((n.to_string(), crate::patch::parse(t).params));
                }
            };
            for line in include_str!("../prism_presets.txt").lines() {
                if line.starts_with('[') && line.ends_with(']') {
                    flush(&mut out, &cur_name, &cur);
                    cur_name = line[1..line.len() - 1].to_string();
                    cur.clear();
                } else {
                    cur.push_str(line);
                    cur.push('\n');
                }
            }
            flush(&mut out, &cur_name, &cur);
            out
        });
        all.iter().find(|(n, _)| n == name).map(|(_, p)| p.clone())
    }
}

impl Instrument for Prism {
    fn note_on(&mut self, key: u8, vel: f32) {
        self.note_on_ex(key, vel, 0.0, 0.5, 0.5);
    }
    fn note_on_ex(&mut self, key: u8, vel: f32, _pan: f32, _cutoff: f32, _reso: f32) {
        self.refresh();
        // the same key again retriggers its own voice
        let i = match self.voices.iter().position(|v| v.active && v.gate && v.key == key) {
            Some(i) => i,
            None => self.alloc_voice(),
        };
        self.clock += 1;
        let glide = self.v[P::GLIDE as usize] > 0.01;
        let any_active = self.voices.iter().any(|v| v.active && v.gate);
        let start = if glide && any_active { self.last_note } else { key as f32 };
        let drift_seed = self.rng.next();
        let v = &mut self.voices[i];
        let fresh = !v.active;
        v.active = true;
        v.gate = true;
        v.key = key;
        v.age = self.clock;
        v.vel = vel.clamp(0.0, 1.0);
        v.target = key as f32;
        v.note = start;
        v.t = 0.0;
        v.amp.on();
        v.flt.on();
        if fresh {
            for k in 0..MAXUNI {
                v.ph_a[k] = (k as f32 * 0.381 + drift_seed.abs()).fract();
                v.ph_b[k] = (k as f32 * 0.618 + 0.25).fract();
            }
            v.amp.level = 0.0;
            v.flt.level = 0.0;
            v.f1 = [Svf::default(); 2];
            v.f1b = [Svf::default(); 2];
            for d in v.comb.iter_mut() {
                d.clear();
            }
        }
        self.last_note = key as f32;
    }
    fn note_off(&mut self, key: u8) {
        // release the newest gated voice on this key
        if let Some(v) = self.voices.iter_mut().filter(|v| v.active && v.gate && v.key == key).max_by_key(|v| v.age) {
            v.gate = false;
            v.amp.off();
            v.flt.off();
        }
    }
    fn all_off(&mut self) {
        for v in self.voices.iter_mut() {
            v.gate = false;
            v.amp.off();
            v.flt.off();
        }
    }

    fn render(&mut self, out: &mut [f32]) {
        self.refresh();
        let frames = out.len() / 2;
        if frames == 0 {
            return;
        }
        let any_voice = self.voices.iter().any(|v| v.active);
        if !any_voice && self.silent as f32 > self.sr * 8.0 {
            return;
        }
        let sr = self.sr;
        let dt = 1.0 / sr;
        let tb = tables();
        let vv = self.v.clone();
        let p = |k: P| vv[k as usize];
        let bpm = 60.0 + p(P::TEMPO) * 140.0;
        let beat = 60.0 / bpm;
        let (oct_a, oct_b) = (self.sel(P::A_OCT as usize, 5) as f32 - 2.0, self.sel(P::B_OCT as usize, 5) as f32 - 2.0);
        let fine_a = (p(P::A_FINE) - 0.5) * 2.0;
        let b_semi = (p(P::B_SEMI) - 0.5) * 48.0;
        let fine_b = (p(P::B_FINE) - 0.5) * 2.0;
        let (na, nb) = (uni_count(p(P::A_UNI)), uni_count(p(P::B_UNI)));
        let mode = self.sel(P::MODE as usize, 6);
        let ftype = self.sel(P::FTYPE as usize, 5);
        let glide_k = {
            let g = p(P::GLIDE);
            1.0 - (-dt / (0.002 + g * g * 0.8)).exp()
        };
        let amp_t = (0.002 + p(P::AMP_A).powi(2) * 3.0, 0.01 + p(P::AMP_D).powi(2) * 4.0, p(P::AMP_S), 0.01 + p(P::AMP_R).powi(2) * 5.0);
        let flt_t = (0.002 + p(P::FLT_A).powi(2) * 3.0, 0.01 + p(P::FLT_D).powi(2) * 4.0, p(P::FLT_S), 0.01 + p(P::FLT_R).powi(2) * 5.0);
        let vel_sens = p(P::VEL);
        let drift_amt = p(P::DRIFT);
        let bend_target = (p(P::BEND) - 0.5) * 2.0 * 2.0;
        let whl = p(P::WHL);
        let cells = self.cells.clone();
        let l1_hz = 0.05 * 2f32.powf(p(P::L1RATE) * 9.0);
        let l2_hz = 0.05 * 2f32.powf(p(P::L2RATE) * 9.0);
        let (l1_shp, l2_shp) = (self.sel(P::L1SHP as usize, 5), self.sel(P::L2SHP as usize, 5));
        let tuning = self.tuning.clone();
        let hum_lvl = p(P::HUM);
        let sub_lvl = p(P::SUB);
        let sub_tri = p(P::SUBW) > 0.5;
        let noise_lvl = p(P::NOISE);
        let noise_c = p(P::NOISEC);
        let comb_t = (p(P::COMBT) - 0.5) * 48.0;
        self.scr.resize(out.len(), 0.0);
        self.scr2.resize(out.len(), 0.0);
        let mut dry = vec![0.0f32; out.len()];
        let mut ctrl_count = 0usize;

        for i in 0..frames {
            // global LFOs and glide of the bend wheel
            for l in 0..2 {
                let hz = if l == 0 { l1_hz } else { l2_hz };
                self.lfo_ph[l] += hz * dt;
                if self.lfo_ph[l] >= 1.0 {
                    self.lfo_ph[l] -= 1.0;
                    self.lfo_hold[l] = self.rng.next();
                }
                self.lfo_val[l] = Prism::lfo_shape(if l == 0 { l1_shp } else { l2_shp }, self.lfo_ph[l], self.lfo_hold[l]);
            }
            self.bend_s += (bend_target - self.bend_s) * 0.002;
            let control = ctrl_count % CTRL == 0;
            ctrl_count += 1;
            let (mut sum_l, mut sum_r) = (0.0f32, 0.0f32);
            for vi in 0..self.voices.len() {
                if !self.voices[vi].active {
                    continue;
                }
                // ── control rate: modulation, filter coefficients, unison layout ──
                if control {
                    let (key_f, vel, t) = {
                        let v = &self.voices[vi];
                        (v.target, v.vel, v.t)
                    };
                    let mut src = [0.0f32; NSRC];
                    for m in 0..NMSEG {
                        if cells.iter().any(|c| c.0 == m) {
                            src[m] = self.mseg_value(m, t, beat);
                        }
                    }
                    src[8] = self.lfo_val[0];
                    src[9] = self.lfo_val[1];
                    src[10] = vel;
                    src[11] = ((key_f - 60.0) / 36.0).clamp(-1.0, 1.0);
                    src[12] = whl;
                    let mut m = [0.0f32; NDST];
                    for &(s, d, a) in &cells {
                        m[d] += src[s] * a;
                    }
                    let v = &mut self.voices[vi];
                    v.m = m;
                    // analog drift: a slow random walk per voice
                    v.drift_c += (self.rng.next() - v.drift_c) * 0.0005;
                    v.drift += (v.drift_c - v.drift) * 0.002;
                    // unison layout
                    let uni_det_a = (p(P::A_DET) + m[15] * 0.5).clamp(0.0, 1.0);
                    let uni_det_b = (p(P::B_DET) + m[15] * 0.5).clamp(0.0, 1.0);
                    let spr_a = (p(P::A_SPRD) + m[19]).clamp(0.0, 1.0);
                    let spr_b = (p(P::B_SPRD) + m[19]).clamp(0.0, 1.0);
                    let norm_a = 1.0 / (na as f32).sqrt();
                    let norm_b = 1.0 / (nb as f32).sqrt();
                    for k in 0..na {
                        let pos = spread_pos(k, na);
                        v.det_a[k] = pos * uni_det_a * uni_det_a * 60.0 + ((k * 7 % 5) as f32 - 2.0) * 0.4 * uni_det_a;
                        let pan = (pos * spr_a * 0.5 + 0.5).clamp(0.0, 1.0);
                        v.gains_a[k] = (((1.0 - pan) * std::f32::consts::FRAC_PI_2).sin() * norm_a, (pan * std::f32::consts::FRAC_PI_2).sin() * norm_a);
                    }
                    for k in 0..nb {
                        let pos = spread_pos(k, nb);
                        v.det_b[k] = pos * uni_det_b * uni_det_b * 60.0 + ((k * 5 % 7) as f32 - 3.0) * 0.4 * uni_det_b;
                        let pan = (pos * spr_b * 0.5 + 0.5).clamp(0.0, 1.0);
                        v.gains_b[k] = (((1.0 - pan) * std::f32::consts::FRAC_PI_2).sin() * norm_b, (pan * std::f32::consts::FRAC_PI_2).sin() * norm_b);
                    }
                    // filter coefficients
                    let mut cut = p(P::CUT) + m[8] + (v.flt.level - 0.0) * (p(P::FENV) - 0.5) * 2.0;
                    cut += (key_f - 60.0) / 60.0 * (p(P::KEYTR) - 0.5) * 2.0 * 0.5;
                    cut += v.drift * drift_amt * 0.03;
                    let cut = cut.clamp(0.0, 1.0);
                    let fc = (20.0 * 2f32.powf(cut * 10.0)).clamp(20.0, sr * 0.45);
                    let res = (p(P::RES) + m[9]).clamp(0.0, 1.0);
                    v.g = (std::f32::consts::PI * fc / sr).tan();
                    v.k = 2.0 - 1.97 * res.sqrt();
                    v.amp_v = 1.0;
                }
                let v = &mut self.voices[vi];
                // ── envelopes, pitch ──
                let a_env = v.amp.tick(amp_t.0, amp_t.1, amp_t.2, amp_t.3, dt);
                v.flt.tick(flt_t.0, flt_t.1, flt_t.2, flt_t.3, dt);
                if v.amp.stage == Stage::Idle {
                    v.active = false;
                    continue;
                }
                v.t += dt;
                v.note += (v.target - v.note) * glide_k;
                let m = v.m;
                let semis = v.note + self.bend_s + m[0] * 24.0 + v.drift * drift_amt * 0.15;
                let f_base = tuning.freq(semis);
                let fa = f_base * 2f32.powf(oct_a + fine_a / 12.0);
                let fb = f_base * 2f32.powf(oct_b + (b_semi + m[16] * 12.0) / 12.0 + fine_b / 12.0);
                // ── oscillators ──
                let wa = (p(P::A_WAVE) + m[1]).clamp(0.0, 1.0);
                let wb = (p(P::B_WAVE) + m[4]).clamp(0.0, 1.0);
                let warp_a = (p(P::A_WARP) + m[2]).clamp(0.0, 1.0);
                let warp_b = (p(P::B_WARP) + m[5]).clamp(0.0, 1.0);
                let la = (p(P::A_LVL) + m[3]).clamp(0.0, 1.0);
                let lb = (p(P::B_LVL) + m[6]).clamp(0.0, 1.0);
                let amt = if mode == 0 { 0.0 } else { (p(P::AMT) + m[7]).clamp(0.0, 1.0) };
                let (mut a_l, mut a_r, mut b_l, mut b_r) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
                let pos_a = wa * (NTABLE - 1) as f32;
                let (ia, fra) = ((pos_a as usize).min(NTABLE - 2), pos_a - (pos_a as usize).min(NTABLE - 2) as f32);
                let pos_b = wb * (NTABLE - 1) as f32;
                let (ib, frb) = ((pos_b as usize).min(NTABLE - 2), pos_b - (pos_b as usize).min(NTABLE - 2) as f32);
                // B first: sync and phase modulation read it
                let mut b_wrapped = false;
                let lvl_for = |f: f32, warp: f32| -> usize {
                    let allowed = (0.45 * sr / (f.max(10.0) * (1.0 + warp * 2.0))).floor() as usize;
                    LEVELS.iter().position(|&h| h <= allowed).unwrap_or(LEVELS.len() - 1)
                };
                if lb > 0.0 || mode != 0 {
                    for k in 0..nb {
                        let f = fb * 2f32.powf((v.det_b[k] + v.drift * drift_amt * 3.0) / 1200.0);
                        let old = v.ph_b[k];
                        v.ph_b[k] += f * dt;
                        if v.ph_b[k] >= 1.0 {
                            v.ph_b[k] -= v.ph_b[k].floor();
                            if k == nb / 2 {
                                b_wrapped = true;
                            }
                        }
                        let _ = old;
                        let lv = lvl_for(f, warp_b);
                        let ph = warp(v.ph_b[k], warp_b);
                        let s = rd(&tb.w[ib][lv], ph) * (1.0 - frb) + rd(&tb.w[ib + 1][lv], ph) * frb;
                        b_l += s * v.gains_b[k].0;
                        b_r += s * v.gains_b[k].1;
                    }
                }
                let b_mono = (b_l + b_r) * 0.5;
                let sync_ratio = if mode == 3 { 1.0 + amt * 3.0 } else { 1.0 };
                for k in 0..na {
                    let f = fa * sync_ratio * 2f32.powf((v.det_a[k] + v.drift * drift_amt * 3.0) / 1200.0);
                    v.ph_a[k] += f * dt;
                    v.ph_a[k] -= v.ph_a[k].floor();
                    if mode == 3 && b_wrapped && amt > 0.01 {
                        v.ph_a[k] = 0.0;
                    }
                    let mut ph_in = v.ph_a[k];
                    if mode == 2 {
                        ph_in = (ph_in + b_mono * amt * 0.5).rem_euclid(1.0);
                    }
                    let lv = lvl_for(f, warp_a);
                    let ph = warp(ph_in, warp_a);
                    let s = rd(&tb.w[ia][lv], ph) * (1.0 - fra) + rd(&tb.w[ia + 1][lv], ph) * fra;
                    a_l += s * v.gains_a[k].0;
                    a_r += s * v.gains_a[k].1;
                }
                let combine = |a: f32, b: f32| -> f32 {
                    let plain = la * a + lb * b;
                    match mode {
                        1 => plain * (1.0 - amt) + a * b * 2.0 * (la + lb) * 0.5 * amt,
                        4 => {
                            let (ia, ib) = ((a.clamp(-1.0, 1.0) * 127.0) as i32 as i8, (b.clamp(-1.0, 1.0) * 127.0) as i32 as i8);
                            let x = (ia ^ ib) as f32 / 127.0;
                            plain * (1.0 - amt) + x * (la + lb) * 0.5 * amt
                        }
                        5 => plain * (1.0 - amt) + a.min(b) * (la + lb) * 0.5 * amt,
                        _ => plain,
                    }
                };
                let mut x_l = combine(a_l, b_l);
                let mut x_r = combine(a_r, b_r);
                // sub, noise, hum
                let sub = {
                    v.ph_sub += fa * 0.5 * dt;
                    v.ph_sub -= v.ph_sub.floor();
                    let s = (p(P::SUB) + m[12]).clamp(0.0, 1.0);
                    let _ = sub_lvl;
                    s * if sub_tri { 4.0 * (v.ph_sub - (v.ph_sub + 0.5).floor()).abs() - 1.0 } else { (TAU * v.ph_sub).sin() }
                };
                x_l += sub;
                x_r += sub;
                let nz = (noise_lvl + m[13]).clamp(0.0, 1.0);
                if nz > 0.001 {
                    let w = self.rng.next();
                    v.noise_lp += (w - v.noise_lp) * (0.02 + (1.0 - noise_c) * 0.9);
                    v.noise_lp2 += (v.noise_lp - v.noise_lp2) * (0.02 + (1.0 - noise_c) * 0.9);
                    let n = if noise_c > 0.66 { v.noise_lp2 * 6.0 } else if noise_c > 0.33 { v.noise_lp * 2.5 } else { w };
                    x_l += n * nz * 0.5;
                    x_r += n * nz * 0.5;
                }
                let hm = (hum_lvl + m[18]).clamp(0.0, 1.0);
                if hm > 0.001 {
                    let d = 1.0 + (p(P::HUMD) - 0.5) * 0.02;
                    let ratios = [1.0f32, 2.003 * d, 3.011 / d, 0.5 * d];
                    let amps = [0.5f32, 0.3, 0.18, 0.4];
                    let mut h = 0.0;
                    for q in 0..4 {
                        v.ph_hum[q] += fa * ratios[q] * dt;
                        v.ph_hum[q] -= v.ph_hum[q].floor();
                        h += (TAU * v.ph_hum[q]).sin() * amps[q];
                    }
                    x_l += h * hm * 0.6;
                    x_r += h * hm * 0.6;
                }
                // drive, filter, comb
                let drv = 1.0 + (p(P::DRV) + m[10]).clamp(0.0, 1.0) * 8.0;
                let mut chan = [x_l, x_r];
                for c in 0..2 {
                    let x = (chan[c] * drv).tanh() / drv.sqrt().max(1.0);
                    let (l1, b1, h1) = v.f1[c].run(x, v.g, v.k);
                    let y = match ftype {
                        0 => l1,
                        1 => {
                            let (l2, _, _) = v.f1b[c].run(l1, v.g, v.k);
                            l2
                        }
                        2 => b1 * v.k,
                        3 => h1,
                        _ => x - v.k * b1,
                    };
                    chan[c] = y;
                }
                let cm = (p(P::COMB) + m[11]).clamp(0.0, 1.0);
                if cm > 0.001 {
                    let fcomb = (f_base * 2f32.powf(comb_t / 12.0)).clamp(30.0, sr * 0.4);
                    let dly = sr / fcomb;
                    let fb = 0.2 + p(P::COMBFB) * 0.74;
                    for c in 0..2 {
                        let d = v.comb[c].read(dly);
                        let y = chan[c] + d * fb;
                        v.comb[c].push(y.clamp(-4.0, 4.0));
                        chan[c] = chan[c] * (1.0 - cm) + y * (1.0 - fb * 0.6) * cm;
                    }
                }
                // amp, velocity, pan
                let vel_g = 1.0 - vel_sens + vel_sens * v.vel;
                let g = a_env * vel_g * (1.0 + m[17]).clamp(0.0, 2.0) * 0.35;
                let pan = m[14].clamp(-1.0, 1.0);
                let ang = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
                let (pl, pr) = (ang.cos() * std::f32::consts::SQRT_2, ang.sin() * std::f32::consts::SQRT_2);
                sum_l += chan[0] * g * pl;
                sum_r += chan[1] * g * pr;
            }
            dry[i * 2] = sum_l;
            dry[i * 2 + 1] = sum_r;
        }

        // ── effects ───────────────────────────────────────────────────────────
        let hz_mix = p(P::HZMIX);
        let ratios = [2f32.powf((p(P::HZ1) - 0.5) * 48.0 / 12.0), 2f32.powf((p(P::HZ2) - 0.5) * 48.0 / 12.0)];
        let (ph_mix, ph_rate, ph_dep, ph_fb) = (p(P::PHMIX), 0.05 * 2f32.powf(p(P::PHRATE) * 7.0), p(P::PHDEP), p(P::PHFB) * 0.85);
        let ph_stages = [2usize, 4, 6, 8][self.sel(P::PHST as usize, 4)];
        let (ch_mix, ch_rate) = (p(P::CHMIX), 0.1 + p(P::CHRATE) * 3.0);
        let (dl_mix, dl_fb) = (p(P::DLMIX), p(P::DLFB) * 0.9);
        let div = [0.25f32, 0.375, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0][self.sel(P::DLTIME as usize, 8)];
        let dl_len = (div * beat * sr).min(self.dl[0].buf.len() as f32 - 4.0);
        let outg = p(P::OUT) * p(P::OUT) * 2.2;
        let (rv_mix, rv_size) = (p(P::RVMIX), p(P::RVSIZE));
        self.verb.set_param("size", rv_size);
        let win = sr * 0.04;
        let mut energy = 0.0f32;
        for i in 0..frames {
            let mut s = [dry[i * 2], dry[i * 2 + 1]];
            // harmonizer: two pitch-shifted copies
            if hz_mix > 0.001 {
                for c in 0..2 {
                    let w = (self.hz[0][c].process(s[c], ratios[0], win) + self.hz[1][c].process(s[c], ratios[1], win)) * 0.5;
                    s[c] = s[c] * (1.0 - hz_mix * 0.35) + w * hz_mix;
                }
            }
            // phaser
            if ph_mix > 0.001 {
                self.ph_lfo += ph_rate * dt;
                self.ph_lfo -= self.ph_lfo.floor();
                for c in 0..2 {
                    let lfo = (TAU * (self.ph_lfo + c as f32 * 0.25)).sin() * 0.5 + 0.5;
                    let fc = 180.0 * 2f32.powf(lfo * ph_dep * 5.5);
                    let t = (std::f32::consts::PI * fc.min(sr * 0.45) / sr).tan();
                    let a = (t - 1.0) / (t + 1.0);
                    let mut x = s[c] + self.ph_fb[c] * ph_fb;
                    for st in 0..ph_stages {
                        let y = -a * x + self.ph_state[c][st];
                        self.ph_state[c][st] = x + a * y;
                        x = y;
                    }
                    self.ph_fb[c] = x;
                    s[c] = s[c] * (1.0 - ph_mix * 0.5) + x * ph_mix * 0.5 + s[c] * 0.0;
                }
            }
            // ensemble chorus
            if ch_mix > 0.001 {
                self.ch_lfo += ch_rate * dt;
                self.ch_lfo -= self.ch_lfo.floor();
                for c in 0..2 {
                    self.ch[c].push(s[c]);
                    let mut w = 0.0;
                    for (q, off) in [0.0f32, 0.33, 0.66].iter().enumerate() {
                        let l = (TAU * (self.ch_lfo + off + c as f32 * 0.17)).sin();
                        w += self.ch[c].read(sr * (0.012 + 0.004 * q as f32 + 0.0035 * l));
                    }
                    s[c] = s[c] * (1.0 - ch_mix * 0.4) + w / 3.0 * ch_mix;
                }
            }
            // ping-pong delay
            if dl_mix > 0.001 {
                let (dl_l, dl_r) = (self.dl[0].read(dl_len), self.dl[1].read(dl_len));
                self.dl_lp[0] += (dl_l - self.dl_lp[0]) * 0.35;
                self.dl_lp[1] += (dl_r - self.dl_lp[1]) * 0.35;
                self.dl[0].push(s[0] * 0.7 + self.dl_lp[1] * dl_fb);
                self.dl[1].push(s[1] * 0.7 + self.dl_lp[0] * dl_fb);
                s[0] += dl_l * dl_mix;
                s[1] += dl_r * dl_mix;
            }
            self.scr[i * 2] = s[0] * rv_mix * 0.8;
            self.scr[i * 2 + 1] = s[1] * rv_mix * 0.8;
            self.scr2[i * 2] = s[0] * outg;
            self.scr2[i * 2 + 1] = s[1] * outg;
        }
        if rv_mix > 0.001 {
            let dry_send = self.scr.clone();
            self.verb.process(&mut self.scr);
            for k in 0..out.len() {
                let wet = (self.scr[k] - dry_send[k]) * outg;
                let o = self.scr2[k] + wet;
                let o = o.tanh();
                out[k] += o;
                energy += o.abs();
            }
        } else {
            for k in 0..out.len() {
                let o = self.scr2[k].tanh();
                out[k] += o;
                energy += o.abs();
            }
        }
        if energy < 1e-5 && !self.voices.iter().any(|v| v.active) {
            self.silent = self.silent.saturating_add(frames as u32);
        } else {
            self.silent = 0;
        }
    }

    fn idle(&mut self, frames: usize) {
        // exactly what render() does with no voice sounding, sample by sample (no audio): the two global LFOs (a fresh random
        // sample-and-hold value at every wrap) and the bend glide; nothing once Prism would have stopped rendering by itself
        if self.voices.iter().any(|v| v.active) || self.silent as f32 > self.sr * 8.0 {
            return;
        }
        self.refresh();
        let p = |k: P, v: &[f32]| v[k as usize];
        let dt = 1.0 / self.sr;
        let l1_hz = 0.05 * 2f32.powf(p(P::L1RATE, &self.v) * 9.0);
        let l2_hz = 0.05 * 2f32.powf(p(P::L2RATE, &self.v) * 9.0);
        let (l1_shp, l2_shp) = (self.sel(P::L1SHP as usize, 5), self.sel(P::L2SHP as usize, 5));
        let bend_target = (p(P::BEND, &self.v) - 0.5) * 2.0 * 2.0;
        for _ in 0..frames {
            for l in 0..2 {
                let hz = if l == 0 { l1_hz } else { l2_hz };
                self.lfo_ph[l] += hz * dt;
                if self.lfo_ph[l] >= 1.0 {
                    self.lfo_ph[l] -= 1.0;
                    self.lfo_hold[l] = self.rng.next();
                }
            }
            self.bend_s += (bend_target - self.bend_s) * 0.002;
        }
        for l in 0..2 {
            self.lfo_val[l] = Prism::lfo_shape(if l == 0 { l1_shp } else { l2_shp }, self.lfo_ph[l], self.lfo_hold[l]);
        }
        self.silent = self.silent.saturating_add(frames as u32);
    }

    fn params(&self) -> Vec<(&'static str, f32)> {
        names().list.iter().zip(self.v.iter()).map(|(n, v)| (*n, *v)).collect()
    }

    fn set_param(&mut self, name: &str, v: f32) {
        if !v.is_finite() || name == "bend range" {
            return;
        }
        if let Some(&i) = names().index.get(name) {
            self.v[i] = v.clamp(0.0, 1.0);
            if i == P::SCALE as usize || i == P::ROOT as usize || i == P::STRETCH as usize || (P::T0 as usize..P::T0 as usize + 12).contains(&i) {
                self.tuning_dirty = true;
            }
            if i >= MAT_BASE {
                self.cells_dirty = true;
            }
        }
    }

    fn get_param(&self, name: &str) -> Option<f32> {
        names().index.get(name).map(|&i| self.v[i])
    }

    fn name(&self) -> &'static str {
        "Ling Prism"
    }
    fn presets(&self) -> &'static [&'static str] {
        &Self::PRESETS
    }
    fn preset_categories(&self) -> Vec<(String, String)> {
        // prism_categories.txt: [Category] lines, each followed by the names in it
        let mut out = vec![];
        let mut cat = String::new();
        for line in include_str!("../prism_categories.txt").lines() {
            let l = line.trim();
            if l.starts_with('[') && l.ends_with(']') {
                cat = l[1..l.len() - 1].to_string();
            } else if !l.is_empty() && !cat.is_empty() {
                out.push((l.to_string(), cat.clone()));
            }
        }
        out
    }
    fn load_preset(&mut self, name: &str) {
        if let Some(st) = Self::preset_state(name) {
            self.load_state(&st);
        }
    }

    /// Compact state for patch files: neutral matrix cells and unused modulation curves are left out.
    fn state(&self) -> Vec<(String, f32)> {
        let mut out = vec![];
        for (i, n) in names().list.iter().enumerate() {
            let v = self.v[i];
            if i >= MAT_BASE && (v - 0.5).abs() < 1e-3 {
                continue;
            }
            if (MSEG_BASE..MAT_BASE).contains(&i) {
                // an envelope nothing listens to is not worth lines in the file
                let m = (i - MSEG_BASE) / MSEG_STRIDE;
                let used = (0..NDST).any(|dd| (self.v[MAT_BASE + m * NDST + dd] - 0.5).abs() > 0.004);
                if !used {
                    continue;
                }
            }
            out.push((n.to_string(), v));
        }
        out
    }

    fn load_state(&mut self, st: &[(String, f32)]) {
        self.v = defaults();
        for (n, v) in st {
            self.set_param(n, *v);
        }
        self.tuning_dirty = true;
        self.cells_dirty = true;
        for d in self.dl.iter_mut() {
            d.clear();
        }
    }

    fn plot(&self, what: &str, n: usize) -> Vec<f32> {
        let n = n.clamp(2, 1024);
        let tb = tables();
        match what {
            "wave" | "wave_b" => {
                let (w, wp) = if what == "wave" { (self.v[P::A_WAVE as usize], self.v[P::A_WARP as usize]) } else { (self.v[P::B_WAVE as usize], self.v[P::B_WARP as usize]) };
                let pos = w * (NTABLE - 1) as f32;
                let i = (pos as usize).min(NTABLE - 2);
                let f = pos - i as f32;
                (0..n).map(|k| {
                    let ph = warp(k as f32 / n as f32, wp);
                    rd(&tb.w[i][0], ph) * (1.0 - f) + rd(&tb.w[i + 1][0], ph) * f
                }).collect()
            }
            "filter" => {
                let cut = self.v[P::CUT as usize];
                let fc = 20.0 * 2f32.powf(cut * 10.0);
                let kq = 2.0 - 1.97 * self.v[P::RES as usize].sqrt();
                let ft = self.sel(P::FTYPE as usize, 5);
                (0..n).map(|i| {
                    let f = 20.0 * 1000f32.powf(i as f32 / (n - 1) as f32);
                    let r = f / fc;
                    let (re, im) = (1.0 - r * r, kq * r);
                    let den = (re * re + im * im).sqrt().max(1e-6);
                    let mag = match ft {
                        0 => 1.0 / den,
                        1 => 1.0 / (den * den),
                        2 => kq * r / den,
                        3 => r * r / den,
                        _ => (re.abs()) / den,
                    };
                    ((20.0 * mag.max(1e-6).log10() + 24.0) / 48.0).clamp(0.0, 1.0)
                }).collect()
            }
            "scale" => {
                // pitches of the scale across one octave as cents / period (0..1), one entry per step
                self.tuning.steps.iter().map(|s| (s / self.tuning.period).clamp(0.0, 1.0)).collect()
            }
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(p: &mut Prism, secs: f32) -> Vec<f32> {
        let n = (p.sr * secs) as usize;
        let mut out = vec![0.0f32; n * 2];
        for c in out.chunks_mut(128) {
            p.render(c);
        }
        out
    }
    fn peak(v: &[f32]) -> f32 {
        v.iter().fold(0.0f32, |m, x| m.max(x.abs()))
    }
    fn f0(x: &[f32], sr: f32) -> f32 {
        let m: Vec<f32> = x.chunks_exact(2).map(|f| f[0] + f[1]).collect();
        let w = &m[(sr * 0.3) as usize..(sr * 0.3) as usize + 4096];
        let (lo, hi) = ((sr / 1500.0) as usize, (sr / 40.0) as usize);
        let mut best = (lo, f32::MIN);
        for lag in lo..hi {
            let c: f32 = (0..w.len() - lag).map(|i| w[i] * w[i + lag]).sum();
            if c > best.1 {
                best = (lag, c);
            }
        }
        sr / best.0 as f32
    }

    #[test]
    fn param_layout_is_consistent() {
        assert_eq!(param_names().len(), NPARAM);
        assert_eq!(param_names()[0], "a wave");
        assert_eq!(param_names()[MSEG_BASE], "m1 len");
        assert_eq!(param_names()[MAT_BASE], "x m1pit");
        assert_eq!(param_names()[NPARAM - 1], "x whlspr");
        assert!(param_names().iter().collect::<std::collections::HashSet<_>>().len() == NPARAM, "names are unique");
    }

    #[test]
    fn every_preset_loads_sounds_and_stays_sane() {
        for name in Prism::PRESETS {
            let mut p = Prism::new(48000.0);
            p.load_preset(name);
            assert!(Prism::preset_state(name).is_some(), "{name} missing");
            for k in [36u8, 48, 60] {
                p.note_on(k, 0.9);
            }
            let a = render(&mut p, 1.2);
            p.all_off();
            let b = render(&mut p, 1.5);
            assert!(a.iter().chain(b.iter()).all(|x| x.is_finite()), "{name}: non-finite");
            let (pa, pb) = (peak(&a), peak(&b));
            eprintln!("{name:<16} peak {pa:.2}");
            assert!(pa > 0.02, "{name}: too quiet ({pa})");
            assert!(pa <= 1.0, "{name}: clips ({pa})");
            // after the release the sound dies away (a long reverb may swell past a slowly rising note, so judge the end)
            let tail = peak(&b[b.len() - 24000..]);
            assert!(tail < pa, "{name}: still as loud at the end of the release ({tail} vs {pa})");
            let _ = pb;
        }
    }

    #[test]
    fn polyphony_plays_chords_and_steals_gracefully() {
        let mut p = Prism::new(48000.0);
        for k in 40..70u8 {
            p.note_on(k, 0.8); // 30 notes into 16 voices
        }
        let o = render(&mut p, 0.5);
        assert!(o.iter().all(|x| x.is_finite()));
        assert!(p.voices.iter().filter(|v| v.active).count() <= MAXVOICES);
        assert!(peak(&o) < 1.0);
    }

    #[test]
    fn microtonal_scales_change_the_pitch_of_the_keys() {
        let sr = 48000.0;
        let play = |scale: &str, key: u8| {
            let mut p = Prism::new(sr);
            p.set_param("scale", SCALE_NAMES.iter().position(|n| *n == scale).unwrap() as f32 / (SCALE_NAMES.len() - 1) as f32);
            p.set_param("a wave", 0.0);
            p.set_param("a uni", 0.0);
            p.set_param("sub", 0.0);
            p.set_param("drift", 0.0);
            p.note_on(key, 0.9);
            f0(&render(&mut p, 0.8), sr)
        };
        // 12-TET: key 69 is A4 = 440
        assert!((play("12 tet", 69) - 440.0).abs() < 4.0);
        // 24-EDO: one key is a quarter tone, so key 61 sits 50 cents above middle C
        let c = play("24 edo", 60);
        let q = play("24 edo", 61);
        assert!(((q / c).log2() * 1200.0 - 50.0).abs() < 6.0, "quarter tone: {} cents", (q / c).log2() * 1200.0);
        // 19-EDO: 19 keys make an octave
        let c = play("19 edo", 60);
        let o = play("19 edo", 79);
        assert!(((o / c) - 2.0).abs() < 0.02, "19 keys = an octave: ratio {}", o / c);
        // Bohlen-Pierce: 13 keys make a tritave (3:1)
        let c = play("bohlen-pierce", 60);
        let t = play("bohlen-pierce", 73);
        assert!(((t / c) - 3.0).abs() < 0.05, "13 keys = a tritave: ratio {}", t / c);
        // just major third: key 64 is 5/4 of key 60
        let c = play("just 5-limit", 60);
        let e = play("just 5-limit", 64);
        assert!(((e / c) - 1.25).abs() < 0.01, "5/4: {}", e / c);
    }

    #[test]
    fn multistep_envelopes_move_parameters() {
        let sr = 48000.0;
        let mut p = Prism::new(sr);
        // MSEG 1: starts open (1.0) and falls to 0 over its length; routed to the cutoff, strongly
        p.set_param("cut", 0.2);
        p.set_param("m1 time", 0.45);
        for (k, y) in [1.0f32, 0.6, 0.2, 0.0, 0.0].iter().enumerate() {
            p.set_param(&format!("m1 y{k}"), *y);
        }
        p.set_param("x m1cut", 1.0);
        p.note_on(48, 0.9);
        let o = render(&mut p, 1.2);
        let bright = |s: &[f32]| s.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>() / s.len() as f32;
        let mono: Vec<f32> = o.chunks_exact(2).map(|f| f[0] + f[1]).collect();
        let early = &mono[(sr * 0.02) as usize..(sr * 0.1) as usize];
        let late = &mono[(sr * 0.9) as usize..(sr * 1.15) as usize];
        assert!(bright(early) > 1.5 * bright(late), "the envelope closes the filter: {} vs {}", bright(early), bright(late));
    }

    #[test]
    fn effects_change_the_sound_and_stay_finite() {
        let sr = 48000.0;
        let dry = {
            let mut p = Prism::new(sr);
            p.set_param("rv mix", 0.0);
            p.note_on(48, 0.9);
            render(&mut p, 1.0)
        };
        for (name, params) in [("harmonizer", vec![("hz mix", 0.8)]), ("phaser", vec![("ph mix", 0.9)]), ("chorus", vec![("ch mix", 0.9)]), ("delay", vec![("dl mix", 0.8)]), ("reverb", vec![("rv mix", 0.8)])] {
            let mut p = Prism::new(sr);
            p.set_param("rv mix", 0.0);
            for (k, v) in params {
                p.set_param(k, v);
            }
            p.note_on(48, 0.9);
            let o = render(&mut p, 1.0);
            assert!(o.iter().all(|x| x.is_finite()), "{name}");
            let diff: f32 = o.iter().zip(dry.iter()).map(|(a, b)| (a - b).abs()).sum::<f32>() / o.len() as f32;
            assert!(diff > 0.002, "{name} must change the sound ({diff})");
            assert!(peak(&o) < 1.0, "{name} clips");
        }
    }

    #[test]
    fn random_parameters_never_blow_up() {
        let mut rng = Rng::new(7);
        for round in 0..24 {
            let mut p = Prism::new(48000.0);
            for n in param_names() {
                if !n.starts_with("t") || n.len() > 3 {
                    p.set_param(n, (rng.next() + 1.0) * 0.5);
                }
            }
            for k in [36u8, 55, 72] {
                p.note_on(k, 0.9);
            }
            let o = render(&mut p, 0.6);
            assert!(o.iter().all(|x| x.is_finite()), "round {round}: non-finite");
            assert!(peak(&o) <= 1.0, "round {round}: peak {}", peak(&o));
        }
    }

    #[test]
    fn state_round_trips_through_a_patch() {
        let mut p = Prism::new(48000.0);
        p.load_preset("Slendro Pad");
        let text = crate::patch::dump("prism", "Slendro Pad", &p.state(), &[]);
        let parsed = crate::patch::parse(&text);
        assert_eq!(parsed.kind, "prism");
        let q = crate::patch::build(&parsed, 48000.0).unwrap();
        for ((a, x), (b, y)) in p.params().iter().zip(q.params().iter()) {
            assert_eq!(a, b);
            assert!((x - y).abs() < 0.002, "{a}: {x} vs {y}");
        }
    }
}
