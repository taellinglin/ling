//! Ling Nova: a wavetable synthesizer for modern electronic music and bass music (colour bass, growls, riddim, neuro and reese,
//! supersaws, future-bass chords, plucks, brass, talking basses, FM basses, big leads, evolving pads).
//!
//! Voice: three wavetable oscillators (A, B, C) - each with a table (48 generated ones, or a user `.wav` table through
//! `wavetable.rs`), a position that morphs smoothly between frames, octave / semitone / fine, level, pan, start phase and random
//! phase, and up to 16 unison voices (detune, blend, stereo width, detune curve) - plus two warps per oscillator (sync, bend, PWM,
//! asymmetry, mirror, remap, quantize, crush, fold, and FM / PM / AM / RM from another oscillator, the sub or the noise). Every
//! table is band-limited (the mip level follows the pitch, the unison spread and the warp), and an oscillator with a warp that
//! aliases runs at twice the sample rate through a half-band filter. A sub (sine / triangle / square / saw, one or two octaves
//! down) and a noise oscillator (white, pink, brown, crackle, digital, metallic) join them.
//!
//! Each source goes to filter 1, filter 2, both, or past them; the filters run in series or in parallel, each one of fourteen
//! types (low / high / band-pass 12 and 24, notch, ladder, diode, comb + / -, flange, phaser, formant) with drive, key tracking,
//! "fat" (saturation after the resonance), a variation knob (the formant's gender, the comb's damping, the phaser's spread) and a
//! mix. Four envelopes (AHDSR with curves, the first is the amp), four LFOs (eight shapes or a drawn eight-point shape; free or
//! note-value rates with triplets and dots; trig / free / one-shot envelope mode; delay and rise), two random sources, velocity,
//! the note, pressure, timbre, the mod wheel and eight macros feed a 32-slot matrix (amount, bipolar, curve, an aux source that
//! scales it) that reaches nearly every parameter, the effect rack included. Poly with a voice limit, mono or legato, portamento
//! (always / legato, a fixed time or a rate, a curve), a pitch-bend range. The rack (`fx.rs`) finishes the sound.
//!
//! The whole state is flat named 0..1 parameters (`layout.rs`), so patches, projects and automation need nothing special.
pub mod fx;
pub mod layout;
pub mod presets;
pub mod tables;
#[cfg(test)]
mod tests;

use super::bassfx::{g_of, Tsvf};
use super::wavetable::WaveTable;
use super::{midi_hz, Instrument, Rng, Smooth};
use layout::*;
use std::f32::consts::{FRAC_PI_4, PI, TAU};
use std::sync::{Arc, OnceLock};

/// voice slots (the poly limit is at most 16; a stolen voice fades out in its own slot)
const SLOTS: usize = 20;
const MAXU: usize = 16;
/// samples per control step (envelopes 2-4, LFOs, the matrix, the filter coefficients)
const CTRL: usize = 16;

// ── 2x oversampling ─────────────────────────────────────────────────────────────

const HB: usize = 31;
fn hb_coefs() -> &'static [f32; HB] {
    static C: OnceLock<[f32; HB]> = OnceLock::new();
    C.get_or_init(|| {
        // a windowed-sinc half-band low-pass (Blackman), cut at a quarter of the doubled rate
        let mut h = [0.0f32; HB];
        let m = (HB - 1) as f32 / 2.0;
        let mut s = 0.0;
        for (i, c) in h.iter_mut().enumerate() {
            let n = i as f32 - m;
            let sinc = if n == 0.0 { 0.5 } else { (PI * n * 0.5).sin() / (PI * n) };
            let w = 0.42 - 0.5 * (TAU * i as f32 / (HB - 1) as f32).cos() + 0.08 * (2.0 * TAU * i as f32 / (HB - 1) as f32).cos();
            *c = sinc * w;
            s += *c;
        }
        for c in h.iter_mut() {
            *c /= s;
        }
        h
    })
}

/// a 2x decimator: two samples at the doubled rate in, one out
#[derive(Clone, Copy)]
pub struct Os2 {
    buf: [f32; HB * 2],
    pos: usize,
    pub last_in: f32,
}
impl Os2 {
    pub fn new() -> Os2 {
        Os2 { buf: [0.0; HB * 2], pos: 0, last_in: 0.0 }
    }
    #[inline]
    fn push(&mut self, x: f32) {
        self.pos = if self.pos == 0 { HB - 1 } else { self.pos - 1 };
        self.buf[self.pos] = x;
        self.buf[self.pos + HB] = x;
    }
    #[inline]
    pub fn down(&mut self, a: f32, b: f32) -> f32 {
        self.push(a);
        self.push(b);
        let h = hb_coefs();
        let w = &self.buf[self.pos..self.pos + HB];
        let mut y = 0.0;
        for i in 0..HB {
            y += h[i] * w[i];
        }
        y
    }
}
impl Default for Os2 {
    fn default() -> Self {
        Os2::new()
    }
}

// ── envelopes ───────────────────────────────────────────────────────────────────

/// an envelope time knob in seconds: 0.5 ms .. 15 s
pub fn env_time(v: f32) -> f32 {
    0.0005 * 30000f32.powf(v.clamp(0.0, 1.0))
}
/// a curve knob as the exponent of the stage (0.5 = straight, higher = a faster start)
fn curve_exp(c: f32) -> f32 {
    8f32.powf((0.5 - c) * 2.0)
}

#[derive(Clone, Copy, Default)]
struct EnvP {
    a: f32,
    h: f32,
    d: f32,
    s: f32,
    r: f32,
    pa: f32,
    pd: f32,
    pr: f32,
}
#[derive(Clone, Copy, Default)]
struct Env {
    stage: u8,
    t: f32,
    lvl: f32,
    from: f32,
}
impl Env {
    fn gate(&mut self) {
        self.stage = 1;
        self.t = 0.0;
        self.from = self.lvl;
    }
    fn release(&mut self) {
        if self.stage != 0 && self.stage != 5 {
            self.stage = 5;
            self.t = 0.0;
            self.from = self.lvl;
        }
    }
    #[inline]
    fn tick(&mut self, p: &EnvP, dt: f32) -> f32 {
        match self.stage {
            1 => {
                self.t += dt;
                let u = (self.t / p.a).min(1.0);
                self.lvl = self.from + (1.0 - self.from) * u.powf(p.pa);
                if u >= 1.0 {
                    self.stage = 2;
                    self.t = 0.0;
                }
            }
            2 => {
                self.t += dt;
                self.lvl = 1.0;
                if self.t >= p.h {
                    self.stage = 3;
                    self.t = 0.0;
                }
            }
            3 => {
                self.t += dt;
                let u = (self.t / p.d).min(1.0);
                self.lvl = p.s + (1.0 - p.s) * (1.0 - u.powf(p.pd));
                if u >= 1.0 {
                    self.stage = 4;
                }
            }
            4 => self.lvl = p.s,
            5 => {
                self.t += dt;
                let u = (self.t / p.r).min(1.0);
                self.lvl = self.from * (1.0 - u.powf(p.pr));
                if u >= 1.0 {
                    self.stage = 0;
                    self.lvl = 0.0;
                }
            }
            _ => self.lvl = 0.0,
        }
        self.lvl
    }
}

/// an LFO's value 0..1 at phase `p` (shapes start at their low point, so a wobble opens from closed)
pub fn lfo_shape(shape: usize, p: f32, pts: &[f32; 8], smooth: f32, sh: (f32, f32)) -> f32 {
    match shape {
        0 => 0.5 - 0.5 * (TAU * p).cos(),
        1 => 1.0 - (2.0 * p - 1.0).abs(),
        2 => p,
        3 => 1.0 - p,
        4 => if p < 0.5 { 1.0 } else { 0.0 },
        5 => if p < 0.25 { 1.0 } else { 0.0 },
        6 => sh.1,
        7 => sh.0 + (sh.1 - sh.0) * (0.5 - 0.5 * (PI * p).cos()),
        _ => {
            let x = p * 8.0;
            let i = (x as usize).min(7);
            let f = x - i as f32;
            let (a, b) = (pts[i], pts[(i + 1) % 8]);
            // 0 = steps, 0.5 = straight lines, 1 = smooth curves
            let lin = a + (b - a) * f;
            if smooth < 0.5 {
                let k = smooth * 2.0;
                a + (lin - a) * k
            } else {
                let cs = a + (b - a) * (0.5 - 0.5 * (PI * f).cos());
                lin + (cs - lin) * (smooth - 0.5) * 2.0
            }
        }
    }
}

/// an LFO rate knob in Hz (free): 0.02 .. 41 Hz
pub fn lfo_hz(v: f32) -> f32 {
    0.02 * 2f32.powf(v.clamp(0.0, 1.0) * 11.0)
}

// ── filters ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Default)]
struct FiltRun {
    on: bool,
    kind: usize,
    g: f32,
    k: f32,
    /// pre-gain of the drive (1 = none), the fat amount, the mix
    dg: f32,
    fat: f32,
    mix: f32,
    /// comb: delay in samples, feedback, damping coefficient, sign of the flange
    cd: f32,
    cfb: f32,
    cdamp: f32,
    /// ladder / diode: one-pole coefficient, feedback
    lg: f32,
    lk: f32,
    /// formant: three band-pass coefficients and gains
    fg: [f32; 3],
    fa: [f32; 3],
    /// phaser: six all-pass coefficients
    pg: [f32; 6],
}

struct FiltState {
    sv: [[Tsvf; 3]; 2],
    lad: [[f32; 4]; 2],
    lad_y: [f32; 2],
    ap: [[f32; 6]; 2],
    ap_y: [f32; 2],
    comb: [Vec<f32>; 2],
    cpos: usize,
    clp: [f32; 2],
}
impl FiltState {
    fn new(sr: f32) -> FiltState {
        let n = ((sr / 12.0) as usize).next_power_of_two();
        FiltState { sv: [[Tsvf::default(); 3]; 2], lad: [[0.0; 4]; 2], lad_y: [0.0; 2], ap: [[0.0; 6]; 2], ap_y: [0.0; 2], comb: [vec![0.0; n], vec![0.0; n]], cpos: 0, clp: [0.0; 2] }
    }
    fn clear(&mut self) {
        self.sv = [[Tsvf::default(); 3]; 2];
        self.lad = [[0.0; 4]; 2];
        self.lad_y = [0.0; 2];
        self.ap = [[0.0; 6]; 2];
        self.ap_y = [0.0; 2];
        for c in self.comb.iter_mut() {
            c.iter_mut().for_each(|x| *x = 0.0);
        }
        self.clp = [0.0; 2];
    }
    /// one stereo sample through the filter
    #[inline]
    fn run(&mut self, f: &FiltRun, x: [f32; 2]) -> [f32; 2] {
        let mut out = x;
        let n = self.comb[0].len();
        if matches!(f.kind, 9 | 10 | 11) {
            self.cpos = (self.cpos + 1) & (n - 1);
        }
        for c in 0..2 {
            let dry = x[c];
            let xi = if f.dg > 1.001 { (dry * f.dg).tanh() * (0.6 + 0.4 / f.dg) * 1.2 } else { dry };
            let y = match f.kind {
                0 => self.sv[c][0].run(xi, f.g, f.k).0,
                1 => {
                    let a = self.sv[c][0].run(xi, f.g, 1.414).0;
                    self.sv[c][1].run(a, f.g, f.k).0
                }
                2 => self.sv[c][0].run(xi, f.g, f.k).2,
                3 => {
                    let a = self.sv[c][0].run(xi, f.g, 1.414).2;
                    self.sv[c][1].run(a, f.g, f.k).2
                }
                4 => self.sv[c][0].run(xi, f.g, f.k).1 * f.k,
                5 => {
                    let a = self.sv[c][0].run(xi, f.g, f.k).1 * f.k;
                    self.sv[c][1].run(a, f.g, f.k).1 * f.k
                }
                6 => {
                    let (lo, _, hi) = self.sv[c][0].run(xi, f.g, f.k.max(0.3));
                    lo + hi
                }
                7 | 8 => {
                    // a four-pole ladder (the diode: three poles, its feedback saturated unevenly - the acid squelch)
                    let poles = if f.kind == 7 { 4 } else { 3 };
                    let fb = self.lad_y[c];
                    let fbs = if f.kind == 7 { fb.tanh() } else if fb > 0.0 { (fb * 1.3).tanh() } else { (fb * 0.7).tanh() * 1.2 };
                    let mut u = (xi * (1.0 + f.lk * 0.25 * (1.0 + f.fat)) - f.lk * fbs).tanh();
                    for s in self.lad[c].iter_mut().take(poles) {
                        let v = (u - *s) * f.lg;
                        let yy = v + *s;
                        *s = yy + v;
                        u = yy;
                    }
                    self.lad_y[c] = u;
                    u
                }
                9 | 10 | 11 => {
                    let buf = &mut self.comb[c];
                    let rp = self.cpos as f32 + n as f32 - f.cd;
                    let i0 = rp.floor();
                    let fr = rp - i0;
                    let i0 = i0 as usize & (n - 1);
                    let d = buf[i0] * (1.0 - fr) + buf[(i0 + 1) & (n - 1)] * fr;
                    self.clp[c] += (d - self.clp[c]) * f.cdamp;
                    let dl = self.clp[c];
                    match f.kind {
                        9 => {
                            buf[self.cpos] = xi + dl * f.cfb;
                            (xi + dl) * (1.0 - f.cfb.abs() * 0.55)
                        }
                        10 => {
                            buf[self.cpos] = xi - dl * f.cfb;
                            (xi - dl) * (1.0 - f.cfb.abs() * 0.55)
                        }
                        _ => {
                            buf[self.cpos] = xi + dl * f.cfb;
                            (xi + dl * f.k) * 0.6
                        }
                    }
                }
                12 => {
                    // six all-pass stages around the cutoff, fed back: the notches of a phaser as a filter
                    let mut u = xi + self.ap_y[c] * f.cfb;
                    for (j, s) in self.ap[c].iter_mut().enumerate() {
                        let v = (u - *s) * f.pg[j];
                        let lp = v + *s;
                        *s = lp + v;
                        u = 2.0 * lp - u;
                    }
                    self.ap_y[c] = u;
                    (xi + u) * 0.5
                }
                _ => {
                    // the formant filter: three band-passes on the vowel's formants
                    let mut s = 0.0;
                    for j in 0..3 {
                        s += self.sv[c][j].run(xi, f.fg[j], f.k).1 * f.k * f.fa[j];
                    }
                    s * 2.6 + xi * 0.04
                }
            };
            let y = if f.fat > 0.001 && !matches!(f.kind, 7 | 8) {
                let d = 1.0 + f.fat * 3.0;
                (y * d).tanh() / d.sqrt()
            } else {
                y
            };
            out[c] = dry + (y - dry) * f.mix;
        }
        out
    }
}

// ── voices ──────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct OscRun {
    on: bool,
    n: usize,
    dt: [f32; MAXU],
    gl: [f32; MAXU],
    gr: [f32; MAXU],
    pos: f32,
    level: usize,
    wt: [usize; 2],
    wa: [f32; 2],
    src: usize,
    route: usize,
}
impl Default for OscRun {
    fn default() -> Self {
        OscRun { on: false, n: 1, dt: [0.0; MAXU], gl: [0.0; MAXU], gr: [0.0; MAXU], pos: 0.0, level: 0, wt: [0; 2], wa: [0.0; 2], src: 0, route: 0 }
    }
}

#[derive(Clone, Copy, Default)]
struct Noise {
    pink: [f32; 7],
    brown: f32,
    hold: f32,
    cnt: f32,
    ph: [f32; 6],
    lp: f32,
}

struct Voice {
    on: bool,
    held: bool,
    kill: bool,
    fade: f32,
    key: u8,
    vel: f32,
    order: u64,
    note: f32,
    g_from: f32,
    g_to: f32,
    g_u: f32,
    g_dur: f32,
    pitch: Smooth,
    press: Smooth,
    timbre: Smooth,
    env: [Env; 4],
    envp: [EnvP; 4],
    lph: [f32; 4],
    lsh: [(f32, f32); 4],
    ldone: [bool; 4],
    age: f32,
    rph: [f32; 2],
    rval: [f32; 2],
    rtgt: [f32; 2],
    src: [f32; NSRC],
    mods: Vec<f32>,
    ph: [[f32; MAXU]; 3],
    run: [OscRun; 3],
    last: [f32; 5],
    osf: usize,
    subph: f32,
    sub: (usize, f32, f32, f32, usize, f32),
    nz: Noise,
    nzp: (usize, f32, f32, f32, usize, f32),
    dec: [Os2; 6],
    filt: [FiltState; 2],
    fr: [FiltRun; 2],
    cut_s: [f32; 2],
    amp_mod: f32,
    dc: [f32; 2],
    rng: Rng,
    ctrl_left: usize,
}

impl Voice {
    fn new(sr: f32, ndst: usize, seed: u32) -> Voice {
        Voice {
            on: false,
            held: false,
            kill: false,
            fade: 1.0,
            key: 60,
            vel: 0.0,
            order: 0,
            note: 60.0,
            g_from: 60.0,
            g_to: 60.0,
            g_u: 1.0,
            g_dur: 0.0,
            pitch: Smooth::at(0.0),
            press: Smooth::at(0.0),
            timbre: Smooth::at(0.5),
            env: [Env::default(); 4],
            envp: [EnvP::default(); 4],
            lph: [0.0; 4],
            lsh: [(0.0, 0.0); 4],
            ldone: [false; 4],
            age: 0.0,
            rph: [0.0; 2],
            rval: [0.5; 2],
            rtgt: [0.5; 2],
            src: [0.0; NSRC],
            mods: vec![0.0; ndst],
            ph: [[0.0; MAXU]; 3],
            run: [OscRun::default(); 3],
            last: [0.0; 5],
            osf: 1,
            subph: 0.0,
            sub: (0, 0.0, 0.0, 0.0, 3, 0.0),
            nz: Noise::default(),
            nzp: (0, 0.0, 0.0, 0.0, 0, 0.0),
            dec: [Os2::new(); 6],
            filt: [FiltState::new(sr), FiltState::new(sr)],
            fr: [FiltRun::default(); 2],
            cut_s: [-1.0; 2],
            amp_mod: 1.0,
            dc: [0.0; 2],
            rng: Rng::new(seed),
            ctrl_left: 0,
        }
    }
    /// the current pitch through the glide
    fn glide_note(&self, curve: f32) -> f32 {
        let u = self.g_u.clamp(0.0, 1.0);
        let e = 1.0 - (1.0 - u).powi(3);
        let s = u + (e - u) * curve;
        self.g_from + (self.g_to - self.g_from) * s
    }
}

/// the user table slot: the file, and the table
type UserTable = Option<Arc<WaveTable>>;

pub struct Nova {
    sr: f32,
    v: Vec<f32>,
    voices: Vec<Voice>,
    order: u64,
    last_note: f32,
    /// mono / legato: the keys held, oldest first
    stack: [u8; 16],
    nstack: usize,
    bpm: f32,
    gph: [f32; 4],
    gmods: Vec<f32>,
    gsrc: [f32; NSRC],
    /// the matrix: (src, dst, amount, bipolar, curve exponent, aux) of the slots in use
    cells: [(usize, usize, f32, bool, f32, usize); NSLOT],
    ncells: usize,
    cells_dirty: bool,
    rack: fx::Rack,
    mix: Vec<f32>,
    user: UserTable,
    preset: String,
    rng: Rng,
}

impl Nova {
    pub fn new(sr: f32) -> Nova {
        let l = layout();
        let n = Nova {
            sr,
            v: l.specs.iter().map(|s| s.def).collect(),
            voices: (0..SLOTS).map(|i| Voice::new(sr, l.ndst(), 977 + i as u32 * 7919)).collect(),
            order: 0,
            last_note: -1.0,
            stack: [0; 16],
            nstack: 0,
            bpm: 120.0,
            gph: [0.0; 4],
            gmods: vec![0.0; l.ndst()],
            gsrc: [0.0; NSRC],
            cells: [(0, 0, 0.0, false, 1.0, 0); NSLOT],
            ncells: 0,
            cells_dirty: true,
            rack: fx::Rack::new(sr),
            mix: vec![0.0; fx::CHUNK * 2],
            user: None,
            preset: String::new(),
            rng: Rng::new(12345),
        };
        n.prepare_tables();
        n
    }

    pub fn defaults() -> Vec<f32> {
        layout().specs.iter().map(|s| s.def).collect()
    }

    /// a parameter's value by name (None = no such parameter)
    pub fn value(&self, name: &str) -> Option<f32> {
        layout().index.get(name).map(|&i| self.v[i])
    }
    pub fn values(&self) -> &[f32] {
        &self.v
    }
    pub fn set_values(&mut self, vals: &[f32]) {
        let n = self.v.len().min(vals.len());
        self.v[..n].copy_from_slice(&vals[..n]);
        self.cells_dirty = true;
        self.prepare_tables();
    }

    /// build the factory tables the oscillators point at now (not in the audio thread)
    fn prepare_tables(&self) {
        let l = layout();
        for o in 0..3 {
            let i = sel(self.v[l.osc[o].table], tables::NT + 1);
            if i < tables::NT {
                let _ = tables::table(i);
            }
        }
    }

    /// Load a wavetable file into the user slot (the last entry of every oscillator's table list).
    pub fn load_table(&mut self, path: &str) -> Result<usize, String> {
        let w = WaveTable::load(path)?;
        let n = w.nframes();
        self.user = Some(w);
        Ok(n)
    }

    fn refresh_cells(&mut self) {
        let l = layout();
        self.ncells = 0;
        for m in l.mat.iter() {
            let s = sel(self.v[m.src], NSRC);
            let a = (self.v[m.amt] - 0.5) * 2.0;
            if s == 0 || a.abs() < 0.002 {
                continue;
            }
            let d = sel(self.v[m.dst], l.ndst());
            let c = 4f32.powf((self.v[m.crv] - 0.5) * 2.0);
            self.cells[self.ncells] = (s, d, a, self.v[m.bi] > 0.5, c, sel(self.v[m.aux], NSRC));
            self.ncells += 1;
        }
        self.cells_dirty = false;
    }

    #[inline]
    fn apply_matrix(&self, src: &[f32; NSRC], mods: &mut [f32]) {
        mods.iter_mut().for_each(|m| *m = 0.0);
        for &(s, d, a, bi, c, aux) in &self.cells[..self.ncells] {
            let mut x = src[s];
            if bi {
                x = x * 2.0 - 1.0;
            }
            if c != 1.0 {
                x = x.signum() * x.abs().powf(c);
            }
            if aux != 0 {
                x *= src[aux];
            }
            mods[d] += a * x;
        }
    }

    /// a parameter with its modulation (a voice's or the global one)
    #[inline]
    fn ev(&self, mods: &[f32], p: usize) -> f32 {
        let d = layout().dst_of[p];
        if d == usize::MAX { self.v[p] } else { (self.v[p] + mods[d]).clamp(0.0, 1.0) }
    }

    fn sync_hz(&self, sync: bool, rate_v: f32, div_v: f32, rmod: f32) -> f32 {
        if sync {
            let beats = DIV_BEATS[sel(div_v, DIVS.len())];
            self.bpm / 60.0 / beats * 2f32.powf(rmod * 4.0)
        } else {
            lfo_hz(rate_v)
        }
    }

    fn voice_limit(&self) -> usize {
        1 + (self.v[layout().voices] * 15.0).round() as usize
    }

    fn start_voice(&mut self, slot: usize, key: u8, vel: f32, glide_from: Option<f32>) {
        let l = layout();
        let porta = self.v[l.porta];
        self.order += 1;
        let order = self.order;
        let seed = self.rng.next();
        let gdur = glide_from.map(|f| self.glide_time(f, key as f32)).unwrap_or(0.0);
        let v = &mut self.voices[slot];
        v.rng = Rng::new((seed.to_bits() ^ (order as u32).wrapping_mul(2654435761)) | 1);
        v.on = true;
        v.held = true;
        v.kill = false;
        v.fade = 1.0;
        v.key = key;
        v.vel = vel.clamp(0.0, 1.0);
        v.order = order;
        v.pitch.reset(0.0);
        v.press.reset(0.0);
        v.timbre.reset(0.5);
        v.age = 0.0;
        v.ldone = [false; 4];
        v.ctrl_left = 0;
        v.cut_s = [-1.0; 2];
        v.dc = [0.0; 2];
        v.last = [0.0; 5];
        for e in v.env.iter_mut() {
            *e = Env::default();
            e.gate();
        }
        for f in v.filt.iter_mut() {
            f.clear();
        }
        v.dec = [Os2::new(); 6];
        v.nz = Noise::default();
        v.subph = 0.0;
        // phases: the start phase, scattered by the random phase knob, per unison voice
        for o in 0..3 {
            let ph0 = self.v[l.osc[o].phase];
            let rnd = self.v[l.osc[o].rand];
            for u in 0..MAXU {
                let r = (v.rng.next() + 1.0) * 0.5;
                v.ph[o][u] = (ph0 + rnd * r).fract();
            }
        }
        for k in 0..4 {
            v.lph[k] = self.v[l.lfo[k].phase];
            let a = (v.rng.next() + 1.0) * 0.5;
            let b = (v.rng.next() + 1.0) * 0.5;
            v.lsh[k] = (a, b);
        }
        for k in 0..2 {
            v.rph[k] = 0.0;
            v.rval[k] = (v.rng.next() + 1.0) * 0.5;
            v.rtgt[k] = v.rval[k];
        }
        let target = key as f32;
        match glide_from {
            Some(f) if porta > 0.001 => {
                v.g_from = f;
                v.g_to = target;
                v.g_u = 0.0;
                v.g_dur = gdur;
            }
            _ => {
                v.g_from = target;
                v.g_to = target;
                v.g_u = 1.0;
            }
        }
        v.note = v.glide_note(0.0);
    }

    fn glide_time(&self, from: f32, to: f32) -> f32 {
        let l = layout();
        let t = 0.0005 + self.v[l.porta].powi(2) * 3.0;
        if self.v[l.porta_rate] > 0.5 { t * ((to - from).abs() / 12.0).max(0.05) } else { t }
    }

    /// retarget a sounding voice (mono / legato): a glide to the new key, the envelopes again unless `legato`
    fn retarget(&mut self, slot: usize, key: u8, vel: f32, retrigger: bool, glide: bool) {
        let l = layout();
        let porta = self.v[l.porta];
        let curve = self.v[l.porta_curve];
        let from = self.voices[slot].glide_note(curve);
        let dur = self.glide_time(from, key as f32);
        let v = &mut self.voices[slot];
        v.key = key;
        v.held = true;
        if glide && porta > 0.001 {
            v.g_from = from;
            v.g_to = key as f32;
            v.g_u = 0.0;
            v.g_dur = dur;
        } else {
            v.g_from = key as f32;
            v.g_to = key as f32;
            v.g_u = 1.0;
        }
        if retrigger {
            v.vel = vel.clamp(0.0, 1.0);
            for e in v.env.iter_mut() {
                e.gate();
            }
            v.age = 0.0;
            v.ldone = [false; 4];
        }
    }

    /// the control step of one voice: envelopes 2-4, LFOs, random, the matrix, then the oscillator / filter set-up
    fn control(&self, v: &mut Voice, tabs: &[&WaveTable; 3]) {
        let l = layout();
        let sr = self.sr;
        let dt = CTRL as f32 / sr;
        // glide
        let curve = self.v[l.porta_curve];
        if v.g_u < 1.0 {
            v.g_u = (v.g_u + dt / v.g_dur.max(1e-4)).min(1.0);
        }
        v.note = v.glide_note(curve);
        v.age += dt;
        // envelopes (from the last step's modulation)
        for k in 0..4 {
            let e = l.env[k];
            v.envp[k] = EnvP {
                a: env_time(self.ev(&v.mods, e.a)),
                h: self.v[e.h].powi(2) * 2.0,
                d: env_time(self.ev(&v.mods, e.d)),
                s: self.ev(&v.mods, e.s),
                r: env_time(self.ev(&v.mods, e.r)),
                pa: curve_exp(self.v[e.ac]),
                pd: curve_exp(self.v[e.dc]),
                pr: curve_exp(self.v[e.rc]),
            };
            if k > 0 {
                let p = v.envp[k];
                v.env[k].tick(&p, dt);
            }
        }
        // LFOs
        for k in 0..4 {
            let x = l.lfo[k];
            let shape = sel(self.v[x.shape], LFO_SHAPES.len());
            let mode = sel(self.v[x.mode], LFO_MODES.len());
            let rd = l.dst_of[x.rate];
            let rmod = if rd == usize::MAX { 0.0 } else { v.mods[rd] };
            let hz = self.sync_hz(self.v[x.sync] > 0.5, (self.v[x.rate] + rmod).clamp(0.0, 1.0), self.v[x.div], if self.v[x.sync] > 0.5 { rmod } else { 0.0 });
            let delay = self.v[x.delay].powi(2) * 4.0;
            let ph = if mode == 1 {
                (self.gph[k] + self.v[x.phase]).fract()
            } else {
                if v.age >= delay && !v.ldone[k] {
                    let np = v.lph[k] + hz * dt;
                    if np >= 1.0 {
                        if mode == 2 {
                            v.ldone[k] = true;
                        } else {
                            // a new cycle: a new random step
                            v.lsh[k].0 = v.lsh[k].1;
                            v.lsh[k].1 = (v.rng.next() + 1.0) * 0.5;
                        }
                    }
                    v.lph[k] = if mode == 2 && np >= 1.0 { 0.99999 } else { np.fract() };
                }
                v.lph[k]
            };
            let mut pts = [0.0f32; 8];
            for (j, p) in pts.iter_mut().enumerate() {
                *p = self.v[x.pts[j]];
            }
            let mut y = lfo_shape(shape, ph, &pts, self.v[x.smooth], v.lsh[k]);
            let rise = self.v[x.rise].powi(2) * 8.0;
            if v.age < delay {
                y = 0.0;
            } else if rise > 0.0 {
                y *= ((v.age - delay) / rise).min(1.0);
            }
            v.src[S_L1 + k] = y;
        }
        // random sources
        for k in 0..2 {
            let x = l.rnd[k];
            if sel(self.v[x.mode], RND_MODES.len()) == 0 {
                let hz = self.sync_hz(self.v[x.sync] > 0.5, self.ev(&v.mods, x.rate), self.v[x.div], 0.0);
                v.rph[k] += hz * dt;
                if v.rph[k] >= 1.0 {
                    v.rph[k] = v.rph[k].fract();
                    v.rtgt[k] = (v.rng.next() + 1.0) * 0.5;
                }
                let sm = self.v[x.smooth];
                let kk = if sm < 0.01 { 1.0 } else { (dt / (sm * sm * 0.5 + 1e-4)).min(1.0) };
                v.rval[k] += (v.rtgt[k] - v.rval[k]) * kk;
            }
            v.src[S_R1 + k] = v.rval[k];
        }
        for k in 0..4 {
            v.src[S_E1 + k] = v.env[k].lvl;
        }
        v.src[S_VEL] = v.vel;
        v.src[S_NOTE] = v.key as f32 / 127.0;
        v.src[S_PRESS] = v.press.cur;
        v.src[S_TIMBRE] = v.timbre.cur;
        v.src[S_WHEEL] = self.v[l.wheel];
        v.src[S_GATE] = if v.held { 1.0 } else { 0.0 };
        for k in 0..8 {
            v.src[S_M1 + k] = self.v[l.macros[k]];
        }
        let src = v.src;
        self.apply_matrix(&src, &mut v.mods);
        let m = &v.mods;
        // pitch: the note, the glide, the gesture, the bend, the pitch modulation
        let bend = (self.v[l.bend] - 0.5) * 2.0 * self.v[l.bend_range] * 48.0;
        let base = v.note + v.pitch.cur + bend + m[D_PITCH] * 24.0;
        // which warps alias: the oscillators then run at twice the rate
        let mut osf = 1;
        for o in 0..3 {
            let x = l.osc[o];
            if self.v[x.on] < 0.5 {
                continue;
            }
            for w in 0..2 {
                if matches!(sel(self.v[x.wt[w]], WARPS.len()), W_SYNC | W_PWM | W_QUANT | W_CRUSH | W_FOLD | W_FM | W_PM | W_RM) {
                    osf = 2;
                }
            }
        }
        v.osf = osf;
        let rate = sr * osf as f32;
        for o in 0..3 {
            let x = l.osc[o];
            let r = &mut v.run[o];
            r.on = self.v[x.on] > 0.5;
            if !r.on {
                continue;
            }
            let oct = sel(self.v[x.oct], 9) as f32 - 4.0;
            let semi = (self.v[x.semi] * 24.0).round() - 12.0;
            let fine = (self.ev(m, x.fine) - 0.5) * 2.0;
            let semis = base + oct * 12.0 + semi + fine + m[D_APITCH + o] * 24.0;
            let f = midi_hz(semis);
            let n = 1 + (self.v[x.uni] * 15.0).round() as usize;
            let det = self.ev(m, x.det).powf(1.5);
            let blend = self.ev(m, x.blend);
            let width = self.ev(m, x.width);
            let e = 1.0 + self.v[x.curve] * 2.5;
            let pan = (self.ev(m, x.pan) - 0.5) * 2.0;
            let lvl = self.ev(m, x.level);
            r.n = n;
            let mut maxr = 1.0f32;
            let mut gs = [0.0f32; MAXU];
            let mut g2 = 0.0;
            for u in 0..n {
                let s = if n == 1 { 0.0 } else { -1.0 + 2.0 * u as f32 / (n - 1) as f32 };
                let sp = s.signum() * s.abs().powf(e);
                let ratio = 2f32.powf(sp * det / 12.0);
                maxr = maxr.max(ratio);
                r.dt[u] = (f * ratio / rate).min(0.49);
                // the middle voices are the louder, the outer ones follow the blend
                let g = 1.0 + (blend - 1.0) * s.abs().sqrt();
                gs[u] = g;
                g2 += g * g;
                // the stereo place alternates side, so the spread and the detune do not line up
                let p = (pan + width * s * if u % 2 == 0 { 1.0 } else { -1.0 }).clamp(-1.0, 1.0);
                let th = (p + 1.0) * FRAC_PI_4;
                r.gl[u] = th.cos();
                r.gr[u] = th.sin();
            }
            let norm = lvl * std::f32::consts::SQRT_2 * 0.5 / g2.max(1e-6).sqrt();
            for u in 0..n {
                r.gl[u] *= gs[u] * norm;
                r.gr[u] *= gs[u] * norm;
            }
            r.pos = self.ev(m, x.pos);
            // warps, and how far they push the harmonics up (for the mip level)
            let mut wf = 1.0f32;
            for w in 0..2 {
                r.wt[w] = sel(self.v[x.wt[w]], WARPS.len());
                r.wa[w] = self.ev(m, x.wa[w]);
                let a = r.wa[w];
                wf *= match r.wt[w] {
                    W_SYNC => 1.0 + a * 7.0,
                    W_BENDP | W_BENDM => 1.0 + a * 2.0,
                    W_PWM => 1.0 / (1.0 - a * 0.95),
                    W_ASYM => 1.0 + a * 2.0,
                    W_REMAP => 1.3,
                    W_FM => 1.0 + a * 4.0,
                    W_PM => 1.0 + a * 3.0,
                    _ => 1.0,
                };
            }
            r.src = sel(self.v[x.src], MOD_SRC.len());
            r.route = sel(self.v[x.route], ROUTES.len());
            // the band limit of the output rate: at 2x the extra octave only catches what the warps add
            r.level = WaveTable::level(f * maxr * wf, sr);
            let _ = tabs;
        }
        // sub and noise
        {
            let s = l.sub;
            let on = self.v[s.on] > 0.5;
            let f = midi_hz(base - 12.0 * (2 - sel(self.v[s.oct], 3)) as f32);
            let pan = (self.ev(m, s.pan) - 0.5) * 2.0;
            v.sub = (if on { sel(self.v[s.shape], 4) + 1 } else { 0 }, f / sr, self.ev(m, s.level), pan, sel(self.v[s.route], 4), 0.0);
            let z = l.noise;
            let on = self.v[z.on] > 0.5;
            let pan = (self.ev(m, z.pan) - 0.5) * 2.0;
            let color = self.ev(m, z.color);
            let tr = self.v[z.track];
            let f = midi_hz(60.0 + (base - 60.0) * tr) / sr;
            v.nzp = (if on { sel(self.v[z.kind], NOISES.len()) + 1 } else { 0 }, self.ev(m, z.level), pan, color, sel(self.v[z.route], 4), f);
        }
        // filters
        for k in 0..2 {
            let x = l.filt[k];
            let fr = &mut v.fr[k];
            fr.on = self.v[x.on] > 0.5;
            if !fr.on {
                continue;
            }
            fr.kind = sel(self.v[x.kind], FTYPES.len());
            let kt = self.v[x.key];
            let tgt = 20.0 * 1000f32.powf(self.ev(m, x.cut)) * 2f32.powf((v.note - 60.0) / 12.0 * kt);
            // the cutoff glides a little between steps (no zipper when it is swept)
            if v.cut_s[k] < 0.0 {
                v.cut_s[k] = tgt;
            } else {
                v.cut_s[k] += (tgt - v.cut_s[k]) * 0.6;
            }
            let fc = v.cut_s[k].clamp(15.0, sr * 0.45);
            let res = self.ev(m, x.res);
            fr.g = g_of(fc, sr);
            fr.k = 2.0 - 1.96 * res.sqrt();
            fr.dg = 1.0 + self.ev(m, x.drive) * 15.0;
            fr.fat = self.ev(m, x.fat);
            fr.mix = self.ev(m, x.mix);
            let var = self.ev(m, x.var);
            match fr.kind {
                7 | 8 => {
                    let g = (PI * fc.min(sr * 0.42) / sr).tan();
                    fr.lg = g / (1.0 + g);
                    fr.lk = res * if fr.kind == 7 { 4.2 } else { 3.6 };
                }
                9 | 10 | 11 => {
                    let n = v.filt[k].comb[0].len() as f32;
                    fr.cd = (sr / fc.max(12.0)).clamp(1.0, n - 2.0);
                    fr.cfb = (res * 0.97).min(0.97);
                    fr.cdamp = 1.0 - (-TAU * (500.0 * 40f32.powf(1.0 - var)).min(sr * 0.45) / sr).exp();
                    if fr.kind == 11 {
                        // the flange: feed-forward and back with the same sign (the var knob turns it negative below half)
                        fr.k = if var < 0.5 { -1.0 } else { 1.0 };
                        fr.cfb *= fr.k * 0.92;
                        fr.cdamp = 1.0;
                    }
                }
                12 => {
                    let spread = 0.2 + var * 1.6;
                    for j in 0..6 {
                        let fj = (fc * 2f32.powf((j as f32 - 2.5) * spread * 0.5)).clamp(20.0, sr * 0.45);
                        let g = (PI * fj / sr).tan();
                        fr.pg[j] = g / (1.0 + g);
                    }
                    fr.cfb = res * 0.9;
                }
                13 => {
                    // the cutoff knob walks the vowel a-e-i-o-u, var the size of the throat
                    let vow = tables::vowel_at(self.ev(m, x.cut));
                    let shift = 2f32.powf((var - 0.5) * 1.4);
                    for j in 0..3 {
                        fr.fg[j] = g_of((vow[j].0 * shift).min(sr * 0.45), sr);
                        fr.fa[j] = vow[j].1;
                    }
                    fr.k = 0.5 - 0.42 * res;
                }
                _ => {}
            }
        }
        v.amp_mod = (1.0 + m[D_AMP]).clamp(0.0, 2.0);
    }

    /// render one voice into `buf` (adds)
    fn voice_render(&self, v: &mut Voice, buf: &mut [f32], tabs: &[&WaveTable; 3]) {
        let l = layout();
        let sr = self.sr;
        let dt1 = 1.0 / sr;
        let serial = sel(self.v[l.froute], 2) == 0;
        let vel_amp = self.v[l.vel_amp];
        let kill_k = (-1.0 / (0.003 * sr)).exp();
        for fr in buf.chunks_exact_mut(2) {
            if !v.on {
                break;
            }
            if v.ctrl_left == 0 {
                self.control(v, tabs);
                v.ctrl_left = CTRL;
            }
            v.ctrl_left -= 1;
            v.pitch.tick();
            v.press.tick();
            v.timbre.tick();
            let p1 = v.envp[0];
            let amp = v.env[0].tick(&p1, dt1);
            // the oscillators, at 1x or 2x, summed onto their routes (f1, f2, dry) per channel
            let mut bus = [[0.0f32; 2]; 3];
            let mut used = [false; 3];
            let mut halves = [[[0.0f32; 2]; 3]; 2];
            for h in 0..v.osf {
                for o in 0..3 {
                    let r = v.run[o];
                    if !r.on {
                        continue;
                    }
                    let tab = tabs[o];
                    let ms = v.last[r.src];
                    let (mut sl, mut sr_) = (0.0f32, 0.0f32);
                    let mid = r.n / 2;
                    let mut center = 0.0;
                    for u in 0..r.n {
                        let mut dt = r.dt[u];
                        for w in 0..2 {
                            if r.wt[w] == W_FM {
                                dt *= 1.0 + r.wa[w] * 4.0 * ms;
                            }
                        }
                        let mut ph = v.ph[o][u] + dt;
                        ph -= ph.floor();
                        v.ph[o][u] = ph;
                        let mut p = ph;
                        let mut gate = 1.0;
                        for w in 0..2 {
                            let a = r.wa[w];
                            if a <= 0.0 {
                                continue;
                            }
                            match r.wt[w] {
                                W_SYNC => {
                                    // the slave restarts with the master: faded to nothing around the restart, so the jump is soft
                                    let fw = (dt * 3.0).max(0.01);
                                    gate *= (ph / fw).min((1.0 - ph) / fw).min(1.0);
                                    p = (p * (1.0 + a * 7.0)).fract();
                                }
                                W_BENDP | W_BENDM => {
                                    let x = 2.0 * p - 1.0;
                                    let e = if r.wt[w] == W_BENDP { 1.0 + a * 3.0 } else { 1.0 / (1.0 + a * 3.0) };
                                    p = (x.signum() * x.abs().powf(e) + 1.0) * 0.5;
                                }
                                W_PWM => {
                                    let wdt = 1.0 - a * 0.95;
                                    if p < wdt {
                                        // the squeezed cycle fades in and out at its edges (no hard step into the silence)
                                        let fw = (dt * 3.0 / wdt).max(0.01);
                                        p /= wdt;
                                        gate *= (p / fw).min((1.0 - p) / fw).min(1.0);
                                    } else {
                                        p = 0.0;
                                        gate = 0.0;
                                    }
                                }
                                W_ASYM => {
                                    let mpt = 0.5 - a * 0.45;
                                    p = if p < mpt { p * 0.5 / mpt } else { 0.5 + (p - mpt) * 0.5 / (1.0 - mpt) };
                                }
                                W_MIRROR => {
                                    let mi = 1.0 - (2.0 * p - 1.0).abs();
                                    p += (mi * 0.5 - p) * a;
                                }
                                W_REMAP => p = (p - a * 0.15 * (TAU * p).sin()).rem_euclid(1.0),
                                W_QUANT => {
                                    let steps = 2f32.powf(2.0 + (1.0 - a) * 9.0);
                                    p = (p * steps).floor() / steps;
                                }
                                W_PM => p = (p + a * ms).rem_euclid(1.0),
                                _ => {}
                            }
                        }
                        let mut y = tab.read(r.pos, r.level, p) * gate;
                        for w in 0..2 {
                            let a = r.wa[w];
                            if a <= 0.0 {
                                continue;
                            }
                            match r.wt[w] {
                                W_CRUSH => {
                                    let q = 2f32.powf(1.0 + (1.0 - a) * 11.0);
                                    y = (y * q).round() / q;
                                }
                                W_FOLD => y = tables::fold(y * (1.0 + a * 6.0)),
                                W_AM => y *= (1.0 - a) + a * (0.5 + 0.5 * ms),
                                W_RM => y = y * (1.0 - a) + y * ms * a,
                                _ => {}
                            }
                        }
                        if u == mid {
                            center = y;
                        }
                        sl += y * r.gl[u];
                        sr_ += y * r.gr[u];
                    }
                    v.last[o] = center;
                    let rt = r.route;
                    let (b1, b2) = match rt {
                        0 => (0, 4),
                        1 => (1, 4),
                        2 => (0, 1),
                        _ => (2, 4),
                    };
                    for b in [b1, b2] {
                        if b < 3 {
                            halves[h][b][0] += sl;
                            halves[h][b][1] += sr_;
                            used[b] = true;
                        }
                    }
                }
            }
            for b in 0..3 {
                if !used[b] {
                    continue;
                }
                if v.osf == 2 {
                    bus[b][0] = v.dec[b * 2].down(halves[0][b][0], halves[1][b][0]);
                    bus[b][1] = v.dec[b * 2 + 1].down(halves[0][b][1], halves[1][b][1]);
                } else {
                    bus[b] = halves[0][b];
                }
            }
            // the sub
            let (sk, sdt, slv, span, srt, _) = v.sub;
            if sk > 0 {
                v.subph += sdt;
                if v.subph >= 1.0 {
                    v.subph -= 1.0;
                }
                let p = v.subph;
                let y = match sk {
                    1 => (TAU * p).sin(),
                    2 => 1.0 - 4.0 * (p - 0.5).abs(),
                    3 => {
                        let mut s = if p < 0.5 { 1.0 } else { -1.0 };
                        s += blep(p, sdt) - blep((p + 0.5).fract(), sdt);
                        s * 0.8
                    }
                    _ => (2.0 * p - 1.0 - blep(p, sdt)) * 0.8,
                };
                v.last[3] = y;
                let y = y * slv * 0.8;
                let th = (span + 1.0) * FRAC_PI_4;
                let (gl, gr) = (th.cos() * std::f32::consts::SQRT_2, th.sin() * std::f32::consts::SQRT_2);
                add_route(&mut bus, &mut used, srt, y * gl, y * gr);
            }
            // the noise
            let (nk, nlv, npan, ncol, nrt, nf) = v.nzp;
            if nk > 0 {
                let w = v.rng.next();
                let z = &mut v.nz;
                let y = match nk {
                    1 => w,
                    2 => {
                        // pink (Kellet)
                        z.pink[0] = 0.99886 * z.pink[0] + w * 0.0555179;
                        z.pink[1] = 0.99332 * z.pink[1] + w * 0.0750759;
                        z.pink[2] = 0.96900 * z.pink[2] + w * 0.1538520;
                        z.pink[3] = 0.86650 * z.pink[3] + w * 0.3104856;
                        z.pink[4] = 0.55000 * z.pink[4] + w * 0.5329522;
                        z.pink[5] = -0.7616 * z.pink[5] - w * 0.0168980;
                        let s = z.pink.iter().sum::<f32>() + w * 0.5362;
                        z.pink[6] = w * 0.115926;
                        s * 0.2
                    }
                    3 => {
                        z.brown = (z.brown + w * 0.04) * 0.995;
                        z.brown * 3.0
                    }
                    4 => {
                        // crackle: sparse clicks of random size
                        let th = 0.9993 - ncol * 0.004;
                        if (w + 1.0) * 0.5 > th { v.rng.next() * 1.5 } else { 0.0 }
                    }
                    5 => {
                        // digital: random steps at a rate that follows the key
                        z.cnt += nf * 4.0;
                        if z.cnt >= 1.0 {
                            z.cnt -= 1.0;
                            z.hold = if w > 0.0 { 0.7 } else { -0.7 };
                        }
                        z.hold
                    }
                    _ => {
                        // metallic: six square waves at inharmonic ratios
                        const R: [f32; 6] = [1.0, 1.342, 1.783, 2.214, 2.617, 3.191];
                        let mut s = 0.0;
                        for j in 0..6 {
                            z.ph[j] = (z.ph[j] + nf * 2.0 * R[j]).fract();
                            s += if z.ph[j] < 0.5 { 1.0 } else { -1.0 };
                        }
                        s * 0.15
                    }
                };
                // the colour: a gentle low-pass (dark) to bright
                let k = 0.02 + ncol * ncol * 0.98;
                z.lp += (y - z.lp) * k;
                let y = z.lp;
                v.last[4] = y;
                let y = y * nlv * 0.7;
                let th = (npan + 1.0) * FRAC_PI_4;
                add_route(&mut bus, &mut used, nrt, y * th.cos() * 1.414, y * th.sin() * 1.414);
            }
            // the filters
            let f1 = if v.fr[0].on && used[0] { v.filt[0].run(&v.fr[0], bus[0]) } else { bus[0] };
            let mut out = [bus[2][0], bus[2][1]];
            if serial {
                let x2 = [bus[1][0] + f1[0], bus[1][1] + f1[1]];
                let y2 = if v.fr[1].on { v.filt[1].run(&v.fr[1], x2) } else { x2 };
                out[0] += y2[0];
                out[1] += y2[1];
            } else {
                let y2 = if v.fr[1].on && used[1] { v.filt[1].run(&v.fr[1], bus[1]) } else { bus[1] };
                out[0] += f1[0] + y2[0];
                out[1] += f1[1] + y2[1];
            }
            // the amp
            let vg = 1.0 - vel_amp * (1.0 - v.vel);
            let press = 1.0 + v.press.cur * 0.3;
            if v.kill {
                v.fade *= kill_k;
            }
            let g = amp * vg * v.amp_mod * press * v.fade;
            for c in 0..2 {
                let y = out[c] * g;
                v.dc[c] += (y - v.dc[c]) * 0.0004;
                let y = y - v.dc[c];
                fr[c] += if y.is_finite() { y } else { 0.0 };
            }
            if v.env[0].stage == 0 || (v.kill && v.fade < 1e-4) {
                v.on = false;
            }
        }
    }

    fn mono(&self) -> usize {
        sel(self.v[layout().vmode], VOICE_MODES.len())
    }

    /// what the usage table says about the loaded preset
    pub fn preset_name(&self) -> &str {
        &self.preset
    }
}

#[inline]
fn add_route(bus: &mut [[f32; 2]; 3], used: &mut [bool; 3], route: usize, l: f32, r: f32) {
    let (a, b) = match route {
        0 => (0, 4),
        1 => (1, 4),
        2 => (0, 1),
        _ => (2, 4),
    };
    for k in [a, b] {
        if k < 3 {
            bus[k][0] += l;
            bus[k][1] += r;
            used[k] = true;
        }
    }
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

/// a soft ceiling: untouched below 0.8, never past 1
#[inline]
fn ceiling(x: f32) -> f32 {
    let a = x.abs();
    if a < 0.8 { x } else { x.signum() * (0.8 + 0.2 * ((a - 0.8) / 0.2).tanh()) }
}

impl Instrument for Nova {
    fn note_on(&mut self, key: u8, vel: f32) {
        if vel <= 0.0 {
            self.note_off(key);
            return;
        }
        let l = layout();
        let mode = self.mono();
        let porta_legato = sel(self.v[l.porta_mode], 2) == 1;
        let curve = self.v[l.porta_curve];
        if mode == 0 {
            let any_held = self.voices.iter().any(|v| v.on && v.held && !v.kill);
            let glide_from = if self.last_note >= 0.0 && (!porta_legato || any_held) { Some(self.last_note) } else { None };
            // the same key again: the old voice fades
            for v in self.voices.iter_mut().filter(|v| v.on && v.key == key && !v.kill) {
                v.kill = true;
            }
            let limit = self.voice_limit();
            let live = self.voices.iter().filter(|v| v.on && !v.kill).count();
            if live >= limit {
                // steal the oldest: released voices first
                let pick = (0..SLOTS).filter(|&i| self.voices[i].on && !self.voices[i].kill).min_by_key(|&i| (self.voices[i].held, self.voices[i].order));
                if let Some(i) = pick {
                    self.voices[i].kill = true;
                }
            }
            let slot = (0..SLOTS).find(|&i| !self.voices[i].on).unwrap_or_else(|| (0..SLOTS).min_by_key(|&i| (!self.voices[i].kill, self.voices[i].order)).unwrap_or(0));
            self.start_voice(slot, key, vel, glide_from);
        } else {
            // mono / legato: one voice, the held keys remembered
            if self.nstack < 16 && !self.stack[..self.nstack].contains(&key) {
                self.stack[self.nstack] = key;
                self.nstack += 1;
            }
            let playing = self.voices[0].on && !self.voices[0].kill && self.voices[0].env[0].stage != 0;
            let held = self.voices[0].held;
            if playing {
                let legato = mode == 2 && held;
                let glide = !porta_legato || held;
                self.retarget(0, key, vel, !legato, glide);
            } else {
                for v in self.voices.iter_mut().skip(1).filter(|v| v.on) {
                    v.kill = true;
                }
                let from = if self.voices[0].on { self.voices[0].glide_note(curve) } else { self.last_note };
                let glide_from = if from >= 0.0 && !porta_legato { Some(from) } else { None };
                if self.voices[0].on {
                    // move the dying voice aside so the new one starts clean
                    let i = (1..SLOTS).find(|&i| !self.voices[i].on).unwrap_or(1);
                    self.voices.swap(0, i);
                    self.voices[i].kill = true;
                }
                self.start_voice(0, key, vel, glide_from);
            }
        }
        self.last_note = key as f32;
    }
    fn note_off(&mut self, key: u8) {
        if self.mono() != 0 {
            if let Some(i) = self.stack[..self.nstack].iter().position(|&k| k == key) {
                self.stack.copy_within(i + 1..self.nstack, i);
                self.nstack -= 1;
            }
            if self.voices[0].on && self.voices[0].key == key {
                if self.nstack > 0 {
                    // back to the last key still held, legato
                    let k = self.stack[self.nstack - 1];
                    self.retarget(0, k, self.voices[0].vel, false, true);
                    self.last_note = k as f32;
                } else {
                    let v = &mut self.voices[0];
                    v.held = false;
                    for e in v.env.iter_mut() {
                        e.release();
                    }
                }
            }
            return;
        }
        for v in self.voices.iter_mut().filter(|v| v.on && v.key == key && v.held) {
            v.held = false;
            for e in v.env.iter_mut() {
                e.release();
            }
        }
    }
    fn note_pitch(&mut self, key: u8, semis: f32) {
        for v in self.voices.iter_mut().filter(|v| v.on && v.key == key) {
            let fresh = v.age == 0.0;
            v.pitch.set(semis, fresh);
        }
    }
    fn note_pressure(&mut self, key: u8, x: f32) {
        for v in self.voices.iter_mut().filter(|v| v.on && v.key == key) {
            let fresh = v.age == 0.0;
            v.press.set(x.clamp(0.0, 1.0), fresh);
        }
    }
    fn note_timbre(&mut self, key: u8, x: f32) {
        for v in self.voices.iter_mut().filter(|v| v.on && v.key == key) {
            let fresh = v.age == 0.0;
            v.timbre.set(x.clamp(0.0, 1.0), fresh);
        }
    }
    fn all_off(&mut self) {
        for v in self.voices.iter_mut() {
            v.on = false;
            v.held = false;
        }
        self.nstack = 0;
        self.rack.reset();
    }
    fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 20.0 {
            self.bpm = bpm as f32;
        }
    }
    fn idle(&mut self, frames: usize) {
        self.advance_free(frames);
    }
    fn render(&mut self, out: &mut [f32]) {
        if self.cells_dirty {
            self.refresh_cells();
        }
        let l = layout();
        let frames = out.len() / 2;
        let user = self.user.clone();
        let mut tix = [0usize; 3];
        for o in 0..3 {
            tix[o] = sel(self.v[l.osc[o].table], tables::NT + 1);
        }
        let pick = |i: usize| -> &WaveTable {
            if i >= tables::NT {
                match user.as_deref() {
                    Some(w) => w,
                    None => tables::table(0),
                }
            } else {
                tables::table(i)
            }
        };
        let tabs: [&WaveTable; 3] = [pick(tix[0]), pick(tix[1]), pick(tix[2])];
        let mut voices = std::mem::take(&mut self.voices);
        let mut mix = std::mem::take(&mut self.mix);
        for v in voices.iter_mut().filter(|v| v.on) {
            v.pitch.begin(frames);
            v.press.begin(frames);
            v.timbre.begin(frames);
        }
        let gain = self.v[l.level].powi(2) * 1.65;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(fx::CHUNK);
            let m = &mut mix[..n * 2];
            m.iter_mut().for_each(|x| *x = 0.0);
            for v in voices.iter_mut().filter(|v| v.on) {
                self.voice_render(v, m, &tabs);
            }
            // the global modulation (the rack): the newest voice's sources, else the macros and the wheel alone
            match voices.iter().filter(|v| v.on).max_by_key(|v| v.order) {
                Some(v) => self.gsrc = v.src,
                None => {
                    for k in 0..8 {
                        self.gsrc[S_M1 + k] = self.v[l.macros[k]];
                    }
                    self.gsrc[S_WHEEL] = self.v[l.wheel];
                    for k in 0..4 {
                        // the envelopes have died away; the LFOs hold where they were
                        self.gsrc[S_E1 + k] = 0.0;
                    }
                    self.gsrc[S_GATE] = 0.0;
                }
            }
            let gsrc = self.gsrc;
            let mut gm = std::mem::take(&mut self.gmods);
            self.apply_matrix(&gsrc, &mut gm);
            {
                let vals = &self.v;
                let ev = |p: usize| -> f32 {
                    let d = l.dst_of[p];
                    if d == usize::MAX || !l.dest_global[d] { vals[p] } else { (vals[p] + gm[d]).clamp(0.0, 1.0) }
                };
                self.rack.process(m, l, &ev, self.bpm);
            }
            self.gmods = gm;
            for i in 0..n {
                out[(done + i) * 2] += ceiling(m[i * 2] * gain);
                out[(done + i) * 2 + 1] += ceiling(m[i * 2 + 1] * gain);
            }
            done += n;
        }
        self.mix = mix;
        self.voices = voices;
        self.advance_free(frames);
    }
    fn params(&self) -> Vec<(&'static str, f32)> {
        layout().specs.iter().zip(self.v.iter()).map(|(s, v)| (s.name, *v)).collect()
    }
    fn get_param(&self, name: &str) -> Option<f32> {
        self.value(name)
    }
    fn set_param(&mut self, name: &str, v: f32) {
        if !v.is_finite() {
            return;
        }
        let l = layout();
        if let Some(&i) = l.index.get(name) {
            self.v[i] = v.clamp(0.0, 1.0);
            if name.starts_with('x') {
                self.cells_dirty = true;
            }
            if name.ends_with(" table") {
                self.prepare_tables();
            }
        }
    }
    fn name(&self) -> &'static str {
        "Ling Nova"
    }
    fn presets(&self) -> &'static [&'static str] {
        presets::names()
    }
    fn preset_categories(&self) -> Vec<(String, String)> {
        presets::all().iter().map(|p| (p.name.clone(), p.cat.clone())).collect()
    }
    fn load_preset(&mut self, name: &str) {
        if let Some(p) = presets::find(name) {
            let vals = p.vals.clone();
            self.set_values(&vals);
            self.preset = p.name.clone();
        }
    }
    fn preset_info(&self, name: &str) -> String {
        presets::find(name).map(|p| p.desc.clone()).unwrap_or_default()
    }
    fn state(&self) -> Vec<(String, f32)> {
        // only what differs from the defaults (a patch file stays short); load_state starts from the defaults
        let l = layout();
        l.specs.iter().zip(self.v.iter()).filter(|(s, v)| (**v - s.def).abs() > 1e-5).map(|(s, v)| (s.name.to_string(), *v)).collect()
    }
    fn load_state(&mut self, st: &[(String, f32)]) {
        self.v = Nova::defaults();
        for (n, v) in st {
            self.set_param(n, *v);
        }
        self.cells_dirty = true;
        self.prepare_tables();
    }
    fn command(&mut self, cmd: &str, args: &[f64], text: &str) -> Vec<f64> {
        let l = layout();
        // the panel passes numbers as text ("3 -1"): daw_inst_command has no number list
        let parsed: Vec<f64>;
        let args = if args.is_empty() && matches!(cmd, "fx_move" | "slot") {
            parsed = text.split_whitespace().filter_map(|w| w.parse().ok()).collect();
            &parsed[..]
        } else {
            args
        };
        match cmd {
            // "load" restores the project's table; "wt_load" also points an oscillator (args[0], default A) at it
            "load" | "wt_load" => match self.load_table(text) {
                Ok(n) => {
                    if cmd == "wt_load" {
                        let o = args.first().map(|x| (*x as usize).min(2)).unwrap_or(0);
                        self.v[l.osc[o].table] = 1.0;
                        self.v[l.osc[o].on] = 1.0;
                    }
                    vec![1.0, n as f64]
                }
                Err(e) => {
                    eprintln!("nova: {e}");
                    vec![0.0]
                }
            },
            "unload" | "wt_unload" => {
                self.user = None;
                for o in 0..3 {
                    if sel(self.v[l.osc[o].table], tables::NT + 1) == tables::NT {
                        self.v[l.osc[o].table] = 0.0;
                    }
                }
                vec![1.0]
            }
            // the spectral table maker: args = harmonic levels of the first frame then of the last (equal halves), `text` the
            // file to save; the frames between are morphed, the table loaded into the user slot of oscillator A
            "wt_spectral" => {
                let h = args.len() / 2;
                if h == 0 || text.is_empty() {
                    return vec![0.0];
                }
                let frames: Vec<Vec<f32>> = (0..16)
                    .map(|f| {
                        let t = f as f32 / 15.0;
                        let amps: Vec<f32> = (0..h).map(|k| (args[k] as f32) * (1.0 - t) + (args[h + k] as f32) * t).collect();
                        (0..super::wavetable::N)
                            .map(|j| {
                                let x = j as f32 / super::wavetable::N as f32;
                                amps.iter().enumerate().map(|(k, a)| a * (TAU * (k + 1) as f32 * x).sin()).sum::<f32>()
                            })
                            .collect()
                    })
                    .collect();
                if let Err(e) = super::wavetable::save(&frames, text) {
                    eprintln!("nova: {e}");
                    return vec![0.0];
                }
                self.command("wt_load", &[0.0], text)
            }
            // the factory preset a patch file is (patch::build): its name, for the usage notes
            "named" => match presets::find(text) {
                Some(p) => {
                    self.preset = p.name.clone();
                    vec![1.0]
                }
                None => vec![0.0],
            },
            // move an effect one place earlier (-1) or later (+1) in the rack: args = [effect, direction]
            "fx_move" => {
                if args.len() < 2 {
                    return vec![];
                }
                let k = (args[0] as usize).min(NFX - 1);
                let mut ord: Vec<usize> = (0..NFX).collect();
                ord.sort_by(|&a, &b| (self.v[l.fx[a].pos], a).partial_cmp(&(self.v[l.fx[b].pos], b)).unwrap_or(std::cmp::Ordering::Equal));
                let i = ord.iter().position(|&x| x == k).unwrap_or(0);
                let j = if args[1] < 0.0 { i.saturating_sub(1) } else { (i + 1).min(NFX - 1) };
                ord.swap(i, j);
                for (p, &x) in ord.iter().enumerate() {
                    self.v[l.fx[x].pos] = p as f32 / (NFX - 1) as f32;
                }
                vec![1.0]
            }
            // the rack's order as effect indices
            "fx_order" => {
                let mut ord: Vec<usize> = (0..NFX).collect();
                ord.sort_by(|&a, &b| (self.v[l.fx[a].pos], a).partial_cmp(&(self.v[l.fx[b].pos], b)).unwrap_or(std::cmp::Ordering::Equal));
                ord.into_iter().map(|x| x as f64).collect()
            }
            // a mod slot: args = [slot] -> [src index, dst index, amount -1..1, bipolar, aux index]
            "slot" => {
                let s = args.first().map(|x| (*x as usize).min(NSLOT - 1)).unwrap_or(0);
                let m = l.mat[s];
                vec![sel(self.v[m.src], NSRC) as f64, sel(self.v[m.dst], l.ndst()) as f64, ((self.v[m.amt] - 0.5) * 2.0) as f64, self.v[m.bi] as f64, sel(self.v[m.aux], NSRC) as f64]
            }
            // the first free mod slot (args: none) -> [slot] or [-1]
            "free_slot" => {
                let s = l.mat.iter().position(|m| sel(self.v[m.src], NSRC) == 0 || (self.v[m.amt] - 0.5).abs() < 0.002);
                vec![s.map(|x| x as f64).unwrap_or(-1.0)]
            }
            _ => vec![],
        }
    }
    fn info(&self, what: &str) -> String {
        let l = layout();
        match what {
            "config" | "file" | "wavetable" => self.user.as_ref().map(|w| w.path.clone()).unwrap_or_default(),
            "frames" => self.user.as_ref().map(|w| w.nframes().to_string()).unwrap_or_default(),
            "preset" => presets::find(&self.preset).map(|p| p.desc.clone()).unwrap_or_default(),
            "usage" => presets::find(&self.preset).map(|p| p.usage.text()).unwrap_or_default(),
            "tables" => tables::names_with_user().join("\n"),
            "sources" => SOURCES.join("\n"),
            "dests" => l.dests.join("\n"),
            "warps" => WARPS.join("\n"),
            "ftypes" => FTYPES.join("\n"),
            "macros" => {
                let p = presets::find(&self.preset);
                (0..8).map(|k| p.map(|p| p.usage.macro_label(k)).unwrap_or_else(|| format!("macro {}", k + 1))).collect::<Vec<_>>().join("\n")
            }
            _ => {
                if let Some(rest) = what.strip_prefix("usage ") {
                    return presets::find(rest).map(|p| p.usage.text()).unwrap_or_default();
                }
                String::new()
            }
        }
    }
    fn randomize(&mut self, seed: u32) -> bool {
        let mut r = Rng::new(seed.wrapping_mul(2654435761).wrapping_add(17));
        let all = presets::all();
        let p = &all[(((r.next() + 1.0) * 0.5 * all.len() as f32) as usize).min(all.len() - 1)];
        self.load_preset(&p.name.clone());
        let l = layout();
        for o in 0..3 {
            for p in [l.osc[o].pos, l.osc[o].det, l.osc[o].wa[0], l.osc[o].wa[1]] {
                self.v[p] = (self.v[p] + r.next() * 0.15).clamp(0.0, 1.0);
            }
        }
        for p in [l.filt[0].cut, l.filt[0].res, l.filt[1].cut] {
            self.v[p] = (self.v[p] + r.next() * 0.1).clamp(0.0, 1.0);
        }
        true
    }
    /// "wave a|b|c": one cycle at the position (warps applied); "stack a|b|c": 8 frames x n/8 points of the table, front to back
    /// (the pseudo-3D view); "pos a|b|c": the position as [pos]; "env1".."env4": the envelope (a held note, then the release);
    /// "lfo1".."lfo4": one cycle; "filter1|2": the response (0..1 = -24..+24 dB) from 20 Hz to 20 kHz
    fn plot(&self, what: &str, n: usize) -> Vec<f32> {
        let l = layout();
        let n = n.clamp(2, 4096);
        let osc_of = |s: &str| match s.chars().last() {
            Some('b') => 1,
            Some('c') => 2,
            _ => 0,
        };
        let tab = |o: usize| -> &WaveTable {
            let i = sel(self.v[l.osc[o].table], tables::NT + 1);
            if i >= tables::NT {
                match self.user.as_deref() {
                    Some(w) => w,
                    None => tables::table(0),
                }
            } else {
                tables::table(i)
            }
        };
        if what.starts_with("wave") {
            let o = osc_of(what);
            let x = l.osc[o];
            let t = tab(o);
            let pos = self.v[x.pos];
            return (0..n)
                .map(|k| {
                    let mut p = k as f32 / n as f32;
                    for w in 0..2 {
                        let a = self.v[x.wa[w]];
                        match sel(self.v[x.wt[w]], WARPS.len()) {
                            W_SYNC => p = (p * (1.0 + a * 7.0)).fract(),
                            W_BENDP => {
                                let y = 2.0 * p - 1.0;
                                p = (y.signum() * y.abs().powf(1.0 + a * 3.0) + 1.0) * 0.5;
                            }
                            W_BENDM => {
                                let y = 2.0 * p - 1.0;
                                p = (y.signum() * y.abs().powf(1.0 / (1.0 + a * 3.0)) + 1.0) * 0.5;
                            }
                            W_PWM => {
                                let wd = 1.0 - a * 0.95;
                                p = if p < wd { p / wd } else { -1.0 };
                            }
                            W_ASYM => {
                                let m = 0.5 - a * 0.45;
                                p = if p < m { p * 0.5 / m } else { 0.5 + (p - m) * 0.5 / (1.0 - m) };
                            }
                            W_MIRROR => {
                                let mi = 1.0 - (2.0 * p - 1.0).abs();
                                p += (mi * 0.5 - p) * a;
                            }
                            W_QUANT => {
                                let s = 2f32.powf(2.0 + (1.0 - a) * 9.0);
                                p = (p * s).floor() / s;
                            }
                            _ => {}
                        }
                        if p < 0.0 {
                            break;
                        }
                    }
                    if p < 0.0 {
                        return 0.5;
                    }
                    let mut y = t.read(pos, 0, p.rem_euclid(1.0));
                    for w in 0..2 {
                        let a = self.v[x.wa[w]];
                        match sel(self.v[x.wt[w]], WARPS.len()) {
                            W_FOLD => y = tables::fold(y * (1.0 + a * 6.0)),
                            W_CRUSH => {
                                let q = 2f32.powf(1.0 + (1.0 - a) * 11.0);
                                y = (y * q).round() / q;
                            }
                            _ => {}
                        }
                    }
                    (0.5 + 0.5 * y).clamp(0.0, 1.0)
                })
                .collect();
        }
        if what.starts_with("stack") {
            let o = osc_of(what);
            let t = tab(o);
            let per = (n / 8).max(2);
            let mut out = Vec::with_capacity(per * 8);
            for f in 0..8 {
                let pos = f as f32 / 7.0;
                for k in 0..per {
                    out.push((0.5 + 0.5 * t.read(pos, 2, k as f32 / per as f32)).clamp(0.0, 1.0));
                }
            }
            return out;
        }
        if what.starts_with("pos") {
            return vec![self.v[l.osc[osc_of(what)].pos]];
        }
        if let Some(k) = what.strip_prefix("env") {
            let k = k.parse::<usize>().unwrap_or(1).clamp(1, 4) - 1;
            let e = l.env[k];
            let p = EnvP {
                a: env_time(self.v[e.a]),
                h: self.v[e.h].powi(2) * 2.0,
                d: env_time(self.v[e.d]),
                s: self.v[e.s],
                r: env_time(self.v[e.r]),
                pa: curve_exp(self.v[e.ac]),
                pd: curve_exp(self.v[e.dc]),
                pr: curve_exp(self.v[e.rc]),
            };
            // the drawing's time: the stages plus a held sustain, release at 75 %
            let hold = (p.a + p.h + p.d) * 1.3 + 0.05;
            let total = hold / 0.75;
            let dt = total / n as f32;
            let mut en = Env::default();
            en.gate();
            return (0..n)
                .map(|i| {
                    if i as f32 * dt >= hold && en.stage != 5 && en.stage != 0 {
                        // the release is drawn over the last quarter at its own speed, squeezed to fit
                        en.release();
                    }
                    let dd = if en.stage == 5 { dt * p.r / (total - hold).max(1e-4) } else { dt };
                    en.tick(&p, dd).clamp(0.0, 1.0)
                })
                .collect();
        }
        if let Some(k) = what.strip_prefix("lfo") {
            let k = k.parse::<usize>().unwrap_or(1).clamp(1, 4) - 1;
            let x = l.lfo[k];
            let mut pts = [0.0f32; 8];
            for (j, p) in pts.iter_mut().enumerate() {
                *p = self.v[x.pts[j]];
            }
            let shape = sel(self.v[x.shape], LFO_SHAPES.len());
            return (0..n).map(|i| lfo_shape(shape, (i as f32 / n as f32 + self.v[x.phase]).fract(), &pts, self.v[x.smooth], (0.3, 0.8)).clamp(0.0, 1.0)).collect();
        }
        if let Some(k) = what.strip_prefix("filter") {
            let k = k.parse::<usize>().unwrap_or(1).clamp(1, 2) - 1;
            return self.filter_response(k, n);
        }
        vec![]
    }
}

impl Nova {
    fn advance_free(&mut self, frames: usize) {
        let l = layout();
        let dt = frames as f32 / self.sr;
        for k in 0..4 {
            let x = l.lfo[k];
            let hz = self.sync_hz(self.v[x.sync] > 0.5, self.v[x.rate], self.v[x.div], 0.0);
            self.gph[k] = (self.gph[k] + hz * dt).fract();
        }
    }

    /// a filter's magnitude response, measured by running a sine sweep through a fresh copy (so it is exactly the filter)
    fn filter_response(&self, k: usize, n: usize) -> Vec<f32> {
        let l = layout();
        let sr = 48000.0;
        let x = l.filt[k];
        let kind = sel(self.v[x.kind], FTYPES.len());
        let mut v = Voice::new(sr, l.ndst(), 1);
        let probe = Nova::new_light(&self.v);
        v.key = 60;
        v.note = 60.0;
        probe.control_filters_only(&mut v);
        let pts = n.min(40);
        let resp: Vec<f32> = (0..pts)
            .map(|i| {
                let f = 20.0 * 1000f32.powf(i as f32 / (pts - 1) as f32);
                let mut st = FiltState::new(sr);
                let fr = v.fr[k];
                let len = (((sr / f) * 4.0) as usize).clamp(600, 2400);
                let (mut e_in, mut e_out) = (0.0f32, 0.0f32);
                for j in 0..len {
                    let s = (TAU * f * j as f32 / sr).sin() * 0.1;
                    let y = st.run(&fr, [s, s])[0];
                    if j > len / 2 {
                        e_in += s * s;
                        e_out += y * y;
                    }
                }
                let _ = kind;
                let db = 10.0 * (e_out.max(1e-12) / e_in.max(1e-12)).log10();
                ((db + 24.0) / 48.0).clamp(0.0, 1.0)
            })
            .collect();
        // resample to n points
        (0..n).map(|i| resp[(i * (pts - 1)) / (n - 1).max(1)]).collect()
    }

    /// a Nova that only carries parameter values (the response plot)
    fn new_light(vals: &[f32]) -> Nova {
        let l = layout();
        Nova {
            sr: 48000.0,
            v: vals.to_vec(),
            voices: vec![],
            order: 0,
            last_note: -1.0,
            stack: [0; 16],
            nstack: 0,
            bpm: 120.0,
            gph: [0.0; 4],
            gmods: vec![0.0; l.ndst()],
            gsrc: [0.0; NSRC],
            cells: [(0, 0, 0.0, false, 1.0, 0); NSLOT],
            ncells: 0,
            cells_dirty: true,
            rack: fx::Rack::new(8000.0),
            mix: vec![],
            user: None,
            preset: String::new(),
            rng: Rng::new(1),
        }
    }

    fn control_filters_only(&self, v: &mut Voice) {
        let tabs = [tables::table(0); 3];
        v.cut_s = [-1.0; 2];
        self.control(v, &tabs);
    }
}
