//! A/B: the OTT and the Comb at their old settings (one stage, no key tracking) must sound exactly as before the stages and key
//! tracking were added. `old` is the code as it was (copied verbatim from dsp/bassfx.rs before the change).
#![allow(dead_code, clippy::all)]
use ling_daw::dsp::fx::{self, Effect};

mod old {
    use std::f32::consts::TAU;
    pub trait OldFx {
        fn process(&mut self, buf: &mut [f32]);
        fn set_param(&mut self, n: &str, v: f32);
        fn name(&self) -> &'static str;
    }
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
    impl OldFx for Ott {
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
    }


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
    impl OldFx for Comb {
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

}

use old::OldFx;

fn signal(n: usize, seed: u32) -> Vec<f32> {
    // a growl-like test signal: a detuned saw pair with a moving level, noise bursts and silences (the upward compressor's edge cases)
    let mut s = seed | 1;
    (0..n)
        .flat_map(|i| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let t = i as f32 / 48000.0;
            let saw = |f: f32| 2.0 * (t * f).fract() - 1.0;
            let env = if (i / 12000) % 4 == 3 { 0.0 } else { 0.2 + 0.6 * (t * 1.3).sin().abs() };
            let nz = (s as f32 / u32::MAX as f32 - 0.5) * 0.1;
            let l = (saw(55.0) * 0.6 + saw(55.4) * 0.4) * env + nz;
            let r = (saw(110.0) * 0.5 + saw(54.7) * 0.5) * env - nz;
            [l, r]
        })
        .collect()
}

fn settings(name: &str) -> Vec<Vec<(&'static str, f32)>> {
    let defs = fx::defaults(name);
    let mut out: Vec<Vec<(&'static str, f32)>> = vec![defs.to_vec()];
    for (_, pre) in fx::presets(name) {
        let mut v = defs.to_vec();
        for (n, x) in pre.iter() {
            if let Some(p) = v.iter_mut().find(|p| p.0 == *n) {
                p.1 = *x;
            }
        }
        out.push(v);
    }
    // random settings of the old parameters
    let mut s = 0x2545_f491u32;
    for _ in 0..6 {
        let v: Vec<(&'static str, f32)> = defs
            .iter()
            .map(|(n, d)| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                let new = matches!(*n, "stages" | "keytrack" | "track" | "kt oct");
                (*n, if new { *d } else { (s % 1001) as f32 / 1000.0 })
            })
            .collect();
        out.push(v);
    }
    out
}

fn ab(name: &str, mk_old: impl Fn() -> Box<dyn OldFx>) {
    let sig = signal(48000 * 3, 7);
    for (k, set) in settings(name).iter().enumerate() {
        let mut a = mk_old();
        let mut b = fx::make(name, 48000.0).unwrap();
        for (n, v) in set {
            a.set_param(n, *v);
            b.set_param(n, *v);
        }
        let (mut x, mut y) = (sig.clone(), sig.clone());
        // uneven block sizes, as the engine's slices and the audio callback give them
        let mut off = 0;
        let mut blk = 64;
        while off < x.len() {
            let e = (off + blk * 2).min(x.len());
            a.process(&mut x[off..e]);
            b.process(&mut y[off..e]);
            off = e;
            blk = if blk == 64 { 517 } else { 64 };
        }
        let same = x.iter().zip(&y).all(|(p, q)| p.to_bits() == q.to_bits());
        assert!(same, "{name}: setting {k} differs from the old code");
    }
}

#[test]
fn ott_with_one_stage_is_bit_identical_to_before() {
    ab("OTT", || {
        let mut o = old::Ott::new(48000.0);
        for (n, v) in old::OTT_PARAMS {
            o.set_param(n, v);
        }
        Box::new(o)
    });
}

#[test]
fn comb_without_key_tracking_is_bit_identical_to_before() {
    ab("Comb", || {
        let mut c = old::Comb::new(48000.0);
        for (n, v) in old::COMB_PARAMS {
            c.set_param(n, v);
        }
        Box::new(c)
    });
}

#[test]
fn the_old_effects_still_make_with_their_defaults() {
    // the registry grew: every name makes an effect with as many parameters as its defaults list
    for n in fx::EFFECT_NAMES {
        let e = fx::make(n, 48000.0);
        assert!(e.is_some(), "{n}");
        assert!(!fx::defaults(n).is_empty(), "{n}");
        let _: Box<dyn Effect> = e.unwrap();
    }
    assert_eq!(fx::defaults("OTT").len(), 10);
    assert_eq!(fx::defaults("Comb").len(), 11);
    assert_eq!(&fx::defaults("OTT")[..9].iter().map(|p| p.0).collect::<Vec<_>>(), &["depth", "time", "upward", "downward", "low", "mid", "high", "in", "out"], "old parameter order (automation indices) kept");
}
