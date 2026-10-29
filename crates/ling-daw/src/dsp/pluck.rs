//! Ling Pluck: polyphonic physical-modelling plucked / struck strings.
//!
//! Each voice is a pair of digital waveguide strings (extended Karplus-Strong): a Thiran fractional delay line, a
//! two-tap loop filter for frequency dependent damping, a 3-section allpass dispersion chain (stiffness /
//! inharmonicity) and an optional one-sided "jawari" contact nonlinearity (sitar / shamisen buzz). The two strings are
//! slightly detuned and weakly coupled at the bridge. The pluck excitation is shaped by position (comb), hardness and
//! velocity. Master section: wooden body modes, a bank of 12 sympathetic resonators tuned to a scale, soft limiter.
use super::{midi_hz, Instrument, Rng};
use std::f32::consts::{PI, TAU};

const NP: usize = 23;
const NAMES: [&str; NP] = [
    "type", "tone", "decay", "damping", "pos", "hardness", "stiffness", "detune", "buzz", "sympathetic", "body",
    "bodytype", "glide", "bend", "vibrato", "vibrate", "strum", "release", "width", "noise", "level", "scale", "root",
];
const NPRE: usize = 26;
const PRESET_NAMES: [&str; NPRE] = [
    "Nylon Guitar",
    "Steel Guitar",
    "12-String",
    "Muted Pluck",
    "Harp",
    "Concert Harp",
    "Lute",
    "Oud",
    "Banjo",
    "Koto",
    "Shamisen",
    "Sitar",
    "Tanpura Drone",
    "Santoor",
    "Kalimba",
    "Mandolin",
    "Ukulele",
    "Bass Pluck",
    "Dulcimer",
    "Zither",
    "Guzheng",
    "Celtic Harp",
    "Pizzicato",
    "Slide Guitar",
    "Dreamy Pad Pluck",
    "Hammered Bell String",
];
// type tone decay damping pos hardness stiffness detune buzz sympathetic body bodytype glide bend vibrato vibrate strum release width noise level scale root
const PRESET_DATA: [[f32; NP]; NPRE] = [
    [0.0, 0.45, 0.5, 0.5, 0.3, 0.3, 0.08, 0.3, 0.0, 0.0, 0.55, 0.0, 0.0, 0.5, 0.0, 0.4, 0.1, 0.6, 0.5, 0.05, 0.7, 0.0, 0.0],
    [0.0, 0.78, 0.6, 0.3, 0.25, 0.7, 0.12, 0.3, 0.0, 0.0, 0.5, 0.0, 0.0, 0.5, 0.0, 0.4, 0.1, 0.6, 0.5, 0.1, 0.7, 0.0, 0.0],
    [0.0, 0.85, 0.62, 0.3, 0.22, 0.75, 0.1, 0.75, 0.0, 0.1, 0.55, 0.0, 0.0, 0.5, 0.0, 0.4, 0.25, 0.5, 0.8, 0.1, 0.65, 0.0, 0.0],
    [0.0, 0.3, 0.12, 0.7, 0.15, 0.4, 0.05, 0.2, 0.0, 0.0, 0.4, 0.0, 0.0, 0.5, 0.0, 0.4, 0.0, 1.0, 0.4, 0.1, 0.8, 0.0, 0.0],
    [0.0, 0.55, 0.7, 0.3, 0.2, 0.35, 0.2, 0.15, 0.0, 0.15, 0.5, 0.4, 0.0, 0.5, 0.0, 0.4, 0.05, 0.0, 0.7, 0.0, 0.7, 0.333, 0.0],
    [0.0, 0.5, 0.88, 0.25, 0.18, 0.3, 0.32, 0.12, 0.0, 0.3, 0.6, 0.4, 0.0, 0.5, 0.0, 0.4, 0.08, 0.0, 0.85, 0.0, 0.7, 0.333, 0.0],
    [0.0, 0.5, 0.5, 0.45, 0.35, 0.4, 0.1, 0.55, 0.0, 0.05, 0.55, 0.8, 0.0, 0.5, 0.0, 0.4, 0.12, 0.5, 0.6, 0.05, 0.7, 0.0, 0.0],
    [0.0, 0.6, 0.5, 0.4, 0.2, 0.85, 0.1, 0.6, 0.0, 0.12, 0.7, 0.8, 0.12, 0.5, 0.08, 0.45, 0.15, 0.4, 0.6, 0.15, 0.7, 0.667, 0.1],
    [0.1, 0.92, 0.35, 0.3, 0.12, 0.9, 0.05, 0.2, 0.12, 0.0, 0.7, 1.0, 0.0, 0.5, 0.0, 0.4, 0.08, 0.5, 0.5, 0.25, 0.65, 0.0, 0.0],
    [0.0, 0.68, 0.55, 0.3, 0.1, 0.85, 0.15, 0.1, 0.08, 0.1, 0.5, 0.4, 0.08, 0.62, 0.0, 0.4, 0.05, 0.2, 0.65, 0.12, 0.7, 0.0, 0.0],
    [0.25, 0.88, 0.4, 0.28, 0.15, 0.92, 0.05, 0.15, 0.55, 0.0, 0.75, 0.2, 0.05, 0.4, 0.0, 0.4, 0.0, 0.6, 0.45, 0.45, 0.7, 0.0, 0.0],
    [0.0, 0.85, 0.75, 0.15, 0.12, 0.75, 0.1, 0.35, 0.75, 0.75, 0.55, 0.6, 0.35, 0.55, 0.15, 0.3, 0.1, 0.1, 0.55, 0.1, 0.65, 0.667, 0.0],
    [0.0, 0.7, 0.95, 0.08, 0.2, 0.5, 0.05, 0.4, 0.85, 0.9, 0.5, 0.6, 0.0, 0.5, 0.0, 0.4, 0.5, 0.0, 0.7, 0.05, 0.6, 1.0, 0.0],
    [0.8, 0.85, 0.7, 0.2, 0.35, 0.9, 0.25, 0.6, 0.0, 0.45, 0.4, 0.4, 0.0, 0.5, 0.0, 0.4, 0.05, 0.0, 0.85, 0.4, 0.65, 0.0, 0.0],
    [1.0, 0.5, 0.35, 0.5, 0.5, 0.3, 0.55, 0.0, 0.0, 0.0, 0.4, 0.4, 0.0, 0.5, 0.0, 0.4, 0.0, 0.2, 0.5, 0.1, 0.8, 0.0, 0.0],
    [0.0, 0.88, 0.35, 0.3, 0.2, 0.9, 0.1, 0.65, 0.0, 0.05, 0.5, 0.8, 0.0, 0.5, 0.0, 0.4, 0.05, 0.5, 0.6, 0.12, 0.65, 0.0, 0.0],
    [0.0, 0.6, 0.3, 0.45, 0.3, 0.45, 0.05, 0.35, 0.0, 0.0, 0.5, 0.0, 0.0, 0.5, 0.0, 0.4, 0.15, 0.6, 0.5, 0.05, 0.75, 0.0, 0.0],
    [0.0, 0.3, 0.5, 0.5, 0.35, 0.5, 0.15, 0.1, 0.0, 0.0, 0.5, 0.0, 0.0, 0.5, 0.0, 0.4, 0.0, 0.7, 0.3, 0.05, 0.85, 0.0, 0.0],
    [0.7, 0.8, 0.6, 0.25, 0.3, 0.8, 0.2, 0.7, 0.0, 0.3, 0.55, 0.4, 0.0, 0.5, 0.0, 0.4, 0.1, 0.0, 0.8, 0.3, 0.65, 0.333, 0.0],
    [0.0, 0.75, 0.7, 0.25, 0.2, 0.6, 0.2, 0.4, 0.0, 0.4, 0.5, 0.4, 0.0, 0.5, 0.0, 0.4, 0.1, 0.1, 0.7, 0.1, 0.65, 0.333, 0.0],
    [0.0, 0.72, 0.65, 0.25, 0.1, 0.75, 0.15, 0.15, 0.05, 0.1, 0.55, 0.4, 0.1, 0.65, 0.2, 0.45, 0.1, 0.1, 0.7, 0.1, 0.7, 0.0, 0.0],
    [0.0, 0.5, 0.65, 0.35, 0.25, 0.3, 0.12, 0.2, 0.0, 0.2, 0.5, 0.4, 0.0, 0.5, 0.0, 0.4, 0.08, 0.1, 0.7, 0.0, 0.7, 0.333, 0.0],
    [0.0, 0.55, 0.08, 0.6, 0.4, 0.5, 0.1, 0.1, 0.0, 0.0, 0.45, 0.0, 0.0, 0.5, 0.0, 0.4, 0.0, 0.9, 0.5, 0.2, 0.8, 0.0, 0.0],
    [0.0, 0.7, 0.75, 0.25, 0.3, 0.55, 0.08, 0.3, 0.0, 0.0, 0.55, 0.0, 0.55, 0.4, 0.35, 0.5, 0.0, 0.2, 0.5, 0.05, 0.7, 0.0, 0.0],
    [0.0, 0.4, 0.9, 0.55, 0.45, 0.15, 0.05, 0.8, 0.0, 0.5, 0.25, 0.4, 0.0, 0.5, 0.3, 0.25, 0.2, 0.0, 1.0, 0.0, 0.6, 0.333, 0.0],
    [1.0, 0.65, 0.8, 0.2, 0.4, 0.7, 0.75, 0.3, 0.0, 0.3, 0.35, 0.4, 0.0, 0.5, 0.0, 0.4, 0.0, 0.0, 0.7, 0.3, 0.6, 0.0, 0.0],
];

const NBODY: usize = 5;
// (Hz, Q, gain) for guitar, shamisen drum, harp soundboard, sitar gourd, oud bowl, banjo head
const BODY_MODES: [[(f32, f32, f32); NBODY]; 6] = [
    [(98.0, 6.0, 1.0), (200.0, 8.0, 0.9), (400.0, 10.0, 0.5), (1100.0, 8.0, 0.3), (2400.0, 6.0, 0.2)],
    [(180.0, 4.0, 1.0), (420.0, 5.0, 0.6), (900.0, 6.0, 0.5), (1800.0, 5.0, 0.4), (3200.0, 4.0, 0.3)],
    [(130.0, 8.0, 0.8), (260.0, 10.0, 0.9), (520.0, 10.0, 0.6), (1000.0, 8.0, 0.4), (2000.0, 6.0, 0.2)],
    [(140.0, 12.0, 1.0), (290.0, 14.0, 0.8), (580.0, 12.0, 0.6), (1250.0, 8.0, 0.4), (2700.0, 6.0, 0.3)],
    [(110.0, 5.0, 1.0), (230.0, 7.0, 0.8), (330.0, 8.0, 0.7), (700.0, 8.0, 0.5), (1500.0, 6.0, 0.3)],
    [(320.0, 3.0, 0.7), (640.0, 4.0, 1.0), (1100.0, 5.0, 0.8), (1900.0, 4.0, 0.6), (3000.0, 3.0, 0.3)],
];
const NSYM: usize = 12;
const SCALES: [[f32; NSYM]; 4] = [
    [0.0, 2.0, 4.0, 7.0, 9.0, 12.0, 14.0, 16.0, 19.0, 21.0, 24.0, 26.0],
    [0.0, 2.0, 4.0, 5.0, 7.0, 9.0, 11.0, 12.0, 14.0, 16.0, 17.0, 19.0],
    [0.0, 1.0, 4.0, 5.0, 7.0, 8.0, 11.0, 12.0, 13.0, 16.0, 17.0, 19.0],
    [0.0, 7.0, 12.0, 19.0, 24.0, 7.0, 12.0, 19.0, 0.0, 12.0, 7.0, 24.0],
];

const CTRL: u32 = 16;
const CHUNK: usize = 256;
const POOL: usize = 20;

// ---------------------------------------------------------------------------------------------------------------
// derived parameters

#[derive(Clone, Copy)]
struct Pr {
    ty: f32,
    tone: f32,
    hard: f32,
    pos: f32,
    decay_s: f32,
    damping: f32,
    stiff: f32,
    det_cents: f32,
    buzz: f32,
    vib_cents: f32,
    vib_rate: f32,
    glide_tc: f32,
    rel_t60: f32,
    noise: f32,
    width: f32,
}
impl Pr {
    fn from(v: &[f32; NP]) -> Pr {
        let g = |n: &str| v[NAMES.iter().position(|x| *x == n).unwrap_or(0)];
        let release = g("release");
        let glide = g("glide");
        Pr {
            ty: g("type"),
            tone: g("tone"),
            hard: g("hardness"),
            pos: g("pos"),
            decay_s: 0.06 * 333.0f32.powf(g("decay")),
            damping: g("damping"),
            stiff: g("stiffness"),
            det_cents: 0.1 + g("detune") * 16.0,
            buzz: g("buzz"),
            vib_cents: g("vibrato") * 60.0,
            vib_rate: 2.0 + g("vibrate") * 8.0,
            glide_tc: if glide < 0.01 { 0.0 } else { 0.01 + glide * 0.5 },
            rel_t60: if release < 0.02 { 0.0 } else { 0.03 * 400.0f32.powf(1.0 - release) },
            noise: g("noise"),
            width: g("width"),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// string

fn lp_delay(a: f32, w: f32) -> f32 {
    let b = 1.0 - a;
    -(-b * w.sin()).atan2(a + b * w.cos()) / w
}
fn ap_delay(c: f32, w: f32) -> f32 {
    let phi = (-w.sin()).atan2(c + w.cos()) - (-c * w.sin()).atan2(1.0 + c * w.cos());
    -phi / w
}

struct Str {
    buf: Vec<f32>,
    mask: usize,
    w: usize,
    n: usize,
    eta: f32,
    a: f32,
    c: f32,
    nst: usize,
    g: f32,
    dec: f32,
    u1: f32,
    y1: f32,
    yp: f32,
    s: [f32; 3],
    env: f32,
}
impl Str {
    fn new(len: usize) -> Str {
        Str {
            buf: vec![0.0; len],
            mask: len - 1,
            w: 0,
            n: 10,
            eta: 0.0,
            a: 1.0,
            c: 0.0,
            nst: 0,
            g: 0.99,
            dec: 0.99,
            u1: 0.0,
            y1: 0.0,
            yp: 0.0,
            s: [0.0; 3],
            env: 0.0,
        }
    }
    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|x| *x = 0.0);
        self.u1 = 0.0;
        self.y1 = 0.0;
        self.yp = 0.0;
        self.s = [0.0; 3];
        self.env = 0.0;
        self.w = 0;
    }
    fn set(&mut self, f: f32, sr: f32, pr: &Pr, rel: f32) {
        let f = f.clamp(20.0, sr * 0.1);
        let w = TAU * f / sr;
        let period = sr / f;
        let a = 0.5 + 0.5 * (1.0 - pr.damping).max(0.0).powf(0.7);
        let (nst, c) = if pr.stiff < 0.02 { (0, 0.0) } else { (3, 0.45 * pr.stiff.powf(1.5) / (1.0 + f / 1500.0)) };
        let mut dl = lp_delay(a, w);
        if nst > 0 {
            dl += 3.0 * ap_delay(c, w);
        }
        let d_tot = (period - dl).clamp(1.6, (self.mask - 6) as f32);
        let n = (d_tot - 0.5).floor();
        let d = d_tot - n;
        self.n = n as usize;
        self.eta = (1.0 - d) / (1.0 + d);
        self.a = a;
        self.c = c;
        self.nst = nst;
        let t60 = (pr.decay_s * (261.63 / f).powf(0.45).clamp(0.12, 4.0)).max(0.02);
        let mut g = (-6.908 * period / (t60 * sr)).exp();
        if rel > 0.0 {
            g *= (-6.908 * period / (rel * sr)).exp();
        }
        self.g = g.min(0.99995);
        self.dec = (-0.7 / period).exp();
    }
    #[inline]
    fn tick(&mut self, inject: f32, buzz: f32) -> f32 {
        let u = self.buf[self.w.wrapping_sub(self.n) & self.mask];
        let y = self.eta * (u - self.y1) + self.u1;
        self.u1 = u;
        self.y1 = y;
        let mut z = self.a * y + (1.0 - self.a) * self.yp;
        self.yp = y;
        for k in 0..self.nst {
            let o = self.c * z + self.s[k];
            self.s[k] = z - self.c * o;
            z = o;
        }
        if buzz > 0.001 {
            let e = z.abs();
            self.env = if e > self.env { e } else { self.env * self.dec };
            let th = self.env * (1.0 - 0.9 * buzz);
            if z > th {
                z = th - (z - th) * (0.6 * buzz);
            }
        }
        let o = (z * self.g).clamp(-4.0, 4.0);
        let o = if o.is_finite() { o } else { 0.0 };
        self.buf[self.w & self.mask] = o + inject;
        self.w = self.w.wrapping_add(1);
        o
    }
}

fn normalise(x: &mut [f32], rms: f32) {
    if x.is_empty() {
        return;
    }
    let m = x.iter().sum::<f32>() / x.len() as f32;
    let mut e = 0.0;
    for v in x.iter_mut() {
        *v -= m;
        e += *v * *v;
    }
    let r = (e / x.len() as f32).sqrt();
    if r > 1e-9 {
        let k = rms / r;
        x.iter_mut().for_each(|v| *v *= k);
    }
}

/// Fill `dst[..p]` with a plucked / struck displacement shape of the given rms.
fn make_burst(dst: &mut [f32], tmp: &mut [f32], p: usize, pr: &Pr, vel: f32, rng: &mut Rng, rms: f32) {
    let h = pr.hard * (0.55 + 0.45 * vel);
    let k = 0.04 + 0.96 * h * h;
    let (mut l1, mut l2) = (0.0f32, 0.0f32);
    for d in dst[..p].iter_mut() {
        l1 += k * (rng.next() - l1);
        l2 += k * (l1 - l2);
        *d = l2;
    }
    normalise(&mut dst[..p], 1.0);
    if pr.ty > 0.001 {
        let wp = (p as f32 * (0.04 + 0.2 * (1.0 - pr.hard))).max(1.5);
        for (i, t) in tmp[..p].iter_mut().enumerate() {
            let x = (i as f32 - p as f32 * 0.5) / wp;
            *t = if x.abs() < 1.0 { 0.5 * (1.0 + (PI * x).cos()) } else { 0.0 };
        }
        normalise(&mut tmp[..p], 1.0);
        for i in 0..p {
            dst[i] = (1.0 - pr.ty) * dst[i] + pr.ty * tmp[i];
        }
    }
    let m = (((0.02 + pr.pos * 0.48) * p as f32).round() as usize).clamp(1, p.saturating_sub(1).max(1));
    for i in 0..p {
        tmp[i] = dst[i] - dst[(i + p - m % p) % p];
    }
    dst[..p].copy_from_slice(&tmp[..p]);
    normalise(&mut dst[..p], rms);
}

// ---------------------------------------------------------------------------------------------------------------
// voice

struct Voice {
    s: [Str; 2],
    active: bool,
    released: bool,
    excited: bool,
    key: u8,
    order: u64,
    wait: u32,
    vel: f32,
    gl: f32,
    gr: f32,
    tgt: f32,
    cur: f32,
    bend_off: f32,
    tone_mod: f32,
    age: u32,
    ctr: u32,
    vib_ph: f32,
    tl: f32,
    tk: f32,
    dc_x: f32,
    dc_y: f32,
    env: f32,
    noise_env: f32,
    noise_dec: f32,
    po: [f32; 2],
    rng: Rng,
    /// per-note expression (targets from the engine and the smoothed pitch gesture, at control rate): pitch gesture in
    /// semitones, pressure 0..1, timbre 0..1 (0.5 neutral); `fresh` = the first value is taken at once
    gest: f32,
    gcur: f32,
    press: f32,
    timb: f32,
    fresh: bool,
}
impl Voice {
    fn new(len: usize) -> Voice {
        Voice {
            s: [Str::new(len), Str::new(len)],
            active: false,
            released: false,
            excited: false,
            key: 0,
            order: 0,
            wait: 0,
            vel: 0.0,
            gl: 1.0,
            gr: 1.0,
            tgt: 60.0,
            cur: 60.0,
            bend_off: 0.0,
            tone_mod: 0.0,
            age: 0,
            ctr: 0,
            vib_ph: 0.0,
            tl: 0.0,
            tk: 1.0,
            dc_x: 0.0,
            dc_y: 0.0,
            env: 0.0,
            noise_env: 0.0,
            noise_dec: 0.0,
            po: [0.0; 2],
            rng: Rng::new(1),
            gest: 0.0,
            gcur: 0.0,
            press: 0.0,
            timb: 0.5,
            fresh: false,
        }
    }

    fn freq_now(&self, sr: f32) -> f32 {
        midi_hz(self.cur + self.bend_off).clamp(28.0, sr * 0.1)
    }

    fn excite(&mut self, pr: &Pr, sr: f32, sa: &mut [f32], sb: &mut [f32]) {
        let len = self.s[0].buf.len();
        let f = self.freq_now(sr);
        let p = ((sr / f).round() as usize).clamp(8, len - 1);
        let velamp = 0.15 + 0.85 * self.vel;
        for si in 0..2 {
            let rms = 0.12 * velamp * if si == 0 { 1.0 } else { 0.8 };
            make_burst(sa, sb, p, pr, self.vel, &mut self.rng, rms);
            let st = &mut self.s[si];
            st.clear();
            st.buf[..p].copy_from_slice(&sa[..p]);
            st.w = p;
        }
        let nlen = (0.012 * sr).max(8.0);
        self.noise_env = if pr.noise > 0.001 { 1.0 } else { 0.0 };
        self.noise_dec = (-4.6 / nlen).exp();
        self.env = 0.0;
        self.tl = 0.0;
        self.dc_x = 0.0;
        self.dc_y = 0.0;
        self.po = [0.0; 2];
    }

    fn control(&mut self, pr: &Pr, sr: f32) {
        let dt = CTRL as f32 / sr;
        if pr.glide_tc > 0.0 {
            self.cur += (self.tgt - self.cur) * (1.0 - (-dt / pr.glide_tc).exp());
        } else {
            self.cur = self.tgt;
        }
        self.bend_off *= (-dt / 0.12).exp();
        if self.bend_off.abs() < 1e-4 {
            self.bend_off = 0.0;
        }
        let mut pitch = self.cur + self.bend_off;
        if self.gest != 0.0 || self.gcur != 0.0 {
            // the pitch gesture, smoothed at control rate (about 3 ms)
            self.gcur += (self.gest - self.gcur) * (1.0 - (-dt / 0.003).exp());
            if (self.gcur - self.gest).abs() < 1e-4 {
                self.gcur = self.gest;
            }
            pitch += self.gcur;
        }
        if pr.vib_cents > 0.01 {
            self.vib_ph += TAU * pr.vib_rate * dt;
            if self.vib_ph > TAU {
                self.vib_ph -= TAU;
            }
            let fade = ((self.age as f32 / sr - 0.2) / 0.4).clamp(0.0, 1.0);
            pitch += self.vib_ph.sin() * pr.vib_cents * 0.01 * fade;
        }
        let f = midi_hz(pitch).clamp(28.0, sr * 0.1);
        let rel = if self.released { pr.rel_t60 } else { 0.0 };
        let half = pr.det_cents / 2400.0;
        let f0 = f * (-half).exp2();
        let f1 = f * half.exp2();
        self.s[0].set(f0, sr, pr, rel);
        self.s[1].set(f1, sr, pr, rel);
        let tone = if self.press != 0.0 || self.timb != 0.5 {
            // pressure brightens the string, timbre moves its tone either way
            (pr.tone + self.tone_mod + self.press * 0.3 + (self.timb - 0.5) * 0.6).clamp(0.0, 1.0)
        } else {
            (pr.tone + self.tone_mod).clamp(0.0, 1.0)
        };
        let cut = 350.0 * 45.0f32.powf(tone) * (0.7 + 0.5 * f / 261.63);
        self.tk = (1.0 - (-TAU * cut / sr).exp()).clamp(0.001, 1.0);
    }

    fn render(&mut self, pr: &Pr, sr: f32, ml: &mut [f32], mr: &mut [f32], sa: &mut [f32], sb: &mut [f32]) {
        let coup = 0.02;
        let velamp = 0.15 + 0.85 * self.vel;
        for i in 0..ml.len() {
            if self.wait > 0 {
                self.wait -= 1;
                continue;
            }
            if !self.excited {
                self.excite(pr, sr, sa, sb);
                self.excited = true;
                self.ctr = 0;
            }
            if self.ctr == 0 {
                self.control(pr, sr);
                self.ctr = CTRL;
            }
            self.ctr -= 1;
            let mut inj = 0.0;
            if self.noise_env > 1e-4 {
                inj = self.rng.next() * 0.25 * pr.noise * velamp * self.noise_env;
                self.noise_env *= self.noise_dec;
            }
            let o0 = self.s[0].tick(inj + coup * (self.po[1] - self.po[0]), pr.buzz);
            let o1 = self.s[1].tick(inj + coup * (self.po[0] - self.po[1]), pr.buzz);
            self.po = [o0, o1];
            let mix = 0.7 * (o0 + o1);
            self.tl += self.tk * (mix - self.tl);
            let y = self.tl - self.dc_x + 0.9995 * self.dc_y;
            self.dc_x = self.tl;
            self.dc_y = y;
            if !y.is_finite() {
                self.active = false;
                return;
            }
            let a = y.abs();
            self.env = if a > self.env { a } else { self.env * 0.9995 };
            ml[i] += y * self.gl;
            mr[i] += y * self.gr;
            self.age = self.age.saturating_add(1);
        }
        if self.excited && self.env < 1e-6 && self.age > 256 {
            self.active = false;
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// master filters

#[derive(Clone, Copy, Default)]
struct Bq {
    b0: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}
impl Bq {
    fn bp(f: f32, q: f32, sr: f32) -> Bq {
        let f = f.min(sr * 0.4);
        let w = TAU * f / sr;
        let al = w.sin() / (2.0 * q);
        let a0 = 1.0 + al;
        Bq { b0: al / a0, a1: -2.0 * w.cos() / a0, a2: (1.0 - al) / a0, z1: 0.0, z2: 0.0 }
    }
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = -self.a1 * y + self.z2;
        self.z2 = -self.b0 * x - self.a2 * y;
        y
    }
}

#[derive(Clone, Copy, Default)]
struct Res {
    b0: f32,
    k1: f32,
    k2: f32,
    y1: f32,
    y2: f32,
}
impl Res {
    fn new(f: f32, t60: f32, sr: f32) -> Res {
        let f = f.min(sr * 0.4);
        let w = TAU * f / sr;
        let r = (-6.908 / (t60 * sr)).exp();
        Res { b0: (1.0 - r) * 2.0 * w.sin(), k1: 2.0 * r * w.cos(), k2: r * r, y1: 0.0, y2: 0.0 }
    }
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.k1 * self.y1 - self.k2 * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

#[inline]
fn soft_limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= 1.0 {
        x
    } else {
        let t = (a - 1.0).min(1.0e6);
        (1.0 + t / (1.0 + t)).copysign(x)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// instrument

pub struct Pluck {
    sr: f32,
    vals: [f32; NP],
    voices: Vec<Voice>,
    sa: Vec<f32>,
    sb: Vec<f32>,
    rng: Rng,
    order: u64,
    since_on: f32,
    group: u32,
    last_pitch: f32,
    last_key: u8,
    last_held: bool,
    body: [[Bq; NBODY]; 2],
    body_gain: [f32; NBODY],
    sym: [Res; NSYM],
}

impl Pluck {
    pub fn new(sr: f32) -> Pluck {
        Pluck::with_voices(sr, POOL)
    }

    fn with_voices(sr: f32, nv: usize) -> Pluck {
        let sr = sr.clamp(8000.0, 192000.0);
        let len = ((sr / 28.0 * 1.05) as usize + 16).next_power_of_two();
        let mut p = Pluck {
            sr,
            vals: PRESET_DATA[0],
            voices: (0..nv.max(1)).map(|_| Voice::new(len)).collect(),
            sa: vec![0.0; len],
            sb: vec![0.0; len],
            rng: Rng::new(0x1234_5678),
            order: 0,
            since_on: 1.0e9,
            group: 0,
            last_pitch: 60.0,
            last_key: 0,
            last_held: false,
            body: [[Bq::default(); NBODY]; 2],
            body_gain: [0.0; NBODY],
            sym: [Res::default(); NSYM],
        };
        p.rebuild();
        p
    }

    fn val(&self, name: &str) -> f32 {
        NAMES.iter().position(|n| *n == name).map(|i| self.vals[i]).unwrap_or(0.0)
    }

    fn rebuild(&mut self) {
        let bt = ((self.val("bodytype") * 5.0).round() as usize).min(5);
        for (k, &(f, q, g)) in BODY_MODES[bt].iter().enumerate() {
            self.body[0][k] = Bq::bp(f * 0.975, q, self.sr);
            self.body[1][k] = Bq::bp(f * 1.025, q, self.sr);
            self.body_gain[k] = g;
        }
        let sc = ((self.val("scale") * 3.0).round() as usize).min(3);
        let root = 48.0 + (self.val("root") * 11.0).round();
        for k in 0..NSYM {
            let f = midi_hz(root + SCALES[sc][k]);
            self.sym[k] = Res::new(f, 3.0, self.sr);
        }
    }

    #[cfg(test)]
    fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count()
    }

    fn alloc(&mut self) -> usize {
        if let Some(i) = self.voices.iter().position(|v| !v.active) {
            return i;
        }
        let mut best = 0;
        let mut score = (true, u64::MAX);
        for (i, v) in self.voices.iter().enumerate() {
            let sc = (!v.released, v.order);
            if sc < score {
                score = sc;
                best = i;
            }
        }
        best
    }
}

impl Instrument for Pluck {
    fn note_on(&mut self, key: u8, vel: f32) {
        self.note_on_ex(key, vel, 0.0, 0.5, 0.5);
    }

    fn note_on_ex(&mut self, key: u8, vel: f32, pan: f32, cutoff: f32, _reso: f32) {
        if vel <= 0.0 {
            self.note_off(key);
            return;
        }
        let pr = Pr::from(&self.vals);
        for v in self.voices.iter_mut() {
            if v.active && v.key == key && !v.released {
                v.released = true;
            }
        }
        let chord = self.since_on < 0.015 * self.sr;
        self.group = if chord { self.group + 1 } else { 0 };
        self.since_on = 0.0;
        let strum = self.val("strum");
        let gap = strum * strum * 0.045 * self.sr;
        let wait = (self.group as f32 * gap) as u32;
        let tgt = key as f32;
        let mut cur = tgt;
        if pr.glide_tc > 0.0 && self.last_held && !chord {
            cur = self.last_pitch;
        }
        let bend = self.val("bend");
        let bend_off = -(bend - 0.5) * 8.0;
        let seed = (self.rng.next().to_bits()) ^ (self.order as u32).wrapping_mul(2654435761);
        let i = self.alloc();
        self.order += 1;
        let order = self.order;
        let pn = (((key as f32 - 60.0) / 30.0).clamp(-1.0, 1.0) * pr.width * 0.7 + pan * 0.8).clamp(-1.0, 1.0);
        let ang = (pn + 1.0) * PI / 4.0;
        let v = &mut self.voices[i];
        v.active = true;
        v.released = false;
        v.excited = false;
        v.key = key;
        v.order = order;
        v.wait = wait;
        v.vel = vel.clamp(0.0, 1.0);
        v.tgt = tgt;
        v.cur = cur;
        v.bend_off = bend_off;
        v.tone_mod = (cutoff - 0.5) * 0.6;
        v.age = 0;
        v.ctr = 0;
        v.vib_ph = 0.0;
        v.env = 1.0;
        v.gl = ang.cos() * std::f32::consts::SQRT_2;
        v.gr = ang.sin() * std::f32::consts::SQRT_2;
        v.rng = Rng::new(seed);
        v.gest = 0.0;
        v.gcur = 0.0;
        v.press = 0.0;
        v.timb = 0.5;
        v.fresh = true;
        self.last_pitch = tgt;
        self.last_key = key;
        self.last_held = true;
    }

    fn note_off(&mut self, key: u8) {
        for v in self.voices.iter_mut() {
            if v.active && v.key == key {
                v.released = true;
            }
        }
        if self.last_key == key {
            self.last_held = false;
        }
    }

    fn all_off(&mut self) {
        for v in self.voices.iter_mut() {
            v.active = false;
            v.released = true;
        }
        self.last_held = false;
    }

    fn note_pitch(&mut self, key: u8, semis: f32) {
        for v in self.voices.iter_mut().filter(|v| v.active && !v.released && v.key == key) {
            v.gest = semis;
            if v.fresh {
                v.gcur = semis;
            }
        }
    }
    fn note_pressure(&mut self, key: u8, x: f32) {
        for v in self.voices.iter_mut().filter(|v| v.active && !v.released && v.key == key) {
            v.press = x;
        }
    }
    fn note_timbre(&mut self, key: u8, x: f32) {
        for v in self.voices.iter_mut().filter(|v| v.active && !v.released && v.key == key) {
            v.timb = x;
        }
    }

    fn render(&mut self, out: &mut [f32]) {
        let pr = Pr::from(&self.vals);
        let sr = self.sr;
        let body_amt = self.val("body") * 1.5;
        let sym_amt = self.val("sympathetic") * 8.0;
        let gain = self.val("level") * 2.0;
        let mut ml = [0.0f32; CHUNK];
        let mut mr = [0.0f32; CHUNK];
        for v in self.voices.iter_mut() {
            v.fresh = false;
        }
        for chunk in out.chunks_mut(CHUNK * 2) {
            let n = chunk.len() / 2;
            ml[..n].iter_mut().for_each(|x| *x = 0.0);
            mr[..n].iter_mut().for_each(|x| *x = 0.0);
            for v in self.voices.iter_mut() {
                if v.active {
                    v.render(&pr, sr, &mut ml[..n], &mut mr[..n], &mut self.sa, &mut self.sb);
                }
            }
            let mut bad = false;
            for i in 0..n {
                let (l, r) = (ml[i], mr[i]);
                let (mut wl, mut wr) = (0.0, 0.0);
                if body_amt > 0.001 {
                    for k in 0..NBODY {
                        wl += self.body_gain[k] * self.body[0][k].run(l);
                        wr += self.body_gain[k] * self.body[1][k].run(r);
                    }
                }
                let mut s = 0.0;
                if sym_amt > 0.001 {
                    let x = 0.5 * (l + r);
                    for k in 0..NSYM {
                        s += self.sym[k].run(x);
                    }
                }
                let mut ol = (l + body_amt * wl + sym_amt * s * 0.5) * gain;
                let mut or = (r + body_amt * wr + sym_amt * s * 0.5) * gain;
                if !ol.is_finite() || !or.is_finite() {
                    ol = 0.0;
                    or = 0.0;
                    bad = true;
                }
                chunk[2 * i] += soft_limit(ol);
                chunk[2 * i + 1] += soft_limit(or);
            }
            if bad {
                for b in self.body.iter_mut().flatten() {
                    b.z1 = 0.0;
                    b.z2 = 0.0;
                }
                for r in self.sym.iter_mut() {
                    r.y1 = 0.0;
                    r.y2 = 0.0;
                }
            }
            self.since_on = (self.since_on + n as f32).min(1.0e9);
        }
    }

    fn params(&self) -> Vec<(&'static str, f32)> {
        NAMES.iter().copied().zip(self.vals.iter().copied()).collect()
    }

    fn set_param(&mut self, name: &str, v: f32) {
        if !v.is_finite() {
            return;
        }
        if let Some(i) = NAMES.iter().position(|n| *n == name) {
            self.vals[i] = v.clamp(0.0, 1.0);
            if name == "bodytype" || name == "scale" || name == "root" {
                self.rebuild();
            }
        }
    }

    fn name(&self) -> &'static str {
        "Ling Pluck"
    }

    fn presets(&self) -> &'static [&'static str] {
        &PRESET_NAMES
    }

    fn load_preset(&mut self, name: &str) {
        if let Some(i) = PRESET_NAMES.iter().position(|n| n.eq_ignore_ascii_case(name)) {
            self.vals = PRESET_DATA[i];
            self.rebuild();
        }
    }

    fn randomize(&mut self, seed: u32) -> bool {
        let mut r = Rng::new(seed.wrapping_mul(2654435761).wrapping_add(0x9e37_79b9));
        for _ in 0..4 {
            r.next();
        }
        let u = |r: &mut Rng| (r.next() + 1.0) * 0.5;
        let base = ((u(&mut r) * NPRE as f32) as usize).min(NPRE - 1);
        let mut v = PRESET_DATA[base];
        for (i, x) in v.iter_mut().enumerate() {
            let name = NAMES[i];
            if name == "level" || name == "root" {
                continue;
            }
            if name == "bodytype" || name == "scale" {
                if u(&mut r) < 0.4 {
                    let steps = if name == "bodytype" { 5.0 } else { 3.0 };
                    *x = (u(&mut r) * (steps + 0.99)).floor().min(steps) / steps;
                }
            } else {
                *x = (*x + r.next() * 0.25).clamp(0.0, 1.0);
            }
        }
        let lim = |n: &str, lo: f32, hi: f32, v: &mut [f32; NP]| {
            let i = NAMES.iter().position(|x| *x == n).unwrap();
            v[i] = v[i].clamp(lo, hi);
        };
        lim("decay", 0.1, 0.95, &mut v);
        lim("tone", 0.2, 0.95, &mut v);
        lim("stiffness", 0.0, 0.6, &mut v);
        lim("buzz", 0.0, 0.9, &mut v);
        lim("glide", 0.0, 0.6, &mut v);
        lim("strum", 0.0, 0.6, &mut v);
        self.vals = v;
        self.rebuild();
        true
    }

    fn plot(&self, what: &str, n: usize) -> Vec<f32> {
        if what != "wave" || n == 0 {
            return vec![];
        }
        let mut p = Pluck::with_voices(self.sr, 1);
        p.vals = self.vals;
        p.set_param("strum", 0.0);
        p.set_param("glide", 0.0);
        p.set_param("bend", 0.5);
        p.set_param("vibrato", 0.0);
        p.rebuild();
        p.note_on(60, 0.8);
        let start = (0.06 * self.sr) as usize;
        let per = (self.sr / 261.63).ceil() as usize + 2;
        let mut buf = vec![0.0f32; (start + per) * 2];
        p.render(&mut buf);
        let seg: Vec<f32> = (0..per).map(|i| buf[(start + i) * 2]).collect();
        let peak = seg.iter().fold(1e-9f32, |m, x| m.max(x.abs()));
        let plen = self.sr / 261.63;
        (0..n)
            .map(|i| {
                let pos = i as f32 / n as f32 * plen;
                let i0 = pos.floor() as usize;
                let fr = pos - i0 as f32;
                let a = seg[i0.min(per - 1)];
                let b = seg[(i0 + 1).min(per - 1)];
                (a + (b - a) * fr) / peak * 0.9
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;

    fn render_mono(p: &mut Pluck, frames: usize) -> Vec<f32> {
        let mut buf = vec![0.0f32; frames * 2];
        for c in buf.chunks_mut(1024) {
            p.render(c);
        }
        buf.chunks(2).map(|f| f[0]).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    fn est_f0(x: &[f32], sr: f32, expect: f32) -> f32 {
        let lo = (sr / expect * 0.8) as usize;
        let hi = (sr / expect * 1.25) as usize;
        let n = x.len() - hi - 1;
        let r = |lag: usize| -> f32 { (0..n).map(|i| x[i] * x[i + lag]).sum::<f32>() };
        let mut best = lo;
        let mut bv = f32::MIN;
        for lag in lo..=hi {
            let v = r(lag);
            if v > bv {
                bv = v;
                best = lag;
            }
        }
        let (a, b, c) = (r(best - 1), r(best), r(best + 1));
        let den = a - 2.0 * b + c;
        let off = if den.abs() > 1e-12 { 0.5 * (a - c) / den } else { 0.0 };
        sr / (best as f32 + off)
    }

    #[test]
    fn pitch_accuracy() {
        for key in [40u8, 57, 69, 81] {
            let mut p = Pluck::new(SR);
            p.note_on(key, 0.8);
            let x = render_mono(&mut p, 24000);
            let seg = &x[2400..2400 + 9600];
            let expect = midi_hz(key as f32);
            let f = est_f0(seg, SR, expect);
            let cents = 1200.0 * (f / expect).log2();
            assert!(cents.abs() < 12.0, "key {key}: {f} Hz vs {expect} ({cents} cents)");
        }
    }

    #[test]
    fn decay_and_release() {
        let mut p = Pluck::new(SR);
        p.note_on(57, 0.8);
        let x = render_mono(&mut p, (SR * 2.2) as usize);
        let early = rms(&x[(0.1 * SR) as usize..(0.2 * SR) as usize]);
        let late = rms(&x[(2.0 * SR) as usize..(2.1 * SR) as usize]);
        assert!(early > 1e-3 && late < early, "early {early} late {late}");
        let tail = |rel: f32| {
            let mut p = Pluck::new(SR);
            p.set_param("release", rel);
            p.note_on(57, 0.8);
            render_mono(&mut p, (0.3 * SR) as usize);
            p.note_off(57);
            let x = render_mono(&mut p, (1.0 * SR) as usize);
            rms(&x[(0.9 * SR) as usize..])
        };
        let ring = tail(0.0);
        let damped = tail(1.0);
        assert!(damped < ring * 0.5, "ring {ring} damped {damped}");
    }

    #[test]
    fn all_presets_finite_and_bounded() {
        let mut p = Pluck::new(SR);
        assert_eq!(p.presets().len(), PRESET_DATA.len());
        for name in p.presets() {
            p.load_preset(name);
            for i in 0..12 {
                p.note_on(36 + i * 5, 1.0);
            }
            let x = render_mono(&mut p, 12000);
            assert!(x.iter().all(|v| v.is_finite() && v.abs() < 4.0), "preset {name}");
            assert!(rms(&x) > 1e-4, "preset {name} silent");
            p.all_off();
        }
    }

    #[test]
    fn extremes_finite_and_bounded() {
        for &sr in &[22050.0f32, 48000.0, 96000.0] {
            for val in [0.0f32, 1.0, 0.5] {
                let mut p = Pluck::new(sr);
                let names: Vec<_> = p.params().iter().map(|(n, _)| *n).collect();
                for n in names {
                    p.set_param(n, val);
                }
                for i in 0..12 {
                    p.note_on(24 + i * 6, 1.0);
                }
                let x = render_mono(&mut p, (sr * 0.4) as usize);
                assert!(x.iter().all(|v| v.is_finite() && v.abs() < 4.0), "sr {sr} val {val}");
            }
        }
        let mut p = Pluck::new(SR);
        for seed in 0..40 {
            assert!(p.randomize(seed));
            for i in 0..8 {
                p.note_on(30 + i * 8, 1.0);
            }
            let x = render_mono(&mut p, 6000);
            assert!(x.iter().all(|v| v.is_finite() && v.abs() < 4.0), "seed {seed}");
        }
    }

    fn hf_ratio(name: &str) -> f32 {
        let mut p = Pluck::new(SR);
        p.load_preset(name);
        p.note_on(57, 0.8);
        let x = render_mono(&mut p, 48000);
        let d: f32 = x.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
        let e: f32 = x.iter().map(|v| v * v).sum();
        d / e.max(1e-12)
    }

    #[test]
    fn sitar_brighter_than_nylon() {
        let s = hf_ratio("Sitar");
        let n = hf_ratio("Nylon Guitar");
        assert!(s > n * 1.5, "sitar {s} nylon {n}");
    }

    #[test]
    fn buzz_adds_highs() {
        let run = |b: f32| {
            let mut p = Pluck::new(SR);
            p.set_param("buzz", b);
            p.set_param("tone", 1.0);
            p.set_param("damping", 0.1);
            p.set_param("body", 0.0);
            p.note_on(57, 0.8);
            let x = render_mono(&mut p, 48000);
            let d: f32 = x.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
            d / x.iter().map(|v| v * v).sum::<f32>().max(1e-12)
        };
        let (a, b) = (run(0.9), run(0.0)); assert!(a > b, "buzz {a} plain {b}");
    }

    #[test]
    fn deterministic() {
        let run = || {
            let mut p = Pluck::new(SR);
            p.load_preset("Shamisen");
            p.note_on(52, 0.7);
            p.note_on(59, 0.9);
            let mut a = render_mono(&mut p, 5000);
            p.note_off(52);
            p.note_on(64, 0.5);
            a.extend(render_mono(&mut p, 5000));
            a
        };
        let (a, b) = (run(), run());
        assert!(a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()));
        let mut p = Pluck::new(SR);
        let mut q = Pluck::new(SR);
        p.randomize(7);
        q.randomize(7);
        assert_eq!(p.params(), q.params());
    }

    #[test]
    fn polyphony_and_stealing() {
        let mut p = Pluck::new(SR);
        for i in 0..20 {
            p.note_on(50 + i, 0.8);
        }
        assert!(p.active_voices() <= p.voices.len());
        for i in 20..60 {
            p.note_on(40 + (i % 40) as u8, 0.8);
        }
        assert_eq!(p.active_voices(), p.voices.len());
        let x = render_mono(&mut p, 4800);
        assert!(x.iter().all(|v| v.is_finite()));
        assert!(rms(&x) > 1e-4);
    }

    #[test]
    fn plot_and_strum() {
        let mut p = Pluck::new(SR);
        let w = p.plot("wave", 64);
        assert_eq!(w.len(), 64);
        assert!(w.iter().all(|v| v.is_finite()));
        p.set_param("strum", 1.0);
        p.note_on(50, 0.8);
        p.note_on(54, 0.8);
        p.note_on(57, 0.8);
        let x = render_mono(&mut p, 12000);
        assert!(rms(&x) > 1e-4 && x.iter().all(|v| v.is_finite()));
    }
}
