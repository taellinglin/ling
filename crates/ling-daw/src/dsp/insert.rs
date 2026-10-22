//! A small insert-effect rack that lives on ONE sound: a drum piece of Ling Membrane or a zone (sample) of the Sampler. Every sample can have
//! its own filter, saturation, bit-crusher, transient shaper, compressor, echo, room and stereo width, in that order, with all knobs 0..1.
//! With every knob at its default the rack is bypassed completely (no cost, no change to the sound).
use serde::{Deserialize, Serialize};

pub const NAMES: [&str; 14] = ["fx filter", "fx cutoff", "fx reso", "fx drive", "fx crush", "fx rate", "fx punch", "fx squash", "fx echo", "fx time", "fx fb", "fx verb", "fx size", "fx widen"];

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct InsertParams {
    /// 0..0.25 off, then low-pass, high-pass, band-pass
    pub filter: f32,
    pub cutoff: f32,
    pub reso: f32,
    pub drive: f32,
    /// bit depth reduction: 0 off .. 1 = 3 bits
    pub crush: f32,
    /// sample-rate reduction: 0 off .. 1 = 1/16 of the rate
    pub rate: f32,
    /// transient shaper: 0.5 neutral, below softens the attack, above adds snap
    pub punch: f32,
    /// compressor amount (fast, with make-up gain)
    pub squash: f32,
    pub echo: f32,
    /// echo time, 10 ms .. 600 ms
    pub time: f32,
    pub fb: f32,
    pub verb: f32,
    pub size: f32,
    /// 0.5 neutral, 0 mono, 1 twice as wide
    pub widen: f32,
}

impl Default for InsertParams {
    fn default() -> Self {
        InsertParams { filter: 0.0, cutoff: 0.8, reso: 0.25, drive: 0.0, crush: 0.0, rate: 0.0, punch: 0.5, squash: 0.0, echo: 0.0, time: 0.35, fb: 0.35, verb: 0.0, size: 0.4, widen: 0.5 }
    }
}

impl InsertParams {
    pub fn is_off(&self) -> bool {
        self.filter < 0.25 && self.drive <= 0.0 && self.crush <= 0.0 && self.rate <= 0.0 && (self.punch - 0.5).abs() < 0.01 && self.squash <= 0.0 && self.echo <= 0.0 && self.verb <= 0.0 && (self.widen - 0.5).abs() < 0.01
    }
    pub fn list(&self) -> [(&'static str, f32); 14] {
        let v = [self.filter, self.cutoff, self.reso, self.drive, self.crush, self.rate, self.punch, self.squash, self.echo, self.time, self.fb, self.verb, self.size, self.widen];
        std::array::from_fn(|i| (NAMES[i], v[i]))
    }
    /// by index into `NAMES`; false for an unknown index
    pub fn set_index(&mut self, i: usize, v: f32) -> bool {
        if !v.is_finite() {
            return true;
        }
        let v = v.clamp(0.0, 1.0);
        match i {
            0 => self.filter = v,
            1 => self.cutoff = v,
            2 => self.reso = v,
            3 => self.drive = v,
            4 => self.crush = v,
            5 => self.rate = v,
            6 => self.punch = v,
            7 => self.squash = v,
            8 => self.echo = v,
            9 => self.time = v,
            10 => self.fb = v,
            11 => self.verb = v,
            12 => self.size = v,
            13 => self.widen = v,
            _ => return false,
        }
        true
    }
    /// by name ("fx cutoff"); false when the name is not an insert knob
    pub fn set(&mut self, name: &str, v: f32) -> bool {
        match NAMES.iter().position(|n| *n == name) {
            Some(i) => self.set_index(i, v),
            None => false,
        }
    }
}

struct Comb {
    buf: Vec<f32>,
    i: usize,
    lp: f32,
}
struct Allpass {
    buf: Vec<f32>,
    i: usize,
}

const COMB_LEN: [usize; 4] = [1116, 1188, 1277, 1356];
const AP_LEN: [usize; 2] = [556, 441];

pub struct Insert {
    sr: f32,
    pub p: InsertParams,
    // filter: two-pole state-variable, per channel
    ic1: [f32; 2],
    ic2: [f32; 2],
    // transient shaper and compressor followers
    fast: f32,
    slow: f32,
    env: f32,
    // crusher hold
    hold: [f32; 2],
    cnt: f32,
    // echo
    el: Vec<f32>,
    er: Vec<f32>,
    ei: usize,
    // room
    combs: [[Comb; 4]; 2],
    aps: [[Allpass; 2]; 2],
}

fn comb(n: usize) -> Comb {
    Comb { buf: vec![0.0; n], i: 0, lp: 0.0 }
}
fn ap(n: usize) -> Allpass {
    Allpass { buf: vec![0.0; n], i: 0 }
}

impl Insert {
    pub fn new(sr: f32) -> Insert {
        // the echo line and the room are allocated the first time they are used: most samples never switch them on
        Insert {
            sr,
            p: InsertParams::default(),
            ic1: [0.0; 2],
            ic2: [0.0; 2],
            fast: 0.0,
            slow: 0.0,
            env: 0.0,
            hold: [0.0; 2],
            cnt: 0.0,
            el: vec![],
            er: vec![],
            ei: 0,
            combs: [[comb(0), comb(0), comb(0), comb(0)], [comb(0), comb(0), comb(0), comb(0)]],
            aps: [[ap(0), ap(0)], [ap(0), ap(0)]],
        }
    }

    fn build_room(&mut self) {
        let scale = (self.sr / 44100.0).max(0.25);
        let s = |n: usize| ((n as f32 * scale) as usize).max(8);
        for side in 0..2 {
            let off = side * 23;
            for (i, c) in self.combs[side].iter_mut().enumerate() {
                *c = comb(s(COMB_LEN[i]) + off);
            }
            for (i, a) in self.aps[side].iter_mut().enumerate() {
                *a = ap(s(AP_LEN[i]) + off);
            }
        }
    }

    pub fn set_params(&mut self, p: InsertParams) {
        self.p = p;
    }

    /// true when the rack changes the sound (a bypassed rack is skipped by the caller)
    pub fn active(&self) -> bool {
        !self.p.is_off()
    }

    pub fn clear(&mut self) {
        self.ic1 = [0.0; 2];
        self.ic2 = [0.0; 2];
        self.fast = 0.0;
        self.slow = 0.0;
        self.env = 0.0;
        self.el.iter_mut().for_each(|x| *x = 0.0);
        self.er.iter_mut().for_each(|x| *x = 0.0);
        for s in 0..2 {
            for c in self.combs[s].iter_mut() {
                c.buf.iter_mut().for_each(|x| *x = 0.0);
                c.lp = 0.0;
            }
            for a in self.aps[s].iter_mut() {
                a.buf.iter_mut().for_each(|x| *x = 0.0);
            }
        }
    }

    /// one stereo frame through the rack
    pub fn process(&mut self, mut l: f32, mut r: f32) -> (f32, f32) {
        let p = self.p;
        if p.is_off() {
            return (l, r);
        }
        // filter
        if p.filter >= 0.25 {
            let mode = if p.filter < 0.5 { 0 } else if p.filter < 0.75 { 1 } else { 2 };
            let fc = 30.0 * (18000.0f32 / 30.0).powf(p.cutoff.clamp(0.0, 1.0));
            let g = (std::f32::consts::PI * fc.min(self.sr * 0.45) / self.sr).tan();
            let k = 2.0 - 1.9 * p.reso.clamp(0.0, 1.0);
            let a1 = 1.0 / (1.0 + g * (g + k));
            let (a2, a3) = (g * a1, g * g * a1);
            for (ch, x) in [(0usize, &mut l), (1usize, &mut r)] {
                let v3 = *x - self.ic2[ch];
                let v1 = a1 * self.ic1[ch] + a2 * v3;
                let v2 = self.ic2[ch] + a2 * self.ic1[ch] + a3 * v3;
                self.ic1[ch] = 2.0 * v1 - self.ic1[ch];
                self.ic2[ch] = 2.0 * v2 - self.ic2[ch];
                *x = match mode {
                    0 => v2,
                    1 => *x - k * v1 - v2,
                    _ => v1 * k,
                };
            }
        }
        // saturation
        if p.drive > 0.0 {
            let g = 1.0 + p.drive * 9.0;
            let n = 1.0 / (1.0 + p.drive * 1.5);
            l = (l * g).tanh() * n * 1.4;
            r = (r * g).tanh() * n * 1.4;
        }
        // bit and sample-rate crusher
        if p.crush > 0.0 || p.rate > 0.0 {
            if p.rate > 0.0 {
                let step = 1.0 + p.rate * 15.0;
                self.cnt += 1.0;
                if self.cnt >= step {
                    self.cnt -= step;
                    self.hold = [l, r];
                }
                l = self.hold[0];
                r = self.hold[1];
            }
            if p.crush > 0.0 {
                let levels = 2f32.powf(15.0 - p.crush * 12.0) * 0.5;
                l = (l * levels).round() / levels;
                r = (r * levels).round() / levels;
            }
        }
        // transient shaper: the gap between a fast and a slow envelope follower is the attack
        if (p.punch - 0.5).abs() >= 0.01 {
            let m = (l.abs() + r.abs()) * 0.5;
            let (af, as_) = (1.0 - (-1.0 / (0.0008 * self.sr)).exp(), 1.0 - (-1.0 / (0.02 * self.sr)).exp());
            self.fast += (m - self.fast) * af;
            self.slow += (m - self.slow) * as_;
            let attack = ((self.fast - self.slow) / (self.slow + 0.02)).clamp(0.0, 4.0);
            let sustain = ((self.slow - self.fast) / (self.fast + 0.02)).clamp(0.0, 4.0);
            let d = (p.punch - 0.5) * 2.0;
            let gain = (1.0 + d * attack * 0.8 - d * sustain * 0.35).clamp(0.2, 4.0);
            l *= gain;
            r *= gain;
        }
        // compressor: a fast feed-forward peak follower, threshold falling as the amount rises
        if p.squash > 0.0 {
            let m = l.abs().max(r.abs());
            let att = 1.0 - (-1.0 / (0.001 * self.sr)).exp();
            let rel = 1.0 - (-1.0 / (0.12 * self.sr)).exp();
            self.env += (m - self.env) * if m > self.env { att } else { rel };
            let thr = 0.6 - 0.5 * p.squash;
            let g = if self.env > thr { (thr + (self.env - thr) / (2.0 + p.squash * 10.0)) / self.env } else { 1.0 };
            let makeup = 1.0 + p.squash * 1.2;
            l *= g * makeup;
            r *= g * makeup;
        }
        // echo (ping-pong)
        if p.echo > 0.0 {
            if self.el.is_empty() {
                let n = (self.sr * 0.62) as usize + 8;
                self.el = vec![0.0; n];
                self.er = vec![0.0; n];
                self.ei = 0;
            }
            let n = self.el.len();
            let d = (((0.01 + p.time * 0.59) * self.sr) as usize).clamp(1, n - 1);
            let rd = (self.ei + n - d) % n;
            let (wl, wr) = (self.el[rd], self.er[rd]);
            let fb = p.fb.clamp(0.0, 0.9);
            self.el[self.ei] = l + wr * fb;
            self.er[self.ei] = r + wl * fb;
            self.ei = (self.ei + 1) % n;
            l += wl * p.echo;
            r += wr * p.echo;
        }
        // room
        if p.verb > 0.0 {
            if self.combs[0][0].buf.is_empty() {
                self.build_room();
            }
            let x = (l + r) * 0.5 * 0.3;
            let fb = 0.7 + 0.26 * p.size.clamp(0.0, 1.0);
            let mut w = [0.0f32; 2];
            for s in 0..2 {
                let mut a = 0.0;
                for c in self.combs[s].iter_mut() {
                    let y = c.buf[c.i];
                    c.lp = y * 0.7 + c.lp * 0.3;
                    c.buf[c.i] = x + c.lp * fb;
                    c.i = (c.i + 1) % c.buf.len();
                    a += y;
                }
                a *= 0.25;
                for q in self.aps[s].iter_mut() {
                    let b = q.buf[q.i];
                    q.buf[q.i] = a + b * 0.5;
                    q.i = (q.i + 1) % q.buf.len();
                    a = b - a;
                }
                w[s] = a;
            }
            l += w[0] * p.verb * 1.5;
            r += w[1] * p.verb * 1.5;
        }
        // width (mid / side)
        if (p.widen - 0.5).abs() >= 0.01 {
            let (m, s) = ((l + r) * 0.5, (l - r) * 0.5);
            let k = p.widen * 2.0;
            l = m + s * k;
            r = m - s * k;
        }
        if !l.is_finite() || !r.is_finite() {
            self.clear();
            return (0.0, 0.0);
        }
        (l, r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(p: InsertParams, f: impl Fn(usize) -> f32, n: usize) -> Vec<(f32, f32)> {
        let mut i = Insert::new(48000.0);
        i.set_params(p);
        (0..n).map(|k| i.process(f(k), f(k))).collect()
    }
    fn rms(v: &[(f32, f32)]) -> f32 {
        (v.iter().map(|x| x.0 * x.0).sum::<f32>() / v.len().max(1) as f32).sqrt()
    }

    #[test]
    fn default_rack_is_bypassed() {
        assert!(InsertParams::default().is_off());
        let out = run(InsertParams::default(), |k| (k as f32 * 0.1).sin(), 500);
        assert!(out.iter().enumerate().all(|(k, o)| o.0 == (k as f32 * 0.1).sin()));
    }

    #[test]
    fn low_pass_removes_highs_and_high_pass_removes_lows() {
        let hi = |k: usize| (k as f32 * 2.0 * std::f32::consts::PI * 8000.0 / 48000.0).sin();
        let lo = |k: usize| (k as f32 * 2.0 * std::f32::consts::PI * 100.0 / 48000.0).sin();
        let lp = InsertParams { filter: 0.4, cutoff: 0.4, ..Default::default() };
        let hp = InsertParams { filter: 0.6, cutoff: 0.4, ..Default::default() };
        assert!(rms(&run(lp, hi, 4000)[1000..]) < 0.1, "lp kills 8 kHz");
        assert!(rms(&run(lp, lo, 4000)[1000..]) > 0.5, "lp keeps 100 Hz");
        assert!(rms(&run(hp, lo, 4000)[1000..]) < 0.1, "hp kills 100 Hz");
        assert!(rms(&run(hp, hi, 4000)[1000..]) > 0.5, "hp keeps 8 kHz");
    }

    #[test]
    fn echo_repeats_and_room_rings_on() {
        let imp = |k: usize| if k == 0 { 1.0 } else { 0.0 };
        let e = run(InsertParams { echo: 0.6, time: 0.2, ..Default::default() }, imp, 20000);
        let at = (0.01 + 0.2 * 0.59) * 48000.0;
        assert!(e[at as usize - 3..at as usize + 3].iter().any(|x| x.0.abs() > 0.3), "echo hit");
        let v = run(InsertParams { verb: 0.5, size: 0.8, ..Default::default() }, imp, 24000);
        assert!(rms(&v[8000..20000]) > 1e-4, "room tail");
    }

    #[test]
    fn crush_quantises_and_stays_finite_under_hostile_knobs() {
        let c = run(InsertParams { crush: 1.0, ..Default::default() }, |k| (k as f32 * 0.01).sin(), 2000);
        let mut levels: Vec<i32> = c.iter().map(|x| (x.0 * 1000.0).round() as i32).collect();
        levels.sort();
        levels.dedup();
        assert!(levels.len() < 40, "few levels: {}", levels.len());
        let mut p = InsertParams::default();
        for i in 0..14 {
            p.set_index(i, 1.0);
        }
        p.set_index(0, 0.9);
        for o in run(p, |k| if k % 97 == 0 { 50.0 } else { -3.0 }, 5000) {
            assert!(o.0.is_finite() && o.1.is_finite());
        }
        let mut q = InsertParams::default();
        q.set("fx drive", f32::NAN);
        q.set("fx cutoff", -5.0);
        assert!(q.drive == 0.0 && q.cutoff == 0.0);
        assert!(!q.set("nope", 1.0));
    }

    #[test]
    fn punch_adds_attack_and_squash_evens_levels() {
        // a decaying hit: more punch = a louder first few ms relative to the tail
        let hit = |k: usize| (k as f32 * 0.3).sin() * (-(k as f32) / 4000.0).exp();
        let neutral = run(InsertParams::default(), hit, 6000);
        let snappy = run(InsertParams { punch: 1.0, ..Default::default() }, hit, 6000);
        let ratio = |v: &[(f32, f32)]| rms(&v[0..200]) / rms(&v[3000..3400]).max(1e-6);
        assert!(ratio(&snappy) > ratio(&neutral) * 1.05, "{} vs {}", ratio(&snappy), ratio(&neutral));
        let loud_quiet = |k: usize| (k as f32 * 0.2).sin() * if (k / 15000) % 2 == 0 { 0.9 } else { 0.1 };
        let sq = run(InsertParams { squash: 1.0, ..Default::default() }, loud_quiet, 30000);
        let dry = run(InsertParams { drive: 0.0001, ..Default::default() }, loud_quiet, 30000);
        let spread = |v: &[(f32, f32)]| rms(&v[8000..14000]) / rms(&v[24000..30000]).max(1e-6);
        assert!(spread(&sq) < spread(&dry) * 0.8, "squash narrows the gap: {} vs {}", spread(&sq), spread(&dry));
    }
}
