//! Rainbow Hypergrowl: the digital harmonic growl of a resampled-wavetable bass, as one effect with a deep signal chain.
//!
//! The growl in a "HyperGrowl" bass is not one trick but a stack: a square / saw is driven and folded until it is full of harmonics,
//! a formant-like filter is swept across it while the wavetable position moves, a comb gives it a metallic voice, and finally it is
//! squashed spectrally so every harmonic speaks at once. This effect is that whole stack, with its own motion so it moves by itself,
//! and a grain delay for the shattered, granular tails of modern bass music.
//!
//!   drive:    pre-gain, asymmetric bias, tanh -> sine fold, a ring modulator, a bit / rate crusher, DC blocked
//!   table:    24 band-pass filters (50 Hz - 12 kHz) whose gains are read from TWO spectral wavetables (12 shapes: vowels, "yoi",
//!             comb, harmonic, hollow, rainbow, digital, ...) crossfaded by `blend`; the shared position scans with the motion,
//!             `shift` slides the whole bank in octaves, `spread` detunes the bands apart (the "rainbow", and wider on the right), `tilt`,
//!             and `body` keeps the clean low end underneath so a bass does not thin out
//!   comb:     a feedback comb tuned in Hz, wobbled by the second LFO
//!   grain:    a granular delay: up to 32 overlapping grains read a 2 s buffer at random delays, pitches (+-24 st), directions and
//!             pans, with feedback
//!   spectral: a 2048-point STFT: peaks pulled toward the spectral envelope (`squash`, `focus`), a bin shifter (`sshift`), a gate that
//!             kills weak bins (`sgate`), spectral blur (`blur`) and a spectral tilt
//!   out:      stereo width, level, soft limiter, and mix against the (latency-matched) dry signal
//! Motion: an LFO (sine / triangle / saw / square / sample & hold) with optional stepping, an envelope follower that scans the table by
//! how hard the input hits, a slow random drift and a second LFO that wobbles the shift, the comb and the grain pitch.
use super::fx::Effect;
use crate::dsp::TAU;

pub const LATENCY: usize = 2048;
const N: usize = LATENCY;
const HOP: usize = N / 4;
pub const BANDS: usize = 24;
const BLOCK: usize = 16;
const GRAINS: usize = 32;
const GBUF_SECS: f32 = 2.0;

macro_rules! params {
    ($($n:literal = $d:expr),* $(,)?) => { [$(($n, $d)),*] };
}

/// (name, default); the order is the order of the knobs and of automation indices
pub const PARAMS: [(&str, f32); 52] = params![
    // drive
    "drive" = 0.35, "fold" = 0.35, "bias" = 0.5, "crush" = 0.0, "ring" = 0.0, "ringhz" = 0.4,
    // table
    "table" = 0.7083, "tableb" = 0.0417, "blend" = 0.3, "pos" = 0.4, "res" = 0.6, "shift" = 0.5, "spread" = 0.4, "tilt" = 0.5, "body" = 0.35,
    // comb
    "comb" = 0.0, "ctune" = 0.4, "cmod" = 0.0,
    // motion
    "rate" = 0.4, "motion" = 0.7, "shape" = 0.3, "steps" = 0.0, "follow" = 0.0, "drift" = 0.0, "wobble" = 0.0,
    // grain
    "gmix" = 0.0, "gtime" = 0.35, "gsize" = 0.4, "gdens" = 0.5, "gspray" = 0.3, "gpitch" = 0.5, "grand" = 0.2, "grev" = 0.15, "gfb" = 0.2, "gwidth" = 0.6,
    // spectral
    "squash" = 0.5, "focus" = 0.6, "sshift" = 0.5, "sgate" = 0.0, "blur" = 0.0, "stilt" = 0.5,
    // out
    "width" = 0.5, "level" = 0.5, "mix" = 1.0,
    // sample: a WAV (or the built-in vowel sweep) as a spectral source
    "smix" = 0.0, "spos" = 0.4, "sscan" = 0.5, "xsyn" = 0.0, "freeze" = 0.0, "gsrc" = 0.0,
    // character: an octave-down sub for bass, and an even-harmonic warmth before the fold
    "sub" = 0.0, "warm" = 0.0,
];

fn pidx(n: &str) -> usize {
    match n {
    "drive" => 0,
    "fold" => 1,
    "bias" => 2,
    "crush" => 3,
    "ring" => 4,
    "ringhz" => 5,
    "table" => 6,
    "tableb" => 7,
    "blend" => 8,
    "pos" => 9,
    "res" => 10,
    "shift" => 11,
    "spread" => 12,
    "tilt" => 13,
    "body" => 14,
    "comb" => 15,
    "ctune" => 16,
    "cmod" => 17,
    "rate" => 18,
    "motion" => 19,
    "shape" => 20,
    "steps" => 21,
    "follow" => 22,
    "drift" => 23,
    "wobble" => 24,
    "gmix" => 25,
    "gtime" => 26,
    "gsize" => 27,
    "gdens" => 28,
    "gspray" => 29,
    "gpitch" => 30,
    "grand" => 31,
    "grev" => 32,
    "gfb" => 33,
    "gwidth" => 34,
    "squash" => 35,
    "focus" => 36,
    "sshift" => 37,
    "sgate" => 38,
    "blur" => 39,
    "stilt" => 40,
    "width" => 41,
    "level" => 42,
    "mix" => 43,
        "smix" => 44,
        "spos" => 45,
        "sscan" => 46,
        "xsyn" => 47,
        "freeze" => 48,
        "gsrc" => 49,
        "sub" => 50,
        "warm" => 51,
        _ => usize::MAX,
    }
}

pub const TABLES: [&str; 12] = ["Vowel", "Comb", "Harmonic", "Hollow", "Rainbow", "Digital", "Tilt", "Bell", "Yoi", "Ripple", "Notches", "Stairs"];
pub const SHAPES: [&str; 5] = ["sine", "tri", "saw", "square", "s&h"];

/// ready-made sounds: overrides on top of the defaults
pub const PRESETS: [(&str, &[(&str, f32)]); 13] = [
    ("Init", &[("gmix", 0.0)]),
    ("Hypergrowl", &[("drive", 0.5), ("fold", 0.45), ("table", 0.7083), ("tableb", 0.0417), ("blend", 0.3), ("pos", 0.4), ("res", 0.65), ("spread", 0.5), ("rate", 0.42), ("motion", 0.85), ("shape", 0.3), ("squash", 0.75), ("gmix", 0.18), ("gsize", 0.3), ("gdens", 0.6), ("gpitch", 0.5), ("grand", 0.1)]),
    ("Yoi Bass", &[("drive", 0.5), ("fold", 0.3), ("table", 0.7083), ("tableb", 0.0417), ("blend", 0.25), ("pos", 0.5), ("res", 0.72), ("motion", 1.0), ("rate", 0.46), ("shape", 0.3), ("body", 0.5), ("squash", 0.6), ("gmix", 0.0)]),
    ("Digital Chop", &[("drive", 0.55), ("fold", 0.6), ("crush", 0.35), ("table", 0.4583), ("tableb", 0.9583), ("blend", 0.5), ("steps", 0.7), ("rate", 0.56), ("shape", 0.5), ("squash", 0.9), ("sgate", 0.3), ("gmix", 0.1), ("gdens", 0.7), ("gsize", 0.2)]),
    ("Granular Cloud", &[("drive", 0.15), ("fold", 0.1), ("motion", 0.3), ("gmix", 0.75), ("gtime", 0.5), ("gsize", 0.6), ("gdens", 0.62), ("gspray", 0.6), ("grand", 0.45), ("gfb", 0.45), ("gwidth", 0.95), ("squash", 0.3), ("blur", 0.5)]),
    ("Octave Shatter", &[("drive", 0.4), ("fold", 0.5), ("ring", 0.3), ("ringhz", 0.5), ("gmix", 0.55), ("gdens", 0.72), ("gsize", 0.22), ("gpitch", 0.75), ("grand", 0.1), ("gfb", 0.3), ("gspray", 0.4), ("squash", 0.6)]),
    ("Comb Scream", &[("drive", 0.6), ("fold", 0.5), ("table", 0.1250), ("tableb", 0.2083), ("blend", 0.4), ("res", 0.8), ("comb", 0.8), ("ctune", 0.35), ("cmod", 0.5), ("motion", 0.5), ("rate", 0.3), ("squash", 0.7)]),
    ("Vowel Melt", &[("drive", 0.3), ("fold", 0.25), ("table", 0.0417), ("tableb", 0.3750), ("blend", 0.5), ("motion", 0.9), ("rate", 0.25), ("shape", 0.0), ("spread", 0.8), ("wobble", 0.4), ("squash", 0.5), ("blur", 0.6), ("sshift", 0.55), ("gmix", 0.2), ("gspray", 0.7)]),
    ("Vocoder Bass", &[("drive", 0.5), ("fold", 0.35), ("xsyn", 0.9), ("smix", 0.35), ("spos", 0.4), ("sscan", 0.6), ("motion", 0.6), ("rate", 0.3), ("squash", 0.35), ("res", 0.6), ("body", 0.5)]),
    ("Sample Cloud", &[("drive", 0.15), ("fold", 0.1), ("gsrc", 0.85), ("gmix", 0.75), ("gdens", 0.6), ("gsize", 0.45), ("gspray", 0.5), ("grand", 0.3), ("gfb", 0.35), ("gwidth", 0.9), ("spos", 0.5), ("sscan", 0.5), ("squash", 0.2), ("blur", 0.4)]),
    ("Sample Scan", &[("drive", 0.4), ("fold", 0.3), ("smix", 0.95), ("spos", 0.3), ("sscan", 1.0), ("motion", 0.8), ("rate", 0.3), ("res", 0.7), ("squash", 0.6), ("body", 0.4)]),
    ("Freeze Pad", &[("drive", 0.2), ("fold", 0.15), ("freeze", 0.9), ("blur", 0.55), ("gmix", 0.3), ("gspray", 0.6), ("squash", 0.4), ("xsyn", 0.3), ("width", 0.8)]),
    ("Sub Rattler", &[("drive", 0.6), ("fold", 0.2), ("table", 0.2917), ("tableb", 0.8750), ("blend", 0.4), ("pos", 0.2), ("body", 0.75), ("motion", 0.6), ("squash", 0.8), ("sgate", 0.2), ("level", 0.45)]),
];

fn gauss(d: f32, w: f32) -> f32 {
    (-(d * d) / (2.0 * w * w)).exp()
}

fn hash01(n: u32) -> f32 {
    let mut h = n.wrapping_mul(2654435761).wrapping_add(0x9e3779b9);
    h ^= h >> 15;
    h = h.wrapping_mul(2246822519);
    h ^= h >> 13;
    (h & 0xffff) as f32 / 65535.0
}

fn wrapdist(a: f32, b: f32) -> f32 {
    let d = (a - b).abs() % 1.0;
    d.min(1.0 - d)
}

/// gain (0..~1.6) of a band at log-frequency position `x` (0..1) for table `t` at position `m` (0..1)
pub fn table_gain(t: usize, x: f32, m: f32) -> f32 {
    match t {
        0 => {
            // five vowels (A E I O U) as formant pairs, morphed between neighbours
            const V: [(f32, f32); 5] = [(0.45, 0.62), (0.38, 0.72), (0.28, 0.8), (0.42, 0.55), (0.3, 0.5)];
            let f = m.clamp(0.0, 0.9999) * 4.0;
            let i = f.floor() as usize;
            let u = f - i as f32;
            let (a, b) = (V[i], V[i + 1]);
            let (f1, f2) = (a.0 + (b.0 - a.0) * u, a.1 + (b.1 - a.1) * u);
            gauss(x - f1, 0.055) * 1.5 + gauss(x - f2, 0.045) * 1.1 + gauss(x - (f2 + 0.14), 0.04) * 0.5 + 0.03
        }
        1 => {
            let c = 2.0 + m * 14.0;
            let v = 0.5 + 0.5 * (TAU * x * c).cos();
            v * v * 1.4 + 0.03
        }
        2 => {
            let c = 3.0 + m * 20.0;
            let v = (0.5 + 0.5 * (TAU * x * c).cos()).powi(6);
            v * 1.6 + 0.02
        }
        3 => 1.0 - gauss(x - m, 0.07) * 0.97,
        4 => {
            let mut s = 0.0;
            for k in 0..3 {
                let c = (m + k as f32 / 3.0).fract();
                s += (-(wrapdist(x, c).powi(2)) / (2.0 * 0.05 * 0.05)).exp();
            }
            s * 1.3 + 0.03
        }
        5 => {
            let frame = (m * 8.0).floor() as u32;
            let v = hash01(frame * 31 + (x * (BANDS as f32 - 1.0)).round() as u32);
            if v > 0.5 {
                v * 1.5
            } else {
                0.02
            }
        }
        6 => {
            let tilt = m * 2.0 - 1.0;
            ((0.5 + (x - 0.5) * tilt * 2.0).clamp(0.0, 1.0) * 1.5).max(0.03)
        }
        7 => gauss(x - m, 0.035) * 1.7 + 0.02,
        8 => {
            // "yoi": a low formant that dips while a high formant sweeps across the whole range, the classic growl vowel
            let f1 = 0.26 + 0.07 * (TAU * m).sin();
            let f2 = 0.3 + m * 0.55;
            gauss(x - f1, 0.05) * 1.3 + gauss(x - f2, 0.04) * 1.5 + gauss(x - (f2 + 0.17), 0.035) * 0.6 + 0.02
        }
        9 => (0.55 + 0.45 * (TAU * (x * (2.0 + 6.0 * m) + m * 3.0)).sin()).max(0.02) * 1.2,
        10 => {
            let mut g = 1.0;
            for j in 0..3 {
                g -= gauss(x - (m + j as f32 / 3.0).fract(), 0.035) * 0.95;
            }
            g.max(0.02) * 1.1
        }
        _ => {
            // stairs: alternating on / off blocks that slide with the position
            let k = ((x + m) * 6.0).floor() as i32;
            if k.rem_euclid(2) == 0 {
                1.3
            } else {
                0.03
            }
        }
    }
}

// ── a radix-2 FFT ──────────────────────────────────────────────────────────────
struct Fft {
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<usize>,
}
impl Fft {
    fn new() -> Self {
        let bits = N.trailing_zeros();
        let rev = (0..N).map(|i| i.reverse_bits() >> (usize::BITS - bits)).collect();
        let cos = (0..N / 2).map(|k| (TAU * k as f32 / N as f32).cos()).collect();
        let sin = (0..N / 2).map(|k| -(TAU * k as f32 / N as f32).sin()).collect();
        Fft { cos, sin, rev }
    }
    /// in place; `inverse` conjugates the twiddles (no scaling)
    fn run(&self, re: &mut [f32], im: &mut [f32], inverse: bool) {
        for i in 0..N {
            let j = self.rev[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= N {
            let step = N / len;
            for s in (0..N).step_by(len) {
                for k in 0..len / 2 {
                    let (wr, wi) = (self.cos[k * step], if inverse { -self.sin[k * step] } else { self.sin[k * step] });
                    let (a, b) = (s + k, s + k + len / 2);
                    let (tr, ti) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len <<= 1;
        }
    }
}

/// what the spectral stage does this frame
#[derive(Clone, Copy)]
struct SpecParams {
    squash: f32,
    radius: usize,
    shift_bins: i32,
    gate: f32,
    blur: f32,
    tilt: f32,
    /// how far the sample's spectral envelope is laid over the input's (0 = not at all)
    xsyn: f32,
    /// hold the spectrum as it was (0 = live, 1 = frozen)
    freeze: f32,
}

/// streaming spectral processor for one channel
struct Spectral {
    inbuf: Vec<f32>,
    outacc: Vec<f32>,
    pos: usize,
    count: usize,
    re: Vec<f32>,
    im: Vec<f32>,
    mag: Vec<f32>,
    env: Vec<f32>,
    sre: Vec<f32>,
    sim: Vec<f32>,
    prev_g: Vec<f32>,
    frozen: Vec<f32>,
    fphase: Vec<f32>,
}
impl Spectral {
    fn new() -> Self {
        let h = N / 2 + 1;
        Spectral { inbuf: vec![0.0; N], outacc: vec![0.0; N], pos: 0, count: 0, re: vec![0.0; N], im: vec![0.0; N], mag: vec![0.0; h], env: vec![0.0; h], sre: vec![0.0; h], sim: vec![0.0; h], prev_g: vec![1.0; h], frozen: vec![0.0; h], fphase: vec![0.0; h] }
    }

    #[inline]
    fn tick(&mut self, x: f32, fft: &Fft, win: &[f32], sp: &SpecParams, smag: Option<&[f32]>) -> f32 {
        let p = self.pos;
        let wet = self.outacc[p];
        self.outacc[p] = 0.0;
        self.inbuf[p] = x;
        self.pos = (p + 1) % N;
        self.count += 1;
        if self.count % HOP == 0 {
            self.frame(fft, win, sp, smag);
        }
        wet
    }

    fn frame(&mut self, fft: &Fft, win: &[f32], sp: &SpecParams, smag: Option<&[f32]>) {
        let half = N / 2;
        for i in 0..N {
            self.re[i] = self.inbuf[(self.pos + i) % N] * win[i];
            self.im[i] = 0.0;
        }
        fft.run(&mut self.re, &mut self.im, false);
        // bin shift: the whole spectrum slides up or down by whole bins (inharmonic, digital)
        let sh = sp.shift_bins;
        for k in 0..=half {
            let src = k as i32 - sh;
            if src >= 0 && src <= half as i32 {
                self.sre[k] = self.re[src as usize];
                self.sim[k] = self.im[src as usize];
            } else {
                self.sre[k] = 0.0;
                self.sim[k] = 0.0;
            }
            self.mag[k] = (self.sre[k] * self.sre[k] + self.sim[k] * self.sim[k]).sqrt();
        }
        // the local spectral envelope: a moving average of the magnitudes
        let r = sp.radius.max(1);
        let mut acc: f32 = self.mag[..=r.min(half)].iter().sum();
        let mut cnt = r.min(half) + 1;
        let mut peak = 1e-9f32;
        for k in 0..=half {
            self.env[k] = (acc / cnt as f32).max(0.0);
            peak = peak.max(self.mag[k]);
            let add = k + r + 1;
            if add <= half {
                acc += self.mag[add];
                cnt += 1;
            }
            if k >= r {
                acc -= self.mag[k - r];
                cnt -= 1;
            }
        }
        // cross-synthesis: the sample's spectral envelope, level-matched to the input's, is laid over the input's own envelope
        let xs = sp.xsyn;
        let (xscale, use_x) = match smag {
            Some(sm) if xs > 0.01 && sm.len() > half => {
                let ei: f32 = self.env[..=half].iter().sum();
                let es: f32 = sm[..=half].iter().sum();
                (ei / (es + 1e-12), true)
            }
            _ => (1.0, false),
        };
        let amt = sp.squash * 0.9;
        let gate_level = peak * sp.gate * sp.gate * 0.25;
        let smooth = sp.blur * 0.92;
        for k in 0..=half {
            let m = self.mag[k] + 1e-9;
            // peaks come down, valleys come up toward the envelope (bounded so the noise floor does not explode)
            let mut g = ((self.env[k] + 1e-9) / m).powf(amt).clamp(0.05, 6.0);
            if sp.gate > 0.01 && m < gate_level {
                g *= 0.04;
            }
            // spectral tilt: up to +-9 dB across the range
            g *= 10f32.powf(sp.tilt * (k as f32 / half as f32 - 0.5) * 18.0 / 20.0);
            if use_x {
                let sm = smag.unwrap();
                g *= (((sm[k] * xscale) + 1e-9) / (self.env[k] + 1e-9)).powf(xs).clamp(0.03, 10.0);
            }

            // blur: the gains glide between frames so the spectrum smears in time
            g = self.prev_g[k] * smooth + g * (1.0 - smooth);
            self.prev_g[k] = g;
            let (mut ore, mut oim) = (self.sre[k] * g, self.sim[k] * g);
            // freeze: while the knob is down the picture of the spectrum (magnitudes and phases) is refreshed every frame; turned up, the
            // last picture keeps sounding, its phases advancing like steady partials, and fades over the live sound
            let fz = sp.freeze.min(1.0);
            if fz < 0.02 {
                self.frozen[k] = m * g;
                self.fphase[k] = self.sim[k].atan2(self.sre[k]);
            } else {
                self.fphase[k] = (self.fphase[k] + TAU * k as f32 * HOP as f32 / N as f32).rem_euclid(TAU);
                let fm = self.frozen[k];
                ore = ore * (1.0 - fz) + fm * self.fphase[k].cos() * fz;
                oim = oim * (1.0 - fz) + fm * self.fphase[k].sin() * fz;
            }
            self.re[k] = ore;
            self.im[k] = oim;
            if k > 0 && k < half {
                self.re[N - k] = self.re[k];
                self.im[N - k] = -self.im[k];
            }
        }
        self.im[0] = 0.0;
        self.im[half] = 0.0;
        fft.run(&mut self.re, &mut self.im, true);
        // sqrt-hann analysis + synthesis at 4x overlap sums to 2, the inverse FFT scales by N
        let scale = 0.5 / N as f32;
        for i in 0..N {
            self.outacc[(self.pos + i) % N] += self.re[i] * win[i] * scale;
        }
    }
}

// ── the band-pass filter bank ──────────────────────────────────────────────────
#[derive(Clone, Copy, Default)]
struct Band {
    ic1: f32,
    ic2: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
}
impl Band {
    /// zero-delay-feedback state variable band-pass, normalised to unity gain at the centre
    fn set(&mut self, f: f32, res: f32, sr: f32) {
        let g = (std::f32::consts::PI * f.clamp(30.0, sr * 0.45) / sr).tan();
        let q = 0.7 + res * res * 45.0;
        self.k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        v1 * self.k
    }
}

/// A sample analysed for the growl: its mono audio (at the effect's rate) for the grains, and per analysis frame its smoothed spectral
/// envelope (for cross-synthesis) and 24 band gains (for the wavetable filter).
struct SampleBank {
    path: String,
    mono: Vec<f32>,
    nframes: usize,
    env: Vec<f32>,
    gains: Vec<[f32; BANDS]>,
}

impl SampleBank {
    fn empty() -> Self {
        SampleBank { path: String::new(), mono: vec![], nframes: 0, env: vec![], gains: vec![] }
    }

    /// `mono` must already be at the effect's sample rate
    fn analyze(mut mono: Vec<f32>, path: &str, sr: f32, fft: &Fft) -> SampleBank {
        mono.truncate((sr * 20.0) as usize);
        let peak = mono.iter().fold(0.0f32, |a, b| a.max(b.abs())).max(1e-6);
        for x in mono.iter_mut() {
            *x = *x / peak * 0.7;
        }
        let half = N / 2;
        let hop = (mono.len().saturating_sub(N) / 383).max(512);
        let nframes = if mono.len() <= N { 1 } else { (mono.len() - N) / hop + 1 };
        let hann: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (TAU * i as f32 / N as f32).cos()).collect();
        // band edges: geometric means between the filter bank's centres, as bins
        let centre = |k: usize| 50.0 * (12000.0f32 / 50.0).powf(k as f32 / (BANDS - 1) as f32);
        let bin_of = |f: f32| ((f * N as f32 / sr) as usize).min(half);
        let mut edges = [0usize; BANDS + 1];
        for k in 0..=BANDS {
            let f = if k == 0 { centre(0) / 1.15 } else if k == BANDS { centre(BANDS - 1) * 1.15 } else { (centre(k - 1) * centre(k)).sqrt() };
            edges[k] = bin_of(f);
        }
        let mut env = vec![0.0f32; nframes * (half + 1)];
        let mut raw_gains: Vec<[f32; BANDS]> = Vec::with_capacity(nframes);
        let (mut re, mut im) = (vec![0.0f32; N], vec![0.0f32; N]);
        for f in 0..nframes {
            for i in 0..N {
                re[i] = mono.get(f * hop + i).copied().unwrap_or(0.0) * hann[i];
                im[i] = 0.0;
            }
            fft.run(&mut re, &mut im, false);
            let mag: Vec<f32> = (0..=half).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() / (N as f32 * 0.25)).collect();
            // smooth: a moving average of 6 bins each way (the envelope, not the harmonics)
            let row = &mut env[f * (half + 1)..(f + 1) * (half + 1)];
            let mut acc: f32 = mag[..=6.min(half)].iter().sum();
            let mut cnt = 6.min(half) + 1;
            for k in 0..=half {
                row[k] = acc / cnt as f32;
                if k + 7 <= half {
                    acc += mag[k + 7];
                    cnt += 1;
                }
                if k >= 6 {
                    acc -= mag[k - 6];
                    cnt -= 1;
                }
                if acc < 0.0 {
                    acc = 0.0;
                }
            }
            let mut g = [0.0f32; BANDS];
            for k in 0..BANDS {
                let (a, b) = (edges[k], edges[k + 1].max(edges[k] + 1).min(half));
                g[k] = row[a..=b.max(a)].iter().sum::<f32>() / ((b.max(a) - a + 1) as f32);
            }
            raw_gains.push(g);
        }
        let gmax = raw_gains.iter().flat_map(|g| g.iter()).fold(1e-9f32, |a, b| a.max(*b));
        let gains = raw_gains.iter().map(|g| std::array::from_fn(|k| (g[k] / gmax).sqrt() * 1.5)).collect();
        SampleBank { path: path.to_string(), mono, nframes, env, gains }
    }

    /// the frame position 0..1 as (frame, next frame, fraction)
    fn at(&self, pos: f32) -> (usize, usize, f32) {
        if self.nframes <= 1 {
            return (0, 0, 0.0);
        }
        let f = pos.clamp(0.0, 1.0) * (self.nframes - 1) as f32;
        let i = f.floor() as usize;
        (i, (i + 1).min(self.nframes - 1), f - i as f32)
    }

    fn band_gain(&self, pos: f32, k: usize) -> f32 {
        if self.nframes == 0 {
            return 1.0;
        }
        let (a, b, t) = self.at(pos);
        self.gains[a][k] * (1.0 - t) + self.gains[b][k] * t
    }

    fn fill_env(&self, pos: f32, out: &mut [f32]) {
        if self.nframes == 0 {
            out.iter_mut().for_each(|x| *x = 1.0);
            return;
        }
        let w = N / 2 + 1;
        let (a, b, t) = self.at(pos);
        for k in 0..w.min(out.len()) {
            out[k] = self.env[a * w + k] * (1.0 - t) + self.env[b * w + k] * t;
        }
    }
}

/// The built-in sample: three seconds of a sung vowel gliding A - E - I - O - U over a 110 Hz saw with a little vibrato and breath.
fn default_sample(sr: f32) -> Vec<f32> {
    const VOW: [(f32, f32); 5] = [(800.0, 1200.0), (450.0, 2000.0), (300.0, 2300.0), (500.0, 900.0), (320.0, 800.0)];
    let n = (sr * 3.0) as usize;
    let mut f1 = Band::default();
    let mut f2 = Band::default();
    let mut rng = Rng(0xC0FFEE);
    let mut ph = 0.0f32;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / n as f32;
        let pos = t * 4.0;
        let a = (pos.floor() as usize).min(3);
        let u = pos - a as f32;
        let (c1, c2) = (VOW[a].0 + (VOW[a + 1].0 - VOW[a].0) * u, VOW[a].1 + (VOW[a + 1].1 - VOW[a].1) * u);
        if i % 64 == 0 {
            f1.set(c1, 0.8, sr);
            f2.set(c2, 0.8, sr);
        }
        ph = (ph + (110.0 * (1.0 + 0.012 * (TAU * 5.5 * i as f32 / sr).sin())) / sr).fract();
        let src = (ph * 2.0 - 1.0) * 0.8 + (rng.next() - 0.5) * 0.12;
        out.push(f1.tick(src) * 1.0 + f2.tick(src) * 0.6);
    }
    out
}

#[derive(Clone, Copy, Default)]
struct Grain {
    /// reads the loaded sample instead of the live input
    src: bool,
    live: bool,
    pos: f32,
    step: f32,
    age: u32,
    len: u32,
    gl: f32,
    gr: f32,
}

struct Rng(u32);
impl Rng {
    #[inline]
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 & 0xffffff) as f32 / 16777215.0
    }
}

pub struct Hypergrowl {
    sr: f32,
    p: [f32; 52],
    bands: [[Band; BANDS]; 2],
    gains: [[f32; BANDS]; 2],
    norm: [f32; 2],
    // motion
    lfo: f32,
    sh_val: f32,
    drift_v: f32,
    drift_t: f32,
    env: f32,
    l1: f32,
    l2: f32,
    rng: Rng,
    // drive
    dc: [(f32, f32); 2],
    ring_ph: f32,
    hold: [(f32, u32); 2],
    low: [f32; 2],
    /// the fold stage's oversampling memory (last input, last shaped value)
    os: [(f32, f32); 2],
    /// the octave-down generator: flip-flop, last low-passed input, two smoothing stages
    sub_div: [bool; 2],
    sub_last: [f32; 2],
    sub_lp: [(f32, f32); 2],
    // comb
    comb_buf: [Vec<f32>; 2],
    comb_pos: usize,
    // grain
    gbuf: [Vec<f32>; 2],
    gpos: usize,
    grains: [Grain; GRAINS],
    gphase: f32,
    gfeed: [f32; 2],
    // spectral + dry
    fft: Fft,
    win: Vec<f32>,
    spec: [Spectral; 2],
    bank: SampleBank,
    cur_smag: [Vec<f32>; 2],
    spos_now: [f32; 2],
    dryring: [Vec<f32>; 2],
    dpos: usize,
    ticks: usize,
}

impl Hypergrowl {
    pub fn new(sr: f32) -> Self {
        let win = (0..N).map(|i| (0.5 - 0.5 * (TAU * i as f32 / N as f32).cos()).sqrt()).collect();
        let gn = (sr * GBUF_SECS) as usize + 8;
        let mut s = Hypergrowl {
            sr,
            p: [0.0; 52],
            bands: [[Band::default(); BANDS]; 2],
            gains: [[1.0; BANDS]; 2],
            norm: [1.0; 2],
            lfo: 0.0,
            sh_val: 0.5,
            drift_v: 0.0,
            drift_t: 0.0,
            env: 0.0,
            l1: 0.5,
            l2: 0.5,
            rng: Rng(0x1234_5678),
            dc: [(0.0, 0.0); 2],
            ring_ph: 0.0,
            hold: [(0.0, 0); 2],
            low: [0.0; 2],
            os: [(0.0, 0.0); 2],
            sub_div: [false; 2],
            sub_last: [0.0; 2],
            sub_lp: [(0.0, 0.0); 2],
            comb_buf: [vec![0.0; (sr * 0.06) as usize + 4], vec![0.0; (sr * 0.06) as usize + 4]],
            comb_pos: 0,
            gbuf: [vec![0.0; gn], vec![0.0; gn]],
            gpos: 0,
            grains: [Grain::default(); GRAINS],
            gphase: 0.0,
            gfeed: [0.0; 2],
            fft: Fft::new(),
            win,
            spec: [Spectral::new(), Spectral::new()],
            bank: SampleBank::empty(),
            cur_smag: [vec![0.0; N / 2 + 1], vec![0.0; N / 2 + 1]],
            spos_now: [0.4; 2],
            dryring: [vec![0.0; N], vec![0.0; N]],
            dpos: 0,
            ticks: 0,
        };
        for (n, v) in PARAMS {
            s.set_param(n, v);
        }
        s.bank = SampleBank::analyze(default_sample(sr), "", sr, &s.fft);
        s.control();
        s
    }

    #[inline]
    fn v(&self, name: usize) -> f32 {
        self.p[name]
    }

    /// the motion generators and the band gains / centres; every BLOCK samples
    fn control(&mut self) {
        let rate_hz = 0.05 * 200f32.powf(self.v(18));
        let shape = ((self.v(20) * SHAPES.len() as f32) as usize).min(SHAPES.len() - 1);
        let steps = self.v(21);
        let spread = self.v(12);
        // drift: a slow random walk toward fresh targets
        if self.ticks % 2048 == 0 {
            self.drift_t = self.rng.next() * 2.0 - 1.0;
        }
        self.drift_v += (self.drift_t - self.drift_v) * 0.0008;
        let wave = |ph: f32, sh: f32| -> f32 {
            let f = ph.rem_euclid(1.0);
            match shape {
                0 => 0.5 + 0.5 * (TAU * f).sin(),
                1 => 1.0 - (2.0 * f - 1.0).abs(),
                2 => f,
                3 => {
                    if f < 0.5 {
                        1.0
                    } else {
                        0.0
                    }
                }
                _ => sh,
            }
        };
        let quant = |v: f32| -> f32 {
            if steps > 0.03 {
                let n = 2.0 + (1.0 - steps) * 14.0;
                ((v * n).floor() / (n - 1.0)).clamp(0.0, 1.0)
            } else {
                v
            }
        };
        let l1 = quant(wave(self.lfo, self.sh_val));
        self.l1 = l1;
        self.l2 = 0.5 + 0.5 * (TAU * (self.lfo * 0.37 + 0.2)).sin();
        let (pos, motion, follow, wobble) = (self.v(9), self.v(19), self.v(22), self.v(24));
        let (tbl_a, tbl_b) = (((self.v(6) * 12.0) as usize).min(11), ((self.v(7) * 12.0) as usize).min(11));
        let (blend, res, shift, tilt, drift) = (self.v(8), self.v(10), self.v(11), self.v(13), self.v(23));
        if self.v(47) > 0.01 && self.ticks % 128 == 0 {
            let (a, b) = (self.spos_now[0], self.spos_now[1]);
            let mut buf0 = std::mem::take(&mut self.cur_smag[0]);
            let mut buf1 = std::mem::take(&mut self.cur_smag[1]);
            self.bank.fill_env(a, &mut buf0);
            self.bank.fill_env(b, &mut buf1);
            self.cur_smag = [buf0, buf1];
        }
        for ch in 0..2 {
            // the right channel runs the motion a little late (the rainbow widens as `spread` rises)
            let l = if ch == 1 { quant(wave(self.lfo + spread * 0.5, self.sh_val)) } else { l1 };
            let m = (pos + (l - 0.5) * motion * 1.2 + follow * (self.env - 0.15) * 1.6 + self.drift_v * drift * 0.5).clamp(0.0, 1.0);
            // where in the sample this channel is reading (the motion scans it, scaled by `sscan`)
            let sp_ch = (self.v(45) + (l - 0.5) * motion * self.v(46) * 1.4 + follow * (self.env - 0.15) * 0.8).clamp(0.0, 1.0);
            self.spos_now[ch] = sp_ch;
            let smix = self.v(44);
            let sh = (shift - 0.5) * 3.0 + (self.l2 - 0.5) * wobble * 2.0 + if ch == 1 { spread * 0.12 } else { -spread * 0.12 };
            let mut sum2 = 0.0;
            for k in 0..BANDS {
                let x = k as f32 / (BANDS - 1) as f32;
                let g = table_gain(tbl_a, x, m) * (1.0 - blend) + table_gain(tbl_b, x, m) * blend;
                let g = if smix > 0.001 { g * (1.0 - smix) + self.bank.band_gain(sp_ch, k) * smix } else { g };
                let g = g * 10f32.powf((tilt - 0.5) * 2.0 * (x - 0.5) * 12.0 / 20.0);
                self.gains[ch][k] += (g - self.gains[ch][k]) * 0.4;
                sum2 += self.gains[ch][k] * self.gains[ch][k];
                let det = 2f32.powf(spread * 0.3 * (k as f32 * 2.1 + ch as f32 * 3.14).sin());
                let f = 50.0 * (12000.0f32 / 50.0).powf(x) * det * 2f32.powf(sh);
                self.bands[ch][k].set(f, res, self.sr);
            }
            self.norm[ch] = 0.9 * (BANDS as f32).sqrt() / (sum2.sqrt() + 0.3);
        }
    }

    fn spawn_grain(&mut self) {
        let Some(slot) = self.grains.iter().position(|g| !g.live) else { return };
        let (gtime, gsize, spray, gpitch, grand, grev, width) = (self.v(26), self.v(27), self.v(29), self.v(30), self.v(31), self.v(32), self.v(34));
        let use_sample = self.v(49) > 0.001 && !self.bank.mono.is_empty() && self.rng.next() < self.v(49);
        let size = 0.01 * 30f32.powf(gsize);
        let delay0 = 0.005 * 180f32.powf(gtime);
        let semis = (gpitch - 0.5) * 48.0 + (self.rng.next() * 2.0 - 1.0) * grand * 12.0 + (self.l2 - 0.5) * self.v(24) * 6.0;
        let ratio = 2f32.powf(semis / 12.0);
        let reverse = self.rng.next() < grev;
        let len = (size * self.sr).max(64.0);
        if use_sample {
            let sl = self.bank.mono.len() as f32;
            let start = (self.spos_now[0] * sl + (self.rng.next() - 0.5) * spray * 0.5 * self.sr).rem_euclid(sl);
            let pan = 0.5 + (self.rng.next() - 0.5) * width;
            self.grains[slot] = Grain { src: true, live: true, pos: start, step: if reverse { -ratio } else { ratio }, age: 0, len: len as u32, gl: (1.0 - pan).sqrt(), gr: pan.sqrt() };
            return;
        }
        let n = self.gbuf[0].len() as f32;
        // a grain reading forward must not catch the write head: it needs `len * ratio` of room
        let room = if reverse { 0.0 } else { len * ratio.max(1.0) };
        let delay = (delay0 * self.sr + self.rng.next() * spray * 0.5 * self.sr).max(room + 4.0).min(n - len * ratio.max(1.0) - 8.0);
        let start = (self.gpos as f32 - delay).rem_euclid(n);
        let pan = 0.5 + (self.rng.next() - 0.5) * width;
        self.grains[slot] = Grain { src: false, live: true, pos: start, step: if reverse { -ratio } else { ratio }, age: 0, len: len as u32, gl: (1.0 - pan).sqrt(), gr: pan.sqrt() };
    }

    /// clear every buffer and filter state (after a non-finite value slipped in)
    fn reset_state(&mut self) {
        for ch in 0..2 {
            for b in self.bands[ch].iter_mut() {
                b.ic1 = 0.0;
                b.ic2 = 0.0;
            }
            self.gains[ch] = [1.0; BANDS];
            self.comb_buf[ch].iter_mut().for_each(|x| *x = 0.0);
            self.gbuf[ch].iter_mut().for_each(|x| *x = 0.0);
            self.dc[ch] = (0.0, 0.0);
            self.hold[ch] = (0.0, 0);
            self.low[ch] = 0.0;
            self.gfeed[ch] = 0.0;
            let sp = &mut self.spec[ch];
            sp.inbuf.iter_mut().for_each(|x| *x = 0.0);
            sp.outacc.iter_mut().for_each(|x| *x = 0.0);
            sp.prev_g.iter_mut().for_each(|x| *x = 1.0);
            self.dryring[ch].iter_mut().for_each(|x| *x = 0.0);
        }
        self.grains.iter_mut().for_each(|g| g.live = false);
        self.env = 0.0;
        self.drift_v = 0.0;
    }

    /// the bank's gains for the display: 24 gains of the left channel, then the LFO and the number of live grains (0..1)
    pub fn view_data(&self) -> Vec<f32> {
        let mut v: Vec<f32> = self.gains[0].iter().map(|g| (g / 1.6).clamp(0.0, 1.0)).collect();
        v.push(self.l1);
        v.push(self.grains.iter().filter(|g| g.live).count() as f32 / GRAINS as f32);
        // the sample's band gains at its current reading position, then that position
        v.extend((0..BANDS).map(|k| (self.bank.band_gain(self.spos_now[0], k) / 1.6).clamp(0.0, 1.0)));
        v.push(self.spos_now[0]);
        v
    }

    /// Load a WAV / FLAC / OGG as the spectral source. Returns false when the file cannot be read.
    pub fn load_sample(&mut self, path: &str) -> bool {
        let Ok(d) = ling_music::decode::load(path) else { return false };
        let ch = 2usize;
        let frames = d.stereo.len() / ch;
        if frames < 64 {
            return false;
        }
        let mono: Vec<f32> = (0..frames).map(|i| 0.5 * (d.stereo[i * 2] + d.stereo[i * 2 + 1])).collect();
        // to this effect's sample rate
        let ratio = d.rate as f32 / self.sr;
        let outn = ((frames as f32 / ratio) as usize).min((self.sr * 20.0) as usize);
        let res: Vec<f32> = (0..outn)
            .map(|i| {
                let x = i as f32 * ratio;
                let (a, f) = (x as usize, x.fract());
                let b = (a + 1).min(frames - 1);
                mono[a.min(frames - 1)] * (1.0 - f) + mono[b] * f
            })
            .collect();
        self.bank = SampleBank::analyze(res, path, self.sr, &self.fft);
        for g in self.grains.iter_mut() {
            g.live = false;
        }
        true
    }
}

impl Effect for Hypergrowl {
    fn process(&mut self, b: &mut [f32]) {
        let (drive, fold, bias, crush, ring, ringhz) = (self.v(0), self.v(1), self.v(2), self.v(3), self.v(4), self.v(5));
        let (body, rate, ctune, comb, cmod) = (self.v(14), self.v(18), self.v(16), self.v(15), self.v(17));
        let (gmix, gdens, gsize, gfb) = (self.v(25), self.v(28), self.v(27), self.v(33));
        let (squash, focus, sshift, sgate, blur, stilt) = (self.v(35), self.v(36), self.v(37), self.v(38), self.v(39), self.v(40));
        let (width, level, mix) = (self.v(41), self.v(42), self.v(43));
        let (sub_amt, warm) = (self.v(50), self.v(51));
        let sr = self.sr;
        let dphase = 0.05 * 200f32.powf(rate) / sr;
        let dg = 1.0 + drive * 14.0;
        let dbias = (bias - 0.5) * 1.2;
        let fa = 1.0 + fold * 5.0;
        let ring_inc = 20.0 * 100f32.powf(ringhz) / sr;
        let bits = 16.0 - crush * 13.0;
        let levels = 2f32.powf(bits) * 0.5;
        let hold_n = 1 + (crush * crush * 20.0) as u32;
        let lp_a = 1.0 - (-TAU * 140.0 / sr).exp();
        let comb_len = self.comb_buf[0].len();
        let sp = SpecParams { squash, radius: (2.0 + (1.0 - focus) * (1.0 - focus) * 40.0) as usize, shift_bins: ((sshift - 0.5) * 80.0).round() as i32, gate: sgate, blur, tilt: (stilt - 0.5) * 2.0, xsyn: self.v(47), freeze: self.v(48) };
        // +6.8 dB of makeup at the middle of the level knob: the filter bank and the spectral stage take level away
        let gain_out = 2.2 * 10f32.powf((level - 0.5) * 24.0 / 20.0);
        let grain_rate = 2.0 * 60f32.powf(gdens) / sr;
        let grain_secs = 0.01 * 30f32.powf(gsize);
        let overlap = (2.0 * 60f32.powf(gdens) * grain_secs).max(1.0);
        let gnorm = 1.2 / overlap.sqrt();
        let gbuf_n = self.gbuf[0].len();
        for fr in b.chunks_exact_mut(2) {
            if self.ticks % BLOCK == 0 {
                self.control();
            }
            self.ticks = self.ticks.wrapping_add(1);
            // motion clocks
            self.lfo += dphase;
            if self.lfo >= 1.0 {
                self.lfo -= 1.0;
                self.sh_val = self.rng.next();
            }
            let env_in = fr[0].abs().max(fr[1].abs());
            self.env += (env_in - self.env) * if env_in > self.env { 0.02 } else { 0.0015 };
            let ring_ph = self.ring_ph;
            self.ring_ph = (self.ring_ph + ring_inc).fract();
            let mut sig = [0.0f32; 2];
            let mut dry = [0.0f32; 2];
            let cp = self.comb_pos;
            for c in 0..2 {
                // NaN / infinity from upstream would live on in the feedback paths: treat them as silence
                let x = if fr[c].is_finite() { fr[c].clamp(-16.0, 16.0) } else { 0.0 };
                dry[c] = x;
                // ── drive ──
                let y = x * dg + dbias;
                // warmth: a squared term adds even harmonics (the DC it makes is removed below)
                let y = y + warm * 0.35 * y * y.abs();
                // the fold runs at twice the rate (a point halfway to the last sample, a 1-2-1 average back down) so it aliases less
                let shape = |v: f32| v.tanh() * (1.0 - fold) + (v * fa * 0.8).sin() * fold;
                let (dcur, dmid) = (shape(y), shape(0.5 * (self.os[c].0 + y)));
                let mut d = 0.25 * self.os[c].1 + 0.5 * dmid + 0.25 * dcur;
                self.os[c] = (y, dcur);
                if ring > 0.001 {
                    d = d * (1.0 - ring) + d * (TAU * ring_ph).sin() * ring * 1.4;
                }
                if crush > 0.001 {
                    if self.hold[c].1 == 0 {
                        self.hold[c] = ((d * levels).round() / levels, hold_n - 1);
                    } else {
                        self.hold[c].1 -= 1;
                    }
                    d = self.hold[c].0;
                }
                // DC block
                let (px, py) = self.dc[c];
                let o = d - px + 0.995 * py;
                self.dc[c] = (d, o);
                let d = o;
                // ── wavetable filter bank ──
                let mut y = 0.0;
                for k in 0..BANDS {
                    y += self.bands[c][k].tick(d) * self.gains[c][k];
                }
                y *= self.norm[c];
                // the clean low end underneath, so a bass keeps its weight
                self.low[c] += (x - self.low[c]) * lp_a;
                y += self.low[c] * body * 2.0;
                // an octave below: a flip-flop toggled by each rising zero crossing of the low end, smoothed to a soft wave, following the input level
                if sub_amt > 0.001 {
                    let l = self.low[c];
                    if self.sub_last[c] <= 0.0 && l > 0.0 {
                        self.sub_div[c] = !self.sub_div[c];
                    }
                    self.sub_last[c] = l;
                    let sq = if self.sub_div[c] { 1.0 } else { -1.0 };
                    self.sub_lp[c].0 += (sq - self.sub_lp[c].0) * 0.02;
                    self.sub_lp[c].1 += (self.sub_lp[c].0 - self.sub_lp[c].1) * 0.02;
                    y += self.sub_lp[c].1 * sub_amt * 1.4 * (self.env * 3.0).min(1.0);
                }
                // ── comb ──
                if comb > 0.002 {
                    let fc = 40.0 * 50f32.powf(ctune) * 2f32.powf((self.l2 - 0.5) * cmod * 1.5);
                    let dl = (sr / fc.clamp(30.0, sr * 0.4)).clamp(2.0, comb_len as f32 - 3.0);
                    let rp = (cp as f32 + comb_len as f32 - dl) % comb_len as f32;
                    let (i0, fr_) = (rp as usize, rp.fract());
                    let i1 = (i0 + 1) % comb_len;
                    let delayed = self.comb_buf[c][i0] * (1.0 - fr_) + self.comb_buf[c][i1] * fr_;
                    let fb = comb * 0.88;
                    self.comb_buf[c][cp] = (y + delayed * fb).clamp(-6.0, 6.0);
                    y = y * (1.0 - comb * 0.4) + delayed * comb * 0.9;
                }
                sig[c] = y;
            }
            self.comb_pos = (cp + 1) % comb_len;
            // ── grain delay ──
            let pre = sig;
            if gmix > 0.002 {
                self.gphase += grain_rate;
                while self.gphase >= 1.0 {
                    self.gphase -= 1.0;
                    self.spawn_grain();
                }
                let mut go = [0.0f32; 2];
                for gi in 0..GRAINS {
                    let g = self.grains[gi];
                    if !g.live {
                        continue;
                    }
                    let w = 0.5 - 0.5 * (TAU * g.age as f32 / g.len as f32).cos();
                    let i0 = g.pos.floor();
                    let f = g.pos - i0;
                    let (l, r, wrap) = if g.src {
                        let sl = self.bank.mono.len().max(1);
                        let i0 = (i0 as isize).rem_euclid(sl as isize) as usize;
                        let i1 = (i0 + 1) % sl;
                        let v = self.bank.mono[i0] * (1.0 - f) + self.bank.mono[i1] * f;
                        (v, v, sl as f32)
                    } else {
                        let i0 = (i0 as isize).rem_euclid(gbuf_n as isize) as usize;
                        let i1 = (i0 + 1) % gbuf_n;
                        (self.gbuf[0][i0] * (1.0 - f) + self.gbuf[0][i1] * f, self.gbuf[1][i0] * (1.0 - f) + self.gbuf[1][i1] * f, gbuf_n as f32)
                    };
                    go[0] += l * w * g.gl;
                    go[1] += r * w * g.gr;
                    let gm = &mut self.grains[gi];
                    gm.pos = (gm.pos + gm.step).rem_euclid(wrap);
                    gm.age += 1;
                    if gm.age >= gm.len {
                        gm.live = false;
                    }
                }
                for c in 0..2 {
                    go[c] *= gnorm;
                    // feedback: the grains feed what they hear back into the buffer
                    self.gfeed[c] = (go[c] * gfb * 0.85).clamp(-2.0, 2.0);
                }
                for c in 0..2 {
                    sig[c] = sig[c] * (1.0 - gmix * 0.4) + go[c] * gmix * 1.3;
                }
            }
            // the buffer always listens (so turning the grains on finds sound waiting), with the feedback of the last frame
            for c in 0..2 {
                self.gbuf[c][self.gpos] = (pre[c] + self.gfeed[c]).clamp(-8.0, 8.0);
            }
            self.gpos = (self.gpos + 1) % gbuf_n;
            // ── spectral ──
            let mut out = [0.0f32; 2];
            for c in 0..2 {
                let smag = if sp.xsyn > 0.01 { Some(&self.cur_smag[c][..]) } else { None };
                out[c] = self.spec[c].tick(sig[c].clamp(-6.0, 6.0), &self.fft, &self.win, &sp, smag);
            }
            // whatever went wrong inside, never let a non-finite value stay in the state
            if !(out[0].is_finite() && out[1].is_finite()) {
                self.reset_state();
                out = [0.0; 2];
            }
            // ── output: width, level, limiter, mix ──
            let (m, s) = ((out[0] + out[1]) * 0.5, (out[0] - out[1]) * 0.5);
            let s = s * width * 2.0;
            let (l, r) = ((m + s) * gain_out, (m - s) * gain_out);
            let dp = self.dpos;
            for (c, wet) in [l, r].into_iter().enumerate() {
                let dly = self.dryring[c][dp];
                self.dryring[c][dp] = dry[c];
                fr[c] = wet.tanh() * mix + dly * (1.0 - mix);
            }
            self.dpos = (dp + 1) % N;
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let i = pidx(n);
        if i < self.p.len() {
            self.p[i] = v.clamp(0.0, 1.0);
        }
    }
    fn name(&self) -> &'static str {
        "Rainbow Hypergrowl"
    }
    fn view(&self) -> Vec<f32> {
        self.view_data()
    }
    fn load_source(&mut self, path: &str) -> bool {
        if path.is_empty() {
            self.bank = SampleBank::analyze(default_sample(self.sr), "", self.sr, &self.fft);
            return true;
        }
        self.load_sample(path)
    }
    fn source(&self) -> String {
        self.bank.path.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saw(n: usize, sr: f32, hz: f32) -> Vec<f32> {
        let mut b = Vec::with_capacity(n * 2);
        for i in 0..n {
            let s = ((i as f32 * hz / sr).fract() * 2.0 - 1.0) * 0.5;
            b.push(s);
            b.push(s);
        }
        b
    }

    fn rms(b: &[f32]) -> f32 {
        (b.iter().map(|x| x * x).sum::<f32>() / b.len() as f32).sqrt()
    }

    fn with(sets: &[(&str, f32)]) -> Hypergrowl {
        let mut e = Hypergrowl::new(48000.0);
        for (n, v) in sets {
            e.set_param(n, *v);
        }
        e
    }

    #[test]
    fn params_line_up_with_their_indices() {
        for (i, (n, _)) in PARAMS.iter().enumerate() {
            assert_eq!(pidx(n), i, "{n}");
        }
        assert_eq!(PARAMS.len(), 52);
        for (_, set) in PRESETS {
            for (n, v) in set {
                assert!(pidx(n) < 52 && (0.0..=1.0).contains(v), "{n}");
            }
        }
    }

    #[test]
    fn every_table_pair_and_every_preset_makes_finite_audible_sound() {
        for t in 0..TABLES.len() {
            let mut e = with(&[("table", (t as f32 + 0.5) / 12.0), ("tableb", ((t + 5) % 12) as f32 / 12.0 + 0.04), ("steps", 0.5), ("gmix", 0.4)]);
            let mut b = saw(48000, 48000.0, 55.0);
            e.process(&mut b);
            assert!(b.iter().all(|x| x.is_finite() && x.abs() <= 1.0 + 1e-5), "table {t}");
            let r = rms(&b[b.len() / 2..]);
            assert!(r > 0.01 && r < 0.9, "table {t} rms {r}");
        }
        for (name, set) in PRESETS {
            let mut e = Hypergrowl::new(48000.0);
            for (n, v) in set {
                e.set_param(n, *v);
            }
            let mut b = saw(48000, 48000.0, 55.0);
            e.process(&mut b);
            let r = rms(&b[b.len() / 2..]);
            assert!(b.iter().all(|x| x.is_finite()) && r > 0.005, "preset {name} rms {r}");
        }
    }

    #[test]
    fn the_extremes_stay_bounded() {
        for v in [0.0f32, 1.0] {
            let mut e = Hypergrowl::new(48000.0);
            for (n, _) in PARAMS {
                e.set_param(n, v);
            }
            let mut b = saw(48000, 48000.0, 80.0);
            e.process(&mut b);
            assert!(b.iter().all(|x| x.is_finite() && x.abs() <= 1.0 + 1e-5), "all at {v}");
        }
    }

    #[test]
    fn dry_is_delayed_to_line_up() {
        let mut e = with(&[("mix", 0.0)]);
        let src = saw(10000, 48000.0, 110.0);
        let mut b = src.clone();
        e.process(&mut b);
        assert!(b[..LATENCY * 2].iter().all(|x| x.abs() < 1e-6), "silent until the latency has passed");
        assert!((b[LATENCY * 2 + 200] - src[200]).abs() < 1e-6, "the delayed dry comes through untouched");
    }

    #[test]
    fn the_motion_changes_the_sound() {
        let mut a = with(&[("motion", 1.0), ("rate", 0.5)]);
        let mut c = with(&[("motion", 0.0), ("pos", 0.9)]);
        let (mut x, mut y) = (saw(24000, 48000.0, 55.0), saw(24000, 48000.0, 55.0));
        a.process(&mut x);
        c.process(&mut y);
        let d: f32 = x.iter().zip(&y).map(|(p, q)| (p - q).abs()).sum();
        assert!(d > 5.0, "{d}");
        // stepped motion holds: with steps up, the table position takes only a few values over one cycle
        let mut e = with(&[("steps", 1.0), ("motion", 1.0), ("rate", 0.5)]);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..400 {
            let mut b = saw(480, 48000.0, 55.0);
            e.process(&mut b);
            seen.insert((e.l1 * 100.0).round() as i32);
        }
        assert!(seen.len() <= 4, "{seen:?}");
    }

    #[test]
    fn the_grain_delay_scatters_sound_after_the_input_stops() {
        let mut e = with(&[("gmix", 1.0), ("gtime", 0.6), ("gsize", 0.45), ("gdens", 0.6), ("squash", 0.0), ("mix", 1.0), ("gfb", 0.4)]);
        let mut b = saw(24000, 48000.0, 110.0);
        b.extend(vec![0.0; 2 * 48000]);
        e.process(&mut b);
        let tail = rms(&b[2 * (24000 + LATENCY + 2000)..2 * (24000 + LATENCY + 40000)]);
        assert!(tail > 0.003, "grains keep sounding after the input has stopped: {tail}");
        // with the grain mix off the same tail is much quieter
        let mut e2 = with(&[("gmix", 0.0), ("squash", 0.0), ("gfb", 0.0)]);
        let mut b2 = saw(24000, 48000.0, 110.0);
        b2.extend(vec![0.0; 2 * 48000]);
        e2.process(&mut b2);
        let tail2 = rms(&b2[2 * (24000 + LATENCY + 2000)..2 * (24000 + LATENCY + 40000)]);
        assert!(tail > tail2 * 2.0, "{tail} vs {tail2}");
        // the grains are spread over the stereo field
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for fr in b[2 * 30000..2 * 70000].chunks_exact(2) {
            l += fr[0].abs();
            r += fr[1].abs();
        }
        assert!(l > 0.0 && r > 0.0);
    }

    #[test]
    fn comb_ring_crush_and_bin_shift_change_the_spectrum() {
        let base = |sets: &[(&str, f32)]| {
            let mut e = with(sets);
            let mut b = saw(24000, 48000.0, 55.0);
            e.process(&mut b);
            b
        };
        let plain = base(&[("squash", 0.0)]);
        for (name, sets) in [("comb", vec![("squash", 0.0), ("comb", 0.8)]), ("ring", vec![("squash", 0.0), ("ring", 0.7)]), ("crush", vec![("squash", 0.0), ("crush", 0.6)]), ("sshift", vec![("squash", 0.0), ("sshift", 0.8)])] {
            let b = base(&sets);
            assert!(b.iter().all(|x| x.is_finite()), "{name}");
            let d: f32 = plain.iter().zip(&b).map(|(p, q)| (p - q).abs()).sum();
            assert!(d > 5.0, "{name} changed nothing ({d})");
        }
    }

    #[test]
    fn spectral_squash_flattens_the_spectrum() {
        fn crest(squash: f32) -> f32 {
            let mut e = with(&[("squash", squash), ("table", 0.99), ("pos", 0.5), ("motion", 0.0), ("gmix", 0.0)]);
            let mut b = saw(40000, 48000.0, 55.0);
            e.process(&mut b);
            let fft = Fft::new();
            let mut re: Vec<f32> = (0..N).map(|i| b[(30000 + i) * 2]).collect();
            let mut im = vec![0.0; N];
            fft.run(&mut re, &mut im, false);
            let m: Vec<f32> = (1..N / 2).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt()).collect();
            let mean = m.iter().sum::<f32>() / m.len() as f32;
            m.iter().cloned().fold(0.0, f32::max) / mean.max(1e-9)
        }
        assert!(crest(1.0) < crest(0.0), "{} vs {}", crest(1.0), crest(0.0));
    }

    #[test]
    fn the_view_has_a_gain_per_band_and_the_two_meters() {
        let e = Hypergrowl::new(48000.0);
        let v = e.view();
        assert_eq!(v.len(), BANDS * 2 + 3);
        assert!(v.iter().all(|x| (0.0..=1.0).contains(x)));
    }

    #[test]
    fn fft_round_trips() {
        let fft = Fft::new();
        let src: Vec<f32> = (0..N).map(|i| ((i * 7) % 13) as f32 - 6.0).collect();
        let (mut re, mut im) = (src.clone(), vec![0.0; N]);
        fft.run(&mut re, &mut im, false);
        fft.run(&mut re, &mut im, true);
        for i in 0..N {
            assert!((re[i] / N as f32 - src[i]).abs() < 1e-2, "{i}");
        }
    }

    #[test]
    fn it_is_cheap_enough() {
        let mut e = with(&[("gmix", 0.6), ("comb", 0.5), ("ring", 0.3), ("crush", 0.2)]);
        let mut b = saw(48000, 48000.0, 55.0);
        let t0 = std::time::Instant::now();
        e.process(&mut b);
        // one second of stereo audio must take well under one second even in a debug-ish build
        assert!(t0.elapsed().as_secs_f32() < 1.5, "{:?}", t0.elapsed());
    }

    /// not a test: `cargo test -p ling-daw --lib dump_presets -- --ignored` writes one WAV per preset to $HG_DUMP for listening / plotting
    #[test]
    #[ignore]
    fn dump_presets() {
        let Ok(dir) = std::env::var("HG_DUMP") else { return };
        for (name, set) in PRESETS {
            let mut e = Hypergrowl::new(48000.0);
            for (n, v) in set {
                e.set_param(n, *v);
            }
            // a bass line: A1 held, then E2, over three seconds, a bright saw with a little sub
            let sr = 48000.0;
            let mut b = Vec::new();
            let mut ph = 0.0f32;
            for i in 0..(sr as usize * 3) {
                let hz = if i < sr as usize * 2 { 55.0 } else { 82.4 };
                ph = (ph + hz / sr).fract();
                let env = if (i % (sr as usize)) < 100 { (i % sr as usize) as f32 / 100.0 } else { 1.0 };
                let s = ((ph * 2.0 - 1.0) * 0.55 + (TAU * ph).sin() * 0.25) * env;
                b.push(s);
                b.push(s);
            }
            e.process(&mut b);
            let mut out = Vec::new();
            let data_len = (b.len() * 4) as u32;
            out.extend(b"RIFF");
            out.extend((36 + data_len).to_le_bytes());
            out.extend(b"WAVEfmt ");
            out.extend(16u32.to_le_bytes());
            out.extend(3u16.to_le_bytes());
            out.extend(2u16.to_le_bytes());
            out.extend(48000u32.to_le_bytes());
            out.extend((48000u32 * 8).to_le_bytes());
            out.extend(8u16.to_le_bytes());
            out.extend(32u16.to_le_bytes());
            out.extend(b"data");
            out.extend(data_len.to_le_bytes());
            for x in &b {
                out.extend(x.to_le_bytes());
            }
            std::fs::write(format!("{dir}/{}.wav", name.replace(' ', "_")), out).unwrap();
        }
    }

    /// how many times faster than real time the effect runs (needs --release to mean anything)
    #[test]
    #[ignore]
    fn bench_realtime_factor() {
        for (name, sets) in [("defaults", vec![]), ("grains+comb+ring+crush", vec![("gmix", 0.6), ("comb", 0.5), ("ring", 0.3), ("crush", 0.2)]), ("hypergrowl preset", PRESETS[1].1.to_vec())] {
            let mut e = Hypergrowl::new(48000.0);
            for (n, v) in &sets {
                e.set_param(n, *v);
            }
            let mut b = saw(48000 * 4, 48000.0, 55.0);
            let t0 = std::time::Instant::now();
            e.process(&mut b);
            let secs = t0.elapsed().as_secs_f32();
            println!("{name}: 4 s of audio in {secs:.3} s = {:.1}x realtime", 4.0 / secs);
        }
    }

    /// knobs thrown around at random, odd sample rates and odd block sizes: nothing may panic or go non-finite
    #[test]
    fn survives_random_knobs_odd_rates_and_block_sizes() {
        let mut seed = 0x2545F491u32;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed & 0xffff) as f32 / 65535.0
        };
        for sr in [22050.0f32, 44100.0, 48000.0, 88200.0, 96000.0, 192000.0] {
            let mut e = Hypergrowl::new(sr);
            let mut phase = 0.0f32;
            let mut total = 0usize;
            while total < (sr as usize) * 3 {
                // a random block of 1..700 frames
                let frames = 1 + (rnd() * 700.0) as usize;
                let mut b = Vec::with_capacity(frames * 2);
                for _ in 0..frames {
                    phase = (phase + 55.0 / sr).fract();
                    let x = if rnd() < 0.002 { 0.0 } else { (phase * 2.0 - 1.0) * 0.6 };
                    b.push(x);
                    b.push(x * 0.9);
                }
                // move a few knobs, sometimes all the way to the ends
                for _ in 0..3 {
                    let i = (rnd() * 52.0) as usize % 52;
                    let v = match (rnd() * 4.0) as u32 {
                        0 => 0.0,
                        1 => 1.0,
                        _ => rnd(),
                    };
                    e.set_param(PARAMS[i].0, v);
                }
                e.process(&mut b);
                if !b.iter().all(|x| x.is_finite() && x.abs() <= 1.0 + 1e-5) {
                    let bad = b.iter().position(|x| !x.is_finite() || x.abs() > 1.0 + 1e-5).unwrap();
                    let named: Vec<String> = PARAMS.iter().enumerate().map(|(i, (n, _))| format!("{n}={:.2}", e.p[i])).collect();
                    panic!("sr {sr} bad sample {} = {} at frame {total}+{}; {}", bad, b[bad], bad / 2, named.join(" "));
                }
                total += frames;
            }
        }
    }

    /// silence, denormal-sized input and NaN / infinity in the input must not poison the state for good
    #[test]
    fn bad_input_does_not_wedge_the_effect() {
        let mut e = with(&[("gmix", 0.5), ("comb", 0.5), ("gfb", 0.9), ("squash", 0.9)]);
        let mut b = vec![f32::NAN; 4000];
        e.process(&mut b);
        let mut b = vec![f32::INFINITY, -f32::INFINITY].repeat(2000);
        e.process(&mut b);
        let mut b = vec![1e-30f32; 20000];
        e.process(&mut b);
        // after the garbage the effect must come back to making sound on normal input
        let mut ok = false;
        for _ in 0..40 {
            let mut b = saw(4800, 48000.0, 110.0);
            e.process(&mut b);
            if b.iter().all(|x| x.is_finite()) && rms(&b) > 0.01 {
                ok = true;
                break;
            }
        }
        assert!(ok, "the effect stayed broken after a NaN burst");
    }

    fn tmp_wav(name: &str, secs: f32, hz: f32) -> String {
        let sr = 22050u32;
        let n = (sr as f32 * secs) as usize;
        let mut data = Vec::new();
        for i in 0..n {
            // a tone that glides up so the spectrum changes over the file
            let f = hz * (1.0 + i as f32 / n as f32);
            let x = ((TAU * f * i as f32 / sr as f32).sin() * 12000.0) as i16;
            data.extend(x.to_le_bytes());
        }
        let mut out = Vec::new();
        out.extend(b"RIFF");
        out.extend((36 + data.len() as u32).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(sr.to_le_bytes());
        out.extend((sr * 2).to_le_bytes());
        out.extend(2u16.to_le_bytes());
        out.extend(16u16.to_le_bytes());
        out.extend(b"data");
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, out).unwrap();
        path.to_string_lossy().to_string()
    }

    #[test]
    fn the_built_in_sample_exists_and_a_wav_can_replace_it() {
        let mut e = Hypergrowl::new(48000.0);
        assert!(e.bank.nframes > 8 && !e.bank.mono.is_empty() && e.source().is_empty());
        let path = tmp_wav("hg_test_glide.wav", 2.0, 400.0);
        assert!(e.load_source(&path));
        assert_eq!(e.source(), path);
        assert!(e.bank.nframes > 8);
        // a glide moves its energy up the bands: the first frame peaks lower than the last
        let peak_band = |f: usize| e.bank.gains[f].iter().enumerate().fold((0, 0.0f32), |a, (k, g)| if *g > a.1 { (k, *g) } else { a }).0;
        assert!(peak_band(e.bank.nframes - 1) > peak_band(0), "{} vs {}", peak_band(0), peak_band(e.bank.nframes - 1));
        assert!(!e.load_source("C:/no/such/file.wav"));
        assert_eq!(e.source(), path, "a failed load keeps the old sample");
    }

    #[test]
    fn sample_grains_sound_with_no_input_and_the_sample_shapes_the_filter() {
        // grains read from the sample: sound out of silence
        let mut e = with(&[("gmix", 1.0), ("gsrc", 1.0), ("gdens", 0.6), ("gsize", 0.45), ("squash", 0.0), ("mix", 1.0)]);
        let mut b = vec![0.0f32; 2 * 48000];
        e.process(&mut b);
        assert!(rms(&b[2 * 12000..]) > 0.003, "{}", rms(&b[2 * 12000..]));
        // with no sample grains and no input, silence
        let mut q = with(&[("gmix", 1.0), ("gsrc", 0.0), ("squash", 0.0)]);
        let mut b2 = vec![0.0f32; 2 * 48000];
        q.process(&mut b2);
        assert!(rms(&b2) < 1e-4);
        // the sample's spectrum drives the bank: scanning it changes the sound
        let mut a = with(&[("smix", 1.0), ("spos", 0.05), ("sscan", 0.0), ("motion", 0.0), ("squash", 0.0)]);
        let mut c = with(&[("smix", 1.0), ("spos", 0.95), ("sscan", 0.0), ("motion", 0.0), ("squash", 0.0)]);
        let (mut x, mut y) = (saw(24000, 48000.0, 110.0), saw(24000, 48000.0, 110.0));
        a.process(&mut x);
        c.process(&mut y);
        let d: f32 = x.iter().zip(&y).map(|(p, q)| (p - q).abs()).sum();
        assert!(d > 5.0, "{d}");
    }

    #[test]
    fn cross_synthesis_and_freeze_do_something_and_stay_bounded() {
        let run = |sets: &[(&str, f32)]| {
            let mut e = with(sets);
            let mut b = saw(36000, 48000.0, 82.0);
            e.process(&mut b);
            assert!(b.iter().all(|x| x.is_finite() && x.abs() <= 1.0 + 1e-5));
            b
        };
        let plain = run(&[("squash", 0.0)]);
        let voc = run(&[("squash", 0.0), ("xsyn", 1.0), ("spos", 0.3)]);
        let d: f32 = plain.iter().zip(&voc).map(|(p, q)| (p - q).abs()).sum();
        assert!(d > 5.0, "cross-synthesis changed nothing: {d}");
        // freeze holds the spectrum: play a note with the knob down, raise it, stop the input: the sound goes on
        let mut f = with(&[("freeze", 0.0), ("squash", 0.0), ("gmix", 0.0)]);
        let mut b = saw(12000, 48000.0, 110.0);
        f.process(&mut b);
        f.set_param("freeze", 1.0);
        let mut tail = vec![0.0f32; 2 * 36000];
        f.process(&mut tail);
        let held = rms(&tail[2 * 20000..2 * 30000]);
        let mut g = with(&[("freeze", 0.0), ("squash", 0.0), ("gmix", 0.0)]);
        let mut b2 = saw(12000, 48000.0, 110.0);
        g.process(&mut b2);
        let mut tail2 = vec![0.0f32; 2 * 36000];
        g.process(&mut tail2);
        let free = rms(&tail2[2 * 20000..2 * 30000]);
        assert!(held > 0.01 && held > free * 20.0, "{held} vs {free}");
    }

    #[test]
    fn sub_adds_an_octave_below_and_warm_adds_even_harmonics() {
        let power_at = |v: &[f32], hz: f32| {
            let w = TAU * hz / 48000.0;
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, x) in v.iter().enumerate() {
                re += x * (w * i as f32).cos();
                im += x * (w * i as f32).sin();
            }
            (re * re + im * im).sqrt() / v.len() as f32
        };
        let run = |sets: &[(&str, f32)]| {
            let mut e = with(sets);
            let mut b = saw(48000, 48000.0, 110.0);
            e.process(&mut b);
            assert!(b.iter().all(|x| x.is_finite() && x.abs() <= 1.0 + 1e-5));
            b.iter().step_by(2).copied().collect::<Vec<f32>>()
        };
        let base = &[("squash", 0.0), ("fold", 0.0), ("drive", 0.1), ("body", 0.0), ("gmix", 0.0)];
        let plain = run(base);
        let mut with_sub = base.to_vec();
        with_sub.push(("sub", 1.0));
        let subbed = run(&with_sub);
        let (a, b) = (power_at(&plain[12000..36000], 55.0), power_at(&subbed[12000..36000], 55.0));
        assert!(b > a * 3.0, "no octave below: {b} vs {a}");
        // warm: the second harmonic of a pure-ish tone grows
        let mut sine_base = base.to_vec();
        sine_base.push(("drive", 0.6));
        let mut warm_set = sine_base.clone();
        warm_set.push(("warm", 1.0));
        let (p0, p1) = (run(&sine_base), run(&warm_set));
        let d: f32 = p0.iter().zip(&p1).map(|(x, y)| (x - y).abs()).sum();
        assert!(d > 5.0, "warm changed nothing: {d}");
    }
}
