//! Vocal effects.
//!
//! * **Vox Filter** - a steep high-pass / low-pass (12, 24 or 48 dB per octave, Butterworth stages) with a resonant corner, a spectral
//!   tilt and a *dynamic* high-pass that climbs when the voice is quiet, so breaths, room rumble and noise between phrases are cut
//!   while the full body of a loud phrase is kept.
//! * **Vox Reverb** - an 8-line modulated feedback-delay-network reverb built for voices: pre-delay, early reflections, decay,
//!   damping, a low-cut and high-cut *on the wet signal only*, a width control and a **duck** that pulls the reverb down while the
//!   voice sings and lets it bloom back in the gaps.
//! * **Vocal Halo** - makes a voice pop and open up in stereo: a mono-safe decorrelated "halo" of the top end (it cancels exactly in
//!   mono), an artificial double that wanders slightly like a second take, a harmonic presence exciter, an air shelf, a de-esser, a
//!   transient shaper, a bass-centred width control and a ducked bloom.
use super::fx::{Effect, Reverb};
use crate::dsp::TAU;

// ── filters ─────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, Default)]
struct Bq {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}
impl Bq {
    #[inline]
    fn tick(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
    fn set(&mut self, b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) {
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }
    fn trig(f: f64, q: f64, sr: f64) -> (f64, f64, f64) {
        let w = TAU as f64 * (f.clamp(10.0, sr * 0.49)) / sr;
        (w.cos(), w.sin(), w.sin() / (2.0 * q.max(0.1)))
    }
    fn lowpass(&mut self, f: f64, q: f64, sr: f64) {
        let (c, _, al) = Self::trig(f, q, sr);
        self.set((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + al, -2.0 * c, 1.0 - al);
    }
    fn highpass(&mut self, f: f64, q: f64, sr: f64) {
        let (c, _, al) = Self::trig(f, q, sr);
        self.set((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + al, -2.0 * c, 1.0 - al);
    }
    fn bandpass(&mut self, f: f64, q: f64, sr: f64) {
        let (c, _, al) = Self::trig(f, q, sr);
        self.set(al, 0.0, -al, 1.0 + al, -2.0 * c, 1.0 - al);
    }
    fn high_shelf(&mut self, f: f64, db: f64, sr: f64) {
        let (c, s, _) = Self::trig(f, 0.7071, sr);
        let a = 10f64.powf(db / 40.0);
        let be = 2.0 * a.sqrt() * (s * 0.70710678);
        self.set(
            a * ((a + 1.0) + (a - 1.0) * c + be),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - be),
            (a + 1.0) - (a - 1.0) * c + be,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - be,
        );
    }
    fn low_shelf(&mut self, f: f64, db: f64, sr: f64) {
        let (c, s, _) = Self::trig(f, 0.7071, sr);
        let a = 10f64.powf(db / 40.0);
        let be = 2.0 * a.sqrt() * (s * 0.70710678);
        self.set(
            a * ((a + 1.0) - (a - 1.0) * c + be),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - be),
            (a + 1.0) + (a - 1.0) * c + be,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - be,
        );
    }
    fn identity() -> Bq {
        Bq { b0: 1.0, ..Bq::default() }
    }
}

/// one-pole level follower with separate attack and release (times in seconds)
#[derive(Clone, Copy)]
struct Follow {
    y: f32,
    a: f32,
    r: f32,
}
impl Follow {
    fn new(sr: f32, attack: f32, release: f32) -> Follow {
        Follow { y: 0.0, a: 1.0 - (-1.0 / (attack * sr)).exp(), r: 1.0 - (-1.0 / (release * sr)).exp() }
    }
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let c = if x > self.y { self.a } else { self.r };
        self.y += (x - self.y) * c;
        self.y
    }
}

fn finite(x: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

// ── Vox Filter ──────────────────────────────────────────────────────────────
const BUTTER: [&[f64]; 3] = [&[0.70710678], &[0.54119610, 1.30656296], &[0.50979558, 0.60134489, 0.89997622, 2.56291545]];

pub struct VoxFilter {
    sr: f64,
    hp_hz: f64,
    lp_hz: f64,
    slope: usize, // index into BUTTER: 0 = 12 dB, 1 = 24, 2 = 48
    res: f64,
    tilt_db: f64,
    dynamic: f64,
    mix: f32,
    hp: [[Bq; 4]; 2],
    lp: [[Bq; 2]; 2],
    low: [Bq; 2],
    high: [Bq; 2],
    env: Follow,
    eff_hp: f64,
    count: usize,
    dirty: bool,
}
impl VoxFilter {
    pub fn new(sr: f32) -> Self {
        let mut f = VoxFilter {
            sr: sr as f64,
            hp_hz: 90.0,
            lp_hz: 20000.0,
            slope: 1,
            res: 0.0,
            tilt_db: 0.0,
            dynamic: 0.0,
            mix: 1.0,
            hp: [[Bq::identity(); 4]; 2],
            lp: [[Bq::identity(); 2]; 2],
            low: [Bq::identity(); 2],
            high: [Bq::identity(); 2],
            env: Follow::new(sr, 0.01, 0.25),
            eff_hp: 90.0,
            count: 0,
            dirty: true,
        };
        f.update(90.0);
        f
    }
    fn stages(&self) -> usize {
        [1, 2, 4][self.slope]
    }
    fn update(&mut self, hp_hz: f64) {
        self.eff_hp = hp_hz;
        let n = self.stages();
        let qs = BUTTER[self.slope];
        for c in 0..2 {
            for (i, q) in qs.iter().enumerate().take(n) {
                // the resonance lifts only the sharpest stage so the corner gets a peak without ringing the whole slope
                let qq = if i + 1 == n { q * (1.0 + self.res * 4.0) } else { *q };
                self.hp[c][i].highpass(hp_hz, qq, self.sr);
            }
            let lq = BUTTER[self.slope.min(1)];
            for (i, q) in lq.iter().enumerate() {
                let qq = if i + 1 == lq.len() { q * (1.0 + self.res * 2.0) } else { *q };
                self.lp[c][i].lowpass(self.lp_hz, qq, self.sr);
            }
            self.low[c].low_shelf(400.0, -self.tilt_db, self.sr);
            self.high[c].high_shelf(3000.0, self.tilt_db, self.sr);
        }
        self.dirty = false;
    }
}
impl Effect for VoxFilter {
    fn process(&mut self, b: &mut [f32]) {
        if self.dirty {
            self.update(self.hp_hz);
        }
        let n = self.stages();
        let lp_on = self.lp_hz < 17000.0;
        let lp_n = BUTTER[self.slope.min(1)].len();
        for fr in b.chunks_exact_mut(2) {
            // the dynamic high-pass follows the level: quiet -> the corner climbs up to three octaves
            let lvl = self.env.tick((fr[0].abs() + fr[1].abs()) * 0.5);
            self.count += 1;
            if self.dynamic > 0.001 && self.count % 48 == 0 {
                let db = 20.0 * (lvl.max(1e-6) as f64).log10();
                let quiet = 1.0 - ((db + 50.0) / 34.0).clamp(0.0, 1.0);
                let target = (self.hp_hz * 2f64.powf(quiet * self.dynamic * 3.0)).min(2500.0);
                if (target / self.eff_hp - 1.0).abs() > 0.01 {
                    self.update(target);
                }
            }
            let dry = [fr[0], fr[1]];
            for c in 0..2 {
                let mut x = dry[c] as f64;
                for i in 0..n {
                    x = self.hp[c][i].tick(x);
                }
                if lp_on {
                    for i in 0..lp_n {
                        x = self.lp[c][i].tick(x);
                    }
                }
                if self.tilt_db.abs() > 0.05 {
                    x = self.high[c].tick(self.low[c].tick(x));
                }
                fr[c] = finite(dry[c] * (1.0 - self.mix) + x as f32 * self.mix);
            }
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0) as f64;
        match n {
            "hp" => self.hp_hz = 20.0 * 40f64.powf(v),
            "slope" => self.slope = if v < 0.34 { 0 } else if v < 0.67 { 1 } else { 2 },
            "lp" => self.lp_hz = 1000.0 * 20f64.powf(v),
            "res" => self.res = v,
            "tilt" => self.tilt_db = (v - 0.5) * 12.0,
            "dyn" => self.dynamic = v,
            "mix" => self.mix = v as f32,
            _ => return,
        }
        self.dirty = true;
    }
    fn name(&self) -> &'static str {
        "Vox Filter"
    }
}

// ── Vox Reverb ──────────────────────────────────────────────────────────────
const NLINE: usize = 8;
const BASE: [f32; NLINE] = [1031.0, 1327.0, 1523.0, 1789.0, 2029.0, 2347.0, 2713.0, 3109.0];

struct Line {
    buf: Vec<f32>,
    w: usize,
    lp: f32,
    ph: f32,
    rate: f32,
}

pub struct VoxReverb {
    sr: f32,
    lines: Vec<Line>,
    pre: [Vec<f32>; 2],
    pre_w: usize,
    er: Vec<f32>,
    er_w: usize,
    mix: f32,
    rt60: f32,
    pre_samples: usize,
    size: f32,
    damp: f32,
    moddepth: f32,
    duck: f32,
    width: f32,
    early: f32,
    gains: [f32; NLINE],
    hp: [Bq; 2],
    lp: [Bq; 2],
    env: Follow,
    duck_g: f32,
    dirty: bool,
    lowcut: f64,
    highcut: f64,
}
impl VoxReverb {
    pub fn new(sr: f32) -> Self {
        let k = sr / 44100.0;
        let lines = (0..NLINE)
            .map(|i| Line { buf: vec![0.0; ((BASE[i] * 1.7 * k) as usize) + 64], w: 0, lp: 0.0, ph: i as f32 * 0.37, rate: 0.11 + i as f32 * 0.043 })
            .collect();
        let mut r = VoxReverb {
            sr,
            lines,
            pre: [vec![0.0; (sr * 0.26) as usize + 8], vec![0.0; (sr * 0.26) as usize + 8]],
            pre_w: 0,
            er: vec![0.0; (sr * 0.12) as usize + 8],
            er_w: 0,
            mix: 0.25,
            rt60: 1.8,
            pre_samples: (0.02 * sr) as usize,
            size: 1.0,
            damp: 0.45,
            moddepth: 0.3,
            duck: 0.35,
            width: 1.0,
            early: 0.4,
            gains: [0.8; NLINE],
            hp: [Bq::identity(); 2],
            lp: [Bq::identity(); 2],
            env: Follow::new(sr, 0.005, 0.18),
            duck_g: 1.0,
            dirty: true,
            lowcut: 200.0,
            highcut: 9000.0,
        };
        r.update();
        r
    }
    fn update(&mut self) {
        let k = self.sr / 44100.0;
        for i in 0..NLINE {
            let d = BASE[i] * k * self.size;
            self.gains[i] = 10f32.powf(-3.0 * d / self.sr / self.rt60.max(0.1)).min(0.9995);
        }
        for c in 0..2 {
            self.hp[c].highpass(self.lowcut, 0.7071, self.sr as f64);
            self.lp[c].lowpass(self.highcut, 0.7071, self.sr as f64);
        }
        self.dirty = false;
    }
}
impl Effect for VoxReverb {
    fn process(&mut self, b: &mut [f32]) {
        if self.dirty {
            self.update();
        }
        let k = self.sr / 44100.0;
        let prelen = self.pre[0].len();
        let erlen = self.er.len();
        let er_taps: [(f32, f32, usize); 6] = [(0.0073, 0.60, 0), (0.0119, 0.50, 1), (0.0171, 0.42, 0), (0.0237, 0.36, 1), (0.0313, 0.28, 0), (0.0419, 0.20, 1)];
        let damp = self.damp.clamp(0.0, 0.93);
        for fr in b.chunks_exact_mut(2) {
            let (l, r) = (fr[0], fr[1]);
            // pre-delay
            self.pre[0][self.pre_w] = l;
            self.pre[1][self.pre_w] = r;
            let rp = (self.pre_w + prelen - self.pre_samples.min(prelen - 1)) % prelen;
            let (pl, pr) = (self.pre[0][rp], self.pre[1][rp]);
            self.pre_w = (self.pre_w + 1) % prelen;
            // early reflections from the mono pre-delayed signal
            self.er[self.er_w] = (pl + pr) * 0.5;
            let (mut el, mut er_) = (0.0f32, 0.0f32);
            for (t, g, side) in er_taps {
                let d = ((t * self.size * self.sr) as usize).min(erlen - 1);
                let v = self.er[(self.er_w + erlen - d) % erlen] * g;
                if side == 0 {
                    el += v;
                } else {
                    er_ += v;
                }
            }
            self.er_w = (self.er_w + 1) % erlen;
            // late tail: eight modulated lines, Hadamard feedback
            let mut v = [0.0f32; NLINE];
            for (i, ln) in self.lines.iter_mut().enumerate() {
                let n = ln.buf.len();
                ln.ph = (ln.ph + ln.rate / self.sr).fract();
                let m = (ln.ph * TAU).sin() * self.moddepth * 5.0 * k;
                let d = (BASE[i] * k * self.size + m).clamp(8.0, (n - 4) as f32);
                let rp = ln.w as f32 + n as f32 - d;
                let i0 = rp.floor();
                let f = rp - i0;
                let a = ln.buf[i0 as usize % n];
                let bb = ln.buf[(i0 as usize + 1) % n];
                let y = a * (1.0 - f) + bb * f;
                ln.lp = y * (1.0 - damp) + ln.lp * damp;
                v[i] = ln.lp * self.gains[i];
            }
            // 8x8 Hadamard (fast butterflies), normalised
            let mut h = v;
            let mut step = 1;
            while step < NLINE {
                for base in (0..NLINE).step_by(step * 2) {
                    for j in base..base + step {
                        let (x, y) = (h[j], h[j + step]);
                        h[j] = x + y;
                        h[j + step] = x - y;
                    }
                }
                step *= 2;
            }
            let norm = 0.35355339;
            for (i, ln) in self.lines.iter_mut().enumerate() {
                let inj = if i % 2 == 0 { pl } else { pr } * if i % 4 < 2 { 0.25 } else { -0.25 };
                let nw = finite(inj + h[i] * norm);
                let n = ln.buf.len();
                ln.buf[ln.w] = nw.clamp(-8.0, 8.0);
                ln.w = (ln.w + 1) % n;
            }
            let late_l = (v[0] - v[2] + v[4] - v[6]) * 0.45;
            let late_r = (v[1] - v[3] + v[5] - v[7]) * 0.45;
            let (mut wl, mut wr) = (late_l + el * self.early, late_r + er_ * self.early);
            // the wet signal only: low cut and high cut
            wl = self.lp[0].tick(self.hp[0].tick(wl as f64)) as f32;
            wr = self.lp[1].tick(self.hp[1].tick(wr as f64)) as f32;
            // duck: the voice pushes the reverb down, the gaps let it bloom
            let lvl = self.env.tick((l.abs() + r.abs()) * 0.5);
            let target = 1.0 - self.duck * (lvl * 7.0).min(1.0) * 0.92;
            self.duck_g += (target - self.duck_g) * 0.002 * (self.sr / 44100.0).recip().min(2.0).max(0.5);
            let (m_, s_) = ((wl + wr) * 0.5, (wl - wr) * 0.5 * self.width * 2.0);
            let g = self.duck_g * self.mix * 1.4;
            fr[0] = finite(l + (m_ + s_) * g);
            fr[1] = finite(r + (m_ - s_) * g);
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "mix" => self.mix = v,
            "decay" => self.rt60 = 0.2 * 40f32.powf(v),
            "predelay" => self.pre_samples = (v * 0.2 * self.sr) as usize,
            "size" => self.size = 0.6 + v,
            "damp" => self.damp = v * 0.9,
            "lowcut" => self.lowcut = 20.0 * 50f64.powf(v as f64),
            "highcut" => self.highcut = 2000.0 * 10f64.powf(v as f64),
            "duck" => self.duck = v,
            "width" => self.width = v,
            "early" => self.early = v,
            "mod" => self.moddepth = v,
            _ => return,
        }
        self.dirty = true;
    }
    fn name(&self) -> &'static str {
        "Vox Reverb"
    }
}

// ── Vocal Halo ──────────────────────────────────────────────────────────────
struct Ring {
    buf: Vec<f32>,
    w: usize,
}
impl Ring {
    fn new(n: usize) -> Ring {
        Ring { buf: vec![0.0; n], w: 0 }
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
        let rp = self.w as f32 + n as f32 - d - 1.0;
        let i0 = rp.floor();
        let f = rp - i0;
        let i0 = i0 as usize % n;
        self.buf[i0] * (1.0 - f) + self.buf[(i0 + 1) % n] * f
    }
}

#[derive(Clone, Copy, Default)]
struct Ap1 {
    a: f32,
    x1: f32,
    y1: f32,
}
impl Ap1 {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let y = -self.a * x + self.x1 + self.a * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

pub struct VocalHalo {
    sr: f32,
    // controls
    width: f32,
    halo: f32,
    double: f32,
    presence: f32,
    air: f32,
    deess: f32,
    punch: f32,
    bloom: f32,
    mix: f32,
    // state
    side_hp: Bq,
    dbl_hp: [Bq; 2],
    dbl_lp: [Bq; 2],
    ring: Ring,
    wander: [f32; 2],
    ph: [f32; 4],
    pres_bp: Bq,
    pres_hp: Bq,
    air_shelf: Bq,
    des_bp: Bq,
    halo_hp: Bq,
    halo_ap: [Ap1; 5],
    bloom_hp: [Bq; 2],
    verb: Reverb,
    tmp: Vec<f32>,
    f_fast: Follow,
    f_slow: Follow,
    f_voice: Follow,
    f_des: Follow,
    duck_g: f32,
    count: usize,
    des_g: f32,
    punch_g: f32,
}
impl VocalHalo {
    pub fn new(sr: f32) -> Self {
        let mut verb = Reverb::new(sr);
        verb.set_param("mix", 1.0);
        verb.set_param("size", 0.55);
        verb.set_param("damp", 0.55);
        let mut h = VocalHalo {
            sr,
            width: 0.55,
            halo: 0.4,
            double: 0.3,
            presence: 0.35,
            air: 0.3,
            deess: 0.3,
            punch: 0.25,
            bloom: 0.2,
            mix: 1.0,
            side_hp: Bq::identity(),
            dbl_hp: [Bq::identity(); 2],
            dbl_lp: [Bq::identity(); 2],
            ring: Ring::new((sr * 0.08) as usize + 8),
            wander: [0.0; 2],
            ph: [0.0, 0.25, 0.5, 0.75],
            pres_bp: Bq::identity(),
            pres_hp: Bq::identity(),
            air_shelf: Bq::identity(),
            des_bp: Bq::identity(),
            halo_hp: Bq::identity(),
            halo_ap: [Ap1::default(); 5],
            bloom_hp: [Bq::identity(); 2],
            verb,
            tmp: vec![],
            f_fast: Follow::new(sr, 0.001, 0.015),
            f_slow: Follow::new(sr, 0.02, 0.15),
            f_voice: Follow::new(sr, 0.004, 0.2),
            f_des: Follow::new(sr, 0.0006, 0.04),
            duck_g: 1.0,
            count: 0,
            des_g: 1.0,
            punch_g: 1.0,
        };
        h.setup(150.0);
        h
    }
    fn setup(&mut self, focus_hz: f64) {
        let sr = self.sr as f64;
        self.side_hp.highpass(focus_hz, 0.7071, sr);
        for c in 0..2 {
            self.dbl_hp[c].highpass(180.0, 0.7071, sr);
            self.dbl_lp[c].lowpass(9500.0, 0.7071, sr);
            self.bloom_hp[c].highpass(350.0, 0.7071, sr);
        }
        self.pres_bp.bandpass(4200.0, 0.9, sr);
        self.pres_hp.highpass(3200.0, 0.7071, sr);
        self.air_shelf.high_shelf(9000.0, self.air as f64 * 6.0, sr);
        self.des_bp.bandpass(7000.0, 1.3, sr);
        self.halo_hp.highpass(4000.0, 0.7071, sr);
        for (i, a) in self.halo_ap.iter_mut().enumerate() {
            a.a = [0.62, -0.48, 0.37, -0.27, 0.19][i];
        }
    }
}
impl Effect for VocalHalo {
    fn process(&mut self, b: &mut [f32]) {
        let frames = b.len() / 2;
        // the bloom: a short ambience of the mid signal, rendered for the whole block by the inner reverb
        let bloom_on = self.bloom > 0.01;
        if bloom_on {
            self.tmp.clear();
            for fr in b.chunks_exact(2) {
                let m = (fr[0] + fr[1]) * 0.5;
                self.tmp.push(m);
                self.tmp.push(m);
            }
            let dry: Vec<f32> = self.tmp.clone();
            self.verb.process(&mut self.tmp);
            for (t, d) in self.tmp.iter_mut().zip(dry.iter()) {
                *t -= *d;
            }
        }
        let sr = self.sr;
        for i in 0..frames {
            let (l, r) = (b[i * 2], b[i * 2 + 1]);
            let m = (l + r) * 0.5;
            let mut s = (l - r) * 0.5;
            self.count += 1;
            // width: the side signal, kept out of the bass so the low end stays centred
            s = self.side_hp.tick(s as f64) as f32 * (self.width * 2.0);
            // voice level for ducking the bloom
            let lvl = self.f_voice.tick(m.abs());
            // ── de-esser: only the sibilant band is turned down, by how far it sticks out
            let hi = self.des_bp.tick(m as f64) as f32;
            let env_hi = self.f_des.tick(hi.abs());
            if self.count % 8 == 0 {
                let db = 20.0 * env_hi.max(1e-5).log10();
                let over = (db + 32.0).max(0.0);
                let red = over * self.deess * 0.75;
                self.des_g = 10f32.powf(-red / 20.0);
            }
            let mut mm = m - hi * (1.0 - self.des_g);
            // ── punch: boost the front of every note (fast level above slow level)
            let fast = self.f_fast.tick(mm.abs());
            let slow = self.f_slow.tick(mm.abs());
            let tgt = 1.0 + self.punch * ((fast - slow) / (slow + 0.02)).clamp(0.0, 1.5) * 0.8;
            self.punch_g += (tgt - self.punch_g) * 0.1;
            mm *= self.punch_g;
            // ── presence: harmonics generated from the 3-6 kHz band, added back
            let pb = self.pres_bp.tick(mm as f64) as f32;
            let ex = self.pres_hp.tick((pb * 3.0).tanh() as f64) as f32;
            mm += ex * self.presence * 0.5;
            // ── air shelf
            mm = self.air_shelf.tick(mm as f64) as f32;
            // ── halo: the top end, phase-smeared, pushed to one side and its negative to the other: nothing is left in mono
            let mut hv = self.halo_hp.tick(mm as f64) as f32;
            for a in self.halo_ap.iter_mut() {
                hv = a.tick(hv);
            }
            let halo = hv * self.halo * 0.9;
            // ── artificial double: two delayed, filtered copies that drift like a second take, one each side
            self.ring.push(mm);
            for k in 0..2 {
                self.ph[k] = (self.ph[k] + [0.31, 0.77][k] / sr).fract();
                self.ph[k + 2] = (self.ph[k + 2] + [1.13, 0.53][k] / sr).fract();
                self.wander[k] = (self.ph[k] * TAU).sin() * 0.7 + (self.ph[k + 2] * TAU).sin() * 0.3;
            }
            let da = self.ring.read((0.017 + self.wander[0] * 0.0012) * sr);
            let db_ = self.ring.read((0.027 + self.wander[1] * 0.0016) * sr);
            let dl = self.dbl_lp[0].tick(self.dbl_hp[0].tick(da as f64)) as f32 * self.double * 0.75;
            let dr = self.dbl_lp[1].tick(self.dbl_hp[1].tick(db_ as f64)) as f32 * self.double * 0.75;
            // ── bloom, ducked by the voice
            let (mut bl, mut br) = (0.0, 0.0);
            if bloom_on {
                let target = 1.0 - (lvl * 8.0).min(1.0) * 0.9;
                self.duck_g += (target - self.duck_g) * 0.003;
                bl = self.bloom_hp[0].tick(self.tmp[i * 2] as f64) as f32 * self.bloom * 2.0 * self.duck_g;
                br = self.bloom_hp[1].tick(self.tmp[i * 2 + 1] as f64) as f32 * self.bloom * 2.0 * self.duck_g;
            }
            let wl = mm + s + halo + dl + bl;
            let wr = mm - s - halo + dr + br;
            b[i * 2] = finite(l * (1.0 - self.mix) + wl * self.mix).clamp(-4.0, 4.0);
            b[i * 2 + 1] = finite(r * (1.0 - self.mix) + wr * self.mix).clamp(-4.0, 4.0);
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "width" => self.width = v,
            "halo" => self.halo = v,
            "double" => self.double = v,
            "presence" => self.presence = v,
            "air" => {
                self.air = v;
                self.air_shelf.high_shelf(9000.0, v as f64 * 6.0, self.sr as f64);
            }
            "deess" => self.deess = v,
            "punch" => self.punch = v,
            "bloom" => self.bloom = v,
            "focus" => {
                let f = 80.0 * 4f64.powf(v as f64);
                self.side_hp.highpass(f, 0.7071, self.sr as f64);
            }
            "mix" => self.mix = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Vocal Halo"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;

    fn sine(f: f32, secs: f32, amp: f32) -> Vec<f32> {
        (0..(SR * secs) as usize).flat_map(|i| {
            let v = (i as f32 / SR * f * TAU).sin() * amp;
            [v, v]
        }).collect()
    }
    fn rms(b: &[f32]) -> f32 {
        (b.iter().map(|x| x * x).sum::<f32>() / b.len().max(1) as f32).sqrt()
    }
    fn tail(b: &[f32]) -> &[f32] {
        &b[b.len() / 2..]
    }
    fn noise(secs: f32, amp: f32) -> Vec<f32> {
        let mut s = 12345u32;
        (0..(SR * secs) as usize).flat_map(|_| {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let v = ((s >> 8) as f32 / 8388608.0 - 1.0) * amp;
            [v, v]
        }).collect()
    }

    #[test]
    fn the_filter_cuts_the_low_end_steeper_with_a_steeper_slope() {
        let gain_at = |hz: f32, slope: f32| {
            let mut f = VoxFilter::new(SR);
            f.set_param("hp", 0.4);
            f.set_param("slope", slope);
            let mut x = sine(hz, 0.6, 0.5);
            f.process(&mut x);
            rms(tail(&x)) / (0.5 * 0.7071)
        };
        let (g12, g48) = (gain_at(40.0, 0.0), gain_at(40.0, 1.0));
        assert!(g12 < 0.8 && g48 < g12 * 0.35, "{g12} {g48}");
        assert!(gain_at(1000.0, 0.5) > 0.95, "the voice band passes");
        // mix 0 is the dry signal
        let mut f = VoxFilter::new(SR);
        f.set_param("mix", 0.0);
        let src = sine(60.0, 0.1, 0.5);
        let mut x = src.clone();
        f.process(&mut x);
        assert_eq!(x, src);
    }

    #[test]
    fn the_dynamic_high_pass_cleans_quiet_passages_more_than_loud_ones() {
        let at = |amp: f32, dynamic: f32| {
            let mut f = VoxFilter::new(SR);
            f.set_param("hp", 0.35);
            f.set_param("dyn", dynamic);
            let mut x = sine(250.0, 1.0, amp);
            f.process(&mut x);
            rms(tail(&x)) / (amp * 0.7071)
        };
        let quiet_static = at(0.003, 0.0);
        let quiet_dynamic = at(0.003, 1.0);
        let loud_dynamic = at(0.5, 1.0);
        assert!(quiet_dynamic < quiet_static * 0.6, "{quiet_dynamic} vs {quiet_static}");
        assert!(loud_dynamic > quiet_dynamic * 1.4, "{loud_dynamic} vs {quiet_dynamic}");
    }

    #[test]
    fn the_reverb_has_a_tail_that_follows_decay_and_a_wet_low_cut() {
        let tail_energy = |decay: f32, lowcut: f32| {
            let mut r = VoxReverb::new(SR);
            r.set_param("decay", decay);
            r.set_param("lowcut", lowcut);
            r.set_param("duck", 0.0);
            r.set_param("mix", 1.0);
            let mut x = vec![0.0f32; (SR * 2.0) as usize * 2];
            x[0] = 1.0;
            x[1] = 1.0;
            r.process(&mut x);
            assert!(x.iter().all(|v| v.is_finite() && v.abs() < 4.0));
            rms(&x[(SR * 1.0) as usize * 2..])
        };
        let (short, long) = (tail_energy(0.2, 0.2), tail_energy(0.8, 0.2));
        assert!(long > short * 3.0 && short >= 0.0, "{short} {long}");
        assert!(long > 1e-4, "the tail is audible after a second: {long}");
        // a high low-cut removes bass from the wet signal
        let wet_of = |lowcut: f32| {
            let mut r = VoxReverb::new(SR);
            r.set_param("lowcut", lowcut);
            r.set_param("duck", 0.0);
            r.set_param("mix", 1.0);
            let src = sine(60.0, 1.0, 0.3);
            let mut x = src.clone();
            r.process(&mut x);
            let diff: Vec<f32> = x.iter().zip(src.iter()).map(|(a, b)| a - b).collect();
            rms(tail(&diff))
        };
        assert!(wet_of(0.9) < wet_of(0.0) * 0.5, "the low cut takes the bass out of the wet signal: {} vs {}", wet_of(0.9), wet_of(0.0));
    }

    #[test]
    fn the_reverb_ducks_under_a_voice() {
        let wet_while_singing = |duck: f32| {
            let mut r = VoxReverb::new(SR);
            r.set_param("duck", duck);
            r.set_param("mix", 1.0);
            r.set_param("lowcut", 0.0);
            let mut x = noise(1.5, 0.4);
            let dry = x.clone();
            r.process(&mut x);
            let diff: Vec<f32> = x.iter().zip(dry.iter()).map(|(a, b)| a - b).collect();
            rms(tail(&diff))
        };
        assert!(wet_while_singing(1.0) < wet_while_singing(0.0) * 0.6);
    }

    #[test]
    fn the_halo_adds_stereo_that_cancels_in_mono_and_stays_in_bounds() {
        let mut h = VocalHalo::new(SR);
        for (n, v) in [("width", 0.5), ("halo", 1.0), ("double", 0.0), ("presence", 0.0), ("air", 0.0), ("deess", 0.0), ("punch", 0.0), ("bloom", 0.0), ("mix", 1.0)] {
            h.set_param(n, v);
        }
        let src = noise(1.0, 0.3);
        let mut x = src.clone();
        h.process(&mut x);
        // mono sum is unchanged, the sides are not
        let mono_err: f32 = x.chunks(2).zip(src.chunks(2)).map(|(a, b)| ((a[0] + a[1]) - (b[0] + b[1])).abs()).fold(0.0, f32::max);
        let side: f32 = rms(&x.chunks(2).flat_map(|a| [a[0] - a[1], 0.0]).collect::<Vec<_>>());
        assert!(mono_err < 1e-3, "halo is mono safe: {mono_err}");
        assert!(side > 0.01, "and wide: {side}");
        // everything on, loud input: finite and bounded
        let mut h = VocalHalo::new(SR);
        for n in ["width", "halo", "double", "presence", "air", "deess", "punch", "bloom"] {
            h.set_param(n, 1.0);
        }
        let mut loud = noise(1.0, 0.95);
        h.process(&mut loud);
        assert!(loud.iter().all(|v| v.is_finite() && v.abs() <= 4.0));
    }

    #[test]
    fn the_deesser_and_the_double_do_their_jobs() {
        // a bright 7 kHz tone is turned down by the de-esser, a 300 Hz tone is not
        let level = |hz: f32, deess: f32| {
            let mut h = VocalHalo::new(SR);
            for n in ["halo", "double", "presence", "air", "punch", "bloom"] {
                h.set_param(n, 0.0);
            }
            h.set_param("width", 0.5);
            h.set_param("deess", deess);
            let mut x = sine(hz, 0.8, 0.5);
            h.process(&mut x);
            rms(tail(&x))
        };
        assert!(level(7000.0, 1.0) < level(7000.0, 0.0) * 0.75);
        assert!((level(300.0, 1.0) / level(300.0, 0.0) - 1.0).abs() < 0.1);
        // the double makes left and right differ for a mono source
        let mut h = VocalHalo::new(SR);
        for n in ["halo", "presence", "air", "deess", "punch", "bloom"] {
            h.set_param(n, 0.0);
        }
        h.set_param("double", 1.0);
        let mut x = noise(0.6, 0.3);
        h.process(&mut x);
        let side = rms(&x.chunks(2).flat_map(|a| [a[0] - a[1], 0.0]).collect::<Vec<_>>());
        assert!(side > 0.02, "{side}");
    }
}
