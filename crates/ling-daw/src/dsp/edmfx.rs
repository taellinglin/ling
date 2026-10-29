//! Effects for EDM production and mastering:
//!
//! * **Colour Bass** - a bank of tuned resonators (or tuned feedback combs) whose tuning follows the chord that is playing, read live
//!   from the song's notes (`crate::fxctx::feed_keys`): any audio - a growl, a noise, a drum loop - comes out singing the chord. The
//!   resonators sit on every chord tone over a few octaves, so the input's energy is pulled onto the chord's notes (the pitch-map /
//!   spectral-resonator way of making "colour bass"); a level follower keeps the output as loud as the input.
//! * **Param EQ** - six bands (bell, low / high shelf, low / high cut at 12 or 24 dB per octave), each on stereo, mid or side: the
//!   classic bass move is a 24 dB low cut at 120 Hz on the side only, which keeps the lows mono.
//! * **Volume Shaper** - a tempo-synced volume curve (LFO-tool style): sidechain pump shapes, gates, trance-gate patterns, ramps or a
//!   drawn curve, at note values from 4 bars to 1/32 including triplets and dotted values.
//! * **Hyper Chorus** - up to eight modulated delay voices, each with its own delay, rate and side: a dense detuned unison from one
//!   voice (the "hyper" / dimension-expander technique).
//! * **Stutter** - a tempo-synced buffer repeat: at each grid step it may (by chance, by pattern, or always while `trigger` is up)
//!   repeat a slice, play the last slice backwards, gate it, or stop like a tape; deterministic, so a render sounds like playback.
//! * **Limiter** - a look-ahead true-peak limiter: peaks are found between the samples (4x oversampled), the gain reaches them before
//!   they arrive and comes back with the release; the output stays under the ceiling in dBTP. It also meters loudness (momentary,
//!   short-term and integrated LUFS, BS.1770) for the master.
//!
//! Everything is allocation-free in `process`, finite for any input and setting (tests below), and stays cheap enough for a Pi.
use super::bassfx::{db2lin, fin, g_of, lin2db, Line, Lr4, Tsvf};
use super::fx::{Effect, KeyFollow, KeyIn, KeySource};
use crate::dsp::midi_hz;

#[inline]
fn choice(v: f32, n: usize) -> usize {
    ((v.clamp(0.0, 1.0) * n as f32) as usize).min(n - 1)
}

/// a cheap sine for LFOs (phase 0..1), within 0.1 % of sin
#[inline]
fn fsin(ph: f32) -> f32 {
    let x = ph - ph.floor();
    // sin(2 pi x) = -sin(pi z) for z = 2x - 1; a parabola for sin(pi z), then one correction step
    let z = x * 2.0 - 1.0;
    let p = 4.0 * z * (1.0 - z.abs());
    -(0.225 * (p * p.abs() - p) + p)
}

/// note names for the chord readout
const PC_NAMES: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

// ── Colour Bass ────────────────────────────────────────────────────────────────

pub const COLOUR_PARAMS: [(&str, f32); 13] = [
    ("follow", 0.0),
    ("track", 0.0),
    ("mode", 0.0),
    ("q", 0.5),
    ("low", 0.33),
    ("span", 0.5),
    ("glide", 0.4),
    ("tilt", 0.5),
    ("keep lo", 0.5),
    ("spread", 0.3),
    ("drive", 0.0),
    ("out", 0.5),
    ("mix", 1.0),
];

const MAXR: usize = 24;
const MAXC: usize = 6;
/// control rate of the resonator tuning (samples)
const CTL: u32 = 16;

#[derive(Clone, Copy, Default)]
struct Res {
    /// pitch now and where it is going (MIDI semitones)
    p: f32,
    pt: f32,
    /// gain now and its target
    g: f32,
    gt: f32,
    /// per channel: a1, a2, a3, k of the TPT band-pass
    c: [[f32; 4]; 2],
    /// per channel: the two integrator states
    s: [[f32; 2]; 2],
    /// comb delays in samples, per channel
    d: [f32; 2],
    on: bool,
}

pub struct ColourBass {
    sr: f32,
    key: KeyIn,
    follow: usize,
    track: usize,
    mode: usize,
    q: f32,
    low: f32,
    span: usize,
    glide: f32,
    tilt: f32,
    keep_hz: f32,
    spread: f32,
    drive: f32,
    out: f32,
    mix: f32,
    /// the chord: a bit per pitch class
    mask: u16,
    res: [Res; MAXR],
    lines: Vec<Line>,
    comb_lp: [[f32; MAXC]; 2],
    split: [Lr4; 2],
    /// level matching: input and output power, the gain
    e_in: f32,
    e_out: f32,
    ag: f32,
    known: f32,
    ctl: u32,
    retune: bool,
}

impl ColourBass {
    pub fn new(sr: f32) -> ColourBass {
        let n = (sr / 20.0) as usize + 8;
        ColourBass {
            sr,
            key: KeyIn::with_src(KeySource::Auto),
            follow: 0,
            track: 0,
            mode: 0,
            q: 45.0,
            low: 36.0,
            span: 3,
            glide: 0.015,
            tilt: 0.0,
            keep_hz: 113.0,
            spread: 0.0,
            drive: 0.0,
            out: 0.0,
            mix: 1.0,
            mask: 0,
            res: [Res::default(); MAXR],
            lines: (0..2 * MAXC).map(|_| Line::new(n)).collect(),
            comb_lp: [[0.0; MAXC]; 2],
            split: [Lr4::default(); 2],
            e_in: 0.0,
            e_out: 0.0,
            ag: 1.0,
            known: 0.0,
            ctl: 0,
            retune: false,
        }
    }

    fn key_src(&self) -> KeySource {
        match self.follow {
            1 => KeySource::Own,
            2 => KeySource::Track(self.track),
            _ => KeySource::Auto,
        }
    }

    /// the chord as pitch classes from MIDI keys; a lone note gets its fifth
    pub fn mask_of(keys: &[u8]) -> u16 {
        let mut m = 0u16;
        for k in keys {
            m |= 1 << (k % 12);
        }
        if m.count_ones() == 1 {
            let pc = m.trailing_zeros();
            m |= 1 << ((pc + 7) % 12);
        }
        m
    }

    /// set the chord directly (tests, or a host that knows the chord)
    pub fn set_chord(&mut self, mask: u16) {
        if mask != 0 && mask != self.mask {
            self.mask = mask & 0xfff;
            self.retune = true;
        }
    }

    /// the resonator targets for the current chord
    fn retarget(&mut self) {
        let mut notes = [0f32; MAXR];
        let mut n = 0;
        if self.mode == 0 {
            // every chord tone from `low` over `span` octaves
            let lo = self.low.round() as i32;
            let hi = lo + 12 * self.span as i32;
            let mut k = lo;
            while k < hi && n < MAXR {
                if self.mask & (1 << (k.rem_euclid(12))) != 0 && midi_hz(k as f32) < self.sr * 0.42 {
                    notes[n] = k as f32;
                    n += 1;
                }
                k += 1;
            }
        } else {
            // one comb per chord tone, at its first note from `low` up: the comb rings on that note's harmonics
            let lo = self.low.round() as i32;
            for k in lo..lo + 12 {
                if n < MAXC && self.mask & (1 << (k.rem_euclid(12))) != 0 {
                    notes[n] = k as f32;
                    n += 1;
                }
            }
        }
        let tilt_db = (self.tilt - 0.5) * 12.0;
        for (i, r) in self.res.iter_mut().enumerate() {
            if i < n {
                r.pt = notes[i];
                // a resonator coming in from silence starts at its note, the others glide
                if r.g < 1e-3 {
                    r.p = r.pt;
                    r.s = [[0.0; 2]; 2];
                }
                let oct = (r.pt - self.low) / 12.0;
                r.gt = if self.mode == 0 { db2lin(tilt_db * oct) } else { 1.0 };
                r.on = true;
            } else {
                r.gt = 0.0;
                r.on = false;
            }
        }
    }

    fn update(&mut self, kg: f32) {
        let k = 1.0 / self.q;
        // the left / right detune, semitones
        let cents = self.spread * 0.01;
        for r in self.res.iter_mut() {
            if !r.on && r.g < 1e-5 {
                r.g = 0.0;
                continue;
            }
            r.p += (r.pt - r.p) * kg;
            r.g += (r.gt - r.g) * kg.max(0.2);
            for c in 0..2 {
                let p = r.p + if c == 0 { -cents } else { cents };
                let hz = midi_hz(p).min(self.sr * 0.45);
                let g = (std::f32::consts::PI * hz / self.sr).tan();
                let a1 = 1.0 / (1.0 + g * (g + k));
                let a2 = g * a1;
                r.c[c] = [a1, a2, g * a2, k];
                r.d[c] = (self.sr / hz.max(15.0)).max(2.0);
                if !(r.s[c][0].is_finite() && r.s[c][1].is_finite()) {
                    r.s[c] = [0.0; 2];
                }
            }
        }
    }

    pub fn chord_text(&self) -> String {
        if self.mask == 0 {
            return String::new();
        }
        (0..12).filter(|p| self.mask & (1 << p) != 0).map(|p| PC_NAMES[p]).collect::<Vec<_>>().join(" ")
    }
}

impl Effect for ColourBass {
    fn process(&mut self, buf: &mut [f32]) {
        if self.key.fresh {
            self.key.fresh = false;
            let m = Self::mask_of(self.key.keys());
            self.set_chord(m);
        }
        if self.retune {
            self.retune = false;
            self.retarget();
        }
        let sr = self.sr;
        let kg = 1.0 - (-(CTL as f32) / (self.glide * sr)).exp();
        let ke = 1.0 - (-1.0 / (0.08 * sr)).exp();
        let ka = 1.0 - (-1.0 / (0.04 * sr)).exp();
        let kk = 1.0 - (-1.0 / (0.02 * sr)).exp();
        let keep = self.keep_hz > 1.0;
        let gk = g_of(self.keep_hz.max(20.0), sr);
        let dg = 1.0 + self.drive * self.drive * 16.0;
        let dn = 1.0 / dg.sqrt();
        let outg = db2lin(self.out);
        let fb = (1.0 - 3.0 / self.q).clamp(0.5, 0.995);
        let damp = 0.25 + self.tilt * 0.7;
        let known_t = if self.mask != 0 { 1.0 } else { 0.0 };
        for fr in buf.chunks_exact_mut(2) {
            if self.ctl == 0 {
                self.update(kg);
            }
            self.ctl = (self.ctl + 1) % CTL;
            let x = [fin(fr[0]), fin(fr[1])];
            let mut lo = [0.0f32; 2];
            let mut inp = x;
            if keep {
                for c in 0..2 {
                    let (l, h) = self.split[c].split(x[c], gk);
                    lo[c] = l;
                    inp[c] = h;
                }
            }
            if self.drive > 0.001 {
                for v in inp.iter_mut() {
                    *v = (*v * dg).tanh() * dn;
                }
            }
            let mut sum = [0.0f32; 2];
            if self.mode == 0 {
                for r in self.res.iter_mut() {
                    if r.g <= 0.0 {
                        continue;
                    }
                    for c in 0..2 {
                        let [a1, a2, a3, k] = r.c[c];
                        let [ic1, ic2] = r.s[c];
                        let v3 = inp[c] - ic2;
                        let v1 = a1 * ic1 + a2 * v3;
                        let v2 = ic2 + a2 * ic1 + a3 * v3;
                        r.s[c] = [2.0 * v1 - ic1, 2.0 * v2 - ic2];
                        sum[c] += k * v1 * r.g;
                    }
                }
            } else {
                for i in 0..MAXC {
                    let r = &self.res[i];
                    for c in 0..2 {
                        let line = &mut self.lines[c * MAXC + i];
                        if r.g > 0.0 {
                            let y = line.read(r.d[c]);
                            let lp = &mut self.comb_lp[c][i];
                            *lp += (y - *lp) * damp;
                            line.push(inp[c] + *lp * fb);
                            sum[c] += y * r.g;
                        } else {
                            // idle combs keep hearing the input, so one that comes in has a full line
                            line.push(inp[c]);
                        }
                    }
                }
            }
            // level matching: the output as loud as what went into the bank (at most +30 dB, so a bank with nothing to ring stays quiet)
            let pi = 0.5 * (inp[0] * inp[0] + inp[1] * inp[1]);
            let po = 0.5 * (sum[0] * sum[0] + sum[1] * sum[1]);
            self.e_in += (pi - self.e_in) * ke;
            self.e_out += (po - self.e_out) * ke;
            if self.e_in > 1e-10 {
                let t = (self.e_in / (self.e_out + 1e-12)).sqrt().clamp(0.05, 31.6);
                self.ag += (t - self.ag) * ka;
            }
            self.known += (known_t - self.known) * kk;
            let w = self.mix * self.known;
            for c in 0..2 {
                let wet = lo[c] + sum[c] * self.ag;
                fr[c] = fin(x[c] * (1.0 - w) + wet * w * outg);
            }
        }
        if !(self.e_in.is_finite() && self.e_out.is_finite() && self.ag.is_finite()) {
            self.e_in = 0.0;
            self.e_out = 0.0;
            self.ag = 1.0;
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "follow" => {
                self.follow = choice(v, 3);
                self.key.set_src(Some(self.key_src()));
            }
            "track" => {
                self.track = (v * 31.0).round() as usize;
                self.key.set_src(Some(self.key_src()));
            }
            "mode" => {
                let m = choice(v, 2);
                if m != self.mode {
                    self.mode = m;
                    for r in self.res.iter_mut() {
                        r.g = 0.0;
                    }
                    self.retune = true;
                }
            }
            "q" => self.q = 8.0 * 2f32.powf(v * 5.0),
            "low" => {
                self.low = 24.0 + (v * 36.0).round();
                self.retune = true;
            }
            "span" => {
                self.span = 1 + (v * 4.0).round() as usize;
                self.retune = true;
            }
            "glide" => self.glide = 0.002 * 150f32.powf(v),
            "tilt" => {
                self.tilt = v;
                self.retune = true;
            }
            "keep lo" => self.keep_hz = if v < 0.02 { 0.0 } else { 40.0 * 2f32.powf(v * 3.0) },
            "spread" => self.spread = v * 15.0,
            "drive" => self.drive = v,
            "out" => self.out = (v - 0.5) * 24.0,
            "mix" => self.mix = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Colour Bass"
    }
    fn view(&self) -> Vec<f32> {
        // the chord: one value per pitch class (1 = in the chord), then how far the level matching lifts the bank
        let mut v: Vec<f32> = (0..12).map(|p| if self.mask & (1 << p) != 0 { 1.0 } else { 0.0 }).collect();
        v.push((lin2db(self.ag) / 30.0).clamp(0.0, 1.0));
        v
    }
    fn info(&self, what: &str) -> String {
        match what {
            "chord" => self.chord_text(),
            _ => String::new(),
        }
    }
    fn key_follow(&self) -> Option<KeyFollow> {
        self.key.follow()
    }
    fn set_keys(&mut self, keys: &[u8], from: f64, until: f64) {
        self.key.set(keys, from, until);
    }
}

// ── Param EQ ────────────────────────────────────────────────────────────────────

/// band types, in the order of each band's `type` choice
pub const PEQ_TYPES: [&str; 8] = ["off", "bell", "lo shelf", "hi shelf", "lo cut", "hi cut", "lo cut 24", "hi cut 24"];

pub const PEQ_PARAMS: [(&str, f32); 31] = [
    ("b1 type", 0.0625),
    ("b1 freq", 0.0585),
    ("b1 gain", 0.5),
    ("b1 q", 0.4247),
    ("b1 ch", 0.0),
    ("b2 type", 0.3125),
    ("b2 freq", 0.2322),
    ("b2 gain", 0.5),
    ("b2 q", 0.4247),
    ("b2 ch", 0.0),
    ("b3 type", 0.1875),
    ("b3 freq", 0.4322),
    ("b3 gain", 0.5),
    ("b3 q", 0.5),
    ("b3 ch", 0.0),
    ("b4 type", 0.1875),
    ("b4 freq", 0.6229),
    ("b4 gain", 0.5),
    ("b4 q", 0.5),
    ("b4 ch", 0.0),
    ("b5 type", 0.1875),
    ("b5 freq", 0.7966),
    ("b5 gain", 0.5),
    ("b5 q", 0.5),
    ("b5 ch", 0.0),
    ("b6 type", 0.4375),
    ("b6 freq", 0.8966),
    ("b6 gain", 0.5),
    ("b6 q", 0.4247),
    ("b6 ch", 0.0),
    ("out", 0.5),
];
const NB: usize = 6;

/// a biquad (transposed direct form II, f64 so a 30 Hz bell stays clean)
#[derive(Clone, Copy, Default)]
struct Bq {
    b: [f64; 3],
    a: [f64; 2],
    z: [[f64; 2]; 2],
}
impl Bq {
    #[inline]
    fn run(&mut self, c: usize, x: f64) -> f64 {
        let z = &mut self.z[c];
        let y = self.b[0] * x + z[0];
        z[0] = self.b[1] * x - self.a[0] * y + z[1];
        z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
    /// |H| at `w` radians per sample
    fn mag(&self, w: f64) -> f64 {
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let nr = self.b[0] + self.b[1] * c1 + self.b[2] * c2;
        let ni = -(self.b[1] * s1 + self.b[2] * s2);
        let dr = 1.0 + self.a[0] * c1 + self.a[1] * c2;
        let di = -(self.a[0] * s1 + self.a[1] * s2);
        ((nr * nr + ni * ni) / (dr * dr + di * di).max(1e-30)).sqrt()
    }
}

/// RBJ cookbook coefficients: kind 1 bell, 2 low shelf, 3 high shelf, 4 high-pass (low cut), 5 low-pass (high cut)
fn rbj(kind: usize, f: f64, gain_db: f64, q: f64, sr: f64) -> Bq {
    let w0 = 2.0 * std::f64::consts::PI * f.clamp(10.0, sr * 0.49) / sr;
    let (cw, sw) = (w0.cos(), w0.sin());
    let q = q.max(0.05);
    let alpha = sw / (2.0 * q);
    let a = 10f64.powf(gain_db / 40.0);
    let (b0, b1, b2, a0, a1, a2) = match kind {
        1 => (1.0 + alpha * a, -2.0 * cw, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cw, 1.0 - alpha / a),
        2 => {
            let s = 2.0 * a.sqrt() * alpha;
            (a * ((a + 1.0) - (a - 1.0) * cw + s), 2.0 * a * ((a - 1.0) - (a + 1.0) * cw), a * ((a + 1.0) - (a - 1.0) * cw - s), (a + 1.0) + (a - 1.0) * cw + s, -2.0 * ((a - 1.0) + (a + 1.0) * cw), (a + 1.0) + (a - 1.0) * cw - s)
        }
        3 => {
            let s = 2.0 * a.sqrt() * alpha;
            (a * ((a + 1.0) + (a - 1.0) * cw + s), -2.0 * a * ((a - 1.0) + (a + 1.0) * cw), a * ((a + 1.0) + (a - 1.0) * cw - s), (a + 1.0) - (a - 1.0) * cw + s, 2.0 * ((a - 1.0) - (a + 1.0) * cw), (a + 1.0) - (a - 1.0) * cw - s)
        }
        4 => ((1.0 + cw) / 2.0, -(1.0 + cw), (1.0 + cw) / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
        _ => ((1.0 - cw) / 2.0, 1.0 - cw, (1.0 - cw) / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
    };
    Bq { b: [b0 / a0, b1 / a0, b2 / a0], a: [a1 / a0, a2 / a0], z: [[0.0; 2]; 2] }
}

#[derive(Clone, Copy, Default)]
struct Band {
    kind: usize,
    freq: f32,
    gain: f32,
    q: f32,
    /// 0 stereo, 1 mid, 2 side
    ch: usize,
    st: [Bq; 2],
    /// how many of the two sections run (0 = the band does nothing)
    n: usize,
}

pub struct ParamEq {
    sr: f32,
    band: [Band; NB],
    out: f32,
    dirty: bool,
}

impl ParamEq {
    pub fn new(sr: f32) -> ParamEq {
        ParamEq { sr, band: [Band { kind: 0, freq: 1000.0, gain: 0.0, q: 0.707, ch: 0, st: [Bq::default(); 2], n: 0 }; NB], out: 0.0, dirty: true }
    }

    fn design(&mut self) {
        let sr = self.sr as f64;
        for b in self.band.iter_mut() {
            let (f, g, q) = (b.freq as f64, b.gain as f64, b.q as f64);
            let keep = b.st;
            b.n = 0;
            match b.kind {
                1..=3 => {
                    if g.abs() >= 0.01 {
                        b.st[0] = rbj(b.kind, f, g, q, sr);
                        b.n = 1;
                    }
                }
                4 | 5 => {
                    b.st[0] = rbj(b.kind, f, 0.0, q, sr);
                    b.n = 1;
                }
                6 | 7 => {
                    // 24 dB per octave: a Butterworth pair, its resonance scaled by the band's Q
                    let s = q / std::f64::consts::FRAC_1_SQRT_2;
                    let k = if b.kind == 6 { 4 } else { 5 };
                    b.st[0] = rbj(k, f, 0.0, 0.541_196_1 * s.sqrt(), sr);
                    b.st[1] = rbj(k, f, 0.0, 1.306_563 * s.sqrt(), sr);
                    b.n = 2;
                }
                _ => {}
            }
            // the filter memory carries over a retune (no click when a knob moves)
            for i in 0..2 {
                b.st[i].z = keep[i].z;
            }
        }
        self.dirty = false;
    }

    /// the response in dB at `hz` (all bands, as on the mid / stereo signal)
    pub fn response_db(&mut self, hz: f32) -> f32 {
        if self.dirty {
            self.design();
        }
        let w = 2.0 * std::f64::consts::PI * hz as f64 / self.sr as f64;
        let mut m = 1.0f64;
        for b in &self.band {
            for k in 0..b.n {
                m *= b.st[k].mag(w);
            }
        }
        (20.0 * m.max(1e-9).log10()) as f32 + self.out
    }
}

impl Effect for ParamEq {
    fn process(&mut self, buf: &mut [f32]) {
        if self.dirty {
            self.design();
        }
        let outg = db2lin(self.out) as f64;
        let any = self.band.iter().any(|b| b.n > 0);
        if !any && self.out == 0.0 {
            return;
        }
        for fr in buf.chunks_exact_mut(2) {
            let (mut l, mut r) = (fin(fr[0]) as f64, fin(fr[1]) as f64);
            for b in self.band.iter_mut() {
                if b.n == 0 {
                    continue;
                }
                match b.ch {
                    0 => {
                        for k in 0..b.n {
                            l = b.st[k].run(0, l);
                            r = b.st[k].run(1, r);
                        }
                    }
                    ch => {
                        let mut m = 0.5 * (l + r);
                        let mut s = 0.5 * (l - r);
                        for k in 0..b.n {
                            if ch == 1 {
                                m = b.st[k].run(0, m);
                            } else {
                                s = b.st[k].run(1, s);
                            }
                        }
                        l = m + s;
                        r = m - s;
                    }
                }
            }
            fr[0] = fin((l * outg) as f32);
            fr[1] = fin((r * outg) as f32);
        }
        for b in self.band.iter_mut() {
            for st in b.st.iter_mut() {
                for z in st.z.iter_mut() {
                    if !(z[0].is_finite() && z[1].is_finite()) {
                        *z = [0.0; 2];
                    }
                }
            }
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        if n == "out" {
            self.out = (v - 0.5) * 24.0;
            return;
        }
        let Some(rest) = n.strip_prefix('b') else { return };
        let Some((num, what)) = rest.split_once(' ') else { return };
        let Some(i) = num.parse::<usize>().ok().filter(|i| (1..=NB).contains(i)).map(|i| i - 1) else { return };
        let b = &mut self.band[i];
        match what {
            "type" => b.kind = choice(v, PEQ_TYPES.len()),
            "freq" => b.freq = 20.0 * 2f32.powf(v * 10.0),
            "gain" => b.gain = (v - 0.5) * 36.0,
            "q" => b.q = 0.1 * 100f32.powf(v),
            "ch" => b.ch = choice(v, 3),
            _ => return,
        }
        self.dirty = true;
    }
    fn name(&self) -> &'static str {
        "Param EQ"
    }
    fn view(&self) -> Vec<f32> {
        // the curve: 64 points from 20 Hz to 20 kHz, 0..1 = -24..+24 dB
        let mut e = ParamEq { sr: self.sr, band: self.band, out: self.out, dirty: true };
        (0..64).map(|i| {
            let hz = 20.0 * 1000f32.powf(i as f32 / 63.0);
            ((e.response_db(hz) + 24.0) / 48.0).clamp(0.0, 1.0)
        }).collect()
    }
}

// ── Volume Shaper ───────────────────────────────────────────────────────────────

pub const SHAPES: [&str; 12] = ["pump", "hard pump", "soft pump", "sine", "ramp up", "ramp down", "gate", "trance", "offbeat", "chop", "stutter", "custom"];
pub const RATES: [&str; 15] = ["4 bars", "2 bars", "1 bar", "1/2", "1/4", "1/8", "1/16", "1/32", "1/2T", "1/4T", "1/8T", "1/16T", "1/4D", "1/8D", "1/16D"];
/// the length of one cycle of each rate, in beats (quarter notes)
pub const RATE_BEATS: [f64; 15] = [16.0, 8.0, 4.0, 2.0, 1.0, 0.5, 0.25, 0.125, 4.0 / 3.0, 2.0 / 3.0, 1.0 / 3.0, 1.0 / 6.0, 1.5, 0.75, 0.375];

pub const SHAPER_PARAMS: [(&str, f32); 6] = [("shape", 0.5 / 12.0), ("rate", 4.5 / 15.0), ("depth", 0.85), ("phase", 0.0), ("smooth", 0.15), ("mix", 1.0)];

const TBL: usize = 256;
const NPTS: usize = 32;

/// 16-step patterns of the gate shapes (1 = open)
const PAT_TRANCE: [u8; 16] = [1, 1, 0, 1, 1, 0, 1, 1, 0, 1, 1, 0, 1, 0, 1, 1];
const PAT_OFFBEAT: [u8; 16] = [0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1];
const PAT_CHOP: [u8; 16] = [1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0];
const PAT_STUTTER: [u8; 16] = [1, 1, 1, 1, 0, 0, 1, 0, 1, 1, 1, 1, 1, 0, 1, 0];

/// the volume (0..1) of shape `s` at `x` (0..1 through one cycle)
pub fn shape_at(s: usize, x: f32, custom: &[f32; NPTS]) -> f32 {
    let x = x - x.floor();
    // the dip at the start of a pump takes 1.5 % of the cycle, so it never clicks
    // the volume falls in the last 1.5 % of the cycle, so it is at the bottom right on the beat (where the kick is) and never clicks
    let pump = |len: f32, pow: f32| -> f32 {
        let a = 0.015;
        if x >= 1.0 - a {
            return (1.0 - x) / a;
        }
        let u = (x / len).min(1.0);
        if pow >= 1.0 {
            u.powf(pow)
        } else {
            (u * std::f32::consts::FRAC_PI_2).sin().powf(pow * 2.0)
        }
    };
    let gate16 = |pat: &[u8; 16], open: f32| -> f32 {
        let p = x * 16.0;
        let i = (p as usize).min(15);
        let f = p - i as f32;
        if pat[i] == 1 && f < open {
            // 3 % ramps at the edges of each open step
            (f / 0.03).min(1.0).min((open - f) / 0.03).max(0.0)
        } else {
            0.0
        }
    };
    match s {
        0 => pump(0.55, 0.5),
        1 => pump(0.45, 1.6),
        2 => pump(0.8, 0.35),
        3 => 0.5 - 0.5 * (std::f32::consts::TAU * x).cos(),
        4 => x,
        5 => 1.0 - x,
        6 => {
            if x < 0.5 {
                (x / 0.02).min(1.0).min((0.5 - x) / 0.02)
            } else {
                0.0
            }
        }
        7 => gate16(&PAT_TRANCE, 0.8),
        8 => gate16(&PAT_OFFBEAT, 0.9),
        9 => gate16(&PAT_CHOP, 0.85),
        10 => gate16(&PAT_STUTTER, 0.7),
        _ => {
            let p = x * NPTS as f32;
            let i = (p as usize).min(NPTS - 1);
            let f = p - i as f32;
            custom[i] * (1.0 - f) + custom[(i + 1) % NPTS] * f
        }
    }
    .clamp(0.0, 1.0)
}

pub struct VolumeShaper {
    sr: f32,
    shape: usize,
    rate: usize,
    depth: f32,
    phase: f32,
    smooth: f32,
    mix: f32,
    custom: [f32; NPTS],
    table: [f32; TBL + 1],
    bpm: f64,
    beat: f64,
    last_base: f64,
    g: f32,
    pos: f32,
}

impl VolumeShaper {
    pub fn new(sr: f32) -> VolumeShaper {
        let mut custom = [0f32; NPTS];
        for (i, c) in custom.iter_mut().enumerate() {
            *c = shape_at(0, i as f32 / NPTS as f32, &[1.0; NPTS]);
        }
        let mut v = VolumeShaper { sr, shape: 0, rate: 4, depth: 0.85, phase: 0.0, smooth: 0.15, mix: 1.0, custom, table: [1.0; TBL + 1], bpm: 120.0, beat: 0.0, last_base: f64::NAN, g: 1.0, pos: 0.0 };
        v.build();
        v
    }
    fn build(&mut self) {
        for i in 0..=TBL {
            self.table[i] = shape_at(self.shape, i as f32 / TBL as f32, &self.custom);
        }
        // the last point equals the first, so the cycle wraps cleanly
        self.table[TBL] = self.table[0];
    }
    /// the volume (0..1, before depth) at `x` through the cycle
    #[inline]
    fn at(&self, x: f32) -> f32 {
        let p = (x - x.floor()) * TBL as f32;
        let i = (p as usize).min(TBL - 1);
        let f = p - i as f32;
        self.table[i] * (1.0 - f) + self.table[i + 1] * f
    }
    pub fn cycle_beats(&self) -> f64 {
        RATE_BEATS[self.rate]
    }
}

impl Effect for VolumeShaper {
    fn process(&mut self, buf: &mut [f32]) {
        let bps = self.bpm / 60.0 / self.sr as f64;
        let cyc = self.cycle_beats();
        let ks = if self.smooth < 0.005 { 1.0 } else { 1.0 - (-1.0 / ((0.0002 + self.smooth * 0.012) * self.sr)).exp() };
        for fr in buf.chunks_exact_mut(2) {
            let x = (self.beat / cyc) as f32 + self.phase;
            self.pos = x - x.floor();
            let target = 1.0 - self.depth * (1.0 - self.at(x));
            self.g += (target - self.g) * ks;
            let g = 1.0 + (self.g - 1.0) * self.mix;
            fr[0] = fin(fr[0] * g);
            fr[1] = fin(fr[1] * g);
            self.beat += bps;
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "shape" => {
                let s = choice(v, SHAPES.len());
                if s != self.shape {
                    self.shape = s;
                    self.build();
                }
            }
            "rate" => self.rate = choice(v, RATES.len()),
            "depth" => self.depth = v,
            "phase" => self.phase = v,
            "smooth" => self.smooth = v,
            "mix" => self.mix = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Volume Shaper"
    }
    fn set_transport(&mut self, bpm: f64, beat: f64) {
        if bpm.is_finite() && bpm > 20.0 {
            self.bpm = bpm;
        }
        if !beat.is_finite() {
            return;
        }
        // stopped (the position does not move between blocks): keep time on our own so a live input still pumps
        let moved = self.last_base.is_nan() || (beat - self.last_base).abs() > 1e-9;
        self.last_base = beat;
        if moved {
            self.beat = beat;
        }
    }
    fn view(&self) -> Vec<f32> {
        // 64 points of the curve (after depth), then where the cycle is now
        let mut v: Vec<f32> = (0..64).map(|i| 1.0 - self.depth * (1.0 - self.at(i as f32 / 64.0 + self.phase))).collect();
        v.push(self.pos);
        v
    }
    fn command(&mut self, cmd: &str, args: &[f64], text: &str) -> Vec<f64> {
        match cmd {
            // (index 0..31, value 0..1): one point of the drawn curve
            "point" if args.len() >= 2 => {
                let i = (args[0].max(0.0) as usize).min(NPTS - 1);
                self.custom[i] = (args[1] as f32).clamp(0.0, 1.0);
                if self.shape == SHAPES.len() - 1 {
                    self.build();
                }
                vec![1.0]
            }
            // copy the shape in use into the drawn curve (before drawing over a preset shape)
            "from_shape" => {
                let c = self.custom;
                for i in 0..NPTS {
                    self.custom[i] = shape_at(self.shape, i as f32 / NPTS as f32, &c);
                }
                vec![1.0]
            }
            "points" => self.custom.iter().map(|x| *x as f64).collect(),
            "load_config" => {
                self.load_points(text);
                vec![]
            }
            _ => vec![],
        }
    }
    fn info(&self, what: &str) -> String {
        match what {
            "config" => {
                // only the drawn curve needs saving (the knobs are parameters); the default curve saves nothing
                let def = VolumeShaper::new(self.sr);
                if def.custom == self.custom {
                    return String::new();
                }
                let pts: Vec<String> = self.custom.iter().map(|x| format!("{:.4}", x)).collect();
                format!("{{\"points\":[{}]}}", pts.join(","))
            }
            _ => String::new(),
        }
    }
}

impl VolumeShaper {
    /// read a saved drawn curve back ({"points":[...]})
    pub fn load_points(&mut self, text: &str) -> bool {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return false };
        let Some(a) = v["points"].as_array() else { return false };
        for (i, x) in a.iter().take(NPTS).enumerate() {
            self.custom[i] = (x.as_f64().unwrap_or(1.0) as f32).clamp(0.0, 1.0);
        }
        self.build();
        true
    }
}

// ── Hyper Chorus ────────────────────────────────────────────────────────────────

pub const HYPER_PARAMS: [(&str, f32); 7] = [("voices", 0.5), ("detune", 0.4), ("rate", 0.3), ("spread", 0.8), ("delay", 0.4), ("lo cut", 0.3), ("mix", 0.5)];
const HV: usize = 8;

pub struct HyperChorus {
    sr: f32,
    line: Line,
    ph: [f32; HV],
    hp: [Tsvf; 2],
    voices: usize,
    detune: f32,
    rate: f32,
    spread: f32,
    delay: f32,
    locut: f32,
    mix: f32,
}

impl HyperChorus {
    pub fn new(sr: f32) -> HyperChorus {
        let mut ph = [0f32; HV];
        for (i, p) in ph.iter_mut().enumerate() {
            *p = i as f32 / HV as f32;
        }
        HyperChorus { sr, line: Line::new((sr * 0.06) as usize + 16), ph, hp: [Tsvf::default(); 2], voices: 5, detune: 0.4, rate: 0.6, spread: 0.8, delay: 14.0, locut: 85.0, mix: 0.5 }
    }
}

impl Effect for HyperChorus {
    fn process(&mut self, buf: &mut [f32]) {
        let n = self.voices.clamp(2, HV);
        let sr = self.sr;
        // each voice: its own base delay (spread around the centre), its own LFO rate (golden-ratio steps, so they never line up) and
        // a side; the modulation depth in ms falls with the rate so every voice detunes by about the same number of cents
        let mut base = [0f32; HV];
        let mut inc = [0f32; HV];
        let mut amp = [0f32; HV];
        let mut gl = [0f32; HV];
        let mut gr = [0f32; HV];
        for i in 0..n {
            let u = i as f32 / (n - 1) as f32 - 0.5;
            base[i] = (self.delay * (1.0 + 0.7 * u)) * 0.001 * sr;
            let r = self.rate * (0.62 + 0.76 * ((i as f32 * 0.618_034).fract()));
            inc[i] = r / sr;
            // about detune x 25 cents of pitch swing
            let cents = self.detune * 25.0;
            amp[i] = (cents / 1731.0) / (std::f32::consts::TAU * r.max(0.05)) * sr;
            let side = if i % 2 == 0 { -1.0 } else { 1.0 } * (0.35 + 0.65 * (2.0 * u.abs()).min(1.0));
            let p = side * self.spread;
            gl[i] = (0.5 * (1.0 - p)).sqrt();
            gr[i] = (0.5 * (1.0 + p)).sqrt();
        }
        let norm = 1.2 / (n as f32).sqrt();
        let g = g_of(self.locut, sr);
        for fr in buf.chunks_exact_mut(2) {
            let (l, r) = (fin(fr[0]), fin(fr[1]));
            self.line.push(0.5 * (l + r));
            let (mut wl, mut wr) = (0.0f32, 0.0f32);
            for i in 0..n {
                self.ph[i] += inc[i];
                if self.ph[i] >= 1.0 {
                    self.ph[i] -= 1.0;
                }
                let d = base[i] + amp[i] * fsin(self.ph[i]);
                let y = self.line.read(d.max(1.0));
                wl += y * gl[i];
                wr += y * gr[i];
            }
            let wl = self.hp[0].run(wl * norm, g, std::f32::consts::SQRT_2).2;
            let wr = self.hp[1].run(wr * norm, g, std::f32::consts::SQRT_2).2;
            fr[0] = fin(l * (1.0 - 0.5 * self.mix) + wl * self.mix);
            fr[1] = fin(r * (1.0 - 0.5 * self.mix) + wr * self.mix);
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "voices" => self.voices = 2 + (v * 6.0).round() as usize,
            "detune" => self.detune = v,
            "rate" => self.rate = 0.05 * 2f32.powf(v * 7.0),
            "spread" => self.spread = v,
            "delay" => self.delay = 6.0 + v * 20.0,
            "lo cut" => self.locut = 30.0 * 2f32.powf(v * 5.0),
            "mix" => self.mix = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Hyper Chorus"
    }
}

// ── Stutter ─────────────────────────────────────────────────────────────────────

pub const ST_RATES: [&str; 11] = ["1/2", "1/4", "1/8", "1/16", "1/32", "1/64", "1/4T", "1/8T", "1/16T", "1/8D", "1/16D"];
pub const ST_RATE_BEATS: [f64; 11] = [2.0, 1.0, 0.5, 0.25, 0.125, 0.0625, 2.0 / 3.0, 1.0 / 3.0, 1.0 / 6.0, 0.75, 0.375];
pub const ST_GRIDS: [&str; 5] = ["1/8", "1/4", "1/2", "bar", "2 bars"];
pub const ST_GRID_BEATS: [f64; 5] = [0.5, 1.0, 2.0, 4.0, 8.0];
pub const ST_MODES: [&str; 5] = ["repeat", "reverse", "gate", "tape", "mixed"];
pub const ST_PATTERNS: [&str; 5] = ["any", "bar end", "2 and 4", "half bar", "phrase end"];

pub const STUTTER_PARAMS: [(&str, f32); 10] = [
    ("rate", 3.5 / 11.0),
    ("grid", 1.5 / 5.0),
    ("chance", 0.25),
    ("trigger", 0.0),
    ("mode", 0.0),
    ("pattern", 0.0),
    ("decay", 0.0),
    ("gate", 0.5),
    ("seed", 0.0),
    ("mix", 1.0),
];

/// a hash of the step number and the seed: the same steps stutter on every playback and in every render
#[inline]
fn hash01(k: i64, seed: u32) -> f32 {
    let mut x = (k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (seed as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

pub struct Stutter {
    sr: f32,
    buf: [Vec<f32>; 2],
    w: usize,
    rate: usize,
    grid: usize,
    chance: f32,
    trigger: bool,
    mode: usize,
    pattern: usize,
    decay: f32,
    gate: f32,
    seed: u32,
    mix: f32,
    bpm: f64,
    beat: f64,
    last_base: f64,
    /// the grid step now, the event playing in it (None = dry), where it started in the buffer and how far in it is
    step: i64,
    event: Option<usize>,
    start: usize,
    t: usize,
    tape: f64,
    wet: f32,
}

impl Stutter {
    pub fn new(sr: f32) -> Stutter {
        let n = (sr * 6.0) as usize;
        Stutter { sr, buf: [vec![0.0; n], vec![0.0; n]], w: 0, rate: 3, grid: 1, chance: 0.25, trigger: false, mode: 0, pattern: 0, decay: 0.0, gate: 0.5, seed: 0, mix: 1.0, bpm: 120.0, beat: 0.0, last_base: f64::NAN, step: i64::MIN, event: None, start: 0, t: 0, tape: 0.0, wet: 0.0 }
    }

    /// does grid step `k` (of `grid` beats) fire, and with which mode?
    pub fn decide(&self, k: i64) -> Option<usize> {
        let grid = ST_GRID_BEATS[self.grid];
        let beat = k as f64 * grid;
        let in_bar = beat.rem_euclid(4.0);
        let end = grid.min(4.0);
        let allowed = match self.pattern {
            1 => in_bar >= 4.0 - end - 1e-9,
            2 => {
                let b = in_bar.floor() as i64;
                b == 1 || b == 3
            }
            3 => in_bar >= 2.0 - 1e-9,
            4 => beat.rem_euclid(16.0) >= 16.0 - end - 1e-9,
            _ => true,
        };
        if !allowed && !self.trigger {
            return None;
        }
        if !self.trigger && hash01(k, self.seed) >= self.chance {
            return None;
        }
        let mode = if self.mode == 4 { (hash01(k ^ 0x5bd1, self.seed.wrapping_add(17)) * 4.0) as usize % 4 } else { self.mode };
        Some(mode)
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

impl Effect for Stutter {
    fn process(&mut self, buf: &mut [f32]) {
        let sr = self.sr as f64;
        let bps = self.bpm / 60.0 / sr;
        let grid = ST_GRID_BEATS[self.grid];
        let n = self.buf[0].len();
        let len = ((ST_RATE_BEATS[self.rate] / bps).round() as usize).clamp(16, n / 2);
        let slice = ((grid / bps).round() as usize).clamp(16, n - 1);
        let ramp = ((0.0015 * sr) as usize).clamp(4, len / 4);
        let kw = 1.0 - (-1.0 / (0.002 * self.sr)).exp();
        for fr in buf.chunks_exact_mut(2) {
            let x = [fin(fr[0]), fin(fr[1])];
            self.buf[0][self.w] = x[0];
            self.buf[1][self.w] = x[1];
            let k = (self.beat / grid + 1e-9).floor() as i64;
            if k != self.step {
                self.step = k;
                self.event = self.decide(k);
                self.start = self.w;
                self.t = 0;
                self.tape = self.w as f64;
            }
            let mut y = x;
            let mut want = 0.0f32;
            if let Some(mode) = self.event {
                want = 1.0;
                let u = self.t % len;
                let rep = (self.t / len) as i32;
                let fade = (u as f32 / ramp as f32).min(1.0).min((len - u) as f32 / ramp as f32);
                match mode {
                    0 | 1 => {
                        let src = if mode == 0 { (self.start + u) % n } else { (self.start + n - 1 - u) % n };
                        let g = (1.0 - self.decay).powi(rep) * fade;
                        y = [self.buf[0][src] * g, self.buf[1][src] * g];
                    }
                    2 => {
                        let open = ((self.gate.clamp(0.05, 1.0) * len as f32) as usize).max(ramp * 2);
                        let open_g = if u < open { (u as f32 / ramp as f32).min(1.0).min((open - u) as f32 / ramp as f32) } else { 0.0 };
                        let g = open_g * (1.0 - self.decay).powi(rep);
                        y = [x[0] * g, x[1] * g];
                    }
                    _ => {
                        // tape stop over the grid step: the speed falls from 1 to 0, the read point lags behind the writing
                        let s = (1.0 - self.t as f64 / slice as f64).max(0.0);
                        let p = self.tape;
                        let tail = ((slice - self.t.min(slice)) as f32 / ramp as f32).min(1.0);
                        y = [self.read(0, p) * tail, self.read(1, p) * tail];
                        self.tape += s * s;
                    }
                }
                self.t += 1;
            }
            self.wet += (want - self.wet) * kw;
            let w = self.wet * self.mix;
            fr[0] = fin(x[0] * (1.0 - w) + y[0] * w);
            fr[1] = fin(x[1] * (1.0 - w) + y[1] * w);
            self.w = (self.w + 1) % n;
            self.beat += bps;
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "rate" => self.rate = choice(v, ST_RATES.len()),
            "grid" => self.grid = choice(v, ST_GRIDS.len()),
            "chance" => self.chance = v,
            "trigger" => self.trigger = v > 0.5,
            "mode" => self.mode = choice(v, ST_MODES.len()),
            "pattern" => self.pattern = choice(v, ST_PATTERNS.len()),
            "decay" => self.decay = v * 0.5,
            "gate" => self.gate = v,
            "seed" => self.seed = (v * 1000.0).round() as u32,
            "mix" => self.mix = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Stutter"
    }
    fn set_transport(&mut self, bpm: f64, beat: f64) {
        if bpm.is_finite() && bpm > 20.0 {
            self.bpm = bpm;
        }
        if !beat.is_finite() {
            return;
        }
        let moved = self.last_base.is_nan() || (beat - self.last_base).abs() > 1e-9;
        self.last_base = beat;
        if moved {
            self.beat = beat;
        }
    }
    fn view(&self) -> Vec<f32> {
        // 16 grid steps from the current one: 1 = fires, 0 = dry; then whether one is playing now
        let mut v: Vec<f32> = (0..16).map(|i| if self.decide(self.step.max(0) + i).is_some() { 1.0 } else { 0.0 }).collect();
        v.push(if self.event.is_some() { 1.0 } else { 0.0 });
        v
    }
}

// ── Limiter ─────────────────────────────────────────────────────────────────────

pub const LIMITER_PARAMS: [(&str, f32); 4] = [("gain", 0.0), ("ceiling", 5.0 / 6.0), ("release", 0.5), ("lookahead", 0.3)];

/// taps per phase of the 4x true-peak interpolator
const TP_TAPS: usize = 12;

/// The 4x oversampled peak finder of BS.1770 (a windowed-sinc interpolator): `push` takes one stereo sample and returns the highest
/// absolute value on the stretch between two samples a few samples back (both ends and three points between), max over the channels.
#[derive(Clone)]
pub struct TruePeak {
    h: [[f32; TP_TAPS]; 3],
    x: [[f32; TP_TAPS * 2]; 2],
    i: usize,
}
impl Default for TruePeak {
    fn default() -> Self {
        Self::new()
    }
}
impl TruePeak {
    /// samples between an input and the stretch its peak value covers
    pub const DELAY: usize = TP_TAPS / 2;
    pub fn new() -> TruePeak {
        let mut h = [[0f32; TP_TAPS]; 3];
        let half = TP_TAPS as f64 / 2.0;
        for (p, row) in h.iter_mut().enumerate() {
            let frac = (p + 1) as f64 / 4.0;
            let mut sum = 0.0;
            for (j, tap) in row.iter_mut().enumerate() {
                // tap j is the sample at offset j - (half - 1) from the left end of the stretch
                let t = frac - (j as f64 - (half - 1.0));
                let sinc = if t.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t) };
                // a Kaiser window (beta 4): it reads at most 0.03 dB under a sine's true peak up to 19 kHz at 48 kHz (and errs high
                // rather than low elsewhere), which a Blackman window of the same length does not (0.3 dB under)
                let u = t / (half + 0.5);
                let w = bessel_i0(4.0 * (1.0 - u * u).max(0.0).sqrt()) / bessel_i0(4.0);
                *tap = (sinc * w) as f32;
                sum += sinc * w;
            }
            for tap in row.iter_mut() {
                *tap = (*tap as f64 / sum) as f32;
            }
        }
        TruePeak { h, x: [[0.0; TP_TAPS * 2]; 2], i: 0 }
    }
    #[inline]
    pub fn push(&mut self, l: f32, r: f32) -> f32 {
        // a doubled ring: the newest TP_TAPS samples are always contiguous at [i, i + TP_TAPS)
        let i = self.i;
        self.x[0][i] = l;
        self.x[0][i + TP_TAPS] = l;
        self.x[1][i] = r;
        self.x[1][i + TP_TAPS] = r;
        self.i = (i + 1) % TP_TAPS;
        let mut pk = 0.0f32;
        for c in 0..2 {
            let w = &self.x[c][self.i..self.i + TP_TAPS];
            let a = w[TP_TAPS / 2 - 1].abs().max(w[TP_TAPS / 2].abs());
            pk = pk.max(a);
            for row in &self.h {
                let mut s = 0.0f32;
                for j in 0..TP_TAPS {
                    s += row[j] * w[j];
                }
                pk = pk.max(s.abs());
            }
        }
        pk
    }
}

/// the modified Bessel function I0 (for the Kaiser window)
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0f64, 1.0f64, 1.0f64);
    while term > 1e-14 * sum {
        term *= (x / (2.0 * k)) * (x / (2.0 * k));
        sum += term;
        k += 1.0;
    }
    sum
}

/// the true peak (dBTP) of interleaved stereo, by the same 4x interpolator
pub fn true_peak_db(stereo: &[f32]) -> f32 {
    let mut tp = TruePeak::new();
    let mut pk = 0.0f32;
    for f in stereo.chunks_exact(2) {
        pk = pk.max(tp.push(f[0], f[1]));
    }
    for _ in 0..TP_TAPS {
        pk = pk.max(tp.push(0.0, 0.0));
    }
    lin2db(pk)
}

/// Loudness meter (BS.1770): momentary (400 ms), short-term (3 s) and integrated (gated, from a histogram, so it never allocates).
#[derive(Clone)]
pub struct LoudMeter {
    k: [[crate::mixai::Bq; 2]; 2],
    acc: f64,
    n: usize,
    block: usize,
    /// the last 30 blocks of 100 ms (mean power each)
    ring: [f64; 30],
    ri: usize,
    filled: usize,
    /// histogram of 400 ms block loudness, 0.1 LU bins from -70 LUFS
    hist: Vec<u32>,
    hsum: Vec<f64>,
    pub momentary: f32,
    pub short: f32,
}
const H_BINS: usize = 800;
impl LoudMeter {
    pub fn new(sr: f32) -> LoudMeter {
        let k = crate::mixai::k_filters(sr);
        LoudMeter { k: [k, k], acc: 0.0, n: 0, block: (sr * 0.1) as usize, ring: [0.0; 30], ri: 0, filled: 0, hist: vec![0; H_BINS], hsum: vec![0.0; H_BINS], momentary: -70.0, short: -70.0 }
    }
    pub fn reset(&mut self) {
        self.acc = 0.0;
        self.n = 0;
        self.ring = [0.0; 30];
        self.filled = 0;
        self.hist.iter_mut().for_each(|h| *h = 0);
        self.hsum.iter_mut().for_each(|h| *h = 0.0);
        self.momentary = -70.0;
        self.short = -70.0;
    }
    #[inline]
    pub fn push(&mut self, l: f32, r: f32) {
        let a0 = self.k[0][0].run(l);
        let a = self.k[0][1].run(a0);
        let b0 = self.k[1][0].run(r);
        let b = self.k[1][1].run(b0);
        self.acc += (a * a + b * b) as f64;
        self.n += 1;
        if self.n >= self.block {
            self.ring[self.ri] = self.acc / self.n as f64;
            self.ri = (self.ri + 1) % 30;
            self.filled = (self.filled + 1).min(30);
            self.acc = 0.0;
            self.n = 0;
            let mean = |k: usize| -> f64 {
                let k = k.min(self.filled).max(1);
                (0..k).map(|j| self.ring[(self.ri + 30 - 1 - j) % 30]).sum::<f64>() / k as f64
            };
            let m = mean(4);
            self.momentary = (-0.691 + 10.0 * m.max(1e-12).log10()) as f32;
            self.short = (-0.691 + 10.0 * mean(30).max(1e-12).log10()) as f32;
            if self.filled >= 4 && self.momentary > -70.0 {
                let bin = (((self.momentary + 70.0) * 10.0) as usize).min(H_BINS - 1);
                self.hist[bin] += 1;
                self.hsum[bin] += m;
            }
            if !(self.momentary.is_finite()) {
                self.reset();
            }
        }
    }
    /// integrated loudness so far (absolute gate -70, relative gate -10 LU)
    pub fn integrated(&self) -> f32 {
        let (n, s): (u64, f64) = self.hist.iter().zip(&self.hsum).fold((0, 0.0), |a, (h, e)| (a.0 + *h as u64, a.1 + e));
        if n == 0 {
            return -70.0;
        }
        let gate = -0.691 + 10.0 * (s / n as f64).log10() - 10.0;
        let start = (((gate + 70.0) * 10.0).max(0.0) as usize).min(H_BINS);
        let (n2, s2): (u64, f64) = self.hist[start..].iter().zip(&self.hsum[start..]).fold((0, 0.0), |a, (h, e)| (a.0 + *h as u64, a.1 + e));
        if n2 == 0 {
            return -70.0;
        }
        (-0.691 + 10.0 * (s2 / n2 as f64).log10()) as f32
    }
}

/// The look-ahead true-peak limiter. The gain each sample needs (ceiling / its true peak) is held for the look-ahead time (a
/// running minimum), comes back up with the release, then is averaged over the look-ahead time; the audio is delayed by the
/// look-ahead, so the averaged gain is already down when the peak passes. A last sample clamp at the ceiling catches the rest.
pub struct Limiter {
    sr: f32,
    gain_db: f32,
    ceil_db: f32,
    release: f32,
    look: f32,
    w: usize,
    tp: TruePeak,
    tp_out: TruePeak,
    /// the delayed audio
    dl: [Vec<f32>; 2],
    di: usize,
    /// the running minimum: (sample index, gain) in increasing gain order
    mq: std::collections::VecDeque<(u64, f32)>,
    /// the box average of the released gain
    box_buf: Vec<f32>,
    bi: usize,
    box_sum: f64,
    rel: f32,
    /// the tp stretch values of the last two stretches (the gain of a sample covers the stretch on each side)
    ip_prev: f32,
    count: u64,
    pub gr_db: f32,
    gr_show: f32,
    peak_out: f32,
    pub meter: LoudMeter,
}

const LIM_MAX_LOOK: f32 = 0.006;

impl Limiter {
    pub fn new(sr: f32) -> Limiter {
        let max_w = (LIM_MAX_LOOK * sr) as usize + 2;
        let dn = max_w + TruePeak::DELAY + 8;
        let mut l = Limiter {
            sr,
            gain_db: 0.0,
            ceil_db: -1.0,
            release: 0.1,
            look: 0.00185,
            w: 1,
            tp: TruePeak::new(),
            tp_out: TruePeak::new(),
            dl: [vec![0.0; dn], vec![0.0; dn]],
            di: 0,
            mq: std::collections::VecDeque::with_capacity(max_w + 4),
            box_buf: vec![1.0; max_w + 2],
            bi: 0,
            box_sum: 1.0,
            rel: 1.0,
            ip_prev: 0.0,
            count: 0,
            gr_db: 0.0,
            gr_show: 0.0,
            peak_out: 0.0,
            meter: LoudMeter::new(sr),
        };
        l.set_look(l.look);
        l
    }
    fn set_look(&mut self, s: f32) {
        self.look = s.clamp(0.0005, LIM_MAX_LOOK);
        let w = ((self.look * self.sr) as usize).clamp(1, self.box_buf.len() - 2);
        if w != self.w {
            self.w = w;
            self.box_buf.iter_mut().for_each(|b| *b = 1.0);
            self.box_sum = w as f64;
            self.bi = 0;
            self.mq.clear();
        }
    }
    /// the latency in samples (the audio is this late)
    pub fn latency(&self) -> usize {
        self.w - 1 + TruePeak::DELAY
    }
    pub fn ceiling_db(&self) -> f32 {
        self.ceil_db
    }
}

impl Effect for Limiter {
    fn process(&mut self, buf: &mut [f32]) {
        let pre = db2lin(self.gain_db);
        // a small margin under the ceiling for what the 12-tap interpolator misses near Nyquist and the gain's own movement
        let ceil = db2lin(self.ceil_db - 0.1);
        let hard = db2lin(self.ceil_db);
        let krel = 1.0 - (-1.0 / (self.release * self.sr)).exp();
        let w = self.w;
        let dn = self.dl[0].len();
        let lat = self.latency();
        let mut gr_max = 0.0f32;
        for fr in buf.chunks_exact_mut(2) {
            let (l, r) = (fin(fr[0]) * pre, fin(fr[1]) * pre);
            // the delay line: written now, read `lat` samples later
            self.dl[0][self.di] = l;
            self.dl[1][self.di] = r;
            // the true peak of the stretch around the sample DELAY back
            let ip = self.tp.push(l, r);
            let pk = ip.max(self.ip_prev);
            self.ip_prev = ip;
            let need = if pk > ceil { ceil / pk } else { 1.0 };
            // running minimum over the last w samples
            let idx = self.count;
            while let Some(&(_, g)) = self.mq.back() {
                if g >= need {
                    self.mq.pop_back();
                } else {
                    break;
                }
            }
            self.mq.push_back((idx, need));
            while let Some(&(i0, _)) = self.mq.front() {
                if i0 + (w as u64) <= idx {
                    self.mq.pop_front();
                } else {
                    break;
                }
            }
            let held = self.mq.front().map(|x| x.1).unwrap_or(1.0);
            // release: down at once, up along the release
            self.rel = if held < self.rel { held } else { self.rel + (held - self.rel) * krel };
            // the box average over w samples
            self.box_sum += self.rel as f64 - self.box_buf[self.bi] as f64;
            self.box_buf[self.bi] = self.rel;
            self.bi = (self.bi + 1) % w;
            let g = (self.box_sum / w as f64).min(1.0) as f32;
            let ri = (self.di + dn - lat) % dn;
            let mut yl = self.dl[0][ri] * g;
            let mut yr = self.dl[1][ri] * g;
            yl = yl.clamp(-hard, hard);
            yr = yr.clamp(-hard, hard);
            self.di = (self.di + 1) % dn;
            self.count += 1;
            gr_max = gr_max.max(-lin2db(g));
            self.peak_out = self.peak_out.max(self.tp_out.push(yl, yr));
            self.meter.push(yl, yr);
            fr[0] = yl;
            fr[1] = yr;
        }
        // recompute the running sum now and then (no drift)
        if self.count % 65536 < (buf.len() / 2) as u64 {
            self.box_sum = self.box_buf[..w].iter().map(|x| *x as f64).sum();
        }
        self.gr_db = gr_max;
        self.gr_show = gr_max.max(self.gr_show * 0.9);
        self.peak_out *= 0.9995f32.powi((buf.len() / 2) as i32);
    }
    fn set_param(&mut self, n: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match n {
            "gain" => self.gain_db = v * 24.0,
            "ceiling" => self.ceil_db = v * 6.0 - 6.0,
            "release" => self.release = 0.01 * 100f32.powf(v),
            "lookahead" => self.set_look(0.0005 + v * 0.0045),
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Limiter"
    }
    fn view(&self) -> Vec<f32> {
        // gain reduction (0..1 = 0..24 dB), then momentary, short-term and integrated LUFS and the output's true peak, each as (x + 60) / 60
        let n = |x: f32| ((x + 60.0) / 60.0).clamp(0.0, 1.0);
        vec![(self.gr_show / 24.0).clamp(0.0, 1.0), n(self.meter.momentary), n(self.meter.short), n(self.meter.integrated()), n(lin2db(self.peak_out))]
    }
    fn command(&mut self, cmd: &str, _args: &[f64], _text: &str) -> Vec<f64> {
        match cmd {
            "reset" => {
                self.meter.reset();
                self.peak_out = 0.0;
                vec![1.0]
            }
            "meter" => vec![self.gr_show as f64, self.meter.momentary as f64, self.meter.short as f64, self.meter.integrated() as f64, lin2db(self.peak_out) as f64],
            _ => vec![],
        }
    }
}

// ── presets ─────────────────────────────────────────────────────────────────────

type Pre = (&'static str, &'static [(&'static str, f32)]);
pub const COLOUR_PRESETS: [Pre; 5] = [
    ("Chord growl (follows the song)", &[]),
    ("Glassy chords", &[("q", 0.75), ("span", 0.75), ("tilt", 0.62), ("glide", 0.3)]),
    ("Comb colour", &[("mode", 0.75), ("q", 0.6), ("tilt", 0.6)]),
    ("Own notes: power chord", &[("follow", 0.5)]),
    ("Gritty resonator", &[("drive", 0.5), ("q", 0.55), ("mix", 0.85)]),
];
pub const PEQ_PRESETS: [Pre; 6] = [
    ("Flat", &[]),
    ("Mono lows: side cut 120 Hz", &[("b1 type", 0.8125), ("b1 freq", 0.2585), ("b1 ch", 0.8333)]),
    ("Sub tidy: low cut 30 Hz", &[("b1 type", 0.8125), ("b1 freq", 0.0585)]),
    ("Mud cut 300 Hz", &[("b3 freq", 0.3907), ("b3 gain", 0.4167), ("b3 q", 0.5)]),
    ("Air shelf 10 kHz", &[("b6 gain", 0.5833)]),
    ("Bass: mono lows + tidy sub", &[("b1 type", 0.8125), ("b1 freq", 0.2585), ("b1 ch", 0.8333), ("b2 type", 0.5625), ("b2 freq", 0.0322)]),
];
pub const SHAPER_PRESETS: [Pre; 8] = [
    ("Kick pump 1/4", &[]),
    ("Hard pump 1/4", &[("shape", 1.5 / 12.0), ("depth", 0.95)]),
    ("Soft breathe 1/2", &[("shape", 2.5 / 12.0), ("rate", 3.5 / 15.0), ("depth", 0.6)]),
    ("Trance gate (1 bar)", &[("shape", 7.5 / 12.0), ("rate", 2.5 / 15.0), ("depth", 1.0)]),
    ("Offbeat chop (1 bar)", &[("shape", 8.5 / 12.0), ("rate", 2.5 / 15.0), ("depth", 1.0)]),
    ("Triplet gate 1/8T", &[("shape", 6.5 / 12.0), ("rate", 10.5 / 15.0), ("depth", 0.9)]),
    ("Dotted pump 1/8D", &[("rate", 13.5 / 15.0), ("depth", 0.7)]),
    ("Swell 4 bars", &[("shape", 4.5 / 12.0), ("rate", 0.5 / 15.0), ("depth", 1.0)]),
];
pub const HYPER_PRESETS: [Pre; 4] = [
    ("Hyper 5", &[]),
    ("Supersaw thickener", &[("voices", 1.0), ("detune", 0.6), ("mix", 0.55)]),
    ("Subtle width", &[("voices", 0.33), ("detune", 0.2), ("mix", 0.3)]),
    ("Seasick", &[("rate", 0.65), ("detune", 0.85), ("mix", 0.6)]),
];
pub const STUTTER_PRESETS: [Pre; 6] = [
    ("Random 1/16 repeats", &[]),
    ("Bar-end fill", &[("pattern", 0.3), ("chance", 1.0)]),
    ("Reverse tails", &[("mode", 0.3), ("rate", 1.5 / 11.0), ("chance", 0.25)]),
    ("Tape stop at phrase end", &[("mode", 0.7), ("pattern", 0.9), ("chance", 1.0), ("grid", 2.5 / 5.0)]),
    ("Gate chop 1/32", &[("mode", 0.5), ("rate", 4.5 / 11.0), ("chance", 0.5)]),
    ("Build: always on, 1/8", &[("trigger", 1.0), ("rate", 2.5 / 11.0), ("decay", 0.2)]),
];
pub const LIMITER_PRESETS: [Pre; 4] = [
    ("Transparent -1 dBTP", &[]),
    ("Loud club (+9 dB)", &[("gain", 0.375), ("release", 0.4)]),
    ("Streaming safe (-1 dBTP, slow)", &[("release", 0.65)]),
    ("Broadcast -2 dBTP", &[("ceiling", 4.0 / 6.0)]),
];

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
    fn mono(b: &[f32]) -> Vec<f32> {
        b.chunks(2).map(|c| 0.5 * (c[0] + c[1])).collect()
    }
    /// power at `f` Hz (Goertzel-style DFT bin over the whole signal)
    fn pow_at(x: &[f32], f: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, v) in x.iter().enumerate() {
            let a = std::f64::consts::TAU * f as f64 * i as f64 / SR as f64;
            re += *v as f64 * a.cos();
            im += *v as f64 * a.sin();
        }
        ((re * re + im * im) / (x.len() as f64 * x.len() as f64)) as f32
    }
    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
    }
    fn mk(name: &str) -> Box<dyn Effect> {
        super::super::fx::make(name, SR).unwrap()
    }

    #[test]
    fn every_new_effect_stays_finite_for_any_settings() {
        for name in ["Colour Bass", "Param EQ", "Volume Shaper", "Hyper Chorus", "Stutter", "Limiter", "Digitize", "Tape Stop", "Shimmer Verb", "Granular Cloud"] {
            for v in [0.0f32, 0.37, 0.5, 1.0] {
                let mut e = mk(name);
                for (p, _) in super::super::fx::defaults(name) {
                    e.set_param(p, v);
                }
                e.set_keys(&[45, 48, 52], 0.0, 4.0);
                e.set_transport(128.0, 3.7);
                let mut b = noise(9600, 7, 0.9);
                b[100] = f32::NAN;
                b[200] = 40.0;
                b[301] = f32::INFINITY;
                for chunk in b.chunks_mut(128) {
                    e.process(chunk);
                }
                assert!(b.iter().all(|x| x.is_finite() && x.abs() <= 8.0), "{name} at {v}");
                let _ = e.view();
            }
        }
    }

    #[test]
    fn colour_bass_pulls_the_energy_onto_the_chord() {
        // white noise through the bank tuned to A minor (A C E): the chord's notes stand far above the notes between them
        let mut e = ColourBass::new(SR);
        for (p, v) in COLOUR_PARAMS {
            e.set_param(p, v);
        }
        e.set_param("keep lo", 0.0);
        e.set_keys(&[57, 60, 64], 0.0, 4.0);
        let mut b = noise(96000, 3, 0.3);
        for c in b.chunks_mut(256) {
            e.process(c);
        }
        let y: Vec<f32> = mono(&b)[48000..].to_vec();
        assert_eq!(e.chord_text(), "C E A");
        let chord: f32 = [220.0f32, 261.63, 329.63, 440.0, 523.25, 659.26].iter().map(|f| pow_at(&y, *f)).sum::<f32>() / 6.0;
        let off: f32 = [233.08f32, 277.18, 311.13, 369.99, 415.3, 493.88, 554.37, 622.25].iter().map(|f| pow_at(&y, *f)).sum::<f32>() / 8.0;
        assert!(chord > 30.0 * off, "chord {chord} off {off}");
        // and the level matches the input's
        let r = rms(&y) / 0.3 * 3f32.sqrt();
        assert!(r > 0.3 && r < 3.0, "level ratio {r}");
        // a new chord (F major) moves the resonators
        e.set_keys(&[53, 57, 60], 4.0, 8.0);
        let mut b = noise(96000, 9, 0.3);
        for c in b.chunks_mut(256) {
            e.process(c);
        }
        let y: Vec<f32> = mono(&b)[48000..].to_vec();
        let band = |f: f32| -> f32 { (-3..=3).map(|k| pow_at(&y, f * (1.0 + k as f32 * 0.002))).sum::<f32>() };
        let (fin_, ein) = (band(174.61) + band(349.23) + band(698.46), band(164.81) + band(329.63) + band(659.26));
        assert!(fin_ > 10.0 * ein, "F is in ({fin_}), E is out ({ein})");
        // the comb mode rings on the chord tones' harmonics
        let mut c = ColourBass::new(SR);
        c.set_param("mode", 0.75);
        c.set_param("keep lo", 0.0);
        c.set_param("q", 0.8);
        c.set_keys(&[45, 49, 52], 0.0, 4.0);
        let mut b = noise(96000, 5, 0.3);
        for ch in b.chunks_mut(256) {
            c.process(ch);
        }
        let y: Vec<f32> = mono(&b)[48000..].to_vec();
        assert!(pow_at(&y, 220.0) > 5.0 * pow_at(&y, 233.08));
    }

    #[test]
    fn colour_bass_passes_the_input_until_it_knows_a_chord() {
        let mut e = ColourBass::new(SR);
        let x = noise(4800, 2, 0.4);
        let mut b = x.clone();
        e.process(&mut b);
        let d: f32 = b.iter().zip(&x).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(d < 1e-6, "{d}");
        assert_eq!(ColourBass::mask_of(&[57]), (1 << 9) | (1 << 4), "a lone A gets its fifth");
    }

    #[test]
    fn param_eq_bands_measure_as_set() {
        let tone = |f: f32, n: usize| -> Vec<f32> { (0..n).flat_map(|i| { let x = (std::f32::consts::TAU * f * i as f32 / SR).sin() * 0.25; [x, x] }).collect() };
        let gain = |e: &mut ParamEq, f: f32| -> f32 {
            let x = tone(f, 48000);
            let mut b = x.clone();
            e.process(&mut b);
            20.0 * (rms(&b[48000..]) / rms(&x[48000..])).log10()
        };
        // the defaults do nothing at all
        let mut e = ParamEq::new(SR);
        for (p, v) in PEQ_PARAMS {
            e.set_param(p, v);
        }
        let x = noise(4800, 4, 0.5);
        let mut b = x.clone();
        e.process(&mut b);
        assert_eq!(b, x);
        // a +6 dB bell at 1 kHz
        e.set_param("b4 freq", (1000f32 / 20.0).log2() / 10.0);
        e.set_param("b4 gain", 0.5 + 6.0 / 36.0);
        let g = gain(&mut e, 1000.0);
        assert!((g - 6.0).abs() < 0.3, "bell {g}");
        let g = gain(&mut clone_eq(&e), 8000.0);
        assert!(g.abs() < 0.5, "far from the bell {g}");
        // a 24 dB low cut at 200 Hz: -3 dB at the corner region, ~-24 dB an octave below, flat far above
        let mut e = ParamEq::new(SR);
        for (p, v) in PEQ_PARAMS {
            e.set_param(p, v);
        }
        e.set_param("b1 type", 6.5 / 8.0);
        e.set_param("b1 freq", (200f32 / 20.0).log2() / 10.0);
        let g100 = gain(&mut clone_eq(&e), 100.0);
        let g2k = gain(&mut clone_eq(&e), 2000.0);
        assert!(g100 < -20.0 && g100 > -28.0, "an octave below {g100}");
        assert!(g2k.abs() < 0.3, "{g2k}");
        let r = e.response_db(100.0);
        assert!((r - g100).abs() < 1.0, "the drawn curve matches: {r} vs {g100}");
    }

    fn clone_eq(e: &ParamEq) -> ParamEq {
        let mut c = ParamEq { sr: e.sr, band: e.band, out: e.out, dirty: true };
        for b in c.band.iter_mut() {
            for s in b.st.iter_mut() {
                s.z = [[0.0; 2]; 2];
            }
        }
        c
    }

    #[test]
    fn param_eq_mid_side_bands_touch_only_their_channel() {
        // a side-only low cut at 120 Hz: a 50 Hz side signal goes, a 50 Hz mid signal stays, the 3 kHz side stays
        let mut e = ParamEq::new(SR);
        for (p, v) in PEQ_PARAMS {
            e.set_param(p, v);
        }
        for (p, v) in PEQ_PRESETS[1].1 {
            e.set_param(p, *v);
        }
        let run = |e: &mut ParamEq, f: f32, side: bool| -> (f32, f32) {
            let mut b: Vec<f32> = (0..96000).flat_map(|i| { let x = (std::f32::consts::TAU * f * i as f32 / SR).sin() * 0.3; [x, if side { -x } else { x }] }).collect();
            e.process(&mut b);
            let t = &b[96000..];
            let m: Vec<f32> = t.chunks(2).map(|c| 0.5 * (c[0] + c[1])).collect();
            let s: Vec<f32> = t.chunks(2).map(|c| 0.5 * (c[0] - c[1])).collect();
            (rms(&m), rms(&s))
        };
        let (m, s) = run(&mut clone_eq(&e), 50.0, true);
        assert!(s < 0.3 / 2f32.sqrt() * 0.08 && m < 1e-4, "50 Hz side cut: mid {m} side {s}");
        let (m, s) = run(&mut clone_eq(&e), 50.0, false);
        assert!((m - 0.3 / 2f32.sqrt()).abs() < 0.005 && s < 1e-4, "50 Hz mid kept: mid {m} side {s}");
        let (m, s) = run(&mut clone_eq(&e), 3000.0, true);
        assert!((s - 0.3 / 2f32.sqrt()).abs() < 0.01 && m < 1e-4, "3 kHz side kept: mid {m} side {s}");
        // the mid-only bell lifts the mid and leaves the side
        let mut e = ParamEq::new(SR);
        e.set_param("b3 type", 1.5 / 8.0);
        e.set_param("b3 freq", (1000f32 / 20.0).log2() / 10.0);
        e.set_param("b3 gain", 0.5 + 6.0 / 36.0);
        e.set_param("b3 ch", 0.5);
        let mut b: Vec<f32> = (0..96000).flat_map(|i| { let x = (std::f32::consts::TAU * 1000.0 * i as f32 / SR).sin() * 0.2; [x, 0.0] }).collect();
        e.process(&mut b);
        let t = &b[96000..];
        let m: Vec<f32> = t.chunks(2).map(|c| 0.5 * (c[0] + c[1])).collect();
        let s: Vec<f32> = t.chunks(2).map(|c| 0.5 * (c[0] - c[1])).collect();
        let (mg, sg) = (20.0 * (rms(&m) / (0.1 / 2f32.sqrt())).log10(), 20.0 * (rms(&s) / (0.1 / 2f32.sqrt())).log10());
        assert!((mg - 6.0).abs() < 0.3 && sg.abs() < 0.2, "mid {mg} side {sg}");
    }

    #[test]
    fn the_shaper_follows_the_tempo() {
        // 1/4 pump at 120 bpm: the volume is lowest right on each beat (every 24000 samples) and back up before the next
        let mut e = VolumeShaper::new(SR);
        for (p, v) in SHAPER_PARAMS {
            e.set_param(p, v);
        }
        e.set_param("depth", 1.0);
        e.set_param("smooth", 0.0);
        e.set_transport(120.0, 0.0);
        let mut b = vec![1.0f32; 96000 * 2];
        e.process(&mut b);
        let g: Vec<f32> = b.chunks(2).map(|c| c[0]).collect();
        for beat in 1..4 {
            let at = beat * 24000;
            let w = &g[at - 2000..at + 2000];
            let (imin, _) = w.iter().enumerate().fold((0, 2.0f32), |a, (i, v)| if *v < a.1 { (i, *v) } else { a });
            let imin = at - 2000 + imin;
            assert!(imin.abs_diff(at) < 100, "beat {beat}: lowest at {imin}");
            assert!(g[imin] < 0.02 && g[at - 500] > 0.95, "dips {} and recovers {}", g[imin], g[at - 500]);
        }
        // 1/8T: three dips per beat
        let mut e = VolumeShaper::new(SR);
        e.set_param("rate", 10.5 / 15.0);
        e.set_param("depth", 1.0);
        e.set_param("smooth", 0.0);
        e.set_transport(120.0, 0.0);
        let mut b = vec![1.0f32; 24000 * 2];
        e.process(&mut b);
        let g: Vec<f32> = b.chunks(2).map(|c| c[0]).collect();
        let dips = g.windows(2).filter(|w| w[0] > 0.5 && w[1] <= 0.5).count();
        assert_eq!(dips, 3);
        // the curve follows the song position, not the block: two halves equal one whole
        let run = |split: usize| -> Vec<f32> {
            let mut e = VolumeShaper::new(SR);
            e.set_param("shape", 7.5 / 12.0);
            e.set_param("rate", 2.5 / 15.0);
            let mut b = vec![0.5f32; 48000 * 2];
            let bps = 120.0 / 60.0 / SR as f64;
            let (a, c) = b.split_at_mut(split * 2);
            e.set_transport(120.0, 1.0);
            e.process(a);
            e.set_transport(120.0, 1.0 + split as f64 * bps);
            e.process(c);
            b
        };
        let (x, y) = (run(10000), run(31111));
        assert!(x.iter().zip(&y).all(|(a, b)| (a - b).abs() < 1e-4));
    }

    #[test]
    fn stutter_repeats_on_the_grid() {
        // always on, repeat 1/16 at 120 bpm (6000 samples), grid 1 beat (24000): inside every beat the first 6000 samples are the
        // input of that beat's start, then they repeat exactly
        let mut e = Stutter::new(SR);
        for (p, v) in STUTTER_PARAMS {
            e.set_param(p, v);
        }
        e.set_param("trigger", 1.0);
        e.set_transport(120.0, 0.0);
        let x = noise(96000, 11, 0.5);
        let mut b = x.clone();
        for c in b.chunks_mut(128) {
            e.process(c);
        }
        let yl: Vec<f32> = b.chunks(2).map(|c| c[0]).collect();
        let xl: Vec<f32> = x.chunks(2).map(|c| c[0]).collect();
        for beat in 1..4usize {
            let s = beat * 24000;
            for rep in 0..4usize {
                for i in (200..5800).step_by(37) {
                    assert!((yl[s + rep * 6000 + i] - xl[s + i]).abs() < 1e-5, "beat {beat} rep {rep} i {i}");
                }
            }
        }
        // by chance: the same steps fire on every run, about `chance` of them
        let mut e = Stutter::new(SR);
        e.set_param("chance", 0.3);
        let fired = (0..1000).filter(|k| e.decide(*k).is_some()).count();
        assert!((200..400).contains(&fired), "{fired}");
        assert_eq!((0..50).map(|k| e.decide(k).is_some()).collect::<Vec<_>>(), (0..50).map(|k| e.decide(k).is_some()).collect::<Vec<_>>());
        // the bar-end pattern only fires on the last beat of a bar
        e.set_param("pattern", 0.3);
        e.set_param("chance", 1.0);
        assert!((0..32).all(|k| e.decide(k).is_some() == (k % 4 == 3)));
        // nothing fires: the sound passes untouched
        let mut e = Stutter::new(SR);
        e.set_param("chance", 0.0);
        e.set_transport(120.0, 0.0);
        let mut b = x.clone();
        e.process(&mut b);
        assert_eq!(b, x);
    }

    /// a better 4x interpolation than the limiter's own (64 taps per phase), for checking it
    fn tp_reference(stereo: &[f32]) -> f32 {
        let half = 32i64;
        let mut pk = 0.0f32;
        for c in 0..2 {
            let x: Vec<f32> = stereo.iter().skip(c).step_by(2).cloned().collect();
            for m in 0..x.len() as i64 {
                pk = pk.max(x[m as usize].abs());
                for p in 1..4 {
                    let f = p as f64 / 4.0;
                    let mut s = 0.0f64;
                    for j in -half + 1..=half {
                        let k = m + j;
                        if k < 0 || k >= x.len() as i64 {
                            continue;
                        }
                        let t = f - j as f64;
                        let sinc = (std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t);
                        let w = 0.42 + 0.5 * (std::f64::consts::PI * t / (half as f64 + 0.5)).cos() + 0.08 * (2.0 * std::f64::consts::PI * t / (half as f64 + 0.5)).cos();
                        s += x[k as usize] as f64 * sinc * w;
                    }
                    pk = pk.max(s.abs() as f32);
                }
            }
        }
        lin2db(pk)
    }

    #[test]
    fn the_limiter_keeps_true_peaks_under_the_ceiling() {
        // a loud mix-like signal (kick thumps, a bass, bright tones near 11 and 15 kHz that make inter-sample peaks, hats: noise
        // rolled off above ~7 kHz as cymbals are, not white up to Nyquist) pushed +12 dB
        let n = 48000;
        let mut x = noise(n, 21, 0.3);
        let (mut zl, mut zr) = (0.0f32, 0.0f32);
        for f in x.chunks_mut(2) {
            zl += (f[0] - zl) * 0.6;
            zr += (f[1] - zr) * 0.6;
            f[0] = zl;
            f[1] = zr;
        }
        for i in 0..n {
            let t = i as f32 / SR;
            let kick = if (i % 12000) < 3000 { (std::f32::consts::TAU * 55.0 * t).sin() * (1.0 - (i % 12000) as f32 / 3000.0) * 0.9 } else { 0.0 };
            let s = kick + 0.3 * (std::f32::consts::TAU * 110.0 * t).sin() + 0.25 * (std::f32::consts::TAU * 11025.3 * t).sin() + 0.2 * (std::f32::consts::TAU * 15013.0 * t + 0.4).sin();
            x[i * 2] += s;
            x[i * 2 + 1] += s * 0.9;
        }
        let mut e = Limiter::new(SR);
        for (p, v) in LIMITER_PARAMS {
            e.set_param(p, v);
        }
        e.set_param("gain", 0.5);
        let mut b = x.clone();
        for c in b.chunks_mut(512) {
            e.process(c);
        }
        let tp = tp_reference(&b);
        assert!(tp <= -1.0 + 0.05, "true peak {tp} dBTP");
        assert!(true_peak_db(&b) <= -1.0 + 0.01);
        // it is loud: far louder than the input simply turned down to the ceiling
        let xpk = x.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        let plain = rms(&x) * db2lin(-1.0) / xpk;
        assert!(rms(&b) > plain * 1.3, "{} vs {}", rms(&b), plain);
        // a quiet signal passes untouched (only delayed)
        let mut e = Limiter::new(SR);
        let q = noise(4800, 2, 0.05);
        let mut b = q.clone();
        e.process(&mut b);
        let lat = e.latency();
        for i in 0..4800 - lat - 1 {
            assert!((b[(i + lat) * 2] - q[i * 2]).abs() < 1e-6);
        }
        // the meter reads a full-scale 1 kHz sine at about -3 LUFS (stereo, 0 dBFS peak -> -3.01 + 0.69 K-weighting at 1 kHz)
        let mut m = LoudMeter::new(SR);
        for i in 0..SR as usize * 4 {
            let s = (std::f32::consts::TAU * 997.0 * i as f32 / SR).sin() * 0.5;
            m.push(s, s);
        }
        let want = crate::mixai::lufs(&(0..SR as usize * 4).flat_map(|i| { let s = (std::f32::consts::TAU * 997.0 * i as f32 / SR).sin() * 0.5; [s, s] }).collect::<Vec<f32>>(), SR);
        assert!((m.integrated() - want).abs() < 0.2 && (m.short - want).abs() < 0.2, "{} {} vs {}", m.integrated(), m.short, want);
    }

    #[test]
    fn hyper_chorus_widens_and_keeps_the_level() {
        let mut e = HyperChorus::new(SR);
        for (p, v) in HYPER_PARAMS {
            e.set_param(p, v);
        }
        let x: Vec<f32> = (0..48000).flat_map(|i| { let s = (std::f32::consts::TAU * 440.0 * i as f32 / SR).sin() * 0.3; [s, s] }).collect();
        let mut b = x.clone();
        e.process(&mut b);
        let t = &b[24000..];
        let side: Vec<f32> = t.chunks(2).map(|c| 0.5 * (c[0] - c[1])).collect();
        let mid: Vec<f32> = t.chunks(2).map(|c| 0.5 * (c[0] + c[1])).collect();
        assert!(rms(&side) > 0.1 * rms(&mid), "wide: side {} mid {}", rms(&side), rms(&mid));
        let r = rms(t) / rms(&x[24000..]);
        assert!(r > 0.6 && r < 1.6, "{r}");
    }
}
