//! The brush engine.
//!
//! A stroke is a stream of pen samples (position, pressure, tilt, azimuth, twist, time). Samples are smoothed
//! (stabilizer), resampled at a spacing that follows the current dab size, and every dab is computed from
//! scratch: its tip is generated procedurally (round / square / chalk / bristle / spray) and shaped by the
//! dynamics (pressure -> size / opacity / flow, tilt -> flattening and widening, azimuth / stroke direction ->
//! angle, velocity -> size, jitter, paper grain, bristle ink load). Nothing is a stamped bitmap.
//!
//! Painting modes: build-up (each dab adds), wash (the stroke has one opacity and dabs only raise a coverage
//! mask, like a marker or ink pen), erase, smudge (drags picked-up colour), blur, and water (colour pickup with
//! wet, darkened edges).

use crate::px::*;
#[allow(unused_imports)]
use crate::prelude::*;
#[cfg(not(feature = "std"))]
use alloc::collections::BTreeMap as HashMap;
use serde::{Deserialize, Serialize};
#[cfg(feature = "std")]
use std::collections::HashMap;

pub const TIP_NAMES: [&str; 5] = ["Round", "Square", "Chalk / Grain", "Bristle", "Spray / Particles"];
pub const MODE_NAMES: [&str; 5] = ["Paint", "Erase", "Smudge", "Blur", "Water"];

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Brush {
    pub name: String,
    pub tip: u8,
    pub mode: u8,
    /// diameter in canvas pixels
    pub size: f32,
    /// fraction of the size left at zero pressure
    pub size_min: f32,
    pub opacity: f32,
    pub flow: f32,
    pub hardness: f32,
    /// dab spacing as a fraction of the diameter
    pub spacing: f32,
    /// degrees
    pub angle: f32,
    pub roundness: f32,
    pub p_size: f32,
    pub p_opacity: f32,
    pub p_flow: f32,
    /// pressure curve exponent (1 linear, < 1 soft, > 1 firm)
    pub gamma: f32,
    pub tilt_angle: f32,
    pub tilt_round: f32,
    pub tilt_size: f32,
    pub dir_angle: f32,
    pub vel_size: f32,
    pub jit_size: f32,
    pub jit_angle: f32,
    pub jit_pos: f32,
    pub jit_opacity: f32,
    pub jit_hue: f32,
    pub jit_val: f32,
    pub count: f32,
    pub density: f32,
    pub grain: f32,
    pub grain_scale: f32,
    /// 1 = stroke opacity cap (wash); 0 = build-up
    pub wash: f32,
    /// colour pickup from the canvas
    pub mix: f32,
    /// smudge / pickup persistence (how far colour is dragged)
    pub smudge: f32,
    pub wet: f32,
    pub stabilize: f32,
    /// taper-in length in pixels
    pub taper: f32,
    /// bristle ink depletion
    pub dry: f32,
}

impl Default for Brush {
    fn default() -> Self {
        Brush {
            name: "Brush".into(),
            tip: 0,
            mode: 0,
            size: 12.0,
            size_min: 0.1,
            opacity: 1.0,
            flow: 1.0,
            hardness: 0.85,
            spacing: 0.08,
            angle: 0.0,
            roundness: 1.0,
            p_size: 1.0,
            p_opacity: 0.0,
            p_flow: 0.0,
            gamma: 1.0,
            tilt_angle: 0.0,
            tilt_round: 0.0,
            tilt_size: 0.0,
            dir_angle: 0.0,
            vel_size: 0.0,
            jit_size: 0.0,
            jit_angle: 0.0,
            jit_pos: 0.0,
            jit_opacity: 0.0,
            jit_hue: 0.0,
            jit_val: 0.0,
            count: 1.0,
            density: 0.5,
            grain: 0.0,
            grain_scale: 3.0,
            wash: 1.0,
            mix: 0.0,
            smudge: 0.5,
            wet: 0.0,
            stabilize: 0.15,
            taper: 0.0,
            dry: 0.0,
        }
    }
}

/// every tunable key, in the order the brush editor shows them: (key, label, min, max)
pub const BRUSH_KEYS: &[(&str, &str, f32, f32)] = &[
    ("size", "Size", 0.5, 1500.0),
    ("size_min", "Min size", 0.0, 1.0),
    ("opacity", "Opacity", 0.0, 1.0),
    ("flow", "Flow / density", 0.0, 1.0),
    ("hardness", "Hardness", 0.0, 1.0),
    ("spacing", "Spacing", 0.01, 2.0),
    ("angle", "Angle", -180.0, 180.0),
    ("roundness", "Roundness", 0.03, 1.0),
    ("p_size", "Pressure > size", 0.0, 1.0),
    ("p_opacity", "Pressure > opacity", 0.0, 1.0),
    ("p_flow", "Pressure > flow", 0.0, 1.0),
    ("gamma", "Pressure curve", 0.25, 4.0),
    ("tilt_angle", "Tilt > angle", 0.0, 1.0),
    ("tilt_round", "Tilt > flatten", 0.0, 1.0),
    ("tilt_size", "Tilt > widen", 0.0, 2.0),
    ("dir_angle", "Follow direction", 0.0, 1.0),
    ("vel_size", "Speed > size", -1.0, 1.0),
    ("jit_size", "Size jitter", 0.0, 1.0),
    ("jit_angle", "Angle jitter", 0.0, 1.0),
    ("jit_pos", "Scatter", 0.0, 4.0),
    ("jit_opacity", "Opacity jitter", 0.0, 1.0),
    ("jit_hue", "Hue jitter", 0.0, 1.0),
    ("jit_val", "Value jitter", 0.0, 1.0),
    ("count", "Dabs per step", 1.0, 16.0),
    ("density", "Particles / bristles", 0.0, 1.0),
    ("grain", "Paper grain", 0.0, 1.0),
    ("grain_scale", "Grain scale", 0.5, 20.0),
    ("wash", "Stroke opacity (wash)", 0.0, 1.0),
    ("mix", "Colour pickup", 0.0, 1.0),
    ("smudge", "Pickup length", 0.0, 1.0),
    ("wet", "Wet edges", 0.0, 1.0),
    ("stabilize", "Stabilizer", 0.0, 1.0),
    ("taper", "Taper in (px)", 0.0, 400.0),
    ("dry", "Dry brush", 0.0, 1.0),
];

impl Brush {
    pub fn get(&self, k: &str) -> f32 {
        match k {
            "tip" => self.tip as f32,
            "mode" => self.mode as f32,
            "size" => self.size,
            "size_min" => self.size_min,
            "opacity" => self.opacity,
            "flow" => self.flow,
            "hardness" => self.hardness,
            "spacing" => self.spacing,
            "angle" => self.angle,
            "roundness" => self.roundness,
            "p_size" => self.p_size,
            "p_opacity" => self.p_opacity,
            "p_flow" => self.p_flow,
            "gamma" => self.gamma,
            "tilt_angle" => self.tilt_angle,
            "tilt_round" => self.tilt_round,
            "tilt_size" => self.tilt_size,
            "dir_angle" => self.dir_angle,
            "vel_size" => self.vel_size,
            "jit_size" => self.jit_size,
            "jit_angle" => self.jit_angle,
            "jit_pos" => self.jit_pos,
            "jit_opacity" => self.jit_opacity,
            "jit_hue" => self.jit_hue,
            "jit_val" => self.jit_val,
            "count" => self.count,
            "density" => self.density,
            "grain" => self.grain,
            "grain_scale" => self.grain_scale,
            "wash" => self.wash,
            "mix" => self.mix,
            "smudge" => self.smudge,
            "wet" => self.wet,
            "stabilize" => self.stabilize,
            "taper" => self.taper,
            "dry" => self.dry,
            _ => 0.0,
        }
    }
    pub fn set(&mut self, k: &str, v: f32) {
        let lim = BRUSH_KEYS.iter().find(|e| e.0 == k).map(|e| (e.2, e.3));
        let v = match lim {
            Some((lo, hi)) => v.clamp(lo, hi),
            None => v,
        };
        match k {
            "tip" => self.tip = (v.max(0.0) as u8).min(4),
            "mode" => self.mode = (v.max(0.0) as u8).min(4),
            "size" => self.size = v,
            "size_min" => self.size_min = v,
            "opacity" => self.opacity = v,
            "flow" => self.flow = v,
            "hardness" => self.hardness = v,
            "spacing" => self.spacing = v,
            "angle" => self.angle = v,
            "roundness" => self.roundness = v,
            "p_size" => self.p_size = v,
            "p_opacity" => self.p_opacity = v,
            "p_flow" => self.p_flow = v,
            "gamma" => self.gamma = v,
            "tilt_angle" => self.tilt_angle = v,
            "tilt_round" => self.tilt_round = v,
            "tilt_size" => self.tilt_size = v,
            "dir_angle" => self.dir_angle = v,
            "vel_size" => self.vel_size = v,
            "jit_size" => self.jit_size = v,
            "jit_angle" => self.jit_angle = v,
            "jit_pos" => self.jit_pos = v,
            "jit_opacity" => self.jit_opacity = v,
            "jit_hue" => self.jit_hue = v,
            "jit_val" => self.jit_val = v,
            "count" => self.count = v,
            "density" => self.density = v,
            "grain" => self.grain = v,
            "grain_scale" => self.grain_scale = v,
            "wash" => self.wash = v,
            "mix" => self.mix = v,
            "smudge" => self.smudge = v,
            "wet" => self.wet = v,
            "stabilize" => self.stabilize = v,
            "taper" => self.taper = v,
            "dry" => self.dry = v,
            _ => {}
        }
    }
}

/// the built-in presets, tuned for anime / manga line work and painting
pub fn presets() -> Vec<Brush> {
    let mk = |name: &str, f: &dyn Fn(&mut Brush)| {
        let mut b = Brush { name: name.into(), ..Default::default() };
        f(&mut b);
        b
    };
    vec![
        mk("G-Pen (ink)", &|b| {
            b.size = 9.0;
            b.size_min = 0.05;
            b.hardness = 0.95;
            b.spacing = 0.05;
            b.gamma = 1.4;
            b.stabilize = 0.35;
            b.taper = 30.0;
        }),
        mk("Mapping Pen", &|b| {
            b.size = 4.0;
            b.size_min = 0.15;
            b.hardness = 1.0;
            b.spacing = 0.05;
            b.stabilize = 0.45;
            b.taper = 20.0;
        }),
        mk("Maru Pen (fine)", &|b| {
            b.size = 2.5;
            b.size_min = 0.3;
            b.hardness = 1.0;
            b.spacing = 0.06;
            b.stabilize = 0.5;
        }),
        mk("Pencil HB", &|b| {
            b.size = 5.0;
            b.size_min = 0.6;
            b.tip = 2;
            b.hardness = 0.7;
            b.p_size = 0.4;
            b.p_opacity = 0.9;
            b.opacity = 0.85;
            b.wash = 0.0;
            b.flow = 0.55;
            b.grain = 0.75;
            b.grain_scale = 1.6;
            b.tilt_size = 1.2;
            b.tilt_round = 0.6;
            b.tilt_angle = 1.0;
            b.spacing = 0.12;
            b.stabilize = 0.1;
        }),
        mk("Charcoal", &|b| {
            b.size = 34.0;
            b.tip = 2;
            b.hardness = 0.5;
            b.p_size = 0.5;
            b.p_flow = 1.0;
            b.wash = 0.0;
            b.flow = 0.5;
            b.grain = 1.0;
            b.grain_scale = 4.0;
            b.roundness = 0.55;
            b.tilt_angle = 1.0;
            b.tilt_size = 1.5;
            b.jit_angle = 0.15;
            b.spacing = 0.1;
        }),
        mk("Soft Airbrush", &|b| {
            b.size = 120.0;
            b.hardness = 0.0;
            b.p_size = 0.0;
            b.p_flow = 1.0;
            b.flow = 0.08;
            b.wash = 0.0;
            b.spacing = 0.06;
            b.stabilize = 0.0;
        }),
        mk("Hard Airbrush", &|b| {
            b.size = 60.0;
            b.hardness = 0.55;
            b.p_size = 0.2;
            b.p_flow = 1.0;
            b.flow = 0.18;
            b.wash = 0.0;
            b.spacing = 0.06;
        }),
        mk("Cel Fill Brush", &|b| {
            b.size = 40.0;
            b.size_min = 0.6;
            b.hardness = 1.0;
            b.p_size = 0.3;
            b.spacing = 0.06;
            b.stabilize = 0.05;
        }),
        mk("Watercolor (wet)", &|b| {
            b.size = 70.0;
            b.mode = 4;
            b.hardness = 0.4;
            b.p_size = 0.5;
            b.p_flow = 0.8;
            b.flow = 0.22;
            b.opacity = 0.6;
            b.wash = 0.0;
            b.mix = 0.45;
            b.smudge = 0.7;
            b.wet = 0.8;
            b.grain = 0.35;
            b.grain_scale = 6.0;
            b.jit_size = 0.15;
            b.spacing = 0.08;
        }),
        mk("Gouache / Oil (bristle)", &|b| {
            b.size = 48.0;
            b.tip = 3;
            b.density = 0.6;
            b.hardness = 0.8;
            b.p_size = 0.4;
            b.p_flow = 0.6;
            b.wash = 0.0;
            b.flow = 0.8;
            b.mix = 0.55;
            b.smudge = 0.75;
            b.dir_angle = 1.0;
            b.roundness = 0.55;
            b.tilt_size = 0.8;
            b.spacing = 0.05;
        }),
        mk("Dry Brush", &|b| {
            b.size = 42.0;
            b.tip = 3;
            b.density = 0.9;
            b.hardness = 0.9;
            b.p_size = 0.5;
            b.wash = 0.0;
            b.flow = 0.9;
            b.dry = 0.7;
            b.dir_angle = 1.0;
            b.roundness = 0.35;
            b.grain = 0.5;
            b.spacing = 0.04;
        }),
        mk("Hair Strands", &|b| {
            b.size = 26.0;
            b.tip = 3;
            b.density = 0.35;
            b.size_min = 0.0;
            b.hardness = 1.0;
            b.p_size = 1.0;
            b.gamma = 1.3;
            b.dir_angle = 1.0;
            b.roundness = 0.4;
            b.wash = 0.0;
            b.taper = 60.0;
            b.spacing = 0.03;
            b.stabilize = 0.3;
        }),
        mk("Marker (flat)", &|b| {
            b.size = 22.0;
            b.tip = 1;
            b.hardness = 0.75;
            b.roundness = 0.35;
            b.angle = 35.0;
            b.p_size = 0.15;
            b.opacity = 0.55;
            b.wash = 1.0;
            b.tilt_angle = 0.6;
            b.spacing = 0.05;
        }),
        mk("Calligraphy Flat", &|b| {
            b.size = 20.0;
            b.hardness = 0.95;
            b.roundness = 0.12;
            b.angle = 40.0;
            b.p_size = 0.6;
            b.tilt_angle = 1.0;
            b.tilt_round = 0.3;
            b.spacing = 0.03;
            b.stabilize = 0.3;
        }),
        mk("Sparkle Spray", &|b| {
            b.size = 90.0;
            b.tip = 4;
            b.density = 0.25;
            b.p_size = 0.3;
            b.p_flow = 0.7;
            b.wash = 0.0;
            b.hardness = 0.8;
            b.jit_hue = 0.12;
            b.jit_val = 0.3;
            b.jit_size = 0.6;
            b.spacing = 0.35;
            b.stabilize = 0.0;
        }),
        mk("Cloud / Foliage", &|b| {
            b.size = 110.0;
            b.tip = 0;
            b.hardness = 0.35;
            b.wash = 0.0;
            b.flow = 0.35;
            b.p_flow = 0.6;
            b.jit_size = 0.7;
            b.jit_pos = 0.6;
            b.count = 3.0;
            b.jit_val = 0.08;
            b.grain = 0.3;
            b.grain_scale = 8.0;
            b.spacing = 0.25;
            b.stabilize = 0.0;
        }),
        mk("Blender (smudge)", &|b| {
            b.size = 50.0;
            b.mode = 2;
            b.hardness = 0.3;
            b.p_flow = 1.0;
            b.flow = 0.6;
            b.smudge = 0.85;
            b.wash = 0.0;
            b.spacing = 0.06;
        }),
        mk("Blur", &|b| {
            b.size = 60.0;
            b.mode = 3;
            b.hardness = 0.2;
            b.flow = 0.6;
            b.wash = 0.0;
            b.spacing = 0.1;
        }),
        mk("Hard Eraser", &|b| {
            b.size = 24.0;
            b.mode = 1;
            b.hardness = 0.95;
            b.p_size = 0.3;
            b.wash = 1.0;
            b.spacing = 0.06;
        }),
        mk("Soft Eraser", &|b| {
            b.size = 90.0;
            b.mode = 1;
            b.hardness = 0.0;
            b.p_size = 0.0;
            b.p_flow = 1.0;
            b.flow = 0.25;
            b.wash = 0.0;
            b.spacing = 0.08;
        }),
    ]
}

/// one pen sample in canvas pixels. tilt 0 (upright) .. 1 (flat), azimuth / twist in radians
#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub x: f32,
    pub y: f32,
    pub p: f32,
    pub tilt: f32,
    pub az: f32,
    pub twist: f32,
    pub t: f32,
}
impl Sample {
    fn lerp(&self, o: &Sample, k: f32) -> Sample {
        Sample {
            x: self.x + (o.x - self.x) * k,
            y: self.y + (o.y - self.y) * k,
            p: self.p + (o.p - self.p) * k,
            tilt: self.tilt + (o.tilt - self.tilt) * k,
            az: self.az + (o.az - self.az) * k,
            twist: self.twist + (o.twist - self.twist) * k,
            t: self.t + (o.t - self.t) * k,
        }
    }
}

/// what a stroke paints into
pub struct Ctx<'a> {
    pub tiles: &'a mut Tiles,
    /// the layer as it was when the stroke began (wash mode and blur read it)
    pub orig: &'a Tiles,
    pub sel: Option<&'a Mask>,
    pub alpha_lock: bool,
}

#[derive(Clone, Copy)]
struct Dab {
    x: f32,
    y: f32,
    r: f32,
    ang: f32,
    round: f32,
    hard: f32,
    /// coverage multiplier (flow, plus opacity in build-up)
    a: f32,
    /// stroke opacity (wash)
    opacity: f32,
    pr: f32,
    color: [f32; 3],
}

pub struct Stroke {
    pub b: Brush,
    pub color: [f32; 3],
    pub erase: bool,
    last: Option<Sample>,
    acc: f32,
    step: f32,
    hist: Vec<Sample>,
    raw_prev: Option<Sample>,
    vel: f32,
    pub travelled: f32,
    mask: HashMap<usize, Vec<f32>>,
    carried: Option<[f32; 4]>,
    bristles: Vec<[f32; 4]>,
    rng: Rng,
    dir: f32,
    pub dirty: [i32; 4],
    pub dabs: u32,
}

impl Stroke {
    pub fn new(b: Brush, color: [f32; 3], erase: bool, seed: u64) -> Stroke {
        let mut rng = Rng(seed | 1);
        let mut bristles = Vec::new();
        if b.tip == 3 {
            let n = (8.0 + b.density * 48.0) as usize;
            let rb = 0.32 / (n as f32 / 8.0).sqrt();
            for _ in 0..n {
                let a = rng.next() * core::f32::consts::TAU;
                let r = rng.next().sqrt() * (1.0 - rb * 0.5);
                bristles.push([a.cos() * r, a.sin() * r, rb * (0.6 + rng.next() * 0.8), 0.75 + rng.next() * 0.25]);
            }
        }
        Stroke {
            b,
            color,
            erase,
            last: None,
            acc: 0.0,
            step: 1.0,
            hist: Vec::new(),
            raw_prev: None,
            vel: 0.0,
            travelled: 0.0,
            mask: HashMap::new(),
            carried: None,
            bristles,
            rng,
            dir: 0.0,
            dirty: [i32::MAX, i32::MAX, i32::MIN, i32::MIN],
            dabs: 0,
        }
    }

    fn window(&self) -> usize {
        1 + (self.b.stabilize * self.b.stabilize * 40.0) as usize
    }

    /// feed one raw pen sample
    pub fn add(&mut self, ctx: &mut Ctx, s: Sample) {
        if let Some(pv) = self.raw_prev {
            let dt = (s.t - pv.t).max(0.0);
            if dt > 1e-4 {
                let v = ((s.x - pv.x).powi(2) + (s.y - pv.y).powi(2)).sqrt() / dt;
                self.vel = self.vel * 0.75 + v.min(20000.0) * 0.25;
            }
        }
        self.raw_prev = Some(s);
        self.hist.push(s);
        let n = self.window();
        if self.hist.len() > n {
            let k = self.hist.len() - n;
            self.hist.drain(0..k);
        }
        let sm = self.smoothed();
        self.advance(ctx, sm);
    }

    fn smoothed(&self) -> Sample {
        let n = self.hist.len() as f32;
        let mut o = Sample::default();
        // pressure and angles follow the newest sample, position is averaged (weighted towards the newest)
        let mut wsum = 0.0;
        for (i, h) in self.hist.iter().enumerate() {
            let w = 1.0 + i as f32 / n.max(1.0);
            o.x += h.x * w;
            o.y += h.y * w;
            o.p += h.p * w;
            wsum += w;
        }
        o.x /= wsum;
        o.y /= wsum;
        o.p /= wsum;
        let l = self.hist.last().copied().unwrap_or_default();
        o.tilt = l.tilt;
        o.az = l.az;
        o.twist = l.twist;
        o.t = l.t;
        o
    }

    /// pull the stabilizer tail onto the last real sample
    pub fn finish(&mut self, ctx: &mut Ctx) {
        if let Some(l) = self.hist.last().copied() {
            let n = self.window();
            for _ in 0..n {
                self.hist.push(l);
                if self.hist.len() > n {
                    self.hist.remove(0);
                }
                let sm = self.smoothed();
                self.advance(ctx, sm);
            }
        }
        if self.dabs == 0 {
            if let Some(l) = self.last {
                self.emit(ctx, l);
            }
        }
    }

    fn advance(&mut self, ctx: &mut Ctx, s: Sample) {
        let last = match self.last {
            None => {
                self.last = Some(s);
                self.emit(ctx, s);
                return;
            }
            Some(l) => l,
        };
        let dx = s.x - last.x;
        let dy = s.y - last.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-3 {
            return;
        }
        let nd = dy.atan2(dx);
        // unwrap the direction so it eases instead of flipping
        let mut dd = nd - self.dir;
        while dd > core::f32::consts::PI {
            dd -= core::f32::consts::TAU;
        }
        while dd < -core::f32::consts::PI {
            dd += core::f32::consts::TAU;
        }
        self.dir += dd * if self.dabs < 2 { 1.0 } else { 0.5 };
        let mut d = 0.0;
        let mut guard = 0;
        while self.acc + (len - d) >= self.step && guard < 20000 {
            d += self.step - self.acc;
            self.acc = 0.0;
            let k = (d / len).clamp(0.0, 1.0);
            let ps = last.lerp(&s, k);
            self.travelled += self.step;
            self.emit(ctx, ps);
            guard += 1;
        }
        self.acc += len - d;
        self.last = Some(s);
    }

    fn emit(&mut self, ctx: &mut Ctx, s: Sample) {
        let b = self.b.clone();
        let pr = s.p.clamp(0.0, 1.0).powf(b.gamma.max(0.05));
        let mut size = b.size * (1.0 + (b.size_min + (1.0 - b.size_min) * pr - 1.0) * b.p_size);
        size *= 1.0 + b.tilt_size * s.tilt * 1.5;
        let vn = (self.vel / 1800.0).min(1.0);
        size *= (1.0 + b.vel_size * vn).max(0.05);
        if b.taper > 0.0 && self.travelled < b.taper {
            size *= 0.12 + 0.88 * (self.travelled / b.taper);
        }
        let opacity = b.opacity * (1.0 + (pr - 1.0) * b.p_opacity);
        let flow = b.flow * (1.0 + (pr - 1.0) * b.p_flow);
        let ang = b.angle.to_radians() + b.dir_angle * self.dir + b.tilt_angle * s.az + s.twist;
        let round = (b.roundness * (1.0 - b.tilt_round * s.tilt * 0.85)).clamp(0.03, 1.0);
        self.step = (b.spacing * size).max(0.35);
        let count = b.count.round().max(1.0) as u32;
        for _ in 0..count {
            let mut sz = size * (1.0 + b.jit_size * self.rng.sym() * 0.9).max(0.05);
            let a2 = ang + b.jit_angle * core::f32::consts::PI * self.rng.sym();
            let mut x = s.x;
            let mut y = s.y;
            if b.jit_pos > 0.0 {
                x += self.rng.gauss() * b.jit_pos * size;
                y += self.rng.gauss() * b.jit_pos * size;
            }
            let mut a = flow * (1.0 - b.jit_opacity * self.rng.next());
            let wash = b.wash > 0.5 && b.mode < 2;
            if !wash {
                a *= opacity;
            }
            let mut col = self.color;
            if b.jit_hue > 0.0 || b.jit_val > 0.0 {
                let mut hsv = rgb2hsv(col);
                hsv[0] += b.jit_hue * self.rng.sym() * 0.5;
                hsv[2] = (hsv[2] + b.jit_val * self.rng.sym() * 0.5).clamp(0.0, 1.0);
                if b.jit_hue > 0.0 {
                    hsv[1] = hsv[1].max(0.25);
                }
                col = hsv2rgb(hsv);
            }
            if sz < 0.5 {
                // keep sub-pixel dabs visible: shrink their alpha instead of their radius
                a *= sz / 0.5;
                sz = 0.5;
            }
            let dab = Dab { x, y, r: sz * 0.5, ang: a2, round, hard: b.hardness, a, opacity, pr, color: col };
            self.dab(ctx, dab);
        }
    }

    fn sample_under(&self, ctx: &Ctx, d: &Dab) -> [f32; 4] {
        let r = d.r * 0.45;
        let pts = [(0.0, 0.0), (r, 0.0), (-r, 0.0), (0.0, r), (0.0, -r)];
        let mut acc = [0.0f32; 4];
        for (ox, oy) in pts {
            let c = ctx.tiles.getf((d.x + ox) as i32, (d.y + oy) as i32);
            for k in 0..4 {
                acc[k] += c[k];
            }
        }
        for v in acc.iter_mut() {
            *v /= 5.0;
        }
        acc
    }

    fn dab(&mut self, ctx: &mut Ctx, d: Dab) {
        self.dabs += 1;
        let b = self.b.clone();
        // spray: a cloud of tiny round dabs
        if b.tip == 4 {
            let n = (1.0 + b.density * 40.0 * (d.r / 15.0).clamp(0.3, 6.0)) as usize;
            for _ in 0..n {
                let ang = self.rng.next() * core::f32::consts::TAU;
                let rr = d.r * self.rng.next().sqrt();
                let pr = (d.r * 0.035 * (0.4 + self.rng.next() * 1.2)).max(0.6);
                let mut m = d;
                m.x = d.x + ang.cos() * rr;
                m.y = d.y + ang.sin() * rr * d.round;
                m.r = pr;
                m.round = 1.0;
                m.hard = 0.7;
                self.paint_dab(ctx, &m, 0, &b);
            }
            return;
        }
        // colour pickup (mix / water) and smudge carry
        if b.mode == 2 || b.mix > 0.0 || b.mode == 4 {
            let under = self.sample_under(ctx, &d);
            let keep = b.smudge.clamp(0.0, 0.98);
            self.carried = Some(match self.carried {
                None => under,
                Some(c) => lerp4(under, c, keep),
            });
        }
        if b.dry > 0.0 && !self.bristles.is_empty() {
            let step = self.step / (d.r * 2.0).max(1.0);
            for br in self.bristles.iter_mut() {
                br[3] = (br[3] - b.dry * step * 0.035 * (0.5 + self.rng.next())).max(0.0);
            }
        }
        self.paint_dab(ctx, &d, b.tip, &b);
    }

    fn paint_dab(&mut self, ctx: &mut Ctx, d: &Dab, tip: u8, b: &Brush) {
        let rx = d.r + 1.5;
        let x0 = (d.x - rx).floor() as i32;
        let y0 = (d.y - rx).floor() as i32;
        let x1 = (d.x + rx).ceil() as i32 + 1;
        let y1 = (d.y + rx).ceil() as i32 + 1;
        if x1 <= 0 || y1 <= 0 || x0 >= ctx.tiles.w as i32 || y0 >= ctx.tiles.h as i32 {
            return;
        }
        self.dirty = [self.dirty[0].min(x0), self.dirty[1].min(y0), self.dirty[2].max(x1), self.dirty[3].max(y1)];
        let (sa, ca) = (-d.ang).sin_cos();
        let inv_r = 1.0 / d.r.max(0.25);
        let inv_rr = 1.0 / (d.r * d.round).max(0.25);
        let edge = (1.0 - d.hard).max(1.2 / d.r.max(0.5));
        let mode = if self.erase { 1 } else { b.mode };
        let wash = b.wash > 0.5 && mode < 2;
        let grain_amt = b.grain;
        let gscale = b.grain_scale;
        let wet = if mode == 4 { b.wet } else { 0.0 };
        let bristles = &self.bristles;
        let spread = 0.65 + 0.35 * d.pr;
        // the colour this dab lays down (straight)
        let mut col = d.color;
        if let Some(c) = self.carried {
            if (b.mix > 0.0 || mode == 4) && c[3] > 0.01 {
                let u = unpre(c);
                let k = (if mode == 4 { b.mix.max(0.3) } else { b.mix }) * c[3].min(1.0);
                for j in 0..3 {
                    col[j] += (u[j] - col[j]) * k;
                }
            }
        }
        let carried = self.carried.unwrap_or([0.0; 4]);
        // blur reads a copy of the area so the kernel does not feed on its own output
        let blur_src: Option<(i32, i32, usize, Vec<[f32; 4]>)> = if mode == 3 {
            let w = (x1 - x0 + 2) as usize;
            let h = (y1 - y0 + 2) as usize;
            let mut v = vec![[0.0f32; 4]; w * h];
            for yy in 0..h {
                for xx in 0..w {
                    v[yy * w + xx] = ctx.tiles.getf(x0 - 1 + xx as i32, y0 - 1 + yy as i32);
                }
            }
            Some((x0 - 1, y0 - 1, w, v))
        } else {
            None
        };
        let tw = ctx.tiles.tw;
        let mask = &mut self.mask;
        let sel = ctx.sel;
        let orig = ctx.orig;
        let alpha_lock = ctx.alpha_lock;
        ctx.tiles.for_rect(x0, y0, x1, y1, |x, y, p| {
            let fx = x as f32 + 0.5 - d.x;
            let fy = y as f32 + 0.5 - d.y;
            let lx = (fx * ca - fy * sa) * inv_r;
            let ly = (fx * sa + fy * ca) * inv_rr;
            let mut cov = match tip {
                1 => {
                    let dd = lx.abs().max(ly.abs());
                    smooth01((1.0 - dd) / edge)
                }
                3 => {
                    let mut m = 0.0f32;
                    for br in bristles.iter() {
                        let bx = lx - br[0] * spread;
                        let by = ly - br[1] * spread;
                        let dd = (bx * bx + by * by).sqrt() / br[2];
                        if dd < 1.0 {
                            let e = (1.2 / (d.r * br[2]).max(0.5)).max(1.0 - d.hard);
                            m = m.max(smooth01((1.0 - dd) / e) * br[3]);
                        }
                    }
                    m
                }
                _ => {
                    let dd = (lx * lx + ly * ly).sqrt();
                    if dd >= 1.0 {
                        0.0
                    } else {
                        let mut c = smooth01((1.0 - dd) / edge);
                        if wet > 0.0 {
                            c *= 1.0 - wet + wet * (0.35 + 0.9 * smoothstep(0.55, 0.97, dd));
                        }
                        c
                    }
                }
            };
            if cov <= 0.0 {
                return;
            }
            if grain_amt > 0.0 || tip == 2 {
                let g = grain(x as f32, y as f32, gscale);
                let amt = if tip == 2 { grain_amt.max(0.6) } else { grain_amt };
                cov *= 1.0 - amt * (1.0 - smoothstep(0.3, 0.7, g + (d.pr - 0.5) * 0.7));
            }
            if let Some(s) = sel {
                cov *= s.at(x, y);
            }
            if cov <= 0.0 {
                return;
            }
            let dst = tof(*p);
            let out = match mode {
                1 => {
                    if wash {
                        let ti = (y as usize / TS) * tw + x as usize / TS;
                        let mi = (y as usize % TS) * TS + x as usize % TS;
                        let m = mask.entry(ti).or_insert_with(|| vec![0.0; TN]);
                        let mv = m[mi].max(cov * d.a);
                        m[mi] = mv;
                        let o = orig.getf(x, y);
                        let k = 1.0 - mv * d.opacity;
                        [o[0] * k, o[1] * k, o[2] * k, o[3] * k]
                    } else {
                        let k = 1.0 - cov * d.a;
                        [dst[0] * k, dst[1] * k, dst[2] * k, dst[3] * k]
                    }
                }
                2 => lerp4(dst, carried, cov * d.a),
                3 => {
                    let (bx0, by0, bw, ref v) = *blur_src.as_ref().unwrap();
                    let cx = (x - bx0) as usize;
                    let cy = (y - by0) as usize;
                    let mut acc = [0.0f32; 4];
                    for oy in 0..3usize {
                        for ox in 0..3usize {
                            let s = v[(cy + oy - 1) * bw + cx + ox - 1];
                            for k in 0..4 {
                                acc[k] += s[k];
                            }
                        }
                    }
                    for a in acc.iter_mut() {
                        *a /= 9.0;
                    }
                    lerp4(dst, acc, cov * d.a)
                }
                _ => {
                    let (base, a) = if wash {
                        let ti = (y as usize / TS) * tw + x as usize / TS;
                        let mi = (y as usize % TS) * TS + x as usize % TS;
                        let m = mask.entry(ti).or_insert_with(|| vec![0.0; TN]);
                        let mv = m[mi].max(cov * d.a);
                        m[mi] = mv;
                        (orig.getf(x, y), mv * d.opacity)
                    } else {
                        (dst, cov * d.a)
                    };
                    let src = [col[0] * a, col[1] * a, col[2] * a, a];
                    if alpha_lock {
                        let ba = base[3];
                        let mixd = [base[0] + (col[0] * ba - base[0]) * a, base[1] + (col[1] * ba - base[1]) * a, base[2] + (col[2] * ba - base[2]) * a, ba];
                        mixd
                    } else {
                        over(base, src)
                    }
                }
            };
            if alpha_lock && mode != 0 && mode != 4 {
                // keep the alpha, rescale colour to it
                let na = dst[3];
                let u = unpre(out);
                *p = fromf([u[0] * na, u[1] * na, u[2] * na, na]);
            } else {
                *p = fromf(out);
            }
        });
    }
}

/// draw a sample S-curve with rising-then-falling pressure into a small image (the brush preview)
pub fn preview(b: &Brush, w: usize, h: usize, color: [f32; 3]) -> Tiles {
    let mut tiles = Tiles::new(w, h);
    let orig = Tiles::new(w, h);
    let mut bb = b.clone();
    let fit = (h as f32 * 0.55).max(2.0);
    if bb.size > fit {
        let k = fit / bb.size;
        bb.size = fit;
        bb.taper *= k;
    }
    bb.stabilize = 0.0;
    let mut st = Stroke::new(bb, color, false, 7);
    let n = 90;
    {
        let mut ctx = Ctx { tiles: &mut tiles, orig: &orig, sel: None, alpha_lock: false };
        for i in 0..=n {
            let t = i as f32 / n as f32;
            let x = w as f32 * (0.08 + 0.84 * t);
            let y = h as f32 * (0.5 + 0.28 * (t * core::f32::consts::TAU).sin());
            let p = (t * core::f32::consts::PI).sin().powf(0.8);
            let s = Sample { x, y, p, tilt: 0.3 + 0.4 * t, az: 0.7, twist: 0.0, t: t * 0.6 };
            st.add(&mut ctx, s);
        }
        st.finish(&mut ctx);
    }
    tiles
}
