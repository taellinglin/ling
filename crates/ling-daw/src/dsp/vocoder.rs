//! Channel vocoder: the audio arriving at the effect is the carrier; the modulator is the routed aux audio
//! or a built-in talking synth (glottal source + formant vowels, consonant bursts, rhythmic gate).
use super::fx::Effect;
use std::f32::consts::PI;

const MAXB: usize = 32;
const NP: usize = 23;
const MAKEUP: f32 = 3.0;

pub const PARAMS: [(&str, f32); NP] = [
    ("source", 0.0),
    ("bands", 0.5),
    ("lo", 0.3),
    ("hi", 0.6),
    ("q", 0.5),
    ("attack", 0.15),
    ("release", 0.3),
    ("formant", 0.5),
    ("width", 0.6),
    ("hiss", 0.4),
    ("flatten", 0.4),
    ("modgain", 0.5),
    ("cargain", 0.5),
    ("freeze", 0.0),
    ("pitch", 0.4),
    ("rate", 0.3),
    ("vowel", 0.0),
    ("shape", 0.5),
    ("cons", 0.3),
    ("gate", 0.0),
    ("grate", 0.5),
    ("out", 0.67),
    ("mix", 1.0),
];

// Source values: index = floor(v*6): 0 auto (aux if routed, else built-in vowels), 1 aux only,
// 2 built-in vowels, 3 built-in robot (monotone pulse), 4 built-in noise (whisper), 5 built-in gated rhythm.
pub const PRESETS: [(&str, &[(&str, f32)]); 14] = [
    ("Classic Robot", &[("source", 0.58), ("pitch", 0.35), ("rate", 0.15), ("bands", 0.45), ("q", 0.6), ("hiss", 0.3), ("flatten", 0.5), ("width", 0.3), ("cons", 0.2)]),
    ("Choir Pad", &[("source", 0.42), ("rate", 0.08), ("pitch", 0.5), ("attack", 0.4), ("release", 0.65), ("width", 0.9), ("flatten", 0.7), ("bands", 0.7), ("cons", 0.0), ("hiss", 0.15)]),
    ("Whisper", &[("source", 0.75), ("rate", 0.25), ("hiss", 0.7), ("flatten", 0.8), ("cons", 0.5), ("release", 0.25), ("bands", 0.6), ("q", 0.45)]),
    ("Talkbox Funk", &[("source", 0.0), ("q", 0.35), ("bands", 0.4), ("release", 0.2), ("hiss", 0.5), ("flatten", 0.5), ("cargain", 0.6), ("width", 0.2)]),
    ("Daft Voice", &[("source", 0.0), ("bands", 0.55), ("q", 0.65), ("hiss", 0.5), ("flatten", 0.6), ("width", 0.5), ("attack", 0.05), ("release", 0.22), ("formant", 0.55), ("pitch", 0.35)]),
    ("Alien Formant", &[("source", 0.42), ("rate", 0.55), ("formant", 0.85), ("pitch", 0.6), ("q", 0.8), ("width", 1.0), ("bands", 0.6), ("cons", 0.1), ("shape", 0.8)]),
    ("Gated Rhythm Pad", &[("source", 0.92), ("gate", 1.0), ("grate", 0.55), ("rate", 0.3), ("width", 0.8), ("flatten", 0.7), ("release", 0.3), ("cons", 0.2)]),
    ("Radio Operator", &[("lo", 0.55), ("hi", 0.35), ("bands", 0.3), ("q", 0.75), ("hiss", 0.6), ("source", 0.0), ("formant", 0.45), ("width", 0.0), ("flatten", 0.3)]),
    ("Small Speaker", &[("lo", 0.6), ("hi", 0.4), ("formant", 0.65), ("bands", 0.25), ("q", 0.5), ("width", 0.1), ("hiss", 0.25), ("mix", 0.85)]),
    ("Deep Giant", &[("formant", 0.1), ("pitch", 0.05), ("lo", 0.1), ("hi", 0.5), ("source", 0.58), ("release", 0.6), ("flatten", 0.3), ("rate", 0.1), ("cons", 0.1), ("modgain", 0.6)]),
    ("Chipmunk", &[("formant", 0.95), ("pitch", 0.9), ("lo", 0.45), ("hi", 0.9), ("source", 0.42), ("rate", 0.5), ("release", 0.15), ("bands", 0.5)]),
    ("Glass Choir", &[("source", 0.42), ("rate", 0.06), ("pitch", 0.7), ("formant", 0.75), ("hi", 1.0), ("q", 0.85), ("width", 1.0), ("bands", 0.85), ("release", 0.75), ("flatten", 0.8), ("cons", 0.0)]),
    ("Robot Singer", &[("source", 0.42), ("rate", 0.3), ("pitch", 0.45), ("shape", 0.3), ("cons", 0.5), ("bands", 0.6), ("q", 0.6), ("width", 0.4), ("flatten", 0.5)]),
    ("Sample Stutter", &[("source", 0.92), ("gate", 1.0), ("grate", 1.0), ("rate", 0.6), ("cons", 0.8), ("attack", 0.0), ("release", 0.05), ("bands", 0.5), ("width", 0.5)]),
];

const I_SOURCE: usize = 0;
const I_BANDS: usize = 1;
const I_LO: usize = 2;
const I_HI: usize = 3;
const I_Q: usize = 4;
const I_ATTACK: usize = 5;
const I_RELEASE: usize = 6;
const I_FORMANT: usize = 7;
const I_WIDTH: usize = 8;
const I_HISS: usize = 9;
const I_FLATTEN: usize = 10;
const I_MODGAIN: usize = 11;
const I_CARGAIN: usize = 12;
const I_FREEZE: usize = 13;
const I_PITCH: usize = 14;
const I_RATE: usize = 15;
const I_VOWEL: usize = 16;
const I_SHAPE: usize = 17;
const I_CONS: usize = 18;
const I_GATE: usize = 19;
const I_GRATE: usize = 20;
const I_OUT: usize = 21;
const I_MIX: usize = 22;

#[derive(Clone, Copy, Default)]
struct Coef {
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
}
impl Coef {
    fn bp(f: f32, q: f32, sr: f32) -> Coef {
        let f = f.clamp(20.0, 0.45 * sr);
        let g = (PI * f / sr).tan();
        let k = 1.0 / q.clamp(0.3, 60.0);
        let a1 = 1.0 / (1.0 + g * (g + k));
        Coef { a1, a2: g * a1, a3: g * g * a1, k }
    }
}

/// Trapezoidal state-variable filter; `tick` returns the band-pass with unity gain at its centre.
#[derive(Clone, Copy, Default)]
struct Svf {
    ic1: f32,
    ic2: f32,
}
impl Svf {
    #[inline]
    fn tick(&mut self, c: &Coef, x: f32) -> f32 {
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        c.k * v1
    }
}
#[inline]
fn tick2(s: &mut [Svf; 2], c: &Coef, x: f32) -> f32 {
    let a = s[0].tick(c, x);
    s[1].tick(c, a)
}

struct Rng(u32);
impl Rng {
    #[inline]
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f32 / 2147483648.0) - 1.0
    }
}

// Vowel formants F1, F2, F3: A E I O U AE(cat) UH OE
const VOWELS: [[f32; 3]; 8] = [
    [800.0, 1150.0, 2900.0],
    [400.0, 1600.0, 2700.0],
    [270.0, 2140.0, 2950.0],
    [450.0, 800.0, 2830.0],
    [325.0, 700.0, 2530.0],
    [660.0, 1720.0, 2410.0],
    [500.0, 1250.0, 2500.0],
    [500.0, 1400.0, 2300.0],
];
const FORMANT_BW: [f32; 4] = [90.0, 120.0, 180.0, 300.0];
const FORMANT_AMP: [f32; 4] = [1.0, 0.65, 0.4, 0.2];

#[derive(Clone, Copy)]
struct SynthCfg {
    mode: u8, // 2 vowels, 3 robot, 4 noise, 5 gate
    pitch: f32,
    rate: f32, // vowel steps per second, 0 = static
    vowel: f32,
    shape: f32,
    cons: f32,
    depth: f32,
    grate: f32,
}

struct Synth {
    phase: f32,
    vib: f32,
    vstep: f32,
    last_idx: i32,
    gp: f32,
    gs: f32,
    cenv: f32,
    prev_g: f32,
    cd_lp: f32,
    ctr: u32,
    st: [Svf; 4],
    co: [Coef; 4],
    rng: Rng,
}
impl Synth {
    fn new() -> Self {
        Synth {
            phase: 0.0,
            vib: 0.0,
            vstep: 0.0,
            last_idx: 0,
            gp: 0.0,
            gs: 0.0,
            cenv: 0.0,
            prev_g: 0.0,
            cd_lp: 0.0,
            ctr: 0,
            st: [Svf::default(); 4],
            co: [Coef::default(); 4],
            rng: Rng(0x1234_5679),
        }
    }
    fn tick(&mut self, c: &SynthCfg, sr: f32) -> f32 {
        // vowel position
        let (i, j, t);
        if c.rate > 0.0 {
            self.vstep += c.rate / sr;
            if self.vstep >= 8.0 {
                self.vstep -= 8.0;
            }
            let fi = self.vstep.floor();
            let frac = self.vstep - fi;
            let x = ((frac - 0.7) / 0.3).clamp(0.0, 1.0);
            i = (fi as usize) & 7;
            j = (i + 1) & 7;
            t = x * x * (3.0 - 2.0 * x);
        } else {
            let p = (c.vowel * 7.0).clamp(0.0, 7.0);
            let fi = p.floor().min(6.0);
            i = fi as usize;
            j = i + 1;
            t = p - fi;
        }
        let mut trig = false;
        if c.rate > 0.0 && i as i32 != self.last_idx {
            trig = true;
        }
        self.last_idx = i as i32;
        // gate
        let mut gate = 1.0;
        if c.depth > 0.001 {
            self.gp += c.grate / sr;
            if self.gp >= 1.0 {
                self.gp -= 1.0;
                trig = true;
            }
            let target = if self.gp < 0.4 { 1.0 } else { 0.0 };
            self.gs += (target - self.gs) * (1.0 - (-1.0 / (0.0015 * sr)).exp());
            gate = 1.0 - c.depth + c.depth * self.gs;
        }
        if trig && c.cons > 0.0 {
            self.cenv = 1.0;
        }
        // formant coefficients at control rate
        if self.ctr & 15 == 0 {
            for k in 0..4 {
                let f = if k < 3 { VOWELS[i][k] + (VOWELS[j][k] - VOWELS[i][k]) * t } else { 3500.0 };
                self.co[k] = Coef::bp(f, f / FORMANT_BW[k], sr);
            }
        }
        self.ctr = self.ctr.wrapping_add(1);
        // glottal source
        let vib = if c.mode == 3 { 0.0 } else { (self.vib * 2.0 * PI).sin() * 0.25 };
        self.vib += 5.5 / sr;
        if self.vib > 1.0 {
            self.vib -= 1.0;
        }
        let f0 = (c.pitch * (vib / 12.0).exp2()).clamp(20.0, 0.4 * sr);
        self.phase += f0 / sr;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let p = self.phase;
        let ex = if c.mode == 4 {
            self.rng.next() * 3.0
        } else {
            let g = if p < 0.6 {
                0.5 * (1.0 - (PI * p / 0.6).cos())
            } else if p < 0.8 {
                (0.5 * PI * (p - 0.6) / 0.2).cos()
            } else {
                0.0
            };
            let d = (g - self.prev_g) * (sr / f0) * 0.4;
            self.prev_g = g;
            let sh = if c.mode == 3 { 0.85 } else { c.shape };
            (1.0 - 2.0 * p) * (1.0 - sh) * 0.6 + d * sh
        };
        let mut y = 0.0;
        for k in 0..4 {
            y += FORMANT_AMP[k] * self.st[k].tick(&self.co[k], ex);
        }
        y *= 5.0;
        // consonant burst: high-passed noise with fast decay
        if self.cenv > 1e-4 {
            let n = self.rng.next();
            self.cd_lp += (n - self.cd_lp) * 0.3;
            y += (n - self.cd_lp) * self.cenv * c.cons * 0.9;
            self.cenv *= (-1.0 / (0.03 * sr)).exp();
        } else {
            self.cenv = 0.0;
        }
        (y * gate).clamp(-1.5, 1.5)
    }
}

pub struct Vocoder {
    sr: f32,
    p: [f32; NP],
    n: usize,
    ana: [Coef; MAXB],
    syn: [Coef; MAXB],
    ana_s: [[Svf; 2]; MAXB],
    syn_s: [[[Svf; 2]; MAXB]; 2],
    env: [f32; MAXB],
    cenv: [f32; MAXB],
    flat: [f32; MAXB],
    pl: [f32; MAXB],
    pr: [f32; MAXB],
    dirty: bool,
    aux: Vec<f32>,
    has_aux: bool,
    synth: Synth,
    rng: Rng,
    hf_lp: [f32; 2],
    nz_lp: [f32; 2],
    henv: f32,
    ctr: u32,
}

impl Vocoder {
    pub fn new(sr: f32) -> Self {
        let sr = if sr.is_finite() { sr.clamp(8000.0, 192000.0) } else { 44100.0 };
        let mut p = [0.0; NP];
        for (i, (_, d)) in PARAMS.iter().enumerate() {
            p[i] = *d;
        }
        let mut v = Vocoder {
            sr,
            p,
            n: 16,
            ana: [Coef::default(); MAXB],
            syn: [Coef::default(); MAXB],
            ana_s: [[Svf::default(); 2]; MAXB],
            syn_s: [[[Svf::default(); 2]; MAXB]; 2],
            env: [0.0; MAXB],
            cenv: [0.0; MAXB],
            flat: [1.0; MAXB],
            pl: [1.0; MAXB],
            pr: [1.0; MAXB],
            dirty: true,
            aux: Vec::with_capacity(16384),
            has_aux: false,
            synth: Synth::new(),
            rng: Rng(0x9e37_79b9),
            hf_lp: [0.0; 2],
            nz_lp: [0.0; 2],
            henv: 0.0,
            ctr: 0,
        };
        v.rebuild();
        v
    }

    fn band_count(&self) -> usize {
        (8 + (self.p[I_BANDS] * 24.0).round() as usize).clamp(8, MAXB)
    }

    fn rebuild(&mut self) {
        self.dirty = false;
        let sr = self.sr;
        let n = self.band_count();
        self.n = n;
        let lo = (40.0 * (500.0f32 / 40.0).powf(self.p[I_LO])).min(0.3 * sr);
        let hi = (2500.0 * (16000.0f32 / 2500.0).powf(self.p[I_HI])).min(0.45 * sr).max(lo * 2.0);
        let ratio = hi / lo;
        let spacing_oct = ratio.log2() / (n as f32 - 1.0);
        let half = (spacing_oct * 0.5).exp2();
        let base_q = 1.0 / (half - 1.0 / half);
        let q = (base_q * (0.6 + self.p[I_Q] * 1.8)).clamp(1.5, 40.0);
        let shift = (self.p[I_FORMANT] - 0.5) * 24.0;
        let sh = (shift / 12.0).exp2();
        let w = self.p[I_WIDTH];
        for b in 0..n {
            let fa = lo * ratio.powf(b as f32 / (n as f32 - 1.0));
            self.ana[b] = Coef::bp(fa, q, sr);
            self.syn[b] = Coef::bp((fa * sh).clamp(30.0, 0.45 * sr), q, sr);
            if b & 1 == 0 {
                self.pl[b] = 1.0;
                self.pr[b] = 1.0 - w;
            } else {
                self.pl[b] = 1.0 - w;
                self.pr[b] = 1.0;
            }
        }
        for b in n..MAXB {
            self.env[b] = 0.0;
            self.cenv[b] = 0.0;
        }
    }

    fn db(v: f32, lo: f32, hi: f32) -> f32 {
        (10.0f32).powf((lo + v * (hi - lo)) / 20.0)
    }
}

#[inline]
fn limit(x: f32) -> f32 {
    let a = x.abs();
    if a < 0.8 {
        x
    } else {
        (0.8 + 0.2 * ((a - 0.8) / 0.2).tanh()).copysign(x)
    }
}

impl Effect for Vocoder {
    fn process(&mut self, buf: &mut [f32]) {
        if self.dirty {
            self.rebuild();
        }
        let sr = self.sr;
        let n = self.n;
        let p = self.p;
        let src = ((p[I_SOURCE] * 6.0) as usize).min(5);
        let use_aux = src == 1 || (src == 0 && self.has_aux);
        let mode: u8 = match src {
            3 => 3,
            4 => 4,
            5 => 5,
            _ => 2,
        };
        let rate = if p[I_RATE] < 0.02 { 0.0 } else { p[I_RATE] * 8.0 };
        let cfg = SynthCfg {
            mode,
            pitch: 55.0 * 8.0f32.powf(p[I_PITCH]),
            rate,
            vowel: p[I_VOWEL],
            shape: p[I_SHAPE],
            cons: p[I_CONS],
            depth: if src == 5 { p[I_GATE].max(0.9) } else { p[I_GATE] },
            grate: 24.0f32.powf(p[I_GRATE]),
        };
        let mg = Self::db(p[I_MODGAIN], -24.0, 24.0);
        let cg = Self::db(p[I_CARGAIN], -24.0, 24.0);
        let og = Self::db(p[I_OUT], -24.0, 12.0);
        let mix = p[I_MIX];
        let hiss = p[I_HISS];
        let flatten = p[I_FLATTEN];
        let freeze = p[I_FREEZE] > 0.5;
        let att = 1.0 - (-1.0 / ((2.0 * 40.0f32.powf(p[I_ATTACK])) * 0.001 * sr)).exp();
        let rel = 1.0 - (-1.0 / ((5.0 * 100.0f32.powf(p[I_RELEASE])) * 0.001 * sr)).exp();
        let cc = 1.0 - (-1.0 / (0.02 * sr)).exp();
        let fc = (5000.0f32).min(0.3 * sr);
        let hp_a = 1.0 - (-2.0 * PI * fc / sr).exp();
        let aux_len = if self.has_aux { self.aux.len() } else { 0 };

        let mut idx = 0usize;
        for fr in buf.chunks_exact_mut(2) {
            let dl = fr[0];
            let dr = fr[1];
            let l = if dl.is_finite() { dl.clamp(-16.0, 16.0) } else { 0.0 };
            let r = if dr.is_finite() { dr.clamp(-16.0, 16.0) } else { 0.0 };
            // modulator
            let m = if use_aux {
                let i = idx * 2;
                if i + 1 < aux_len {
                    (self.aux[i] + self.aux[i + 1]) * 0.5
                } else if i < aux_len {
                    self.aux[i]
                } else {
                    0.0
                }
            } else {
                self.synth.tick(&cfg, sr)
            };
            idx += 1;
            let m = if m.is_finite() { (m * mg).clamp(-16.0, 16.0) } else { 0.0 } + 1e-20;
            // sibilance detector
            self.hf_lp[0] += (m - self.hf_lp[0]) * hp_a;
            let h1 = m - self.hf_lp[0];
            self.hf_lp[1] += (h1 - self.hf_lp[1]) * hp_a;
            let h2 = h1 - self.hf_lp[1];
            if !freeze {
                let rr = h2.abs();
                self.henv += (rr - self.henv) * if rr > self.henv { att } else { rel };
            }
            // control-rate flattening gains
            if self.ctr & 15 == 0 {
                let mut cref = 0.0;
                for b in 0..n {
                    cref += self.cenv[b];
                }
                cref /= n as f32;
                for b in 0..n {
                    self.flat[b] = if flatten < 0.001 { 1.0 } else { (cref / (self.cenv[b] + 1e-5)).min(30.0).powf(flatten) };
                }
            }
            self.ctr = self.ctr.wrapping_add(1);
            let mut yl = 0.0;
            let mut yr = 0.0;
            let cl = l * cg + 1e-20;
            let cr = r * cg + 1e-20;
            for b in 0..n {
                let a = tick2(&mut self.ana_s[b], &self.ana[b], m);
                let ar = a.abs();
                let sl = tick2(&mut self.syn_s[0][b], &self.syn[b], cl);
                let sr_ = tick2(&mut self.syn_s[1][b], &self.syn[b], cr);
                if !freeze {
                    self.env[b] += (ar - self.env[b]) * if ar > self.env[b] { att } else { rel };
                    let cr_ = (sl.abs() + sr_.abs()) * 0.5;
                    self.cenv[b] += (cr_ - self.cenv[b]) * cc;
                }
                let g = self.env[b] * self.flat[b] * MAKEUP;
                yl += sl * g * self.pl[b];
                yr += sr_ * g * self.pr[b];
            }
            // unvoiced noise from the modulator's high-frequency energy
            let hn = self.henv * hiss * 4.0;
            if hn > 1e-6 {
                let nl = self.rng.next();
                let nr = self.rng.next();
                self.nz_lp[0] += (nl - self.nz_lp[0]) * hp_a;
                self.nz_lp[1] += (nr - self.nz_lp[1]) * hp_a;
                yl += (nl - self.nz_lp[0]) * hn;
                yr += (nr - self.nz_lp[1]) * hn;
            }
            let wl = limit(yl * og);
            let wr = limit(yr * og);
            let wl = if wl.is_finite() { wl } else { 0.0 };
            let wr = if wr.is_finite() { wr } else { 0.0 };
            fr[0] = dl * (1.0 - mix) + wl * mix;
            fr[1] = dr * (1.0 - mix) + wr * mix;
        }
        self.has_aux = false;
    }

    fn set_param(&mut self, name: &str, v: f32) {
        if let Some(i) = PARAMS.iter().position(|(n, _)| *n == name) {
            let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { PARAMS[i].1 };
            if self.p[i] != v {
                self.p[i] = v;
                if matches!(i, I_BANDS | I_LO | I_HI | I_Q | I_FORMANT | I_WIDTH) {
                    self.dirty = true;
                }
            }
        }
    }

    fn set_aux(&mut self, aux: Option<&[f32]>) {
        match aux {
            Some(a) => {
                self.aux.clear();
                self.aux.extend_from_slice(a);
                self.has_aux = true;
            }
            None => {
                self.has_aux = false;
            }
        }
    }

    fn name(&self) -> &'static str {
        "Vocoder"
    }

    fn view(&self) -> Vec<f32> {
        let n = self.band_count();
        (0..n.min(MAXB))
            .map(|b| {
                let v = self.env[b] * 2.0;
                if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, seed: u32, amp: f32) -> Vec<f32> {
        let mut r = Rng(seed);
        (0..n).map(|_| r.next() * amp).collect()
    }
    fn inter(m: &[f32]) -> Vec<f32> {
        m.iter().flat_map(|&x| [x, x]).collect()
    }
    fn run(v: &mut Vocoder, carrier: &[f32], modu: Option<&[f32]>) -> Vec<f32> {
        let mut out = Vec::new();
        let blk = 512;
        let mut i = 0;
        while i < carrier.len() {
            let e = (i + blk).min(carrier.len());
            let mut b = inter(&carrier[i..e]);
            match modu {
                Some(m) => v.set_aux(Some(&inter(&m[i..e]))),
                None => v.set_aux(None),
            }
            v.process(&mut b);
            out.extend(b.chunks(2).map(|c| c[0]));
            i = e;
        }
        out
    }
    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn follows_modulator_band() {
        let sr = 44100.0;
        let len = (sr * 1.6) as usize;
        let carrier: Vec<f32> = {
            let mut ph = 0.0f32;
            (0..len)
                .map(|_| {
                    ph += 110.0 / sr;
                    if ph >= 1.0 {
                        ph -= 1.0;
                    }
                    (1.0 - 2.0 * ph) * 0.5
                })
                .collect()
        };
        let nz = noise(len, 77, 1.0);
        let c = Coef::bp(1000.0, 6.0, sr);
        let mut s = Svf::default();
        let period = (sr * 0.4) as usize;
        let on = (sr * 0.2) as usize;
        let modu: Vec<f32> = (0..len).map(|i| if i % period < on { s.tick(&c, nz[i]) * 8.0 } else { s.tick(&c, 0.0) * 0.0 }).collect();
        let mut v = Vocoder::new(sr);
        for (k, val) in [("source", 0.25), ("hiss", 0.0), ("flatten", 0.0), ("release", 0.1), ("attack", 0.05), ("bands", 0.5), ("lo", 0.3), ("hi", 0.6)] {
            v.set_param(k, val);
        }
        let out = run(&mut v, &carrier, Some(&modu));
        // energy near 1 kHz
        let mut oc = Svf::default();
        let ob = Coef::bp(1000.0, 5.0, sr);
        let near: Vec<f32> = out.iter().map(|&x| oc.tick(&ob, x)).collect();
        let w = (sr * 0.1) as usize;
        let mut on_e = 0.0;
        let mut off_e = 0.0;
        for k in 0..4 {
            let s0 = k * period;
            on_e += rms(&near[s0 + on - w..s0 + on]);
            off_e += rms(&near[s0 + period - w..s0 + period]);
        }
        assert!(on_e > 8.0 * off_e, "on {on_e} off {off_e}");
        // envelope correlation
        let blk = 441;
        let nb = len / blk;
        let a: Vec<f32> = (0..nb).map(|i| rms(&modu[i * blk..(i + 1) * blk])).collect();
        let b: Vec<f32> = (0..nb).map(|i| rms(&out[i * blk..(i + 1) * blk])).collect();
        let ma = a.iter().sum::<f32>() / nb as f32;
        let mb = b.iter().sum::<f32>() / nb as f32;
        let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
        for i in 0..nb {
            sab += (a[i] - ma) * (b[i] - mb);
            saa += (a[i] - ma).powi(2);
            sbb += (b[i] - mb).powi(2);
        }
        let corr = sab / (saa * sbb).sqrt().max(1e-12);
        assert!(corr > 0.6, "corr {corr}");
    }

    fn centroid(x: &[f32], sr: f32) -> f32 {
        let (mut num, mut den) = (0.0, 0.0);
        let mut f = 100.0f32;
        while f < 5000.0 {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, &s) in x.iter().enumerate() {
                let a = 2.0 * PI * f * i as f32 / sr;
                re += s * a.cos();
                im += s * a.sin();
            }
            let m = re * re + im * im;
            num += f * m;
            den += m;
            f += 50.0;
        }
        num / den.max(1e-20)
    }

    #[test]
    fn builtin_modulator_cycles_vowels() {
        let sr = 44100.0;
        let len = (sr * 2.0) as usize;
        let carrier = noise(len, 5, 0.5);
        let mut v = Vocoder::new(sr);
        for (k, val) in [("source", 0.42), ("rate", 0.25), ("hiss", 0.0), ("cons", 0.0), ("flatten", 0.8), ("bands", 1.0), ("q", 0.8), ("release", 0.1)] {
            v.set_param(k, val);
        }
        let out = run(&mut v, &carrier, None);
        assert!(rms(&out) > 0.005, "silent {}", rms(&out));
        let seg = (sr * 0.25) as usize;
        let cs: Vec<f32> = (0..8).map(|k| centroid(&out[k * seg + seg / 2 - 1024..k * seg + seg / 2 + 1024], sr)).collect();
        let mx = cs.iter().cloned().fold(f32::MIN, f32::max);
        let mn = cs.iter().cloned().fold(f32::MAX, f32::min);
        assert!(mx - mn > 150.0, "centroids {cs:?}");
        // auto mode with nothing routed also speaks
        let mut v2 = Vocoder::new(sr);
        let out2 = run(&mut v2, &carrier[..8192], None);
        assert!(rms(&out2) > 0.001);
    }

    fn stress(v: &mut Vocoder) {
        let loud = noise(1024, 3, 1.0);
        let aux = noise(1024, 4, 1.0);
        for _ in 0..4 {
            let mut b = inter(&loud);
            v.set_aux(Some(&inter(&aux)));
            v.process(&mut b);
            for x in b {
                assert!(x.is_finite() && x.abs() < 4.0, "bad {x}");
            }
        }
    }

    #[test]
    fn finite_and_bounded() {
        for (name, ov) in PRESETS.iter() {
            for sr in [22050.0, 44100.0, 96000.0] {
                let mut v = Vocoder::new(sr);
                for (k, x) in ov.iter() {
                    v.set_param(k, *x);
                }
                stress(&mut v);
            }
            let _ = name;
        }
        for sr in [22050.0, 96000.0] {
            for val in [0.0, 1.0] {
                for (k, _) in PARAMS.iter() {
                    let mut v = Vocoder::new(sr);
                    v.set_param(k, val);
                    stress(&mut v);
                }
                let mut v = Vocoder::new(sr);
                for (k, _) in PARAMS.iter() {
                    v.set_param(k, val);
                }
                stress(&mut v);
            }
        }
        let mut v = Vocoder::new(44100.0);
        v.set_param("nonexistent", 0.5);
        for (name, _) in PARAMS.iter() {
            v.set_param(name, 0.5);
        }
        assert_eq!(v.name(), "Vocoder");
    }

    #[test]
    fn aux_length_tolerant() {
        let mut v = Vocoder::new(44100.0);
        v.set_param("source", 0.25);
        for alen in [0usize, 2, 100, 1024, 4000, 4001] {
            let mut b = vec![0.3f32; 2048];
            let a = vec![0.2f32; alen];
            v.set_aux(Some(&a));
            v.process(&mut b);
            assert!(b.iter().all(|x| x.is_finite()));
        }
        let mut odd = vec![0.1f32; 7];
        v.set_aux(None);
        v.process(&mut odd);
    }

    #[test]
    fn deterministic() {
        let c = noise(8192, 11, 0.5);
        let m = noise(8192, 12, 0.5);
        let mut a = Vocoder::new(44100.0);
        let mut b = Vocoder::new(44100.0);
        assert_eq!(run(&mut a, &c, Some(&m)), run(&mut b, &c, Some(&m)));
        let mut a = Vocoder::new(44100.0);
        let mut b = Vocoder::new(44100.0);
        assert_eq!(run(&mut a, &c, None), run(&mut b, &c, None));
    }

    #[test]
    fn view_matches_bands() {
        for bands in [0.0, 0.25, 0.5, 1.0] {
            let mut v = Vocoder::new(44100.0);
            v.set_param("bands", bands);
            let want = 8 + (bands * 24.0f32).round() as usize;
            let c = noise(4096, 1, 0.5);
            let m = noise(4096, 2, 0.8);
            run(&mut v, &c, Some(&m));
            let w = v.view();
            assert_eq!(w.len(), want);
            assert!(w.iter().all(|x| (0.0..=1.0).contains(x)));
        }
    }
}
