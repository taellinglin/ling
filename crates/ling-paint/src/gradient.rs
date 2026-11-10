//! Multi-stop gradients and their shapes (linear, radial, conic, diamond, reflected, spiral, square).

#[allow(unused_imports)]
use crate::prelude::*;
use serde::{Deserialize, Serialize};

pub const GRAD_KINDS: [&str; 7] = ["Linear", "Radial", "Conic (Angle)", "Diamond", "Reflected", "Spiral", "Square"];
pub const GRAD_REPEAT: [&str; 3] = ["Clamp", "Repeat", "Mirror"];
pub const GRAD_PRESETS: [&str; 12] = [
    "FG to BG", "FG to Transparent", "Anime Sky (day)", "Anime Sky (dusk)", "Night Sky", "Sakura", "Neon", "Ocean", "Skin Shade", "Hair Shine", "Fire", "Rainbow",
];

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Gradient {
    /// (position 0..1, straight rgba)
    pub stops: Vec<(f32, [f32; 4])>,
}

impl Default for Gradient {
    fn default() -> Self {
        Gradient { stops: vec![(0.0, [0.0, 0.0, 0.0, 1.0]), (1.0, [1.0, 1.0, 1.0, 1.0])] }
    }
}

fn hex(c: u32, a: f32) -> [f32; 4] {
    [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0, a]
}

impl Gradient {
    pub fn sort(&mut self) {
        self.stops.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
    }
    /// straight rgba at t (0..1). Colour is interpolated premultiplied so fades to transparent stay clean.
    pub fn eval(&self, t: f32) -> [f32; 4] {
        let n = self.stops.len();
        if n == 0 {
            return [0.0; 4];
        }
        if t <= self.stops[0].0 || n == 1 {
            return self.stops[0].1;
        }
        if t >= self.stops[n - 1].0 {
            return self.stops[n - 1].1;
        }
        for i in 0..n - 1 {
            let (p0, c0) = self.stops[i];
            let (p1, c1) = self.stops[i + 1];
            if t >= p0 && t <= p1 {
                let k = if p1 - p0 > 1e-6 { (t - p0) / (p1 - p0) } else { 0.0 };
                let a = c0[3] + (c1[3] - c0[3]) * k;
                let mut o = [0.0, 0.0, 0.0, a];
                for j in 0..3 {
                    let v = c0[j] * c0[3] + (c1[j] * c1[3] - c0[j] * c0[3]) * k;
                    o[j] = if a > 1e-6 { v / a } else { c0[j] + (c1[j] - c0[j]) * k };
                }
                return o;
            }
        }
        self.stops[n - 1].1
    }
    pub fn preset(name: &str, fg: [f32; 4], bg: [f32; 4]) -> Gradient {
        let s = |v: &[(f32, u32)]| Gradient { stops: v.iter().map(|&(p, c)| (p, hex(c, 1.0))).collect() };
        match name {
            "FG to Transparent" => Gradient { stops: vec![(0.0, fg), (1.0, [fg[0], fg[1], fg[2], 0.0])] },
            "Anime Sky (day)" => s(&[(0.0, 0x1f5fd6), (0.45, 0x4fa3f0), (0.8, 0x9fd8ff), (1.0, 0xeaf8ff)]),
            "Anime Sky (dusk)" => s(&[(0.0, 0x2a2470), (0.35, 0x7b3fa0), (0.62, 0xf06a6a), (0.82, 0xffb36b), (1.0, 0xffe7b0)]),
            "Night Sky" => s(&[(0.0, 0x05060f), (0.5, 0x101a44), (0.85, 0x283a7a), (1.0, 0x5a4b8f)]),
            "Sakura" => s(&[(0.0, 0xffe4ef), (0.5, 0xffb3cf), (1.0, 0xf27aa5)]),
            "Neon" => s(&[(0.0, 0x00f0ff), (0.5, 0xb300ff), (1.0, 0xff2d95)]),
            "Ocean" => s(&[(0.0, 0x021b3a), (0.4, 0x065a82), (0.75, 0x1fa2b5), (1.0, 0x8fe3d6)]),
            "Skin Shade" => s(&[(0.0, 0xffe9dc), (0.55, 0xf7c9b4), (1.0, 0xd98f86)]),
            "Hair Shine" => Gradient { stops: vec![(0.0, hex(0xffffff, 0.0)), (0.45, hex(0xffffff, 0.85)), (0.55, hex(0xffffff, 0.85)), (1.0, hex(0xffffff, 0.0))] },
            "Fire" => s(&[(0.0, 0x200000), (0.35, 0xb3100b), (0.65, 0xff7a00), (0.9, 0xffd34d), (1.0, 0xfffbe0)]),
            "Rainbow" => s(&[(0.0, 0xff3b30), (0.17, 0xff9500), (0.33, 0xffcc00), (0.5, 0x34c759), (0.67, 0x0a84ff), (0.83, 0x5e5ce6), (1.0, 0xbf5af2)]),
            _ => Gradient { stops: vec![(0.0, fg), (1.0, bg)] },
        }
    }
}

/// the 0..1 gradient parameter of point (x, y) for a gradient dragged from (x0, y0) to (x1, y1)
pub fn param(kind: u8, rep: u8, x0: f32, y0: f32, x1: f32, y1: f32, x: f32, y: f32) -> f32 {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len2 = (dx * dx + dy * dy).max(1e-6);
    let len = len2.sqrt();
    let px = x - x0;
    let py = y - y0;
    let t = match kind {
        1 => (px * px + py * py).sqrt() / len,
        2 => {
            let a0 = dy.atan2(dx);
            let a = py.atan2(px);
            (a - a0).rem_euclid(core::f32::consts::TAU) / core::f32::consts::TAU
        }
        3 | 6 => {
            // rotate into the drag frame
            let ux = (px * dx + py * dy) / len;
            let uy = (-px * dy + py * dx) / len;
            if kind == 3 {
                (ux.abs() + uy.abs()) / len
            } else {
                ux.abs().max(uy.abs()) / len
            }
        }
        4 => ((px * dx + py * dy) / len2).abs(),
        5 => {
            let a0 = dy.atan2(dx);
            let a = py.atan2(px);
            let r = (px * px + py * py).sqrt() / len;
            ((a - a0).rem_euclid(core::f32::consts::TAU) / core::f32::consts::TAU + r).fract()
        }
        _ => (px * dx + py * dy) / len2,
    };
    match rep {
        1 => t.rem_euclid(1.0),
        2 => {
            let m = t.rem_euclid(2.0);
            if m > 1.0 {
                2.0 - m
            } else {
                m
            }
        }
        _ => t.clamp(0.0, 1.0),
    }
}
