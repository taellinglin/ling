//! Ling Nova's effect rack: twelve effects in an order of your choosing (each has a position), every one with its own dry / wet
//! mix. Most are the DAW's own effects (OTT, flanger, phaser, hyper chorus, chorus, compressor, EQ, imager); the distortion (nine
//! modes, a filter before or after it, 2x oversampled), the delay (ping-pong, tempo-synced, filtered), the filter and the reverb
//! (a plate or a hall on the FDN of `cinefx`) are built here.
use super::super::bassfx::{g_of, Flanger, Imager, Ott, Phaser, Tsvf};
use super::super::cinefx::Fdn;
use super::super::edmfx::HyperChorus;
use super::super::fx::{Chorus, Compressor, Effect, Eq3};
use super::layout::{sel, Layout, DIST_FILT, DIST_MODES, DIVS, DIV_BEATS, FX_FTYPES, NFX};
use super::Os2;

pub const CHUNK: usize = 256;

/// the distortion: drive into one of nine shapes, a filter before or after, 2x oversampled
pub struct Dist {
    sr: f32,
    os: [Os2; 2],
    pf: [Tsvf; 2],
    hold: [f32; 2],
    cnt: f32,
    dc: [f32; 2],
}
impl Dist {
    fn new(sr: f32) -> Dist {
        Dist { sr, os: [Os2::new(), Os2::new()], pf: [Tsvf::default(); 2], hold: [0.0; 2], cnt: 0.0, dc: [0.0; 2] }
    }
    /// one shape: `x` already driven
    #[inline]
    fn shape(mode: usize, x: f32, bias: f32) -> f32 {
        match mode {
            // tube: soft and a little asymmetric (the bias shifts it more)
            0 => (x + 0.15 + bias).tanh() - (0.15 + bias).tanh(),
            1 => x / (1.0 + x.abs()),
            2 => x.clamp(-1.0, 1.0),
            // diode: a hard knee on the positive side, softer below
            3 => {
                if x > 0.0 { 1.0 - (-x * 1.5).exp() } else { -(1.0 - (x * 0.8).exp()) * 0.8 }
            }
            4 => super::tables::fold(x + bias),
            5 => ((x + bias) * std::f32::consts::FRAC_PI_2).sin(),
            8 => {
                // asymmetric: the top clipped harder than the bottom, a touch of rectification
                let y = if x > 0.0 { (x * 1.5).tanh() } else { (x * 0.6).tanh() };
                y + bias * x.abs() * 0.5
            }
            _ => x,
        }
    }
    fn process(&mut self, buf: &mut [f32], ev: &dyn Fn(usize) -> f32, p: &[usize; 8]) {
        let mode = sel(ev(p[0]), DIST_MODES.len());
        let drive = ev(p[1]);
        let bias = (ev(p[2])) * 0.8;
        let fhz = 40.0 * 400f32.powf(ev(p[3]));
        let k = 2.0 - 1.9 * ev(p[4]);
        let outg = 10f32.powf((ev(p[5]) - 0.5) * 24.0 / 20.0);
        let filt = sel(ev(p[6]), DIST_FILT.len());
        let g = 10f32.powf(drive * 36.0 / 20.0);
        let gf = g_of(fhz, self.sr);
        let hold_n = 1.0 + drive * drive * 40.0;
        let bits = 16.0 - drive * 14.0;
        let q = 2f32.powf(bits - 1.0);
        // loudness: the clippers end near 1, the folds get louder, so they are kept down a little
        let comp = match mode {
            4 | 5 => 0.8,
            6 | 7 => 1.0,
            _ => 1.0 / (1.0 + drive * 0.3),
        };
        for fr in buf.chunks_exact_mut(2) {
            for c in 0..2 {
                let mut x = fr[c];
                if (1..=3).contains(&filt) {
                    let (lo, bp, hi) = self.pf[c].run(x, gf, k);
                    x = [lo, bp, hi][filt - 1];
                }
                let y = match mode {
                    6 => {
                        if c == 0 {
                            self.cnt += 1.0;
                        }
                        if self.cnt >= hold_n {
                            if c == 1 {
                                self.cnt -= hold_n;
                            }
                            self.hold[c] = x;
                        }
                        self.hold[c]
                    }
                    7 => (x * q).round() / q,
                    _ => {
                        // 2x: the input twice (linear), each shaped, then the half-band back down
                        let prev = self.os[c].last_in;
                        self.os[c].last_in = x;
                        let a = Dist::shape(mode, (0.5 * (prev + x)) * g, bias);
                        let b = Dist::shape(mode, x * g, bias);
                        self.os[c].down(a, b)
                    }
                };
                let mut y = y * comp;
                if (4..=6).contains(&filt) {
                    let (lo, bp, hi) = self.pf[c].run(y, gf, k);
                    y = [lo, bp, hi][filt - 4];
                }
                // no DC out of the asymmetric shapes
                self.dc[c] += (y - self.dc[c]) * 0.0005;
                fr[c] = (y - self.dc[c]) * outg;
            }
        }
    }
}

/// a stereo delay: free time or a note value, ping-pong, a high and a low cut in the feedback
pub struct Delay {
    sr: f32,
    buf: [Vec<f32>; 2],
    pos: usize,
    hp: [f32; 2],
    lp: [f32; 2],
    time_s: f32,
}
impl Delay {
    fn new(sr: f32) -> Delay {
        let n = (sr * 4.0) as usize + 8;
        Delay { sr, buf: [vec![0.0; n], vec![0.0; n]], pos: 0, hp: [0.0; 2], lp: [0.0; 2], time_s: 0.3 }
    }
    fn process(&mut self, buf: &mut [f32], ev: &dyn Fn(usize) -> f32, p: &[usize; 8], bpm: f32) {
        let n = self.buf[0].len();
        let want = if ev(p[1]) > 0.5 { DIV_BEATS[sel(ev(p[2]), DIVS.len())] * 60.0 / bpm.max(20.0) } else { 0.001 * 2000f32.powf(ev(p[0])) };
        let fb = ev(p[3]) * 0.95;
        let pp = ev(p[4]) > 0.5;
        let khp = 1.0 - (-std::f32::consts::TAU * (20.0 * 50f32.powf(ev(p[5]))) / self.sr).exp();
        let klp = 1.0 - (-std::f32::consts::TAU * (500.0 * 40f32.powf(ev(p[6]))).min(self.sr * 0.45) / self.sr).exp();
        for fr in buf.chunks_exact_mut(2) {
            // the time glides (no clicks when it is moved)
            self.time_s += (want.min(3.9) - self.time_s) * 0.0005;
            let d = (self.time_s * self.sr).clamp(1.0, (n - 2) as f32);
            let rp = self.pos as f32 + n as f32 - d;
            let i0 = rp.floor();
            let f = rp - i0;
            let i0 = i0 as usize % n;
            let i1 = (i0 + 1) % n;
            let mut w = [0.0f32; 2];
            for c in 0..2 {
                let x = self.buf[c][i0] * (1.0 - f) + self.buf[c][i1] * f;
                self.lp[c] += (x - self.lp[c]) * klp;
                self.hp[c] += (self.lp[c] - self.hp[c]) * khp;
                w[c] = self.lp[c] - self.hp[c];
            }
            let (inl, inr) = (fr[0], fr[1]);
            if pp {
                // the input enters on the left, the repeats cross over
                self.buf[0][self.pos] = (inl + inr) * 0.5 + w[1] * fb;
                self.buf[1][self.pos] = w[0] * fb;
            } else {
                self.buf[0][self.pos] = inl + w[0] * fb;
                self.buf[1][self.pos] = inr + w[1] * fb;
            }
            fr[0] = w[0];
            fr[1] = w[1];
            self.pos = (self.pos + 1) % n;
        }
    }
}

pub struct Reverb {
    sr: f32,
    fdn: Fdn,
    set: (f32, f32, f32, f32),
    hp: [f32; 2],
}
impl Reverb {
    fn process(&mut self, buf: &mut [f32], ev: &dyn Fn(usize) -> f32, p: &[usize; 8]) {
        let plate = ev(p[3]) > 0.5;
        let want = (ev(p[0]), ev(p[1]), ev(p[2]), if plate { 1.0 } else { 0.0 });
        if want != self.set {
            self.set = want;
            // a plate: small, dense and bright; a hall: bigger and darker
            let size = if plate { 0.05 + want.0 * 0.3 } else { 0.3 + want.0 * 0.7 };
            let rt = 0.3 + want.1 * want.1 * (if plate { 5.0 } else { 9.0 });
            let damp = if plate { want.2 * 0.5 } else { 0.15 + want.2 * 0.75 };
            self.fdn.set(size, rt, damp);
        }
        // the lows kept out of the tail (a bass stays clean)
        let khp = 1.0 - (-std::f32::consts::TAU * (20.0 * 30f32.powf(ev(p[4]))) / self.sr).exp();
        for fr in buf.chunks_exact_mut(2) {
            let mut x = [fr[0], fr[1]];
            for c in 0..2 {
                self.hp[c] += (x[c] - self.hp[c]) * khp;
                x[c] -= self.hp[c];
            }
            let (l, r) = self.fdn.run(x[0], x[1]);
            fr[0] = l * 1.4;
            fr[1] = r * 1.4;
        }
    }
}

pub struct Rack {
    sr: f32,
    pub dist: Dist,
    flt: [Tsvf; 2],
    flg: Flanger,
    phs: Phaser,
    hyp: HyperChorus,
    cho: Chorus,
    ott: Ott,
    cmp: Compressor,
    eq: Eq3,
    dly: Delay,
    rev: Reverb,
    img: Imager,
    dry: Vec<f32>,
    /// the value each boxed effect last got, per (effect, parameter)
    sent: [[f32; 8]; NFX],
    order: [usize; NFX],
}

impl Rack {
    pub fn new(sr: f32) -> Rack {
        let mut r = Rack {
            sr,
            dist: Dist::new(sr),
            flt: [Tsvf::default(); 2],
            flg: Flanger::new(sr),
            phs: Phaser::new(sr),
            hyp: HyperChorus::new(sr),
            cho: Chorus::new(sr),
            ott: Ott::new(sr),
            cmp: Compressor::new(sr),
            eq: Eq3::new(sr),
            dly: Delay::new(sr),
            rev: Reverb { sr, fdn: Fdn::new(sr), set: (-1.0, -1.0, -1.0, -1.0), hp: [0.0; 2] },
            img: Imager::new(sr),
            dry: vec![0.0; CHUNK * 2],
            sent: [[f32::NAN; 8]; NFX],
            order: [0; NFX],
        };
        // the effects whose mix the rack does are fully wet
        r.flg.set_param("mix", 1.0);
        r.phs.set_param("mix", 1.0);
        r.hyp.set_param("mix", 1.0);
        r.cho.set_param("mix", 1.0);
        r
    }

    pub fn reset(&mut self) {
        let sr = self.sr;
        *self = Rack::new(sr);
    }

    /// hand the boxed effects their parameters (only the ones that changed)
    fn send(&mut self, k: usize, ev: &dyn Fn(usize) -> f32, p: &[usize; 8]) {
        let names: &[&str] = match k {
            2 => &["rate", "depth", "feedback", "delay"],
            3 => &["rate", "depth", "feedback", "center", "stages"],
            4 => &["voices", "detune", "rate", "spread"],
            5 => &["rate", "depth"],
            6 => &["depth", "time", "upward", "downward", "stages", "out", "low", "high"],
            7 => &["threshold", "ratio", "attack", "release", "makeup"],
            8 => &["low", "mid", "high"],
            11 => &["width", "mono below"],
            _ => &[],
        };
        for (j, n) in names.iter().enumerate() {
            let v = ev(p[j]);
            if self.sent[k][j] != v {
                self.sent[k][j] = v;
                let e: &mut dyn Effect = match k {
                    2 => &mut self.flg,
                    3 => &mut self.phs,
                    4 => &mut self.hyp,
                    5 => &mut self.cho,
                    6 => &mut self.ott,
                    7 => &mut self.cmp,
                    8 => &mut self.eq,
                    _ => &mut self.img,
                };
                e.set_param(n, v);
            }
        }
    }

    /// run the rack over `buf` (interleaved stereo, at most CHUNK frames); `ev` is a parameter's value with its modulation
    pub fn process(&mut self, buf: &mut [f32], l: &Layout, ev: &dyn Fn(usize) -> f32, bpm: f32) {
        // the order: by position, ties by the default order
        for (i, o) in self.order.iter_mut().enumerate() {
            *o = i;
        }
        for i in 1..NFX {
            let mut j = i;
            while j > 0 && (ev(l.fx[self.order[j - 1]].pos), self.order[j - 1]) > (ev(l.fx[self.order[j]].pos), self.order[j]) {
                self.order.swap(j - 1, j);
                j -= 1;
            }
        }
        let n = buf.len();
        for oi in 0..NFX {
            let k = self.order[oi];
            let fx = l.fx[k];
            if ev(fx.on) < 0.5 {
                continue;
            }
            let mix = ev(fx.mix);
            if mix <= 0.001 {
                continue;
            }
            self.dry[..n].copy_from_slice(buf);
            self.send(k, ev, &fx.p);
            match k {
                0 => self.dist.process(buf, ev, &fx.p),
                1 => {
                    let ft = sel(ev(fx.p[3]), FX_FTYPES.len());
                    let fc = 20.0 * 1000f32.powf(ev(fx.p[0]));
                    let g = g_of(fc, self.sr);
                    let kq = 2.0 - 1.95 * ev(fx.p[1]).sqrt();
                    let dg = 1.0 + ev(fx.p[2]) * 8.0;
                    for fr in buf.chunks_exact_mut(2) {
                        for c in 0..2 {
                            let x = if dg > 1.01 { (fr[c] * dg).tanh() / dg.sqrt() } else { fr[c] };
                            let (lo, bp, hi) = self.flt[c].run(x, g, kq);
                            fr[c] = match ft {
                                0 => lo,
                                1 => bp * kq,
                                2 => hi,
                                _ => lo + hi,
                            };
                        }
                    }
                }
                2 => self.flg.process(buf),
                3 => self.phs.process(buf),
                4 => self.hyp.process(buf),
                5 => self.cho.process(buf),
                6 => self.ott.process(buf),
                7 => self.cmp.process(buf),
                8 => self.eq.process(buf),
                9 => self.dly.process(buf, ev, &fx.p, bpm),
                10 => self.rev.process(buf, ev, &fx.p),
                _ => self.img.process(buf),
            }
            if mix < 0.999 {
                for (y, d) in buf.iter_mut().zip(self.dry[..n].iter()) {
                    *y = d + (*y - d) * mix;
                }
            }
            for y in buf.iter_mut() {
                if !y.is_finite() {
                    *y = 0.0;
                }
            }
        }
    }
}
