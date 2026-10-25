//! Sound-design effects for modern bass music (and anything else):
//!
//! * **OTT** - three-band upward + downward compression (88 Hz / 2.5 kHz Linkwitz-Riley crossovers): quiet detail comes up, loud
//!   peaks go down, every band glued to the same level. The "everything speaks at once" sheen of a modern growl.
//! * **Split Drive** - multiband distortion: the lows below the split stay clean (and optionally mono) while the mids and highs are
//!   driven (tube / fold / clip / crush), so a bass gets dirt without losing its sub.
//! * **Comb** - a stereo feedback comb tuned to a note: positive feedback rings at the note and its harmonics (metallic), negative at
//!   the odd harmonics an octave up (hollow); an LFO sweeps it. The spectral-comb bass sound.
//! * **Phaser** - 4 to 12 modulated all-pass stages with feedback.
//! * **Flanger** - a short modulated delay with feedback (jet sweeps, metallic swirls).
//! * **Freq Shifter** - a Hilbert-transform frequency shifter: every partial moves by the same number of Hz (inharmonic, metallic,
//!   the "talking robot" shimmer); the right channel can shift the other way.
//! * **Imager** - stereo width by mid / side, with everything below a frequency folded to mono (a bass's sub stays centred).
//! * **Transient** - transient shaper: more or less attack and sustain, from the difference of a fast and a slow envelope.
//!
//! All parameters are 0..1 like every other effect; every effect is finite for any input and settings (tests below).
use super::fx::Effect;
use crate::dsp::TAU;

// ── shared parts ────────────────────────────────────────────────────────────────

/// a TPT state-variable filter (Zavalishin): low, band and high pass from one structure, stable under modulation
#[derive(Clone, Copy, Default)]
pub struct Tsvf {
    ic1: f32,
    ic2: f32,
}
impl Tsvf {
    /// (low, band, high) for g = tan(pi fc / sr), k = 1 / Q
    #[inline]
    pub fn run(&mut self, x: f32, g: f32, k: f32) -> (f32, f32, f32) {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        if !(self.ic1.is_finite() && self.ic2.is_finite()) {
            self.ic1 = 0.0;
            self.ic2 = 0.0;
        }
        (v2, v1, x - k * v1 - v2)
    }
}

fn g_of(fc: f32, sr: f32) -> f32 {
    (std::f32::consts::PI * fc.clamp(10.0, sr * 0.45) / sr).tan()
}

/// a Linkwitz-Riley 4th-order split (two Butterworth sections per side): low + high sum to an all-pass
#[derive(Clone, Copy, Default)]
struct Lr4 {
    l1: Tsvf,
    l2: Tsvf,
    h1: Tsvf,
    h2: Tsvf,
}
impl Lr4 {
    #[inline]
    fn split(&mut self, x: f32, g: f32) -> (f32, f32) {
        const K: f32 = std::f32::consts::SQRT_2;
        let lo = self.l2.run(self.l1.run(x, g, K).0, g, K).0;
        let hi = self.h2.run(self.h1.run(x, g, K).2, g, K).2;
        (lo, hi)
    }
}

/// a 90-degree phase-difference network (two chains of 4 all-passes, Niemitalo's coefficients): (in-phase, quadrature)
#[derive(Clone, Copy, Default)]
pub struct Hilbert {
    a: [[f32; 2]; 4],
    ay: [[f32; 2]; 4],
    b: [[f32; 2]; 4],
    by: [[f32; 2]; 4],
    bd: f32,
}
const HA: [f32; 4] = [0.693_387_8 * 0.693_387_8, 0.936_065_4 * 0.936_065_4, 0.988_229_5 * 0.988_229_5, 0.998_748_8 * 0.998_748_8];
const HB: [f32; 4] = [0.402_192_1 * 0.402_192_1, 0.856_171_1 * 0.856_171_1, 0.972_290_9 * 0.972_290_9, 0.995_288_5 * 0.995_288_5];
impl Hilbert {
    #[inline]
    pub fn run(&mut self, x: f32) -> (f32, f32) {
        let mut u = x;
        for k in 0..4 {
            let y = HA[k] * (u + self.ay[k][1]) - self.a[k][1];
            self.a[k][1] = self.a[k][0];
            self.a[k][0] = u;
            self.ay[k][1] = self.ay[k][0];
            self.ay[k][0] = y;
            u = y;
        }
        let mut v = x;
        for k in 0..4 {
            let y = HB[k] * (v + self.by[k][1]) - self.b[k][1];
            self.b[k][1] = self.b[k][0];
            self.b[k][0] = v;
            self.by[k][1] = self.by[k][0];
            self.by[k][0] = y;
            v = y;
        }
        // the first path is one sample late
        let i = self.bd;
        self.bd = u;
        if !(i.is_finite() && v.is_finite()) {
            *self = Hilbert::default();
            return (0.0, 0.0);
        }
        (i, v)
    }
}

/// a fractional delay line
struct Line {
    buf: Vec<f32>,
    w: usize,
}
impl Line {
    fn new(n: usize) -> Line {
        Line { buf: vec![0.0; n.max(16)], w: 0 }
    }
    #[inline]
    fn push(&mut self, x: f32) {
        self.buf[self.w] = if x.is_finite() { x } else { 0.0 };
        self.w = (self.w + 1) % self.buf.len();
    }
    #[inline]
    fn read(&self, d: f32) -> f32 {
        let n = self.buf.len();
        let d = d.clamp(1.0, (n - 3) as f32);
        let p = self.w as f32 - d + n as f32;
        let i = p as usize;
        let f = p - p.floor();
        // cubic (Hermite) read: a comb tuned to a high note stays bright and in tune
        let y0 = self.buf[(i + n - 1) % n];
        let y1 = self.buf[i % n];
        let y2 = self.buf[(i + 1) % n];
        let y3 = self.buf[(i + 2) % n];
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        ((c3 * f + c2) * f + c1) * f + y1
    }
}

#[inline]
fn db2lin(d: f32) -> f32 {
    10f32.powf(d / 20.0)
}
#[inline]
fn lin2db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

/// a quick sanity net: a non-finite sample becomes silence
#[inline]
fn fin(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(-8.0, 8.0)
    } else {
        0.0
    }
}

// ── OTT ─────────────────────────────────────────────────────────────────────────

pub const OTT_PARAMS: [(&str, f32); 9] = [("depth", 0.6), ("time", 0.3), ("upward", 0.6), ("downward", 0.8), ("low", 0.5), ("mid", 0.5), ("high", 0.5), ("in", 0.5), ("out", 0.5)];

pub struct Ott {
    sr: f32,
    x1: [Lr4; 2],
    x2: [Lr4; 2],
    ap_low: [Lr4; 2],
    env: [f32; 3],
    /// a fast peak follower per band: the upward gain drops the moment a note starts, so there is no spike out of silence
    fast: [f32; 3],
    gain_db: [f32; 3],
    depth: f32,
    time: f32,
    up: f32,
    down: f32,
    band: [f32; 3],
    pre: f32,
    post: f32,
}
impl Ott {
    pub fn new(sr: f32) -> Ott {
        Ott { sr, x1: Default::default(), x2: Default::default(), ap_low: Default::default(), env: [0.0; 3], fast: [0.0; 3], gain_db: [0.0; 3], depth: 0.6, time: 0.3, up: 0.6, down: 0.8, band: [0.0; 3], pre: 0.0, post: 0.0 }
    }
}
/// per band: downward threshold (dBFS), attack and release at time = 1 (seconds)
const OTT_BAND: [(f32, f32, f32); 3] = [(-28.0, 0.048, 0.28), (-30.0, 0.022, 0.28), (-34.0, 0.0135, 0.13)];
impl Effect for Ott {
    fn process(&mut self, buf: &mut [f32]) {
        let (g1, g2) = (g_of(88.0, self.sr), g_of(2500.0, self.sr));
        let pre = db2lin(self.pre);
        let post = db2lin(self.post);
        let tscale = 0.05 + self.time * 1.95;
        let coef: Vec<(f32, f32)> = OTT_BAND.iter().map(|b| ((-1.0 / (b.1 * tscale * self.sr)).exp(), (-1.0 / (b.2 * tscale * self.sr)).exp())).collect();
        let bgain = [db2lin(self.band[0]), db2lin(self.band[1]), db2lin(self.band[2])];
        let kfa = (-1.0 / (0.0005 * self.sr)).exp();
        for fr in buf.chunks_exact_mut(2) {
            let mut bands = [[0.0f32; 2]; 3];
            for c in 0..2 {
                let x = fin(fr[c]) * pre;
                let (lo, rest) = self.x1[c].split(x, g1);
                let (mid, hi) = self.x2[c].split(rest, g2);
                // the low band through the same all-pass the others went through, so the bands sum flat
                let (a, b) = self.ap_low[c].split(lo, g2);
                bands[0][c] = a + b;
                bands[1][c] = mid;
                bands[2][c] = hi;
            }
            for b in 0..3 {
                let lvl = bands[b][0].abs().max(bands[b][1].abs());
                let (ka, kr) = coef[b];
                let k = if lvl > self.env[b] { ka } else { kr };
                self.env[b] = lvl + (self.env[b] - lvl) * k;
                self.fast[b] = lvl + (self.fast[b] - lvl) * if lvl > self.fast[b] { kfa } else { kr };
                let d = lin2db(self.env[b]);
                let du = lin2db(self.env[b].max(self.fast[b]));
                let t = OTT_BAND[b].0;
                let mut g = 0.0;
                if d > t {
                    g -= (d - t) * (1.0 - 1.0 / 66.0) * self.down;
                } else {
                    // upward, faded out near silence so the noise floor is not pulled up
                    let floor = ((du + 78.0) / 18.0).clamp(0.0, 1.0);
                    g += ((t - du).max(0.0) * (1.0 - 1.0 / 4.0) * self.up).min(36.0) * floor;
                }
                self.gain_db[b] = g * self.depth;
            }
            for c in 0..2 {
                let mut y = 0.0;
                for b in 0..3 {
                    y += bands[b][c] * db2lin(self.gain_db[b]) * bgain[b];
                }
                fr[c] = fin(y * post);
            }
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "depth" => self.depth = v,
            "time" => self.time = v,
            "upward" => self.up = v,
            "downward" => self.down = v,
            "low" => self.band[0] = (v - 0.5) * 24.0,
            "mid" => self.band[1] = (v - 0.5) * 24.0,
            "high" => self.band[2] = (v - 0.5) * 24.0,
            "in" => self.pre = (v - 0.5) * 24.0,
            "out" => self.post = (v - 0.5) * 24.0,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "OTT"
    }
    fn view(&self) -> Vec<f32> {
        self.gain_db.iter().map(|g| ((g + 24.0) / 48.0).clamp(0.0, 1.0)).collect()
    }
}

// ── Split Drive ────────────────────────────────────────────────────────────────

pub const SPLIT_PARAMS: [(&str, f32); 8] = [("split", 0.3), ("drive", 0.5), ("mode", 0.0), ("tone", 0.7), ("mix", 1.0), ("low", 0.5), ("mono low", 1.0), ("out", 0.5)];

pub struct SplitDrive {
    sr: f32,
    x: [Lr4; 2],
    tone: [Tsvf; 2],
    os: [(f32, f32); 2],
    crush: [(f32, f32); 2],
    dc: [(f32, f32); 2],
    split: f32,
    drive: f32,
    mode: usize,
    tone_v: f32,
    mix: f32,
    low: f32,
    mono_low: bool,
    out: f32,
}
impl SplitDrive {
    pub fn new(sr: f32) -> SplitDrive {
        SplitDrive { sr, x: Default::default(), tone: Default::default(), os: [(0.0, 0.0); 2], crush: [(0.0, 0.0); 2], dc: [(0.0, 0.0); 2], split: 150.0, drive: 0.5, mode: 0, tone_v: 0.7, mix: 1.0, low: 0.0, mono_low: true, out: 0.0 }
    }
    #[inline]
    fn shape(mode: usize, y: f32) -> f32 {
        match mode {
            // tube: asymmetric soft clip (even harmonics)
            0 => {
                let z = y + 0.25;
                (z.tanh() - 0.25f32.tanh()) * 0.9
            }
            // fold: sine wavefolder
            1 => (y * std::f32::consts::FRAC_PI_2).sin(),
            // hard clip
            2 => y.clamp(-1.0, 1.0),
            _ => (y * 1.2).tanh(),
        }
    }
}
impl Effect for SplitDrive {
    fn process(&mut self, buf: &mut [f32]) {
        let g = g_of(self.split, self.sr);
        let gt = g_of(800.0 * 2f32.powf(self.tone_v * 4.6), self.sr);
        let pre = 1.0 + self.drive * self.drive * 40.0;
        let comp = 1.0 / (1.0 + self.drive * 1.5);
        let low_g = db2lin(self.low);
        let outg = db2lin(self.out);
        // crush: bits and sample-hold from the drive in mode 3
        let bits = 2f32.powf(12.0 - self.drive * 9.0);
        let hold = 1.0 + (self.drive * self.drive * 24.0).floor();
        let mut lows = [0.0f32; 2];
        for fr in buf.chunks_exact_mut(2) {
            let mut highs = [0.0f32; 2];
            for c in 0..2 {
                let (lo, hi) = self.x[c].split(fin(fr[c]), g);
                lows[c] = lo;
                let y = hi * pre;
                let wet = if self.mode == 3 {
                    // bit / rate crusher
                    let (h, cnt) = &mut self.crush[c];
                    *cnt += 1.0;
                    if *cnt >= hold {
                        *cnt = 0.0;
                        *h = (Self::shape(3, y) * bits).round() / bits;
                    }
                    *h
                } else {
                    // twice the rate: a midpoint, then a 1-2-1 average back down (less aliasing from the new harmonics)
                    let (py, ps) = self.os[c];
                    let cur = Self::shape(self.mode, y);
                    let mid = Self::shape(self.mode, 0.5 * (py + y));
                    self.os[c] = (y, cur);
                    0.25 * ps + 0.5 * mid + 0.25 * cur
                };
                // DC from the asymmetric curve out, then the tone low-pass
                let (px, py) = self.dc[c];
                let d = wet - px + 0.995 * py;
                self.dc[c] = (wet, d);
                let toned = self.tone[c].run(d, gt, 0.9).0 * comp;
                highs[c] = hi * (1.0 - self.mix) + toned * self.mix;
            }
            if self.mono_low {
                let m = 0.5 * (lows[0] + lows[1]);
                lows = [m, m];
            }
            fr[0] = fin((lows[0] * low_g + highs[0]) * outg);
            fr[1] = fin((lows[1] * low_g + highs[1]) * outg);
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "split" => self.split = 40.0 * 2f32.powf(v * 4.3),
            "drive" => self.drive = v,
            "mode" => self.mode = (v * 3.0).round() as usize,
            "tone" => self.tone_v = v,
            "mix" => self.mix = v,
            "low" => self.low = (v - 0.5) * 24.0,
            "mono low" => self.mono_low = v > 0.5,
            "out" => self.out = (v - 0.5) * 24.0,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Split Drive"
    }
}

// ── Comb ────────────────────────────────────────────────────────────────────────

pub const COMB_PARAMS: [(&str, f32); 8] = [("note", 0.375), ("fine", 0.5), ("feedback", 0.85), ("damp", 0.3), ("rate", 0.2), ("sweep", 0.0), ("stereo", 0.2), ("mix", 0.5)];

pub struct Comb {
    sr: f32,
    line: [Line; 2],
    lp: [f32; 2],
    ph: f32,
    note: f32,
    fine: f32,
    fb: f32,
    damp: f32,
    rate: f32,
    sweep: f32,
    stereo: f32,
    mix: f32,
}
impl Comb {
    pub fn new(sr: f32) -> Comb {
        let n = (sr / 15.0) as usize + 8;
        Comb { sr, line: [Line::new(n), Line::new(n)], lp: [0.0; 2], ph: 0.0, note: 48.0, fine: 0.0, fb: 0.7, damp: 0.3, rate: 0.5, sweep: 0.0, stereo: 0.0, mix: 0.5 }
    }
}
impl Effect for Comb {
    fn process(&mut self, buf: &mut [f32]) {
        let dl = 1.0 - self.damp * 0.92;
        // a comb's gain grows with its feedback: scaled back so the level stays even across the knob
        let norm = 1.0 - self.fb.abs() * 0.85;
        for fr in buf.chunks_exact_mut(2) {
            self.ph = (self.ph + self.rate / self.sr).fract();
            let lfo = (TAU * self.ph).sin() * self.sweep * 12.0;
            for c in 0..2 {
                let st = if c == 0 { -self.stereo } else { self.stereo } * 0.35;
                let semis = self.note + self.fine + lfo + st;
                let f = 440.0 * 2f32.powf((semis - 69.0) / 12.0);
                let d = self.sr / f.max(15.0);
                let x = fin(fr[c]);
                let y = self.line[c].read(d);
                self.lp[c] += (y - self.lp[c]) * dl;
                self.line[c].push(x + self.lp[c] * self.fb);
                fr[c] = fin(x * (1.0 - self.mix) + y * norm * self.mix * 1.4);
            }
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "note" => self.note = 12.0 + v * 96.0,
            "fine" => self.fine = (v - 0.5) * 2.0,
            "feedback" => self.fb = (v * 2.0 - 1.0) * 0.97,
            "damp" => self.damp = v,
            "rate" => self.rate = 0.02 * 2f32.powf(v * 9.0),
            "sweep" => self.sweep = v,
            "stereo" => self.stereo = v,
            "mix" => self.mix = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Comb"
    }
}

// ── Phaser ──────────────────────────────────────────────────────────────────────

pub const PHASER_PARAMS: [(&str, f32); 7] = [("stages", 0.5), ("rate", 0.25), ("depth", 0.7), ("center", 0.45), ("feedback", 0.65), ("stereo", 0.5), ("mix", 0.5)];

pub struct Phaser {
    sr: f32,
    ap: [[f32; 12]; 2],
    fbs: [f32; 2],
    ph: f32,
    stages: usize,
    rate: f32,
    depth: f32,
    center: f32,
    fb: f32,
    stereo: f32,
    mix: f32,
}
impl Phaser {
    pub fn new(sr: f32) -> Phaser {
        Phaser { sr, ap: [[0.0; 12]; 2], fbs: [0.0; 2], ph: 0.0, stages: 8, rate: 0.3, depth: 0.7, center: 800.0, fb: 0.4, stereo: 0.5, mix: 0.5 }
    }
}
impl Effect for Phaser {
    fn process(&mut self, buf: &mut [f32]) {
        for fr in buf.chunks_exact_mut(2) {
            self.ph = (self.ph + self.rate / self.sr).fract();
            for c in 0..2 {
                let p = self.ph + if c == 1 { self.stereo * 0.25 } else { 0.0 };
                let l = 0.5 - 0.5 * (TAU * p).cos();
                // the notches sweep over +-3 octaves around the centre
                let f = (self.center * 2f32.powf((l - 0.5) * 6.0 * self.depth)).clamp(30.0, self.sr * 0.45);
                let t = (std::f32::consts::PI * f / self.sr).tan();
                let a = (t - 1.0) / (t + 1.0);
                let x = fin(fr[c]);
                let mut y = x + self.fbs[c] * self.fb;
                for k in 0..self.stages {
                    let s = self.ap[c][k];
                    let o = a * y + s;
                    self.ap[c][k] = y - a * o;
                    y = o;
                }
                self.fbs[c] = fin(y).clamp(-2.0, 2.0);
                fr[c] = fin(x * (1.0 - self.mix * 0.5) + y * self.mix * 0.5);
            }
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "stages" => self.stages = [4, 6, 8, 10, 12][(v * 4.0).round() as usize],
            "rate" => self.rate = 0.02 * 2f32.powf(v * 9.0),
            "depth" => self.depth = v,
            "center" => self.center = 100.0 * 2f32.powf(v * 6.5),
            "feedback" => self.fb = (v * 2.0 - 1.0) * 0.9,
            "stereo" => self.stereo = v,
            "mix" => self.mix = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Phaser"
    }
}

// ── Flanger ─────────────────────────────────────────────────────────────────────

pub const FLANGER_PARAMS: [(&str, f32); 6] = [("delay", 0.3), ("depth", 0.6), ("rate", 0.2), ("feedback", 0.75), ("stereo", 0.5), ("mix", 0.5)];

pub struct Flanger {
    sr: f32,
    line: [Line; 2],
    ph: f32,
    delay: f32,
    depth: f32,
    rate: f32,
    fb: f32,
    stereo: f32,
    mix: f32,
    last: [f32; 2],
}
impl Flanger {
    pub fn new(sr: f32) -> Flanger {
        let n = (sr * 0.03) as usize;
        Flanger { sr, line: [Line::new(n), Line::new(n)], ph: 0.0, delay: 3.0, depth: 0.6, rate: 0.2, fb: 0.5, stereo: 0.5, mix: 0.5, last: [0.0; 2] }
    }
}
impl Effect for Flanger {
    fn process(&mut self, buf: &mut [f32]) {
        for fr in buf.chunks_exact_mut(2) {
            self.ph = (self.ph + self.rate / self.sr).fract();
            for c in 0..2 {
                let p = self.ph + if c == 1 { self.stereo * 0.25 } else { 0.0 };
                let l = 0.5 - 0.5 * (TAU * p).cos();
                let ms = 0.1 + self.delay * (1.0 - self.depth * 0.95) + self.delay * self.depth * 0.95 * l * 2.0;
                let d = (ms * 0.001 * self.sr).max(1.0);
                let x = fin(fr[c]);
                let y = self.line[c].read(d);
                self.line[c].push(x + y * self.fb);
                self.last[c] = y;
                fr[c] = fin(x * (1.0 - self.mix * 0.5) + y * self.mix * 0.5 * (1.0 - self.fb.abs() * 0.5));
            }
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "delay" => self.delay = 0.3 + v * 9.7,
            "depth" => self.depth = v,
            "rate" => self.rate = 0.02 * 2f32.powf(v * 9.0),
            "feedback" => self.fb = (v * 2.0 - 1.0) * 0.95,
            "stereo" => self.stereo = v,
            "mix" => self.mix = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Flanger"
    }
}

// ── Freq Shifter ────────────────────────────────────────────────────────────────

pub const FSHIFT_PARAMS: [(&str, f32); 6] = [("shift", 0.6), ("fine", 0.5), ("rate", 0.2), ("sweep", 0.0), ("stereo", 0.0), ("mix", 0.5)];

/// the shift in Hz of a 0..1 knob: centre = 0, a square curve out to +-2 kHz (fine control near zero)
pub fn shift_hz(v: f32) -> f32 {
    let x = (v - 0.5) * 2.0;
    x.signum() * x * x * 2000.0
}

pub struct FreqShifter {
    sr: f32,
    h: [Hilbert; 2],
    ph: [f64; 2],
    lfo: f32,
    shift: f32,
    fine: f32,
    rate: f32,
    sweep: f32,
    stereo: f32,
    mix: f32,
}
impl FreqShifter {
    pub fn new(sr: f32) -> FreqShifter {
        FreqShifter { sr, h: Default::default(), ph: [0.0; 2], lfo: 0.0, shift: 20.0, fine: 0.0, rate: 0.2, sweep: 0.0, stereo: 0.0, mix: 0.5 }
    }
}
impl Effect for FreqShifter {
    fn process(&mut self, buf: &mut [f32]) {
        for fr in buf.chunks_exact_mut(2) {
            self.lfo = (self.lfo + self.rate / self.sr).fract();
            let base = self.shift + self.fine + (TAU * self.lfo).sin() * self.sweep * 400.0;
            for c in 0..2 {
                // stereo: the right channel shifts by a blend towards the opposite direction
                let hz = if c == 1 { base * (1.0 - 2.0 * self.stereo) } else { base };
                self.ph[c] = (self.ph[c] + hz as f64 / self.sr as f64).rem_euclid(1.0);
                let x = fin(fr[c]);
                let (i, q) = self.h[c].run(x);
                let a = (std::f64::consts::TAU * self.ph[c]) as f32;
                let y = i * a.cos() + q * a.sin();
                fr[c] = fin(x * (1.0 - self.mix) + y * self.mix);
            }
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "shift" => self.shift = shift_hz(v),
            "fine" => self.fine = (v - 0.5) * 20.0,
            "rate" => self.rate = 0.02 * 2f32.powf(v * 9.0),
            "sweep" => self.sweep = v,
            "stereo" => self.stereo = v,
            "mix" => self.mix = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Freq Shifter"
    }
}

// ── Imager ──────────────────────────────────────────────────────────────────────

pub const IMAGER_PARAMS: [(&str, f32); 3] = [("width", 0.5), ("mono below", 0.35), ("out", 0.5)];

pub struct Imager {
    sr: f32,
    x: Lr4,
    xs: Lr4,
    width: f32,
    mono_hz: f32,
    out: f32,
}
impl Imager {
    pub fn new(sr: f32) -> Imager {
        Imager { sr, x: Lr4::default(), xs: Lr4::default(), width: 1.0, mono_hz: 120.0, out: 0.0 }
    }
}
impl Effect for Imager {
    fn process(&mut self, buf: &mut [f32]) {
        let g = g_of(self.mono_hz.max(20.0), self.sr);
        let outg = db2lin(self.out);
        for fr in buf.chunks_exact_mut(2) {
            let (l, r) = (fin(fr[0]), fin(fr[1]));
            let m = 0.5 * (l + r);
            let mut s = 0.5 * (l - r);
            if self.mono_hz > 21.0 {
                // the side's lows removed (the mid keeps them): below the split everything is mono; the mid takes the same all-pass
                let (_, hi) = self.xs.split(s, g);
                s = hi;
                let (a, b) = self.x.split(m, g);
                let m2 = a + b;
                fr[0] = fin((m2 + s * self.width) * outg);
                fr[1] = fin((m2 - s * self.width) * outg);
            } else {
                fr[0] = fin((m + s * self.width) * outg);
                fr[1] = fin((m - s * self.width) * outg);
            }
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "width" => self.width = v * 2.0,
            "mono below" => self.mono_hz = if v < 0.02 { 0.0 } else { 20.0 * 2f32.powf(v * 5.0) },
            "out" => self.out = (v - 0.5) * 24.0,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Imager"
    }
}

// ── Transient ───────────────────────────────────────────────────────────────────

pub const TRANSIENT_PARAMS: [(&str, f32); 5] = [("attack", 0.7), ("sustain", 0.4), ("speed", 0.5), ("out", 0.5), ("mix", 1.0)];

pub struct Transient {
    sr: f32,
    fast: f32,
    slow: f32,
    att: f32,
    sus: f32,
    speed: f32,
    out: f32,
    mix: f32,
}
impl Transient {
    pub fn new(sr: f32) -> Transient {
        Transient { sr, fast: 0.0, slow: 0.0, att: 0.0, sus: 0.0, speed: 0.5, out: 0.0, mix: 1.0 }
    }
}
impl Effect for Transient {
    fn process(&mut self, buf: &mut [f32]) {
        let tf = 0.0008 + (1.0 - self.speed) * 0.004;
        let ts = 0.02 + (1.0 - self.speed) * 0.15;
        let kf = (-1.0 / (tf * self.sr)).exp();
        let ks = (-1.0 / (ts * self.sr)).exp();
        let outg = db2lin(self.out);
        for fr in buf.chunks_exact_mut(2) {
            let lvl = fin(fr[0]).abs().max(fin(fr[1]).abs());
            self.fast = lvl + (self.fast - lvl) * kf;
            // the slow one rises slowly and falls slowly: fast > slow means an attack, fast < slow the tail
            self.slow = lvl + (self.slow - lvl) * ks;
            let diff = lin2db(self.fast + 1e-6) - lin2db(self.slow + 1e-6);
            // boosts stop at +9 dB (a note out of silence would otherwise jump by the whole difference), cuts at -12
            let g_db = if diff > 0.0 { (diff.min(24.0) * self.att).clamp(-12.0, 9.0) } else { (-diff.max(-24.0) * self.sus).clamp(-12.0, 9.0) };
            let g = db2lin(g_db) * outg;
            for c in 0..2 {
                let x = fin(fr[c]);
                fr[c] = fin(x * (1.0 - self.mix) + x * g * self.mix);
            }
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "attack" => self.att = (v - 0.5) * 1.5,
            "sustain" => self.sus = (v - 0.5) * 1.5,
            "speed" => self.speed = v,
            "out" => self.out = (v - 0.5) * 24.0,
            "mix" => self.mix = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Transient"
    }
}

// ── presets ─────────────────────────────────────────────────────────────────────

type Pre = (&'static str, &'static [(&'static str, f32)]);
pub const OTT_PRESETS: [Pre; 4] = [
    ("Default 60%", &[]),
    ("Bass sheen (full)", &[("depth", 1.0), ("upward", 0.75), ("low", 0.45), ("high", 0.55), ("out", 0.42)]),
    ("Downward only (glue)", &[("depth", 0.5), ("upward", 0.0), ("downward", 1.0), ("out", 0.6)]),
    ("Upward only (detail)", &[("depth", 0.7), ("upward", 1.0), ("downward", 0.0), ("out", 0.38)]),
];
pub const SPLIT_PRESETS: [Pre; 5] = [
    ("Tube mids", &[("drive", 0.45), ("mode", 0.0), ("tone", 0.7)]),
    ("Folded growl", &[("drive", 0.6), ("mode", 0.333), ("tone", 0.65), ("split", 0.35)]),
    ("Hard clip", &[("drive", 0.55), ("mode", 0.667), ("tone", 0.6)]),
    ("Crushed", &[("drive", 0.5), ("mode", 1.0), ("tone", 0.75), ("mix", 0.7)]),
    ("Parallel grit", &[("drive", 0.7), ("mode", 0.0), ("mix", 0.4)]),
];
pub const COMB_PRESETS: [Pre; 4] = [
    ("Metal ring", &[("feedback", 0.92), ("damp", 0.15), ("mix", 0.45)]),
    ("Hollow (negative)", &[("feedback", 0.1), ("damp", 0.25), ("mix", 0.5)]),
    ("Swept comb", &[("feedback", 0.88), ("sweep", 0.35), ("rate", 0.3), ("mix", 0.5)]),
    ("Wide shimmer", &[("feedback", 0.85), ("stereo", 0.8), ("mix", 0.35)]),
];
pub const PHASER_PRESETS: [Pre; 3] = [("Slow swirl", &[("rate", 0.15), ("feedback", 0.7)]), ("Jet", &[("stages", 1.0), ("rate", 0.35), ("feedback", 0.85), ("mix", 0.6)]), ("Notch growl", &[("stages", 0.75), ("rate", 0.55), ("depth", 0.9), ("feedback", 0.25)])];
pub const FLANGER_PRESETS: [Pre; 3] = [("Jet sweep", &[("rate", 0.15), ("feedback", 0.88)]), ("Metallic", &[("delay", 0.1), ("feedback", 0.95), ("rate", 0.3)]), ("Negative hollow", &[("feedback", 0.1), ("mix", 0.6)])];
pub const FSHIFT_PRESETS: [Pre; 3] = [("Slight shimmer", &[("shift", 0.53), ("stereo", 0.5), ("mix", 0.4)]), ("Robot", &[("shift", 0.72), ("mix", 0.8)]), ("Barber pole", &[("shift", 0.52), ("sweep", 0.3), ("rate", 0.25), ("mix", 0.5)])];
pub const IMAGER_PRESETS: [Pre; 3] = [("Bass: mono below 120 Hz", &[]), ("Wide top", &[("width", 0.75), ("mono below", 0.45)]), ("Mono", &[("width", 0.0)])];
pub const TRANSIENT_PRESETS: [Pre; 3] = [("Punch", &[]), ("Tight (short tails)", &[("attack", 0.6), ("sustain", 0.2)]), ("Soften", &[("attack", 0.3), ("sustain", 0.6)])];

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(sr: f32, f: f32, n: usize, amp: f32) -> Vec<f32> {
        (0..n).flat_map(|i| {
            let x = (TAU * f * i as f32 / sr).sin() * amp;
            [x, x]
        }).collect()
    }
    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn every_effect_stays_finite_for_any_settings() {
        let sr = 48000.0;
        for name in ["OTT", "Split Drive", "Comb", "Phaser", "Flanger", "Freq Shifter", "Imager", "Transient"] {
            for v in [0.0f32, 0.5, 1.0] {
                let mut e = super::super::fx::make(name, sr).unwrap();
                for (p, _) in super::super::fx::defaults(name) {
                    e.set_param(p, v);
                }
                let mut b: Vec<f32> = (0..9600).map(|i| ((i * 7919) % 2000) as f32 / 1000.0 - 1.0).collect();
                b[100] = f32::NAN;
                b[200] = 40.0;
                e.process(&mut b);
                assert!(b.iter().all(|x| x.is_finite() && x.abs() <= 8.0), "{name} at {v}");
            }
        }
    }

    #[test]
    fn ott_brings_quiet_up_and_loud_down() {
        let sr = 48000.0;
        let run = |amp: f32| {
            let mut o = Ott::new(sr);
            for (p, v) in OTT_PARAMS {
                o.set_param(p, v);
            }
            o.set_param("depth", 1.0);
            o.set_param("upward", 1.0);
            let mut b = tone(sr, 1000.0, 48000, amp);
            o.process(&mut b);
            rms(&b[48000..]) / rms(&tone(sr, 1000.0, 48000, amp)[48000..])
        };
        assert!(run(0.01) > 2.0, "quiet came up only {}", run(0.01));
        assert!(run(0.9) < 0.5, "loud went down only to {}", run(0.9));
    }

    #[test]
    fn crossovers_sum_flat_and_split_drive_keeps_the_sub_clean() {
        let sr = 48000.0;
        // the OTT at depth 0 is transparent in level across the spectrum
        for f in [50.0, 300.0, 2500.0, 8000.0] {
            let mut o = Ott::new(sr);
            o.set_param("depth", 0.0);
            let mut b = tone(sr, f, 24000, 0.5);
            o.process(&mut b);
            let r = rms(&b[12000..]) / rms(&tone(sr, f, 24000, 0.5)[12000..]);
            assert!((r - 1.0).abs() < 0.06, "{f} Hz: {r}");
        }
        // a 40 Hz sine through heavy split drive comes out a sine: (almost) nothing at its 3rd harmonic
        let mut d = SplitDrive::new(sr);
        for (p, v) in SPLIT_PARAMS {
            d.set_param(p, v);
        }
        d.set_param("drive", 1.0);
        let mut b = tone(sr, 40.0, 48000, 0.8);
        d.process(&mut b);
        let mono: Vec<f32> = b.chunks(2).map(|c| c[0]).skip(24000).collect();
        let mag = |f: f32| {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, x) in mono.iter().enumerate() {
                let a = TAU * f * i as f32 / sr;
                re += x * a.cos();
                im += x * a.sin();
            }
            (re * re + im * im).sqrt()
        };
        assert!(mag(120.0) < mag(40.0) * 0.05, "the sub distorted: {} vs {}", mag(120.0), mag(40.0));
    }

    #[test]
    fn the_comb_rings_at_its_note_and_the_shifter_moves_partials() {
        let sr = 48000.0;
        // white-ish noise through a comb at A2 (110 Hz): energy piles up at 110 Hz and its multiples
        let mut c = Comb::new(sr);
        for (p, v) in COMB_PARAMS {
            c.set_param(p, v);
        }
        c.set_param("note", (45.0 - 12.0) / 96.0);
        c.set_param("feedback", 0.97);
        c.set_param("mix", 1.0);
        let mut seed = 1u32;
        let mut b: Vec<f32> = (0..96000).map(|_| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 9) as f32 / 8388608.0 - 1.0
        }).flat_map(|x| [x * 0.2, x * 0.2]).collect();
        c.process(&mut b);
        let mono: Vec<f32> = b.chunks(2).map(|x| x[0]).skip(48000).collect();
        let mag = |f: f32| {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, x) in mono.iter().enumerate() {
                let a = TAU * f * i as f32 / sr;
                re += x * a.cos();
                im += x * a.sin();
            }
            (re * re + im * im).sqrt()
        };
        assert!(mag(220.0) > 3.0 * mag(165.0), "no peak at the 2nd harmonic: {} vs {}", mag(220.0), mag(165.0));
        // 1 kHz shifted by +100 Hz: the energy moves to 1.1 kHz
        let mut s = FreqShifter::new(sr);
        s.set_param("mix", 1.0);
        s.shift = 100.0;
        let mut b = tone(sr, 1000.0, 48000, 0.5);
        s.process(&mut b);
        let mono: Vec<f32> = b.chunks(2).map(|x| x[0]).skip(24000).collect();
        let mag = |f: f32| {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, x) in mono.iter().enumerate() {
                let a = TAU * f * i as f32 / sr;
                re += x * a.cos();
                im += x * a.sin();
            }
            (re * re + im * im).sqrt()
        };
        assert!(mag(1100.0) > 10.0 * mag(1000.0) && mag(1100.0) > 10.0 * mag(900.0), "{} {} {}", mag(900.0), mag(1000.0), mag(1100.0));
    }

    #[test]
    fn the_imager_keeps_the_lows_mono() {
        let sr = 48000.0;
        let mut im = Imager::new(sr);
        im.set_param("width", 0.5);
        im.set_param("mono below", 0.45);
        // opposite-polarity 50 Hz (all side): mostly removed; opposite-polarity 3 kHz: widened
        let side = |f: f32, im: &mut Imager| {
            let mut b: Vec<f32> = (0..48000).flat_map(|i| {
                let x = (TAU * f * i as f32 / sr).sin() * 0.3;
                [x, -x]
            }).collect();
            im.process(&mut b);
            rms(&b[48000..])
        };
        let low = side(50.0, &mut im);
        assert!(low < 0.06, "{low}");
        let mut im2 = Imager::new(sr);
        im2.set_param("width", 1.0);
        assert!(side(3000.0, &mut im2) > 0.35);
    }
}
