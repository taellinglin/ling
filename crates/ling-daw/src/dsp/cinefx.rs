//! Sound-design effects for film, trailer and game work (and anything else):
//!
//! * **Digitize** - a voice turning digital: a telephone / walkie-talkie / AM radio / megaphone band-pass, a low-bitrate "codec"
//!   (short FFT frames whose magnitudes are quantised, whose bins drop out and whose frames hold and smear: the warbling artefacts of
//!   an overloaded MP3 / GSM stream), a robotiser (the frames' phases zeroed, so the voice buzzes at the frame rate), bit-depth and
//!   sample-rate reduction, a ring modulator, a buffer-repeat glitch and a metallic comb. One knob, `transform`, brings the stages in
//!   one after another (band-pass first, the glitches last), so automating it from 0 to 1 over a yell is the moment a voice is
//!   swallowed by the machine; at 0 the signal passes untouched.
//! * **Tape Stop** - the tape (or turntable) stops and starts again: `stop` is the switch (a knob or an automation lane), the
//!   playback slows to a standstill over `time` and spins back up over `start`, pitch and tone falling with the speed.
//! * **Shimmer Verb** - a large eight-line feedback-delay-network reverb whose tail is pitch-shifted (an octave up by default) and
//!   fed back, so every repeat climbs: the cinematic halo. `freeze` holds the tail forever.
//! * **Granular Cloud** - the input is recorded into a few seconds of buffer and played back as a cloud of short grains: size,
//!   density, pitch and its spread, position jitter, reverse grains and feedback; `freeze` stops the recording, so the cloud keeps
//!   playing what it holds - any sound becomes a soundscape.
//!
//! The reverb (`Fdn`) and the pitch shifter (`PShift`) are shared with Ling Cinema (cinema.rs). Everything is allocation-free in
//! `process`, finite for any input and setting, and deterministic (the random choices come from a seeded hash), so a render sounds
//! like playback.
use super::bassfx::{db2lin, fin, g_of, Line, Tsvf};
use super::fx::Effect;
use crate::dsp::TAU;

type Pre = (&'static str, &'static [(&'static str, f32)]);

#[inline]
fn choice(v: f32, n: usize) -> usize {
    ((v.clamp(0.0, 1.0) * n as f32) as usize).min(n - 1)
}

/// 0 below `a`, 1 above `b`, a smooth step between
#[inline]
fn ramp(x: f32, a: f32, b: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// a hash of a counter and a seed, 0..1
#[inline]
pub(crate) fn hash01(k: u64, seed: u32) -> f32 {
    let mut x = k.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (seed as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

// ── shared parts ────────────────────────────────────────────────────────────────

/// An eight-line feedback delay network (Hadamard mixing, a damping low-pass and a decay gain per line, two lines slowly
/// modulated): a smooth, dense hall that can hold its tail forever (`freeze`).
pub struct Fdn {
    lines: Vec<Line>,
    len: [f32; 8],
    g: [f32; 8],
    lp: [f32; 8],
    damp: f32,
    sr: f32,
    ph: f32,
    pub modu: f32,
    pub freeze: bool,
    /// extra loss per pass (0..1 of the energy): what a feedback path from the output back in (the shimmer) adds, taken off
    /// the lines so the loop as a whole still decays
    pub loss: f32,
}

const FDN_MS: [f32; 8] = [29.7, 37.1, 41.1, 43.7, 53.0, 59.9, 67.7, 73.1];

impl Fdn {
    pub fn new(sr: f32) -> Fdn {
        let n = (sr * 0.36) as usize;
        let mut f = Fdn { lines: (0..8).map(|_| Line::new(n)).collect(), len: [1.0; 8], g: [0.0; 8], lp: [0.0; 8], damp: 0.3, sr, ph: 0.0, modu: 0.3, freeze: false, loss: 0.0 };
        f.set(0.6, 2.5, 0.3);
        f
    }
    /// size 0..1 (the delay lengths, x0.4 .. x4), decay in seconds (RT60), damping 0..1
    pub fn set(&mut self, size: f32, rt60: f32, damp: f32) {
        let k = 0.4 + size.clamp(0.0, 1.0) * 3.6;
        let rt = rt60.clamp(0.1, 60.0);
        for i in 0..8 {
            self.len[i] = (FDN_MS[i] * k * 0.001 * self.sr).min(self.sr * 0.34);
            self.g[i] = 10f32.powf(-3.0 * (self.len[i] / self.sr) / rt);
        }
        self.damp = damp.clamp(0.0, 0.95);
    }
    /// one stereo sample in, the reverb out (wet only)
    #[inline]
    pub fn run(&mut self, l: f32, r: f32) -> (f32, f32) {
        self.ph = (self.ph + 0.37 / self.sr).fract();
        let m = (self.ph * TAU).sin() * self.modu * 0.0016 * self.sr;
        let mut x = [0.0f32; 8];
        for i in 0..8 {
            let d = self.len[i] + if i == 1 { m } else if i == 6 { -m } else { 0.0 };
            x[i] = self.lines[i].read(d.max(2.0));
        }
        let out_l = (x[0] + x[2] + x[4] + x[6]) * 0.35;
        let out_r = (x[1] + x[3] + x[5] + x[7]) * 0.35;
        let (damp, freeze) = (if self.freeze { 0.0 } else { self.damp }, self.freeze);
        let keep = (1.0 - self.loss.clamp(0.0, 0.9)).sqrt();
        for i in 0..8 {
            self.lp[i] += (x[i] - self.lp[i]) * (1.0 - damp);
            x[i] = self.lp[i] * if freeze { 1.0 } else { self.g[i] * keep };
        }
        // a fast Walsh-Hadamard transform, scaled to stay orthonormal
        let mut h = 1;
        while h < 8 {
            let mut i = 0;
            while i < 8 {
                for j in i..i + h {
                    let (a, b) = (x[j], x[j + h]);
                    x[j] = a + b;
                    x[j + h] = a - b;
                }
                i += h * 2;
            }
            h *= 2;
        }
        let s = 0.353_553_4;
        let gin = if freeze { 0.0 } else { 0.35 };
        for i in 0..8 {
            let inp = if i % 2 == 0 { l } else { r };
            self.lines[i].push(x[i] * s + inp * gin);
        }
        (fin(out_l), fin(out_r))
    }
}

/// A delay-line pitch shifter: two read taps sweep through a short window in opposite phase, crossfaded with sin^2 / cos^2.
pub struct PShift {
    line: Line,
    ph: f32,
    win: f32,
}
impl PShift {
    pub fn new(sr: f32, win_s: f32) -> PShift {
        let win = (sr * win_s).max(64.0);
        PShift { line: Line::new(win as usize + 16), ph: 0.0, win }
    }
    /// one sample through, played at `ratio` times the speed (2 = an octave up)
    #[inline]
    pub fn run(&mut self, x: f32, ratio: f32) -> f32 {
        self.line.push(x);
        self.ph = (self.ph + (1.0 - ratio) / self.win).rem_euclid(1.0);
        let p2 = (self.ph + 0.5).fract();
        let a = self.line.read(2.0 + self.ph * self.win);
        let b = self.line.read(2.0 + p2 * self.win);
        let ga = (std::f32::consts::PI * self.ph).sin();
        let gb = (std::f32::consts::PI * p2).sin();
        a * ga * ga + b * gb * gb
    }
}

/// a radix-2 FFT with its tables made once
struct Fft {
    n: usize,
    cs: Vec<f32>,
    sn: Vec<f32>,
    rev: Vec<usize>,
}
impl Fft {
    fn new(n: usize) -> Fft {
        let bits = n.trailing_zeros();
        let rev = (0..n).map(|i| i.reverse_bits() >> (usize::BITS - bits)).collect();
        let cs = (0..n / 2).map(|k| (std::f64::consts::TAU * k as f64 / n as f64).cos() as f32).collect();
        let sn = (0..n / 2).map(|k| (std::f64::consts::TAU * k as f64 / n as f64).sin() as f32).collect();
        Fft { n, cs, sn, rev }
    }
    fn run(&self, re: &mut [f32], im: &mut [f32], inverse: bool) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i];
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let sg = if inverse { 1.0 } else { -1.0 };
        let mut len = 2;
        while len <= n {
            let step = n / len;
            for s in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let (wr, wi) = (self.cs[k * step], sg * self.sn[k * step]);
                    let (a, b) = (s + k, s + k + len / 2);
                    let (xr, xi) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                    re[b] = re[a] - xr;
                    im[b] = im[a] - xi;
                    re[a] += xr;
                    im[a] += xi;
                }
            }
            len <<= 1;
        }
        if inverse {
            let s = 1.0 / n as f32;
            re.iter_mut().for_each(|x| *x *= s);
            im.iter_mut().for_each(|x| *x *= s);
        }
    }
}

// ── Digitize ────────────────────────────────────────────────────────────────────

pub const DG_CHARS: [&str; 5] = ["off", "phone", "walkie", "am radio", "megaphone"];

pub const DIGITIZE_PARAMS: [(&str, f32); 17] = [
    ("transform", 0.7),
    ("character", 0.3),
    ("band", 1.0),
    ("codec", 0.6),
    ("robot", 0.0),
    ("bits", 0.4),
    ("rate", 0.3),
    ("ring", 0.2),
    ("ring hz", 0.4),
    ("glitch", 0.25),
    ("glitch len", 0.3),
    ("comb", 0.25),
    ("comb hz", 0.5),
    ("noise", 0.3),
    ("seed", 0.0),
    ("mix", 1.0),
    ("out", 0.5),
];

pub const DIGITIZE_PRESETS: [Pre; 8] = [
    ("Matrix Mirror", &[("transform", 1.0), ("character", 0.3), ("codec", 0.85), ("robot", 0.35), ("bits", 0.55), ("rate", 0.45), ("ring", 0.35), ("ring hz", 0.3), ("glitch", 0.45), ("glitch len", 0.2), ("comb", 0.4), ("comb hz", 0.55), ("noise", 0.2)]),
    ("Telephone", &[("transform", 0.3), ("character", 0.3), ("codec", 0.0), ("bits", 0.0), ("rate", 0.0), ("ring", 0.0), ("glitch", 0.0), ("comb", 0.0), ("noise", 0.25)]),
    ("Walkie Talkie", &[("transform", 0.45), ("character", 0.5), ("codec", 0.3), ("bits", 0.2), ("rate", 0.1), ("ring", 0.0), ("glitch", 0.0), ("comb", 0.0), ("noise", 0.6)]),
    ("AM Radio", &[("transform", 0.35), ("character", 0.7), ("codec", 0.15), ("bits", 0.0), ("rate", 0.0), ("ring", 0.0), ("glitch", 0.0), ("comb", 0.0), ("noise", 0.5)]),
    ("Robot", &[("transform", 1.0), ("character", 0.0), ("codec", 0.0), ("robot", 1.0), ("out", 0.7), ("bits", 0.25), ("rate", 0.0), ("ring", 0.55), ("ring hz", 0.25), ("glitch", 0.0), ("comb", 0.5), ("comb hz", 0.35), ("noise", 0.0)]),
    ("Data Corrupt", &[("transform", 1.0), ("character", 0.0), ("codec", 1.0), ("bits", 0.8), ("rate", 0.7), ("ring", 0.2), ("ring hz", 0.8), ("glitch", 0.9), ("glitch len", 0.12), ("comb", 0.2), ("noise", 0.1), ("seed", 0.37)]),
    ("Low Bitrate Codec", &[("transform", 0.7), ("character", 0.0), ("codec", 1.0), ("bits", 0.0), ("rate", 0.0), ("ring", 0.0), ("glitch", 0.0), ("comb", 0.0), ("noise", 0.0)]),
    ("Megaphone", &[("transform", 0.4), ("character", 0.9), ("codec", 0.0), ("bits", 0.1), ("rate", 0.0), ("ring", 0.0), ("glitch", 0.0), ("comb", 0.15), ("comb hz", 0.6), ("noise", 0.2)]),
];

const DG_N: usize = 512;
const DG_HOP: usize = 128;

/// a band-pass character: high-pass, low-pass, a resonant peak (Hz, gain), drive, makeup gain (dB), hiss
struct Character {
    hp: f32,
    lp: f32,
    peak: f32,
    peak_g: f32,
    drive: f32,
    makeup: f32,
    hiss: f32,
}
static CHARS: [Character; 5] = [
    Character { hp: 20.0, lp: 20000.0, peak: 1000.0, peak_g: 0.0, drive: 1.0, makeup: 0.0, hiss: 0.0 },
    Character { hp: 300.0, lp: 3400.0, peak: 1500.0, peak_g: 0.4, drive: 1.6, makeup: 5.0, hiss: 0.004 },
    Character { hp: 480.0, lp: 2700.0, peak: 1200.0, peak_g: 0.8, drive: 6.0, makeup: 3.0, hiss: 0.02 },
    Character { hp: 130.0, lp: 4500.0, peak: 900.0, peak_g: 0.3, drive: 1.3, makeup: 3.0, hiss: 0.012 },
    Character { hp: 560.0, lp: 4200.0, peak: 1800.0, peak_g: 2.2, drive: 4.0, makeup: 2.0, hiss: 0.003 },
];

pub struct Digitize {
    sr: f32,
    p: [f32; 17],
    // smoothed stage amounts: band, codec, robot, crush, ring, glitch, comb
    amt: [f32; 7],
    // band-pass
    hp: [[Tsvf; 2]; 2],
    lp: [[Tsvf; 2]; 2],
    pk: [Tsvf; 2],
    wob: f32,
    // the codec: input / output rings, the window, the frames
    fft: Fft,
    win: Vec<f32>,
    inb: [Vec<f32>; 2],
    outb: [Vec<f32>; 2],
    dry_d: [Vec<f32>; 2],
    pos: usize,
    hop_n: usize,
    frame: u64,
    re: Vec<f32>,
    im: Vec<f32>,
    prev: [Vec<f32>; 2],
    held: [Vec<f32>; 2],
    // crush
    hold: [f32; 2],
    hold_t: f32,
    // ring
    ring_ph: f32,
    // glitch
    gbuf: [Vec<f32>; 2],
    gw: usize,
    g_t: usize,
    g_slice: u64,
    g_on: bool,
    g_start: usize,
    g_k: usize,
    // comb
    comb: [Line; 2],
    comb_lp: [f32; 2],
    rng: u32,
    /// the input's level: the radio hiss opens with it (a squelch), so silence stays silent
    env: f32,
    dc: [f32; 2],
}

impl Digitize {
    pub fn new(sr: f32) -> Digitize {
        let win: Vec<f32> = (0..DG_N).map(|i| 0.5 - 0.5 * (TAU * i as f32 / DG_N as f32).cos()).collect();
        let gn = (sr * 0.6) as usize;
        let mut d = Digitize {
            sr,
            p: [0.0; 17],
            amt: [0.0; 7],
            hp: [[Tsvf::default(); 2]; 2],
            lp: [[Tsvf::default(); 2]; 2],
            pk: [Tsvf::default(); 2],
            wob: 0.0,
            fft: Fft::new(DG_N),
            win,
            inb: [vec![0.0; DG_N], vec![0.0; DG_N]],
            outb: [vec![0.0; DG_N], vec![0.0; DG_N]],
            dry_d: [vec![0.0; DG_N], vec![0.0; DG_N]],
            pos: 0,
            hop_n: 0,
            frame: 0,
            re: vec![0.0; DG_N],
            im: vec![0.0; DG_N],
            prev: [vec![0.0; DG_N / 2 + 1], vec![0.0; DG_N / 2 + 1]],
            held: [vec![0.0; DG_N / 2 + 1], vec![0.0; DG_N / 2 + 1]],
            hold: [0.0; 2],
            hold_t: 0.0,
            ring_ph: 0.0,
            gbuf: [vec![0.0; gn], vec![0.0; gn]],
            gw: 0,
            g_t: 0,
            g_slice: 0,
            g_on: false,
            g_start: 0,
            g_k: 0,
            comb: [Line::new((sr / 40.0) as usize + 8), Line::new((sr / 40.0) as usize + 8)],
            comb_lp: [0.0; 2],
            rng: 0x2545_F491,
            env: 0.0,
            dc: [0.0; 2],
        };
        for (i, (_, v)) in DIGITIZE_PARAMS.iter().enumerate() {
            d.p[i] = *v;
        }
        d
    }
    fn get(&self, n: &str) -> f32 {
        DIGITIZE_PARAMS.iter().position(|p| p.0 == n).map(|i| self.p[i]).unwrap_or(0.0)
    }
    /// the stage amounts the knobs and the transform ask for: band, codec, robot, crush, ring, glitch, comb
    pub fn targets(&self) -> [f32; 7] {
        let t = self.get("transform");
        let band = if choice(self.get("character"), 5) == 0 { 0.0 } else { self.get("band") };
        [
            band * ramp(t, 0.0, 0.3),
            self.get("codec") * ramp(t, 0.15, 0.7),
            self.get("robot") * ramp(t, 0.5, 1.0),
            ramp(t, 0.35, 0.9),
            self.get("ring") * ramp(t, 0.5, 1.0),
            self.get("glitch") * ramp(t, 0.6, 1.0),
            self.get("comb") * ramp(t, 0.25, 0.8),
        ]
    }
    #[inline]
    fn noise(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    /// one codec frame on both channels (the input ring is full and `pos` is where it starts)
    fn codec_frame(&mut self, codec: f32, robot: f32) {
        let n2 = DG_N / 2;
        let seed = (self.get("seed") * 1000.0) as u32;
        self.frame += 1;
        let fr = self.frame;
        // hold this frame (repeat the last magnitudes) or smear it into the last one
        let hold = hash01(fr, seed ^ 0x51) < codec * 0.3;
        let smear = codec * 0.55;
        let top = ((n2 as f32) * (1.0 - codec * 0.78)).max(12.0);
        let step_db = codec * 13.0;
        for c in 0..2 {
            for i in 0..DG_N {
                self.re[i] = self.inb[c][(self.pos + i) % DG_N] * self.win[i];
                self.im[i] = 0.0;
            }
            self.fft.run(&mut self.re, &mut self.im, false);
            for b in 0..=n2 {
                let (r, im) = (self.re[b], self.im[b]);
                let mut mag = (r * r + im * im).sqrt();
                let (cs, sn) = if mag > 1e-12 { (r / mag, im / mag) } else { (1.0, 0.0) };
                if codec > 0.0 {
                    // the bitrate's bandwidth: bins above the top fade out
                    let bf = b as f32;
                    if bf > top {
                        mag *= (1.0 - (bf - top) / 12.0).max(0.0);
                    }
                    // magnitudes in coarse dB steps
                    if step_db > 0.3 && mag > 1e-9 {
                        let db = 20.0 * mag.log10();
                        mag = 10f32.powf(((db / step_db).round() * step_db) / 20.0);
                    }
                    // bins drop out, more of them up high
                    if hash01(fr * 1031 + b as u64, seed) < codec * 0.55 * (0.25 + 0.75 * bf / n2 as f32) {
                        mag = 0.0;
                    }
                    let pv = self.prev[c][b];
                    mag = mag * (1.0 - smear) + pv * smear;
                    if hold {
                        mag = self.held[c][b];
                    }
                    self.prev[c][b] = mag;
                    if !hold {
                        self.held[c][b] = mag;
                    }
                }
                // the robot: the frame's phases set to zero, crossfaded
                let (mut nr, mut ni) = (mag * cs, mag * sn);
                if robot > 0.0 {
                    // phase zero at the middle of the frame (where the window is open): (-1)^bin
                    let sgn = if b % 2 == 0 { 1.0 } else { -1.0 };
                    nr = nr * (1.0 - robot) + mag * sgn * robot;
                    ni *= 1.0 - robot;
                }
                self.re[b] = nr;
                self.im[b] = ni;
                if b > 0 && b < n2 {
                    self.re[DG_N - b] = nr;
                    self.im[DG_N - b] = -ni;
                }
            }
            self.fft.run(&mut self.re, &mut self.im, true);
            // overlap-add (Hann in and out at a quarter-frame hop sums to 1.5)
            for i in 0..DG_N {
                let o = (self.pos + i) % DG_N;
                if i < DG_N - DG_HOP {
                    self.outb[c][o] += self.re[i] * self.win[i] / 1.5;
                } else {
                    self.outb[c][o] = self.re[i] * self.win[i] / 1.5;
                }
            }
        }
    }
}

impl Effect for Digitize {
    fn process(&mut self, buf: &mut [f32]) {
        let sr = self.sr;
        let tg = self.targets();
        let ka = 1.0 - (-1.0 / (0.02 * sr)).exp();
        let ch = &CHARS[choice(self.get("character"), 5)];
        let (ghp, glp, gpk) = (g_of(ch.hp, sr), g_of(ch.lp, sr), g_of(ch.peak, sr));
        let rate = self.get("rate");
        let bits = self.get("bits");
        let ring_hz = 30.0 * 2f32.powf(self.get("ring hz") * 6.0);
        let comb_hz = 60.0 * 2f32.powf(self.get("comb hz") * 5.0);
        let comb_d = sr / comb_hz;
        let glen = ((0.02 + self.get("glitch len").powi(2) * 0.28) * sr) as usize;
        let glen = glen.clamp(64, self.gbuf[0].len() / 2);
        let seed = (self.get("seed") * 1000.0) as u32;
        let hiss = ch.hiss * (0.2 + self.get("noise") * 1.6);
        let makeup = db2lin(ch.makeup);
        let mix = self.get("mix");
        let out = db2lin((self.get("out") - 0.5) * 24.0);
        let gn = self.gbuf[0].len();
        for fr in buf.chunks_exact_mut(2) {
            for k in 0..7 {
                self.amt[k] += (tg[k] - self.amt[k]) * ka;
                if (self.amt[k] - tg[k]).abs() < 1e-5 {
                    self.amt[k] = tg[k];
                }
            }
            let [band, codec, robot, crush, ring, glitch, comb] = self.amt;
            let dry = [fin(fr[0]), fin(fr[1])];
            let mut x = dry;
            let lv = dry[0].abs().max(dry[1].abs());
            self.env += (lv - self.env) * if lv > self.env { 0.01 } else { 0.0004 };
            // 1. the band-pass character
            if band > 0.0 {
                self.wob = (self.wob + 0.7 / sr).fract();
                let hs = self.noise() * hiss * (self.env * 12.0).min(1.0);
                for c in 0..2 {
                    let mut y = x[c];
                    for s in 0..2 {
                        y = self.hp[c][s].run(y, ghp, 1.3).2;
                        y = self.lp[c][s].run(y, glp, 1.3).0;
                    }
                    let bp = self.pk[c].run(y, gpk, 0.35).1;
                    y += bp * ch.peak_g;
                    y = (y * ch.drive).tanh() / ch.drive.tanh().max(0.3) * makeup + hs;
                    // the drive's DC out
                    self.dc[c] += (y - self.dc[c]) * 0.0015;
                    y -= self.dc[c];
                    x[c] = dry[c] * (1.0 - band) + y * band;
                }
            }
            // 2. the codec (and the robot inside it): a quarter-frame hop, latency of one frame
            let spectral = codec > 0.0 || robot > 0.0;
            for c in 0..2 {
                self.inb[c][self.pos] = x[c];
            }
            let y = [self.outb[0][self.pos], self.outb[1][self.pos]];
            self.hop_n += 1;
            if self.hop_n >= DG_HOP {
                self.hop_n = 0;
                let p0 = (self.pos + 1) % DG_N;
                let save = self.pos;
                self.pos = p0;
                if spectral {
                    self.codec_frame(codec, robot);
                } else {
                    for c in 0..2 {
                        for i in 0..DG_N {
                            let o = (p0 + i) % DG_N;
                            if i >= DG_N - DG_HOP {
                                self.outb[c][o] = 0.0;
                            }
                        }
                    }
                }
                self.pos = save;
            }
            if spectral {
                let w = (codec.max(robot) * 6.0).min(1.0);
                for c in 0..2 {
                    x[c] = x[c] * (1.0 - w) + y[c] * w;
                }
            }
            self.pos = (self.pos + 1) % DG_N;
            // 3. bit depth and sample rate
            if crush > 0.0 {
                let hold_n = 1.0 + (rate * crush).powi(2) * 30.0;
                self.hold_t += 1.0;
                if self.hold_t >= hold_n {
                    self.hold_t -= hold_n;
                    self.hold = x;
                }
                let b = 16.0 - bits * crush * 13.0;
                let q = 2f32.powf(b - 1.0);
                for c in 0..2 {
                    let h = if rate * crush > 0.001 { self.hold[c] } else { x[c] };
                    let qv = (h * q).round() / q;
                    x[c] = if bits * crush > 0.001 { qv } else { h };
                }
            }
            // 4. the ring modulator
            if ring > 0.0 {
                self.ring_ph = (self.ring_ph + ring_hz / sr).fract();
                let m = (self.ring_ph * TAU).sin();
                for c in 0..2 {
                    x[c] = x[c] * (1.0 - ring) + x[c] * m * ring * 1.4;
                }
            }
            // 5. the buffer-repeat glitch: at each slice it may loop the slice before
            self.gbuf[0][self.gw] = x[0];
            self.gbuf[1][self.gw] = x[1];
            if glitch > 0.0 || self.g_on {
                if self.g_t == 0 {
                    self.g_slice += 1;
                    let fire = hash01(self.g_slice, seed ^ 0x9e3) < glitch * 0.6;
                    if fire && !self.g_on {
                        self.g_on = true;
                        self.g_start = (self.gw + gn - glen) % gn;
                        self.g_k = 1 + (hash01(self.g_slice, seed ^ 0x77) * 3.0) as usize;
                    } else if self.g_on {
                        self.g_k = self.g_k.saturating_sub(1);
                        if self.g_k == 0 {
                            self.g_on = false;
                        }
                    }
                }
                if self.g_on {
                    let u = self.g_t;
                    let r = (self.g_start + u) % gn;
                    let ramp_n = 48.0f32;
                    let f = (u as f32 / ramp_n).min(1.0).min((glen - u) as f32 / ramp_n);
                    for c in 0..2 {
                        x[c] = self.gbuf[c][r] * f + x[c] * (1.0 - f);
                    }
                }
                self.g_t = (self.g_t + 1) % glen;
            } else {
                self.g_t = 0;
            }
            self.gw = (self.gw + 1) % gn;
            // 6. the metallic comb
            if comb > 0.0 {
                let fb = 0.55 + comb * 0.4;
                for c in 0..2 {
                    let d = self.comb[c].read(comb_d * if c == 0 { 1.0 } else { 1.013 });
                    self.comb_lp[c] += (d - self.comb_lp[c]) * 0.6;
                    let yv = x[c] + self.comb_lp[c] * fb;
                    self.comb[c].push(yv.clamp(-4.0, 4.0));
                    x[c] = x[c] * (1.0 - comb) + yv * comb * (1.0 - fb * 0.6);
                }
            } else {
                for c in 0..2 {
                    self.comb[c].push(0.0);
                }
            }
            // 7. mix and out
            fr[0] = fin((dry[0] * (1.0 - mix) + x[0] * mix) * out);
            fr[1] = fin((dry[1] * (1.0 - mix) + x[1] * mix) * out);
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        if !v.is_finite() {
            return;
        }
        if let Some(i) = DIGITIZE_PARAMS.iter().position(|p| p.0 == n) {
            self.p[i] = v.clamp(0.0, 1.0);
        }
    }
    fn name(&self) -> &'static str {
        "Digitize"
    }
    fn view(&self) -> Vec<f32> {
        // the stages as they are now: band, codec, robot, crush, ring, glitch, comb
        self.amt.to_vec()
    }
}

// ── Tape Stop ───────────────────────────────────────────────────────────────────

pub const TAPESTOP_PARAMS: [(&str, f32); 6] = [("stop", 0.0), ("time", 0.4), ("start", 0.25), ("curve", 0.5), ("filter", 0.5), ("mix", 1.0)];

pub const TAPESTOP_PRESETS: [Pre; 5] = [
    ("Quick stop", &[("time", 0.25), ("start", 0.15), ("curve", 0.5)]),
    ("Slow power-down", &[("time", 0.8), ("start", 0.5), ("curve", 0.3), ("filter", 0.7)]),
    ("Turntable brake", &[("time", 0.35), ("start", 0.1), ("curve", 0.85), ("filter", 0.3)]),
    ("Spin back up", &[("time", 0.3), ("start", 0.6), ("curve", 0.5)]),
    ("Dead battery", &[("time", 1.0), ("start", 0.8), ("curve", 0.15), ("filter", 0.9)]),
];

pub struct TapeStop {
    sr: f32,
    p: [f32; 6],
    buf: [Vec<f32>; 2],
    w: usize,
    /// read position (behind `w` while slowed), the speed now and where it goes
    rp: f64,
    speed: f32,
    lp: [f32; 2],
    /// crossfade back to the live input once the tape is up to speed again
    catch: f32,
}

impl TapeStop {
    pub fn new(sr: f32) -> TapeStop {
        let n = (sr * 12.0) as usize;
        let mut t = TapeStop { sr, p: [0.0; 6], buf: [vec![0.0; n], vec![0.0; n]], w: 0, rp: 0.0, speed: 1.0, lp: [0.0; 2], catch: 1.0 };
        for (i, (_, v)) in TAPESTOP_PARAMS.iter().enumerate() {
            t.p[i] = *v;
        }
        t
    }
    #[inline]
    fn read(&self, c: usize, pos: f64) -> f32 {
        let n = self.buf[c].len();
        let p = pos.rem_euclid(n as f64);
        let i = p as usize % n;
        let f = (p - p.floor()) as f32;
        self.buf[c][i] * (1.0 - f) + self.buf[c][(i + 1) % n] * f
    }
    pub fn speed(&self) -> f32 {
        self.speed
    }
}

impl Effect for TapeStop {
    fn process(&mut self, buf: &mut [f32]) {
        let sr = self.sr;
        let n = self.buf[0].len();
        let stop = self.p[0] > 0.5;
        let t_stop = 0.05 + self.p[1] * self.p[1] * 3.95;
        let t_start = 0.02 + self.p[2] * self.p[2] * 2.0;
        let curve = 0.4 + self.p[3] * 2.6;
        let filt = self.p[4];
        let mix = self.p[5];
        // the speed moves on a linear "position" u in 0..1, the playback speed is u^curve
        let du_stop = 1.0 / (t_stop * sr);
        let du_start = 1.0 / (t_start * sr);
        for fr in buf.chunks_exact_mut(2) {
            let x = [fin(fr[0]), fin(fr[1])];
            self.buf[0][self.w] = x[0];
            self.buf[1][self.w] = x[1];
            // u: invert speed = u^curve
            let mut u = self.speed.max(0.0).powf(1.0 / curve);
            if stop {
                u = (u - du_stop).max(0.0);
                self.catch = 0.0;
            } else {
                u = (u + du_start).min(1.0);
            }
            self.speed = u.powf(curve);
            if self.catch >= 1.0 {
                self.rp = self.w as f64;
            } else {
                self.rp += self.speed as f64;
            }
            let mut y = [self.read(0, self.rp), self.read(1, self.rp)];
            // the tone darkens with the speed
            let fc = (300.0 + 18000.0 * self.speed.powf(1.0 + filt * 2.0)).min(sr * 0.45);
            let k = 1.0 - (-TAU * fc / sr).exp();
            for c in 0..2 {
                self.lp[c] += (y[c] - self.lp[c]) * k;
                y[c] = self.lp[c] * self.speed.sqrt().max(0.0);
            }
            // up to speed again: crossfade from the lagging tape back to the live input
            if !stop && self.speed >= 0.999 && self.catch < 1.0 {
                self.catch = (self.catch + 1.0 / (0.03 * sr)).min(1.0);
            }
            let live = self.catch;
            let wet = [y[0] * (1.0 - live) + x[0] * live, y[1] * (1.0 - live) + x[1] * live];
            fr[0] = fin(x[0] * (1.0 - mix) + wet[0] * mix);
            fr[1] = fin(x[1] * (1.0 - mix) + wet[1] * mix);
            self.w = (self.w + 1) % n;
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        if !v.is_finite() {
            return;
        }
        if let Some(i) = TAPESTOP_PARAMS.iter().position(|p| p.0 == n) {
            self.p[i] = v.clamp(0.0, 1.0);
        }
    }
    fn name(&self) -> &'static str {
        "Tape Stop"
    }
    fn view(&self) -> Vec<f32> {
        vec![self.speed]
    }
}

// ── Shimmer Verb ────────────────────────────────────────────────────────────────

pub const SH_INTERVALS: [&str; 5] = ["+12", "+7", "+19", "+24", "-12"];
const SH_SEMIS: [f32; 5] = [12.0, 7.0, 19.0, 24.0, -12.0];

pub const SHIMMER_PARAMS: [(&str, f32); 11] = [
    ("mix", 0.35),
    ("size", 0.7),
    ("decay", 0.6),
    ("shimmer", 0.5),
    ("interval", 0.0),
    ("damp", 0.35),
    ("mod", 0.3),
    ("predelay", 0.15),
    ("low cut", 0.3),
    ("freeze", 0.0),
    ("width", 0.8),
];

pub const SHIMMER_PRESETS: [Pre; 6] = [
    ("Heaven shimmer", &[]),
    ("Octave cathedral", &[("size", 1.0), ("decay", 0.8), ("shimmer", 0.6), ("mix", 0.45)]),
    ("Fifth cloud", &[("interval", 0.3), ("shimmer", 0.45), ("decay", 0.7)]),
    ("Frozen pad", &[("freeze", 1.0), ("mix", 0.6), ("shimmer", 0.4)]),
    ("Dark shimmer (octave down)", &[("interval", 0.9), ("damp", 0.6), ("shimmer", 0.45), ("decay", 0.7)]),
    ("Trailer space", &[("size", 0.9), ("decay", 0.75), ("shimmer", 0.25), ("mix", 0.4), ("predelay", 0.3), ("low cut", 0.45)]),
];

pub struct ShimmerVerb {
    sr: f32,
    p: [f32; 11],
    fdn: Fdn,
    ps: [PShift; 2],
    pre: [Line; 2],
    fb: [f32; 2],
    hp: [Tsvf; 2],
    /// the shimmer loop's own high-pass (no DC or rumble climbs round it)
    fb_hp: [Tsvf; 2],
}

impl ShimmerVerb {
    pub fn new(sr: f32) -> ShimmerVerb {
        let mut s = ShimmerVerb { sr, p: [0.0; 11], fdn: Fdn::new(sr), ps: [PShift::new(sr, 0.06), PShift::new(sr, 0.071)], pre: [Line::new((sr * 0.25) as usize), Line::new((sr * 0.25) as usize)], fb: [0.0; 2], hp: [Tsvf::default(); 2], fb_hp: [Tsvf::default(); 2] };
        for (i, (_, v)) in SHIMMER_PARAMS.iter().enumerate() {
            s.p[i] = *v;
        }
        s.update();
        s
    }
    fn update(&mut self) {
        let rt = 0.5 * 2f32.powf(self.p[2] * 6.0);
        self.fdn.set(self.p[1], rt, self.p[5] * 0.8);
        self.fdn.modu = self.p[6];
        self.fdn.freeze = self.p[9] > 0.5;
    }
}

impl Effect for ShimmerVerb {
    fn process(&mut self, buf: &mut [f32]) {
        self.update();
        let sr = self.sr;
        let mix = self.p[0];
        let sh = self.p[3];
        let ratio = 2f32.powf(SH_SEMIS[choice(self.p[4], 5)] / 12.0);
        let pd = 1.0 + self.p[7] * 0.2 * sr;
        let ghp = g_of(20.0 * 2f32.powf(self.p[8] * 5.0), sr);
        let width = self.p[10];
        let freeze = self.p[9] > 0.5;
        // the shimmer path's gain, and the loss the lines take for it (see Fdn::loss)
        let k = sh * 0.55;
        self.fdn.loss = k * k * 0.12;
        let gfb = g_of(120.0, sr);
        for fr in buf.chunks_exact_mut(2) {
            let x = [fin(fr[0]), fin(fr[1])];
            let mut inp = [0.0f32; 2];
            for c in 0..2 {
                self.pre[c].push(x[c]);
                let d = self.pre[c].read(pd);
                let lc = self.hp[c].run(d, ghp, 1.4).2;
                // the shimmer: the tail, pitched, back into the input (gently saturated so it can never run away)
                let s = self.fb_hp[c].run(self.fb[c], gfb, 1.4).2;
                inp[c] = if freeze { 0.0 } else { lc + (s * k).tanh() };
            }
            let (l, r) = self.fdn.run(inp[0], inp[1]);
            self.fb[0] = self.ps[0].run(l, ratio);
            self.fb[1] = self.ps[1].run(r, ratio);
            // the pitched tail is heard as well as fed back: the halo over the hall
            let (l, r) = (l + self.fb[0] * sh * 0.7, r + self.fb[1] * sh * 0.7);
            // width on the wet
            let (m, s) = ((l + r) * 0.5, (l - r) * 0.5 * width * 1.4);
            let (wl, wr) = (m + s, m - s);
            fr[0] = fin(x[0] * (1.0 - mix * 0.5) + wl * mix * 1.2);
            fr[1] = fin(x[1] * (1.0 - mix * 0.5) + wr * mix * 1.2);
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        if !v.is_finite() {
            return;
        }
        if let Some(i) = SHIMMER_PARAMS.iter().position(|p| p.0 == n) {
            self.p[i] = v.clamp(0.0, 1.0);
        }
    }
    fn name(&self) -> &'static str {
        "Shimmer Verb"
    }
}

// ── Granular Cloud ──────────────────────────────────────────────────────────────

pub const GRANULAR_PARAMS: [(&str, f32); 12] = [
    ("size", 0.45),
    ("density", 0.5),
    ("pitch", 0.5),
    ("spread", 0.15),
    ("quantize", 0.0),
    ("jitter", 0.4),
    ("position", 0.15),
    ("reverse", 0.2),
    ("freeze", 0.0),
    ("feedback", 0.15),
    ("width", 0.8),
    ("mix", 0.6),
];

pub const GRANULAR_PRESETS: [Pre; 6] = [
    ("Soft cloud", &[]),
    ("Frozen texture", &[("freeze", 1.0), ("density", 0.7), ("size", 0.6), ("jitter", 0.8), ("mix", 0.8)]),
    ("Octave shimmer grains", &[("spread", 0.5), ("quantize", 1.0), ("density", 0.65), ("size", 0.35), ("reverse", 0.1)]),
    ("Reverse haze", &[("reverse", 1.0), ("size", 0.65), ("density", 0.55), ("feedback", 0.35), ("jitter", 0.6)]),
    ("Glitch spray", &[("size", 0.1), ("density", 0.85), ("spread", 0.6), ("jitter", 0.9), ("reverse", 0.5), ("mix", 0.7)]),
    ("Deep smear", &[("pitch", 0.25), ("size", 0.75), ("density", 0.6), ("feedback", 0.4), ("position", 0.4), ("mix", 0.75)]),
];

#[derive(Clone, Copy, Default)]
struct Grain {
    on: bool,
    pos: f64,
    inc: f64,
    age: usize,
    len: usize,
    gl: f32,
    gr: f32,
}

const MAX_GRAINS: usize = 48;

pub struct GranularCloud {
    sr: f32,
    p: [f32; 12],
    buf: [Vec<f32>; 2],
    w: usize,
    grains: [Grain; MAX_GRAINS],
    next: f32,
    count: u64,
    last: [f32; 2],
    dc: [f32; 2],
}

impl GranularCloud {
    pub fn new(sr: f32) -> GranularCloud {
        let n = (sr * 4.0) as usize;
        let mut g = GranularCloud { sr, p: [0.0; 12], buf: [vec![0.0; n], vec![0.0; n]], w: 0, grains: [Grain::default(); MAX_GRAINS], next: 0.0, count: 0, last: [0.0; 2], dc: [0.0; 2] };
        for (i, (_, v)) in GRANULAR_PARAMS.iter().enumerate() {
            g.p[i] = *v;
        }
        g
    }
    fn spawn(&mut self) {
        let Some(slot) = self.grains.iter().position(|g| !g.on) else { return };
        let sr = self.sr as f64;
        self.count += 1;
        let k = self.count;
        let h = |s: u32| hash01(k, s);
        let size = 0.01 * 2f64.powf(self.p[0] as f64 * 5.6);
        let len = (size * sr).max(32.0) as usize;
        let mut semis = (self.p[2] - 0.5) * 24.0 + (h(3) * 2.0 - 1.0) * self.p[3] * 12.0;
        if self.p[4] > 0.5 {
            // the spread lands on octaves and fifths
            let steps = [-12.0f32, -5.0, 0.0, 7.0, 12.0, 19.0];
            let r = (h(3) * self.p[3] * 6.0) as usize;
            semis = (self.p[2] - 0.5) * 24.0 + if r == 0 { 0.0 } else { steps[(r + (h(11) * 6.0) as usize) % 6] };
        }
        let inc = 2f64.powf(semis as f64 / 12.0);
        let rev = h(5) < self.p[7];
        let n = self.buf[0].len() as f64;
        let back = self.p[6] as f64 * 2.0 * sr + h(7) as f64 * self.p[5] as f64 * sr + len as f64 * inc.max(1.0) + 64.0;
        let back = back.min(n - len as f64 * inc - 64.0).max(64.0);
        let start = (self.w as f64 - back).rem_euclid(n);
        let pos = if rev { (start + len as f64 * inc).rem_euclid(n) } else { start };
        let pan = (h(9) * 2.0 - 1.0) * self.p[10];
        let a = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
        self.grains[slot] = Grain { on: true, pos, inc: if rev { -inc } else { inc }, age: 0, len, gl: a.cos() * 1.414, gr: a.sin() * 1.414 };
    }
    #[inline]
    fn read(&self, c: usize, pos: f64) -> f32 {
        let n = self.buf[c].len();
        let p = pos.rem_euclid(n as f64);
        let i = p as usize % n;
        let f = (p - p.floor()) as f32;
        self.buf[c][i] * (1.0 - f) + self.buf[c][(i + 1) % n] * f
    }
}

impl Effect for GranularCloud {
    fn process(&mut self, buf: &mut [f32]) {
        let sr = self.sr;
        let n = self.buf[0].len();
        let dens = 1.0 * 2f32.powf(self.p[1] * 6.6);
        let size = 0.01 * 2f32.powf(self.p[0] * 5.6);
        let norm = 1.0 / (dens * size).max(1.0).sqrt();
        let freeze = self.p[8] > 0.5;
        let fb = self.p[9] * 0.85;
        let mix = self.p[11];
        let period = sr / dens;
        for fr in buf.chunks_exact_mut(2) {
            let x = [fin(fr[0]), fin(fr[1])];
            self.next -= 1.0;
            if self.next <= 0.0 {
                self.spawn();
                // the next grain a little early or late (clouds, not a grid)
                self.next += period * (0.5 + hash01(self.count, 0x1234));
            }
            let mut y = [0.0f32; 2];
            for gi in 0..MAX_GRAINS {
                let g = self.grains[gi];
                if !g.on {
                    continue;
                }
                let ph = g.age as f32 / g.len as f32;
                let env = 0.5 - 0.5 * (TAU * ph).cos();
                let s0 = self.read(0, g.pos);
                let s1 = self.read(1, g.pos);
                y[0] += s0 * env * g.gl;
                y[1] += s1 * env * g.gr;
                let gm = &mut self.grains[gi];
                gm.pos += gm.inc;
                gm.age += 1;
                if gm.age >= gm.len {
                    gm.on = false;
                }
            }
            for c in 0..2 {
                y[c] *= norm;
                // a slow DC blocker on the cloud
                self.dc[c] += (y[c] - self.dc[c]) * 0.0005;
                y[c] -= self.dc[c];
            }
            if !freeze {
                for c in 0..2 {
                    self.buf[c][self.w] = x[c] + (self.last[c] * fb).tanh();
                }
                self.w = (self.w + 1) % n;
            }
            self.last = y;
            fr[0] = fin(x[0] * (1.0 - mix) + y[0] * mix);
            fr[1] = fin(x[1] * (1.0 - mix) + y[1] * mix);
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        if !v.is_finite() {
            return;
        }
        if let Some(i) = GRANULAR_PARAMS.iter().position(|p| p.0 == n) {
            self.p[i] = v.clamp(0.0, 1.0);
        }
    }
    fn name(&self) -> &'static str {
        "Granular Cloud"
    }
    fn view(&self) -> Vec<f32> {
        // grains sounding now, out of the most there can be
        vec![self.grains.iter().filter(|g| g.on).count() as f32 / MAX_GRAINS as f32]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;

    fn noise(n: usize, seed: u32, amp: f32) -> Vec<f32> {
        let mut s = seed | 1;
        (0..n * 2)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s as f32 / u32::MAX as f32 * 2.0 - 1.0) * amp
            })
            .collect()
    }
    /// a vowel-ish test voice: a 150 Hz pulse train through two formants, wobbling a little
    fn voiceish(n: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; n * 2];
        let (mut ph, mut f1, mut f2) = (0.0f32, Tsvf::default(), Tsvf::default());
        for i in 0..n {
            let f0 = 150.0 * (1.0 + 0.03 * (i as f32 / SR * 5.0 * TAU).sin());
            ph += f0 / SR;
            let pulse = if ph >= 1.0 {
                ph -= 1.0;
                1.0
            } else {
                0.0
            };
            let a = f1.run(pulse, g_of(700.0, SR), 0.15).1;
            let b = f2.run(pulse, g_of(1200.0, SR), 0.15).1;
            let y = (a + b * 0.6) * 0.4;
            out[i * 2] = y;
            out[i * 2 + 1] = y;
        }
        out
    }
    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
    }
    fn mean(v: &[f32]) -> f32 {
        v.iter().sum::<f32>() / v.len().max(1) as f32
    }
    fn mk(name: &str) -> Box<dyn Effect> {
        super::super::fx::make(name, SR).unwrap()
    }
    fn run(e: &mut dyn Effect, b: &mut [f32]) {
        for c in b.chunks_mut(256) {
            e.process(c);
        }
    }

    pub(super) const NAMES: [&str; 4] = ["Digitize", "Tape Stop", "Shimmer Verb", "Granular Cloud"];

    #[test]
    fn every_cinema_effect_stays_finite_for_any_settings() {
        for name in NAMES {
            for v in [0.0f32, 0.37, 0.5, 0.71, 1.0] {
                let mut e = mk(name);
                for (p, _) in super::super::fx::defaults(name) {
                    e.set_param(p, v);
                }
                let mut b = noise(24000, 7, 0.9);
                b[100] = f32::NAN;
                b[200] = 40.0;
                b[301] = f32::INFINITY;
                run(e.as_mut(), &mut b);
                assert!(b.iter().all(|x| x.is_finite() && x.abs() <= 8.0), "{name} at {v}");
            }
        }
    }

    #[test]
    fn every_preset_renders_sane_audio() {
        for name in NAMES {
            for (pn, set) in super::super::fx::presets(name) {
                let mut e = mk(name);
                for (k, v) in set.iter() {
                    e.set_param(k, *v);
                }
                // the tape stop is triggered half way
                let mut b = voiceish(SR as usize * 2);
                let (a, rest) = b.split_at_mut(SR as usize);
                run(e.as_mut(), a);
                e.set_param("stop", 1.0);
                run(e.as_mut(), rest);
                let (pk, r) = (b.iter().fold(0.0f32, |m, x| m.max(x.abs())), rms(&b));
                assert!(b.iter().all(|x| x.is_finite()), "{name} / {pn}");
                assert!(pk < 4.0 && r > 1e-4, "{name} / {pn}: peak {pk} rms {r}");
                let l: Vec<f32> = b.chunks(2).map(|c| c[0]).collect();
                assert!(mean(&l).abs() < 0.05, "{name} / {pn}: dc {}", mean(&l));
            }
        }
    }

    #[test]
    fn zero_mix_passes_the_input_and_silence_stays_silent() {
        for name in NAMES {
            let mut e = mk(name);
            e.set_param("mix", 0.0);
            if name == "Digitize" {
                e.set_param("out", 0.5);
            }
            let src = voiceish(SR as usize);
            let mut b = src.clone();
            run(e.as_mut(), &mut b);
            let err = b.iter().zip(&src).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(err < 1e-4, "{name}: mix 0 changed the signal by {err}");
            // silence in, silence out (no self-oscillation, no hiss without input)
            let mut e = mk(name);
            let mut z = vec![0.0f32; SR as usize * 2];
            run(e.as_mut(), &mut z);
            let pk = z.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(pk < 1e-3, "{name}: {pk} out of silence");
        }
    }

    #[test]
    fn transform_zero_is_the_dry_voice_and_one_is_digital() {
        let src = voiceish(SR as usize);
        let mut e = Digitize::new(SR);
        for (k, v) in DIGITIZE_PRESETS[0].1 {
            e.set_param(k, *v);
        }
        e.set_param("transform", 0.0);
        let mut b = src.clone();
        run(&mut e, &mut b);
        let err = b.iter().zip(&src).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 1e-4, "transform 0 must be clean: {err}");
        // fully transformed: a different signal, band-limited (little energy below the phone band), still about as loud
        e.set_param("transform", 1.0);
        let mut b = src.clone();
        run(&mut e, &mut b);
        let diff = rms(&b.iter().zip(&src).map(|(a, b)| a - b).collect::<Vec<_>>());
        assert!(diff > 0.3 * rms(&src), "the voice is transformed ({diff})");
        let ratio = rms(&b[b.len() / 2..]) / rms(&src[src.len() / 2..]);
        assert!(ratio > 0.15 && ratio < 4.0, "level {ratio}");
        // the transform brings the stages in one after another
        let mut d = Digitize::new(SR);
        d.set_param("transform", 0.2);
        let t = d.targets();
        assert!(t[0] > 0.3 && t[5] == 0.0 && t[4] == 0.0, "{t:?}");
    }

    #[test]
    fn the_tape_stops_and_starts_again() {
        let mut e = TapeStop::new(SR);
        e.set_param("time", 0.3);
        e.set_param("start", 0.2);
        let src = voiceish(SR as usize * 3);
        let mut b = src.clone();
        let third = SR as usize * 2;
        run(&mut e, &mut b[..third]);
        assert!((e.speed() - 1.0).abs() < 1e-6);
        e.set_param("stop", 1.0);
        run(&mut e, &mut b[third..third * 2]);
        assert!(e.speed() < 1e-3, "stopped: {}", e.speed());
        let stopped = rms(&b[third + third / 2..third * 2]);
        assert!(stopped < 0.01, "silent while stopped: {stopped}");
        e.set_param("stop", 0.0);
        run(&mut e, &mut b[third * 2..]);
        assert!(e.speed() > 0.999);
        // back to the live input by the end
        let n = b.len();
        let err = b[n - 2000..].iter().zip(&src[n - 2000..]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 1e-3, "caught up: {err}");
    }

    #[test]
    fn the_shimmer_tail_climbs_and_freeze_holds_it() {
        // a 220 Hz burst: the shimmer tail has energy an octave up
        let mut e = ShimmerVerb::new(SR);
        e.set_param("mix", 1.0);
        e.set_param("shimmer", 0.8);
        let n = SR as usize * 3;
        let mut b = vec![0.0f32; n * 2];
        for i in 0..(SR as usize / 4) {
            let y = (TAU * 220.0 * i as f32 / SR).sin() * 0.5;
            b[i * 2] = y;
            b[i * 2 + 1] = y;
        }
        run(&mut e, &mut b);
        let tail: Vec<f32> = b[SR as usize * 2..].chunks(2).map(|c| c[0]).collect();
        let pw = |f: f32| {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, v) in tail.iter().enumerate() {
                let a = std::f64::consts::TAU * f as f64 * i as f64 / SR as f64;
                re += *v as f64 * a.cos();
                im += *v as f64 * a.sin();
            }
            re * re + im * im
        };
        // the shifter's window smears the octave over a few Hz: compare bands, not bins
        let band = |f: f64| (-15..=15).map(|k| pw((f + k as f64 * 2.0) as f32)).sum::<f64>();
        assert!(band(440.0) > 0.08 * band(220.0), "octave {} root {}", band(440.0), band(220.0));
        // never runs away: with the most shimmer and the longest decay the tail still falls once the input stops
        let mut g = ShimmerVerb::new(SR);
        for (k, v) in [("mix", 1.0), ("shimmer", 1.0), ("decay", 1.0), ("size", 1.0), ("damp", 0.0)] {
            g.set_param(k, v);
        }
        let mut b = noise(SR as usize / 2, 5, 0.5);
        b.extend(vec![0.0f32; SR as usize * 40]);
        run(&mut g, &mut b);
        let at = |s: usize| rms(&b[s * SR as usize * 2..(s + 1) * SR as usize * 2]);
        assert!(at(19) < at(2) && at(19) < 0.5, "shimmer tail {} -> {}", at(2), at(19));
        let l: Vec<f32> = b.chunks(2).map(|c| c[0]).collect();
        assert!(mean(&l).abs() < 0.01, "dc {}", mean(&l));
        // frozen: the tail does not die
        let mut f = ShimmerVerb::new(SR);
        f.set_param("mix", 1.0);
        let mut b = noise(SR as usize / 2, 3, 0.3);
        run(&mut f, &mut b);
        f.set_param("freeze", 1.0);
        let mut z = vec![0.0f32; SR as usize * 4];
        run(&mut f, &mut z);
        let late = rms(&z[z.len() - 20000..]);
        assert!(late > 0.005 && late < 2.0, "frozen tail {late}");
    }

    #[test]
    fn the_cloud_plays_grains_and_freeze_keeps_them() {
        let mut e = GranularCloud::new(SR);
        e.set_param("mix", 1.0);
        let mut b = voiceish(SR as usize);
        run(&mut e, &mut b);
        assert!(rms(&b[b.len() / 2..]) > 0.005);
        e.set_param("freeze", 1.0);
        let mut z = vec![0.0f32; SR as usize * 2];
        run(&mut e, &mut z);
        assert!(rms(&z[z.len() / 2..]) > 0.005, "the frozen buffer keeps sounding");
        assert!(!e.view().is_empty());
    }
}
