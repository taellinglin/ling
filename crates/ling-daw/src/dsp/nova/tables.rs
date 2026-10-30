//! Ling Nova's factory wavetables: every table is generated (no recordings), 16 frames of 2048 samples that morph from the first to
//! the last, band-limited by `wavetable::WaveTable` like a user table. A table is built the first time a sound asks for it and kept
//! (a few MB each), so an instrument that uses three tables only ever builds three.
//!
//! Some tables are drawn in time (shapes, sync, folds, crushes, phase distortion), the rest are summed from harmonics (sweeps, organs,
//! formants and vowels, metallic spectra): a harmonic recipe is turned into a cycle with one inverse FFT.
use super::super::wavetable::{fft, WaveTable, N};
use std::f32::consts::{PI, TAU};
use std::sync::OnceLock;

/// frames per factory table
pub const FRAMES: usize = 16;

/// (name, family) of every factory table, in selector order; the user table (a loaded .wav) comes after the last
pub const TABLES: [(&str, &str); 48] = [
    ("Basic Shapes", "basic"),
    ("Saw Stack", "basic"),
    ("Pulse Width", "basic"),
    ("Mirror Saw", "basic"),
    ("Harmonic Sweep", "harmonic"),
    ("Odd Harmonics", "harmonic"),
    ("Even Harmonics", "harmonic"),
    ("Sine Partials", "harmonic"),
    ("Saw Octaves", "harmonic"),
    ("Hollow Saw", "harmonic"),
    ("Sync Saw", "sync"),
    ("Sync Sine", "sync"),
    ("Sync Square", "sync"),
    ("Vowel AEIOU", "vowel"),
    ("Talk Box", "vowel"),
    ("Yoi", "vowel"),
    ("Choir Ah", "vowel"),
    ("Growl Formant", "growl"),
    ("Riddim Square", "growl"),
    ("Neuro Grind", "growl"),
    ("Screech", "growl"),
    ("Wub Sweep", "growl"),
    ("FM 1:1", "fm"),
    ("FM 2:1", "fm"),
    ("FM 3:2", "fm"),
    ("FM Feedback", "fm"),
    ("FM Bass", "fm"),
    ("Metallic", "metal"),
    ("Bell Partials", "metal"),
    ("Glass", "metal"),
    ("Spectral Noise", "metal"),
    ("Chip Pulse", "digital"),
    ("4-bit Ramp", "digital"),
    ("Bitcrush Saw", "digital"),
    ("Digital Steps", "digital"),
    ("Fold Sine", "dist"),
    ("Fold Saw", "dist"),
    ("Tanh Drive", "dist"),
    ("Asym Drive", "dist"),
    ("Dirty Square", "dist"),
    ("Organ Drawbars", "organ"),
    ("Additive Pad", "organ"),
    ("Brass Swell", "organ"),
    ("Pluck Decay", "organ"),
    ("Phase Distort", "phase"),
    ("Reso Saw", "phase"),
    ("Comb Saw", "phase"),
    ("Formant Pulse", "phase"),
];
pub const NT: usize = TABLES.len();

/// the factory table `i` (built on first use)
pub fn table(i: usize) -> &'static WaveTable {
    static T: OnceLock<Vec<OnceLock<WaveTable>>> = OnceLock::new();
    let all = T.get_or_init(|| (0..NT).map(|_| OnceLock::new()).collect());
    let i = i.min(NT - 1);
    all[i].get_or_init(|| {
        let frames: Vec<Vec<f32>> = (0..FRAMES).map(|f| frame(i, f as f32 / (FRAMES - 1) as f32)).collect();
        WaveTable::from_frames(&frames, "").unwrap_or_else(|_| WaveTable::from_frames(&[sine_cycle()], "").expect("sine"))
    })
}

fn sine_cycle() -> Vec<f32> {
    (0..N).map(|j| (TAU * j as f32 / N as f32).sin()).collect()
}

/// a cycle from harmonic amplitudes (sine phase), harmonics 1..amps.len()
fn additive(amps: &[f32]) -> Vec<f32> {
    let (mut re, mut im) = (vec![0.0f32; N], vec![0.0f32; N]);
    for (h, a) in amps.iter().enumerate() {
        let k = h + 1;
        if k >= N / 2 || *a == 0.0 {
            continue;
        }
        im[k] = -a * N as f32 * 0.5;
        im[N - k] = a * N as f32 * 0.5;
    }
    fft(&mut re, &mut im, true);
    re
}

/// additive with a phase per harmonic (for denser, less peaky stacks)
fn additive_ph(amps: &[f32], ph: &[f32]) -> Vec<f32> {
    let (mut re, mut im) = (vec![0.0f32; N], vec![0.0f32; N]);
    for (h, a) in amps.iter().enumerate() {
        let k = h + 1;
        if k >= N / 2 || *a == 0.0 {
            continue;
        }
        let p = ph.get(h).copied().unwrap_or(0.0);
        // a * sin(2 pi k t + p)
        let (s, c) = p.sin_cos();
        re[k] = a * s * N as f32 * 0.5;
        re[N - k] = a * s * N as f32 * 0.5;
        im[k] = -a * c * N as f32 * 0.5;
        im[N - k] = a * c * N as f32 * 0.5;
    }
    fft(&mut re, &mut im, true);
    re
}

/// a time-domain cycle from a function of the phase 0..1
fn draw(f: impl Fn(f32) -> f32) -> Vec<f32> {
    (0..N).map(|j| f(j as f32 / N as f32)).collect()
}

/// vowel formants (Hz, gain) for a, e, i, o, u
pub const VOWELS: [[(f32, f32); 3]; 5] = [
    [(800.0, 1.0), (1150.0, 0.5), (2900.0, 0.12)],
    [(400.0, 1.0), (1700.0, 0.35), (2600.0, 0.15)],
    [(280.0, 1.0), (2250.0, 0.25), (2950.0, 0.15)],
    [(450.0, 1.0), (800.0, 0.6), (2830.0, 0.05)],
    [(325.0, 1.0), (700.0, 0.2), (2530.0, 0.03)],
];

/// the formants of a vowel position 0..1 over a-e-i-o-u (interpolated)
pub fn vowel_at(p: f32) -> [(f32, f32); 3] {
    let x = p.clamp(0.0, 1.0) * 4.0;
    let i = (x as usize).min(3);
    let f = x - i as f32;
    let mut out = [(0.0, 0.0); 3];
    for k in 0..3 {
        let (a, b) = (VOWELS[i][k], VOWELS[i + 1][k]);
        // the frequency moves on a log scale
        out[k] = (a.0 * (b.0 / a.0).powf(f), a.1 + (b.1 - a.1) * f);
    }
    out
}

/// the gain of a formant set at `hz` (resonance peaks with bandwidths that grow with frequency)
fn formant_gain(fm: &[(f32, f32)], hz: f32, q: f32) -> f32 {
    let mut g = 0.015;
    for &(fc, a) in fm {
        let bw = (60.0 + fc * 0.08) / q;
        let d = (hz - fc) / bw;
        g += a / (1.0 + d * d);
    }
    g
}

/// a saw's harmonics shaped by formants; `f0` is the pitch the formants are drawn for (the table plays them relative to the note)
fn formant_saw(fm: &[(f32, f32)], f0: f32, q: f32, odd_only: bool) -> Vec<f32> {
    let amps: Vec<f32> = (1..=200)
        .map(|k| {
            if odd_only && k % 2 == 0 {
                return 0.0;
            }
            formant_gain(fm, k as f32 * f0, q) / (k as f32).powf(0.6)
        })
        .collect();
    additive(&amps)
}

/// a cheap fixed hash -> 0..1
fn hash(i: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

fn saw(x: f32) -> f32 {
    2.0 * x.fract() - 1.0
}
fn sq(x: f32, w: f32) -> f32 {
    if x.fract() < w { 1.0 } else { -1.0 }
}
fn tri(x: f32) -> f32 {
    1.0 - 4.0 * (x.fract() - 0.5).abs()
}
fn sine(x: f32) -> f32 {
    (TAU * x).sin()
}
/// a triangle fold of x into -1..1
pub fn fold(x: f32) -> f32 {
    let y = (x + 1.0).rem_euclid(4.0);
    if y < 2.0 { y - 1.0 } else { 3.0 - y }
}

/// one frame of table `i` at morph position `t` (0 = first frame, 1 = last)
fn frame(i: usize, t: f32) -> Vec<f32> {
    match TABLES[i].0 {
        "Basic Shapes" => {
            // sine -> triangle -> saw -> square
            let s = t * 3.0;
            let k = (s as usize).min(2);
            let f = s - k as f32;
            let w = |k: usize, x: f32| match k {
                0 => sine(x),
                1 => tri(x),
                2 => saw(x + 0.5),
                _ => sq(x, 0.5),
            };
            draw(|x| w(k, x) * (1.0 - f) + w(k + 1, x) * f)
        }
        "Saw Stack" => {
            // a saw that grows into a dense stack: octaves and fifths with scattered phases (a supersaw in one cycle)
            let amps: Vec<f32> = (1..=256).map(|k| 1.0 / k as f32 * (1.0 + t * 1.5 * hash(k as u32 * 7)).min(2.0)).collect();
            let ph: Vec<f32> = (1..=256).map(|k| t * TAU * hash(k as u32 * 13 + 5)).collect();
            additive_ph(&amps, &ph)
        }
        "Pulse Width" => {
            let w = 0.5 - t * 0.47;
            draw(|x| sq(x, w))
        }
        "Mirror Saw" => {
            // saw -> triangle by moving the peak
            let m = 0.999 - t * 0.499;
            draw(|x| if x < m { -1.0 + 2.0 * x / m } else { 1.0 - 2.0 * (x - m) / (1.0 - m) })
        }
        "Harmonic Sweep" => {
            // one harmonic at a time added, up to 64, saw-weighted
            let n = 1.0 + t * 63.0;
            let amps: Vec<f32> = (1..=64).map(|k| ((n - k as f32 + 1.0).clamp(0.0, 1.0)) / k as f32).collect();
            additive(&amps)
        }
        "Odd Harmonics" => {
            let n = 1.0 + t * 80.0;
            let amps: Vec<f32> = (1..=81).map(|k| if k % 2 == 1 { ((n - k as f32 + 1.0).clamp(0.0, 1.0)) / k as f32 } else { 0.0 }).collect();
            additive(&amps)
        }
        "Even Harmonics" => {
            let amps: Vec<f32> = (1..=64).map(|k| if k == 1 { 1.0 } else if k % 2 == 0 { t / (k as f32).powf(0.8) } else { (1.0 - t) * 0.3 / k as f32 }).collect();
            additive(&amps)
        }
        "Sine Partials" => {
            // a sine and one partial sweeping up from the 2nd to the 16th
            let p = 2.0 + t * 14.0;
            let k0 = p.floor() as usize;
            let f = p - k0 as f32;
            let mut amps = vec![0.0f32; 17];
            amps[0] = 1.0;
            amps[k0 - 1] += 0.7 * (1.0 - f);
            if k0 < 17 {
                amps[k0] += 0.7 * f;
            }
            additive(&amps)
        }
        "Saw Octaves" => {
            // a saw with its octave and two octaves up fading in (stacked saws in one cycle)
            draw(|x| (saw(x + 0.5) + t * 0.7 * saw(2.0 * x + 0.5) + t * t * 0.5 * saw(4.0 * x + 0.5)) / (1.0 + t * 1.2))
        }
        "Hollow Saw" => {
            // a saw minus its octave-shifted copy: the hollow, pulse-like middle of a reese
            let d = 0.05 + t * 0.45;
            draw(|x| saw(x + 0.5) - saw(x + 0.5 + d))
        }
        "Sync Saw" => {
            let r = 1.0 + t * 7.0;
            // the sync'd phase restarts every cycle: a slave saw at ratio r, faded at the reset so the seam stays soft
            draw(|x| saw((x * r).fract() + 0.5) * (1.0 - x).powf(0.15))
        }
        "Sync Sine" => {
            let r = 1.0 + t * 6.0;
            draw(|x| sine(x * r) * (PI * x).sin().powf(0.3))
        }
        "Sync Square" => {
            let r = 1.0 + t * 5.0;
            draw(|x| sq(x * r, 0.5) * 0.8)
        }
        "Vowel AEIOU" => formant_saw(&vowel_at(t), 110.0, 1.0, false),
        "Talk Box" => {
            // o - a - i - a - o: a mouth opening and closing, narrower formants
            let p = [0.75, 0.0, 0.5, 0.0, 1.0];
            let x = t * 4.0;
            let k = (x as usize).min(3);
            let f = x - k as f32;
            let (a, b) = (vowel_at(p[k]), vowel_at(p[k + 1]));
            let fm: Vec<(f32, f32)> = (0..3).map(|j| (a[j].0 * (b[j].0 / a[j].0).powf(f), a[j].1 + (b[j].1 - a[j].1) * f)).collect();
            formant_saw(&fm, 90.0, 1.8, false)
        }
        "Yoi" => {
            // "oo" opening to "ee" through "o": the yoi of bass music, on a square-ish (odd heavy) source
            let p = if t < 0.5 { 1.0 - t * 0.5 } else { 0.75 - (t - 0.5) * 1.0 };
            let fm = vowel_at(p.clamp(0.0, 1.0));
            let mut amps: Vec<f32> = (1..=200).map(|k| formant_gain(&fm, k as f32 * 80.0, 1.6) * if k % 2 == 1 { 1.0 } else { 0.55 } / (k as f32).powf(0.45)).collect();
            amps[0] += 0.3;
            additive(&amps)
        }
        "Choir Ah" => {
            let fm = vowel_at(0.0 + t * 0.2);
            let amps: Vec<f32> = (1..=120).map(|k| formant_gain(&fm, k as f32 * 220.0, 1.0) / (k as f32).powf(1.1)).collect();
            additive(&amps)
        }
        "Growl Formant" => {
            // one strong resonance on a saw sweeping 300 Hz -> 2.4 kHz, with a second one a ratio above: a growl in a table
            let f1 = 300.0 * 8f32.powf(t);
            let fm = [(f1, 1.0), (f1 * 2.6, 0.5)];
            let mut amps: Vec<f32> = (1..=220).map(|k| formant_gain(&fm, k as f32 * 70.0, 2.2) / (k as f32).powf(0.3)).collect();
            amps[0] += 0.5;
            additive(&amps)
        }
        "Riddim Square" => {
            // a square through a closing-to-open resonance: the hollow riddim honk
            let f1 = 200.0 * 10f32.powf(t);
            let fm = [(f1, 1.0)];
            let amps: Vec<f32> = (1..=220).map(|k| if k % 2 == 1 { (0.35 + formant_gain(&fm, k as f32 * 60.0, 3.0)) / k as f32 * 2.0 } else { 0.0 }).collect();
            additive(&amps)
        }
        "Neuro Grind" => {
            // a saw ring-modulated by a harmonic that climbs: clangy, metallic neuro grind
            let r = (1.0 + t * 11.0).round();
            let m = t * 0.9;
            draw(|x| saw(x + 0.5) * ((1.0 - m) + m * sine(x * r)) + 0.15 * t * sq(x * r * 0.5, 0.5))
        }
        "Screech" => {
            // a narrow high formant on a synced saw: a screech rising
            let f1 = 1200.0 * 3f32.powf(t);
            let fm = [(f1, 1.0), (f1 * 1.5, 0.6)];
            let amps: Vec<f32> = (1..=250).map(|k| (formant_gain(&fm, k as f32 * 100.0, 4.0) + 0.04) / (k as f32).powf(0.25)).collect();
            additive(&amps)
        }
        "Wub Sweep" => {
            // a resonant low-pass opening over a saw: a wobble baked into the table
            let fc = 1.5 * 60f32.powf(t);
            let amps: Vec<f32> = (1..=200).map(|k| {
                let r = k as f32 / fc;
                let res = 1.0 / ((1.0 - r * r).powi(2) + (0.25 * r).powi(2)).sqrt();
                (res.min(4.0) / (1.0 + r.powi(4))) / k as f32
            }).collect();
            additive(&amps)
        }
        "FM 1:1" => {
            let idx = t * 6.0;
            draw(|x| (TAU * x + idx * (TAU * x).sin()).sin())
        }
        "FM 2:1" => {
            let idx = t * 5.0;
            draw(|x| (TAU * x + idx * (TAU * 2.0 * x).sin()).sin())
        }
        "FM 3:2" => {
            // carrier 2, modulator 3 (relative to the cycle): a reedy, hollow FM sound
            let idx = t * 4.0;
            draw(|x| (TAU * 2.0 * x + idx * (TAU * 3.0 * x).sin()).sin() * 0.6 + (TAU * x).sin() * 0.4)
        }
        "FM Feedback" => {
            // a feedback FM operator: sine -> saw-like as the feedback rises (solved per sample by iteration)
            let b = t * 1.4;
            draw(|x| {
                let mut y = 0.0f32;
                for _ in 0..12 {
                    y = (TAU * x + b * y).sin();
                }
                y
            })
        }
        "FM Bass" => {
            // two modulators (1:1 and 1:2) stacking: from round to a gritty, talking FM bass
            let a = t * 3.0;
            let b = t * t * 2.5;
            draw(|x| (TAU * x + a * (TAU * x).sin() + b * (TAU * 2.0 * x + 0.5 * (TAU * x).sin()).sin()).sin())
        }
        "Metallic" => {
            // scattered harmonics (primes and their neighbours) with random weights that move: clangy and inharmonic-sounding
            let amps: Vec<f32> = (1..=128).map(|k| {
                let w = hash(k as u32 * 31 + 3);
                let w2 = hash(k as u32 * 17 + 99);
                let a = w * (1.0 - t) + w2 * t;
                if k == 1 { 0.6 } else if a > 0.72 { a / (k as f32).powf(0.5) } else { 0.0 }
            }).collect();
            additive(&amps)
        }
        "Bell Partials" => {
            // bell-like partials (as near as a cycle allows: 1, 2, 3, 5, 7, 9, 13, 17) whose upper ones swell
            let ks = [1usize, 2, 3, 5, 7, 9, 13, 17, 21];
            let mut amps = vec![0.0f32; 22];
            for (j, &k) in ks.iter().enumerate() {
                amps[k - 1] = if j == 0 { 1.0 } else { (0.6 - j as f32 * 0.05) * (0.2 + t * 0.8) };
            }
            additive(&amps)
        }
        "Glass" => {
            let lo = 4.0 + t * 20.0;
            let amps: Vec<f32> = (1..=64).map(|k| if k == 1 { 0.5 } else { (-((k as f32 - lo) / 3.0).powi(2)).exp() * 0.8 }).collect();
            additive(&amps)
        }
        "Spectral Noise" => {
            // random harmonics, a new random set per frame (morphs like a texture)
            let fr = (t * (FRAMES - 1) as f32).round() as u32;
            let amps: Vec<f32> = (1..=250).map(|k| hash(k as u32 * 977 + fr * 7919) / (k as f32).powf(0.5)).collect();
            let ph: Vec<f32> = (1..=250).map(|k| TAU * hash(k as u32 * 131 + fr * 31)).collect();
            additive_ph(&amps, &ph)
        }
        "Chip Pulse" => {
            // the 8-bit duty cycles: 50, 25, 12.5 % and a 4-step quantised triangle at the end
            let s = t * 3.0;
            let k = (s as usize).min(2);
            let f = s - k as f32;
            let w = |k: usize, x: f32| match k {
                0 => sq(x, 0.5),
                1 => sq(x, 0.25),
                2 => sq(x, 0.125),
                _ => (tri(x) * 4.0).round() / 4.0,
            };
            draw(|x| w(k, x) * (1.0 - f) + w(k + 1, x) * f)
        }
        "4-bit Ramp" => {
            let steps = 16.0 - t * 12.0;
            draw(|x| (saw(x + 0.5) * steps).round() / steps)
        }
        "Bitcrush Saw" => {
            let bits = 8.0 - t * 6.5;
            let q = 2f32.powf(bits - 1.0);
            draw(|x| ((saw(x + 0.5) * 0.98 + sine(x) * 0.3) * q).round() / q)
        }
        "Digital Steps" => {
            // a sine held in fewer and fewer steps per cycle
            let n = (64.0 * 0.0625f32.powf(t)).max(3.0);
            draw(|x| sine((x * n).floor() / n + 0.5 / n))
        }
        "Fold Sine" => {
            let g = 1.0 + t * 7.0;
            draw(|x| fold(sine(x) * g))
        }
        "Fold Saw" => {
            let g = 1.0 + t * 5.0;
            draw(|x| fold(saw(x + 0.5) * g + 0.2 * t))
        }
        "Tanh Drive" => {
            let g = 1.0 + t * 20.0;
            draw(|x| ((sine(x) + 0.3 * saw(x + 0.5)) * g).tanh())
        }
        "Asym Drive" => {
            let g = 1.0 + t * 10.0;
            draw(|x| {
                let s = sine(x) * g + t * 1.5;
                if s > 0.0 { s.tanh() } else { (s * 0.4).tanh() * 1.6 }
            })
        }
        "Dirty Square" => {
            let g = 1.0 + t * 8.0;
            let bits = 10.0 - t * 6.0;
            let q = 2f32.powf(bits);
            draw(|x| (((sq(x, 0.5) * 0.7 + saw(x * 2.0) * 0.3 * t) * g).tanh() * q).round() / q)
        }
        "Organ Drawbars" => {
            // drawbar registrations, gentle to full: 16' 8' 5 1/3' 4' 2 2/3' 2' 1 3/5' 1 1/3' 1'
            let regs = [[8.0f32, 8.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], [8.0, 8.0, 8.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], [8.0, 8.0, 8.0, 6.0, 0.0, 4.0, 0.0, 0.0, 0.0], [8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0]];
            let s = t * 3.0;
            let k = (s as usize).min(2);
            let f = s - k as f32;
            // harmonic numbers relative to the 16' bar
            let hs = [1usize, 2, 3, 4, 6, 8, 10, 12, 16];
            let mut amps = vec![0.0f32; 16];
            for j in 0..9 {
                let a = regs[k][j] * (1.0 - f) + regs[k + 1][j] * f;
                amps[hs[j] - 1] += a / 8.0 * 0.6;
            }
            additive(&amps)
        }
        "Additive Pad" => {
            // a soft spectrum whose upper half breathes in
            let amps: Vec<f32> = (1..=48).map(|k| {
                let kk = k as f32;
                (1.0 / kk) * (if kk > 4.0 { t + 0.15 * (kk * 0.7).sin().abs() } else { 1.0 }) * (-kk / (6.0 + t * 30.0)).exp()
            }).collect();
            additive(&amps)
        }
        "Brass Swell" => {
            // a saw whose brightness follows the swell, with a brass formant around the 10th-15th harmonic
            let n = 3.0 + t * 40.0;
            let amps: Vec<f32> = (1..=120).map(|k| {
                let kk = k as f32;
                (1.0 / kk) * (-(kk / n).powi(2)).exp().max(0.0) * (1.0 + 0.8 * (-((kk - 12.0) / 6.0).powi(2)).exp() * t)
            }).collect();
            additive(&amps)
        }
        "Pluck Decay" => {
            // bright to dark: a saw losing its top the way a plucked string does
            let n = 2.0 + (1.0 - t) * 120.0;
            let amps: Vec<f32> = (1..=200).map(|k| (1.0 / k as f32) * (-(k as f32) / n).exp()).collect();
            additive(&amps)
        }
        "Phase Distort" => {
            // Casio-style phase distortion: a cosine read through a bent phase, sharper as it goes
            let d = 0.5 - t * 0.48;
            draw(|x| {
                let p = if x < d { x * 0.5 / d } else { 0.5 + (x - d) * 0.5 / (1.0 - d) };
                -(TAU * p).cos()
            })
        }
        "Reso Saw" => {
            // the resonant phase-distortion wave: a sine at a rising ratio inside a falling saw window
            let r = 1.0 + t * 12.0;
            draw(|x| ((TAU * x * r).sin() * (1.0 - x)) * 1.2)
        }
        "Comb Saw" => {
            // a saw with comb notches walking through it
            let d = 0.02 + t * 0.3;
            let amps: Vec<f32> = (1..=200).map(|k| (1.0 / k as f32) * (0.5 + 0.5 * (TAU * k as f32 * d).cos()).abs()).collect();
            additive(&amps)
        }
        _ => {
            // "Formant Pulse": a narrow pulse whose formant climbs (pulse width and a resonance at once)
            let w = 0.5 - t * 0.4;
            let f1 = 400.0 * 6f32.powf(t);
            let fm = [(f1, 1.0)];
            let amps: Vec<f32> = (1..=200).map(|k| {
                let kk = k as f32;
                let pulse = (PI * kk * w).sin().abs() / kk;
                pulse * (0.4 + formant_gain(&fm, kk * 100.0, 2.0))
            }).collect();
            additive(&amps)
        }
    }
}

/// the names in selector order, the user slot last
pub fn names_with_user() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = TABLES.iter().map(|t| t.0).collect();
    v.push("User");
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_factory_table_builds_with_moving_frames() {
        assert!(NT >= 40);
        for i in 0..NT {
            let t = table(i);
            assert_eq!(t.nframes(), FRAMES, "{}", TABLES[i].0);
            let a = t.cycle(0, 0);
            let b = t.cycle(FRAMES - 1, 0);
            assert!(a.iter().chain(b.iter()).all(|x| x.is_finite()), "{}", TABLES[i].0);
            let pk = a.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(pk > 0.05, "{} frame 0 is silent", TABLES[i].0);
            let diff: f32 = a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f32>() / N as f32;
            assert!(diff > 0.01, "{}: the table does not morph ({diff})", TABLES[i].0);
        }
    }
}
