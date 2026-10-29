//! Ling Growl — a monophonic bass / lead synth for dubstep and electro with drawable modulation.
//!
//! The whole instrument state is flat, named, 0..1 parameters (selectors are stored as `index / (n-1)`,
//! lane points as 0..1, matrix cells as `(amount + 4) / 8`), so patches, project saves, undo and
//! automation all work through the generic [`Instrument`] interface without any special casing.
//!
//! Two signature extras sit after the matrix in the parameter list (so older patches and automation keep their meaning):
//!   * TALK: a 16-step, tempo-synced vowel sequencer driving a formant filter after the main filter ("t s0".."t s15" are the vowels,
//!     A at 0 through U at 1; `talk` blends it in, `t rate` sets steps per beat, `t glid` how the vowel slides between steps, `t len` the
//!     number of steps, `t res` / `t sh` the formant resonance and shift). This is what makes a bass speak.
//!   * SYNC: hard sync of osc A, the neuro screech: the stack is reset by a master at the note's pitch while it runs `1 + sync * 7` times
//!     faster, and lane 3 sweeps the ratio by `s mod`.
//!
//! The sound-design block (appended last, all neutral by default, so older patches sound as they did):
//!   * COMB (`comb`, `c tune`, `c fb`, `c damp`): a feedback comb after the filter, tuned relative to the note (+-24 semitones, matrix
//!     destination k in cents): positive feedback rings on the harmonics (metal), negative on the odd ones an octave up (hollow).
//!   * FREQ SHIFT (`fshift`, matrix s in Hz): a Hilbert frequency shifter after the comb - every partial moves by the same Hz.
//!   * FOLD / ASYM / CRUSH: a sine wavefolder before the drive (matrix o), an offset into the shaper for even harmonics, a bit and
//!     sample-rate crusher after it. The folder and the drive share the shaper's oversampling.
//!   * PITCH ENVELOPE (`penv` +-48 semitones, `pdec` its decay): zaps, chirps, kick-like drops on each new note.
//!   * `uni`: 3, 5, 7 or 9 stacked voices (the extra ones spread around the stack's notes by `detn`); `sub oct` puts the sub on the
//!     note instead of an octave below.
//!
//! The pro block (appended after the sound-design block, all neutral by default):
//!   * MULTIBAND DISTORTION (`mb drv`, `mb xo`): after the drive the sound is split in three at `mb xo` and eight times that; the lows stay
//!     clean (a tight sub), the mids are distorted hard and the highs softly, which is what makes a modern growl big without mud.
//!   * FORMANT (`formant`): shifts the oscillator's spectral envelope up or down without moving the pitch (windowed, time-compressed cycles).
//!   * OTT (`ott`): a three-band upward + downward compressor, the "Over The Top" sound of electronic radio.
//!   * SUB SAT (`sub sat`): harmonics on the sub so it carries on small speakers; STEREO (`s width` M/S width, `mono lo` bass kept in
//!     mono below the corner), `tilt` (a +-6 dB tilt EQ around 700 Hz) and `air` (high shelf at 8 kHz).
//!
//! Signal path: (osc A + osc B + noise) -> filter -> talk -> comb -> freq shift -> fold -> drive -> crush -> 11 kHz low-pass -> amp envelope ->
//! amp modulation -> pan -> width / delay / reverb sends -> limiter -> output. The sub joins after the
//! drive so it stays clean.
use super::fx::{Effect, Reverb};
use super::prism::{ProCfg, ProState};
use super::{midi_hz, Instrument, Rng, TAU};
use std::sync::OnceLock;

pub const NLANE: usize = 4;
pub const NPTS: usize = 16;
pub const NSRC: usize = 7;
pub const NDST: usize = 16;
/// the matrix destinations past the first 13: FM index, reverb send, delay send
const D_FMI: usize = 13;
const D_VERB: usize = 14;
const D_DELY: usize = 15;

const CORE: [&str; 33] = [
    "table", "pos", "detn", "stack", "b wave", "b rat", "b fm", "b lvl", "sub", "nois", "ftype", "cut", "res", "vow", "drv", "shaper", "amp a", "amp d", "amp s", "amp r", "env a",
    "env d", "env s", "env r", "glid", "wide", "dely", "verb", "out", "tempo", "bend", "whl", "hold",
];
const P_TABLE: usize = 0;
const P_POS: usize = 1;
const P_DETN: usize = 2;
const P_STACK: usize = 3;
const P_BWAVE: usize = 4;
const P_BRAT: usize = 5;
const P_BFM: usize = 6;
const P_BLVL: usize = 7;
const P_SUB: usize = 8;
const P_NOIS: usize = 9;
const P_FTYPE: usize = 10;
const P_CUT: usize = 11;
const P_RES: usize = 12;
const P_VOW: usize = 13;
const P_DRV: usize = 14;
const P_SHAPER: usize = 15;
const P_AMP: usize = 16; // a d s r
const P_ENV: usize = 20; // a d s r
const P_GLID: usize = 24;
const P_WIDE: usize = 25;
const P_DELY: usize = 26;
const P_VERB: usize = 27;
const P_OUT: usize = 28;
const P_TEMPO: usize = 29;
const P_BEND: usize = 30;
const P_WHL: usize = 31;
const P_HOLD: usize = 32;
/// per lane: len, int, loop, trig, then 16 points
pub const LANE_BASE: usize = 33;
pub const LANE_STRIDE: usize = 4 + NPTS;
pub const MAT_BASE: usize = LANE_BASE + NLANE * LANE_STRIDE;
/// the extras after the matrix: talk, sync and the 16 vowel steps
pub const EXT_BASE: usize = MAT_BASE + NSRC * NDST;
pub const NSTEP: usize = 16;
const EXT: [&str; 8] = ["talk", "t rate", "t glid", "t len", "t res", "t sh", "sync", "s mod"];
const P_TALK: usize = EXT_BASE;
const P_TRATE: usize = EXT_BASE + 1;
const P_TGLID: usize = EXT_BASE + 2;
const P_TLEN: usize = EXT_BASE + 3;
const P_TRES: usize = EXT_BASE + 4;
const P_TSH: usize = EXT_BASE + 5;
const P_SYNC: usize = EXT_BASE + 6;
const P_SMOD: usize = EXT_BASE + 7;
pub const P_TSTEP: usize = EXT_BASE + 8;
/// the sound-design block after the talk steps
pub const SD_BASE: usize = EXT_BASE + 8 + NSTEP;
const SD: [&str; 12] = ["comb", "c tune", "c fb", "c damp", "fold", "crush", "asym", "fshift", "penv", "pdec", "uni", "sub oct"];
const P_COMB: usize = SD_BASE;
const P_CTUNE: usize = SD_BASE + 1;
const P_CFB: usize = SD_BASE + 2;
const P_CDAMP: usize = SD_BASE + 3;
const P_FOLD: usize = SD_BASE + 4;
const P_CRUSH: usize = SD_BASE + 5;
const P_ASYM: usize = SD_BASE + 6;
const P_FSHIFT: usize = SD_BASE + 7;
const P_PENV: usize = SD_BASE + 8;
const P_PDEC: usize = SD_BASE + 9;
const P_UNI: usize = SD_BASE + 10;
const P_SUBOCT: usize = SD_BASE + 11;
/// the pro block after the sound design: multiband drive and crossover, formant, OTT, sub saturation, stereo width, mono bass, tilt, air
pub const PRO_BASE: usize = SD_BASE + 12;
const PRO: [&str; 9] = ["mb drv", "mb xo", "formant", "ott", "s width", "mono lo", "sub sat", "tilt", "air"];
const P_MBD: usize = PRO_BASE;
const P_MBXO: usize = PRO_BASE + 1;
const P_FORMANT: usize = PRO_BASE + 2;
const P_OTT: usize = PRO_BASE + 3;
const P_SWIDTH: usize = PRO_BASE + 4;
const P_MONOLO: usize = PRO_BASE + 5;
const P_SUBSAT: usize = PRO_BASE + 6;
const P_TILT: usize = PRO_BASE + 7;
const P_AIR: usize = PRO_BASE + 8;
/// the synthesis block after the pro block: polyphony, the FM block, the user wavetable switch, note-value lane lengths
pub const X_BASE: usize = PRO_BASE + PRO.len();
const XP: [&str; 17] = ["poly", "fm", "fm alg", "fm r1", "fm r2", "fm r3", "fm r4", "fm 2", "fm 3", "fm 4", "fm idx", "fm fb", "user", "l1 div", "l2 div", "l3 div", "l4 div"];
const P_POLY: usize = X_BASE;
const P_FM: usize = X_BASE + 1;
const P_FMALG: usize = X_BASE + 2;
/// operator ratios r1..r4 (op 1 is the carrier)
const P_FMR: usize = X_BASE + 3;
/// output levels of operators 2..4
const P_FML: usize = X_BASE + 7;
const P_FMIDX: usize = X_BASE + 10;
const P_FMFB: usize = X_BASE + 11;
const P_USER: usize = X_BASE + 12;
/// l1 div .. l4 div: 0 = the lane's `len`, else a note value from `NOTE_DIVS` (index + 1) / 16
const P_LDIV: usize = X_BASE + 13;
pub const NPARAM: usize = X_BASE + XP.len();
/// voices for `poly` (1 = mono, as Growl always was)
pub const POLY: [usize; 6] = [1, 2, 3, 4, 6, 8];
const MAXV: usize = 8;
/// FM operator ratios for `fm r1`..`fm r4` (index / 15)
pub const FM_RATIOS: [f32; 16] = [0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 16.0];
/// FM algorithms: 4>3>2>1, 4>3 and 2 into 1, 2+3+4 into 1, two pairs (2>1, 4>3); operator 4 has the feedback
pub const FM_ALGS: [&str; 4] = ["stack", "split", "fan", "pair"];
/// Note values for tempo-synced cycles (Growl lanes `l* div`, Prism LFOs `l1 div` / `l2 div`): (name, length of one cycle in beats).
/// T = triplet (2/3 of the note), . = dotted (3/2 of it).
pub const NOTE_DIVS: [(&str, f32); 16] = [
    ("1/32", 0.125),
    ("1/16T", 1.0 / 6.0),
    ("1/16", 0.25),
    ("1/16.", 0.375),
    ("1/8T", 1.0 / 3.0),
    ("1/8", 0.5),
    ("1/8.", 0.75),
    ("1/4T", 2.0 / 3.0),
    ("1/4", 1.0),
    ("1/4.", 1.5),
    ("1/2T", 4.0 / 3.0),
    ("1/2", 2.0),
    ("1/2.", 3.0),
    ("1bar", 4.0),
    ("2bar", 8.0),
    ("4bar", 16.0),
];
/// the beats of one cycle for a `div` parameter value, None = off (0)
pub fn note_div(v: f32) -> Option<f32> {
    let i = (v * 16.0).round() as usize;
    if i == 0 {
        None
    } else {
        Some(NOTE_DIVS[i.min(16) - 1].1)
    }
}
/// voices per `uni` step, and where the voices past the first three sit (times `detn` x 40 cents)
const UNI: [usize; 4] = [3, 5, 7, 9];
const UNI_SPREAD: [f32; 6] = [-0.5, 0.5, -0.8, 0.8, -0.2, 0.2];
/// steps per beat for `t rate`
const T_RATES: [f64; 5] = [0.5, 1.0, 2.0, 4.0, 8.0];

pub const SRC_NAMES: [&str; NSRC] = ["l1", "l2", "l3", "l4", "env", "vel", "whl"];
pub const DST_NAMES: &str = "pwfcqvdanrkosiey";
pub const LANE_LEN: [f32; 6] = [0.5, 1.0, 2.0, 4.0, 8.0, 16.0];
const B_RATIOS: [f32; 8] = [0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 5.0, 7.0];
const STACKS: [[f32; 3]; 5] = [[0.0, 0.0, 0.0], [0.0, 700.0, 1200.0], [0.0, 1200.0, -1200.0], [0.0, 300.0, 700.0], [0.0, 400.0, 700.0]];
const DETN_SPREAD: [f32; 3] = [-1.0, 1.0, 0.35];
/// modulation per destination at |amount| 1..4
const MOD_AMT: [[f32; 4]; NDST] = [
    [50.0, 300.0, 1200.0, 2400.0],
    [0.1, 0.25, 0.5, 1.0],
    [0.1, 0.25, 0.5, 1.0],
    [600.0, 1500.0, 3000.0, 6000.0],
    [1.6, 4.0, 8.0, 16.0],
    [0.1, 0.25, 0.5, 1.0],
    [3.0, 7.5, 15.0, 30.0],
    [0.1, 0.25, 0.5, 1.0],
    [0.1, 0.25, 0.5, 1.0],
    [240.0, 600.0, 1200.0, 2400.0],
    // k: comb tune (cents), o: fold, s: frequency shift (Hz)
    [100.0, 300.0, 1200.0, 2400.0],
    [0.1, 0.25, 0.5, 1.0],
    [10.0, 50.0, 200.0, 800.0],
    // i: FM index, e: reverb send, y: delay send
    [0.1, 0.25, 0.5, 1.0],
    [0.1, 0.25, 0.5, 1.0],
    [0.1, 0.25, 0.5, 1.0],
];
const VOWELS: [[f32; 3]; 5] = [[300.0, 870.0, 2240.0], [570.0, 840.0, 2410.0], [730.0, 1090.0, 2440.0], [530.0, 1840.0, 2480.0], [270.0, 2290.0, 3010.0]];
const VOW_GAIN: [f32; 3] = [1.6, 1.1, 0.7];

pub fn param_names() -> &'static [&'static str] {
    static N: OnceLock<Vec<&'static str>> = OnceLock::new();
    N.get_or_init(|| {
        let mut v: Vec<&'static str> = CORE.to_vec();
        for l in 0..NLANE {
            for s in ["len", "int", "loop", "trig"] {
                v.push(Box::leak(format!("l{} {}", l + 1, s).into_boxed_str()));
            }
            for k in 0..NPTS {
                v.push(Box::leak(format!("l{} p{}", l + 1, k).into_boxed_str()));
            }
        }
        for s in SRC_NAMES {
            for d in DST_NAMES.chars() {
                v.push(Box::leak(format!("m {s}{d}").into_boxed_str()));
            }
        }
        for n in EXT {
            v.push(n);
        }
        for k in 0..NSTEP {
            v.push(Box::leak(format!("t s{k}").into_boxed_str()));
        }
        for n in SD {
            v.push(n);
        }
        for n in PRO {
            v.push(n);
        }
        for n in XP {
            v.push(n);
        }
        v
    })
}

fn defaults() -> Vec<f32> {
    let mut v = vec![0.0f32; NPARAM];
    let set = |v: &mut Vec<f32>, k: usize, x: f32| v[k] = x;
    set(&mut v, P_DETN, 0.2);
    set(&mut v, P_BRAT, 1.0 / 7.0);
    set(&mut v, P_SUB, 0.5);
    set(&mut v, P_CUT, 0.4);
    set(&mut v, P_RES, 0.3);
    set(&mut v, P_VOW, 0.5);
    set(&mut v, P_DRV, 0.3);
    set(&mut v, P_AMP, 0.02);
    set(&mut v, P_AMP + 1, 0.4);
    set(&mut v, P_AMP + 2, 1.0);
    set(&mut v, P_AMP + 3, 0.2);
    set(&mut v, P_ENV, 0.01);
    set(&mut v, P_ENV + 1, 0.35);
    set(&mut v, P_ENV + 3, 0.2);
    set(&mut v, P_GLID, 0.15);
    set(&mut v, P_WIDE, 0.2);
    set(&mut v, P_VERB, 0.1);
    set(&mut v, P_OUT, 0.8);
    set(&mut v, P_TEMPO, (140.0 - 60.0) / 140.0);
    set(&mut v, P_BEND, 0.5);
    for l in 0..NLANE {
        let b = LANE_BASE + l * LANE_STRIDE;
        v[b] = 1.0 / 5.0; // 1 beat
        v[b + 1] = 0.5; // lin
        v[b + 2] = 1.0; // loop
    }
    for i in 0..NSRC * NDST {
        v[MAT_BASE + i] = 0.5;
    }
    v[P_TRATE] = 0.5;
    v[P_TGLID] = 0.3;
    v[P_TLEN] = 1.0;
    v[P_TRES] = 0.4;
    v[P_TSH] = 0.5;
    // a talking figure: A O I E ... as the starting pattern
    for (k, x) in [0.0, 0.5, 1.0, 0.75, 0.25, 0.5, 0.75, 1.0, 0.0, 0.25, 0.5, 1.0, 0.75, 0.5, 0.25, 0.0].iter().enumerate() {
        v[P_TSTEP + k] = *x;
    }
    // the sound-design block: off / centred
    v[P_CTUNE] = 0.5;
    v[P_CFB] = 0.8;
    v[P_CDAMP] = 0.25;
    v[P_FSHIFT] = 0.5;
    v[P_PENV] = 0.5;
    v[P_PDEC] = 0.3;
    // the pro block: off / flat
    v[P_MBXO] = 0.4;
    v[P_FORMANT] = 0.5;
    v[P_SWIDTH] = 0.5;
    v[P_TILT] = 0.5;
    // the FM block: off; carrier and op 2 at the note, op 3 an octave up, op 4 (feedback) at the note
    v[P_FMR] = 1.0 / 15.0;
    v[P_FMR + 1] = 1.0 / 15.0;
    v[P_FMR + 2] = 3.0 / 15.0;
    v[P_FMR + 3] = 1.0 / 15.0;
    v[P_FML] = 0.6;
    v[P_FMIDX] = 0.5;
    v
}

// ── wavetables ──────────────────────────────────────────────────────────────
const TN: usize = 2048;
const LEVELS: [usize; 6] = [32, 16, 8, 4, 2, 1];

fn bump(n: f32, c: f32, w: f32) -> f32 {
    (-((n - c) / w).powi(2)).exp()
}

/// harmonic amplitudes (index 1..=32) of table `t`, side 0 = X, 1 = Y
fn harmonics(t: usize, side: usize) -> [f32; 33] {
    let mut a = [0.0f32; 33];
    for n in 1..=32usize {
        let nf = n as f32;
        a[n] = match (t, side) {
            (0, 0) => 1.0 / nf,
            (0, _) => {
                if n % 2 == 1 {
                    1.0 / nf
                } else {
                    0.0
                }
            }
            (1, 0) => 1.0 / (nf * nf),
            (1, _) => {
                let s = if n % 3 == 0 { -1.0 } else { 1.0 };
                s * (1.0 / nf) * (0.35 + 3.0 * bump(nf, 7.0, 2.0) + 2.2 * bump(nf, 15.0, 3.0) + 1.4 * bump(nf, 24.0, 3.0))
            }
            (2, 0) => (1.0 / nf) * (0.2 + 2.5 * bump(nf, 4.0, 1.5) + 1.2 * bump(nf, 8.0, 2.0)),
            (2, _) => (1.0 / nf) * (0.2 + 2.0 * bump(nf, 5.0, 1.5) + 2.4 * bump(nf, 17.0, 3.0)),
            (_, 0) => {
                // triangle
                if n % 2 == 1 {
                    let s = if (n / 2) % 2 == 0 { 1.0 } else { -1.0 };
                    s / (nf * nf)
                } else {
                    0.0
                }
            }
            _ => {
                if [1usize, 2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31].contains(&n) {
                    1.0 / nf.sqrt()
                } else {
                    0.04 / nf
                }
            }
        };
    }
    a
}

struct Tables {
    /// [table][side][level] -> one cycle
    w: Vec<Vec<Vec<Vec<f32>>>>,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let sin_t: Vec<f32> = (0..TN).map(|i| (TAU * i as f32 / TN as f32).sin()).collect();
        let mut w = vec![];
        for t in 0..4 {
            let mut sides = vec![];
            for side in 0..2 {
                let h = harmonics(t, side);
                let build = |hmax: usize| -> Vec<f32> { (0..TN).map(|i| (1..=hmax).map(|n| h[n] * sin_t[(n * i) % TN]).sum()).collect() };
                let full = build(32);
                let peak = full.iter().fold(1e-6f32, |m, x| m.max(x.abs()));
                let levels: Vec<Vec<f32>> = LEVELS.iter().map(|&hm| build(hm).into_iter().map(|x| x / peak).collect()).collect();
                sides.push(levels);
            }
            w.push(sides);
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

// ── small DSP parts ─────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Debug)]
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
    /// a, d, r in seconds, s level
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
                if self.level < 0.0001 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }
}

#[derive(Clone, Copy, Default)]
struct Tpt {
    ic1: f32,
    ic2: f32,
}
impl Tpt {
    /// returns (lp, bp, hp-ish helpers) for a given cutoff / Q
    #[inline]
    fn run(&mut self, x: f32, g: f32, k: f32) -> (f32, f32) {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, v1)
    }
}

struct DelayLine {
    buf: Vec<f32>,
    w: usize,
}
impl DelayLine {
    fn new(n: usize) -> Self {
        DelayLine { buf: vec![0.0; n.max(8)], w: 0 }
    }
    #[inline]
    fn push(&mut self, x: f32) {
        self.buf[self.w] = x;
        self.w = (self.w + 1) % self.buf.len();
    }
    /// read `d` samples ago (fractional)
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

/// A three-band "Over The Top" compressor: the stereo signal is split at 160 Hz and 2.5 kHz, every band is pulled toward its own target
/// level from both sides (quiet parts up, loud parts down, linked across the channels), and the bands are summed again.
struct Ott {
    x1: [Tpt; 2],
    x2: [Tpt; 2],
    env: [f32; 3],
    tg: [f32; 3],
    gain: [f32; 3],
    n: u32,
}
impl Ott {
    const T: [f32; 3] = [0.18, 0.10, 0.04];
    fn new() -> Self {
        Ott { x1: [Tpt::default(); 2], x2: [Tpt::default(); 2], env: [0.0; 3], tg: [1.0; 3], gain: [1.0; 3], n: 0 }
    }
    /// `c` the exponent (strength), `g1`/`g2` the crossover coefficients, `att`/`rel` the follower coefficients per band
    #[inline]
    fn process(&mut self, l: f32, r: f32, c: f32, g1: f32, g2: f32, att: &[f32; 3], rel: &[f32; 3]) -> (f32, f32) {
        let mut bands = [[0.0f32; 3]; 2];
        for ch in 0..2 {
            let x = if ch == 0 { l } else { r };
            let (lo, _) = self.x1[ch].run(x, g1, 1.414);
            let rest = x - lo;
            let (mid, _) = self.x2[ch].run(rest, g2, 1.414);
            bands[ch] = [lo, mid, rest - mid];
        }
        for b in 0..3 {
            let lvl = bands[0][b].abs().max(bands[1][b].abs());
            let a = if lvl > self.env[b] { att[b] } else { rel[b] };
            self.env[b] += (lvl - self.env[b]) * a;
        }
        self.n = self.n.wrapping_add(1);
        if self.n & 7 == 0 {
            for b in 0..3 {
                self.tg[b] = (Ott::T[b] / (self.env[b] + 2e-4)).powf(c).clamp(0.2, 4.0);
            }
        }
        let mut o = [0.0f32; 2];
        for b in 0..3 {
            self.gain[b] += (self.tg[b] - self.gain[b]) * 0.03;
            o[0] += bands[0][b] * self.gain[b];
            o[1] += bands[1][b] * self.gain[b];
        }
        (o[0], o[1])
    }
}

// ── the instrument ──────────────────────────────────────────────────────────
/// One voice: everything that follows a note, from the oscillators to the pan. Mono mode plays voice 0 only (as Growl always did);
/// `poly` > 1 hands each new note its own voice.
struct GVoice {
    key: u8,
    gate: bool,
    age: u64,
    note: f32,
    target: f32,
    vel: f32,
    ph_a: [f32; 9],
    ph_b: f32,
    ph_sub: f32,
    amp: Env,
    menv: Env,
    rng: Rng,
    f_main: Tpt,
    f_form: [Tpt; 3],
    f_lp: Tpt,
    /// the talk formant filters
    f_talk: [Tpt; 3],
    /// hard sync master phase
    ph_sync: f32,
    /// the shaper's oversampling memory: the last input and its shaped value
    os_y: f32,
    os_sh: f32,
    /// comb delay, its damping memory; the frequency shifter; seconds since the last new note (pitch envelope); crusher and DC blocker
    comb_line: DelayLine,
    comb_lp: f32,
    hil: super::bassfx::Hilbert,
    fs_ph: f64,
    p_t: f32,
    cr_h: f32,
    cr_n: u32,
    dc_x: f32,
    dc_y: f32,
    /// a DC blocker at the voice output (about 3 Hz)
    out_dc: (f32, f32),
    /// the multiband crossovers
    f_mb: [Tpt; 2],
    /// filter coefficients between their four-sample updates
    g_main: f32,
    k_main: f32,
    form_g: [f32; 3],
    form_k: f32,
    t_g: [f32; 3],
    /// the FM block: operator phases, the feedback memory, the last full-rate output (the 2x decimator)
    fm_ph: [f32; 4],
    fm_fb: [f32; 2],
    fm_last: f32,
    /// per-note pitch (semitones from the key): now, where it is heading this block, the step per sample; `pb_fresh` = the first
    /// value after the note started is taken at once (a scoop starts below)
    pb: f32,
    pb_tgt: f32,
    pb_step: f32,
    pb_fresh: bool,
    /// per-note pressure (0 = none) and timbre (0.5 = neutral), ramped across the block; `x_fresh` = the first values jump
    prs: super::Smooth,
    tmb: super::Smooth,
    prs_fresh: bool,
    tmb_fresh: bool,
    /// poly: how much of the sub this voice plays (only the lowest note keeps it), smoothed
    sub_g: f32,
}

impl GVoice {
    fn new(sr: f32, k: usize) -> GVoice {
        GVoice {
            key: 0,
            gate: false,
            age: 0,
            note: 48.0,
            target: 48.0,
            vel: 0.8,
            ph_a: [0.0, 0.31, 0.67, 0.13, 0.52, 0.88, 0.24, 0.71, 0.45],
            ph_b: 0.0,
            ph_sub: 0.0,
            amp: Env::new(),
            menv: Env::new(),
            rng: Rng::new(0x9e3779b9u32.wrapping_add((k as u32).wrapping_mul(0x85eb_ca6b))),
            f_main: Tpt::default(),
            f_form: [Tpt::default(); 3],
            f_lp: Tpt::default(),
            f_talk: [Tpt::default(); 3],
            ph_sync: 0.0,
            os_y: 0.0,
            os_sh: 0.0,
            comb_line: DelayLine::new((sr * 0.06) as usize),
            comb_lp: 0.0,
            hil: Default::default(),
            fs_ph: 0.0,
            p_t: 10.0,
            cr_h: 0.0,
            cr_n: 0,
            dc_x: 0.0,
            dc_y: 0.0,
            out_dc: (0.0, 0.0),
            f_mb: [Tpt::default(); 2],
            g_main: 0.0,
            k_main: 1.0,
            form_g: [0.0; 3],
            form_k: 1.0,
            t_g: [0.0; 3],
            fm_ph: [0.0; 4],
            fm_fb: [0.0; 2],
            fm_last: 0.0,
            pb: 0.0,
            pb_tgt: 0.0,
            pb_step: 0.0,
            pb_fresh: true,
            prs: super::Smooth::at(0.0),
            tmb: super::Smooth::at(0.5),
            prs_fresh: true,
            tmb_fresh: true,
            sub_g: 1.0,
        }
    }
    /// a new note on this voice: the pitch gesture starts fresh
    fn new_key(&mut self, key: u8) {
        self.key = key;
        self.pb = 0.0;
        self.pb_tgt = 0.0;
        self.pb_step = 0.0;
        self.pb_fresh = true;
        self.prs.reset(0.0);
        self.tmb.reset(0.5);
        self.prs_fresh = true;
        self.tmb_fresh = true;
    }
}

/// the FM block's settings for one block
#[derive(Clone, Copy)]
struct FmCfg {
    alg: usize,
    /// operator frequency ratios (op 1 is the carrier)
    r: [f32; 4],
    /// output levels of operators 2..4 (index 0 = op 2)
    l: [f32; 3],
    /// feedback of operator 4, in cycles
    fb: f32,
}

/// sin(2 pi ph) from the table; the phase may be anywhere (a modulated phase goes below zero)
#[inline]
fn fm_sin(ph: f32) -> f32 {
    rd(&sine_table()[..], ph - ph.floor())
}

fn sine_table() -> &'static [f32] {
    static S: OnceLock<Vec<f32>> = OnceLock::new();
    S.get_or_init(|| (0..TN).map(|i| (TAU * i as f32 / TN as f32).sin()).collect())
}

impl GVoice {
    /// one FM sample at the operators' current phases; `idx` = the modulation index (cycles of phase per unit of output)
    #[inline]
    fn fm_eval(&mut self, c: &FmCfg, idx: f32) -> f32 {
        let fbv = (self.fm_fb[0] + self.fm_fb[1]) * 0.5 * c.fb;
        let o4 = fm_sin(self.fm_ph[3] + fbv);
        self.fm_fb = [self.fm_fb[1], o4];
        let (m2, m3, m4) = (c.l[0] * idx, c.l[1] * idx, c.l[2] * idx);
        match c.alg {
            // 4 -> 3 -> 2 -> 1
            0 => {
                let o3 = fm_sin(self.fm_ph[2] + m4 * o4);
                let o2 = fm_sin(self.fm_ph[1] + m3 * o3);
                fm_sin(self.fm_ph[0] + m2 * o2)
            }
            // 4 -> 3, then 3 and 2 -> 1
            1 => {
                let o3 = fm_sin(self.fm_ph[2] + m4 * o4);
                let o2 = fm_sin(self.fm_ph[1]);
                fm_sin(self.fm_ph[0] + m2 * o2 + m3 * o3)
            }
            // 2, 3 and 4 -> 1
            2 => {
                let o2 = fm_sin(self.fm_ph[1]);
                let o3 = fm_sin(self.fm_ph[2]);
                fm_sin(self.fm_ph[0] + m2 * o2 + m3 * o3 + m4 * o4)
            }
            // two pairs: 2 -> 1 and 4 -> 3, both carriers heard (op 3 at its level)
            _ => {
                let o3 = fm_sin(self.fm_ph[2] + m4 * o4);
                let o1 = fm_sin(self.fm_ph[0] + m2 * fm_sin(self.fm_ph[1]));
                (o1 + c.l[1] * o3) / (1.0 + c.l[1])
            }
        }
    }
    /// advance the operators by `inc` cycles of the note
    #[inline]
    fn fm_step(&mut self, c: &FmCfg, inc: f32) {
        for k in 0..4 {
            self.fm_ph[k] += inc * c.r[k];
            self.fm_ph[k] -= self.fm_ph[k].floor();
        }
    }
    /// one output sample at twice the rate (a half step, a full step, and a 1-2-1 average back down: the sidebands of a high index
    /// alias far less)
    #[inline]
    fn fm_sample(&mut self, c: &FmCfg, f0: f32, sr: f32, idx: f32) -> f32 {
        let half = 0.5 * f0 / sr;
        self.fm_step(c, half);
        let mid = self.fm_eval(c, idx);
        self.fm_step(c, half);
        let cur = self.fm_eval(c, idx);
        let y = 0.25 * self.fm_last + 0.5 * mid + 0.25 * cur;
        self.fm_last = cur;
        y
    }
}

pub struct Growl {
    sr: f32,
    v: Vec<f32>,
    /// modulation amounts per source -> destination (resolved from the matrix)
    mat: [[f32; NDST]; NSRC],
    mat_dirty: bool,
    held: Vec<(u8, f32)>,
    /// the voices (mono: voice 0 only)
    vo: Vec<GVoice>,
    /// the voice whose modulation drives the shared parts (lane 1 rate, the sends): the newest note's
    lead: usize,
    clock: u64,
    lane_t: [f64; NLANE],
    lane_val: [f32; NLANE],
    wide_l: DelayLine,
    wide_r: DelayLine,
    wide_ph: f32,
    dly: DelayLine,
    dly_lp: f32,
    verb: Reverb,
    scr: Vec<f32>,
    scr2: Vec<f32>,
    lim: f32,
    silent: u32,
    last_mod: [f32; NDST],
    /// the talk sequencer: step clock and the smoothed vowel position (shared by the voices)
    talk_t: f64,
    talk_pos: f32,
    /// the OTT compressor and the output stage
    ott: Ott,
    pro: ProState,
    /// a user wavetable (resampled audio or a wavetable file), played when `user` is on
    wt: Option<std::sync::Arc<super::wavetable::WaveTable>>,
}

pub const ARCHETYPES: [&str; 10] = ["wobble", "talk", "screech", "riddim", "lead", "pad", "pluck", "sub", "fm", "vox"];

/// fill 16 lane points with a shape: 0 sine, 1 triangle, 2 saw up, 3 saw down, 4 square, 5 random on / off runs, 6 random, 7 zigzag, 8 swell
fn lane_pts(kind: usize, rng: &mut Rng, out: &mut [f32]) {
    let n = out.len();
    let mut hold = 0.0f32;
    for (i, o) in out.iter_mut().enumerate() {
        let x = i as f32 / n as f32;
        *o = match kind {
            0 => 0.5 - 0.5 * (TAU * x).cos(),
            1 => 1.0 - (2.0 * x - 1.0).abs(),
            2 => x,
            3 => 1.0 - x,
            4 => {
                if x < 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            5 => {
                if i % 2 == 0 {
                    hold = if (rng.next() + 1.0) * 0.5 > 0.45 { 1.0 } else { 0.0 };
                }
                hold
            }
            6 => (rng.next() + 1.0) * 0.5,
            7 => [0.0, 1.0, 0.25, 0.75, 0.5, 1.0, 0.0, 0.5][i % 8],
            _ => {
                if x < 0.5 {
                    (x * 2.0).min(1.0)
                } else {
                    1.0 - (x - 0.5)
                }
            }
        };
    }
}

impl Growl {
    fn set_mat(&mut self, src: usize, dst: usize, amt: i32) {
        self.v[MAT_BASE + src * NDST + dst] = (amt.clamp(-4, 4) as f32 + 4.0) / 8.0;
        self.mat_dirty = true;
    }

    fn set_lane(&mut self, l: usize, len_idx: usize, interp: f32, looping: bool, trig: bool, kind: usize, rng: &mut Rng) {
        let b = LANE_BASE + l * LANE_STRIDE;
        self.v[b] = len_idx as f32 / 5.0;
        self.v[b + 1] = interp;
        self.v[b + 2] = if looping { 1.0 } else { 0.0 };
        self.v[b + 3] = if trig { 1.0 } else { 0.0 };
        let mut pts = [0.0f32; NPTS];
        lane_pts(kind, rng, &mut pts);
        self.v[b + 4..b + 4 + NPTS].copy_from_slice(&pts);
    }

    /// A new random but musical patch. `arche` picks the family (see `ARCHETYPES`), else the seed does; the same seed always gives the
    /// same sound. Tempo, output level, bend range, wheel and hold are left as they were. Returns the family used.
    pub fn random_patch(&mut self, seed: u32, arche: Option<usize>) -> usize {
        let mut r = Rng::new(seed.wrapping_mul(2654435761).wrapping_add(0x9e3779b9));
        for _ in 0..6 {
            r.next();
        }
        macro_rules! rn {
            ($a:expr, $b:expr) => {{
                let x = (r.next() + 1.0) * 0.5;
                ($a as f32) + (($b as f32) - ($a as f32)) * x
            }};
        }
        macro_rules! pick {
            ($n:expr) => {{
                let n: usize = $n;
                (((r.next() + 1.0) * 0.5 * n as f32) as usize).min(n - 1)
            }};
        }
        let keep = [P_TEMPO, P_OUT, P_BEND, P_WHL, P_HOLD].map(|k| (k, self.v[k]));
        self.v = defaults();
        for (k, x) in keep {
            self.v[k] = x;
        }
        let a = arche.unwrap_or_else(|| pick!(ARCHETYPES.len())).min(ARCHETYPES.len() - 1);
        let selv = |i: usize, n: usize| i as f32 / (n - 1) as f32;
        self.v[P_TABLE] = selv(pick!(4), 4);
        self.v[P_POS] = rn!(0.0, 0.7);
        self.v[P_DETN] = rn!(0.1, 0.55);
        self.v[P_SUB] = rn!(0.3, 0.7);
        self.v[P_CUT] = rn!(0.3, 0.7);
        self.v[P_RES] = rn!(0.2, 0.5);
        self.v[P_DRV] = rn!(0.25, 0.55);
        self.v[P_SHAPER] = selv(pick!(3), 3);
        self.v[P_GLID] = rn!(0.05, 0.3);
        self.v[P_WIDE] = rn!(0.1, 0.4);
        self.v[P_VERB] = rn!(0.05, 0.25);
        match a {
            0 => {
                // wobble: a filter lane at a musical length and a second lane moving the wavetable
                let len = [0usize, 1, 1, 2, 2, 3][pick!(6)];
                self.set_lane(0, len, if pick!(2) == 0 { 1.0 } else { 0.5 }, true, false, [0, 1, 1, 7, 8][pick!(5)], &mut r);
                self.set_lane(1, (len + 1).min(4), 0.5, true, false, [0, 1, 6][pick!(3)], &mut r);
                self.v[P_CUT] = rn!(0.12, 0.35);
                self.v[P_RES] = rn!(0.3, 0.6);
                self.v[P_DRV] = rn!(0.35, 0.7);
                self.v[P_SUB] = rn!(0.5, 0.85);
                self.set_mat(0, 3, 2 + pick!(3) as i32);
                self.set_mat(1, 1, 1 + pick!(3) as i32);
                if pick!(3) == 0 {
                    self.set_mat(0, 6, 1);
                }
            }
            1 => {
                // talk: the vowel sequencer with runs of repeated vowels
                self.v[P_TALK] = rn!(0.7, 1.0);
                self.v[P_TRATE] = selv(2 + pick!(3), 5);
                self.v[P_TGLID] = rn!(0.05, 0.55);
                self.v[P_TRES] = rn!(0.3, 0.8);
                self.v[P_TSH] = rn!(0.4, 0.7);
                let mut last = 0.0;
                for k in 0..NSTEP {
                    if k == 0 || (r.next() + 1.0) * 0.5 > 0.35 {
                        last = pick!(5) as f32 / 4.0;
                    }
                    self.v[P_TSTEP + k] = last;
                }
                self.v[P_CUT] = rn!(0.45, 0.75);
                self.v[P_DETN] = rn!(0.2, 0.5);
                self.v[P_WIDE] = rn!(0.15, 0.5);
            }
            2 => {
                // screech: hard sync swept by lane 3, plenty of drive
                self.v[P_SYNC] = rn!(0.3, 0.75);
                self.v[P_SMOD] = rn!(0.3, 1.0);
                self.set_lane(2, 1 + pick!(3), 0.5, true, false, [2, 1, 7][pick!(3)], &mut r);
                self.set_lane(0, 1 + pick!(2), 1.0, true, false, 0, &mut r);
                self.v[P_TABLE] = selv(1 + pick!(3), 4);
                self.v[P_CUT] = rn!(0.6, 0.85);
                self.v[P_DRV] = rn!(0.5, 0.8);
                self.v[P_SHAPER] = selv(1 + pick!(2), 3);
                self.set_mat(0, 3, 2);
            }
            3 => {
                // riddim: an on / off rhythm lane on the amplitude and a filter lane
                self.set_lane(0, 1 + pick!(2), 0.0, true, false, 5, &mut r);
                self.set_lane(1, 1 + pick!(3), 1.0, true, false, [0, 1, 2][pick!(3)], &mut r);
                self.set_mat(0, 7, -4);
                self.set_mat(1, 3, 2 + pick!(2) as i32);
                self.v[P_CUT] = rn!(0.35, 0.6);
                self.v[P_DRV] = rn!(0.5, 0.75);
                self.v[P_SHAPER] = selv(1 + pick!(2), 3);
                if pick!(3) == 0 {
                    self.v[P_TALK] = rn!(0.25, 0.5);
                    self.v[P_TRATE] = selv(3, 5);
                }
            }
            4 => {
                // lead: sustained, detuned, a little vibrato
                self.v[P_STACK] = selv(pick!(5), 5);
                self.v[P_DETN] = rn!(0.4, 0.85);
                self.v[P_SUB] = rn!(0.0, 0.2);
                self.v[P_CUT] = rn!(0.55, 0.85);
                self.v[P_AMP + 2] = 1.0;
                self.v[P_AMP + 3] = rn!(0.15, 0.35);
                self.v[P_GLID] = rn!(0.15, 0.4);
                self.v[P_VERB] = rn!(0.2, 0.4);
                self.v[P_DELY] = rn!(0.0, 0.3);
                self.set_lane(0, 2, 1.0, true, false, 0, &mut r);
                self.set_mat(0, 0, 1);
                if pick!(2) == 0 {
                    self.v[P_SYNC] = rn!(0.3, 0.6);
                }
            }
            5 => {
                // pad: slow attack, wide, moving filter
                self.v[P_STACK] = selv(1 + pick!(4), 5);
                self.v[P_DETN] = rn!(0.45, 0.8);
                self.v[P_AMP] = rn!(0.4, 0.65);
                self.v[P_AMP + 1] = 0.5;
                self.v[P_AMP + 2] = 1.0;
                self.v[P_AMP + 3] = rn!(0.45, 0.7);
                self.v[P_SUB] = rn!(0.1, 0.3);
                self.v[P_CUT] = rn!(0.45, 0.7);
                self.v[P_WIDE] = rn!(0.4, 0.7);
                self.v[P_VERB] = rn!(0.35, 0.6);
                self.v[P_DRV] = rn!(0.05, 0.25);
                self.set_lane(0, 4, 1.0, true, false, [0, 8][pick!(2)], &mut r);
                self.set_mat(0, 3, 2);
                if pick!(2) == 0 {
                    self.v[P_TALK] = rn!(0.3, 0.6);
                    self.v[P_TRATE] = 0.0;
                    self.v[P_TGLID] = rn!(0.7, 0.95);
                }
            }
            6 => {
                // pluck: a short amplitude and a filter that closes fast
                self.v[P_AMP + 1] = rn!(0.25, 0.45);
                self.v[P_AMP + 2] = 0.0;
                self.v[P_AMP + 3] = rn!(0.05, 0.2);
                self.v[P_ENV + 1] = rn!(0.2, 0.4);
                self.v[P_CUT] = rn!(0.3, 0.55);
                self.v[P_RES] = rn!(0.3, 0.55);
                self.set_mat(4, 3, 3 + pick!(2) as i32);
                if pick!(3) == 0 {
                    self.v[P_SYNC] = rn!(0.3, 0.6);
                }
            }
            7 => {
                // sub: the pitch falls from the note on, a low filter, a long amplitude
                self.v[P_TABLE] = 0.0;
                self.v[P_POS] = 0.0;
                self.v[P_SUB] = rn!(0.75, 0.95);
                self.v[P_CUT] = rn!(0.14, 0.28);
                self.v[P_DRV] = rn!(0.15, 0.5);
                self.v[P_AMP + 1] = rn!(0.6, 0.9);
                self.v[P_AMP + 2] = rn!(0.6, 0.95);
                self.set_lane(0, 1 + pick!(2), 0.5, false, true, 3, &mut r);
                self.set_mat(0, 0, 3 + pick!(2) as i32);
            }
            8 => {
                // fm: osc B modulating osc A, mostly in the bright, bell-like ratios
                self.v[P_BWAVE] = selv(pick!(2) * 3, 4);
                self.v[P_BRAT] = selv([1usize, 3, 4, 5, 7][pick!(5)], 8);
                self.v[P_BFM] = rn!(0.35, 0.85);
                self.v[P_BLVL] = rn!(0.0, 0.3);
                self.v[P_CUT] = rn!(0.6, 0.9);
                self.v[P_DRV] = rn!(0.1, 0.4);
                self.v[P_AMP + 1] = rn!(0.35, 0.7);
                self.v[P_AMP + 2] = rn!(0.0, 0.4);
                self.v[P_AMP + 3] = rn!(0.25, 0.5);
                self.v[P_VERB] = rn!(0.25, 0.5);
                self.set_mat(4, 2, -(1 + pick!(3) as i32));
                self.set_lane(0, 2, 1.0, true, false, 0, &mut r);
                self.set_mat(0, 2, 1 + pick!(2) as i32);
            }
            _ => {
                // vox: the vowel filter swept by a lane, the talk sequencer underneath
                self.v[P_FTYPE] = 1.0;
                self.v[P_RES] = rn!(0.35, 0.65);
                self.v[P_VOW] = rn!(0.0, 0.3);
                self.v[P_TALK] = rn!(0.3, 0.6);
                self.v[P_TRATE] = selv(1 + pick!(3), 5);
                self.v[P_TGLID] = rn!(0.3, 0.8);
                for k in 0..NSTEP {
                    self.v[P_TSTEP + k] = pick!(5) as f32 / 4.0;
                }
                self.set_lane(0, 1 + pick!(2), 0.5, true, true, [2, 7, 8][pick!(3)], &mut r);
                self.set_mat(0, 5, 3 + pick!(2) as i32);
            }
        }
        // the sound-design block, now and then: a comb swept by lane 2, a folder on the drive, a pitch zap, wider unison
        if matches!(ARCHETYPES[a], "screech" | "riddim" | "fm" | "wobble") && pick!(3) == 0 {
            self.v[P_COMB] = rn!(0.25, 0.6);
            self.v[P_CTUNE] = [0.5, 0.75, 0.625][pick!(3)];
            self.v[P_CFB] = rn!(0.75, 0.95);
            self.set_mat(1, 10, 1 + pick!(3) as i32);
        }
        if matches!(ARCHETYPES[a], "riddim" | "screech" | "talk") && pick!(3) == 0 {
            self.v[P_FOLD] = rn!(0.15, 0.5);
        }
        if matches!(ARCHETYPES[a], "pluck" | "lead" | "sub") && pick!(3) == 0 {
            self.v[P_PENV] = rn!(0.55, 0.75);
            self.v[P_PDEC] = rn!(0.1, 0.25);
        }
        if matches!(ARCHETYPES[a], "lead" | "pad") {
            self.v[P_UNI] = selv(1 + pick!(3), 4);
        }
        // two more things that make a patch feel alive: a velocity touch on the drive, the wheel on the cutoff
        if pick!(2) == 0 {
            self.set_mat(5, 6, 1);
        }
        self.set_mat(6, 3, 2);
        self.mat_dirty = true;
        self.dly.clear();
        self.lane_t = [0.0; NLANE];
        a
    }

    pub const PRESETS: [&'static str; 141] = ["wobble", "growl", "yoi", "reese", "laser", "stab", "riser", "neuro", "talkbox", "robot vowels", "formant bass", "yoi talk", "alien talk", "vox pad", "choir stack", "glitch talk", "screech", "neuro sync", "hard lead", "sync sweep", "sync pluck", "tear out", "dub wobble", "half-time wob", "triplet wob", "riddim", "riddim 2", "minimal wub", "mid growl", "rasp", "gnarl", "future bass", "supersaw", "detune lead", "fm bell", "pluck bass", "sub drop", "808 growl", "acid squeal", "chirp", "swarm", "wide stab", "gentle wub", "sunken speaker", "rural shriek", "gravel stab", "brass hook", "crimson pad", "solar pop", "thunder drop", "molten metal", "violet throat", "sweet wub", "ghost speaker", "neon shriek", "wooden stab", "narrow anthem", "marble pad", "velvet tick", "savage drop", "rapid metal", "ancient throat", "satin wub", "static voice", "tidal shriek", "polar riddim", "whisper anthem", "silver pad", "scarlet tick", "salty drop", "shadow bell", "dusty throat", "liquid wobble", "mean voice", "copper shriek", "acid riddim", "frozen anthem", "heavy haze", "future tick", "chrome boom", "cosmic bell", "ashen throat", "tribal wobble", "razor voice", "toxic squeal", "amber riddim", "smoky beacon", "ember haze", "feral tick", "rotten boom", "kind bell", "carbon formant", "glass wobble", "burnt talker", "thin squeal", "retro riddim", "jade beacon", "bitter pad", "urban pop", "cotton drop", "hollow metal", "cobalt yoi", "stormy wub", "frost speaker", "lunar shriek", "golden stab", "comb growl", "spectral comb", "comb screech", "hollow comb", "vowel comb talk", "comb pluck", "color bass", "color wobble", "future chord", "riddim fold", "fold growl", "crushed riddim", "metal tearout", "machine gun", "shifted growl", "robot shifter", "barber growl", "complextro stab", "electro wob", "fidget squeal", "big room lead", "screech lead", "prog pluck", "prog chord stab", "minor chord stab", "saw bass prog", "slow filter saw", "zap", "pew pluck", "kick bass", "neuro reese", "phaser growl", "flanged reese", "brass growl", "yoi comb"];

    pub fn new(sr: f32) -> Self {
        let mut verb = Reverb::new(sr);
        verb.set_param("mix", 1.0);
        verb.set_param("size", 0.83);
        verb.set_param("damp", 0.35);
        let mut s = Growl {
            sr,
            v: defaults(),
            mat: [[0.0; NDST]; NSRC],
            mat_dirty: true,
            held: vec![],
            vo: (0..MAXV).map(|k| GVoice::new(sr, k)).collect(),
            lead: 0,
            clock: 0,
            lane_t: [0.0; NLANE],
            lane_val: [0.0; NLANE],
            wide_l: DelayLine::new((sr * 0.05) as usize),
            wide_r: DelayLine::new((sr * 0.05) as usize),
            wide_ph: 0.0,
            dly: DelayLine::new((sr * 1.4) as usize),
            dly_lp: 0.0,
            verb,
            scr: vec![],
            scr2: vec![],
            lim: 1.0,
            silent: 0,
            last_mod: [0.0; NDST],
            talk_t: 0.0,
            talk_pos: 0.0,
            ott: Ott::new(),
            pro: ProState::default(),
            wt: None,
        };
        s.resolve_matrix();
        s
    }

    /// voices in use: 1 = mono (legato, glide), more = each note its own voice
    fn npoly(&self) -> usize {
        POLY[self.sel(P_POLY, POLY.len())]
    }

    /// the voice for a new note in poly mode: the same key again, a silent voice, the quietest releasing one, else the oldest
    fn alloc_voice(&self, key: u8, n: usize) -> usize {
        if let Some(i) = (0..n).find(|&i| self.vo[i].gate && self.vo[i].key == key) {
            return i;
        }
        if let Some(i) = (0..n).find(|&i| self.vo[i].amp.stage == Stage::Idle && !self.vo[i].gate) {
            return i;
        }
        if let Some(i) = (0..n).filter(|&i| !self.vo[i].gate).min_by(|&a, &b| self.vo[a].amp.level.total_cmp(&self.vo[b].amp.level)) {
            return i;
        }
        (0..n).min_by_key(|&i| self.vo[i].age).unwrap_or(0)
    }

    /// Load a wavetable file into the user slot ("" or a bad file: an error, the slot keeps what it had).
    pub fn load_table(&mut self, path: &str) -> Result<usize, String> {
        let w = super::wavetable::WaveTable::load(path)?;
        let n = w.nframes();
        self.wt = Some(w);
        Ok(n)
    }

    fn resolve_matrix(&mut self) {
        for s in 0..NSRC {
            for d in 0..NDST {
                let a = (self.v[MAT_BASE + s * NDST + d] * 8.0 - 4.0).round() as i32;
                self.mat[s][d] = if a == 0 { 0.0 } else { a.signum() as f32 * MOD_AMT[d][(a.unsigned_abs() as usize).min(4) - 1] };
            }
        }
        self.mat_dirty = false;
    }

    #[inline]
    fn sel(&self, k: usize, n: usize) -> usize {
        ((self.v[k] * (n - 1) as f32).round() as usize).min(n - 1)
    }

    fn lane_value(&self, l: usize) -> f32 {
        let b = LANE_BASE + l * LANE_STRIDE;
        let len = match note_div(self.v[P_LDIV + l]) {
            Some(d) => d as f64,
            None => LANE_LEN[((self.v[b] * 5.0).round() as usize).min(5)] as f64,
        };
        let interp = ((self.v[b + 1] * 2.0).round() as usize).min(2);
        let looping = self.v[b + 2] > 0.5;
        let pts = &self.v[b + 4..b + 4 + NPTS];
        let x = self.lane_t[l] / len;
        if looping {
            let x = (x - x.floor()) as f32;
            let pos = x * NPTS as f32;
            let i = (pos as usize).min(NPTS - 1);
            if interp == 0 {
                return pts[i];
            }
            let f = pos - i as f32;
            let f = if interp == 2 { (1.0 - (std::f32::consts::PI * f).cos()) * 0.5 } else { f };
            pts[i] + (pts[(i + 1) % NPTS] - pts[i]) * f
        } else {
            let x = x.min(1.0) as f32;
            if interp == 0 {
                return pts[((x * NPTS as f32) as usize).min(NPTS - 1)];
            }
            let pos = x * (NPTS - 1) as f32;
            let i = (pos as usize).min(NPTS - 2);
            let f = pos - i as f32;
            let f = if interp == 2 { (1.0 - (std::f32::consts::PI * f).cos()) * 0.5 } else { f };
            pts[i] + (pts[i + 1] - pts[i]) * f
        }
    }

    fn trigger(&mut self) {
        for l in 0..NLANE {
            let b = LANE_BASE + l * LANE_STRIDE;
            let looping = self.v[b + 2] > 0.5;
            let trig = self.v[b + 3] > 0.5;
            if !looping || trig {
                self.lane_t[l] = 0.0;
            }
        }
    }

/// The effect chain a preset suggests (`fx Name k=v` lines in growl_presets.txt; `_` stands for a space in names): (effect, params).
    pub fn preset_fx(name: &str) -> Vec<(String, Vec<(String, f32)>)> {
        static F: OnceLock<Vec<(String, Vec<(String, Vec<(String, f32)>)>)>> = OnceLock::new();
        let all = F.get_or_init(|| {
            let mut out: Vec<(String, Vec<(String, Vec<(String, f32)>)>)> = vec![];
            for line in include_str!("../growl_presets.txt").lines() {
                if line.starts_with('[') && line.ends_with(']') {
                    out.push((line[1..line.len() - 1].to_string(), vec![]));
                } else if let (Some(rest), Some(cur)) = (line.trim().strip_prefix("fx "), out.last_mut()) {
                    let mut it = rest.split_whitespace();
                    let Some(fxn) = it.next() else { continue };
                    let ps = it.filter_map(|t| t.split_once('=')).filter_map(|(k, v)| v.parse::<f32>().ok().map(|v| (k.replace('_', " "), v))).collect();
                    cur.1.push((fxn.replace('_', " "), ps));
                }
            }
            out
        });
        all.iter().find(|(n, _)| n == name).map(|(_, f)| f.clone()).unwrap_or_default()
    }

    fn preset_state(name: &str) -> Option<Vec<(String, f32)>> {
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
            for line in include_str!("../growl_presets.txt").lines() {
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

impl Instrument for Growl {
    fn note_on(&mut self, key: u8, vel: f32) {
        self.held.retain(|h| h.0 != key);
        self.held.push((key, vel));
        let n = self.npoly();
        if n <= 1 {
            // mono: one voice, legato with glide between overlapping notes
            let gate = self.vo[0].gate;
            {
                let v = &mut self.vo[0];
                v.target = key as f32;
                v.vel = vel.clamp(0.0, 1.0);
                v.new_key(key);
            }
            if !gate {
                // after silence: jump to the pitch and restart the envelopes / lanes
                self.trigger();
                self.talk_t = 0.0;
                let v = &mut self.vo[0];
                v.note = v.target;
                v.amp.on();
                v.menv.on();
                v.ph_sync = 0.0;
                v.p_t = 0.0;
                v.gate = true;
            }
            self.lead = 0;
            return;
        }
        let any_gate = self.vo.iter().any(|v| v.gate);
        let i = self.alloc_voice(key, n);
        self.clock += 1;
        if !any_gate {
            self.trigger();
            self.talk_t = 0.0;
        }
        let v = &mut self.vo[i];
        v.age = self.clock;
        v.new_key(key);
        v.target = key as f32;
        v.note = key as f32;
        v.vel = vel.clamp(0.0, 1.0);
        v.amp.on();
        v.menv.on();
        v.ph_sync = 0.0;
        v.p_t = 0.0;
        v.gate = true;
        self.lead = i;
    }
    fn note_off(&mut self, key: u8) {
        self.held.retain(|h| h.0 != key);
        if self.npoly() <= 1 {
            if let Some(&(k, vl)) = self.held.last() {
                let v = &mut self.vo[0];
                v.target = k as f32;
                v.vel = vl;
                if v.key != k {
                    // back to an older held note: its own pitch, the gesture eases out
                    v.key = k;
                    v.pb_tgt = 0.0;
                    v.pb_fresh = false;
                }
            } else if self.v[P_HOLD] < 0.5 {
                let v = &mut self.vo[0];
                v.gate = false;
                v.amp.off();
                v.menv.off();
            }
            return;
        }
        if self.v[P_HOLD] >= 0.5 {
            return;
        }
        for v in self.vo.iter_mut().filter(|v| v.gate && v.key == key) {
            v.gate = false;
            v.amp.off();
            v.menv.off();
        }
    }
    fn note_pitch(&mut self, key: u8, semis: f32) {
        if !semis.is_finite() {
            return;
        }
        let s = semis.clamp(-48.0, 48.0);
        // only held voices: a releasing note keeps its last bend (the tail of a dive does not snap back)
        for v in self.vo.iter_mut().filter(|v| v.key == key && v.gate) {
            if v.pb_fresh {
                v.pb = s;
                v.pb_step = 0.0;
            }
            v.pb_tgt = s;
            v.pb_fresh = false;
        }
    }
    fn note_pressure(&mut self, key: u8, p: f32) {
        if !p.is_finite() {
            return;
        }
        for v in self.vo.iter_mut().filter(|v| v.key == key && v.gate) {
            v.prs.set(p.clamp(0.0, 1.0), v.prs_fresh);
            v.prs_fresh = false;
        }
    }
    fn note_timbre(&mut self, key: u8, t: f32) {
        if !t.is_finite() {
            return;
        }
        for v in self.vo.iter_mut().filter(|v| v.key == key && v.gate) {
            v.tmb.set(t.clamp(0.0, 1.0), v.tmb_fresh);
            v.tmb_fresh = false;
        }
    }
    fn all_off(&mut self) {
        self.held.clear();
        for v in self.vo.iter_mut() {
            v.gate = false;
            v.amp.off();
            v.menv.off();
        }
    }

    fn render(&mut self, out: &mut [f32]) {
        if self.mat_dirty {
            self.resolve_matrix();
        }
        let frames = out.len() / 2;
        if frames == 0 {
            return;
        }
        // which voices sound this block (a note cannot start inside a block)
        let mut act = [false; MAXV];
        for k in 0..MAXV {
            act[k] = self.vo[k].amp.stage != Stage::Idle;
        }
        // a quiet voice with dead effect tails costs nothing
        if !act.iter().any(|a| *a) && self.silent as f32 > self.sr * 6.0 {
            return;
        }
        let sr = self.sr;
        let dt = 1.0 / sr;
        let bpm = 60.0 + self.v[P_TEMPO] * 140.0;
        let bps = (bpm / 60.0) as f64 / sr as f64;
        let tb = tables();
        let tbl = self.sel(P_TABLE, 4);
        let stack = STACKS[self.sel(P_STACK, 5)];
        let bwave = self.sel(P_BWAVE, 4);
        let brat = B_RATIOS[self.sel(P_BRAT, 8)];
        let ftype = self.sel(P_FTYPE, 5);
        let shaper = self.sel(P_SHAPER, 3);
        let glide_tau = 0.005 + self.v[P_GLID] * self.v[P_GLID] * 0.35;
        let glide_k = 1.0 - (-dt / glide_tau).exp();
        let (aa, ad, as_, ar) = (0.002 + self.v[P_AMP].powi(2) * 2.0, 0.01 + self.v[P_AMP + 1].powi(2) * 3.0, self.v[P_AMP + 2], 0.01 + self.v[P_AMP + 3].powi(2) * 3.0);
        let (ea, ed, es, er) = (0.002 + self.v[P_ENV].powi(2) * 2.0, 0.01 + self.v[P_ENV + 1].powi(2) * 3.0, self.v[P_ENV + 2], 0.01 + self.v[P_ENV + 3].powi(2) * 3.0);
        let (wide, dely, verb) = (self.v[P_WIDE], self.v[P_DELY], self.v[P_VERB]);
        let outg = self.v[P_OUT] * self.v[P_OUT] * 0.9;
        let bend_c = (self.v[P_BEND] - 0.5) * 2.0 * 200.0;
        let whl = self.v[P_WHL];
        let dly_len = (0.75 * 60.0 / bpm * sr).min(self.dly.buf.len() as f32 - 4.0);
        let lp11 = (std::f32::consts::PI * 11000.0f32.min(sr * 0.45) / sr).tan();
        let k07 = 1.0 / 0.707;
        let wide_base = [0.011 * sr, 0.019 * sr];
        let mats = self.mat;
        // talk: steps per beat, number of steps, the glide of the vowel between steps, the formant shift and resonance
        let talk = self.v[P_TALK];
        let t_rate = T_RATES[self.sel(P_TRATE, 5)];
        let t_len = 1 + (self.v[P_TLEN] * 15.0).round() as usize;
        let t_k = 1.0 - (-dt / (0.002 + self.v[P_TGLID] * self.v[P_TGLID] * 0.15)).exp();
        let t_shift = 2f32.powf((self.v[P_TSH] - 0.5) * 2.0);
        let t_q = 1.0 / (4.0 + self.v[P_TRES] * 14.0);
        let sync_p = self.v[P_SYNC];
        let comb = self.v[P_COMB];
        let c_tune = (self.v[P_CTUNE] - 0.5) * 48.0;
        let c_fb = (self.v[P_CFB] * 2.0 - 1.0) * 0.97;
        let c_damp = 1.0 - self.v[P_CDAMP] * 0.92;
        let c_norm = 1.0 - c_fb.abs() * 0.85;
        let fold_p = self.v[P_FOLD];
        let crush = self.v[P_CRUSH];
        let asym = self.v[P_ASYM] * 0.8;
        let fshift = super::bassfx::shift_hz(self.v[P_FSHIFT]);
        let penv = (self.v[P_PENV] - 0.5) * 96.0;
        let ptau = 0.004 + self.v[P_PDEC] * self.v[P_PDEC] * 0.8;
        let nuni = UNI[self.sel(P_UNI, 4)];
        let uni_g = (3.0 / nuni as f32).sqrt();
        let sub_mul = if self.v[P_SUBOCT] > 0.5 { 1.0 } else { 0.5 };
        let cr_hold = 1 + (crush * crush * 24.0) as u32;
        let cr_bits = 2f32.powf(12.0 - crush * 9.0);
        // the pro block
        let mbd = self.v[P_MBD];
        let mb_on = mbd > 0.002;
        let mb_f1 = 60.0 * 2f32.powf(self.v[P_MBXO] * 4.0);
        let (mb_g1, mb_g2) = ((std::f32::consts::PI * mb_f1 / sr).tan(), (std::f32::consts::PI * (mb_f1 * 8.0).min(sr * 0.4) / sr).tan());
        let (mb_dm, mb_dh) = (1.0 + mbd * 14.0, 1.0 + mbd * 5.0);
        let fmt = (self.v[P_FORMANT] - 0.5) * 2.0;
        let fmt_on = fmt.abs() > 0.004;
        let fmt_r = 2f32.powf(fmt * 1.5);
        let fmt_mul = if fmt_on { fmt_r.max(1.0) } else { 1.0 };
        let ott_amt = self.v[P_OTT];
        let ott_on = ott_amt > 0.002;
        let ott_c = ott_amt * 0.7;
        let (ott_g1, ott_g2) = ((std::f32::consts::PI * 160.0 / sr).tan(), (std::f32::consts::PI * 2500.0f32.min(sr * 0.4) / sr).tan());
        let ott_att = [0.008f32, 0.004, 0.0015].map(|t| 1.0 - (-1.0 / (sr * t)).exp());
        let ott_rel = [0.12f32, 0.08, 0.05].map(|t| 1.0 - (-1.0 / (sr * t)).exp());
        let sub_sat = self.v[P_SUBSAT];
        let sub_sat_d = 1.0 + sub_sat * 5.0;
        let sub_sat_n = 1.0 / sub_sat_d.tanh();
        let mono_v = if self.v[P_MONOLO] > 0.02 { self.v[P_MONOLO] } else { 0.0 };
        let pro = ProCfg::new(sr, self.v[P_SWIDTH], mono_v, self.v[P_AIR], 0.0, self.v[P_TILT]);
        // the synthesis block: voices, FM, the user wavetable, sends under modulation
        let npoly = self.npoly();
        let poly = npoly > 1;
        // a full chord stays near the level of one mono note
        let poly_g = if npoly > 2 { (2.0 / npoly as f32).sqrt() } else { 1.0 };
        let fm_mix = self.v[P_FM];
        let fm_on = fm_mix > 0.002;
        let wt_skip = fm_mix > 0.998;
        let fm_cfg = FmCfg {
            alg: self.sel(P_FMALG, FM_ALGS.len()),
            r: [0, 1, 2, 3].map(|k| FM_RATIOS[self.sel(P_FMR + k, FM_RATIOS.len())]),
            l: [0, 1, 2].map(|k| self.v[P_FML + k]),
            fb: self.v[P_FMFB] * 0.45,
        };
        let has_dst = |d: usize| (0..NSRC).any(|s| mats[s][d] != 0.0);
        let fm_mod = has_dst(D_FMI);
        let verb_mod = has_dst(D_VERB);
        let dely_mod = has_dst(D_DELY);
        let ut = if self.v[P_USER] > 0.5 { self.wt.clone() } else { None };
        // poly: the sub only under the lowest sounding note (smoothed, so it moves without a click)
        let mut sub_tgt = [1.0f32; MAXV];
        if poly {
            let low = (0..MAXV).filter(|&k| act[k] && self.vo[k].gate).min_by(|&a, &b| self.vo[a].target.total_cmp(&self.vo[b].target));
            let low = low.or_else(|| (0..MAXV).filter(|&k| act[k]).min_by(|&a, &b| self.vo[a].target.total_cmp(&self.vo[b].target)));
            for k in 0..MAXV {
                sub_tgt[k] = if Some(k) == low { 1.0 } else { 0.0 };
            }
        }
        let sub_k = 1.0 - (-dt / 0.012f32).exp();
        // per-note pitch: a straight line from where it is to the value the engine asked for, across this block
        for v in self.vo.iter_mut() {
            v.pb_step = (v.pb_tgt - v.pb) / frames as f32;
            v.prs.begin(frames);
            v.tmb.begin(frames);
        }
        let lead = self.lead.min(MAXV - 1);
        let mut energy = 0.0f32;
        self.scr.resize(out.len(), 0.0);
        self.scr2.resize(out.len(), 0.0);

        for i in 0..frames {
            // lanes
            let r_cents = self.last_mod[9];
            for l in 0..NLANE {
                let rate = if l == 0 { 2f64.powf(r_cents as f64 / 1200.0) } else { 1.0 };
                self.lane_t[l] += bps * rate;
                self.lane_val[l] = self.lane_value(l);
            }
            if talk > 0.002 {
                self.talk_t += bps * t_rate;
                let step = (self.talk_t as usize) % t_len;
                let target = self.v[P_TSTEP + step];
                self.talk_pos += (target - self.talk_pos) * t_k;
            }
            let lane_val = self.lane_val;
            let talk_pos = self.talk_pos;
            let (mut l_out, mut r_out) = (0.0f32, 0.0f32);
            for vk in 0..MAXV {
                if vk != lead && !act[vk] {
                    continue;
                }
                let v = &mut self.vo[vk];
                let env_v = v.menv.tick(ea, ed, es, er, dt);
                let amp_v = v.amp.tick(aa, ad, as_, ar, dt);
                v.note += (v.target - v.note) * glide_k;

                let src = [lane_val[0], lane_val[1], lane_val[2], lane_val[3], env_v, v.vel, whl];
                let mut m = [0.0f32; NDST];
                for s in 0..NSRC {
                    if src[s] != 0.0 {
                        for d in 0..NDST {
                            m[d] += src[s] * mats[s][d];
                        }
                    }
                }
                if vk == lead {
                    self.last_mod = m;
                }
                // per-note expression: pressure opens the filter, pushes the drive and the level; timbre moves the wavetable
                // position and the cutoff (+-1 octave)
                let prs = v.prs.tick();
                let tmb = v.tmb.tick();
                if prs != 0.0 {
                    m[3] += prs * 1800.0;
                    m[6] += prs * 6.0;
                    m[7] += prs * 0.3;
                }
                if tmb != 0.5 {
                    m[1] += (tmb - 0.5) * 0.8;
                    m[3] += (tmb - 0.5) * 2400.0;
                }
                let pb_on = v.pb != 0.0 || v.pb_step != 0.0;
                if pb_on {
                    v.pb += v.pb_step;
                }
                if poly {
                    v.sub_g += (sub_tgt[vk] - v.sub_g) * sub_k;
                }
                if !(act[vk] || amp_v > 0.0) {
                    continue;
                }
                let pe = if penv != 0.0 && v.p_t < ptau * 8.0 { penv * (-v.p_t / ptau).exp() } else { 0.0 };
                v.p_t += dt;
                let mut semis = v.note + (bend_c + m[0]) / 100.0 + pe;
                if pb_on {
                    semis += v.pb;
                }
                let f0 = midi_hz(semis);
                // osc B first: it drives the FM
                let fb = f0 * brat;
                v.ph_b += fb / sr;
                v.ph_b -= v.ph_b.floor();
                let pb = v.ph_b;
                let dtb = (fb / sr).abs();
                let b_out = match bwave {
                    0 => (TAU * pb).sin(),
                    1 => 4.0 * (pb - (pb + 0.5).floor()).abs() - 1.0,
                    2 => (2.0 * pb - 1.0) - poly_blep(pb, dtb),
                    _ => {
                        let sq = if pb < 0.5 { 1.0 } else { -1.0 };
                        sq + poly_blep(pb, dtb) - poly_blep((pb + 0.5).fract(), dtb)
                    }
                };
                let fm_amt = (self.v[P_BFM] + m[2]).clamp(0.0, 1.0);
                let fm_dev = fm_amt * 6.0 * fb * b_out;
                // osc A stack
                let pos = (self.v[P_POS] + m[1]).clamp(0.0, 1.0);
                let th = pos * std::f32::consts::FRAC_PI_2;
                let (cx, cy) = (th.cos(), th.sin());
                // hard sync: the stack runs `ratio` times faster and is reset by a master at the note's pitch
                let ratio = if sync_p > 0.005 { (1.0 + (sync_p + lane_val[2] * self.v[P_SMOD]) * 7.0).clamp(1.0, 9.0) } else { 1.0 };
                if ratio > 1.0001 {
                    v.ph_sync += f0 / sr;
                    if v.ph_sync >= 1.0 {
                        v.ph_sync -= v.ph_sync.floor();
                        v.ph_a = [0.0; 9];
                    }
                }
                let mut a_sum = 0.0;
                if !wt_skip {
                    let allowed = (0.45 * sr / (f0 * ratio * fmt_mul).max(10.0)).floor() as usize;
                    let lvl = LEVELS.iter().position(|&h| h <= allowed).unwrap_or(LEVELS.len() - 1);
                    let (tx, ty) = (&tb.w[tbl][0][lvl], &tb.w[tbl][1][lvl]);
                    // the user table: two frames around the position, crossfaded
                    let uw = ut.as_deref().map(|w| {
                        let (a, b, fr) = w.frames_at(pos);
                        let ul = super::wavetable::WaveTable::level(f0 * ratio * fmt_mul, sr);
                        (w.cycle(a, ul), w.cycle(b, ul), fr)
                    });
                    for k in 0..nuni {
                        let spread = if k < 3 { DETN_SPREAD[k] } else { UNI_SPREAD[k - 3] };
                        let cents = stack[k % 3] + spread * self.v[P_DETN] * 40.0;
                        let fk = f0 * ratio * 2f32.powf(cents / 1200.0) + fm_dev;
                        v.ph_a[k] += fk / sr;
                        v.ph_a[k] -= v.ph_a[k].floor();
                        let (q, w) = if fmt_on {
                            // formant shift: the cycle is read `fmt_r` times faster and faded in and out, so the pitch stays and the
                            // spectral envelope moves
                            let ph = v.ph_a[k];
                            ((ph * fmt_r), (0.5 - 0.5 * (TAU * ph).cos()) * 1.6)
                        } else {
                            (v.ph_a[k], 1.0)
                        };
                        if let Some((c0, c1, fr)) = uw {
                            let s0 = rd(c0, q);
                            a_sum += w * (s0 + (rd(c1, q) - s0) * fr);
                        } else if fmt_on {
                            a_sum += w * (cx * rd(tx, q) + cy * rd(ty, q));
                        } else {
                            a_sum += cx * rd(tx, q) + cy * rd(ty, q);
                        }
                    }
                }
                let (osc, b_part, nz) = (a_sum * 0.5 * uni_g, b_out * self.v[P_BLVL] * 0.6 * 0.6, v.rng.next() * self.v[P_NOIS] * 0.5);
                let mut x = osc + b_part + nz;
                // the FM block: four sine operators at the note, blended with (or instead of) the wavetable stack; osc B stays as it is
                if fm_on {
                    let ix = if fm_mod { (self.v[P_FMIDX] + m[D_FMI]).clamp(0.0, 1.0) } else { self.v[P_FMIDX] };
                    let y = v.fm_sample(&fm_cfg, f0, sr, ix * ix * 2.5);
                    x = (osc + nz) * (1.0 - fm_mix) + y * 0.6 * fm_mix + b_part;
                }

                // filter (coefficients every 4 samples)
                let cut_c = m[3];
                if i & 3 == 0 {
                    if ftype == 4 {
                        let vow = (self.v[P_VOW] + m[5]).clamp(0.0, 1.0);
                        let fpos = vow * 4.0;
                        let i0 = (fpos as usize).min(3);
                        let fr = fpos - i0 as f32;
                        let shift = 2f32.powf(((self.v[P_CUT] - 0.5) * 2400.0 + cut_c) / 1200.0);
                        for j in 0..3 {
                            let fc = ((VOWELS[i0][j] + (VOWELS[i0 + 1][j] - VOWELS[i0][j]) * fr) * shift).clamp(40.0, sr * 0.45);
                            v.form_g[j] = (std::f32::consts::PI * fc / sr).tan();
                        }
                        v.form_k = 1.0 / (5.0 + self.v[P_RES] * 12.0);
                    } else {
                        let fc = (30.0 * 2f32.powf(self.v[P_CUT] * 9.2 + cut_c / 1200.0)).clamp(20.0, sr * 0.45);
                        let q = (0.5 + self.v[P_RES] * self.v[P_RES] * 18.0 + m[4]).max(0.3);
                        v.g_main = (std::f32::consts::PI * fc / sr).tan();
                        v.k_main = 1.0 / q;
                    }
                }
                let mut filtered = if ftype == 4 {
                    let mut s = 0.0;
                    for j in 0..3 {
                        let (_, bp) = v.f_form[j].run(x, v.form_g[j], v.form_k);
                        s += bp * v.form_k * VOW_GAIN[j];
                    }
                    s
                } else {
                    let (lp, bp) = v.f_main.run(x, v.g_main, v.k_main);
                    match ftype {
                        0 => lp,
                        1 => bp * v.k_main,
                        2 => x - v.k_main * bp - lp,
                        _ => x - v.k_main * bp,
                    }
                };
                // talk: the sequenced vowel through three formant filters, blended over the filtered sound
                if talk > 0.002 {
                    if i & 3 == 0 {
                        let fpos = talk_pos.clamp(0.0, 1.0) * 4.0;
                        let i0 = (fpos as usize).min(3);
                        let fr = fpos - i0 as f32;
                        for j in 0..3 {
                            let fc = ((VOWELS[i0][j] + (VOWELS[i0 + 1][j] - VOWELS[i0][j]) * fr) * t_shift).clamp(40.0, sr * 0.45);
                            v.t_g[j] = (std::f32::consts::PI * fc / sr).tan();
                        }
                    }
                    let mut vs = 0.0;
                    for j in 0..3 {
                        let (_, bp) = v.f_talk[j].run(filtered, v.t_g[j], t_q);
                        vs += bp * t_q * VOW_GAIN[j];
                    }
                    filtered = filtered * (1.0 - talk) + vs * talk * 1.8;
                }
                // comb: tuned to the note (plus `c tune` and matrix k), damped feedback, level kept even across the feedback
                if comb > 0.001 {
                    let fc = (f0 * 2f32.powf((c_tune + m[10] / 100.0) / 12.0)).clamp(20.0, sr * 0.4);
                    let y = v.comb_line.read(sr / fc);
                    v.comb_lp += (y - v.comb_lp) * c_damp;
                    if !v.comb_lp.is_finite() {
                        v.comb_lp = 0.0;
                    }
                    v.comb_line.push((filtered + v.comb_lp * c_fb).clamp(-8.0, 8.0));
                    filtered = filtered * (1.0 - comb) + y * c_norm * 1.4 * comb;
                }
                // frequency shift: every partial up or down by the same Hz
                let fs = fshift + m[12];
                if fs.abs() > 0.5 {
                    let (re, im) = v.hil.run(filtered);
                    v.fs_ph = (v.fs_ph + fs as f64 / sr as f64).rem_euclid(1.0);
                    let a = (std::f64::consts::TAU * v.fs_ph) as f32;
                    filtered = re * a.cos() + im * a.sin();
                }
                // drive (the folder and the asymmetry live in the shaper below)
                let pre = 1.0 + (self.v[P_DRV] * 30.0 + m[6]).max(0.0) * 1.0;
                let fold = (fold_p + m[11]).clamp(0.0, 1.0);
                // the shaper runs at twice the rate (an extra point halfway to the last sample, and a 1-2-1 average back down), which keeps
                // the new harmonics a fold or a clip makes from aliasing down into the audible range
                let sh = |u: f32| {
                    // fold first (a sine folder blended in), then the drive gain and the offset for even harmonics
                    let u = if fold > 0.001 { u * (1.0 - fold) + (u * (1.0 + 5.0 * fold) * std::f32::consts::FRAC_PI_2).sin() * fold } else { u };
                    let y = u * pre + asym;
                    let v = match shaper {
                        0 => (2.5 * y).tanh() / 2.5f32.tanh(),
                        1 => (3.5 * std::f32::consts::PI * (y / 24.0)).sin(),
                        _ => (4.0 * y).clamp(-1.0, 1.0),
                    };
                    v * 0.4
                };
                let u = filtered;
                let (cur, mid) = (sh(u), sh(0.5 * (v.os_y + u)));
                let mut shaped = 0.25 * v.os_sh + 0.5 * mid + 0.25 * cur;
                v.os_y = u;
                v.os_sh = cur;
                // the offset's DC out again
                if asym > 0.0 {
                    let d = shaped - v.dc_x + 0.995 * v.dc_y;
                    v.dc_x = shaped;
                    v.dc_y = d;
                    shaped = d;
                }
                // crush: fewer bits, a held sample
                if crush > 0.001 {
                    v.cr_n += 1;
                    if v.cr_n >= cr_hold {
                        v.cr_n = 0;
                        v.cr_h = (shaped / 0.4 * cr_bits).round() / cr_bits * 0.4;
                    }
                    shaped = v.cr_h;
                }
                x = shaped;
                // multiband distortion: lows clean, mids hard, highs soft
                if mb_on {
                    let (lo, _) = v.f_mb[0].run(x, mb_g1, 1.414);
                    let rest = x - lo;
                    let (mid, _) = v.f_mb[1].run(rest, mb_g2, 1.414);
                    let hi = rest - mid;
                    let ms = (mid * mb_dm).tanh() / mb_dm.sqrt();
                    let hs = (hi * mb_dh).tanh() / mb_dh.sqrt();
                    x = lo + mid * (1.0 - mbd) + ms * mbd + hi * (1.0 - mbd) + hs * mbd;
                }
                // 11 kHz low-pass
                let (lpo, _) = v.f_lp.run(x, lp11, k07);
                x = lpo;
                // sub, after the drive
                v.ph_sub += f0 * sub_mul / sr;
                v.ph_sub -= v.ph_sub.floor();
                let sub_w = (TAU * v.ph_sub).sin();
                let sub_w = if sub_sat > 0.002 { (sub_w * sub_sat_d).tanh() * sub_sat_n } else { sub_w };
                if poly {
                    x += sub_w * self.v[P_SUB] * 0.8 * v.sub_g;
                } else {
                    x += sub_w * self.v[P_SUB] * 0.8;
                }
                // amp
                let g = (1.0 + m[7]).clamp(0.0, 2.0);
                x *= amp_v * g * (0.3 + 0.7 * v.vel);
                if poly {
                    x *= poly_g;
                }
                let d = x - v.out_dc.0 + 0.9995 * v.out_dc.1;
                v.out_dc = (x, d);
                x = d;
                // pan
                let pan = m[8].clamp(-1.0, 1.0);
                let a = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
                l_out += x * a.cos() * std::f32::consts::SQRT_2;
                r_out += x * a.sin() * std::f32::consts::SQRT_2;
            }
            // sends (lanes can move the reverb and delay sends: matrix e and y)
            let lm = self.last_mod;
            let verb_s = if verb_mod { (verb + lm[D_VERB]).clamp(0.0, 1.0) } else { verb };
            let dely_s = if dely_mod { (dely + lm[D_DELY]).clamp(0.0, 1.0) } else { dely };
            let mono = (l_out + r_out) * 0.5;
            self.wide_l.push(mono);
            self.wide_r.push(mono);
            self.wide_ph += 1.0 / sr;
            let md1 = 0.0007 * sr * (TAU * 0.31 * self.wide_ph).sin();
            let md2 = 0.0007 * sr * (TAU * 0.23 * self.wide_ph + 1.3).sin();
            let wl = self.wide_l.read(wide_base[0] + md1);
            let wr = self.wide_r.read(wide_base[1] + md2);
            let d_out = self.dly.read(dly_len);
            self.dly_lp += (d_out - self.dly_lp) * 0.32;
            self.dly.push(mono * dely_s * 0.7 + self.dly_lp * 0.42);
            let mut l = l_out + wl * wide * 0.7 + d_out;
            let mut r = r_out + wr * wide * 0.7 + d_out;
            if ott_on {
                let (a, b) = self.ott.process(l, r, ott_c, ott_g1, ott_g2, &ott_att, &ott_rel);
                l = a;
                r = b;
            }
            if pro.on {
                let (a, b) = self.pro.process(&pro, l, r);
                l = a;
                r = b;
            }
            self.scr[i * 2] = l * verb_s * 0.9;
            self.scr[i * 2 + 1] = r * verb_s * 0.9;
            // limiter
            let pk = l.abs().max(r.abs());
            let tgt = if pk > 0.95 { 0.95 / pk } else { 1.0 };
            if tgt < self.lim {
                self.lim = tgt;
            } else {
                self.lim += (1.0 - self.lim) * 0.0008;
            }
            l *= self.lim * outg;
            r *= self.lim * outg;
            self.scr2[i * 2] = l;
            self.scr2[i * 2 + 1] = r;
            energy += l.abs() + r.abs();
        }
        for v in self.vo.iter_mut() {
            v.pb = v.pb_tgt;
            v.pb_step = 0.0;
        }
        // reverb send: wet only
        if verb > 0.001 || verb_mod {
            let dry = self.scr.clone();
            self.verb.process(&mut self.scr);
            for k in 0..out.len() {
                let wet = (self.scr[k] - dry[k]) * outg * self.lim;
                out[k] += self.scr2[k] + wet;
                energy += wet.abs();
            }
        } else {
            for k in 0..out.len() {
                out[k] += self.scr2[k];
            }
        }
        if energy < 1e-5 && self.vo.iter().all(|v| v.amp.stage == Stage::Idle) {
            self.silent = self.silent.saturating_add(frames as u32);
        } else {
            self.silent = 0;
        }
    }

    fn idle(&mut self, frames: usize) {
        // exactly what render() does to the free-running clocks when no note sounds, sample by sample (no audio): the lanes, the
        // talk sequencer, the width modulation; nothing at all once the Growl would have stopped rendering by itself
        if self.vo.iter().all(|v| v.amp.stage == Stage::Idle) && self.silent as f32 > self.sr * 6.0 {
            return;
        }
        let bpm = 60.0 + self.v[P_TEMPO] * 140.0;
        let bps = (bpm / 60.0) as f64 / self.sr as f64;
        let talk = self.v[P_TALK];
        let t_rate = T_RATES[self.sel(P_TRATE, 5)];
        let t_len = 1 + (self.v[P_TLEN] * 15.0).round() as usize;
        let dt = 1.0 / self.sr;
        let t_k = 1.0 - (-dt / (0.002 + self.v[P_TGLID] * self.v[P_TGLID] * 0.15)).exp();
        let r_cents = self.last_mod[9];
        for _ in 0..frames {
            for l in 0..NLANE {
                let rate = if l == 0 { 2f64.powf(r_cents as f64 / 1200.0) } else { 1.0 };
                self.lane_t[l] += bps * rate;
            }
            if talk > 0.002 {
                self.talk_t += bps * t_rate;
                let step = (self.talk_t as usize) % t_len;
                let target = self.v[P_TSTEP + step];
                self.talk_pos += (target - self.talk_pos) * t_k;
            }
            self.wide_ph += 1.0 / self.sr;
        }
        for l in 0..NLANE {
            self.lane_val[l] = self.lane_value(l);
        }
        self.silent = self.silent.saturating_add(frames as u32);
    }

    fn params(&self) -> Vec<(&'static str, f32)> {
        param_names().iter().zip(self.v.iter()).map(|(n, v)| (*n, *v)).collect()
    }

    fn set_param(&mut self, name: &str, v: f32) {
        if !v.is_finite() {
            return;
        }
        if name == "bend range" {
            return;
        }
        if let Some(i) = param_names().iter().position(|n| *n == name) {
            self.v[i] = v.clamp(0.0, 1.0);
            if (MAT_BASE..EXT_BASE).contains(&i) {
                self.mat_dirty = true;
            }
            if i == P_HOLD && v < 0.5 {
                // unlatching releases the voices no key holds any more
                let held: Vec<u8> = self.held.iter().map(|h| h.0).collect();
                for vo in self.vo.iter_mut().filter(|vo| vo.gate && !held.contains(&vo.key)) {
                    vo.gate = false;
                    vo.amp.off();
                    vo.menv.off();
                }
            }
        }
    }

    fn command(&mut self, cmd: &str, _args: &[f64], text: &str) -> Vec<f64> {
        match cmd {
            // a wavetable file into the user slot (and switch it on)
            // "load" restores a saved table (the project keeps the switch); "wt_load" also switches it on
            "load" | "wt_load" => match self.load_table(text) {
                Ok(n) => {
                    if cmd == "wt_load" {
                        self.v[P_USER] = 1.0;
                    }
                    vec![1.0, n as f64]
                }
                Err(e) => {
                    eprintln!("growl: {e}");
                    vec![0.0]
                }
            },
            "unload" | "wt_unload" => {
                self.wt = None;
                self.v[P_USER] = 0.0;
                vec![1.0]
            }
            _ => vec![],
        }
    }
    fn info(&self, what: &str) -> String {
        match what {
            // the user wavetable's file (saved with the project, loaded back through command "load")
            "config" | "file" | "wavetable" => self.wt.as_ref().map(|w| w.path.clone()).unwrap_or_default(),
            "frames" => self.wt.as_ref().map(|w| w.nframes().to_string()).unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn name(&self) -> &'static str {
        "Ling Growl"
    }
    fn get_param(&self, name: &str) -> Option<f32> {
        param_names().iter().position(|n| *n == name).map(|i| self.v[i])
    }
    fn plot(&self, what: &str, n: usize) -> Vec<f32> {
        let n = n.clamp(2, 1024);
        match what {
            "wave" if self.v[P_USER] > 0.5 && self.wt.is_some() => self.wt.as_ref().map(|w| w.plot(self.v[P_POS], n)).unwrap_or_default(),
            "wave" => {
                let tb = tables();
                let t = self.sel(P_TABLE, 4);
                let th = self.v[P_POS] * std::f32::consts::FRAC_PI_2;
                let (cx, cy) = (th.cos(), th.sin());
                (0..n).map(|i| {
                    let ph = i as f32 / n as f32;
                    cx * rd(&tb.w[t][0][0], ph) + cy * rd(&tb.w[t][1][0], ph)
                }).collect()
            }
            "filter" => {
                let ft = self.sel(P_FTYPE, 5);
                let q = 0.5 + self.v[P_RES] * self.v[P_RES] * 18.0;
                let fc = 30.0 * 2f32.powf(self.v[P_CUT] * 9.2);
                let vow_pos = self.v[P_VOW] * 4.0;
                let i0 = (vow_pos as usize).min(3);
                let fr = vow_pos - i0 as f32;
                let shift = 2f32.powf((self.v[P_CUT] - 0.5) * 2400.0 / 1200.0);
                (0..n).map(|i| {
                    let f = 20.0 * 1000f32.powf(i as f32 / (n - 1) as f32);
                    // complex response as (re, im)
                    let cmul = |a: (f32, f32), b: (f32, f32)| (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0);
                    let cdiv = |a: (f32, f32), b: (f32, f32)| {
                        let d = b.0 * b.0 + b.1 * b.1;
                        ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
                    };
                    let resp = |fc: f32, k: f32, kind: usize| {
                        let s = (0.0, f / fc);
                        let s2 = cmul(s, s);
                        let den = (s2.0 + 1.0, s2.1 + k * s.1);
                        let num = match kind {
                            0 => (1.0, 0.0),
                            1 => (0.0, k * s.1),
                            2 => s2,
                            _ => (s2.0 + 1.0, s2.1),
                        };
                        cdiv(num, den)
                    };
                    let h = if ft == 4 {
                        let k = 1.0 / (5.0 + self.v[P_RES] * 12.0);
                        let mut acc = (0.0f32, 0.0f32);
                        for j in 0..3 {
                            let fcj = (VOWELS[i0][j] + (VOWELS[i0 + 1][j] - VOWELS[i0][j]) * fr) * shift;
                            let r = resp(fcj, k, 1);
                            acc = (acc.0 + r.0 * VOW_GAIN[j], acc.1 + r.1 * VOW_GAIN[j]);
                        }
                        acc
                    } else {
                        resp(fc, 1.0 / q, ft)
                    };
                    let db = 20.0 * (h.0 * h.0 + h.1 * h.1).sqrt().max(1e-6).log10();
                    ((db + 24.0) / 48.0).clamp(0.0, 1.0)
                }).collect()
            }
            _ => vec![],
        }
    }
    fn presets(&self) -> &'static [&'static str] {
        &Self::PRESETS
    }
    fn randomize(&mut self, seed: u32) -> bool {
        self.random_patch(seed, None);
        true
    }
    fn load_preset(&mut self, name: &str) {
        if let Some(st) = Self::preset_state(name) {
            self.load_state(&st);
        }
    }

    /// Compact state for patch files: flat lanes and empty matrix cells are left out.
    fn state(&self) -> Vec<(String, f32)> {
        let names = param_names();
        let mut out = vec![];
        for (i, n) in names.iter().enumerate() {
            let v = self.v[i];
            if (MAT_BASE..EXT_BASE).contains(&i) && (v - 0.5).abs() < 1e-3 {
                continue;
            }
            if i >= EXT_BASE && (v - defaults()[i]).abs() < 1e-3 {
                continue;
            }
            if (LANE_BASE..MAT_BASE).contains(&i) {
                let l = (i - LANE_BASE) / LANE_STRIDE;
                let o = (i - LANE_BASE) % LANE_STRIDE;
                if o >= 4 {
                    let b = LANE_BASE + l * LANE_STRIDE + 4;
                    if self.v[b..b + NPTS].iter().all(|x| *x == 0.0) {
                        continue;
                    }
                }
            }
            out.push((n.to_string(), v));
        }
        out
    }

    /// Loading a state starts from the defaults, so a patch only has to name what it changes.
    fn load_state(&mut self, st: &[(String, f32)]) {
        self.v = defaults();
        for (n, v) in st {
            self.set_param(n, *v);
        }
        self.mat_dirty = true;
        self.dly.clear();
        for v in self.vo.iter_mut() {
            v.comb_line.clear();
        }
        self.lane_t = [0.0; NLANE];
    }
}

#[inline]
fn poly_blep(t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        return 0.0;
    }
    if t < dt {
        let x = t / dt;
        2.0 * x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + 2.0 * x + 1.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(g: &mut Growl, secs: f32) -> Vec<f32> {
        let n = (g.sr * secs) as usize;
        let mut out = vec![0.0f32; n * 2];
        for c in out.chunks_mut(128) {
            g.render(c);
        }
        out
    }
    fn peak(v: &[f32]) -> f32 {
        v.iter().fold(0.0f32, |m, x| m.max(x.abs()))
    }

    /// The new presets with their effect chains, one after another on a 4-bar riff at 140 bpm, into a WAV (and a cue list) at
    /// $GROWL_SHOWCASE (a folder). `cargo test -p ling-daw --release --lib render_preset_showcase -- --ignored`
    #[test]
    #[ignore]
    fn render_preset_showcase() {
        let dir = std::env::var("GROWL_SHOWCASE").unwrap_or_else(|_| ".".into());
        let sr = 44100.0f32;
        let spb = 60.0 / 140.0;
        let first = Growl::PRESETS.iter().position(|p| *p == "comb growl").unwrap_or(0);
        let mut all: Vec<f32> = vec![];
        let mut cues = String::new();
        for name in &Growl::PRESETS[first..] {
            let mut g = Growl::new(sr);
            g.load_preset(name);
            g.set_param("tempo", (140.0 - 60.0) / 140.0);
            let high = ["lead", "pluck", "chord", "stab", "pew", "future", "color", "zap", "screech", "squeal", "slow"].iter().any(|w| name.contains(w));
            let root: i32 = if high { 57 } else { 41 };
            // (beat, length, semitones): a held note, hits, a glide, a fall
            let riff: [(f64, f64, i32); 9] = [(0.0, 1.5, 0), (1.5, 0.5, 0), (2.0, 0.5, 3), (2.5, 1.5, 5), (4.0, 0.75, 0), (4.75, 0.75, 0), (5.5, 0.5, 7), (6.0, 1.0, 5), (6.9, 1.1, 3)];
            let mut chain: Vec<Box<dyn crate::dsp::fx::Effect>> = vec![];
            for (fxn, ps) in Growl::preset_fx(name) {
                if let Some(mut e) = crate::dsp::fx::make(&fxn, sr) {
                    for (k, v) in ps {
                        e.set_param(&k, v);
                    }
                    chain.push(e);
                }
            }
            let total = ((8.0 * 2.0 + 1.0) * spb * sr as f64) as usize;
            let mut out = vec![0f32; total * 2];
            let mut ev: Vec<(usize, bool, u8)> = vec![];
            for bar2 in 0..2 {
                for (b, l, o) in riff {
                    let st = b + bar2 as f64 * 8.0;
                    let k = (root + o) as u8;
                    ev.push(((st * spb * sr as f64) as usize, true, k));
                    ev.push((((st + l) * spb * sr as f64) as usize, false, k));
                }
            }
            ev.sort_by_key(|e| (e.0, e.1));
            let mut k = 0;
            let blk = 256;
            let mut f = 0;
            while f < total {
                while k < ev.len() && ev[k].0 <= f {
                    if ev[k].1 { g.note_on(ev[k].2, 0.9) } else { g.note_off(ev[k].2) }
                    k += 1;
                }
                let n = blk.min(total - f);
                let mut buf = vec![0f32; n * 2];
                g.render(&mut buf);
                for e in chain.iter_mut() {
                    e.process(&mut buf);
                }
                out[f * 2..(f + n) * 2].copy_from_slice(&buf);
                f += n;
            }
            let pk = out.iter().fold(1e-6f32, |m, x| m.max(x.abs()));
            let gain = 0.7 / pk;
            cues.push_str(&format!("{:7.2}s  {}  (peak before levelling {:.2})
", all.len() as f32 / 2.0 / sr, name, pk));
            all.extend(out.iter().map(|x| x * gain));
            all.extend(std::iter::repeat(0.0).take((sr * 0.4) as usize * 2));
        }
        let spec = hound::WavSpec { channels: 2, sample_rate: sr as u32, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let path = std::path::Path::new(&dir).join("growl_showcase.wav");
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for x in &all {
            w.write_sample((x.clamp(-1.0, 1.0) * 32767.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        std::fs::write(std::path::Path::new(&dir).join("growl_showcase.txt"), cues).unwrap();
    }

    fn amp_at(x: &[f32], hz: f32, sr: f32) -> f32 {
        let w = TAU * hz / sr;
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, v) in x.iter().enumerate() {
            re += v * (w * i as f32).cos();
            im += v * (w * i as f32).sin();
        }
        (re * re + im * im).sqrt() / x.len() as f32 * 2.0
    }
    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn the_pro_block_is_neutral_by_default_and_each_part_does_what_it_says() {
        let sr = 48000.0;
        // not written to a patch while neutral
        let g = Growl::new(sr);
        for (n, _) in g.state() {
            assert!(!PRO.contains(&n.as_str()), "{n}");
        }
        let play = |set: &[(&str, f32)], key: u8, vel: f32, secs: f32| {
            let mut g = Growl::new(sr);
            g.load_preset("growl");
            g.set_param("verb", 0.0);
            g.set_param("wide", 0.0);
            for (k, v) in set {
                g.set_param(k, *v);
            }
            g.note_on(key, vel);
            render(&mut g, secs)
        };
        let tail = |o: &[f32]| o[o.len() / 2..].to_vec();
        let mono = |o: &[f32]| o.chunks_exact(2).map(|f| f[0] + f[1]).collect::<Vec<f32>>();
        let hf = |o: &[f32]| {
            let m = mono(o);
            (4..14).map(|k| amp_at(&m, 3000.0 + k as f32 * 530.0, sr).powi(2)).sum::<f32>()
        };
        // multiband drive raises the highs
        assert!(hf(&tail(&play(&[("mb drv", 1.0), ("drv", 0.1)], 40, 0.9, 1.0))) > hf(&tail(&play(&[("drv", 0.1)], 40, 0.9, 1.0))) * 1.3, "mb drv");
        // air: more above 8 kHz
        let top = |o: &[f32]| {
            let m = mono(o);
            (2..8).map(|k| amp_at(&m, 8500.0 + k as f32 * 400.0, sr).powi(2)).sum::<f32>()
        };
        assert!(top(&tail(&play(&[("air", 1.0), ("drv", 0.7)], 40, 0.9, 1.0))) > top(&tail(&play(&[("drv", 0.7)], 40, 0.9, 1.0))) * 1.5, "air");
        // tilt: up = brighter, down = darker
        assert!(hf(&tail(&play(&[("tilt", 1.0)], 40, 0.9, 1.0))) > hf(&tail(&play(&[("tilt", 0.0)], 40, 0.9, 1.0))) * 2.0, "tilt");
        // sub saturation makes harmonics of the sub (the 3rd partial of a sine sub at A1 = 55 Hz)
        let sub3 = |sat: f32| {
            let o = tail(&play(&[("sub", 1.0), ("cut", 0.0), ("drv", 0.0), ("sub sat", sat), ("glid", 0.0), ("table", 0.0), ("pos", 0.0)], 45, 1.0, 1.0));
            let m = mono(&o);
            amp_at(&m, 55.0 * 3.0, sr) / amp_at(&m, 55.0, sr)
        };
        assert!(sub3(1.0) > sub3(0.0) * 3.0 + 0.01, "sub sat {} vs {}", sub3(1.0), sub3(0.0));
        // stereo: width 0 = mono, 1 = wider; mono lo keeps the sides out of the bass
        let side = |o: &[f32]| rms(&o.chunks_exact(2).map(|f| f[0] - f[1]).collect::<Vec<f32>>());
        let wide_p = [("wide", 0.9), ("dely", 0.0)];
        let _ = wide_p;
        let w1 = |set: &[(&str, f32)]| {
            let mut g = Growl::new(sr);
            g.load_preset("growl");
            g.set_param("verb", 0.0);
            g.set_param("wide", 0.9);
            for (k, v) in set {
                g.set_param(k, *v);
            }
            g.note_on(40, 0.9);
            tail(&render(&mut g, 1.2))
        };
        let (base, w0, wmax) = (side(&w1(&[])), side(&w1(&[("s width", 0.0)])), side(&w1(&[("s width", 1.0)])));
        assert!(w0 < base * 0.01 && wmax > base * 1.5, "width {w0} {base} {wmax}");
        let sd = |o: &[f32], lo: f32, hi: f32| {
            let s: Vec<f32> = o.chunks_exact(2).map(|f| f[0] - f[1]).collect();
            (0..8).map(|k| amp_at(&s, lo + (hi - lo) * k as f32 / 8.0, sr).powi(2)).sum::<f32>().sqrt()
        };
        let (off, on) = (w1(&[("wide", 0.9)]), w1(&[("mono lo", 0.9)]));
        assert!(sd(&on, 70.0, 140.0) < sd(&off, 70.0, 140.0) * 0.3 + 1e-5, "mono lo {} vs {}", sd(&on, 70.0, 140.0), sd(&off, 70.0, 140.0));
        // formant: the pitch stays, the brightness moves
        let pitch = |o: &[f32]| {
            let m = mono(o);
            let w = &m[m.len() / 2..m.len() / 2 + 4096];
            let mut best = (20usize, f32::MIN);
            for lag in 20..800 {
                let c: f32 = (0..w.len() - lag).map(|i| w[i] * w[i + lag]).sum();
                if c > best.1 {
                    best = (lag, c);
                }
            }
            sr / best.0 as f32
        };
        let fset = |f: f32| [("formant", f), ("sub", 0.0), ("uni", 0.0), ("detn", 0.0), ("table", 0.0), ("cut", 1.0), ("drv", 0.0), ("glid", 0.0)];
        let (flo, fneutral, fhi) = (play(&fset(0.5), 52, 0.9, 1.0), play(&fset(0.5), 52, 0.9, 1.0), play(&fset(0.9), 52, 0.9, 1.0));
        assert!((pitch(&fhi) / pitch(&fneutral) - 1.0).abs() < 0.03, "pitch stays: {} vs {}", pitch(&fhi), pitch(&fneutral));
        let _ = flo;
        assert!(hf(&tail(&fhi)) > hf(&tail(&fneutral)) * 1.5, "formant up = brighter");
        // OTT squeezes the dynamics: a soft and a loud note get closer in level
        let ratio = |ott: f32| {
            let q = rms(&tail(&play(&[("ott", ott), ("out", 0.6)], 40, 0.2, 1.5)));
            let l = rms(&tail(&play(&[("ott", ott), ("out", 0.6)], 40, 1.0, 1.5)));
            l / q
        };
        assert!(ratio(1.0) < ratio(0.0) * 0.8, "ott: {} vs {}", ratio(1.0), ratio(0.0));
        // all of it at once stays finite and under the limiter
        let all = play(&[("mb drv", 1.0), ("formant", 0.1), ("ott", 1.0), ("s width", 1.0), ("mono lo", 1.0), ("sub sat", 1.0), ("tilt", 0.8), ("air", 1.0)], 40, 1.0, 1.0);
        assert!(all.iter().all(|x| x.is_finite()));
        assert!(peak(&all) <= 1.0, "peak {}", peak(&all));
    }

    #[test]
    fn the_sound_design_block_stays_sane_at_its_extremes_and_does_what_it_says() {
        let sr = 44100.0;
        let base = |g: &mut Growl| {
            g.load_preset("growl");
        };
        for (k, v) in [("comb", 1.0), ("c fb", 0.0), ("c fb", 1.0), ("c tune", 0.0), ("c tune", 1.0), ("fold", 1.0), ("crush", 1.0), ("asym", 1.0), ("fshift", 0.0), ("fshift", 1.0), ("penv", 1.0), ("penv", 0.0), ("uni", 1.0), ("sub oct", 1.0)] {
            let mut g = Growl::new(sr);
            base(&mut g);
            g.set_param("comb", 0.6);
            g.set_param(k, v);
            for key in [24u8, 48, 84] {
                g.note_on(key, 1.0);
                let o = render(&mut g, 0.4);
                g.note_off(key);
                assert!(o.iter().all(|x| x.is_finite()), "{k}={v} key {key}: NaN");
                assert!(peak(&o) <= 1.0, "{k}={v} key {key}: peak {}", peak(&o));
                assert!(peak(&o) > 0.005, "{k}={v} key {key}: silent");
            }
        }
        // the pitch envelope: the first milliseconds are far higher than the rest (zero crossings per second)
        let mut g = Growl::new(sr);
        g.load_preset("zap");
        g.note_on(48, 1.0);
        let o = render(&mut g, 0.5);
        let zc = |v: &[f32]| v.chunks(2).map(|c| c[0]).collect::<Vec<_>>().windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        let n = (sr * 0.015) as usize * 2;
        assert!(zc(&o[..n]) * 2 > 3 * zc(&o[o.len() - n..]), "{} vs {}", zc(&o[..n]), zc(&o[o.len() - n..]));
        // every effect a preset suggests exists, with real parameter names
        for name in Growl::PRESETS {
            for (fxn, ps) in Growl::preset_fx(name) {
                let names: Vec<&str> = crate::dsp::fx::defaults(&fxn).iter().map(|p| p.0).collect();
                assert!(!names.is_empty(), "{name}: no effect {fxn}");
                for (p, _) in ps {
                    assert!(names.contains(&p.as_str()), "{name}: {fxn} has no {p}");
                }
            }
        }
        assert!(Growl::preset_fx("comb growl").len() == 2);
    }

    #[test]
    fn param_layout_is_consistent() {
        assert_eq!(param_names().len(), NPARAM);
        assert_eq!(param_names()[MAT_BASE], "m l1p");
        assert_eq!(param_names()[LANE_BASE + 4], "l1 p0");
        assert_eq!(param_names()[EXT_BASE - 1], "m whly");
        assert_eq!(param_names()[EXT_BASE], "talk");
        assert_eq!(param_names()[P_TSTEP], "t s0");
        assert_eq!(param_names()[SD_BASE - 1], "t s15");
        assert_eq!(param_names()[PRO_BASE - 1], "sub oct");
        assert_eq!(param_names()[PRO_BASE], "mb drv");
        assert_eq!(param_names()[X_BASE - 1], "air");
        assert_eq!(param_names()[X_BASE], "poly");
        assert_eq!(param_names()[NPARAM - 1], "l4 div");
    }

    #[test]
    fn every_preset_sounds_and_stays_sane() {
        for name in Growl::PRESETS {
            let mut g = Growl::new(48000.0);
            g.load_preset(name);
            assert!(Growl::preset_state(name).is_some(), "{name} missing");
            g.note_on(36, 0.9);
            // two seconds: a slow pad is still opening in the first one
            let a = render(&mut g, 2.0);
            g.note_off(36);
            let b = render(&mut g, 1.0);
            assert!(a.iter().chain(b.iter()).all(|x| x.is_finite()), "{name}: non-finite sample");
            let (pa, pb) = (peak(&a), peak(&b));
            let rms = (a.iter().map(|x| x * x).sum::<f32>() / a.len() as f32).sqrt();
            eprintln!("{name}: peak {pa:.3} rms {rms:.3}");
            assert!(pa > 0.02, "{name}: too quiet ({pa})");
            assert!(pa < 1.0, "{name}: clips ({pa})");
            assert!(pb <= pa + 1e-3, "{name}: release louder than note");
        }
    }

    #[test]
    fn state_round_trips_through_a_patch() {
        let mut g = Growl::new(48000.0);
        g.load_preset("neuro");
        let text = crate::patch::dump("growl", "neuro", &g.state(), &[]);
        let p = crate::patch::parse(&text);
        assert_eq!(p.kind, "growl");
        let g2 = crate::patch::build(&p, 48000.0).unwrap();
        for ((a, x), (b, y)) in g.params().iter().zip(g2.params().iter()) {
            assert_eq!(a, b);
            assert!((x - y).abs() < 0.002, "{a}: {x} vs {y}");
        }
    }

    #[test]
    fn loading_a_preset_resets_the_previous_one() {
        let mut g = Growl::new(48000.0);
        g.load_preset("neuro");
        g.load_preset("stab");
        let d = Growl::new(48000.0);
        // neuro's lane 3 must be gone
        let l3 = |g: &Growl| g.params().iter().filter(|(n, _)| n.starts_with("l3 p")).map(|(_, v)| *v).sum::<f32>();
        assert_eq!(l3(&g), l3(&d));
    }

    #[test]
    fn overlapping_notes_glide_without_retriggering() {
        let mut g = Growl::new(48000.0);
        g.note_on(36, 0.8);
        render(&mut g, 0.3);
        g.note_on(48, 0.8); // overlaps: legato
        assert_eq!(g.vo[0].amp.stage, Stage::Sus);
        g.note_off(48);
        assert!(g.vo[0].gate, "returns to the held note");
        assert_eq!(g.vo[0].target, 36.0);
        g.note_off(36);
        assert!(!g.vo[0].gate);
    }

    #[test]
    fn hold_latches_the_last_note() {
        let mut g = Growl::new(48000.0);
        g.set_param("hold", 1.0);
        g.note_on(40, 0.8);
        g.note_off(40);
        let a = render(&mut g, 0.5);
        assert!(peak(&a) > 0.02, "latched note must keep sounding");
        g.set_param("hold", 0.0);
        let b = render(&mut g, 2.0);
        assert!(peak(&b[b.len() - 2000..]) < 0.001, "unlatching releases it");
    }

    #[test]
    fn matrix_cells_map_to_the_documented_amounts() {
        let mut g = Growl::new(48000.0);
        g.set_param("m l1c", 1.0); // +4
        g.set_param("m envq", 0.125); // -3
        g.resolve_matrix();
        assert_eq!(g.mat[0][3], 6000.0);
        assert_eq!(g.mat[4][4], -8.0);
    }

    #[test]
    fn lanes_follow_their_shape_and_loop_settings() {
        let mut g = Growl::new(48000.0);
        for k in 0..16 {
            g.set_param(&format!("l1 p{k}"), k as f32 / 15.0);
        }
        g.set_param("l1 int", 0.5); // lin
        g.set_param("l1 len", 0.6); // 4 beats
        g.set_param("l1 loop", 0.0);
        g.lane_t[0] = 100.0; // long past the end: holds the last point
        assert!((g.lane_value(0) - 1.0).abs() < 1e-5);
        g.set_param("l1 loop", 1.0);
        g.lane_t[0] = 2.0; // half way round
        let v = g.lane_value(0);
        assert!((v - 8.0 / 15.0).abs() < 1e-4, "{v}");
        g.set_param("l1 int", 0.0); // step
        g.lane_t[0] = 0.1;
        assert_eq!(g.lane_value(0), 0.0);
    }

    #[test]
    fn talk_makes_the_vowel_move_and_sync_adds_screech() {
        let hold = |sets: &[(&str, f32)]| {
            let mut g = Growl::new(48000.0);
            for (n, v) in sets {
                g.set_param(n, *v);
            }
            g.note_on(40, 0.9);
            let out = render(&mut g, 2.0);
            assert!(out.iter().all(|x| x.is_finite()), "{sets:?}");
            out
        };
        let plain = hold(&[]);
        let talking = hold(&[("talk", 1.0)]);
        let d: f32 = plain.iter().zip(&talking).map(|(a, b)| (a - b).abs()).sum();
        assert!(d > 50.0, "talk changed nothing: {d}");
        // the sequence is audible as a change over time: the sound in the first beat differs from the same note a step later
        let a = &talking[..24000];
        let b = &talking[24000 * 2..24000 * 3];
        let dd: f32 = a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum();
        assert!(dd > 20.0, "the vowel does not move: {dd}");
        // sync: the stack runs 1 + 0.642857 * 7 = 5.5 times faster than the master, so a held E2 (82.4 Hz) grows a strong partial at 453 Hz, which the plain saw (harmonics at 412 and 494 Hz) does not have
        let power_at = |v: &[f32], hz: f32| {
            let w = TAU * hz / 48000.0;
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, x) in v.iter().enumerate() {
                re += x * (w * i as f32).cos();
                im += x * (w * i as f32).sin();
            }
            (re * re + im * im).sqrt() / v.len() as f32
        };
        let dry = hold(&[("pos", 0.0), ("cut", 0.9), ("drv", 0.0), ("sub", 0.0)]);
        let synced = hold(&[("pos", 0.0), ("cut", 0.9), ("drv", 0.0), ("sub", 0.0), ("sync", 0.642857)]);
        let f = 82.41 * 5.5;
        let (pd, ps) = (power_at(&dry[48000..48000 + 24000], f), power_at(&synced[48000..48000 + 24000], f));
        assert!(ps > pd * 2.0, "{ps} vs {pd}");
        assert!(peak(&synced) < 1.0 && peak(&talking) < 1.0);
        // both together, and a lane sweeping the sync ratio, stay finite and bounded
        let all = hold(&[("talk", 0.8), ("sync", 0.5), ("s mod", 1.0), ("t len", 0.4), ("t rate", 1.0), ("t res", 1.0), ("drv", 0.9)]);
        assert!(peak(&all) < 1.0);
    }

    #[test]
    fn extras_survive_a_patch_round_trip_and_default_ones_are_left_out() {
        let mut g = Growl::new(48000.0);
        g.set_param("talk", 0.5); // 0.5 must be saved: it is not the default
        g.set_param("t s3", 0.123);
        let st = g.state();
        assert!(st.iter().any(|(n, v)| n == "talk" && (*v - 0.5).abs() < 1e-6));
        assert!(st.iter().any(|(n, _)| n == "t s3"));
        assert!(!st.iter().any(|(n, _)| n == "t sh"), "defaults are not written");
        let text = crate::patch::dump("growl", "x", &st, &[]);
        let g2 = crate::patch::build(&crate::patch::parse(&text), 48000.0).unwrap();
        let get = |g: &Box<dyn Instrument>, n: &str| g.get_param(n).unwrap();
        assert!((get(&g2, "talk") - 0.5).abs() < 0.002 && (get(&g2, "t s3") - 0.123).abs() < 0.002);
    }

    #[test]
    fn random_patches_are_repeatable_musical_and_cover_every_family() {
        let mut seen = [false; ARCHETYPES.len()];
        for seed in 1..=60u32 {
            let mut g = Growl::new(48000.0);
            let fam = g.random_patch(seed, None);
            seen[fam] = true;
            let state = g.state();
            let mut g2 = Growl::new(48000.0);
            g2.random_patch(seed, None);
            assert_eq!(state, g2.state(), "seed {seed} is not repeatable");
            g.note_on(38, 0.9);
            let a = render(&mut g, 1.0);
            assert!(a.iter().all(|x| x.is_finite()), "seed {seed} ({}) non-finite", ARCHETYPES[fam]);
            assert!(peak(&a) < 1.0, "seed {seed} ({}) clips: {}", ARCHETYPES[fam], peak(&a));
            assert!(peak(&a) > 0.02, "seed {seed} ({}) silent: {}", ARCHETYPES[fam], peak(&a));
        }
        assert!(seen.iter().all(|s| *s), "{seen:?}");
        // the user's tempo and output level survive
        let mut g = Growl::new(48000.0);
        g.set_param("tempo", 0.9);
        g.set_param("out", 0.6);
        g.randomize(5);
        assert!((g.get_param("tempo").unwrap() - 0.9).abs() < 1e-6 && (g.get_param("out").unwrap() - 0.6).abs() < 1e-6);
    }

    /// not a test: writes 64 generated, named presets (`GROWL_OUT=<src dir> GROWL_INST=<instruments/Growl dir> cargo test ... dump_generated -- --ignored`)
    #[test]
    #[ignore]
    fn dump_generated_presets() {
        let (Ok(src), Ok(inst)) = (std::env::var("GROWL_OUT"), std::env::var("GROWL_INST")) else { return };
        const ADJ: [&str; 72] = [
            "rusty", "velvet", "acid", "glass", "iron", "neon", "dusty", "feral", "lunar", "molten", "silver", "toxic", "hollow", "brass", "static", "cosmic", "wicked", "gentle", "savage", "frozen", "burnt", "electric", "wooden", "liquid",
            "rotten", "golden", "violet", "scarlet", "amber", "cobalt", "crimson", "tidal", "ashen", "bitter", "sunken", "rapid", "heavy", "thin", "wide", "narrow", "mean", "kind", "sour", "sweet", "salty", "smoky", "stormy", "solar",
            "polar", "tribal", "urban", "rural", "ancient", "future", "retro", "crystal", "marble", "copper", "carbon", "plasma", "ghost", "shadow", "ember", "frost", "thunder", "whisper", "razor", "cotton", "gravel", "satin", "chrome", "jade",
        ];
        const NOUN: [[&str; 4]; 10] = [
            ["wub", "wobble", "womp", "throb"],
            ["talker", "mouth", "speaker", "voice"],
            ["screech", "shriek", "squeal", "scream"],
            ["riddim", "skank", "chop", "stab"],
            ["lead", "hook", "anthem", "beacon"],
            ["pad", "haze", "drift", "choir"],
            ["pluck", "snap", "pop", "tick"],
            ["sub", "drop", "boom", "rumble"],
            ["bell", "glass", "chime", "metal"],
            ["vox", "yoi", "throat", "formant"],
        ];
        let existing = std::fs::read_to_string(format!("{src}/growl_presets.txt")).unwrap();
        let mut taken: std::collections::HashSet<String> = existing.lines().filter(|l| l.starts_with('[') && l.ends_with(']')).map(|l| l[1..l.len() - 1].to_string()).collect();
        let mut text = existing.trim_end().to_string() + "
";
        let mut names: Vec<String> = vec![];
        let mut seed = 9000u32;
        let mut n = 0usize;
        while names.len() < 64 && n < 2000 {
            let fam = names.len() % ARCHETYPES.len();
            seed += 17;
            n += 1;
            let mut g = Growl::new(48000.0);
            g.random_patch(seed, Some(fam));
            g.note_on(38, 0.9);
            let a = render(&mut g, 1.0);
            g.note_off(38);
            let b = render(&mut g, 0.6);
            let pk = peak(&a);
            if !a.iter().chain(b.iter()).all(|x| x.is_finite()) || pk < 0.08 || pk > 0.95 || peak(&b) > pk + 1e-3 {
                continue;
            }
            // a name from the family's nouns and an adjective chosen by the seed, never one already taken
            let mut k = seed as usize;
            let name = loop {
                let cand = format!("{} {}", ADJ[k % ADJ.len()], NOUN[fam][(k / 7) % 4]);
                if !taken.contains(&cand) {
                    break cand;
                }
                k += 1;
            };
            taken.insert(name.clone());
            let body = crate::patch::dump("growl", &name, &g.state(), &[]);
            text.push_str(&format!("
[{name}]
{body}"));
            std::fs::write(format!("{inst}/Growl_{}.ling", name.replace(' ', "_")), &body).unwrap();
            names.push(name);
        }
        assert_eq!(names.len(), 64, "only {} presets passed the checks", names.len());
        std::fs::write(format!("{src}/growl_presets.txt"), text).unwrap();
        std::fs::write(format!("{src}/growl_generated_names.txt"), names.join("
")).unwrap();
    }

    // ── the synthesis block: per-note pitch, note-value lanes, FM, poly, the user wavetable ──
    /// a plain, bright, single-voice sound whose pitch is easy to measure
    fn plain(g: &mut Growl) {
        for (k, v) in [("table", 0.0), ("pos", 0.0), ("detn", 0.0), ("sub", 0.0), ("drv", 0.0), ("cut", 1.0), ("res", 0.0), ("verb", 0.0), ("wide", 0.0), ("glid", 0.0), ("uni", 0.0)] {
            g.set_param(k, v);
        }
    }
    /// a pure sine at the note (the FM carrier alone): its pitch is unambiguous
    fn sine(g: &mut Growl) {
        plain(g);
        g.set_param("fm", 1.0);
        g.set_param("fm idx", 0.0);
    }
    /// the strongest frequency in 30..2000 Hz of a stretch (fine Goertzel scan around the autocorrelation guess)
    fn pitch_of(x: &[f32], sr: f32) -> f32 {
        let m: Vec<f32> = x.chunks_exact(2).map(|f| f[0] + f[1]).collect();
        let w = &m[..m.len().min(8192)];
        let mut best = (20usize, f32::MIN);
        for lag in (sr / 1500.0) as usize..(sr / 35.0) as usize {
            let c: f32 = (0..w.len() - lag).map(|i| w[i] * w[i + lag]).sum::<f32>() / (w.len() - lag) as f32;
            if c > best.1 * 1.02 {
                best = (lag, c);
            }
        }
        let guess = sr / best.0 as f32;
        // refine: the strongest partial within a semitone of the guess
        (-50..=50).map(|c| guess * 2f32.powf(c as f32 / 1200.0)).max_by(|a, b| amp_at(&m, *a, sr).total_cmp(&amp_at(&m, *b, sr))).unwrap()
    }
    fn cents(a: f32, b: f32) -> f32 {
        1200.0 * (a / b).log2()
    }

    #[test]
    fn note_pitch_moves_the_sounding_note_by_the_asked_semitones() {
        let sr = 48000.0;
        let mut g = Growl::new(sr);
        sine(&mut g);
        g.note_on(45, 0.9); // A2 = 110 Hz
        let blk = 256;
        let run = |g: &mut Growl, secs: f32, semis: Option<(u8, f32)>| {
            let mut o = vec![];
            for _ in 0..(sr * secs) as usize / blk {
                if let Some((k, s)) = semis {
                    g.note_pitch(k, s);
                }
                let mut b = vec![0.0f32; blk * 2];
                g.render(&mut b);
                o.extend(b);
            }
            o
        };
        let a = run(&mut g, 0.4, None);
        assert!(cents(pitch_of(&a[a.len() / 2..], sr), 110.0).abs() < 8.0);
        let b = run(&mut g, 0.5, Some((45, 7.0)));
        let f = pitch_of(&b[b.len() / 2..], sr);
        assert!(cents(f, 110.0 * 2f32.powf(7.0 / 12.0)).abs() < 8.0, "+7 semitones: {f}");
        // a gesture for another key does nothing
        let c = run(&mut g, 0.3, Some((50, -12.0)));
        assert!(cents(pitch_of(&c[c.len() / 2..], sr), 164.81).abs() < 8.0);
        // smooth: no sample-to-sample jump while it slides (the steps of a 1-octave dive over 0.3 s stay small)
        let mut o = vec![];
        for k in 0..60 {
            g.note_pitch(45, 7.0 - 12.0 * k as f32 / 60.0);
            let mut b = vec![0.0f32; blk * 2];
            g.render(&mut b);
            o.extend(b);
        }
        assert!(o.iter().all(|x| x.is_finite()));
        let jump = o.chunks_exact(2).map(|f| f[0]).collect::<Vec<_>>().windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(jump < 0.5, "jump {jump}");
        // legato: a new key takes over with its own (fresh) gesture; the first value is taken at once (a scoop starts below)
        g.note_on(50, 0.9);
        let d = run(&mut g, 0.4, Some((50, -2.0)));
        assert!(cents(pitch_of(&d[d.len() / 2..], sr), 146.83 * 2f32.powf(-2.0 / 12.0)).abs() < 8.0, "legato key with its gesture");
        // poly: one note of a chord bends, the other stays
        let mut p = Growl::new(sr);
        sine(&mut p);
        p.set_param("poly", 0.4);
        p.note_on(45, 0.9);
        p.note_on(52, 0.9);
        let e = run(&mut p, 0.5, Some((52, 1.0)));
        let m: Vec<f32> = e[e.len() / 2..].chunks_exact(2).map(|f| f[0] + f[1]).collect();
        let a_bent = amp_at(&m, 164.81 * 2f32.powf(1.0 / 12.0), sr);
        let a_old = amp_at(&m, 164.81, sr);
        assert!(a_bent > a_old * 3.0, "the bent note moved: {a_bent} vs {a_old}");
        assert!(amp_at(&m, 110.0, sr) > a_old * 3.0, "the other note stays");
    }

    #[test]
    fn note_value_lanes_hit_triplet_and_dotted_times() {
        assert_eq!(note_div(0.0), None);
        assert_eq!(NOTE_DIVS.len(), 16);
        let idx = |name: &str| (NOTE_DIVS.iter().position(|d| d.0 == name).unwrap() + 1) as f32 / 16.0;
        assert!((note_div(idx("1/8T")).unwrap() - 1.0 / 3.0).abs() < 1e-6);
        assert!((note_div(idx("1/16T")).unwrap() - 1.0 / 6.0).abs() < 1e-6);
        assert_eq!(note_div(idx("1/8.")), Some(0.75));
        assert_eq!(note_div(idx("1/16.")), Some(0.375));
        // a gate lane (8 on, 8 off, steps) on the amplitude at 120 bpm: the gate opens every lane cycle
        let sr = 48000.0;
        for (name, beats) in [("1/8T", 1.0 / 3.0), ("1/16T", 1.0 / 6.0), ("1/8.", 0.75), ("1/4T", 2.0 / 3.0)] {
            let mut g = Growl::new(sr);
            plain(&mut g);
            g.set_param("tempo", (120.0 - 60.0) / 140.0);
            g.set_param("l1 int", 0.0);
            g.set_param("l1 div", idx(name));
            for k in 0..16 {
                g.set_param(&format!("l1 p{k}"), if k < 8 { 0.0 } else { 1.0 });
            }
            g.set_param("m l1a", 0.0); // -4: the lane closes the amp
            g.note_on(57, 0.9);
            let o = render(&mut g, 2.0);
            // 1 ms level steps, then the rising edges of the gate
            let lv: Vec<f32> = o.chunks(96).map(|c| c.iter().map(|x| x.abs()).fold(0.0f32, f32::max)).collect();
            let th = lv.iter().fold(0.0f32, |m, x| m.max(*x)) * 0.5;
            let edges: Vec<usize> = (1..lv.len()).filter(|&i| lv[i] >= th && lv[i - 1] < th).collect();
            assert!(edges.len() >= 3, "{name}: {edges:?}");
            let per: Vec<f32> = edges.windows(2).map(|w| (w[1] - w[0]) as f32 / 1000.0).collect();
            let want = beats * 0.5; // seconds at 120 bpm
            for p in &per[1..] {
                assert!((p - want).abs() < 0.003, "{name}: period {p} s, want {want}");
            }
            // the gate is open in the first half of every cycle: the note starts open, the next opening is one cycle later
            assert!(edges[0] < 5 && ((edges[1] - edges[0]) as f32 / 1000.0 - want).abs() < 0.004, "{name}: edges {edges:?}");
        }
    }

    #[test]
    fn the_fm_block_makes_the_expected_sidebands() {
        let sr = 48000.0;
        // two operators: carrier at f, a modulator at 1.5 f, index 1 radian: the sidebands are Bessel J0(1), J1(1), J2(1)
        let mut v = GVoice::new(sr, 0);
        let cfg = FmCfg { alg: 2, r: [1.0, 1.5, 1.0, 1.0], l: [1.0, 0.0, 0.0], fb: 0.0 };
        let f = 200.0;
        let x: Vec<f32> = (0..48000).map(|_| v.fm_sample(&cfg, f, sr, 1.0 / TAU)).collect();
        let a = |hz: f32| amp_at(&x, hz, sr);
        let (j0, j1, j2) = (0.7652f32, 0.4401f32, 0.1149f32);
        assert!((a(200.0) - j0).abs() < 0.02, "J0 {}", a(200.0));
        assert!((a(500.0) - j1).abs() < 0.02 && (a(100.0) - j1).abs() < 0.02, "J1 {} {}", a(500.0), a(100.0));
        assert!((a(800.0) - j2).abs() < 0.015 && (a(400.0) - j2).abs() < 0.015, "J2 {} {}", a(800.0), a(400.0));
        assert!(a(300.0) < 0.005 && a(600.0) < 0.005, "nothing between the sidebands: {} {}", a(300.0), a(600.0));
        // feedback on op 4 alone (alg fan, op 4 into the carrier) turns its sine into a saw-like stack: more harmonics
        let mut v2 = GVoice::new(sr, 0);
        let fbc = FmCfg { alg: 2, r: [1.0, 1.0, 1.0, 1.0], l: [0.0, 0.0, 1.0], fb: 0.4 };
        let y: Vec<f32> = (0..48000).map(|_| v2.fm_sample(&fbc, f, sr, 0.15)).collect();
        let nofb = FmCfg { fb: 0.0, ..fbc };
        let mut v3 = GVoice::new(sr, 0);
        let z: Vec<f32> = (0..48000).map(|_| v3.fm_sample(&nofb, f, sr, 0.15)).collect();
        let hi = |s: &[f32]| (3..8).map(|h| amp_at(s, f * h as f32, sr)).sum::<f32>();
        assert!(hi(&y) > hi(&z) * 1.5, "feedback brightens: {} vs {}", hi(&y), hi(&z));
        // in the instrument: fm 1 with op 2 at 1.5 puts a partial at half the note (the stack never has one there)
        let play = |fm: f32| {
            let mut g = Growl::new(sr);
            plain(&mut g);
            g.set_param("fm", fm);
            g.set_param("fm alg", 2.0 / 3.0);
            g.set_param("fm r2", 2.0 / 15.0);
            g.set_param("fm 2", 1.0);
            g.set_param("fm idx", 0.5);
            g.note_on(57, 0.9);
            let o = render(&mut g, 0.6);
            let m: Vec<f32> = o[o.len() / 2..].chunks_exact(2).map(|f| f[0] + f[1]).collect();
            (amp_at(&m, 110.0, sr), amp_at(&m, 220.0, sr), o)
        };
        let (h0, f0a, _) = play(0.0);
        let (h1, f1a, o1) = play(1.0);
        assert!(h1 > 0.005 && h1 > h0 * 100.0, "half-note sideband {h1} vs {h0}");
        assert!(f1a > 0.01 && f0a > 0.01);
        assert!(o1.iter().all(|x| x.is_finite()) && peak(&o1) < 1.0);
        // the index follows a lane through matrix i
        let mut g = Growl::new(sr);
        plain(&mut g);
        g.set_param("fm", 1.0);
        g.set_param("fm idx", 0.0);
        g.set_param("m l1i", 1.0);
        for k in 0..16 {
            g.set_param(&format!("l1 p{k}"), 1.0);
        }
        g.note_on(57, 0.9);
        let o = render(&mut g, 0.5);
        let m: Vec<f32> = o[o.len() / 2..].chunks_exact(2).map(|f| f[0] + f[1]).collect();
        assert!(amp_at(&m, 440.0, sr) > 0.01, "the lane opens the index");
    }

    #[test]
    fn poly_plays_chords_in_tune_and_the_default_stays_mono() {
        let sr = 48000.0;
        let g = Growl::new(sr);
        assert_eq!(g.npoly(), 1, "mono by default");
        assert!(!g.state().iter().any(|(n, _)| XP.contains(&n.as_str())), "the new block is not written while at its defaults");
        let chord = [45u8, 49, 52]; // A2 C#3 E3
        let play = |poly: f32| {
            let mut g = Growl::new(sr);
            sine(&mut g);
            g.set_param("fm idx", 0.3);
            g.set_param("poly", poly);
            for k in chord {
                g.note_on(k, 0.9);
            }
            render(&mut g, 1.0)
        };
        let fr = |k: u8| 440.0 * 2f32.powf((k as f32 - 69.0) / 12.0);
        let mono = play(0.0);
        let poly = play(0.4);
        let m = |o: &[f32]| o[o.len() / 2..].chunks_exact(2).map(|f| f[0] + f[1]).collect::<Vec<f32>>();
        let (mm, pm) = (m(&mono), m(&poly));
        // mono: only the last note sounds (A2 and C#3 are not harmonics of E3)
        assert!(amp_at(&mm, fr(45), sr) < amp_at(&mm, fr(52), sr) * 0.1, "mono plays one note");
        assert!(amp_at(&mm, fr(49), sr) < amp_at(&mm, fr(52), sr) * 0.1);
        // poly: every note, each at its own pitch (the strongest partial within +-40 cents sits within 4 cents of the key)
        for k in chord {
            let want = fr(k);
            let best = (-40i32..=40).map(|c| (c, amp_at(&pm, want * 2f32.powf(c as f32 / 1200.0), sr))).max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
            assert!(best.0.abs() <= 4, "key {k}: {} cents", best.0);
            assert!(best.1 > amp_at(&pm, want * 2f32.powf(0.5 / 12.0), sr) * 3.0, "key {k} stands out");
        }
        assert!(poly.iter().all(|x| x.is_finite()) && peak(&poly) < 1.0);
        // the tune check passes in poly mode too (as it does in mono: a preset without pitch modulation)
        let mut g = Growl::new(sr);
        g.load_preset("supersaw");
        g.set_param("poly", 1.0);
        let t = crate::tunecheck::check(&mut g, sr);
        assert!(t.in_tune(), "{t:?}");
        // more notes than voices: the oldest is stolen, nothing breaks
        let mut g = Growl::new(sr);
        g.set_param("poly", 0.2); // 2 voices
        for k in [40u8, 44, 47, 52] {
            g.note_on(k, 0.9);
        }
        assert_eq!(g.vo.iter().filter(|v| v.gate).count(), 2);
        let o = render(&mut g, 0.3);
        assert!(o.iter().all(|x| x.is_finite()));
        // release: every voice ends
        for k in [40u8, 44, 47, 52] {
            g.note_off(k);
        }
        render(&mut g, 2.0);
        assert!(g.vo.iter().all(|v| v.amp.stage == Stage::Idle));
    }

    #[test]
    fn a_user_wavetable_plays_its_frames() {
        let sr = 48000.0;
        // frame 0 a sine, frame 1 a bright pulse-ish cycle
        let n = super::super::wavetable::N;
        let sine: Vec<f32> = (0..n).map(|i| (TAU * i as f32 / n as f32).sin()).collect();
        let bright: Vec<f32> = (0..n).map(|i| (1..20).map(|h| ((TAU * (h * i) as f32 / n as f32).sin()) / h as f32).sum()).collect();
        let w = std::sync::Arc::new(super::super::wavetable::WaveTable::from_frames(&[sine, bright], "").unwrap());
        let play = |pos: f32| {
            let mut g = Growl::new(sr);
            plain(&mut g);
            g.wt = Some(w.clone());
            g.set_param("user", 1.0);
            g.set_param("pos", pos);
            g.note_on(45, 0.9);
            let o = render(&mut g, 0.5);
            o[o.len() / 2..].to_vec()
        };
        let (a, b) = (play(0.0), play(1.0));
        assert!(cents(pitch_of(&a, sr), 110.0).abs() < 8.0);
        let h = |o: &[f32]| {
            let m: Vec<f32> = o.chunks_exact(2).map(|f| f[0] + f[1]).collect();
            (2..6).map(|k| amp_at(&m, 110.0 * k as f32, sr)).sum::<f32>() / amp_at(&m, 110.0, sr)
        };
        assert!(h(&a) < 0.15, "frame 0 is a sine: {}", h(&a));
        assert!(h(&b) > h(&a) * 5.0 + 0.05, "frame 1 is bright: {} vs {}", h(&b), h(&a));
        let mut g = Growl::new(sr);
        g.wt = Some(w);
        g.set_param("user", 1.0);
        assert_eq!(g.plot("wave", 32).len(), 32);
    }

    #[test]
    fn pressure_and_timbre_shape_held_notes_and_a_release_keeps_its_bend() {
        let sr = 48000.0;
        let play = |prs: f32, tmb: f32| {
            let mut g = Growl::new(sr);
            plain(&mut g);
            g.set_param("cut", 0.45);
            g.note_on(45, 0.8);
            let mut o = vec![];
            for _ in 0..150 {
                g.note_pressure(45, prs);
                g.note_timbre(45, tmb);
                let mut b = vec![0.0f32; 512];
                g.render(&mut b);
                o.extend(b);
            }
            o[o.len() / 2..].to_vec()
        };
        let bright = |o: &[f32]| {
            let m: Vec<f32> = o.chunks_exact(2).map(|f| f[0] + f[1]).collect();
            (6..16).map(|k| amp_at(&m, 110.0 * k as f32, sr)).sum::<f32>()
        };
        let (n, p, t_up, t_dn) = (play(0.0, 0.5), play(1.0, 0.5), play(0.0, 1.0), play(0.0, 0.0));
        assert!(rms(&p) > rms(&n) * 1.1 && bright(&p) > bright(&n) * 1.5, "pressure: louder and brighter");
        assert!(bright(&t_up) > bright(&n) * 1.3 && bright(&t_dn) < bright(&n) * 0.8, "timbre moves the brightness");
        // neutral values change nothing
        let mut a = Growl::new(sr);
        let mut b = Growl::new(sr);
        a.note_on(45, 0.8);
        b.note_on(45, 0.8);
        b.note_pressure(45, 0.0);
        b.note_timbre(45, 0.5);
        assert_eq!(render(&mut a, 0.2), render(&mut b, 0.2));
        // a released note keeps its bend: the engine's reset after the note-off does not reach it
        let mut g = Growl::new(sr);
        sine(&mut g);
        g.set_param("amp r", 0.6);
        g.note_on(57, 0.9);
        for _ in 0..40 {
            g.note_pitch(57, -12.0);
            let mut b = vec![0.0f32; 512];
            g.render(&mut b);
        }
        g.note_off(57);
        g.note_pitch(57, 0.0);
        g.note_pressure(57, 0.0);
        g.note_timbre(57, 0.5);
        let o = render(&mut g, 0.15);
        assert!(cents(pitch_of(&o, sr), 110.0).abs() < 15.0, "the tail stays an octave down: {}", pitch_of(&o, sr));
    }
}
