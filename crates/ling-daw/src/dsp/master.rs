//! **Ling Master** - a multiband mastering maximizer for the end of the master chain:
//!
//! input gain -> 3 or 4 bands (Linkwitz-Riley crossovers, phase-compensated so the bands sum flat) each with downward and upward
//! compression, gain, stereo width and solo -> mono below a frequency -> saturation -> soft clipper -> the look-ahead true-peak
//! brickwall limiter (the same one as the Limiter effect: `push` drives it, `ceiling` is never passed) -> output trim (never above
//! 0 dB, so the ceiling holds) -> loudness meter (momentary, short-term, integrated LUFS) and true-peak meter.
//!
//! With every band neutral the crossovers are bypassed (crossfaded in when a band is moved), so the defaults are exactly the Limiter
//! with an input gain: AI Mix & Master can model it (loudness.rs) and use it in place of a plain Limiter. Latency: the limiter's
//! look-ahead plus 6 samples (about 2 ms at the default), reported by `info("latency")`.
use super::bassfx::{db2lin, fin, g_of, lin2db, Lr4, Tsvf};
use super::edmfx::{Limiter, LoudMeter, TruePeak};
use super::fx::Effect;

pub const NB: usize = 4;
const NGLOB: usize = 15;
const NPER: usize = 6;

const fn ch(k: usize, n: usize) -> f32 {
    (k as f32 + 0.5) / n as f32
}

pub const MASTER_PARAMS: [(&str, f32); NGLOB + NB * NPER] = [
    ("input", 0.5),
    ("bands", ch(1, 2)),
    ("x1", 0.477),
    ("x2", 0.523),
    ("x3", 0.511),
    ("attack", 0.35),
    ("release", 0.45),
    ("mono below", 0.0),
    ("sat", 0.0),
    ("clip", 0.0),
    ("push", 0.0),
    ("ceiling", 5.0 / 6.0),
    ("lim rel", 0.5),
    ("look", 0.3),
    ("output", 1.0),
    ("b1 thresh", 0.55),
    ("b1 ratio", 0.0),
    ("b1 up", 0.0),
    ("b1 gain", 0.5),
    ("b1 width", 0.5),
    ("b1 solo", 0.0),
    ("b2 thresh", 0.55),
    ("b2 ratio", 0.0),
    ("b2 up", 0.0),
    ("b2 gain", 0.5),
    ("b2 width", 0.5),
    ("b2 solo", 0.0),
    ("b3 thresh", 0.55),
    ("b3 ratio", 0.0),
    ("b3 up", 0.0),
    ("b3 gain", 0.5),
    ("b3 width", 0.5),
    ("b3 solo", 0.0),
    ("b4 thresh", 0.55),
    ("b4 ratio", 0.0),
    ("b4 up", 0.0),
    ("b4 gain", 0.5),
    ("b4 width", 0.5),
    ("b4 solo", 0.0),
];

type Pre = (&'static str, &'static [(&'static str, f32)]);
const fn db(d: f32) -> f32 {
    d / 24.0
}
const fn bg(d: f32) -> f32 {
    0.5 + d / 24.0
}
/// mono below `hz` (20..300 Hz)
const fn mono(hz: f32) -> f32 {
    (hz - 20.0) / 280.0
}

pub const MASTER_PRESETS: [Pre; 12] = [
    ("Clean Ceiling -1 dBTP", &[]),
    (
        "Streaming Polish (-14 LUFS)",
        &[("push", db(4.0)), ("b1 ratio", 0.12), ("b2 ratio", 0.1), ("b3 ratio", 0.1), ("b4 ratio", 0.12), ("b1 thresh", 0.5), ("b2 thresh", 0.5), ("b3 thresh", 0.5), ("b4 thresh", 0.45), ("lim rel", 0.62)],
    ),
    (
        "Club Loud",
        &[("push", db(9.0)), ("clip", 0.45), ("sat", 0.15), ("b1 ratio", 0.25), ("b2 ratio", 0.2), ("b3 ratio", 0.2), ("b4 ratio", 0.2), ("b4 up", 0.2), ("mono below", mono(120.0)), ("lim rel", 0.38)],
    ),
    ("Gentle Glue", &[("push", db(2.0)), ("b1 ratio", 0.08), ("b2 ratio", 0.08), ("b3 ratio", 0.08), ("b4 ratio", 0.08), ("attack", 0.55), ("release", 0.6)]),
    (
        "Lo-Fi Warmth",
        &[("bands", ch(0, 2)), ("sat", 0.55), ("push", db(4.0)), ("b1 gain", bg(1.5)), ("b3 gain", bg(-4.0)), ("b3 width", 0.4), ("b2 ratio", 0.2), ("mono below", mono(150.0)), ("lim rel", 0.7)],
    ),
    (
        "Bass Music Slam",
        &[("push", db(12.0)), ("clip", 0.7), ("sat", 0.2), ("b1 ratio", 0.4), ("b1 thresh", 0.5), ("b2 ratio", 0.3), ("b3 ratio", 0.25), ("b4 ratio", 0.2), ("mono below", mono(150.0)), ("lim rel", 0.32)],
    ),
    ("Wide Air", &[("push", db(3.0)), ("b3 width", 0.62), ("b4 width", 0.7), ("b4 up", 0.3), ("b4 gain", bg(1.0)), ("mono below", mono(100.0))]),
    ("Upward Detail", &[("push", db(3.0)), ("b1 up", 0.35), ("b2 up", 0.45), ("b3 up", 0.45), ("b4 up", 0.4), ("b1 ratio", 0.15), ("b2 ratio", 0.15), ("b3 ratio", 0.15), ("b4 ratio", 0.15)]),
    ("Broadcast Safe -2 dBTP", &[("ceiling", 4.0 / 6.0), ("push", db(3.0)), ("b1 ratio", 0.2), ("b2 ratio", 0.2), ("b3 ratio", 0.2), ("b4 ratio", 0.2), ("lim rel", 0.65)]),
    ("Vocal Forward", &[("push", db(4.0)), ("b2 gain", bg(1.5)), ("b3 gain", bg(2.0)), ("b1 ratio", 0.3), ("b3 ratio", 0.15)]),
    (
        "Spoken Word",
        &[("bands", ch(0, 2)), ("push", db(6.0)), ("mono below", mono(200.0)), ("b1 ratio", 0.35), ("b2 ratio", 0.35), ("b3 ratio", 0.3), ("b2 up", 0.3), ("b3 up", 0.25), ("b1 gain", bg(-2.0))],
    ),
    ("Mono-Safe Club", &[("push", db(7.0)), ("mono below", mono(140.0)), ("b1 width", 0.0), ("b2 width", 0.4), ("b1 ratio", 0.25), ("clip", 0.3), ("lim rel", 0.4)]),
];

/// the crossover frequencies of x1, x2, x3 (0..1)
pub fn x_hz(i: usize, v: f32) -> f32 {
    match i {
        0 => 40.0 * 10f32.powf(v),
        1 => 300.0 * 10f32.powf(v),
        _ => 2000.0 * 6f32.powf(v),
    }
}

/// the 2nd-order all-pass a Linkwitz-Riley 4 split sums to (k = sqrt 2): the bands that do not pass a crossover get it, so all sum flat
#[derive(Clone, Copy, Default)]
struct Ap {
    s: Tsvf,
}
impl Ap {
    #[inline]
    fn run(&mut self, x: f32, g: f32) -> f32 {
        const K: f32 = std::f32::consts::SQRT_2;
        let (_, bp, _) = self.s.run(x, g, K);
        x - 2.0 * K * bp
    }
}

#[derive(Clone, Copy, Default)]
struct Band {
    env: f32,
    /// the gain change in dB this block (most negative / most positive), for the meter
    gmin: f32,
    gmax: f32,
    lvl: f32,
}

pub struct Master {
    sr: f32,
    p: [f32; NGLOB + NB * NPER],
    xo: [[Lr4; 3]; 2],
    ap: [[Ap; 3]; 2],
    bands: [Band; NB],
    /// the multiband path blended in (0 = straight through, bit-exact)
    mbw: f32,
    mono_xo: [Lr4; 2],
    monow: f32,
    lim: Limiter,
    analysis: bool,
    meter: LoudMeter,
    tp: TruePeak,
    peak_out: f32,
    peak_in: f32,
    gr_show: f32,
    gb_show: [f32; NB],
}

impl Master {
    pub fn new(sr: f32) -> Master {
        let mut m = Master {
            sr,
            p: [0.0; NGLOB + NB * NPER],
            xo: [[Lr4::default(); 3]; 2],
            ap: [[Ap::default(); 3]; 2],
            bands: [Band { lvl: -100.0, ..Band::default() }; NB],
            mbw: 0.0,
            mono_xo: [Lr4::default(); 2],
            monow: 0.0,
            lim: Limiter::new(sr),
            analysis: false,
            meter: LoudMeter::new(sr),
            tp: TruePeak::new(),
            peak_out: 0.0,
            peak_in: 0.0,
            gr_show: 0.0,
            gb_show: [0.0; NB],
        };
        for (n, v) in MASTER_PARAMS {
            m.set_param(n, v);
        }
        m
    }
    #[inline]
    fn bp(&self, b: usize, k: usize) -> f32 {
        self.p[NGLOB + b * NPER + k]
    }
    fn nbands(&self) -> usize {
        if self.p[1] < 0.5 {
            3
        } else {
            4
        }
    }
    fn band_neutral(&self, b: usize) -> bool {
        self.bp(b, 1) < 0.001 && self.bp(b, 2) < 0.001 && (self.bp(b, 3) - 0.5).abs() < 0.001 && (self.bp(b, 4) - 0.5).abs() < 0.001 && self.bp(b, 5) < 0.5
    }
    fn mb_needed(&self) -> bool {
        (0..self.nbands()).any(|b| !self.band_neutral(b))
    }
    /// the latency in samples (the limiter's look-ahead)
    pub fn latency(&self) -> usize {
        self.lim.latency()
    }
    /// the limiter's settings in the Limiter effect's own parameter names (for the loudness model of AI Mix & Master)
    pub fn limiter_params(params: &[(&'static str, f32)]) -> Vec<(&'static str, f32)> {
        let get = |n: &str, d: f32| params.iter().find(|p| p.0 == n).map(|p| p.1).unwrap_or(d);
        vec![("gain", get("push", 0.0)), ("ceiling", get("ceiling", 5.0 / 6.0)), ("release", get("lim rel", 0.5)), ("lookahead", get("look", 0.3))]
    }
    /// the output trim (linear) of a parameter list
    pub fn out_gain(params: &[(&'static str, f32)]) -> f32 {
        db2lin(params.iter().find(|p| p.0 == "output").map(|p| p.1).unwrap_or(1.0) * 12.0 - 12.0)
    }
    pub fn integrated(&self) -> f32 {
        self.meter.integrated()
    }
}

impl Effect for Master {
    fn process(&mut self, buf: &mut [f32]) {
        let sr = self.sr;
        let frames = buf.len() / 2;
        let gin = db2lin(self.p[0] * 24.0 - 12.0);
        let nb = self.nbands();
        let gx: [f32; 3] = std::array::from_fn(|i| g_of(x_hz(i, self.p[2 + i]), sr));
        let att_s = 0.0005 * 100f32.powf(self.p[5]);
        let rel_s = 0.02 * 40f32.powf(self.p[6]);
        // the lows react slower than the highs
        const TSCALE: [f32; NB] = [2.0, 1.0, 0.7, 0.5];
        let mut ka = [0f32; NB];
        let mut kr = [0f32; NB];
        let mut thr = [0f32; NB];
        let mut ratio = [0f32; NB];
        let mut up = [0f32; NB];
        let mut gain = [0f32; NB];
        let mut width = [0f32; NB];
        let any_solo = (0..nb).any(|b| self.bp(b, 5) >= 0.5);
        let mut on = [true; NB];
        for b in 0..NB {
            ka[b] = 1.0 - (-1.0 / (att_s * TSCALE[b] * sr)).exp();
            kr[b] = 1.0 - (-1.0 / (rel_s * TSCALE[b] * sr)).exp();
            thr[b] = self.bp(b, 0) * 48.0 - 48.0;
            ratio[b] = 1.0 + self.bp(b, 1) * 9.0;
            up[b] = self.bp(b, 2);
            gain[b] = self.bp(b, 3) * 24.0 - 12.0;
            width[b] = self.bp(b, 4) * 2.0;
            on[b] = b < nb && (!any_solo || self.bp(b, 5) >= 0.5);
            self.bands[b].gmin = 0.0;
            self.bands[b].gmax = 0.0;
        }
        let mb_t = if self.mb_needed() { 1.0 } else { 0.0 };
        let kmb = 1.0 / (0.02 * sr);
        let mono_on = self.p[7] >= 0.01;
        let gmono = g_of(20.0 + self.p[7] * 280.0, sr);
        let kmono = 1.0 / (0.02 * sr);
        let sat = self.p[8];
        let ksat = 1.0 + sat * 3.0;
        let clip_g = db2lin(self.p[9] * 9.0);
        let gout = db2lin(self.p[14] * 12.0 - 12.0);
        let mut pin = 0.0f32;
        for fr in buf.chunks_exact_mut(2) {
            let mut x = [fin(fr[0]) * gin, fin(fr[1]) * gin];
            pin = pin.max(x[0].abs().max(x[1].abs()));
            // 1. the bands
            if mb_t > 0.0 || self.mbw > 0.0 {
                self.mbw = if mb_t > self.mbw { (self.mbw + kmb).min(1.0) } else { (self.mbw - kmb).max(0.0) };
                let mut bs = [[0f32; NB]; 2];
                for c in 0..2 {
                    let (lo, r1) = self.xo[c][0].split(x[c], gx[0]);
                    let (b2, r2) = self.xo[c][1].split(r1, gx[1]);
                    let (b3, b4) = if nb == 4 { self.xo[c][2].split(r2, gx[2]) } else { (r2, 0.0) };
                    // phase compensation: each band gets the all-passes of the crossovers it did not go through
                    let lo = self.ap[c][0].run(lo, gx[1]);
                    let (lo, b2) = if nb == 4 { (self.ap[c][1].run(lo, gx[2]), self.ap[c][2].run(b2, gx[2])) } else { (lo, b2) };
                    bs[c] = [lo, b2, b3, b4];
                }
                let mut y = [0f32; 2];
                for b in 0..nb {
                    let (l, r) = (bs[0][b], bs[1][b]);
                    // the band's level (stereo-linked peak follower) and its gain: down above the threshold, up below it
                    let m = l.abs().max(r.abs());
                    let bd = &mut self.bands[b];
                    bd.env += (m - bd.env) * if m > bd.env { ka[b] } else { kr[b] };
                    let lv = lin2db(bd.env);
                    bd.lvl = bd.lvl.max(lv);
                    let mut gdb = 0.0;
                    let over = lv - thr[b];
                    if ratio[b] > 1.0001 {
                        // a 6 dB soft knee
                        if over > 3.0 {
                            gdb -= over * (1.0 - 1.0 / ratio[b]);
                        } else if over > -3.0 {
                            let o = over + 3.0;
                            gdb -= (1.0 - 1.0 / ratio[b]) * o * o / 12.0;
                        }
                    }
                    if up[b] > 0.0 && over < 0.0 {
                        // upward: the quiet detail comes up (at most 12 dB, nothing under -60 dBFS: no noise floor pumped up)
                        let floor = ((lv + 66.0) / 6.0).clamp(0.0, 1.0);
                        gdb += ((-over) * up[b] * 0.75).min(12.0) * floor;
                    }
                    bd.gmin = bd.gmin.min(gdb);
                    bd.gmax = bd.gmax.max(gdb);
                    if !on[b] {
                        continue;
                    }
                    let k = db2lin(gdb + gain[b]);
                    let (mut l, mut r) = (l * k, r * k);
                    if (width[b] - 1.0).abs() > 1e-4 {
                        let (mm, ss) = ((l + r) * 0.5, (l - r) * 0.5 * width[b]);
                        l = mm + ss;
                        r = mm - ss;
                    }
                    y[0] += l;
                    y[1] += r;
                }
                let w = self.mbw;
                x = [x[0] * (1.0 - w) + y[0] * w, x[1] * (1.0 - w) + y[1] * w];
            }
            // 2. mono below: the side's lows go, the mid gets the same all-pass so both stay in phase
            if mono_on || self.monow > 0.0 {
                self.monow = if mono_on { (self.monow + kmono).min(1.0) } else { (self.monow - kmono).max(0.0) };
                let (m, s) = ((x[0] + x[1]) * 0.5, (x[0] - x[1]) * 0.5);
                let (ml, mh) = self.mono_xo[0].split(m, gmono);
                let (_, sh) = self.mono_xo[1].split(s, gmono);
                let w = self.monow;
                let m2 = m * (1.0 - w) + (ml + mh) * w;
                let s2 = s * (1.0 - w) + sh * w;
                x = [m2 + s2, m2 - s2];
            }
            // 3. saturation (unity gain for small signals) and the soft clipper (full scale)
            if sat > 0.0 {
                for v in x.iter_mut() {
                    *v = (*v * ksat).tanh() / ksat * (1.0 + sat * 0.25);
                }
            }
            if self.p[9] > 0.0 {
                for v in x.iter_mut() {
                    let a = (*v * clip_g).abs();
                    let y = if a < 0.7 { a } else { 0.7 + 0.3 * ((a - 0.7) / 0.3).tanh() };
                    *v = y.copysign(*v);
                }
            }
            fr[0] = x[0];
            fr[1] = x[1];
        }
        self.peak_in = pin.max(self.peak_in * 0.999f32.powi(frames as i32));
        // 4. the limiter, the output trim, the meters
        if !self.analysis {
            self.lim.process(buf);
            for fr in buf.chunks_exact_mut(2) {
                fr[0] *= gout;
                fr[1] *= gout;
            }
        }
        let mut pk = 0.0f32;
        for fr in buf.chunks_exact(2) {
            self.meter.push(fr[0], fr[1]);
            pk = pk.max(self.tp.push(fr[0], fr[1]));
        }
        self.peak_out = pk.max(self.peak_out * 0.9995f32.powi(frames as i32));
        let fall = 0.5f32.powf(frames as f32 / (0.15 * sr));
        self.gr_show = self.lim.gr_db.max(self.gr_show * fall);
        for b in 0..NB {
            let g = if -self.bands[b].gmin > self.bands[b].gmax { self.bands[b].gmin } else { self.bands[b].gmax };
            self.gb_show[b] = if g.abs() > self.gb_show[b].abs() { g } else { self.gb_show[b] * fall };
            // the level display falls 20 dB a second
            self.bands[b].lvl = (self.bands[b].lvl - 20.0 * frames as f32 / sr).max(-100.0);
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        if !v.is_finite() {
            return;
        }
        let v = v.clamp(0.0, 1.0);
        let Some(i) = MASTER_PARAMS.iter().position(|p| p.0 == n) else { return };
        self.p[i] = v;
        match n {
            "push" => self.lim.set_param("gain", v),
            "ceiling" => self.lim.set_param("ceiling", v),
            "lim rel" => self.lim.set_param("release", v),
            "look" => self.lim.set_param("lookahead", v),
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Ling Master"
    }
    fn view(&self) -> Vec<f32> {
        // per band the gain change ((dB + 24) / 48: 0.5 = none) and the level ((dB + 60) / 60); the limiter's reduction (0..24 dB);
        // momentary, short-term and integrated LUFS and the true peak out ((x + 60) / 60); the crossovers (log 20 Hz - 20 kHz);
        // bands in use, latency (ms), the input peak ((dB + 60) / 60), the ceiling (dBTP + 60) / 60, band solo flags
        let n = |x: f32| ((x + 60.0) / 60.0).clamp(0.0, 1.0);
        let mut v = Vec::with_capacity(32);
        for b in 0..NB {
            v.push(((self.gb_show[b] + 24.0) / 48.0).clamp(0.0, 1.0));
        }
        for b in 0..NB {
            v.push(n(self.bands[b].lvl));
        }
        v.push((self.gr_show / 24.0).clamp(0.0, 1.0));
        v.extend([n(self.meter.momentary), n(self.meter.short), n(self.meter.integrated()), n(lin2db(self.peak_out))]);
        for i in 0..3 {
            v.push(((x_hz(i, self.p[2 + i]) / 20.0).log10() / 3.0).clamp(0.0, 1.0));
        }
        v.push(self.nbands() as f32);
        v.push(self.latency() as f32 * 1000.0 / self.sr);
        v.push(n(lin2db(self.peak_in)));
        v.push(n(self.lim.ceiling_db()));
        for b in 0..NB {
            v.push((self.bp(b, 5) >= 0.5) as u8 as f32);
        }
        v
    }
    fn command(&mut self, cmd: &str, args: &[f64], _text: &str) -> Vec<f64> {
        match cmd {
            "reset" => {
                self.meter.reset();
                self.peak_out = 0.0;
                vec![1.0]
            }
            // [gain reduction dB, momentary, short-term, integrated LUFS, true peak dBTP]
            "meter" => vec![self.gr_show as f64, self.meter.momentary as f64, self.meter.short as f64, self.meter.integrated() as f64, lin2db(self.peak_out) as f64],
            // (1/0): the limiter and the output trim off, for measuring what goes into the limiter (AI Mix & Master's loudness model)
            "prelimit" => {
                self.analysis = args.first().copied().unwrap_or(1.0) > 0.5;
                vec![1.0]
            }
            _ => vec![],
        }
    }
    fn info(&self, what: &str) -> String {
        match what {
            "latency" => format!("{}", self.latency()),
            _ => String::new(),
        }
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
    fn music(n: usize) -> Vec<f32> {
        // kick, bass, chord and hats at 128 bpm, loud
        let spb = (SR * 60.0 / 128.0) as usize;
        let nz = noise(n, 3, 1.0);
        (0..n)
            .flat_map(|i| {
                let t = i as f32 / SR;
                let kt = (i % spb) as f32 / SR;
                let kick = (std::f32::consts::TAU * (48.0 + 110.0 * (-kt * 28.0).exp()) * kt).sin() * (-kt * 8.0).exp() * 0.9;
                let bass = (std::f32::consts::TAU * 49.0 * t).sin() * 0.35;
                let stab = [196.0f32, 233.1, 293.7].iter().map(|f| (std::f32::consts::TAU * f * t).sin()).sum::<f32>() * 0.1;
                let hat = nz[i * 2] * (-(((i + spb / 2) % spb) as f32 / SR) * 50.0).exp() * 0.3;
                [kick + bass + stab + hat, kick + bass + stab - hat * 0.5]
            })
            .collect()
    }
    /// the true peak of a signal from frame `from` on, without the edges (a signal starting or stopping mid-wave overshoots there)
    fn tp_inner(b: &[f32], from: usize) -> f32 {
        let mut tp = TruePeak::new();
        let pk = b.chunks_exact(2).enumerate().map(|(i, f)| if i >= from { tp.push(f[0], f[1]) } else { tp.push(f[0], f[1]) * 0.0 }).fold(0.0f32, f32::max);
        lin2db(pk)
    }
    fn run(e: &mut dyn Effect, b: &mut [f32]) {
        for c in b.chunks_mut(512) {
            e.process(c);
        }
    }
    fn set(e: &mut Master, set: &[(&str, f32)]) {
        for (k, v) in set {
            e.set_param(k, *v);
        }
    }

    #[test]
    fn the_ceiling_is_never_passed() {
        for (pn, pre) in MASTER_PRESETS.iter() {
            for extra in [0.0f32, 1.0] {
                let mut e = Master::new(SR);
                set(&mut e, pre);
                if extra > 0.0 {
                    // everything at its loudest
                    set(&mut e, &[("input", 1.0), ("push", 1.0), ("clip", 1.0), ("sat", 1.0), ("b1 up", 1.0), ("b4 up", 1.0), ("b2 gain", 1.0)]);
                }
                let mut b = music(SR as usize * 3);
                run(&mut e, &mut b);
                let ceil = e.lim.ceiling_db();
                let pk = b.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                assert!(lin2db(pk) <= ceil + 1e-3, "{pn}: sample peak {} over the ceiling {ceil}", lin2db(pk));
                let tp = tp_inner(&b, SR as usize / 2);
                assert!(tp <= ceil + 0.05, "{pn}: true peak {tp} over the ceiling {ceil}");
                assert!(b.iter().all(|x| x.is_finite()));
            }
        }
    }

    #[test]
    fn the_loudness_meter_reads_a_sine_right() {
        // a 997 Hz sine at -20 dBFS in both channels is -20 LUFS (BS.1770); at -14 dBFS -14
        for lvl in [-20.0f32, -14.0, -30.0] {
            let mut e = Master::new(SR);
            // BS.1770 is calibrated on the peak level: a 0 dBFS sine in one channel is -3.01 LUFS, in both 0 LUFS
            let a = db2lin(lvl);
            let mut b: Vec<f32> = (0..SR as usize * 6)
                .flat_map(|i| {
                    let y = (std::f32::consts::TAU * 997.0 * i as f32 / SR).sin() * a;
                    [y, y]
                })
                .collect();
            run(&mut e, &mut b);
            let i = e.integrated();
            assert!((i - lvl).abs() < 0.5, "{lvl} dBFS sine read {i} LUFS");
            assert!((e.meter.short - lvl).abs() < 0.5, "short-term {}", e.meter.short);
        }
    }

    #[test]
    fn neutral_is_the_plain_limiter_and_the_bands_sum_flat() {
        // the defaults (bands neutral) are bit-identical to the Limiter
        let src = music(SR as usize * 2);
        let mut a = src.clone();
        let mut e = Master::new(SR);
        e.set_param("push", 0.3);
        run(&mut e, &mut a);
        let mut b = src.clone();
        let mut l = Limiter::new(SR);
        for (n, v) in crate::dsp::edmfx::LIMITER_PARAMS {
            l.set_param(n, v);
        }
        l.set_param("gain", 0.3);
        run(&mut l, &mut b);
        assert_eq!(a, b, "neutral Ling Master = Limiter");
        // the split with a neutral band moved only by a hair sums to an all-pass: the level of noise is kept
        let mut e = Master::new(SR);
        e.set_param("ceiling", 1.0);
        e.set_param("b2 width", 0.5001);
        let mut n = noise(SR as usize * 2, 9, 0.1);
        let rin = (n.iter().map(|x| x * x).sum::<f32>() / n.len() as f32).sqrt();
        run(&mut e, &mut n);
        let rout = (n[SR as usize..].iter().map(|x| x * x).sum::<f32>() / (n.len() - SR as usize) as f32).sqrt();
        assert!((lin2db(rout) - lin2db(rin)).abs() < 0.3, "flat sum: {} dB", lin2db(rout) - lin2db(rin));
        assert!(e.latency() < (SR * 0.004) as usize, "low latency: {} samples", e.latency());
    }

    #[test]
    fn bands_compress_lift_widen_and_solo() {
        let tone = |hz: f32, amp: f32, n: usize| -> Vec<f32> {
            (0..n)
                .flat_map(|i| {
                    let y = (std::f32::consts::TAU * hz * i as f32 / SR).sin() * amp;
                    [y, y]
                })
                .collect()
        };
        let rms = |b: &[f32]| (b[b.len() / 2..].iter().map(|x| x * x).sum::<f32>() / (b.len() / 2) as f32).sqrt();
        let base = |set_: &[(&str, f32)], src: &[f32]| {
            let mut e = Master::new(SR);
            e.set_param("ceiling", 1.0);
            set(&mut e, set_);
            let mut b = src.to_vec();
            run(&mut e, &mut b);
            b
        };
        // a loud 60 Hz tone: band 1 at 10:1 from -30 dB brings it down a lot; band 4 compression leaves it alone
        let lo = tone(60.0, 0.5, SR as usize * 2);
        let c1 = rms(&base(&[("b1 ratio", 1.0), ("b1 thresh", 0.375)], &lo));
        let c4 = rms(&base(&[("b4 ratio", 1.0), ("b4 thresh", 0.375)], &lo));
        assert!(c1 < c4 * 0.4, "band 1 compresses the lows: {c1} vs {c4}");
        // a quiet 3 kHz tone comes up with upward compression in band 3
        let q = tone(3000.0, 0.01, SR as usize * 2);
        let u = rms(&base(&[("b3 up", 1.0), ("b3 thresh", 0.6)], &q));
        assert!(u > rms(&q) * 2.0, "upward lifts: {u}");
        // solo band 1 on a high tone: almost nothing
        let hi = tone(5000.0, 0.3, SR as usize);
        assert!(rms(&base(&[("b1 solo", 1.0)], &hi)) < 0.01);
        // width 0 on every band makes it mono; mono below kills the side of the lows
        let mut st = tone(80.0, 0.3, SR as usize * 2);
        for f in st.chunks_mut(2) {
            f[1] = -f[0];
        }
        let side = |b: &[f32]| (b[b.len() / 2..].chunks(2).map(|c| ((c[0] - c[1]) * 0.5).powi(2)).sum::<f32>() / (b.len() / 4) as f32).sqrt();
        let m = base(&[("mono below", mono(200.0))], &st);
        assert!(side(&m) < side(&st) * 0.2, "mono below: {} vs {}", side(&m), side(&st));
        let w = base(&[("b1 width", 0.0), ("b2 width", 0.0), ("b3 width", 0.0), ("b4 width", 0.0)], &st);
        assert!(side(&w) < side(&st) * 0.05);
        // analysis mode leaves the limiter out
        let mut e = Master::new(SR);
        e.command("prelimit", &[1.0], "");
        let mut b = tone(100.0, 1.5, 4800);
        run(&mut e, &mut b);
        assert!(b.iter().fold(0.0f32, |m, x| m.max(x.abs())) > 1.2);
        assert_eq!(e.view().len(), 4 + 4 + 1 + 4 + 3 + 4 + 4);
        assert!(MASTER_PRESETS.len() >= 10);
    }
}
