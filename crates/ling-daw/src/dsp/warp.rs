//! **Ling Warp** - a tempo-synced time and volume shaper. The input is recorded into a long buffer and played back by a read head
//! whose position through every cycle (a beat, half a bar, a bar or two bars) follows a *time shape*, while a *gate shape* rides the
//! volume. Straight playback is the diagonal; a flatter line plays slower (half speed, tape stop), a falling line plays backwards,
//! a saw-tooth repeats (stutter, beat repeat), a line below the diagonal looks back into what was played before.
//!
//! * Eight **slots**, each with its own time shape, gate shape, cycle length and filter; `slot` switches between them live or from
//!   automation, at once, on the next beat or on the next cycle.
//! * 24 time shapes (half speed, tape stop / start, reverse, stutter and repeat, scratch, triplet and swing shuffle, beat juggle,
//!   lookback ...) and 16 gate shapes (sidechain pumps, trance and triplet gates, fades ...), each slot also drawable by hand.
//! * Every jump of the read head (a stutter restarting, a slot switch, the cycle wrapping) is crossfaded between two read heads, so
//!   nothing clicks; the gate and the filter are smoothed the same way; a head slowing to a stop fades out like a real tape.
//! * `swing` shifts every second 16th later by jumps of the head (not by speeding it up), so it swings without bending the pitch;
//!   `human` lets the gate's steps drift a little in time and level; `vary` adds fills at phrase ends (stutters, reverse beats,
//!   tape stops ...), chosen from the cycle number and `seed`, so a render sounds like playback.
//! * `warp` morphs between straight playback and the time shape, `depth` scales the gate; `mix` is the dry / wet.
//!
//! No latency: the head never reads ahead of the input. Allocation-free in `process`, finite for any input or setting.
use super::bassfx::{fin, g_of, Tsvf};
use super::cinefx::hash01;
use super::fx::Effect;

pub const NSLOT: usize = 8;
const NPTS: usize = 64;

pub const TIME_SHAPES: [&str; 25] = [
    "normal",
    "half speed",
    "half-time end",
    "slow-mo echo",
    "catch-up 2x",
    "tape stop",
    "stop on 4",
    "tape start",
    "stop & start",
    "reverse",
    "mirror half",
    "reverse beats",
    "repeat half",
    "repeat beat",
    "stutter 8th",
    "stutter 16th",
    "roll build",
    "scratch",
    "scratch fast",
    "triplet feel",
    "swing 16ths",
    "beat juggle",
    "lookback half",
    "lookback full",
    "drawn",
];

pub const GATE_SHAPES: [&str; 17] = [
    "open",
    "pump beat",
    "hard pump",
    "soft pump",
    "pump 8th",
    "gate 8th",
    "gate 16th",
    "trance",
    "offbeat",
    "triplet gate",
    "dotted gate",
    "fade in",
    "fade out",
    "swell",
    "end cut",
    "stutter gate",
    "drawn",
];

pub const LENS: [&str; 4] = ["beat", "half", "bar", "2 bars"];
const LEN_BEATS: [f64; 4] = [1.0, 2.0, 4.0, 8.0];
pub const FILTERS: [&str; 4] = ["off", "LP", "HP", "BP"];
pub const SWITCHES: [&str; 3] = ["instant", "next beat", "next cycle"];

/// the parameters that are not per slot
const NGLOB: usize = 11;
/// per slot: time, gate, len, filter, cut
const NPER: usize = 5;

const fn t(k: usize) -> f32 {
    (k as f32 + 0.5) / TIME_SHAPES.len() as f32
}
const fn g(k: usize) -> f32 {
    (k as f32 + 0.5) / GATE_SHAPES.len() as f32
}
const fn l(k: usize) -> f32 {
    (k as f32 + 0.5) / 4.0
}
const BAR: f32 = l(2);

pub const WARP_PARAMS: [(&str, f32); NGLOB + NSLOT * NPER] = [
    ("slot", 0.0),
    ("switch", 0.0),
    ("warp", 1.0),
    ("depth", 1.0),
    ("swing", 0.0),
    ("human", 0.0),
    ("vary", 0.0),
    ("seed", 0.0),
    ("xfade", 0.3),
    ("res", 0.2),
    ("mix", 1.0),
    ("s1 time", t(1)),
    ("s1 gate", g(0)),
    ("s1 len", BAR),
    ("s1 filter", 0.0),
    ("s1 cut", 0.6),
    ("s2 time", t(5)),
    ("s2 gate", g(0)),
    ("s2 len", BAR),
    ("s2 filter", 0.0),
    ("s2 cut", 0.6),
    ("s3 time", t(11)),
    ("s3 gate", g(0)),
    ("s3 len", BAR),
    ("s3 filter", 0.0),
    ("s3 cut", 0.6),
    ("s4 time", t(16)),
    ("s4 gate", g(0)),
    ("s4 len", BAR),
    ("s4 filter", 0.0),
    ("s4 cut", 0.6),
    ("s5 time", t(17)),
    ("s5 gate", g(0)),
    ("s5 len", BAR),
    ("s5 filter", 0.0),
    ("s5 cut", 0.6),
    ("s6 time", t(19)),
    ("s6 gate", g(0)),
    ("s6 len", BAR),
    ("s6 filter", 0.0),
    ("s6 cut", 0.6),
    ("s7 time", t(0)),
    ("s7 gate", g(1)),
    ("s7 len", BAR),
    ("s7 filter", 0.0),
    ("s7 cut", 0.6),
    ("s8 time", t(0)),
    ("s8 gate", g(7)),
    ("s8 len", BAR),
    ("s8 filter", 0.0),
    ("s8 cut", 0.6),
];

type Pre = (&'static str, &'static [(&'static str, f32)]);
const LP: f32 = 1.5 / 4.0;
const HP: f32 = 2.5 / 4.0;
const BP: f32 = 3.5 / 4.0;

/// Every preset sets up slot 1 (and the feel knobs); slots 2-8 keep the default bank, so a preset is a starting point to switch away from.
pub const WARP_PRESETS: [Pre; 34] = [
    ("Half-Time Groove", &[]),
    ("Half-Time Drop", &[("s1 time", t(2))]),
    ("Slow-Mo Echo", &[("s1 time", t(3)), ("s1 filter", LP), ("s1 cut", 0.72), ("mix", 0.7)]),
    ("Double Catch-Up", &[("s1 time", t(4)), ("s1 len", l(1))]),
    ("Tape Brake", &[("s1 time", t(5))]),
    ("Brake on the Four", &[("s1 time", t(6))]),
    ("Spin-Up", &[("s1 time", t(7)), ("s1 len", l(1))]),
    ("Stop and Go", &[("s1 time", t(8)), ("s1 len", l(3))]),
    ("Full Reverse", &[("s1 time", t(9))]),
    ("Mirror Half", &[("s1 time", t(10))]),
    ("Backspin Beats", &[("s1 time", t(11))]),
    ("Repeat Half Bar", &[("s1 time", t(12))]),
    ("Beat Repeat", &[("s1 time", t(13))]),
    ("Stutter Eighths", &[("s1 time", t(14))]),
    ("Stutter Sixteenths", &[("s1 time", t(15)), ("s1 gate", g(6)), ("depth", 0.4)]),
    ("Roll Builder", &[("s1 time", t(16)), ("s1 len", l(3))]),
    ("Baby Scratch", &[("s1 time", t(17))]),
    ("Chirp Scratch", &[("s1 time", t(18)), ("s1 filter", HP), ("s1 cut", 0.25)]),
    ("Triplet Shuffle", &[("s1 time", t(19))]),
    ("Swung Sixteenths", &[("s1 time", t(20))]),
    ("Beat Juggle", &[("s1 time", t(21))]),
    ("Lookback Half Bar", &[("s1 time", t(22)), ("mix", 0.6)]),
    ("Bar Echo", &[("s1 time", t(23)), ("mix", 0.45), ("s1 filter", BP), ("s1 cut", 0.62)]),
    ("Sidechain Pump", &[("s1 time", t(0)), ("s1 gate", g(1))]),
    ("Hard Pump", &[("s1 time", t(0)), ("s1 gate", g(2))]),
    ("Breathing Pump", &[("s1 time", t(0)), ("s1 gate", g(3))]),
    ("Eighth Pump", &[("s1 time", t(0)), ("s1 gate", g(4)), ("depth", 0.8)]),
    ("Trance Gate", &[("s1 time", t(0)), ("s1 gate", g(7))]),
    ("Offbeat Chop", &[("s1 time", t(0)), ("s1 gate", g(8))]),
    ("Triplet Gate", &[("s1 time", t(0)), ("s1 gate", g(9))]),
    ("Dotted Gate", &[("s1 time", t(0)), ("s1 gate", g(10)), ("depth", 0.85)]),
    ("Phrase Fills", &[("s1 time", t(0)), ("vary", 0.75)]),
    ("Swing and Hands", &[("s1 time", t(0)), ("s1 gate", g(5)), ("swing", 0.5), ("human", 0.55), ("depth", 0.7)]),
    ("Underwater Half-Time", &[("s1 time", t(1)), ("s1 filter", LP), ("s1 cut", 0.45), ("res", 0.45)]),
];

#[inline]
fn choice(v: f32, n: usize) -> usize {
    ((v.clamp(0.0, 1.0) * n as f32) as usize).min(n - 1)
}
#[inline]
fn frac(x: f32) -> f32 {
    x - x.floor()
}

/// The read position (in cycles, 0 = the start of this cycle) that time shape `s` plays at `x` (0..1) through the cycle. Never
/// ahead of the input (at most `x`) and never more than two cycles back.
pub fn time_at(s: usize, x: f32, drawn: &[f32; NPTS]) -> f32 {
    let x = x.clamp(0.0, 0.999_999);
    let beat = |x: f32| {
        let q = (x * 4.0).floor() / 4.0;
        (q, (x - q) * 4.0)
    };
    let p = match s {
        0 => x,
        1 => x * 0.5,
        2 => {
            if x < 0.5 {
                x
            } else {
                0.5 + (x - 0.5) * 0.5
            }
        }
        3 => x * 0.5 - 0.5,
        4 => 2.0 * x - 1.0,
        5 => x - x * x * 0.5,
        6 => {
            if x < 0.75 {
                x
            } else {
                let u = (x - 0.75) * 4.0;
                0.75 + (u - u * u * 0.5) * 0.25
            }
        }
        7 => x * x * 0.5,
        8 => {
            if x < 0.5 {
                let u = x * 2.0;
                0.5 * (u - u * u * 0.5)
            } else {
                let u = x * 2.0 - 1.0;
                0.25 + 0.25 * u * u
            }
        }
        9 => -x,
        10 => {
            if x < 0.5 {
                x
            } else {
                1.0 - x
            }
        }
        11 => {
            let (q, u) = beat(x);
            q - u * 0.25
        }
        12 => x % 0.5,
        13 => x % 0.25,
        14 => x % 0.125,
        15 => x % 0.0625,
        16 => {
            if x < 0.5 {
                x
            } else if x < 0.75 {
                0.5 + (x - 0.5) % 0.125
            } else if x < 0.875 {
                0.5 + (x - 0.75) % 0.0625
            } else {
                0.5 + (x - 0.875) % 0.031_25
            }
        }
        17 => {
            let (q, u) = beat(x);
            let s = (std::f32::consts::TAU * u).sin();
            q + 0.055 * s * s
        }
        18 => {
            let (q, u) = beat(x);
            let s = (2.0 * std::f32::consts::TAU * u).sin();
            q + 0.03 * s * s
        }
        19 => {
            let (q, u) = beat(x);
            let w = if u < 2.0 / 3.0 { u * 0.75 } else { 0.5 + (u - 2.0 / 3.0) * 1.5 };
            q + w * 0.25
        }
        20 => {
            let k = (x * 8.0).floor();
            let u = x * 8.0 - k;
            let w = if u < 0.6 { u * 0.5 / 0.6 } else { 0.5 + (u - 0.6) * 1.25 };
            (k + w) / 8.0
        }
        21 => {
            const ORDER: [f32; 8] = [0.0, 1.0, 0.0, 3.0, 4.0, 2.0, 6.0, 5.0];
            let k = ((x * 8.0) as usize).min(7);
            ORDER[k] / 8.0 + (x - k as f32 / 8.0)
        }
        22 => x - 0.5,
        23 => x - 1.0,
        _ => drawn_at(drawn, x, true),
    };
    p.min(x).max(x - 2.0)
}

/// a drawn curve at `x`: linear between neighbouring points, but a step (a jump, crossfaded) where the time curve jumps
fn drawn_at(pts: &[f32; NPTS], x: f32, steps: bool) -> f32 {
    let p = x.clamp(0.0, 0.999_999) * NPTS as f32;
    let i = (p as usize).min(NPTS - 1);
    let f = p - i as f32;
    let a = pts[i];
    let b = if i + 1 < NPTS { pts[i + 1] } else { a + (a - pts[i - 1]) };
    if steps && (b - a).abs() > 0.1 {
        a
    } else {
        a + (b - a) * f
    }
}

/// The volume (0..1) of gate shape `s` at `x` through the cycle.
pub fn gate_at(s: usize, x: f32, drawn: &[f32; NPTS]) -> f32 {
    let x = x.clamp(0.0, 0.999_999);
    let pump = |u: f32| (1.0 - (-7.0 * u).exp()) / (1.0 - (-7.0f32).exp());
    match s {
        0 => 1.0,
        1 => pump(frac(x * 4.0)),
        2 => {
            let u = frac(x * 4.0);
            let r = ((u - 0.06) / 0.62).clamp(0.0, 1.0);
            r * r * (3.0 - 2.0 * r)
        }
        3 => 0.45 + 0.55 * pump(frac(x * 4.0)),
        4 => pump(frac(x * 8.0)),
        5 => (frac(x * 8.0) < 0.55) as u8 as f32,
        6 => (frac(x * 16.0) < 0.5) as u8 as f32,
        7 => {
            const PAT: [u8; 16] = [1, 0, 1, 1, 0, 1, 1, 0, 1, 0, 1, 1, 0, 1, 0, 1];
            let k = ((x * 16.0) as usize).min(15);
            (PAT[k] == 1 && frac(x * 16.0) < 0.85) as u8 as f32
        }
        8 => ((x * 8.0) as usize % 2 == 1 && frac(x * 8.0) < 0.9) as u8 as f32,
        9 => {
            let k = (x * 12.0) as usize;
            (k % 3 != 2 && frac(x * 12.0) < 0.8) as u8 as f32
        }
        10 => (frac(x * 16.0 / 3.0) < 0.5) as u8 as f32,
        11 => x,
        12 => 1.0 - x,
        13 => 0.5 - 0.5 * (std::f32::consts::TAU * x).cos(),
        14 => (x < 0.875) as u8 as f32,
        15 => {
            if x < 0.5 {
                1.0
            } else if x < 0.75 {
                (frac(x * 16.0) < 0.5) as u8 as f32
            } else {
                (frac(x * 32.0) < 0.5) as u8 as f32
            }
        }
        _ => drawn_at(drawn, x, false).clamp(0.0, 1.0),
    }
}

/// the fills the variations mode plays: (time shape, where in the cycle the fill starts)
const FILLS: [(usize, f32); 7] = [(13, 0.5), (14, 0.75), (12, 0.75), (5, 0.75), (9, 0.75), (1, 0.5), (16, 0.5)];

#[derive(Clone, Copy)]
struct Slot {
    time: usize,
    gate: usize,
    len: usize,
    filter: usize,
    cut: f32,
    dt: [f32; NPTS],
    dg: [f32; NPTS],
}
impl Slot {
    fn new() -> Slot {
        let mut dt = [0.0; NPTS];
        for (i, d) in dt.iter_mut().enumerate() {
            *d = i as f32 / NPTS as f32;
        }
        Slot { time: 0, gate: 0, len: 2, filter: 0, cut: 0.6, dt, dg: [1.0; NPTS] }
    }
}

#[derive(Clone, Copy, Default)]
struct Head {
    r: f64,
    v: f64,
}

pub struct Warp {
    sr: f32,
    p: [f32; NGLOB + NSLOT * NPER],
    slots: [Slot; NSLOT],
    active: usize,
    want: usize,
    buf: [Vec<f32>; 2],
    /// samples written so far (the write position on an endless timeline)
    n: u64,
    cur: Head,
    old: Head,
    fade: f32,
    pending: bool,
    bpm: f64,
    beat: f64,
    last_base: f64,
    /// the cycle and beat numbers last seen (slot switches wait for them to change)
    last_cycle: i64,
    last_beat: i64,
    /// the fill of this cycle (variations): index into FILLS
    fill: Option<usize>,
    gate_s: f32,
    lvl_s: f32,
    svf: [Tsvf; 2],
    /// filter blend weights: off, low, high, band
    fw: [f32; 4],
    cut_s: f32,
    // for the display
    phase: f32,
    speed: f32,
}

impl Warp {
    pub fn new(sr: f32) -> Warp {
        let n = (sr * 20.0) as usize;
        let mut w = Warp {
            sr,
            p: [0.0; NGLOB + NSLOT * NPER],
            slots: [Slot::new(); NSLOT],
            active: 0,
            want: 0,
            buf: [vec![0.0; n], vec![0.0; n]],
            n: 0,
            cur: Head { r: -1.0, v: 1.0 },
            old: Head::default(),
            fade: 1.0,
            pending: false,
            bpm: 120.0,
            beat: 0.0,
            last_base: f64::NAN,
            last_cycle: i64::MIN,
            last_beat: i64::MIN,
            fill: None,
            gate_s: 1.0,
            lvl_s: 1.0,
            svf: [Tsvf::default(); 2],
            fw: [1.0, 0.0, 0.0, 0.0],
            cut_s: 0.6,
            phase: 0.0,
            speed: 1.0,
        };
        for (n, v) in WARP_PARAMS {
            w.set_param(n, v);
        }
        w.active = w.want;
        w
    }

    pub fn active_slot(&self) -> usize {
        self.active
    }

    fn cycle_beats(&self, s: usize) -> f64 {
        LEN_BEATS[self.slots[s].len]
    }

    #[inline]
    fn read(&self, c: usize, r: f64) -> f32 {
        let n = self.buf[c].len();
        let now = self.n as f64;
        let r = r.clamp(now - (n - 8) as f64, now).max(0.0);
        let i = r.floor();
        let f = (r - i) as f32;
        let at = |k: f64| -> f32 {
            if k < 0.0 {
                return 0.0;
            }
            let k = k.min(now) as u64;
            self.buf[c][(k % n as u64) as usize]
        };
        let (y0, y1, y2, y3) = (at(i - 1.0), at(i), at(i + 1.0), at(i + 2.0));
        if f == 0.0 {
            return y1;
        }
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        ((c3 * f + c2) * f + c1) * f + y1
    }

    /// the swing of the head: every second 16th starts late by a jump back (no change of speed, so no change of pitch); `n16` 16ths per cycle
    #[inline]
    fn swing_time(x: f32, sw: f32, n16: f32) -> f32 {
        if sw <= 0.0 {
            return x;
        }
        let s = 0.5 + sw * 0.25;
        let v = x * n16 * 0.5;
        let q = v.floor();
        let u = v - q;
        let w = if u < s { u } else { u - (s - 0.5) };
        (q + w) * 2.0 / n16
    }
    /// the swing of the gate: the pattern itself is bent so the second 16th opens late
    #[inline]
    fn swing_gate(x: f32, sw: f32, n16: f32) -> f32 {
        if sw <= 0.0 {
            return x;
        }
        let s = 0.5 + sw * 0.25;
        let v = x * n16 * 0.5;
        let q = v.floor();
        let u = v - q;
        let w = if u < s { u * 0.5 / s } else { 0.5 + (u - s) * 0.5 / (1.0 - s) };
        (q + w) * 2.0 / n16
    }

    /// the read position (cycles from this cycle's start) and the gate of slot `s` at `x` through cycle number `k`
    fn shape(&self, s: usize, x: f32, k: i64) -> (f32, f32) {
        let sl = &self.slots[s];
        let n16 = (LEN_BEATS[sl.len] * 4.0) as f32;
        let sw = self.p[4];
        let xt = Self::swing_time(x, sw, n16);
        let mut p = time_at(sl.time, xt, &sl.dt);
        if let Some(fi) = self.fill {
            let (fs, st) = (FILLS[fi].0, FILLS[fi].1);
            if x >= st {
                let u = (x - st) / (1.0 - st);
                p = st + time_at(fs, u, &sl.dt) * (1.0 - st);
            }
        }
        let warp = self.p[2];
        let p = x + warp * (p - x);
        // the gate: swung, and with `human` each 16th a little early or late and a little softer or louder
        let mut xg = Self::swing_gate(x, sw, n16);
        let hu = self.p[5];
        let mut acc = 1.0;
        if hu > 0.0 {
            let step = (xg * n16).floor() as i64 + k * n16 as i64;
            let seed = (self.p[7] * 1000.0) as u32;
            xg = (xg + (hash01(step as u64, seed ^ 0x51ED) - 0.5) * hu * 0.35 / n16).clamp(0.0, 0.999_999);
            acc = 1.0 - hu * 0.3 * hash01(step as u64, seed ^ 0xACC);
        }
        let gv = gate_at(sl.gate, xg, &sl.dg) * acc;
        (p, 1.0 - self.p[3] * (1.0 - gv))
    }

    /// whether cycle `k` gets a fill (variations): more likely on the last cycle of four, then of two
    fn pick_fill(&self, k: i64) -> Option<usize> {
        let vary = self.p[6];
        if vary <= 0.0 {
            return None;
        }
        let w = if k.rem_euclid(4) == 3 {
            1.0
        } else if k.rem_euclid(2) == 1 {
            0.35
        } else {
            0.12
        };
        let seed = (self.p[7] * 1000.0) as u32;
        if hash01(k as u64, seed ^ 0xF111) < vary * w {
            Some(((hash01(k as u64, seed ^ 0x7E57) * FILLS.len() as f32) as usize).min(FILLS.len() - 1))
        } else {
            None
        }
    }

    fn slot_param(&mut self, s: usize, which: usize, v: f32) {
        let sl = &mut self.slots[s];
        match which {
            0 => sl.time = choice(v, TIME_SHAPES.len()),
            1 => sl.gate = choice(v, GATE_SHAPES.len()),
            2 => sl.len = choice(v, LENS.len()),
            3 => sl.filter = choice(v, FILTERS.len()),
            _ => sl.cut = v,
        }
    }

    /// read a saved state back: {"slots":[{"t":[...],"g":[...]}, ...]}
    pub fn load_config(&mut self, text: &str) -> bool {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return false };
        let Some(a) = v["slots"].as_array() else { return false };
        for (s, o) in a.iter().take(NSLOT).enumerate() {
            if let Some(t) = o["t"].as_array() {
                for (i, x) in t.iter().take(NPTS).enumerate() {
                    self.slots[s].dt[i] = (x.as_f64().unwrap_or(0.0) as f32).clamp(-1.0, 1.0);
                }
            }
            if let Some(g) = o["g"].as_array() {
                for (i, x) in g.iter().take(NPTS).enumerate() {
                    self.slots[s].dg[i] = (x.as_f64().unwrap_or(1.0) as f32).clamp(0.0, 1.0);
                }
            }
        }
        true
    }
}

impl Effect for Warp {
    fn process(&mut self, buf: &mut [f32]) {
        let sr = self.sr;
        let nbuf = self.buf[0].len();
        let bps = self.bpm / 60.0 / sr as f64;
        let mix = self.p[10];
        let xf_len = (0.001 + self.p[8] * 0.019) * sr;
        let dfade = 1.0 / xf_len.max(1.0);
        let kg = 1.0 - (-1.0 / (0.0012 * sr)).exp();
        let kl = 1.0 - (-1.0 / (0.004 * sr)).exp();
        let kf = 1.0 - (-1.0 / (0.012 * sr)).exp();
        let res = self.p[9];
        let kq = 2.0 - 1.85 * res;
        let switch = choice(self.p[1], SWITCHES.len());
        const THR: f64 = 3.0;
        for fr in buf.chunks_exact_mut(2) {
            let x = [fin(fr[0]), fin(fr[1])];
            let wi = (self.n % nbuf as u64) as usize;
            self.buf[0][wi] = x[0];
            self.buf[1][wi] = x[1];
            // the slot switch, at once or on the next beat / cycle
            let bi = self.beat.floor() as i64;
            let cb = self.cycle_beats(self.active);
            let ci = (self.beat / cb).floor() as i64;
            if self.want != self.active {
                let go = match switch {
                    0 => true,
                    1 => bi != self.last_beat,
                    _ => ci != self.last_cycle,
                };
                if go {
                    self.active = self.want;
                }
            }
            self.last_beat = bi;
            let cb = self.cycle_beats(self.active);
            let k = (self.beat / cb).floor() as i64;
            if k != self.last_cycle {
                self.last_cycle = k;
                self.fill = self.pick_fill(k);
            }
            let xph = ((self.beat / cb) - k as f64) as f32;
            self.phase = xph;
            let (p, gt) = self.shape(self.active, xph, k);
            let ls = cb * 60.0 / self.bpm * sr as f64;
            let d = (((xph - p) as f64) * ls).clamp(0.0, (nbuf - 16) as f64);
            let now = self.n as f64;
            let target = now - d;
            // the read heads: the current one follows the target; a jump hands it to the old head, which fades out as the new one fades in
            let pred = self.cur.r + self.cur.v;
            let jumped = (target - pred).abs() > THR;
            if self.fade >= 1.0 {
                if jumped {
                    self.old = self.cur;
                    self.cur = Head { r: target, v: 1.0 };
                    self.fade = 0.0;
                } else {
                    self.cur.v = (target - self.cur.r).clamp(-4.0, 4.0);
                    self.cur.r = target;
                }
            } else {
                if jumped || self.pending {
                    // a second jump during a fade waits for the fade to end: until then the head runs on
                    self.pending = true;
                    self.cur.r = (self.cur.r + self.cur.v).min(now);
                } else {
                    self.cur.v = (target - self.cur.r).clamp(-4.0, 4.0);
                    self.cur.r = target;
                }
                self.old.r = (self.old.r + self.old.v).min(now);
                self.fade += dfade;
                if self.fade >= 1.0 {
                    self.fade = 1.0;
                    self.pending = false;
                }
            }
            let (a, b) = if self.fade >= 1.0 {
                (1.0, 0.0)
            } else {
                let h = self.fade * std::f32::consts::FRAC_PI_2;
                (h.sin(), h.cos())
            };
            let mut y = [0.0f32; 2];
            for (c, yc) in y.iter_mut().enumerate() {
                *yc = self.read(c, self.cur.r) * a;
                if b > 0.0 {
                    *yc += self.read(c, self.old.r) * b;
                }
            }
            // a head slowing to a standstill fades out (a stopped tape is silent, not a held sample)
            let spd = (self.cur.v.abs() as f32) * a + (self.old.v.abs() as f32) * b;
            self.speed = spd;
            let lv = (spd * 3.0).min(1.0).sqrt();
            self.lvl_s += (lv - self.lvl_s) * kl;
            self.gate_s += (gt - self.gate_s) * kg;
            let gain = self.gate_s * self.lvl_s;
            // the slot's filter, blended so a change of type or cutoff never clicks
            let sl = &self.slots[self.active];
            let mut tw = [0.0f32; 4];
            tw[sl.filter] = 1.0;
            self.cut_s += (sl.cut - self.cut_s) * kf;
            for (w, t) in self.fw.iter_mut().zip(tw) {
                *w += (t - *w) * kf;
            }
            let filt = self.fw[0] < 0.9999;
            let gg = if filt { g_of(20.0 * 1000f32.powf(self.cut_s), sr) } else { 0.0 };
            for c in 0..2 {
                let mut v = y[c] * gain;
                if filt {
                    let (lo, bp, hi) = self.svf[c].run(v, gg, kq);
                    v = v * self.fw[0] + lo * self.fw[1] + hi * self.fw[2] + bp * kq * self.fw[3];
                }
                fr[c] = fin(x[c] * (1.0 - mix) + v * mix);
            }
            self.n += 1;
            self.beat += bps;
        }
    }
    fn set_param(&mut self, n: &str, v: f32) {
        if !v.is_finite() {
            return;
        }
        let v = v.clamp(0.0, 1.0);
        let Some(i) = WARP_PARAMS.iter().position(|p| p.0 == n) else { return };
        self.p[i] = v;
        if i == 0 {
            self.want = choice(v, NSLOT);
        } else if i >= NGLOB {
            let j = i - NGLOB;
            self.slot_param(j / NPER, j % NPER, v);
        }
    }
    fn name(&self) -> &'static str {
        "Ling Warp"
    }
    fn set_transport(&mut self, bpm: f64, beat: f64) {
        if bpm.is_finite() && bpm > 20.0 {
            self.bpm = bpm;
        }
        if !beat.is_finite() {
            return;
        }
        // stopped (the position does not move between blocks): keep time on our own so a live input still warps
        let moved = self.last_base.is_nan() || (beat - self.last_base).abs() > 1e-9;
        self.last_base = beat;
        if moved {
            self.beat = beat;
        }
    }
    fn view(&self) -> Vec<f32> {
        // 64 points of the active slot's time curve ((position + 1) / 2: the diagonal from 0.5 to 1 is straight playback), 64 of its
        // gate, then: phase, active slot, wanted slot, fill on, fill start, head speed, cycle beats
        let s = self.active;
        let k = self.last_cycle.max(0);
        let mut v = Vec::with_capacity(136);
        for i in 0..NPTS {
            let x = i as f32 / NPTS as f32;
            v.push(((self.shape(s, x, k).0 + 1.0) * 0.5).clamp(0.0, 1.0));
        }
        for i in 0..NPTS {
            let x = (i as f32 + 0.5) / NPTS as f32;
            v.push(self.shape(s, x, k).1.clamp(0.0, 1.0));
        }
        v.push(self.phase);
        v.push(self.active as f32);
        v.push(self.want as f32);
        v.push(self.fill.is_some() as u8 as f32);
        v.push(self.fill.map(|f| FILLS[f].1).unwrap_or(1.0));
        v.push(self.speed.min(4.0));
        v.push(self.cycle_beats(s) as f32);
        v
    }
    fn command(&mut self, cmd: &str, args: &[f64], text: &str) -> Vec<f64> {
        let s = self.active;
        match cmd {
            // (index 0..63, position -1..1): one point of the active slot's drawn time curve
            "tpoint" if args.len() >= 2 => {
                let i = (args[0].max(0.0) as usize).min(NPTS - 1);
                self.slots[s].dt[i] = (args[1] as f32).clamp(-1.0, i as f32 / NPTS as f32);
                vec![1.0]
            }
            // (index 0..63, volume 0..1): one point of the active slot's drawn gate
            "gpoint" if args.len() >= 2 => {
                let i = (args[0].max(0.0) as usize).min(NPTS - 1);
                self.slots[s].dg[i] = (args[1] as f32).clamp(0.0, 1.0);
                vec![1.0]
            }
            // copy the shape in use into the drawn curve (before drawing over a built-in shape)
            "from_time" => {
                let sl = self.slots[s];
                for i in 0..NPTS {
                    self.slots[s].dt[i] = time_at(sl.time, i as f32 / NPTS as f32, &sl.dt);
                }
                vec![1.0]
            }
            "from_gate" => {
                let sl = self.slots[s];
                for i in 0..NPTS {
                    self.slots[s].dg[i] = gate_at(sl.gate, (i as f32 + 0.5) / NPTS as f32, &sl.dg);
                }
                vec![1.0]
            }
            "load_config" => {
                self.load_config(text);
                vec![]
            }
            _ => vec![],
        }
    }
    fn info(&self, what: &str) -> String {
        match what {
            "config" => {
                // only drawn curves that differ from the defaults are saved (the knobs are parameters)
                let def = Slot::new();
                if self.slots.iter().all(|s| s.dt == def.dt && s.dg == def.dg) {
                    return String::new();
                }
                let f = |a: &[f32]| a.iter().map(|x| format!("{x:.4}")).collect::<Vec<_>>().join(",");
                let parts: Vec<String> = self.slots.iter().map(|s| format!("{{\"t\":[{}],\"g\":[{}]}}", f(&s.dt), f(&s.dg))).collect();
                format!("{{\"slots\":[{}]}}", parts.join(","))
            }
            "latency" => "0".into(),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;

    fn sine(n: usize, hz: f32, amp: f32) -> Vec<f32> {
        (0..n).flat_map(|i| {
            let y = (std::f32::consts::TAU * hz * i as f32 / SR).sin() * amp;
            [y, y]
        })
        .collect()
    }
    fn run(e: &mut Warp, b: &mut [f32], bpm: f64, beat0: f64) {
        let mut beat = beat0;
        for c in b.chunks_mut(256) {
            e.set_transport(bpm, beat);
            e.process(c);
            beat += (c.len() / 2) as f64 * bpm / 60.0 / SR as f64;
        }
    }
    fn max_step(b: &[f32]) -> f32 {
        let l: Vec<f32> = b.chunks(2).map(|c| c[0]).collect();
        l.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
    }
    fn set_slot1(e: &mut Warp, time: usize, gate: usize) {
        e.set_param("s1 time", t(time));
        e.set_param("s1 gate", g(gate));
    }

    #[test]
    fn every_time_shape_is_causal_and_bounded() {
        let d = Slot::new().dt;
        for s in 0..TIME_SHAPES.len() {
            for i in 0..1000 {
                let x = i as f32 / 1000.0;
                let p = time_at(s, x, &d);
                assert!(p <= x + 1e-6 && p >= x - 2.0 - 1e-6, "{} at {x}: {p}", TIME_SHAPES[s]);
            }
        }
        for s in 0..GATE_SHAPES.len() {
            for i in 0..1000 {
                let v = gate_at(s, i as f32 / 1000.0, &[1.0; NPTS]);
                assert!((0.0..=1.0).contains(&v));
            }
        }
        assert_eq!(WARP_PARAMS.len(), NGLOB + NSLOT * NPER);
        assert!(WARP_PRESETS.len() >= 30);
    }

    #[test]
    fn the_read_head_is_tempo_synced() {
        // the input is its own sample index, so the output says where the head reads
        for bpm in [90.0f64, 128.0, 174.0] {
            let mut e = Warp::new(SR);
            set_slot1(&mut e, 1, 0); // half speed, 1 bar
            let bar = (4.0 * 60.0 / bpm * SR as f64) as usize;
            let n = bar * 3;
            let mut b: Vec<f32> = (0..n).flat_map(|i| [i as f32 * 1e-6, i as f32 * 1e-6]).collect();
            run(&mut e, &mut b, bpm, 0.0);
            // half way through bar 2 the head is a quarter bar into it; three quarters through, three eighths
            for (at, want) in [(0.5, 0.25), (0.75, 0.375), (0.9, 0.45)] {
                let i = bar + (bar as f64 * at) as usize;
                let got = b[i * 2] as f64 * 1e6;
                let expect = bar as f64 + bar as f64 * want;
                assert!((got - expect).abs() < 4.0, "{bpm} bpm at {at}: read {got}, expected {expect}");
            }
            // and every bar starts on the live input again (after the crossfade)
            let i = 2 * bar + 2000;
            assert!((b[i * 2] as f64 * 1e6 - (2 * bar + 1000) as f64).abs() < 4.0, "{bpm}: bar 3 starts live: {}", b[i * 2] as f64 * 1e6);
        }
        // the beat the transport reports is followed: starting half a bar in, the head is already a quarter bar behind
        let mut e = Warp::new(SR);
        set_slot1(&mut e, 1, 0);
        let bar = (4.0 * 60.0 / 120.0 * SR as f64) as usize;
        let mut b: Vec<f32> = (0..bar).flat_map(|i| [i as f32 * 1e-6, i as f32 * 1e-6]).collect();
        run(&mut e, &mut b, 120.0, 2.0);
        let i = bar / 4;
        let got = b[i * 2] as f64 * 1e6;
        // three quarters through the bar, position 3/8: the head reads 3/8 bar - 1/2 bar back from the start of the buffer (clamped at 0)
        assert!(got < i as f64 - 1000.0, "the head lags: {got} at {i}");
    }

    #[test]
    fn no_clicks_at_jumps_and_slot_switches() {
        // a sine through every time shape, with slot switches every 0.37 s: the biggest step between samples stays near the sine's own
        let amp = 0.5;
        let src = sine(SR as usize * 8, 110.0, amp);
        let own = max_step(&src);
        for ts in 0..TIME_SHAPES.len() - 1 {
            let mut e = Warp::new(SR);
            set_slot1(&mut e, ts, 0);
            let mut b = src.clone();
            run(&mut e, &mut b, 128.0, 0.0);
            let st = max_step(&b[2000..]);
            assert!(st < own * 4.5 + 0.02, "{}: step {st} (sine {own})", TIME_SHAPES[ts]);
        }
        let mut e = Warp::new(SR);
        let mut b = src.clone();
        let seg = (SR * 0.37) as usize * 2;
        let mut beat = 0.0;
        for (k, c) in b.chunks_mut(seg).enumerate() {
            e.set_param("slot", ((k % NSLOT) as f32 + 0.5) / NSLOT as f32);
            for cc in c.chunks_mut(256) {
                e.set_transport(128.0, beat);
                e.process(cc);
                beat += (cc.len() / 2) as f64 * 128.0 / 60.0 / SR as f64;
            }
        }
        let st = max_step(&b[2000..]);
        assert!(st < own * 4.5 + 0.03, "slot switches: step {st}");
        // without the crossfade a jump of the head is a step of up to twice the amplitude: make sure the test would see one
        assert!(own * 4.5 + 0.03 < amp);
    }

    #[test]
    fn gates_pump_and_mix_zero_is_dry() {
        let src = sine(SR as usize * 2, 220.0, 0.5);
        let mut e = Warp::new(SR);
        set_slot1(&mut e, 0, 1);
        let mut b = src.clone();
        run(&mut e, &mut b, 120.0, 0.0);
        // right on a beat the pump is down, late in the beat it is open
        let beat = (SR * 0.5) as usize;
        let rms = |a: usize, n: usize| (b[a * 2..(a + n) * 2].iter().map(|x| x * x).sum::<f32>() / (n * 2) as f32).sqrt();
        assert!(rms(beat * 2 + 100, 400) < 0.12, "ducked {}", rms(beat * 2 + 100, 400));
        assert!(rms(beat * 2 + beat * 3 / 4, 2000) > 0.3);
        let mut e = Warp::new(SR);
        e.set_param("mix", 0.0);
        let mut b = src.clone();
        run(&mut e, &mut b, 120.0, 0.0);
        assert!(b.iter().zip(&src).all(|(a, b)| (a - b).abs() < 1e-6));
        // normal + open is a straight copy
        let mut e = Warp::new(SR);
        set_slot1(&mut e, 0, 0);
        let mut b = src.clone();
        run(&mut e, &mut b, 120.0, 0.0);
        let err = b.iter().zip(&src).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 1e-5, "normal changed the signal by {err}");
    }

    #[test]
    fn swing_and_variations_are_deterministic_and_stay_finite() {
        let src = sine(SR as usize * 10, 330.0, 0.4);
        let render = |vary: f32, swing: f32, human: f32| {
            let mut e = Warp::new(SR);
            set_slot1(&mut e, 0, 6);
            e.set_param("vary", vary);
            e.set_param("swing", swing);
            e.set_param("human", human);
            let mut b = src.clone();
            run(&mut e, &mut b, 140.0, 0.0);
            b
        };
        let a = render(1.0, 0.5, 0.6);
        let b = render(1.0, 0.5, 0.6);
        assert_eq!(a, b, "the same settings render the same");
        assert!(a.iter().all(|x| x.is_finite() && x.abs() < 2.0));
        assert_ne!(a, render(0.0, 0.5, 0.6), "variations change something");
        // the fills come mostly at phrase ends
        let e = {
            let mut e = Warp::new(SR);
            e.set_param("vary", 0.5);
            e
        };
        let ends = (0..400).filter(|k| k % 4 == 3 && e.pick_fill(*k).is_some()).count();
        let starts = (0..400).filter(|k| k % 4 == 0 && e.pick_fill(*k).is_some()).count();
        assert!(ends > starts * 3, "{ends} fills on the 4th cycle, {starts} on the 1st");
        // hostile input and every knob at the ends
        for v in [0.0f32, 1.0] {
            let mut e = Warp::new(SR);
            for (n, _) in WARP_PARAMS {
                e.set_param(n, v);
            }
            let mut b = sine(SR as usize, 50.0, 0.9);
            b[100] = f32::NAN;
            b[301] = f32::INFINITY;
            b[500] = 40.0;
            run(&mut e, &mut b, 300.0, 0.0);
            assert!(b.iter().all(|x| x.is_finite() && x.abs() <= 8.0));
        }
    }

    #[test]
    fn drawn_curves_are_saved_and_loaded() {
        let mut e = Warp::new(SR);
        assert_eq!(e.info("config"), "");
        e.command("from_time", &[], "");
        e.command("tpoint", &[10.0, -0.5], "");
        e.command("gpoint", &[3.0, 0.25], "");
        let cfg = e.info("config");
        let mut f = Warp::new(SR);
        assert!(f.load_config(&cfg));
        assert!((f.slots[0].dt[10] + 0.5).abs() < 1e-3 && (f.slots[0].dg[3] - 0.25).abs() < 1e-3);
        // a drawn point can not be ahead of the input
        e.command("tpoint", &[5.0, 0.9], "");
        assert!(e.slots[0].dt[5] <= 5.0 / 64.0);
        assert_eq!(e.view().len(), 128 + 7);
    }
}
