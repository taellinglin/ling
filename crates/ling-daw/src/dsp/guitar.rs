//! Ringstorm: a modular pedalboard (pedals, amps, cabs, racks) wired as a small acyclic graph, as one effect.
//!
//! Graph: up to 16 nodes processed in index order. Each node reads either the rig input (`input == -1`) or the
//! output of an earlier node. The rig output is the sum of every node with `out == true` (times its `level`);
//! when none is flagged the last node is used. All pedal DSP is self-contained (no file IO, no allocation in
//! `process`).
use super::fx::Effect;
use serde_json::{json, Map, Value};
use std::f32::consts::PI;

const MAX_NODES: usize = 16;
const CHUNK: usize = 128;

// ---------------------------------------------------------------------------------------------------------
// Public tables
// ---------------------------------------------------------------------------------------------------------

pub const PARAMS: [(&str, f32); 3] = [("input", 0.5), ("level", 0.5), ("mix", 1.0)];

pub const PRESETS: [(&str, &[(&str, f32)]); 20] = [
    ("Clean Sparkle", &[]),
    ("Blues Crunch", &[]),
    ("Classic Rock Stack", &[]),
    ("High Gain Metal", &[]),
    ("Fuzz Face 70s", &[]),
    ("Shoegaze Wall", &[]),
    ("Ambient Swell", &[]),
    ("Funk Wah", &[]),
    ("Surf Tremolo", &[]),
    ("Jazz Hollow", &[]),
    ("Acoustic Shimmer", &[]),
    ("Country Slap", &[]),
    ("Bass Drive", &[]),
    ("Dual Rig Parallel", &[]),
    ("Space Echo", &[]),
    ("Lo-Fi Tape", &[]),
    ("Metal Rhythm Tight", &[]),
    ("Psychedelic Phaser", &[]),
    ("Edge Delay Rig", &[]),
    ("Worship Ambient", &[]),
];

type KindDef = (&'static str, &'static [(&'static str, f32)]);

/// Pedal kinds with their named params (all 0..1) and defaults.
pub static KINDS: [KindDef; 20] = [
    ("Noise Gate", &[("threshold", 0.5), ("attack", 0.2), ("hold", 0.3), ("release", 0.4), ("range", 0.8)]),
    ("Compressor", &[("sustain", 0.5), ("attack", 0.3), ("release", 0.4), ("level", 0.5), ("blend", 1.0), ("tone", 0.5)]),
    ("Boost", &[("gain", 0.4), ("treble", 0.5), ("bass_cut", 0.2), ("level", 0.5), ("grit", 0.1)]),
    ("Overdrive", &[("drive", 0.4), ("tone", 0.5), ("level", 0.5), ("mid", 0.5), ("asym", 0.3), ("tight", 0.3)]),
    ("Distortion", &[("distortion", 0.5), ("filter", 0.5), ("tone", 0.5), ("level", 0.5), ("tight", 0.3), ("clip", 0.5)]),
    ("Fuzz", &[("fuzz", 0.6), ("tone", 0.5), ("level", 0.5), ("bias", 0.5), ("gate", 0.0), ("mode", 0.0)]),
    (
        "Amp",
        &[
            ("gain", 0.4),
            ("bass", 0.5),
            ("mid", 0.5),
            ("treble", 0.5),
            ("presence", 0.4),
            ("master", 0.5),
            ("channel", 0.0),
            ("sag", 0.3),
            ("bright", 0.3),
        ],
    ),
    ("Cab", &[("type", 0.5), ("mic", 0.3), ("low_cut", 0.1), ("high_cut", 0.7), ("level", 0.5), ("room", 0.1)]),
    ("EQ", &[("g100", 0.5), ("g400", 0.5), ("g800", 0.5), ("g1600", 0.5), ("g3200", 0.5), ("level", 0.5)]),
    ("Wah", &[("pedal", 0.5), ("auto", 0.0), ("sens", 0.5), ("decay", 0.4), ("q", 0.5), ("mix", 1.0), ("level", 0.5)]),
    ("Chorus", &[("rate", 0.3), ("depth", 0.5), ("delay", 0.4), ("mix", 0.8), ("tone", 0.6), ("spread", 0.5)]),
    ("Flanger", &[("rate", 0.25), ("depth", 0.6), ("delay", 0.3), ("feedback", 0.7), ("mix", 0.8), ("spread", 0.4)]),
    (
        "Phaser",
        &[("rate", 0.25), ("depth", 0.7), ("center", 0.5), ("feedback", 0.4), ("stages", 0.5), ("mix", 0.8), ("spread", 0.3)],
    ),
    ("Tremolo", &[("rate", 0.4), ("depth", 0.6), ("shape", 0.0), ("stereo", 0.0)]),
    ("Vibrato", &[("rate", 0.4), ("depth", 0.4), ("stereo", 0.3), ("tone", 0.7), ("mix", 1.0)]),
    (
        "Delay",
        &[("time", 0.3), ("feedback", 0.35), ("tone", 0.5), ("mix", 0.35), ("mode", 0.0), ("wow", 0.2), ("pingpong", 0.0)],
    ),
    (
        "Reverb",
        &[
            ("type", 0.5),
            ("size", 0.5),
            ("decay", 0.5),
            ("damping", 0.5),
            ("predelay", 0.1),
            ("mix", 0.3),
            ("tone", 0.2),
            ("mod", 0.3),
        ],
    ),
    ("Octaver", &[("sub", 0.6), ("up", 0.3), ("dry", 0.6), ("tone", 0.5), ("range", 0.5), ("level", 0.5)]),
    ("Volume", &[("volume", 1.0), ("boost", 0.0), ("pan", 0.5), ("swell", 0.0), ("sens", 0.5)]),
    ("Slicer", &[("rate", 0.4), ("depth", 0.9), ("smooth", 0.3), ("pattern", 0.0), ("stereo", 0.0)]),
];

pub fn kind_index(name: &str) -> Option<usize> {
    KINDS.iter().position(|k| k.0.eq_ignore_ascii_case(name.trim()))
}

// ---------------------------------------------------------------------------------------------------------
// Small DSP helpers
// ---------------------------------------------------------------------------------------------------------

#[inline]
fn db(x: f32) -> f32 {
    10f32.powf(x * 0.05)
}
#[inline]
fn xp(v: f32, lo: f32, hi: f32) -> f32 {
    lo * (hi / lo).powf(v.clamp(0.0, 1.0))
}
#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
#[inline]
fn fl(x: f32) -> f32 {
    if x.abs() < 1e-18 {
        0.0
    } else {
        x
    }
}
/// Smooth saturator, exactly +-1 beyond |x| = 3, unity slope at 0.
#[inline]
fn th(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}
/// Transparent below 1.5, soft-limits to 2.5.
#[inline]
fn lim(x: f32) -> f32 {
    let a = x.abs();
    if a <= 1.5 {
        x
    } else {
        (1.5 + th(a - 1.5)).copysign(x)
    }
}
#[inline]
fn onepole(sr: f32, f: f32) -> f32 {
    1.0 - (-2.0 * PI * f.clamp(1.0, 0.45 * sr) / sr).exp()
}
#[inline]
fn ms_coef(sr: f32, ms: f32) -> f32 {
    1.0 - (-1.0 / (ms * 0.001 * sr).max(1.0)).exp()
}
#[inline]
fn wrap1(p: f32) -> f32 {
    if p >= 1.0 {
        p - 1.0
    } else {
        p
    }
}

#[derive(Clone, Copy)]
struct Bq {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}
impl Default for Bq {
    fn default() -> Self {
        Bq { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: 0.0, z2: 0.0 }
    }
}
impl Bq {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = fl(self.b1 * x - self.a1 * y + self.z2);
        self.z2 = fl(self.b2 * x - self.a2 * y);
        y
    }
    fn set(&mut self, b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) {
        let i = 1.0 / a0;
        self.b0 = b0 * i;
        self.b1 = b1 * i;
        self.b2 = b2 * i;
        self.a1 = a1 * i;
        self.a2 = a2 * i;
    }
    fn trig(sr: f32, f: f32) -> (f32, f32) {
        let w = 2.0 * PI * f.clamp(10.0, 0.45 * sr) / sr;
        (w.cos(), w.sin())
    }
    fn lp(&mut self, sr: f32, f: f32, q: f32) {
        let (c, s) = Self::trig(sr, f);
        let al = s / (2.0 * q);
        self.set((1.0 - c) * 0.5, 1.0 - c, (1.0 - c) * 0.5, 1.0 + al, -2.0 * c, 1.0 - al);
    }
    fn hp(&mut self, sr: f32, f: f32, q: f32) {
        let (c, s) = Self::trig(sr, f);
        let al = s / (2.0 * q);
        self.set((1.0 + c) * 0.5, -(1.0 + c), (1.0 + c) * 0.5, 1.0 + al, -2.0 * c, 1.0 - al);
    }
    fn peak(&mut self, sr: f32, f: f32, q: f32, gdb: f32) {
        let (c, s) = Self::trig(sr, f);
        let a = db(gdb * 0.5);
        let al = s / (2.0 * q);
        self.set(1.0 + al * a, -2.0 * c, 1.0 - al * a, 1.0 + al / a, -2.0 * c, 1.0 - al / a);
    }
    fn lshelf(&mut self, sr: f32, f: f32, gdb: f32) {
        let (c, s) = Self::trig(sr, f);
        let a = db(gdb * 0.5);
        let t = 2.0 * a.sqrt() * (s * 0.5 * std::f32::consts::SQRT_2);
        self.set(
            a * ((a + 1.0) - (a - 1.0) * c + t),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - t),
            (a + 1.0) + (a - 1.0) * c + t,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - t,
        );
    }
    fn hshelf(&mut self, sr: f32, f: f32, gdb: f32) {
        let (c, s) = Self::trig(sr, f);
        let a = db(gdb * 0.5);
        let t = 2.0 * a.sqrt() * (s * 0.5 * std::f32::consts::SQRT_2);
        self.set(
            a * ((a + 1.0) + (a - 1.0) * c + t),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - t),
            (a + 1.0) - (a - 1.0) * c + t,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - t,
        );
    }
}

#[derive(Clone, Copy, Default)]
struct Lp1 {
    z: f32,
    a: f32,
}
impl Lp1 {
    fn set(&mut self, sr: f32, f: f32) {
        self.a = onepole(sr, f);
    }
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        self.z = fl(self.z + self.a * (x - self.z));
        self.z
    }
}

/// 2x oversampling wrapper around a memoryless nonlinearity (linear-interp upsample, 4th-order lowpass down).
#[derive(Clone, Default)]
struct Os2 {
    prev: f32,
    a: Bq,
    b: Bq,
}
impl Os2 {
    fn new(sr: f32) -> Self {
        let mut o = Os2::default();
        o.a.lp(2.0 * sr, 0.42 * sr, 0.5412);
        o.b.lp(2.0 * sr, 0.42 * sr, 1.3066);
        o
    }
    #[inline]
    fn run<F: Fn(f32) -> f32>(&mut self, x: f32, f: F) -> f32 {
        let m = f(0.5 * (self.prev + x));
        let n = f(x);
        self.prev = x;
        let _ = self.b.tick(self.a.tick(m));
        self.b.tick(self.a.tick(n))
    }
}

/// TPT state-variable filter.
#[derive(Clone, Copy, Default)]
struct Svf {
    ic1: f32,
    ic2: f32,
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}
impl Svf {
    fn set(&mut self, sr: f32, f: f32, q: f32) {
        let g = (PI * f.clamp(20.0, 0.45 * sr) / sr).tan();
        self.k = 1.0 / q.max(0.3);
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }
    /// Returns the band-pass output (peak gain Q).
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = fl(2.0 * v1 - self.ic1);
        self.ic2 = fl(2.0 * v2 - self.ic2);
        v1
    }
}

#[derive(Clone)]
struct DLine {
    buf: Vec<f32>,
    w: usize,
}
impl DLine {
    fn new(n: usize) -> Self {
        DLine { buf: vec![0.0; n.max(8)], w: 0 }
    }
    #[inline]
    fn push(&mut self, x: f32) {
        self.w += 1;
        if self.w >= self.buf.len() {
            self.w = 0;
        }
        self.buf[self.w] = x;
    }
    /// Sample `d` samples back (0 = newest), linearly interpolated.
    #[inline]
    fn read(&self, d: f32) -> f32 {
        let n = self.buf.len();
        let d = d.clamp(1.0, (n - 2) as f32);
        let i = d as usize;
        let fr = d - i as f32;
        let p = self.w + n - i;
        let a = self.buf[p % n];
        let b = self.buf[(p - 1) % n];
        a + (b - a) * fr
    }
}

#[derive(Clone)]
struct Ap {
    line: DLine,
    d: f32,
    g: f32,
}
impl Ap {
    fn new(cap: usize) -> Self {
        Ap { line: DLine::new(cap), d: 8.0, g: 0.7 }
    }
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let z = self.line.read(self.d);
        let w = fl(x + self.g * z);
        self.line.push(w);
        z - self.g * w
    }
}

#[inline]
fn follow(env: &mut f32, a: f32, att: f32, rel: f32) {
    let c = if a > *env { att } else { rel };
    *env = fl(*env + c * (a - *env));
}

// ---------------------------------------------------------------------------------------------------------
// Pedal traits
// ---------------------------------------------------------------------------------------------------------

trait Ped: Send {
    fn update(&mut self, p: &[f32], sr: f32);
    fn tick(&mut self, l: f32, r: f32) -> (f32, f32);
    fn gr(&self) -> f32 {
        0.0
    }
}

/// Mono processor run independently on each channel.
trait Ch: Send {
    fn upd(&mut self, p: &[f32], sr: f32);
    fn run(&mut self, x: f32) -> f32;
    fn gr(&self) -> f32 {
        0.0
    }
}

struct Dual<T: Ch>(T, T);
impl<T: Ch> Dual<T> {
    fn new(f: impl Fn(usize) -> T) -> Self {
        Dual(f(0), f(1))
    }
}
impl<T: Ch> Ped for Dual<T> {
    fn update(&mut self, p: &[f32], sr: f32) {
        self.0.upd(p, sr);
        self.1.upd(p, sr);
    }
    #[inline]
    fn tick(&mut self, l: f32, r: f32) -> (f32, f32) {
        (self.0.run(l), self.1.run(r))
    }
    fn gr(&self) -> f32 {
        self.0.gr().max(self.1.gr())
    }
}

// ---------------------------------------------------------------------------------------------------------
// Pedals
// ---------------------------------------------------------------------------------------------------------

// ---- Noise gate ----
struct Gate {
    thr: f32,
    hold_n: f32,
    ga: f32,
    gr_c: f32,
    floor: f32,
    ea: f32,
    er: f32,
    env: f32,
    gain: f32,
    hold: f32,
    open: bool,
}
impl Gate {
    fn new() -> Self {
        Gate {
            thr: 0.003,
            hold_n: 1000.0,
            ga: 0.1,
            gr_c: 0.001,
            floor: 0.01,
            ea: 0.1,
            er: 0.01,
            env: 0.0,
            gain: 1.0,
            hold: 0.0,
            open: false,
        }
    }
}
impl Ch for Gate {
    fn upd(&mut self, p: &[f32], sr: f32) {
        self.thr = db(-80.0 + p[0] * 60.0);
        self.ga = ms_coef(sr, xp(p[1], 0.3, 15.0));
        self.hold_n = xp(p[2], 5.0, 300.0) * 0.001 * sr;
        self.gr_c = ms_coef(sr, xp(p[3], 20.0, 800.0));
        self.floor = db(-p[4] * 80.0);
        self.ea = ms_coef(sr, 0.3);
        self.er = ms_coef(sr, 8.0);
    }
    fn run(&mut self, x: f32) -> f32 {
        follow(&mut self.env, x.abs(), self.ea, self.er);
        let t = if self.open { self.thr * 0.7 } else { self.thr };
        if self.env > t {
            self.open = true;
            self.hold = self.hold_n;
        } else if self.hold > 0.0 {
            self.hold -= 1.0;
        } else {
            self.open = false;
        }
        let tg = if self.open { 1.0 } else { self.floor };
        let c = if tg > self.gain { self.ga } else { self.gr_c };
        self.gain += c * (tg - self.gain);
        x * self.gain
    }
    fn gr(&self) -> f32 {
        (1.0 - self.gain).clamp(0.0, 1.0)
    }
}

// ---- Compressor ----
struct Comp {
    thr: f32,
    ratio: f32,
    makeup: f32,
    blend: f32,
    att: f32,
    rel: f32,
    env: f32,
    tone: Bq,
    grdb: f32,
}
impl Comp {
    fn new() -> Self {
        Comp { thr: -25.0, ratio: 6.0, makeup: 1.0, blend: 1.0, att: 0.05, rel: 0.001, env: 0.0, tone: Bq::default(), grdb: 0.0 }
    }
}
impl Ch for Comp {
    fn upd(&mut self, p: &[f32], sr: f32) {
        self.thr = -8.0 - p[0] * 34.0;
        self.ratio = 3.0 + p[0] * 9.0;
        self.att = ms_coef(sr, xp(p[1], 0.5, 40.0));
        self.rel = ms_coef(sr, xp(p[2], 30.0, 1000.0));
        self.makeup = db(-self.thr * (1.0 - 1.0 / self.ratio) * 0.45 + (p[3] - 0.5) * 24.0);
        self.blend = p[4];
        self.tone.hshelf(sr, 3000.0, (p[5] - 0.5) * 12.0);
    }
    fn run(&mut self, x: f32) -> f32 {
        follow(&mut self.env, x.abs(), self.att, self.rel);
        let lv = 20.0 * (self.env + 1e-9).log10();
        let over = lv - self.thr;
        let oc = if over < -3.0 {
            0.0
        } else if over > 3.0 {
            over
        } else {
            (over + 3.0) * (over + 3.0) / 12.0
        };
        self.grdb = oc * (1.0 - 1.0 / self.ratio);
        let y = x * db(-self.grdb) * self.makeup;
        self.tone.tick(lerp(x, y, self.blend))
    }
    fn gr(&self) -> f32 {
        (self.grdb / 20.0).clamp(0.0, 1.0)
    }
}

// ---- Boost ----
struct Boost {
    hp: Bq,
    sh: Bq,
    g: f32,
    grit: f32,
    out: f32,
}
impl Boost {
    fn new() -> Self {
        Boost { hp: Bq::default(), sh: Bq::default(), g: 1.0, grit: 0.0, out: 1.0 }
    }
}
impl Ch for Boost {
    fn upd(&mut self, p: &[f32], sr: f32) {
        self.hp.hp(sr, xp(p[2], 20.0, 400.0), 0.7);
        self.sh.hshelf(sr, 2200.0, p[1] * 12.0);
        self.g = db(p[0] * 26.0);
        self.grit = p[4];
        self.out = db((p[3] - 0.5) * 18.0);
    }
    fn run(&mut self, x: f32) -> f32 {
        let v = self.sh.tick(self.hp.tick(x) * self.g);
        lerp(v, th(v), self.grit) * self.out
    }
}

// ---- Overdrive ----
struct Od {
    hp: Bq,
    mid: Bq,
    tone: Bq,
    post: Bq,
    dc: Bq,
    os: Os2,
    g: f32,
    k: f32,
    out: f32,
}
impl Od {
    fn new(sr: f32) -> Self {
        Od {
            hp: Bq::default(),
            mid: Bq::default(),
            tone: Bq::default(),
            post: Bq::default(),
            dc: Bq::default(),
            os: Os2::new(sr),
            g: 1.0,
            k: 1.0,
            out: 0.5,
        }
    }
}
impl Ch for Od {
    fn upd(&mut self, p: &[f32], sr: f32) {
        self.hp.hp(sr, xp(p[5], 40.0, 500.0), 0.7);
        self.mid.peak(sr, 720.0, 0.8, p[3] * 14.0);
        self.g = db(6.0 + p[0] * 34.0);
        self.k = 1.0 + p[4] * 1.5;
        self.tone.lp(sr, xp(p[1], 700.0, 7000.0), 0.707);
        self.post.lp(sr, 9000.0f32.min(0.4 * sr), 0.707);
        self.dc.hp(sr, 20.0, 0.707);
        self.out = db((p[2] - 0.5) * 20.0) * 0.5;
    }
    fn run(&mut self, x: f32) -> f32 {
        let v = self.mid.tick(self.hp.tick(x)) * self.g;
        let k = self.k;
        let y = self.os.run(v, move |u| if u >= 0.0 { th(u) } else { th(u * k) / k });
        let y = self.dc.tick(self.post.tick(self.tone.tick(y)));
        y * self.out
    }
}

// ---- Distortion ----
struct Dist {
    hp: Bq,
    pre: Bq,
    post: Bq,
    tone: Bq,
    os: Os2,
    g: f32,
    clip: f32,
    out: f32,
}
impl Dist {
    fn new(sr: f32) -> Self {
        Dist {
            hp: Bq::default(),
            pre: Bq::default(),
            post: Bq::default(),
            tone: Bq::default(),
            os: Os2::new(sr),
            g: 1.0,
            clip: 0.5,
            out: 0.5,
        }
    }
}
impl Ch for Dist {
    fn upd(&mut self, p: &[f32], sr: f32) {
        self.hp.hp(sr, xp(p[4], 30.0, 400.0), 0.7);
        self.pre.lp(sr, xp(1.0 - p[0], 3000.0, 10000.0), 0.7);
        self.g = db(12.0 + p[0] * 38.0);
        self.clip = p[5];
        self.post.lp(sr, xp(1.0 - p[1], 500.0, 8000.0), 0.707);
        self.tone.hshelf(sr, 2000.0, (p[2] - 0.5) * 16.0);
        self.out = db((p[3] - 0.5) * 20.0) * 0.5;
    }
    fn run(&mut self, x: f32) -> f32 {
        let v = self.pre.tick(self.hp.tick(x)) * self.g;
        let c = self.clip;
        let y = self.os.run(v, move |u| lerp(th(u), u.clamp(-1.0, 1.0), c));
        self.tone.tick(self.post.tick(y)) * self.out
    }
}

// ---- Fuzz ----
struct Fuzz {
    hp: Bq,
    lp: Bq,
    hp2: Bq,
    dc: Bq,
    os: Os2,
    os2: Os2,
    g1: f32,
    asym: f32,
    dz: f32,
    tone: f32,
    muff: bool,
    out: f32,
}
impl Fuzz {
    fn new(sr: f32) -> Self {
        Fuzz {
            hp: Bq::default(),
            lp: Bq::default(),
            hp2: Bq::default(),
            dc: Bq::default(),
            os: Os2::new(sr),
            os2: Os2::new(sr),
            g1: 1.0,
            asym: 1.0,
            dz: 0.0,
            tone: 0.5,
            muff: false,
            out: 0.4,
        }
    }
}
impl Ch for Fuzz {
    fn upd(&mut self, p: &[f32], sr: f32) {
        self.muff = p[5] >= 0.5;
        self.hp.hp(sr, if self.muff { 40.0 } else { 60.0 }, 0.7);
        self.g1 = if self.muff { db(10.0 + p[0] * 30.0) } else { db(8.0 + p[0] * 38.0) };
        self.asym = 0.6 + p[3] * 0.8;
        self.dz = p[4] * p[4] * 0.25;
        self.tone = p[1];
        if self.muff {
            self.lp.lp(sr, 1500.0, 0.707);
            self.hp2.hp(sr, 1500.0, 0.707);
        } else {
            self.lp.lp(sr, xp(p[1], 1000.0, 8000.0), 0.707);
        }
        self.dc.hp(sr, 20.0, 0.707);
        self.out = db((p[2] - 0.5) * 20.0) * 0.45;
    }
    fn run(&mut self, x: f32) -> f32 {
        let mut v = self.hp.tick(x);
        let a = v.abs();
        v = if a < self.dz { 0.0 } else { (a - self.dz).copysign(v) };
        let y;
        if !self.muff {
            let k = self.asym;
            let c = self.os.run(v * self.g1, move |u| if u >= 0.0 { th(u) } else { th(u * k) / k });
            y = self.lp.tick(c);
        } else {
            let s1 = self.os.run(v * self.g1, th);
            let s2 = self.os2.run(s1 * 8.0, th);
            let l = self.lp.tick(s2);
            let h = self.hp2.tick(s2);
            y = lerp(l, h, self.tone) * 0.8;
        }
        self.dc.tick(y) * self.out
    }
}

// ---- Amp ----
struct Amp {
    hp: Bq,
    bright: Bq,
    c_hp: Bq,
    bass: Bq,
    mid: Bq,
    treb: Bq,
    pres: Bq,
    lp: Bq,
    os1: Os2,
    os2: Os2,
    g1: f32,
    g2: f32,
    sag: f32,
    master: f32,
    env: f32,
    ea: f32,
    er: f32,
}
impl Amp {
    fn new(sr: f32) -> Self {
        Amp {
            hp: Bq::default(),
            bright: Bq::default(),
            c_hp: Bq::default(),
            bass: Bq::default(),
            mid: Bq::default(),
            treb: Bq::default(),
            pres: Bq::default(),
            lp: Bq::default(),
            os1: Os2::new(sr),
            os2: Os2::new(sr),
            g1: 2.0,
            g2: 1.0,
            sag: 0.3,
            master: 1.0,
            env: 0.0,
            ea: 0.01,
            er: 0.001,
        }
    }
}
impl Ch for Amp {
    fn upd(&mut self, p: &[f32], sr: f32) {
        let gain = p[0];
        let chan = if p[6] < 0.34 {
            0
        } else if p[6] < 0.67 {
            1
        } else {
            2
        };
        let (g1, g2, coupling, hpf, norm) = match chan {
            0 => (0.8 + gain * 5.0, 1.0, 40.0, 60.0, 1.0),
            1 => (2.0 + gain * 16.0, 3.0, 80.0, 70.0, 0.55),
            _ => (6.0 + gain * 55.0, 8.0, 150.0, 90.0, 0.45),
        };
        self.g1 = g1;
        self.g2 = g2;
        self.hp.hp(sr, hpf, 0.7);
        self.bright.hshelf(sr, 3200.0, p[8] * 9.0 * (1.0 - gain * 0.7));
        self.c_hp.hp(sr, coupling, 0.7);
        self.bass.lshelf(sr, 110.0, (p[1] - 0.5) * 24.0);
        self.mid.peak(sr, 550.0, 0.7, (p[2] - 0.5) * 20.0);
        self.treb.hshelf(sr, 3000.0, (p[3] - 0.5) * 24.0);
        self.pres.hshelf(sr, 4500.0, (p[4] - 0.4) * 20.0);
        self.lp.lp(sr, 7500.0f32.min(0.4 * sr), 0.707);
        self.sag = p[7];
        self.master = db((p[5] - 0.5) * 18.0) * norm;
        self.ea = ms_coef(sr, 40.0);
        self.er = ms_coef(sr, 250.0);
    }
    fn run(&mut self, x: f32) -> f32 {
        let v = self.bright.tick(self.hp.tick(x));
        let s1 = self.os1.run(v * self.g1, |u| if u >= 0.0 { th(u) } else { th(u * 0.8) / 0.8 });
        let s1 = self.c_hp.tick(s1);
        let t = self.treb.tick(self.mid.tick(self.bass.tick(s1)));
        let s2 = self.os2.run(t * self.g2, th);
        follow(&mut self.env, s2.abs(), self.ea, self.er);
        let supply = 1.0 / (1.0 + self.sag * 1.2 * self.env);
        let y = th(s2 * 1.6 * supply) / 1.6;
        self.lp.tick(self.pres.tick(y)) * self.master
    }
}

// ---- Cab ----
// [hp, res_f, res_q, res_gain, dip_f, dip_gain, presence_f, presence_gain, lowpass]
const CABS: [[f32; 9]; 5] = [
    [90.0, 110.0, 1.2, 4.0, 700.0, -2.0, 2800.0, 4.0, 5500.0],  // 1x12
    [80.0, 100.0, 1.1, 3.0, 600.0, -3.0, 2500.0, 4.0, 5200.0],  // 2x12
    [75.0, 95.0, 1.0, 5.0, 450.0, -3.0, 2200.0, 5.0, 4800.0],   // 4x12
    [110.0, 150.0, 0.9, 2.0, 900.0, -2.0, 3000.0, 3.0, 5800.0], // open back
    [100.0, 120.0, 1.1, 3.0, 1000.0, -1.0, 1800.0, 5.0, 4200.0], // vintage
];
struct Cab {
    hp: Bq,
    res: Bq,
    dip: Bq,
    pres: Bq,
    lp1: Bq,
    lp2: Bq,
    lc: Bq,
    hc: Bq,
    room: f32,
    dl: DLine,
    d1: f32,
    d2: f32,
    out: f32,
}
impl Cab {
    fn new(sr: f32) -> Self {
        Cab {
            hp: Bq::default(),
            res: Bq::default(),
            dip: Bq::default(),
            pres: Bq::default(),
            lp1: Bq::default(),
            lp2: Bq::default(),
            lc: Bq::default(),
            hc: Bq::default(),
            room: 0.0,
            dl: DLine::new((0.006 * sr) as usize + 8),
            d1: 30.0,
            d2: 80.0,
            out: 1.0,
        }
    }
}
impl Ch for Cab {
    fn upd(&mut self, p: &[f32], sr: f32) {
        let t = ((p[0] * 4.0 + 0.5).floor() as usize).min(4);
        let c = &CABS[t];
        let m = p[1];
        self.hp.hp(sr, c[0], 0.8);
        self.res.peak(sr, c[1], c[2], c[3] + m * 2.0);
        self.dip.peak(sr, c[4], 1.0, c[5]);
        self.pres.peak(sr, c[6], 1.2, c[7] + lerp(3.0, -3.0, m));
        let lpf = c[8] * lerp(1.3, 0.7, m);
        self.lp1.lp(sr, lpf, 0.5412);
        self.lp2.lp(sr, lpf, 1.3066);
        self.lc.hp(sr, xp(p[2], 20.0, 300.0), 0.707);
        self.hc.lp(sr, xp(p[3], 2500.0, 12000.0), 0.707);
        self.room = p[5];
        self.d1 = 0.0008 * sr;
        self.d2 = 0.0021 * sr;
        self.out = db((p[4] - 0.5) * 20.0 + 1.0);
    }
    fn run(&mut self, x: f32) -> f32 {
        let mut y = self.hp.tick(x);
        y = self.pres.tick(self.dip.tick(self.res.tick(y)));
        y = self.hc.tick(self.lc.tick(self.lp2.tick(self.lp1.tick(y))));
        let w = y + self.room * (0.3 * self.dl.read(self.d1) + 0.18 * self.dl.read(self.d2));
        self.dl.push(y);
        w * self.out
    }
}

// ---- EQ ----
struct Eq5 {
    b: [Bq; 5],
    out: f32,
}
impl Ch for Eq5 {
    fn upd(&mut self, p: &[f32], sr: f32) {
        const F: [f32; 5] = [100.0, 400.0, 800.0, 1600.0, 3200.0];
        for i in 0..5 {
            self.b[i].peak(sr, F[i], 1.0, (p[i] - 0.5) * 24.0);
        }
        self.out = db((p[5] - 0.5) * 24.0);
    }
    fn run(&mut self, x: f32) -> f32 {
        let mut y = x;
        for b in self.b.iter_mut() {
            y = b.tick(y);
        }
        y * self.out
    }
}

// ---- Wah ----
struct Wah {
    svf: Svf,
    env: f32,
    ea: f32,
    er: f32,
    pos: f32,
    ps: f32,
    pedal: f32,
    auto: f32,
    sens: f32,
    q: f32,
    mix: f32,
    out: f32,
    cnt: u32,
    sr: f32,
}
impl Ch for Wah {
    fn upd(&mut self, p: &[f32], sr: f32) {
        self.sr = sr;
        self.pedal = p[0];
        self.auto = p[1];
        self.sens = p[2];
        self.er = ms_coef(sr, xp(p[3], 20.0, 400.0));
        self.ea = ms_coef(sr, 3.0);
        self.q = 1.5 + p[4] * 9.0;
        self.mix = p[5];
        self.out = db((p[6] - 0.5) * 18.0);
        self.ps = ms_coef(sr, 4.0);
    }
    fn run(&mut self, x: f32) -> f32 {
        follow(&mut self.env, x.abs(), self.ea, self.er);
        let tgt = (self.pedal + self.auto * (self.env * self.sens * 12.0).min(1.0)).clamp(0.0, 1.0);
        self.pos += self.ps * (tgt - self.pos);
        if self.cnt == 0 {
            self.svf.set(self.sr, xp(self.pos, 350.0, 2200.0), self.q);
        }
        self.cnt = (self.cnt + 1) & 7;
        let bp = self.svf.tick(x) * self.svf.k;
        lerp(x, bp * 2.5, self.mix) * self.out
    }
}

// ---- Chorus ----
struct Chorus {
    dl: DLine,
    ph: f32,
    inc: f32,
    centre: f32,
    depth: f32,
    mix: f32,
    lp: Bq,
    off: f32,
    ch: f32,
}
impl Ch for Chorus {
    fn upd(&mut self, p: &[f32], sr: f32) {
        self.inc = xp(p[0], 0.1, 8.0) / sr;
        self.centre = (6.0 + p[2] * 20.0) * 0.001 * sr;
        self.depth = (p[1] * 6.0 * 0.001 * sr).min(self.centre - 0.002 * sr);
        self.mix = p[3] * 0.5;
        self.lp.lp(sr, xp(p[4], 1500.0, 12000.0), 0.707);
        self.off = p[5] * 0.5 * self.ch;
    }
    fn run(&mut self, x: f32) -> f32 {
        self.dl.push(x);
        let a = (2.0 * PI * (self.ph + self.off)).sin();
        let b = (2.0 * PI * (self.ph + self.off + 0.5)).sin();
        self.ph = wrap1(self.ph + self.inc);
        let w = 0.5 * (self.dl.read(self.centre + self.depth * a) + self.dl.read(self.centre + self.depth * b));
        lerp(x, self.lp.tick(w), self.mix)
    }
}

// ---- Flanger ----
struct Flanger {
    dl: DLine,
    ph: f32,
    inc: f32,
    centre: f32,
    depth: f32,
    fb: f32,
    mix: f32,
    lp: Lp1,
    off: f32,
    ch: f32,
}
impl Ch for Flanger {
    fn upd(&mut self, p: &[f32], sr: f32) {
        self.inc = xp(p[0], 0.05, 5.0) / sr;
        self.centre = xp(p[2], 0.4, 4.0) * 0.001 * sr;
        self.depth = p[1] * 0.9;
        self.fb = (p[3] - 0.5) * 1.8;
        self.mix = p[4] * 0.5;
        self.lp.set(sr, 6000.0);
        self.off = p[5] * 0.5 * self.ch;
    }
    fn run(&mut self, x: f32) -> f32 {
        let l = (2.0 * PI * (self.ph + self.off)).sin();
        self.ph = wrap1(self.ph + self.inc);
        let y = self.dl.read(self.centre * (1.0 + self.depth * l));
        self.dl.push(x + self.fb * self.lp.tick(y));
        lerp(x, y, self.mix)
    }
}

// ---- Phaser ----
struct Phaser {
    z: [f32; 8],
    n: usize,
    ph: f32,
    inc: f32,
    center: f32,
    depth: f32,
    fb: f32,
    last: f32,
    a: f32,
    cnt: u32,
    mix: f32,
    sr: f32,
    off: f32,
    ch: f32,
}
impl Ch for Phaser {
    fn upd(&mut self, p: &[f32], sr: f32) {
        self.sr = sr;
        self.inc = xp(p[0], 0.05, 6.0) / sr;
        self.depth = p[1] * 2.2;
        self.center = xp(p[2], 250.0, 2500.0);
        self.fb = p[3] * 0.85;
        self.n = 4 + 2 * ((p[4] * 2.0).round() as usize).min(2);
        self.mix = p[5] * 0.5;
        self.off = p[6] * 0.5 * self.ch;
    }
    fn run(&mut self, x: f32) -> f32 {
        if self.cnt == 0 {
            let l = (2.0 * PI * (self.ph + self.off)).sin();
            let f = (self.center * (self.depth * l).exp2()).clamp(40.0, 0.45 * self.sr);
            let t = (PI * f / self.sr).tan();
            self.a = (t - 1.0) / (t + 1.0);
        }
        self.cnt = (self.cnt + 1) & 7;
        self.ph = wrap1(self.ph + self.inc);
        let mut v = x + self.fb * self.last;
        for i in 0..self.n {
            let y = self.a * v + self.z[i];
            self.z[i] = fl(v - self.a * y);
            v = y;
        }
        self.last = fl(v);
        lerp(x, v, self.mix)
    }
}

// ---- Tremolo ----
struct Trem {
    ph: f32,
    inc: f32,
    depth: f32,
    shape: u8,
    off: f32,
    lp: Bq,
    ch: f32,
}
impl Ch for Trem {
    fn upd(&mut self, p: &[f32], sr: f32) {
        self.inc = xp(p[0], 0.5, 14.0) / sr;
        self.depth = p[1];
        self.shape = if p[2] < 0.34 {
            0
        } else if p[2] < 0.67 {
            1
        } else {
            2
        };
        self.off = p[3] * 0.5 * self.ch;
        self.lp.lp(sr, 700.0, 0.707);
    }
    fn run(&mut self, x: f32) -> f32 {
        let s = (2.0 * PI * (self.ph + self.off)).sin();
        self.ph = wrap1(self.ph + self.inc);
        let d = self.depth * 0.5;
        match self.shape {
            0 => x * (1.0 - d * (1.0 - s)),
            1 => x * (1.0 - d * (1.0 - th(s * 6.0))),
            _ => {
                let lo = self.lp.tick(x);
                let hi = x - lo;
                lo * (1.0 - d * (1.0 - s)) + hi * (1.0 - d * (1.0 + s))
            }
        }
    }
}

// ---- Vibrato ----
struct Vib {
    dl: DLine,
    ph: f32,
    inc: f32,
    centre: f32,
    depth: f32,
    mix: f32,
    lp: Bq,
    off: f32,
    ch: f32,
}
impl Ch for Vib {
    fn upd(&mut self, p: &[f32], sr: f32) {
        self.inc = xp(p[0], 0.5, 9.0) / sr;
        let dms = p[1] * 3.0;
        self.depth = dms * 0.001 * sr;
        self.centre = (1.0 + dms) * 0.001 * sr + 1.0;
        self.off = p[2] * 0.5 * self.ch;
        self.lp.lp(sr, xp(p[3], 2000.0, 14000.0), 0.707);
        self.mix = p[4];
    }
    fn run(&mut self, x: f32) -> f32 {
        self.dl.push(x);
        let s = (2.0 * PI * (self.ph + self.off)).sin();
        self.ph = wrap1(self.ph + self.inc);
        let w = self.lp.tick(self.dl.read(self.centre + self.depth * s));
        lerp(x, w, self.mix)
    }
}

// ---- Octaver ----
struct Oct {
    pre: Bq,
    lp_sub: Bq,
    hp_up: Bq,
    env: f32,
    ea: f32,
    er: f32,
    state: bool,
    ff: f32,
    dry: f32,
    sub: f32,
    up: f32,
    out: f32,
}
impl Ch for Oct {
    fn upd(&mut self, p: &[f32], sr: f32) {
        self.pre.lp(sr, xp(p[4], 200.0, 1500.0), 0.707);
        self.lp_sub.lp(sr, xp(p[3], 150.0, 900.0), 0.707);
        self.hp_up.hp(sr, 90.0, 0.707);
        self.ea = ms_coef(sr, 2.0);
        self.er = ms_coef(sr, 40.0);
        self.sub = p[0];
        self.up = p[1];
        self.dry = p[2];
        self.out = db((p[5] - 0.5) * 18.0);
    }
    fn run(&mut self, x: f32) -> f32 {
        let a = x.abs();
        follow(&mut self.env, a, self.ea, self.er);
        let f = self.pre.tick(x);
        let h = (0.1 * self.env).max(1e-4);
        if !self.state && f > h {
            self.state = true;
            self.ff = -self.ff;
        } else if self.state && f < -h {
            self.state = false;
        }
        let s = self.lp_sub.tick(self.ff * self.env * 1.4);
        let u = self.hp_up.tick(a) * 2.0;
        (self.dry * x + 1.5 * self.sub * s + 1.5 * self.up * u) * self.out
    }
}

// ---- Slicer ----
const SLICE_PATTERNS: [u8; 8] = [0b10101010, 0b11001100, 0b10010010, 0b11101110, 0b10110110, 0b11110000, 0b10001000, 0b11011010];
struct Slicer {
    ph: f32,
    inc: f32,
    pat: u8,
    depth: f32,
    sa: f32,
    g: f32,
    off: f32,
    ch: f32,
}
impl Ch for Slicer {
    fn upd(&mut self, p: &[f32], sr: f32) {
        self.inc = xp(p[0], 2.0, 16.0) / sr;
        self.depth = p[1];
        self.sa = ms_coef(sr, xp(p[2], 0.5, 30.0));
        self.pat = SLICE_PATTERNS[((p[3] * 7.99) as usize).min(7)];
        self.off = p[4] * 4.0 * self.ch;
    }
    fn run(&mut self, x: f32) -> f32 {
        self.ph += self.inc;
        if self.ph >= 8.0 {
            self.ph -= 8.0;
        }
        let s = (((self.ph + self.off) % 8.0) as usize) & 7;
        let on = (self.pat >> s) & 1 == 1;
        let tg = if on { 1.0 } else { 1.0 - self.depth };
        self.g += self.sa * (tg - self.g);
        x * self.g
    }
}

// ---- Volume / pan / swell (stereo) ----
struct Vol {
    tgt: f32,
    g: f32,
    init: bool,
    a: f32,
    pl: f32,
    pr: f32,
    swell_on: bool,
    inc: f32,
    thr: f32,
    env: f32,
    ea: f32,
    er: f32,
    armed: bool,
    ramp: f32,
}
impl Ped for Vol {
    fn update(&mut self, p: &[f32], sr: f32) {
        self.tgt = p[0] * p[0] * db(p[1] * 12.0);
        self.pl = (2.0 * (1.0 - p[2])).min(1.0);
        self.pr = (2.0 * p[2]).min(1.0);
        self.swell_on = p[3] > 0.001;
        self.inc = 1.0 / (xp(p[3].max(0.001), 0.05, 4.0) * sr);
        self.thr = xp(p[4], 0.004, 0.15);
        self.a = ms_coef(sr, 8.0);
        self.ea = ms_coef(sr, 1.0);
        self.er = ms_coef(sr, 30.0);
    }
    fn tick(&mut self, l: f32, r: f32) -> (f32, f32) {
        if !self.init {
            self.g = self.tgt;
            self.init = true;
        }
        self.g += self.a * (self.tgt - self.g);
        let mut sw = 1.0;
        if self.swell_on {
            follow(&mut self.env, l.abs().max(r.abs()), self.ea, self.er);
            if self.env < self.thr * 0.3 {
                self.ramp = 0.0;
                self.armed = true;
            } else if self.armed && self.env > self.thr {
                self.armed = false;
            }
            if !self.armed && self.ramp < 1.0 {
                self.ramp = (self.ramp + self.inc).min(1.0);
            }
            sw = self.ramp * self.ramp;
        }
        let g = self.g * sw;
        (l * g * self.pl, r * g * self.pr)
    }
}

// ---- Delay (stereo) ----
struct Delay {
    sr: f32,
    l: DLine,
    r: DLine,
    cur: f32,
    tgt: f32,
    slew: f32,
    fb: f32,
    mode: u8,
    wow: f32,
    lpl: Lp1,
    lpr: Lp1,
    hpl: Bq,
    hpr: Bq,
    mix: f32,
    pp: f32,
    ph1: f32,
    ph2: f32,
    inc1: f32,
    inc2: f32,
}
impl Delay {
    fn new(sr: f32) -> Self {
        let cap = (1.3 * sr) as usize + 16;
        Delay {
            sr,
            l: DLine::new(cap),
            r: DLine::new(cap),
            cur: -1.0,
            tgt: 0.1 * sr,
            slew: 0.001,
            fb: 0.3,
            mode: 0,
            wow: 0.0,
            lpl: Lp1::default(),
            lpr: Lp1::default(),
            hpl: Bq::default(),
            hpr: Bq::default(),
            mix: 0.3,
            pp: 0.0,
            ph1: 0.0,
            ph2: 0.3,
            inc1: 0.5 / sr,
            inc2: 4.7 / sr,
        }
    }
}
impl Ped for Delay {
    fn update(&mut self, p: &[f32], sr: f32) {
        self.sr = sr;
        self.tgt = xp(p[0], 20.0, 1200.0) * 0.001 * sr;
        self.fb = p[1] * 0.96;
        self.mode = if p[4] < 0.34 {
            0
        } else if p[4] < 0.67 {
            1
        } else {
            2
        };
        let fc = match self.mode {
            0 => xp(p[2], 2000.0, 16000.0),
            1 => xp(p[2], 800.0, 5000.0),
            _ => xp(p[2], 1200.0, 7000.0),
        };
        self.lpl.set(sr, fc);
        self.lpr.set(sr, fc);
        self.hpl.hp(sr, 120.0, 0.707);
        self.hpr.hp(sr, 120.0, 0.707);
        self.mix = p[3];
        self.wow = if self.mode == 2 { p[5] * 0.003 * sr } else { 0.0 };
        self.pp = p[6];
        self.slew = ms_coef(sr, 50.0);
    }
    fn tick(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.cur < 0.0 {
            self.cur = self.tgt;
        }
        self.cur += self.slew * (self.tgt - self.cur);
        let s1 = (2.0 * PI * self.ph1).sin();
        let s2 = (2.0 * PI * self.ph2).sin();
        let s1r = (2.0 * PI * (self.ph1 + 0.25)).sin();
        self.ph1 = wrap1(self.ph1 + self.inc1);
        self.ph2 = wrap1(self.ph2 + self.inc2);
        let tl = self.cur + self.wow * (s1 + 0.3 * s2);
        let tr = self.cur + self.wow * (s1r + 0.3 * s2);
        let dl = self.l.read(tl);
        let dr = self.r.read(tr);
        let mut fl_ = self.lpl.tick(dl);
        let mut fr_ = self.lpr.tick(dr);
        if self.mode >= 1 {
            fl_ = th(self.hpl.tick(fl_) * 1.3) / 1.3;
            fr_ = th(self.hpr.tick(fr_) * 1.3) / 1.3;
        }
        let pp = self.pp;
        let cl = fl_ * (1.0 - pp) + fr_ * pp;
        let cr = fr_ * (1.0 - pp) + fl_ * pp;
        let m = 0.5 * (l + r);
        let in_l = l * (1.0 - pp) + m * pp;
        let in_r = r * (1.0 - pp);
        self.l.push(fl(in_l + self.fb * cl));
        self.r.push(fl(in_r + self.fb * cr));
        (l + fl_ * self.mix, r + fr_ * self.mix)
    }
}

// ---- Reverb (stereo FDN) ----
const RV_BASE: [[f32; 8]; 3] = [
    [4.1, 5.3, 6.7, 8.3, 9.9, 11.3, 13.1, 14.7],
    [11.3, 14.9, 19.1, 23.3, 27.7, 31.9, 36.7, 41.3],
    [29.7, 37.1, 41.1, 43.7, 53.9, 61.3, 70.1, 79.3],
];
const RV_INJ: [f32; 8] = [1.0, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0];
const RV_OL: [f32; 8] = [1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 1.0, -1.0];
const RV_OR: [f32; 8] = [1.0, 1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 1.0];
const RV_AP_MS: [[f32; 6]; 2] = [[5.1, 7.7, 10.9, 13.3, 0.0, 0.0], [1.7, 2.3, 3.1, 3.9, 5.3, 6.7]];
struct Rev {
    sr: f32,
    lines: Vec<DLine>,
    len: [f32; 8],
    tgt: [f32; 8],
    g: [f32; 8],
    lpz: [f32; 8],
    lpk: f32,
    ph: [f32; 8],
    pinc: [f32; 8],
    moddep: f32,
    pre: DLine,
    pre_n: f32,
    aps: Vec<Ap>,
    nap: usize,
    inj: f32,
    hp: Bq,
    mix: f32,
    started: bool,
}
impl Rev {
    fn new(sr: f32) -> Self {
        let cap = (0.2 * sr) as usize + 16;
        let apcap = (0.02 * sr) as usize + 16;
        Rev {
            sr,
            lines: (0..8).map(|_| DLine::new(cap)).collect(),
            len: [100.0; 8],
            tgt: [100.0; 8],
            g: [0.8; 8],
            lpz: [0.0; 8],
            lpk: 0.5,
            ph: [0.0, 0.13, 0.29, 0.41, 0.53, 0.67, 0.79, 0.91],
            pinc: [0.0; 8],
            moddep: 0.0,
            pre: DLine::new((0.16 * sr) as usize + 16),
            pre_n: 1.0,
            aps: (0..6).map(|_| Ap::new(apcap)).collect(),
            nap: 4,
            inj: 0.1,
            hp: Bq::default(),
            mix: 0.3,
            started: false,
        }
    }
}
impl Ped for Rev {
    fn update(&mut self, p: &[f32], sr: f32) {
        self.sr = sr;
        let ty = if p[0] < 0.34 {
            0
        } else if p[0] < 0.67 {
            1
        } else {
            2
        };
        let sc = 0.5 + p[1] * 1.3;
        let t60 = xp(p[2], 0.4, 10.0) * [0.45, 0.8, 1.0][ty];
        let mut gsum = 0.0;
        for i in 0..8 {
            self.tgt[i] = RV_BASE[ty][i] * sc * 0.001 * sr;
            self.g[i] = 10f32.powf(-3.0 * (self.tgt[i] / sr) / t60);
            gsum += self.g[i];
            self.pinc[i] = (0.2 + 0.09 * i as f32) / sr;
        }
        if !self.started {
            self.len = self.tgt;
            self.started = true;
        }
        let gavg = gsum / 8.0;
        self.inj = (0.5 * (1.0 - gavg * gavg).max(0.0).sqrt()).max(0.02);
        self.lpk = onepole(sr, xp(1.0 - p[3], 1200.0, 14000.0));
        self.pre_n = (p[4] * 0.15 * sr).max(1.0);
        self.hp.hp(sr, xp(p[6], 20.0, 600.0), 0.707);
        self.mix = p[5];
        self.moddep = p[7] * 0.0015 * sr;
        let apt = if ty == 0 { 1 } else { 0 };
        self.nap = if ty == 0 { 6 } else { 4 };
        for i in 0..6 {
            self.aps[i].d = (RV_AP_MS[apt][i] * 0.001 * sr).max(1.0);
            self.aps[i].g = if ty == 0 { 0.72 } else { 0.65 };
        }
    }
    fn tick(&mut self, l: f32, r: f32) -> (f32, f32) {
        self.pre.push(0.5 * (l + r));
        let mut x = self.hp.tick(self.pre.read(self.pre_n));
        for i in 0..self.nap {
            x = self.aps[i].tick(x);
        }
        let mut d = [0f32; 8];
        for i in 0..8 {
            self.len[i] += (self.tgt[i] - self.len[i]) * 0.0004;
            self.ph[i] = wrap1(self.ph[i] + self.pinc[i]);
            let m = self.moddep * (2.0 * PI * self.ph[i]).sin();
            let rd = self.lines[i].read(self.len[i] + m);
            self.lpz[i] = fl(self.lpz[i] + self.lpk * (rd - self.lpz[i]));
            d[i] = self.lpz[i] * self.g[i];
        }
        let mut wl = 0.0;
        let mut wr = 0.0;
        for i in 0..8 {
            wl += RV_OL[i] * d[i];
            wr += RV_OR[i] * d[i];
        }
        wl *= 0.35355 * 2.0;
        wr *= 0.35355 * 2.0;
        // 8-point Walsh-Hadamard mix
        let mut h = 1;
        while h < 8 {
            let mut i = 0;
            while i < 8 {
                for j in i..i + h {
                    let a = d[j];
                    let b = d[j + h];
                    d[j] = a + b;
                    d[j + h] = a - b;
                }
                i += h * 2;
            }
            h *= 2;
        }
        for i in 0..8 {
            self.lines[i].push(fl(x * self.inj * RV_INJ[i] + d[i] * 0.35355));
        }
        let m = self.mix;
        (l * (1.0 - m) + wl * m, r * (1.0 - m) + wr * m)
    }
}

fn make(kind: usize, sr: f32) -> Box<dyn Ped> {
    let cap = |s: f32| (s * sr) as usize + 16;
    match kind {
        0 => Box::new(Dual::new(|_| Gate::new())),
        1 => Box::new(Dual::new(|_| Comp::new())),
        2 => Box::new(Dual::new(|_| Boost::new())),
        3 => Box::new(Dual::new(|_| Od::new(sr))),
        4 => Box::new(Dual::new(|_| Dist::new(sr))),
        5 => Box::new(Dual::new(|_| Fuzz::new(sr))),
        6 => Box::new(Dual::new(|_| Amp::new(sr))),
        7 => Box::new(Dual::new(|_| Cab::new(sr))),
        8 => Box::new(Dual::new(|_| Eq5 { b: [Bq::default(); 5], out: 1.0 })),
        9 => Box::new(Dual::new(|_| Wah {
            svf: Svf::default(),
            env: 0.0,
            ea: 0.1,
            er: 0.01,
            pos: 0.5,
            ps: 0.01,
            pedal: 0.5,
            auto: 0.0,
            sens: 0.5,
            q: 4.0,
            mix: 1.0,
            out: 1.0,
            cnt: 0,
            sr,
        })),
        10 => Box::new(Dual::new(|c| Chorus {
            dl: DLine::new(cap(0.06)),
            ph: 0.0,
            inc: 0.0,
            centre: 300.0,
            depth: 0.0,
            mix: 0.4,
            lp: Bq::default(),
            off: 0.0,
            ch: c as f32,
        })),
        11 => Box::new(Dual::new(|c| Flanger {
            dl: DLine::new(cap(0.02)),
            ph: 0.0,
            inc: 0.0,
            centre: 50.0,
            depth: 0.5,
            fb: 0.0,
            mix: 0.4,
            lp: Lp1::default(),
            off: 0.0,
            ch: c as f32,
        })),
        12 => Box::new(Dual::new(|c| Phaser {
            z: [0.0; 8],
            n: 4,
            ph: 0.0,
            inc: 0.0,
            center: 800.0,
            depth: 1.0,
            fb: 0.0,
            last: 0.0,
            a: 0.0,
            cnt: 0,
            mix: 0.4,
            sr,
            off: 0.0,
            ch: c as f32,
        })),
        13 => Box::new(Dual::new(|c| Trem { ph: 0.0, inc: 0.0, depth: 0.5, shape: 0, off: 0.0, lp: Bq::default(), ch: c as f32 })),
        14 => Box::new(Dual::new(|c| Vib {
            dl: DLine::new(cap(0.02)),
            ph: 0.0,
            inc: 0.0,
            centre: 50.0,
            depth: 10.0,
            mix: 1.0,
            lp: Bq::default(),
            off: 0.0,
            ch: c as f32,
        })),
        15 => Box::new(Delay::new(sr)),
        16 => Box::new(Rev::new(sr)),
        17 => Box::new(Dual::new(|_| Oct {
            pre: Bq::default(),
            lp_sub: Bq::default(),
            hp_up: Bq::default(),
            env: 0.0,
            ea: 0.1,
            er: 0.01,
            state: false,
            ff: 1.0,
            dry: 0.6,
            sub: 0.6,
            up: 0.3,
            out: 1.0,
        })),
        18 => Box::new(Vol {
            tgt: 1.0,
            g: 1.0,
            init: false,
            a: 0.01,
            pl: 1.0,
            pr: 1.0,
            swell_on: false,
            inc: 0.0001,
            thr: 0.03,
            env: 0.0,
            ea: 0.1,
            er: 0.001,
            armed: true,
            ramp: 1.0,
        }),
        _ => Box::new(Dual::new(|c| Slicer { ph: 0.0, inc: 0.0, pat: 0xAA, depth: 0.9, sa: 0.05, g: 1.0, off: 0.0, ch: c as f32 })),
    }
}

// ---------------------------------------------------------------------------------------------------------
// The rig
// ---------------------------------------------------------------------------------------------------------

struct Node {
    kind: usize,
    params: Vec<f32>,
    bypass: bool,
    input: i32,
    out: bool,
    level: f32,
    dsp: Box<dyn Ped>,
    peak: f32,
    gr: f32,
}

pub struct Rig {
    sr: f32,
    nodes: Vec<Node>,
    p_input: f32,
    p_level: f32,
    p_mix: f32,
    gi: f32,
    go: f32,
}

type PresetNode = (&'static str, &'static [(&'static str, f32)], i32, bool, f32);

impl Rig {
    pub fn new(sr: f32) -> Self {
        Rig {
            sr: sr.clamp(8000.0, 192000.0),
            nodes: Vec::with_capacity(MAX_NODES),
            p_input: 0.5,
            p_level: 0.5,
            p_mix: 1.0,
            gi: 1.0,
            go: 1.0,
        }
    }

    fn new_node(&self, kind: usize) -> Node {
        Node {
            kind,
            params: KINDS[kind].1.iter().map(|p| p.1).collect(),
            bypass: false,
            input: -1,
            out: false,
            level: 1.0,
            dsp: make(kind, self.sr),
            peak: 0.0,
            gr: 0.0,
        }
    }

    fn idx(&self, v: Option<&f64>) -> Option<usize> {
        let v = *v?;
        if !v.is_finite() || v < 0.0 {
            return None;
        }
        let i = v.round() as usize;
        if i < self.nodes.len() {
            Some(i)
        } else {
            None
        }
    }

    fn is_chain(&self) -> bool {
        self.nodes.iter().enumerate().all(|(i, n)| n.input == i as i32 - 1)
    }

    fn move_node(&mut self, from: usize, to: usize) {
        let n = self.nodes.len();
        if from == to || n < 2 {
            return;
        }
        let chain = self.is_chain();
        let only_last_out = self.nodes.iter().enumerate().all(|(i, nd)| nd.out == (i == n - 1));
        let old_in: Vec<i32> = self.nodes.iter().map(|x| x.input).collect();
        let mut order: Vec<usize> = (0..n).collect();
        let v = order.remove(from);
        order.insert(to, v);
        let mut newpos = vec![0usize; n];
        for (nw, &old) in order.iter().enumerate() {
            newpos[old] = nw;
        }
        let mut slots: Vec<Option<Node>> = std::mem::take(&mut self.nodes).into_iter().map(Some).collect();
        let mut out = Vec::with_capacity(MAX_NODES);
        for (nw, &old) in order.iter().enumerate() {
            let mut nd = slots[old].take().unwrap();
            let ni = if chain {
                nw as i32 - 1
            } else {
                let mut j = old_in[old];
                while j >= 0 && newpos[j as usize] >= nw {
                    j = old_in[j as usize];
                }
                if j < 0 {
                    -1
                } else {
                    newpos[j as usize] as i32
                }
            };
            nd.input = ni;
            if only_last_out {
                nd.out = nw == n - 1;
            }
            out.push(nd);
        }
        self.nodes = out;
    }

    fn remove_node(&mut self, idx: usize) {
        let rm_in = self.nodes[idx].input;
        let rm_out = self.nodes[idx].out;
        self.nodes.remove(idx);
        let i = idx as i32;
        for nd in self.nodes.iter_mut() {
            if nd.input == i {
                nd.input = rm_in;
            } else if nd.input > i {
                nd.input -= 1;
            }
        }
        if rm_out && rm_in >= 0 {
            self.nodes[rm_in as usize].out = true;
        }
    }

    fn node_json(nd: &Node) -> Value {
        let mut m = Map::new();
        for (i, (name, _)) in KINDS[nd.kind].1.iter().enumerate() {
            m.insert((*name).to_string(), json!(nd.params[i]));
        }
        json!({
            "kind": KINDS[nd.kind].0,
            "params": Value::Object(m),
            "bypass": nd.bypass,
            "input": nd.input,
            "out": nd.out,
            "level": nd.level,
        })
    }

    fn load_config(&mut self, text: &str) -> bool {
        let v: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let arr = if v.is_array() { v.as_array() } else { v.get("nodes").and_then(|n| n.as_array()) };
        let arr = match arr {
            Some(a) => a,
            None => return false,
        };
        let mut nodes: Vec<Node> = Vec::with_capacity(MAX_NODES);
        let mut map: Vec<i32> = Vec::new();
        for item in arr {
            let raw = item.get("input").and_then(|x| x.as_i64()).unwrap_or(-1);
            let inp = if raw >= 0 && (raw as usize) < map.len() { map[raw as usize] } else { -1 };
            let kind = item.get("kind").and_then(|k| k.as_str()).and_then(kind_index);
            match kind {
                Some(k) if nodes.len() < MAX_NODES => {
                    let mut nd = self.new_node(k);
                    nd.input = inp;
                    nd.bypass = item.get("bypass").and_then(|b| b.as_bool()).unwrap_or(false);
                    nd.out = item.get("out").and_then(|b| b.as_bool()).unwrap_or(false);
                    nd.level = item.get("level").and_then(|b| b.as_f64()).map(|x| (x as f32).clamp(0.0, 1.5)).unwrap_or(1.0);
                    if let Some(po) = item.get("params").and_then(|p| p.as_object()) {
                        for (i, (name, _)) in KINDS[k].1.iter().enumerate() {
                            if let Some(x) = po.get(*name).and_then(|x| x.as_f64()) {
                                if x.is_finite() {
                                    nd.params[i] = (x as f32).clamp(0.0, 1.0);
                                }
                            }
                        }
                    }
                    map.push(nodes.len() as i32);
                    nodes.push(nd);
                }
                _ => map.push(inp), // skipped node: later references fall through to its input
            }
        }
        if let Some(x) = v.get("input").and_then(|x| x.as_f64()) {
            self.p_input = (x as f32).clamp(0.0, 1.0);
        }
        if let Some(x) = v.get("level").and_then(|x| x.as_f64()) {
            self.p_level = (x as f32).clamp(0.0, 1.0);
        }
        if let Some(x) = v.get("mix").and_then(|x| x.as_f64()) {
            self.p_mix = (x as f32).clamp(0.0, 1.0);
        }
        self.nodes = nodes;
        true
    }

    fn load_preset(&mut self, name: &str) -> bool {
        let def = match PRESET_DEFS.iter().find(|p| p.0.eq_ignore_ascii_case(name.trim())) {
            Some(d) => d,
            None => return false,
        };
        let mut nodes: Vec<Node> = Vec::new();
        for (kname, ps, input, out, level) in def.1.iter() {
            let k = match kind_index(kname) {
                Some(k) => k,
                None => continue,
            };
            let mut nd = self.new_node(k);
            for (pn, pv) in ps.iter() {
                if let Some(i) = KINDS[k].1.iter().position(|q| q.0 == *pn) {
                    nd.params[i] = pv.clamp(0.0, 1.0);
                }
            }
            nd.input = if *input < nodes.len() as i32 { *input } else { -1 };
            nd.out = *out;
            nd.level = *level;
            nodes.push(nd);
        }
        self.nodes = nodes;
        true
    }
}

impl Effect for Rig {
    fn process(&mut self, buf: &mut [f32]) {
        let frames = buf.len() / 2;
        let n = self.nodes.len();
        let gi_t = db((self.p_input - 0.5) * 36.0);
        let go_t = db((self.p_level - 0.5) * 36.0);
        let mix = self.p_mix;
        let any_out = self.nodes.iter().any(|x| x.out);
        let sr = self.sr;
        let mut f = 0;
        while f < frames {
            let end = (f + CHUNK).min(frames);
            for nd in self.nodes.iter_mut() {
                nd.dsp.update(&nd.params, sr);
            }
            let mut cm = [0f32; MAX_NODES];
            for i in f..end {
                let dl = buf[2 * i];
                let dr = buf[2 * i + 1];
                self.gi += (gi_t - self.gi) * 0.002;
                self.go += (go_t - self.go) * 0.002;
                let il = dl * self.gi;
                let ir = dr * self.gi;
                let mut outs = [(0f32, 0f32); MAX_NODES];
                let (mut sl, mut sr_) = if n == 0 { (il, ir) } else { (0.0, 0.0) };
                for k in 0..n {
                    let nd = &mut self.nodes[k];
                    let (a, b) = if nd.input < 0 { (il, ir) } else { outs[nd.input as usize] };
                    let (mut y0, mut y1) = if nd.bypass { (a, b) } else { nd.dsp.tick(a, b) };
                    if !y0.is_finite() {
                        y0 = 0.0;
                    }
                    if !y1.is_finite() {
                        y1 = 0.0;
                    }
                    outs[k] = (y0, y1);
                    cm[k] = cm[k].max(y0.abs().max(y1.abs()));
                    if nd.out || (!any_out && k == n - 1) {
                        sl += y0 * nd.level;
                        sr_ += y1 * nd.level;
                    }
                }
                let wl = lim(sl * self.go);
                let wr = lim(sr_ * self.go);
                buf[2 * i] = wl * mix + dl * (1.0 - mix);
                buf[2 * i + 1] = wr * mix + dr * (1.0 - mix);
            }
            for (k, nd) in self.nodes.iter_mut().enumerate() {
                nd.peak = (nd.peak * 0.9).max(cm[k]).min(1.0);
                nd.gr = nd.dsp.gr();
            }
            f = end;
        }
    }

    fn set_param(&mut self, name: &str, v: f32) {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { return };
        match name {
            "input" => self.p_input = v,
            "level" => self.p_level = v,
            "mix" => self.p_mix = v,
            _ => {}
        }
    }

    fn name(&self) -> &'static str {
        "Ringstorm"
    }

    fn view(&self) -> Vec<f32> {
        self.nodes.iter().map(|n| n.peak).collect()
    }

    fn command(&mut self, cmd: &str, args: &[f64], text: &str) -> Vec<f64> {
        let a0 = args.first();
        let a1 = args.get(1);
        match cmd {
            "kinds" => vec![KINDS.len() as f64],
            "count" => vec![self.nodes.len() as f64],
            "clear" => {
                self.nodes.clear();
                vec![1.0]
            }
            "add" => {
                if self.nodes.len() >= MAX_NODES {
                    return vec![];
                }
                let k = match kind_index(text) {
                    Some(k) => k,
                    None => match a0 {
                        Some(&v) if text.trim().is_empty() && v >= 0.0 && (v as usize) < KINDS.len() => v as usize,
                        _ => return vec![],
                    },
                };
                let mut nd = self.new_node(k);
                if let Some(last) = self.nodes.last_mut() {
                    last.out = false;
                    nd.input = self.nodes.len() as i32 - 1;
                }
                nd.out = true;
                self.nodes.push(nd);
                vec![(self.nodes.len() - 1) as f64]
            }
            "remove" => match self.idx(a0) {
                Some(i) => {
                    self.remove_node(i);
                    vec![1.0]
                }
                None => vec![],
            },
            "move" => match (self.idx(a0), self.idx(a1)) {
                (Some(f), Some(t)) => {
                    self.move_node(f, t);
                    vec![1.0]
                }
                _ => vec![],
            },
            "set_input" => match (self.idx(a0), a1) {
                (Some(i), Some(&v)) if v.is_finite() => {
                    let inp = v.round() as i64;
                    if inp == -1 || (inp >= 0 && (inp as usize) < i) {
                        self.nodes[i].input = inp as i32;
                        vec![1.0]
                    } else {
                        vec![]
                    }
                }
                _ => vec![],
            },
            "set_out" => match (self.idx(a0), a1) {
                (Some(i), Some(&v)) => {
                    self.nodes[i].out = v >= 0.5;
                    vec![1.0]
                }
                _ => vec![],
            },
            "set_level" => match (self.idx(a0), a1) {
                (Some(i), Some(&v)) if v.is_finite() => {
                    self.nodes[i].level = (v as f32).clamp(0.0, 1.5);
                    vec![1.0]
                }
                _ => vec![],
            },
            "bypass" => match (self.idx(a0), a1) {
                (Some(i), Some(&v)) => {
                    self.nodes[i].bypass = v >= 0.5;
                    vec![1.0]
                }
                _ => vec![],
            },
            "param" => match (self.idx(a0), a1) {
                (Some(i), Some(&v)) if v.is_finite() => {
                    let k = self.nodes[i].kind;
                    match KINDS[k].1.iter().position(|q| q.0 == text.trim()) {
                        Some(pi) => {
                            self.nodes[i].params[pi] = (v as f32).clamp(0.0, 1.0);
                            vec![1.0]
                        }
                        None => vec![],
                    }
                }
                _ => vec![],
            },
            "get_param" => match self.idx(a0) {
                Some(i) => {
                    let k = self.nodes[i].kind;
                    match KINDS[k].1.iter().position(|q| q.0 == text.trim()) {
                        Some(pi) => vec![self.nodes[i].params[pi] as f64],
                        None => vec![],
                    }
                }
                None => vec![],
            },
            "meter" => match self.idx(a0) {
                Some(i) => vec![self.nodes[i].peak as f64, self.nodes[i].gr as f64],
                None => vec![],
            },
            "load_config" => {
                if self.load_config(text) {
                    vec![1.0]
                } else {
                    vec![]
                }
            }
            "load_preset" => {
                if self.load_preset(text) {
                    vec![1.0]
                } else {
                    vec![]
                }
            }
            _ => vec![],
        }
    }

    fn info(&self, what: &str) -> String {
        match what {
            "config" => json!({
                "input": self.p_input,
                "level": self.p_level,
                "mix": self.p_mix,
                "nodes": self.nodes.iter().map(Self::node_json).collect::<Vec<_>>(),
            })
            .to_string(),
            "nodes" => Value::Array(self.nodes.iter().map(Self::node_json).collect()).to_string(),
            "kinds" => Value::Array(
                KINDS
                    .iter()
                    .map(|k| {
                        json!({
                            "kind": k.0,
                            "params": k.1.iter().map(|p| json!({"name": p.0, "def": p.1})).collect::<Vec<_>>(),
                        })
                    })
                    .collect(),
            )
            .to_string(),
            "presets" => Value::Array(PRESETS.iter().map(|p| json!(p.0)).collect()).to_string(),
            _ => String::new(),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Presets: (kind, param overrides, input, out, level)
// ---------------------------------------------------------------------------------------------------------

macro_rules! pn {
    ($k:expr, $p:expr, $i:expr, $o:expr) => {
        ($k, { const P: &[(&str, f32)] = &$p; P }, $i, $o, 1.0f32)
    };
    ($k:expr, $p:expr, $i:expr, $o:expr, $l:expr) => {
        ($k, { const P: &[(&str, f32)] = &$p; P }, $i, $o, $l)
    };
}

static PRESET_DEFS: [(&str, &[PresetNode]); 20] = [
    (
        "Clean Sparkle",
        &[
            pn!("Compressor", [("sustain", 0.35), ("attack", 0.4)], -1, false),
            pn!("Amp", [("gain", 0.35), ("treble", 0.6), ("bright", 0.5), ("presence", 0.5), ("channel", 0.0)], 0, false),
            pn!("Cab", [("type", 0.25), ("mic", 0.3)], 1, false),
            pn!("Chorus", [("rate", 0.25), ("depth", 0.4), ("mix", 0.9)], 2, false),
            pn!("Reverb", [("type", 0.5), ("mix", 0.25), ("decay", 0.45)], 3, true),
        ],
    ),
    (
        "Blues Crunch",
        &[
            pn!("Boost", [("gain", 0.3), ("grit", 0.2)], -1, false),
            pn!("Overdrive", [("drive", 0.35), ("tone", 0.5)], 0, false),
            pn!("Amp", [("gain", 0.4), ("mid", 0.6), ("channel", 0.5)], 1, false),
            pn!("Cab", [("type", 0.25), ("mic", 0.4)], 2, false),
            pn!("Reverb", [("type", 0.0), ("mix", 0.2), ("decay", 0.35)], 3, true),
        ],
    ),
    (
        "Classic Rock Stack",
        &[
            pn!("Overdrive", [("drive", 0.25), ("level", 0.6)], -1, false),
            pn!("Amp", [("gain", 0.6), ("mid", 0.65), ("presence", 0.55), ("master", 0.6), ("sag", 0.5), ("channel", 0.5)], 0, false),
            pn!("Cab", [("type", 0.5), ("mic", 0.2)], 1, false),
            pn!("Delay", [("time", 0.5), ("feedback", 0.2), ("mix", 0.15), ("mode", 0.5)], 2, true),
        ],
    ),
    (
        "High Gain Metal",
        &[
            pn!("Noise Gate", [("threshold", 0.55)], -1, false),
            pn!("Overdrive", [("drive", 0.3), ("tone", 0.6), ("level", 0.7), ("tight", 0.7)], 0, false),
            pn!("Amp", [("gain", 0.7), ("bass", 0.55), ("mid", 0.35), ("treble", 0.6), ("presence", 0.5), ("sag", 0.2), ("channel", 1.0)], 1, false),
            pn!("Cab", [("type", 0.5), ("mic", 0.15), ("low_cut", 0.3)], 2, true),
        ],
    ),
    (
        "Fuzz Face 70s",
        &[
            pn!("Fuzz", [("fuzz", 0.7), ("bias", 0.45), ("tone", 0.5), ("mode", 0.0)], -1, false),
            pn!("Amp", [("gain", 0.5), ("master", 0.6), ("channel", 0.0)], 0, false),
            pn!("Cab", [("type", 1.0)], 1, false),
            pn!("Reverb", [("type", 0.0), ("mix", 0.2)], 2, true),
        ],
    ),
    (
        "Shoegaze Wall",
        &[
            pn!("Fuzz", [("fuzz", 0.7), ("tone", 0.45), ("mode", 1.0)], -1, false),
            pn!("Cab", [("type", 0.75), ("mic", 0.5)], 0, false),
            pn!("Tremolo", [("rate", 0.3), ("depth", 0.35)], 1, false),
            pn!("Delay", [("time", 0.6), ("feedback", 0.55), ("mix", 0.45), ("mode", 0.5)], 2, false),
            pn!("Reverb", [("type", 1.0), ("size", 0.8), ("decay", 0.85), ("mix", 0.6), ("mod", 0.6)], 3, true),
        ],
    ),
    (
        "Ambient Swell",
        &[
            pn!("Volume", [("swell", 0.6), ("sens", 0.4)], -1, false),
            pn!("Delay", [("time", 0.65), ("feedback", 0.6), ("mix", 0.5), ("mode", 0.5), ("pingpong", 0.5)], 0, false),
            pn!("Reverb", [("type", 1.0), ("size", 0.9), ("decay", 0.9), ("mix", 0.7)], 1, true),
        ],
    ),
    (
        "Funk Wah",
        &[
            pn!("Compressor", [("sustain", 0.6)], -1, false),
            pn!("Wah", [("auto", 0.8), ("sens", 0.6), ("q", 0.6)], 0, false),
            pn!("Amp", [("gain", 0.3), ("treble", 0.55), ("channel", 0.0)], 1, false),
            pn!("Cab", [("type", 0.0)], 2, true),
        ],
    ),
    (
        "Surf Tremolo",
        &[
            pn!("Tremolo", [("rate", 0.45), ("depth", 0.7)], -1, false),
            pn!("Amp", [("gain", 0.35), ("treble", 0.6), ("bright", 0.6), ("channel", 0.0)], 0, false),
            pn!("Cab", [("type", 0.75)], 1, false),
            pn!("Reverb", [("type", 0.0), ("size", 0.6), ("decay", 0.6), ("mix", 0.45)], 2, true),
        ],
    ),
    (
        "Jazz Hollow",
        &[
            pn!("Compressor", [("sustain", 0.2)], -1, false),
            pn!("Amp", [("gain", 0.2), ("bass", 0.6), ("treble", 0.3), ("bright", 0.0), ("presence", 0.2), ("channel", 0.0)], 0, false),
            pn!("Cab", [("type", 0.0), ("mic", 0.8), ("high_cut", 0.45)], 1, false),
            pn!("EQ", [("g100", 0.55), ("g3200", 0.3)], 2, false),
            pn!("Reverb", [("type", 0.5), ("mix", 0.15)], 3, true),
        ],
    ),
    (
        "Acoustic Shimmer",
        &[
            pn!("EQ", [("g100", 0.4), ("g400", 0.45), ("g1600", 0.55), ("g3200", 0.62)], -1, false),
            pn!("Compressor", [("sustain", 0.3)], 0, false),
            pn!("Chorus", [("rate", 0.2), ("mix", 1.0)], 1, false),
            pn!("Reverb", [("type", 1.0), ("size", 0.7), ("decay", 0.65), ("mix", 0.35)], 2, true),
        ],
    ),
    (
        "Country Slap",
        &[
            pn!("Compressor", [("sustain", 0.55)], -1, false),
            pn!("Amp", [("gain", 0.35), ("bright", 0.6), ("channel", 0.0)], 0, false),
            pn!("Cab", [("type", 0.0)], 1, false),
            pn!("Delay", [("time", 0.4), ("feedback", 0.1), ("mix", 0.4), ("mode", 0.0)], 2, false),
            pn!("Reverb", [("type", 0.0), ("mix", 0.15)], 3, true),
        ],
    ),
    (
        "Bass Drive",
        &[
            pn!("Octaver", [("sub", 0.7), ("up", 0.0), ("dry", 0.5)], -1, false),
            pn!("Overdrive", [("drive", 0.5), ("tone", 0.4)], 0, true, 0.8),
            pn!("Compressor", [("sustain", 0.5)], -1, true, 0.7),
        ],
    ),
    (
        "Dual Rig Parallel",
        &[
            pn!("Amp", [("gain", 0.3), ("treble", 0.6), ("channel", 0.0)], -1, false),
            pn!("Cab", [("type", 0.0), ("mic", 0.2)], 0, true, 0.8),
            pn!("Amp", [("gain", 0.5), ("mid", 0.6), ("channel", 0.5)], -1, false),
            pn!("Cab", [("type", 0.5), ("mic", 0.5)], 2, true, 0.7),
        ],
    ),
    (
        "Space Echo",
        &[
            pn!("Amp", [("gain", 0.3), ("channel", 0.0)], -1, false),
            pn!("Cab", [("type", 0.0)], 0, false),
            pn!("Delay", [("time", 0.5), ("feedback", 0.45), ("mix", 0.5), ("mode", 0.5), ("wow", 0.4)], 1, false),
            pn!("Delay", [("time", 0.65), ("feedback", 0.3), ("mix", 0.3), ("mode", 0.5), ("pingpong", 0.6)], 2, false),
            pn!("Reverb", [("type", 0.0), ("mix", 0.3)], 3, true),
        ],
    ),
    (
        "Lo-Fi Tape",
        &[
            pn!("Vibrato", [("rate", 0.35), ("depth", 0.3), ("stereo", 0.0)], -1, false),
            pn!("EQ", [("g100", 0.4), ("g1600", 0.4), ("g3200", 0.3)], 0, false),
            pn!("Boost", [("gain", 0.35), ("grit", 0.8)], 1, false),
            pn!("Delay", [("time", 0.5), ("feedback", 0.3), ("tone", 0.35), ("mode", 1.0), ("wow", 0.6)], 2, false),
            pn!("Reverb", [("type", 0.0), ("mix", 0.2)], 3, true),
        ],
    ),
    (
        "Metal Rhythm Tight",
        &[
            pn!("Noise Gate", [("threshold", 0.6), ("release", 0.2), ("hold", 0.1)], -1, false),
            pn!("Distortion", [("distortion", 0.6), ("filter", 0.45), ("tight", 0.8), ("clip", 0.8)], 0, false),
            pn!("Amp", [("gain", 0.5), ("bass", 0.6), ("mid", 0.3), ("channel", 1.0)], 1, false),
            pn!("Cab", [("type", 0.5), ("mic", 0.1)], 2, false),
            pn!("EQ", [("g100", 0.55), ("g800", 0.35), ("g3200", 0.55)], 3, true),
        ],
    ),
    (
        "Psychedelic Phaser",
        &[
            pn!("Fuzz", [("fuzz", 0.45), ("bias", 0.4), ("mode", 0.0)], -1, false),
            pn!("Phaser", [("rate", 0.2), ("depth", 0.85), ("stages", 1.0), ("feedback", 0.6), ("mix", 1.0)], 0, false),
            pn!("Amp", [("gain", 0.4), ("channel", 0.5)], 1, false),
            pn!("Cab", [("type", 1.0)], 2, false),
            pn!("Reverb", [("type", 0.5), ("mix", 0.25)], 3, true),
        ],
    ),
    (
        "Edge Delay Rig",
        &[
            pn!("Compressor", [("sustain", 0.4)], -1, false),
            pn!("Amp", [("gain", 0.5), ("treble", 0.6), ("bright", 0.5), ("channel", 0.0)], 0, false),
            pn!("Cab", [("type", 0.25)], 1, false),
            pn!("Delay", [("time", 0.72), ("feedback", 0.42), ("mix", 0.5), ("mode", 0.0)], 2, false),
            pn!("Reverb", [("type", 0.5), ("mix", 0.2)], 3, true),
        ],
    ),
    (
        "Worship Ambient",
        &[
            pn!("Boost", [("gain", 0.25), ("treble", 0.55)], -1, false),
            pn!("Amp", [("gain", 0.35), ("channel", 0.0)], 0, false),
            pn!("Cab", [("type", 0.25)], 1, false),
            pn!("Delay", [("time", 0.7), ("feedback", 0.5), ("mix", 0.4), ("mode", 0.5), ("pingpong", 0.4)], 2, false),
            pn!("Chorus", [("mix", 1.0), ("rate", 0.2)], 3, false),
            pn!("Reverb", [("type", 1.0), ("size", 0.85), ("decay", 0.8), ("mix", 0.5)], 4, true),
        ],
    ),
];

// ---------------------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 44100.0;

    fn sine(f: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * PI * f * i as f32 / SR).sin()).collect()
    }
    fn run(rig: &mut Rig, mono: &[f32]) -> Vec<f32> {
        let mut buf: Vec<f32> = mono.iter().flat_map(|&x| [x, x]).collect();
        for c in buf.chunks_mut(512) {
            rig.process(c);
        }
        buf.chunks(2).map(|c| c[0]).collect()
    }
    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }
    fn goertzel(x: &[f32], f: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &v) in x.iter().enumerate() {
            let w = 2.0 * std::f64::consts::PI * f as f64 * i as f64 / SR as f64;
            re += v as f64 * w.cos();
            im += v as f64 * w.sin();
        }
        ((re * re + im * im).sqrt() / x.len() as f64) as f32
    }
    fn single(kind: &str) -> Rig {
        let mut r = Rig::new(SR);
        assert_eq!(r.command("add", &[], kind).len(), 1, "add {kind}");
        r
    }
    fn pluck() -> Vec<f32> {
        let mut out = vec![0.0f32; (SR * 2.0) as usize];
        let notes = [110.0f32, 146.8, 196.0, 247.0];
        for (ni, f) in notes.iter().enumerate() {
            let start = (ni as f32 * 0.5 * SR) as usize;
            for i in 0..(0.5 * SR) as usize {
                let t = i as f32 / SR;
                let mut s = 0.0;
                for h in 1..=8 {
                    s += (2.0 * PI * f * h as f32 * t).sin() / (h as f32).powf(1.2);
                }
                out[start + i] += 0.4 * s * (-t * 4.0).exp() * (1.0 - (-t * 400.0).exp());
            }
        }
        out
    }
    fn validate(r: &Rig) {
        assert!(r.nodes.len() <= MAX_NODES);
        for (i, n) in r.nodes.iter().enumerate() {
            assert!(n.input >= -1 && n.input < i as i32, "node {i} input {}", n.input);
            assert!(n.params.len() == KINDS[n.kind].1.len());
        }
    }

    #[test]
    fn tables_consistent() {
        assert_eq!(PRESETS.len(), PRESET_DEFS.len());
        for (i, p) in PRESETS.iter().enumerate() {
            assert_eq!(p.0, PRESET_DEFS[i].0);
            for (kn, ps, _, _, _) in PRESET_DEFS[i].1.iter() {
                let k = kind_index(kn).unwrap_or_else(|| panic!("kind {kn}"));
                for (pn, _) in ps.iter() {
                    assert!(KINDS[k].1.iter().any(|q| q.0 == *pn), "{} has no param {pn} (preset {})", kn, p.0);
                }
            }
        }
        let mut r = Rig::new(SR);
        assert_eq!(r.command("kinds", &[], ""), vec![KINDS.len() as f64]);
        let v: Value = serde_json::from_str(&r.info("kinds")).unwrap();
        assert_eq!(v.as_array().unwrap().len(), KINDS.len());
        let v: Value = serde_json::from_str(&r.info("presets")).unwrap();
        assert_eq!(v.as_array().unwrap().len(), PRESETS.len());
    }

    #[test]
    fn every_kind_finite_with_tail() {
        for k in KINDS.iter() {
            let mut r = single(k.0);
            let mut x = sine(220.0, 0.3, SR as usize);
            x.extend(vec![0.0; SR as usize]);
            let y = run(&mut r, &x);
            for v in &y {
                assert!(v.is_finite() && v.abs() < 4.0, "{}: {v}", k.0);
            }
        }
    }

    #[test]
    fn extremes_are_finite() {
        for k in KINDS.iter() {
            for val in [0.0f64, 1.0] {
                let mut r = single(k.0);
                for (pn, _) in k.1.iter() {
                    assert_eq!(r.command("param", &[0.0, val], pn), vec![1.0]);
                }
                let mut x = sine(110.0, 0.9, SR as usize / 2);
                x.extend(vec![0.0; SR as usize / 4]);
                for v in run(&mut r, &x) {
                    assert!(v.is_finite() && v.abs() <= 2.6, "{} @{val}: {v}", k.0);
                }
            }
        }
    }

    #[test]
    fn distortions_make_harmonics() {
        for (kind, pname) in [("Overdrive", "drive"), ("Distortion", "distortion"), ("Fuzz", "fuzz"), ("Amp", "gain")] {
            let ratio = |v: f64| {
                let mut r = single(kind);
                r.command("param", &[0.0, v], pname);
                let y = run(&mut r, &sine(220.0, 0.3, SR as usize));
                let t = &y[SR as usize / 3..];
                goertzel(t, 660.0) / goertzel(t, 220.0).max(1e-9)
            };
            let lo = ratio(0.1);
            let hi = ratio(0.9);
            assert!(hi > lo * 1.5, "{kind}: 3rd/1st lo {lo} hi {hi}");
        }
    }

    #[test]
    fn gate_and_compressor() {
        let mut seed = 12345u32;
        let noise: Vec<f32> = (0..22050)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                ((seed >> 8) as f32 / 8388608.0 - 1.0) * 0.001
            })
            .collect();
        let mut g = single("Noise Gate");
        let y = run(&mut g, &noise);
        assert!(rms(&y[11025..]) < 0.2 * rms(&noise[11025..]), "gate leaks noise");
        let mut g = single("Noise Gate");
        let loud = sine(220.0, 0.3, 22050);
        let y = run(&mut g, &loud);
        assert!(rms(&y[11025..]) > 0.8 * rms(&loud[11025..]), "gate eats signal");
        assert!(g.command("meter", &[0.0], "")[1] < 0.2);
        let mut c = single("Compressor");
        let mut x = sine(220.0, 0.5, 22050);
        x.extend(sine(220.0, 0.05, 22050));
        let y = run(&mut c, &x);
        let in_ratio = rms(&x[11025..22050]) / rms(&x[33075..]);
        let out_ratio = rms(&y[11025..22050]) / rms(&y[33075..]);
        assert!(out_ratio < in_ratio * 0.6, "in {in_ratio} out {out_ratio}");
    }

    #[test]
    fn all_presets_play() {
        let x = pluck();
        for (name, _) in PRESETS.iter() {
            let mut r = Rig::new(SR);
            assert_eq!(r.command("load_preset", &[], name), vec![1.0], "{name}");
            validate(&r);
            let y = run(&mut r, &x);
            assert!(y.iter().all(|v| v.is_finite() && v.abs() <= 2.6), "{name}");
            let dbfs = 20.0 * rms(&y).max(1e-9).log10();
            eprintln!("preset {name}: {dbfs:.1} dBFS");
            assert!(dbfs > -50.0 && dbfs < -3.0, "{name}: {dbfs} dBFS");
        }
        assert!(Rig::new(SR).command("load_preset", &[], "nope").is_empty());
    }

    #[test]
    fn parallel_lanes_sum() {
        let mut r = single("Volume");
        r.command("add", &[], "Volume");
        r.command("set_input", &[1.0, -1.0], "");
        r.command("set_out", &[0.0, 1.0], "");
        validate(&r);
        let x = sine(220.0, 0.2, 8192);
        let y = run(&mut r, &x);
        let g = rms(&y[2000..]) / rms(&x[2000..]);
        assert!((20.0 * g.log10() - 6.02).abs() < 0.5, "gain {g}");
    }

    #[test]
    fn config_round_trip() {
        let x = pluck();
        let x = &x[..(SR as usize)];
        for name in ["Clean Sparkle", "Shoegaze Wall", "Dual Rig Parallel", "Bass Drive", "Space Echo"] {
            let mut a = Rig::new(SR);
            a.command("load_preset", &[], name);
            a.set_param("input", 0.6);
            let cfg = a.info("config");
            let mut b = Rig::new(SR);
            assert_eq!(b.command("load_config", &[], &cfg), vec![1.0]);
            assert_eq!(b.info("config"), cfg, "{name}");
            let ya = run(&mut a, x);
            let yb = run(&mut b, x);
            let d = ya.iter().zip(&yb).map(|(p, q)| (p - q).abs()).fold(0.0, f32::max);
            assert!(d < 1e-5, "{name}: diff {d}");
        }
        // unknown kinds / params do not panic and are skipped, inputs fall through
        let mut r = Rig::new(SR);
        let cfg = r"[{'kind':'Boost','params':{'bogus':1,'gain':0.7}},{'kind':'Nonsense','input':0},{'kind':'Cab','input':1}]".replace('\'', "\"");
        assert_eq!(r.command("load_config", &[], &cfg), vec![1.0]);
        assert_eq!(r.command("count", &[], ""), vec![2.0]);
        assert_eq!(r.nodes[1].input, 0);
        validate(&r);
        assert!(r.command("load_config", &[], "not json").is_empty());
    }

    #[test]
    fn edit_commands_keep_graph_valid() {
        let mut r = Rig::new(SR);
        for k in ["Compressor", "Overdrive", "Amp", "Cab", "Delay"] {
            r.command("add", &[], k);
        }
        validate(&r);
        assert!(r.is_chain());
        assert_eq!(r.command("move", &[0.0, 4.0], ""), vec![1.0]);
        validate(&r);
        assert!(r.is_chain());
        assert_eq!(KINDS[r.nodes[4].kind].0, "Compressor");
        assert!(r.nodes[4].out && !r.nodes[0].out);
        assert_eq!(r.command("remove", &[2.0], ""), vec![1.0]);
        validate(&r);
        assert_eq!(r.command("count", &[], ""), vec![4.0]);
        assert!(r.command("set_input", &[1.0, 3.0], "").is_empty());
        assert!(r.command("set_input", &[0.0, 0.0], "").is_empty());
        assert_eq!(r.command("set_input", &[3.0, 0.0], ""), vec![1.0]);
        assert_eq!(r.command("param", &[0.0, 0.25], "drive"), vec![1.0]);
        assert_eq!(r.command("get_param", &[0.0], "drive"), vec![0.25]);
        assert!(r.command("param", &[0.0, 0.25], "nonsense").is_empty());
        let mut seed = 99u32;
        let mut rnd = |m: u32| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 10) % m
        };
        for _ in 0..600 {
            let n = r.nodes.len() as u32;
            match rnd(6) {
                0 => {
                    r.command("add", &[], KINDS[rnd(KINDS.len() as u32) as usize].0);
                }
                1 if n > 0 => {
                    r.command("remove", &[rnd(n) as f64], "");
                }
                2 | 3 if n > 0 => {
                    r.command("move", &[rnd(n) as f64, rnd(n) as f64], "");
                }
                4 if n > 0 => {
                    r.command("set_input", &[rnd(n) as f64, rnd(n + 1) as f64 - 1.0], "");
                }
                _ => {
                    r.command("set_out", &[rnd(n.max(1)) as f64, rnd(2) as f64], "");
                }
            }
            validate(&r);
        }
        let y = run(&mut r, &sine(220.0, 0.3, 4096));
        assert!(y.iter().all(|v| v.is_finite()));
        assert_eq!(r.command("clear", &[], ""), vec![1.0]);
        assert_eq!(r.command("count", &[], ""), vec![0.0]);
        for _ in 0..20 {
            r.command("add", &[], "Boost");
        }
        assert_eq!(r.nodes.len(), MAX_NODES);
    }

    #[test]
    fn bypass_is_transparent() {
        for k in KINDS.iter() {
            let mut r = single(k.0);
            r.command("bypass", &[0.0, 1.0], "");
            let x = sine(220.0, 0.3, 4096);
            let y = run(&mut r, &x);
            let d = x.iter().zip(&y).map(|(p, q)| (p - q).abs()).fold(0.0, f32::max);
            assert!(d < 1e-6, "{}: {d}", k.0);
        }
    }

    #[test]
    fn delay_and_reverb_ring_after_input_stops() {
        for (k, p, v) in [("Delay", "mix", 0.8), ("Reverb", "mix", 0.8)] {
            let mut r = single(k);
            r.command("param", &[0.0, v], p);
            let mut x = sine(220.0, 0.3, 4410);
            x.extend(vec![0.0; 44100]);
            let y = run(&mut r, &x);
            assert!(rms(&y[8000..30000]) > 1e-4, "{k} tail");
        }
    }
}
