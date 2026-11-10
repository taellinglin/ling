//! 2.5D scenes: picture parts (from PNGs or layers) on subdivided meshes, skinned to a 2D bone rig, placed in
//! depth and seen through an orthographic, perspective, fisheye or hyperbolic (Poincare disk) camera; keyframe
//! tracks animate every number. Rendering is a band-parallel textured-triangle rasteriser.

#[allow(unused_imports)]
use crate::prelude::*;
use crate::px::*;
#[cfg(not(feature = "std"))]
use alloc::sync::Arc;
#[cfg(feature = "std")]
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
#[cfg(feature = "std")]
use std::sync::Arc;

pub const PROJ_NAMES: [&str; 4] = ["Orthographic", "Perspective", "Fisheye", "Hyperbolic (Poincare)"];
pub const EASE_NAMES: [&str; 10] = ["Linear", "Hold (step)", "Smooth", "Ease In", "Ease Out", "Back (overshoot)", "Elastic", "Bounce", "Sine In-Out", "Expo In-Out"];
pub const PART_PROPS: [&str; 10] = ["x", "y", "z", "rx", "ry", "rz", "sx", "sy", "opacity", "visible"];
pub const BONE_PROPS: [&str; 5] = ["rot", "tx", "ty", "sc", "sway"];
pub const CAM_PROPS: [&str; 11] = ["x", "y", "dist", "yaw", "pitch", "roll", "fov", "strength", "curv", "zoom", "fog"];

/// premultiplied RGBA8 picture
#[derive(Clone, Default)]
pub struct Img {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[u8; 4]>,
}
impl Img {
    pub fn from_straight(w: usize, h: usize, rgba: &[u8]) -> Img {
        let mut px = Vec::with_capacity(w * h);
        for i in 0..w * h {
            let a = rgba[i * 4 + 3] as u32;
            let m = |v: u8| ((v as u32 * a + 127) / 255) as u8;
            px.push([m(rgba[i * 4]), m(rgba[i * 4 + 1]), m(rgba[i * 4 + 2]), a as u8]);
        }
        Img { w, h, px }
    }
    pub fn to_straight(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.w * self.h * 4);
        for p in &self.px {
            let a = p[3] as u32;
            let u = |v: u8| if a == 0 { 0 } else { ((v as u32 * 255 + a / 2) / a).min(255) as u8 };
            out.extend_from_slice(&[u(p[0]), u(p[1]), u(p[2]), p[3]]);
        }
        out
    }
    #[inline(always)]
    fn at(&self, x: i32, y: i32) -> [f32; 4] {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return [0.0; 4];
        }
        let p = self.px[y as usize * self.w + x as usize];
        const K: f32 = 1.0 / 255.0;
        [p[0] as f32 * K, p[1] as f32 * K, p[2] as f32 * K, p[3] as f32 * K]
    }
    #[inline(always)]
    pub fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let fx = u * self.w as f32 - 0.5;
        let fy = v * self.h as f32 - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = fx - x0;
        let ty = fy - y0;
        let (xi, yi) = (x0 as i32, y0 as i32);
        lerp4(lerp4(self.at(xi, yi), self.at(xi + 1, yi), tx), lerp4(self.at(xi, yi + 1), self.at(xi + 1, yi + 1), tx), ty)
    }
    /// is any pixel of the cell [u0,u1)x[v0,v1) visible
    fn occupied(&self, u0: f32, v0: f32, u1: f32, v1: f32) -> bool {
        let x0 = ((u0 * self.w as f32).floor() as i32 - 1).max(0) as usize;
        let y0 = ((v0 * self.h as f32).floor() as i32 - 1).max(0) as usize;
        let x1 = ((u1 * self.w as f32).ceil() as usize + 1).min(self.w);
        let y1 = ((v1 * self.h as f32).ceil() as usize + 1).min(self.h);
        for y in y0..y1 {
            for x in x0..x1 {
                if self.px[y * self.w + x][3] > 2 {
                    return true;
                }
            }
        }
        false
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Part {
    pub name: String,
    #[serde(skip)]
    pub img: Arc<Img>,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub rx: f32,
    pub ry: f32,
    pub rz: f32,
    pub sx: f32,
    pub sy: f32,
    /// pivot in image pixels
    pub px: f32,
    pub py: f32,
    pub opacity: f32,
    pub visible: f32,
    pub cols: u32,
    pub rows: u32,
    pub bones: Vec<i32>,
    pub rigid: i32,
    /// per mesh vertex: two (bone, weight) influences
    pub weights: Vec<[(i32, f32); 2]>,
    #[serde(skip)]
    pub occ: Vec<bool>,
}
impl Default for Part {
    fn default() -> Self {
        Part {
            name: "Part".into(),
            img: Arc::new(Img::default()),
            x: 0.0,
            y: 0.0,
            z: 0.0,
            rx: 0.0,
            ry: 0.0,
            rz: 0.0,
            sx: 1.0,
            sy: 1.0,
            px: 0.0,
            py: 0.0,
            opacity: 1.0,
            visible: 1.0,
            cols: 10,
            rows: 10,
            bones: vec![],
            rigid: -1,
            weights: vec![],
            occ: vec![],
        }
    }
}
impl Part {
    pub fn new(name: &str, img: Img, cx: f32, cy: f32) -> Part {
        let (w, h) = (img.w as f32, img.h as f32);
        // mesh density follows the picture size (about one cell per 48 px, 4..24)
        let cols = ((w / 48.0).round() as u32).clamp(4, 24);
        let rows = ((h / 48.0).round() as u32).clamp(4, 24);
        let mut p = Part { name: name.into(), img: Arc::new(img), x: cx, y: cy, px: w * 0.5, py: h * 0.5, cols, rows, ..Default::default() };
        p.rebuild_occ();
        p
    }
    pub fn rebuild_occ(&mut self) {
        let (c, r) = (self.cols.max(1), self.rows.max(1));
        let mut occ = Vec::with_capacity((c * r) as usize);
        for gy in 0..r {
            for gx in 0..c {
                occ.push(self.img.occupied(gx as f32 / c as f32, gy as f32 / r as f32, (gx + 1) as f32 / c as f32, (gy + 1) as f32 / r as f32));
            }
        }
        self.occ = occ;
    }
    /// rest position of mesh vertex (gx, gy) in world XY
    fn rest_vertex(&self, gx: u32, gy: u32) -> (f32, f32) {
        let ix = gx as f32 / self.cols as f32 * self.img.w as f32;
        let iy = gy as f32 / self.rows as f32 * self.img.h as f32;
        let lx = (ix - self.px) * self.sx;
        let ly = (iy - self.py) * self.sy;
        let (s, c) = self.rz.to_radians().sin_cos();
        (self.x + lx * c - ly * s, self.y + lx * s + ly * c)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Bone {
    pub name: String,
    pub parent: i32,
    /// rest head in world XY, rest angle (degrees), length
    pub hx: f32,
    pub hy: f32,
    pub ang: f32,
    pub len: f32,
    /// pose: rotation (deg), translation, stretch along the bone
    pub rot: f32,
    pub tx: f32,
    pub ty: f32,
    pub sc: f32,
    /// automatic sway for hair / cloth / tails: amplitude (deg), frequency (Hz), phase (cycles)
    pub sway: f32,
    pub sway_f: f32,
    pub sway_ph: f32,
}
impl Default for Bone {
    fn default() -> Self {
        Bone { name: "Bone".into(), parent: -1, hx: 0.0, hy: 0.0, ang: 0.0, len: 50.0, rot: 0.0, tx: 0.0, ty: 0.0, sc: 1.0, sway: 0.0, sway_f: 0.8, sway_ph: 0.0 }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Cam {
    pub proj: u8,
    pub x: f32,
    pub y: f32,
    pub dist: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub fov: f32,
    pub strength: f32,
    pub curv: f32,
    pub zoom: f32,
    pub fog: f32,
    pub fog_col: [f32; 3],
    pub bg: [f32; 4],
}
impl Default for Cam {
    fn default() -> Self {
        Cam { proj: 1, x: 0.0, y: 0.0, dist: 1000.0, yaw: 0.0, pitch: 0.0, roll: 0.0, fov: 40.0, strength: 1.0, curv: 1.0, zoom: 1.0, fog: 0.0, fog_col: [0.75, 0.85, 1.0], bg: [0.0, 0.0, 0.0, 0.0] }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Key {
    pub t: f32,
    pub v: f32,
    pub e: u8,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Track {
    pub target: String,
    pub keys: Vec<Key>,
}

pub fn ease(e: u8, t: f32) -> f32 {
    use core::f32::consts::PI;
    let t = t.clamp(0.0, 1.0);
    match e {
        1 => 0.0,
        2 => t * t * (3.0 - 2.0 * t),
        3 => t * t,
        4 => 1.0 - (1.0 - t) * (1.0 - t),
        5 => {
            let c1 = 1.70158;
            let c3 = c1 + 1.0;
            1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
        }
        6 => {
            if t <= 0.0 || t >= 1.0 {
                t
            } else {
                2f32.powf(-10.0 * t) * ((t * 10.0 - 0.75) * (2.0 * PI / 3.0)).sin() + 1.0
            }
        }
        7 => {
            let n1 = 7.5625;
            let d1 = 2.75;
            if t < 1.0 / d1 {
                n1 * t * t
            } else if t < 2.0 / d1 {
                let t = t - 1.5 / d1;
                n1 * t * t + 0.75
            } else if t < 2.5 / d1 {
                let t = t - 2.25 / d1;
                n1 * t * t + 0.9375
            } else {
                let t = t - 2.625 / d1;
                n1 * t * t + 0.984375
            }
        }
        8 => -((PI * t).cos() - 1.0) / 2.0,
        9 => {
            if t <= 0.0 || t >= 1.0 {
                t
            } else if t < 0.5 {
                2f32.powf(20.0 * t - 10.0) / 2.0
            } else {
                (2.0 - 2f32.powf(-20.0 * t + 10.0)) / 2.0
            }
        }
        _ => t,
    }
}

impl Track {
    pub fn value(&self, t: f32) -> Option<f32> {
        let k = &self.keys;
        if k.is_empty() {
            return None;
        }
        if t <= k[0].t {
            return Some(k[0].v);
        }
        for i in 0..k.len() - 1 {
            if t < k[i + 1].t {
                let span = (k[i + 1].t - k[i].t).max(1e-6);
                let e = ease(k[i].e, (t - k[i].t) / span);
                return Some(k[i].v + (k[i + 1].v - k[i].v) * e);
            }
        }
        Some(k[k.len() - 1].v)
    }
}

/// 2D affine: x' = a x + c y + e, y' = b x + d y + f
#[derive(Clone, Copy, Debug)]
pub struct Aff([f32; 6]);
impl Aff {
    pub fn id() -> Aff {
        Aff([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
    }
    pub fn trs(tx: f32, ty: f32, ang: f32, sx: f32) -> Aff {
        let (s, c) = ang.sin_cos();
        Aff([c * sx, s * sx, -s, c, tx, ty])
    }
    pub fn mul(&self, o: &Aff) -> Aff {
        let a = self.0;
        let b = o.0;
        Aff([
            a[0] * b[0] + a[2] * b[1],
            a[1] * b[0] + a[3] * b[1],
            a[0] * b[2] + a[2] * b[3],
            a[1] * b[2] + a[3] * b[3],
            a[0] * b[4] + a[2] * b[5] + a[4],
            a[1] * b[4] + a[3] * b[5] + a[5],
        ])
    }
    pub fn inv(&self) -> Aff {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        let k = if det.abs() < 1e-9 { 0.0 } else { 1.0 / det };
        Aff([d * k, -b * k, -c * k, a * k, (c * f - d * e) * k, (b * e - a * f) * k])
    }
    #[inline(always)]
    pub fn ap(&self, x: f32, y: f32) -> (f32, f32) {
        let m = self.0;
        (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Scene {
    pub w: f32,
    pub h: f32,
    pub parts: Vec<Part>,
    pub bones: Vec<Bone>,
    pub cam: Cam,
    pub tracks: Vec<Track>,
    pub fps: f32,
    pub len: f32,
    #[serde(skip)]
    pub time: f32,
}
impl Default for Scene {
    fn default() -> Self {
        Scene { w: 1920.0, h: 1080.0, parts: vec![], bones: vec![], cam: Cam::default(), tracks: vec![], fps: 24.0, len: 4.0, time: 0.0 }
    }
}

/// frame -> target mapping and the camera maths
pub struct Proj {
    cam: Cam,
    f: f32,
    cx: f32,
    cy: f32,
    r: [[f32; 3]; 3],
    k: f32,
    ox: f32,
    oy: f32,
    disk: f32,
}
impl Proj {
    pub fn new(sc: &Scene, k: f32, ox: f32, oy: f32) -> Proj {
        let cam = sc.cam.clone();
        let f = (sc.h * 0.5) / (cam.fov.clamp(1.0, 170.0).to_radians() * 0.5).tan() * cam.zoom.max(0.01);
        let (sy, cy) = cam.yaw.to_radians().sin_cos();
        let (sp, cp) = cam.pitch.to_radians().sin_cos();
        let (sr, cr) = cam.roll.to_radians().sin_cos();
        // R = Rz(roll) * Rx(pitch) * Ry(yaw)
        let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
        let rx = [[1.0, 0.0, 0.0], [0.0, cp, -sp], [0.0, sp, cp]];
        let rz = [[cr, -sr, 0.0], [sr, cr, 0.0], [0.0, 0.0, 1.0]];
        let m = |a: [[f32; 3]; 3], b: [[f32; 3]; 3]| {
            let mut o = [[0.0f32; 3]; 3];
            for i in 0..3 {
                for j in 0..3 {
                    o[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
                }
            }
            o
        };
        let r = m(rz, m(rx, ry));
        let disk = cam.curv.max(0.05) * sc.w.min(sc.h) * 0.5;
        Proj { cam, f, cx: sc.w * 0.5, cy: sc.h * 0.5, r, k, ox, oy, disk }
    }
    #[inline(always)]
    fn view(&self, p: [f32; 3]) -> [f32; 3] {
        let q = [p[0] - self.cam.x, p[1] - self.cam.y, p[2]];
        let r = &self.r;
        [r[0][0] * q[0] + r[0][1] * q[1] + r[0][2] * q[2], r[1][0] * q[0] + r[1][1] * q[1] + r[1][2] * q[2], r[2][0] * q[0] + r[2][1] * q[1] + r[2][2] * q[2] + self.cam.dist]
    }
    /// world -> target pixel (x, y, 1/w, view depth)
    pub fn project(&self, p: [f32; 3]) -> Option<[f32; 4]> {
        let v = self.view(p);
        let (fx, fy, iw) = match self.cam.proj {
            0 => {
                let s = self.f / self.cam.dist.max(1.0);
                (v[0] * s, v[1] * s, 1.0)
            }
            2 => {
                let r = (v[0] * v[0] + v[1] * v[1]).sqrt();
                let th = r.atan2(v[2]);
                let k = self.cam.strength.clamp(0.0, 1.0);
                if th > 3.1 || (k < 1.0 && th > 1.5) {
                    return None;
                }
                let rr = self.f * (k * th + (1.0 - k) * th.min(1.5).tan());
                let len = (r * r + v[2] * v[2]).sqrt().max(1e-3);
                if r < 1e-6 {
                    (0.0, 0.0, 1.0 / len)
                } else {
                    (v[0] / r * rr, v[1] / r * rr, 1.0 / len)
                }
            }
            3 => {
                if v[2] < 1.0 {
                    return None;
                }
                let px = self.f * v[0] / v[2];
                let py = self.f * v[1] / v[2];
                let rp = (px * px + py * py).sqrt();
                if rp < 1e-6 {
                    (0.0, 0.0, 1.0 / v[2])
                } else {
                    let rh = self.disk * (rp / self.disk).tanh();
                    (px / rp * rh, py / rp * rh, 1.0 / v[2])
                }
            }
            _ => {
                if v[2] < 1.0 {
                    return None;
                }
                (self.f * v[0] / v[2], self.f * v[1] / v[2], 1.0 / v[2])
            }
        };
        Some([self.ox + (self.cx + fx) * self.k, self.oy + (self.cy + fy) * self.k, iw, v[2]])
    }
    /// target pixel -> world point on the plane z = pz
    pub fn unproject(&self, tx: f32, ty: f32, pz: f32) -> Option<(f32, f32)> {
        let ux = (tx - self.ox) / self.k - self.cx;
        let uy = (ty - self.oy) / self.k - self.cy;
        let d = self.cam.dist;
        let (o, dir) = match self.cam.proj {
            0 => {
                let s = self.f / d.max(1.0);
                ([ux / s, uy / s, -d], [0.0, 0.0, 1.0])
            }
            2 => {
                let r = (ux * ux + uy * uy).sqrt();
                let k = self.cam.strength.clamp(0.0, 1.0);
                let g = |th: f32| self.f * (k * th + (1.0 - k) * th.min(1.5).tan());
                let (mut lo, mut hi) = (0.0f32, if k < 1.0 { 1.5 } else { 3.1 });
                for _ in 0..40 {
                    let mid = (lo + hi) * 0.5;
                    if g(mid) < r {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                let th = (lo + hi) * 0.5;
                let (s, c) = th.sin_cos();
                if r < 1e-6 {
                    ([0.0, 0.0, -d], [0.0, 0.0, 1.0])
                } else {
                    ([0.0, 0.0, -d], [s * ux / r, s * uy / r, c])
                }
            }
            3 => {
                let rh = (ux * ux + uy * uy).sqrt();
                if rh < 1e-6 {
                    ([0.0, 0.0, -d], [0.0, 0.0, 1.0])
                } else {
                    let q = (rh / self.disk).min(0.9999);
                    let rp = self.disk * 0.5 * ((1.0 + q) / (1.0 - q)).ln();
                    ([0.0, 0.0, -d], [ux / rh * rp / self.f, uy / rh * rp / self.f, 1.0])
                }
            }
            _ => ([0.0, 0.0, -d], [ux / self.f, uy / self.f, 1.0]),
        };
        // q-space (rotated, before the dist shift) -> world: p = R^T q
        let rt = |q: [f32; 3]| {
            let r = &self.r;
            [r[0][0] * q[0] + r[1][0] * q[1] + r[2][0] * q[2], r[0][1] * q[0] + r[1][1] * q[1] + r[2][1] * q[2], r[0][2] * q[0] + r[1][2] * q[1] + r[2][2] * q[2]]
        };
        let ow = rt(o);
        let dw = rt(dir);
        if dw[2].abs() < 1e-6 {
            return None;
        }
        let t = (pz - ow[2]) / dw[2];
        Some((ow[0] + dw[0] * t + self.cam.x, ow[1] + dw[1] * t + self.cam.y))
    }
}

struct Tri {
    part: usize,
    v: [[f32; 4]; 3],
    uv: [[f32; 2]; 3],
    op: f32,
    fog: f32,
}

impl Scene {
    pub fn fit_camera(&mut self) {
        self.cam.x = self.w * 0.5;
        self.cam.y = self.h * 0.5;
        self.cam.dist = (self.h * 0.5) / (self.cam.fov.clamp(1.0, 170.0).to_radians() * 0.5).tan();
    }

    // ── properties by name (animation targets: "p3.x", "b0.rot", "cam.fov") ─────
    pub fn get(&self, target: &str) -> Option<f32> {
        let (who, prop) = target.split_once('.')?;
        if who == "cam" {
            let c = &self.cam;
            return Some(match prop {
                "proj" => c.proj as f32,
                "x" => c.x,
                "y" => c.y,
                "dist" => c.dist,
                "yaw" => c.yaw,
                "pitch" => c.pitch,
                "roll" => c.roll,
                "fov" => c.fov,
                "strength" => c.strength,
                "curv" => c.curv,
                "zoom" => c.zoom,
                "fog" => c.fog,
                _ => return None,
            });
        }
        let idx: usize = who[1..].parse().ok()?;
        if who.starts_with('p') {
            let p = self.parts.get(idx)?;
            Some(match prop {
                "x" => p.x,
                "y" => p.y,
                "z" => p.z,
                "rx" => p.rx,
                "ry" => p.ry,
                "rz" => p.rz,
                "sx" => p.sx,
                "sy" => p.sy,
                "px" => p.px,
                "py" => p.py,
                "opacity" => p.opacity,
                "visible" => p.visible,
                "cols" => p.cols as f32,
                "rows" => p.rows as f32,
                "rigid" => p.rigid as f32,
                "w" => p.img.w as f32,
                "h" => p.img.h as f32,
                _ => return None,
            })
        } else if who.starts_with('b') {
            let b = self.bones.get(idx)?;
            Some(match prop {
                "parent" => b.parent as f32,
                "hx" => b.hx,
                "hy" => b.hy,
                "ang" => b.ang,
                "len" => b.len,
                "rot" => b.rot,
                "tx" => b.tx,
                "ty" => b.ty,
                "sc" => b.sc,
                "sway" => b.sway,
                "sway_f" => b.sway_f,
                "sway_ph" => b.sway_ph,
                _ => return None,
            })
        } else {
            None
        }
    }
    pub fn set(&mut self, target: &str, v: f32) {
        let Some((who, prop)) = target.split_once('.') else { return };
        if who == "cam" {
            let c = &mut self.cam;
            match prop {
                "proj" => c.proj = (v.max(0.0) as u8).min(3),
                "x" => c.x = v,
                "y" => c.y = v,
                "dist" => c.dist = v.max(1.0),
                "yaw" => c.yaw = v,
                "pitch" => c.pitch = v,
                "roll" => c.roll = v,
                "fov" => c.fov = v.clamp(1.0, 170.0),
                "strength" => c.strength = v.clamp(0.0, 1.0),
                "curv" => c.curv = v.clamp(0.05, 4.0),
                "zoom" => c.zoom = v.max(0.01),
                "fog" => c.fog = v.max(0.0),
                _ => {}
            }
            return;
        }
        let Ok(idx) = who[1..].parse::<usize>() else { return };
        if who.starts_with('p') {
            let mut rebind = false;
            if let Some(p) = self.parts.get_mut(idx) {
                match prop {
                    "x" => p.x = v,
                    "y" => p.y = v,
                    "z" => p.z = v,
                    "rx" => p.rx = v,
                    "ry" => p.ry = v,
                    "rz" => p.rz = v,
                    "sx" => p.sx = v,
                    "sy" => p.sy = v,
                    "px" => p.px = v,
                    "py" => p.py = v,
                    "opacity" => p.opacity = v.clamp(0.0, 1.0),
                    "visible" => p.visible = if v > 0.5 { 1.0 } else { 0.0 },
                    "cols" | "rows" => {
                        let n = (v.round() as u32).clamp(1, 64);
                        if prop == "cols" {
                            p.cols = n;
                        } else {
                            p.rows = n;
                        }
                        p.rebuild_occ();
                        rebind = true;
                    }
                    _ => {}
                }
            }
            if rebind {
                let (bones, rigid) = (self.parts[idx].bones.clone(), self.parts[idx].rigid);
                self.bind(idx, &bones, rigid >= 0);
            }
        } else if who.starts_with('b') {
            if let Some(b) = self.bones.get_mut(idx) {
                match prop {
                    "hx" => b.hx = v,
                    "hy" => b.hy = v,
                    "ang" => b.ang = v,
                    "len" => b.len = v.max(1.0),
                    "rot" => b.rot = v,
                    "tx" => b.tx = v,
                    "ty" => b.ty = v,
                    "sc" => b.sc = v,
                    "sway" => b.sway = v,
                    "sway_f" => b.sway_f = v.max(0.0),
                    "sway_ph" => b.sway_ph = v,
                    _ => {}
                }
            }
        }
    }

    // ── animation ────────────────────────────────────────────────────────────
    pub fn eval(&mut self, t: f32) {
        self.time = t;
        let vals: Vec<(String, f32)> = self.tracks.iter().filter_map(|tr| tr.value(t).map(|v| (tr.target.clone(), v))).collect();
        for (k, v) in vals {
            self.set(&k, v);
        }
    }
    pub fn track_mut(&mut self, target: &str) -> &mut Track {
        if let Some(i) = self.tracks.iter().position(|t| t.target == target) {
            return &mut self.tracks[i];
        }
        self.tracks.push(Track { target: target.into(), keys: vec![] });
        self.tracks.last_mut().unwrap()
    }
    pub fn key_set(&mut self, target: &str, t: f32, v: f32, e: i32) {
        let tr = self.track_mut(target);
        let snap = 1e-4;
        if let Some(k) = tr.keys.iter_mut().find(|k| (k.t - t).abs() < snap) {
            k.v = v;
            if e >= 0 {
                k.e = e as u8;
            }
        } else {
            tr.keys.push(Key { t, v, e: if e >= 0 { e as u8 } else { 2 } });
            tr.keys.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(core::cmp::Ordering::Equal));
        }
    }
    pub fn key_del(&mut self, target: &str, t: f32, tol: f32) {
        if let Some(tr) = self.tracks.iter_mut().find(|tr| tr.target == target) {
            tr.keys.retain(|k| (k.t - t).abs() > tol);
        }
        self.tracks.retain(|tr| !tr.keys.is_empty());
    }
    /// rename targets after removing item `idx` of kind 'p' or 'b'
    fn remap(&mut self, kind: char, idx: usize) {
        let mut keep = Vec::new();
        for mut tr in core::mem::take(&mut self.tracks) {
            if let Some((who, prop)) = tr.target.clone().split_once('.') {
                if who.starts_with(kind) {
                    if let Ok(i) = who[1..].parse::<usize>() {
                        if i == idx {
                            continue;
                        }
                        if i > idx {
                            tr.target = format!("{}{}.{}", kind, i - 1, prop);
                        }
                    }
                }
            }
            keep.push(tr);
        }
        self.tracks = keep;
    }
    pub fn del_part(&mut self, i: usize) {
        if i < self.parts.len() {
            self.parts.remove(i);
            self.remap('p', i);
        }
    }
    pub fn del_bone(&mut self, i: usize) {
        if i >= self.bones.len() {
            return;
        }
        let gp = self.bones[i].parent;
        self.bones.remove(i);
        for b in self.bones.iter_mut() {
            if b.parent == i as i32 {
                b.parent = gp;
            } else if b.parent > i as i32 {
                b.parent -= 1;
            }
        }
        for p in self.parts.iter_mut() {
            p.bones.retain(|&b| b != i as i32);
            for b in p.bones.iter_mut() {
                if *b > i as i32 {
                    *b -= 1;
                }
            }
            if p.rigid == i as i32 {
                p.rigid = -1;
            } else if p.rigid > i as i32 {
                p.rigid -= 1;
            }
            for w in p.weights.iter_mut() {
                for inf in w.iter_mut() {
                    if inf.0 == i as i32 {
                        *inf = (-1, 0.0);
                    } else if inf.0 > i as i32 {
                        inf.0 -= 1;
                    }
                }
            }
        }
        self.remap('b', i);
    }
    /// move part i to position j in the draw order (for parts at equal depth)
    pub fn reorder_part(&mut self, i: usize, j: usize) {
        if i >= self.parts.len() || j >= self.parts.len() || i == j {
            return;
        }
        let p = self.parts.remove(i);
        self.parts.insert(j, p);
        let order: Vec<usize> = {
            let mut o: Vec<usize> = (0..self.parts.len()).collect();
            let x = o.remove(i);
            o.insert(j, x);
            o
        };
        for tr in self.tracks.iter_mut() {
            if let Some((who, prop)) = tr.target.clone().split_once('.') {
                if who.starts_with('p') {
                    if let Ok(old) = who[1..].parse::<usize>() {
                        if let Some(newi) = order.iter().position(|&o| o == old) {
                            tr.target = format!("p{newi}.{prop}");
                        }
                    }
                }
            }
        }
    }

    // ── rig ──────────────────────────────────────────────────────────────────
    fn rest_world(&self, i: usize) -> Aff {
        let b = &self.bones[i];
        Aff::trs(b.hx, b.hy, b.ang.to_radians(), 1.0)
    }
    /// posed world matrix of every bone
    pub fn pose(&self) -> Vec<Aff> {
        let n = self.bones.len();
        let mut out: Vec<Option<Aff>> = vec![None; n];
        fn solve(sc: &Scene, i: usize, out: &mut Vec<Option<Aff>>, depth: u32) -> Aff {
            if let Some(m) = out[i] {
                return m;
            }
            let b = &sc.bones[i];
            let sway = b.sway * (core::f32::consts::TAU * (b.sway_f * sc.time + b.sway_ph)).sin();
            let delta = Aff::trs(b.tx, b.ty, (b.rot + sway).to_radians(), b.sc);
            let rest = sc.rest_world(i);
            let m = if b.parent >= 0 && (b.parent as usize) < sc.bones.len() && b.parent as usize != i && depth < 64 {
                let p = b.parent as usize;
                let pw = solve(sc, p, out, depth + 1);
                let local = sc.rest_world(p).inv().mul(&rest);
                pw.mul(&local).mul(&delta)
            } else {
                rest.mul(&delta)
            };
            out[i] = Some(m);
            m
        }
        for i in 0..n {
            solve(self, i, &mut out, 0);
        }
        out.into_iter().map(|m| m.unwrap_or(Aff::id())).collect()
    }
    pub fn skins(&self) -> Vec<Aff> {
        let pose = self.pose();
        (0..self.bones.len()).map(|i| pose[i].mul(&self.rest_world(i).inv())).collect()
    }
    /// bind part i to bones (auto weights by distance, or rigid to the first bone)
    pub fn bind(&mut self, i: usize, bones: &[i32], rigid: bool) {
        if i >= self.parts.len() {
            return;
        }
        let bones: Vec<i32> = bones.iter().copied().filter(|&b| b >= 0 && (b as usize) < self.bones.len()).collect();
        let segs: Vec<(i32, f32, f32, f32, f32, f32)> = bones
            .iter()
            .map(|&b| {
                let bn = &self.bones[b as usize];
                let (s, c) = bn.ang.to_radians().sin_cos();
                (b, bn.hx, bn.hy, bn.hx + c * bn.len, bn.hy + s * bn.len, bn.len.max(1.0))
            })
            .collect();
        let p = &self.parts[i];
        let mut w = Vec::new();
        if !bones.is_empty() {
            for gy in 0..=p.rows {
                for gx in 0..=p.cols {
                    if rigid {
                        w.push([(bones[0], 1.0), (-1, 0.0)]);
                        continue;
                    }
                    let (x, y) = p.rest_vertex(gx, gy);
                    let mut best: Vec<(i32, f32)> = segs
                        .iter()
                        .map(|&(b, ax, ay, bx, by, len)| {
                            let (dx, dy) = (bx - ax, by - ay);
                            let l2 = (dx * dx + dy * dy).max(1e-6);
                            let t = (((x - ax) * dx + (y - ay) * dy) / l2).clamp(0.0, 1.0);
                            let d = (x - ax - dx * t).hypot(y - ay - dy * t) / len;
                            (b, 1.0 / (d + 0.08).powi(3))
                        })
                        .collect();
                    best.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
                    let a = best[0];
                    let b = best.get(1).copied().unwrap_or((-1, 0.0));
                    let s = a.1 + b.1;
                    w.push([(a.0, a.1 / s), (b.0, b.1 / s)]);
                }
            }
        }
        let p = &mut self.parts[i];
        p.bones = bones.clone();
        p.rigid = if rigid && !bones.is_empty() { bones[0] } else { -1 };
        p.weights = w;
    }

    /// projected mesh of part i: per vertex (x, y, 1/w, depth) or None when clipped
    fn mesh(&self, i: usize, pj: &Proj, skins: &[Aff]) -> Vec<Option<[f32; 4]>> {
        let p = &self.parts[i];
        let (sxr, cxr) = p.rx.to_radians().sin_cos();
        let (syr, cyr) = p.ry.to_radians().sin_cos();
        let mut out = Vec::with_capacity(((p.cols + 1) * (p.rows + 1)) as usize);
        let bound = p.weights.len() == ((p.cols + 1) * (p.rows + 1)) as usize;
        let mut vi = 0;
        for gy in 0..=p.rows {
            for gx in 0..=p.cols {
                let (mut x, mut y) = p.rest_vertex(gx, gy);
                if bound {
                    let w = p.weights[vi];
                    let (mut ax, mut ay, mut ws) = (0.0, 0.0, 0.0);
                    for (b, wt) in w {
                        if b >= 0 && (b as usize) < skins.len() && wt > 0.0 {
                            let (px, py) = skins[b as usize].ap(x, y);
                            ax += px * wt;
                            ay += py * wt;
                            ws += wt;
                        }
                    }
                    if ws > 0.0 {
                        x = ax / ws;
                        y = ay / ws;
                    }
                }
                vi += 1;
                // part tilt around its pivot (rx about X, ry about Y)
                let lx = x - p.x;
                let ly = y - p.y;
                let y1 = ly * cxr;
                let z1 = ly * sxr;
                let x2 = lx * cyr + z1 * syr;
                let z2 = -lx * syr + z1 * cyr;
                out.push(pj.project([p.x + x2, p.y + y1, p.z + z2]));
            }
        }
        out
    }

    fn tris(&self, pj: &Proj) -> Vec<Tri> {
        let skins = self.skins();
        let mut order: Vec<(usize, f32, Vec<Option<[f32; 4]>>)> = Vec::new();
        for i in 0..self.parts.len() {
            let p = &self.parts[i];
            if p.visible < 0.5 || p.opacity <= 0.0 || p.img.w == 0 {
                continue;
            }
            let m = self.mesh(i, pj, &skins);
            let (mut s, mut n) = (0.0, 0.0);
            for v in m.iter().flatten() {
                s += v[3];
                n += 1.0;
            }
            if n == 0.0 {
                continue;
            }
            order.push((i, s / n, m));
        }
        // far first; equal depth keeps the list order (later parts on top)
        order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
        let mut tris = Vec::new();
        for (i, _, m) in order {
            let p = &self.parts[i];
            let c1 = p.cols + 1;
            for gy in 0..p.rows {
                for gx in 0..p.cols {
                    if !p.occ.is_empty() && !p.occ[(gy * p.cols + gx) as usize] {
                        continue;
                    }
                    let i00 = (gy * c1 + gx) as usize;
                    let i10 = i00 + 1;
                    let i01 = i00 + c1 as usize;
                    let i11 = i01 + 1;
                    let uv = |gx: u32, gy: u32| [gx as f32 / p.cols as f32, gy as f32 / p.rows as f32];
                    let (u00, u10, u01, u11) = (uv(gx, gy), uv(gx + 1, gy), uv(gx, gy + 1), uv(gx + 1, gy + 1));
                    let depth = |v: [f32; 4]| v[3];
                    for (a, b, c, ua, ub, uc) in [(i00, i10, i11, u00, u10, u11), (i00, i11, i01, u00, u11, u01)] {
                        if let (Some(va), Some(vb), Some(vc)) = (m[a], m[b], m[c]) {
                            let dz = ((depth(va) + depth(vb) + depth(vc)) / 3.0 - self.cam.dist).max(0.0);
                            let fog = if self.cam.fog > 0.0 { 1.0 - (-self.cam.fog * dz / 1000.0).exp() } else { 0.0 };
                            tris.push(Tri { part: i, v: [va, vb, vc], uv: [ua, ub, uc], op: p.opacity, fog });
                        }
                    }
                }
            }
        }
        tris
    }

    /// render into a premultiplied target (tw x th); frame -> target by scale k and offset (ox, oy)
    pub fn render(&self, out: &mut [[f32; 4]], tw: usize, th: usize, k: f32, ox: f32, oy: f32) {
        let pj = Proj::new(self, k, ox, oy);
        let tris = self.tris(&pj);
        let bg = self.cam.bg;
        let bgp = [bg[0] * bg[3], bg[1] * bg[3], bg[2] * bg[3], bg[3]];
        let fogc = self.cam.fog_col;
        let band = 16usize;
        let band_job = |bi: usize, rows: &mut [[f32; 4]]| {
            let y0 = bi * band;
            let nrows = rows.len() / tw;
            for p in rows.iter_mut() {
                *p = bgp;
            }
            for t in &tris {
                let img = &self.parts[t.part].img;
                raster_tri(t, img, rows, tw, th, y0, nrows, fogc);
            }
        };
        #[cfg(feature = "std")]
        out.par_chunks_mut(tw * band).enumerate().for_each(|(bi, rows)| band_job(bi, rows));
        #[cfg(not(feature = "std"))]
        crate::os::par_chunks(out, tw * band, &band_job);
    }

    /// topmost part under target pixel (tx, ty) whose picture is visible there
    pub fn pick(&self, tx: f32, ty: f32, k: f32, ox: f32, oy: f32) -> i32 {
        let pj = Proj::new(self, k, ox, oy);
        let tris = self.tris(&pj);
        for t in tris.iter().rev() {
            let [a, b, c] = t.v;
            let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            if area.abs() < 1e-6 {
                continue;
            }
            let w0 = ((b[0] - tx) * (c[1] - ty) - (b[1] - ty) * (c[0] - tx)) / area;
            let w1 = ((c[0] - tx) * (a[1] - ty) - (c[1] - ty) * (a[0] - tx)) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                let iw = w0 * a[2] + w1 * b[2] + w2 * c[2];
                let u = (w0 * t.uv[0][0] * a[2] + w1 * t.uv[1][0] * b[2] + w2 * t.uv[2][0] * c[2]) / iw;
                let v = (w0 * t.uv[0][1] * a[2] + w1 * t.uv[1][1] * b[2] + w2 * t.uv[2][1] * c[2]) / iw;
                if self.parts[t.part].img.sample(u, v)[3] > 0.15 {
                    return t.part as i32;
                }
            }
        }
        -1
    }
    /// posed head and tail of every bone projected on the rig plane z = 0: [hx, hy, tx, ty] per bone
    pub fn bone_screen(&self, k: f32, ox: f32, oy: f32) -> Vec<f32> {
        let pj = Proj::new(self, k, ox, oy);
        let pose = self.pose();
        let mut out = Vec::new();
        for (i, b) in self.bones.iter().enumerate() {
            let (hx, hy) = pose[i].ap(0.0, 0.0);
            let (tx, ty) = pose[i].ap(b.len, 0.0);
            let h = pj.project([hx, hy, 0.0]).unwrap_or([-9999.0; 4]);
            let t = pj.project([tx, ty, 0.0]).unwrap_or([-9999.0; 4]);
            out.extend_from_slice(&[h[0], h[1], t[0], t[1]]);
        }
        out
    }
    /// put part i at depth z without changing how big it looks through the camera at rest
    pub fn set_depth_keep(&mut self, i: usize, z: f32) {
        if i >= self.parts.len() {
            return;
        }
        let d = self.cam.dist.max(1.0);
        let (cx, cy) = (self.cam.x, self.cam.y);
        let p = &mut self.parts[i];
        let old = (d + p.z) / d;
        let new = (d + z) / d;
        if old <= 0.01 || new <= 0.01 {
            p.z = z;
            return;
        }
        let k = new / old;
        p.x = cx + (p.x - cx) * k;
        p.y = cy + (p.y - cy) * k;
        p.sx *= k;
        p.sy *= k;
        p.z = z;
    }
    /// straight RGBA8 frame at time t, scaled by `scale`
    pub fn frame(&mut self, t: f32, scale: f32) -> (usize, usize, Vec<u8>) {
        self.eval(t);
        let w = ((self.w * scale).round() as usize).max(1);
        let h = ((self.h * scale).round() as usize).max(1);
        let mut buf = vec![[0.0f32; 4]; w * h];
        self.render(&mut buf, w, h, scale, 0.0, 0.0);
        let mut out = Vec::with_capacity(w * h * 4);
        for c in buf {
            let u = unpre(c);
            out.extend_from_slice(&[(u[0] * 255.0 + 0.5) as u8, (u[1] * 255.0 + 0.5) as u8, (u[2] * 255.0 + 0.5) as u8, (u[3] * 255.0 + 0.5) as u8]);
        }
        (w, h, out)
    }
}

#[allow(clippy::too_many_arguments)]
fn raster_tri(t: &Tri, img: &Img, rows: &mut [[f32; 4]], tw: usize, th: usize, y0: usize, nrows: usize, fogc: [f32; 3]) {
    let [a, b, c] = t.v;
    let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    if area.abs() < 1e-6 {
        return;
    }
    let minx = a[0].min(b[0]).min(c[0]).floor().max(0.0) as i32;
    let maxx = a[0].max(b[0]).max(c[0]).ceil().min(tw as f32 - 1.0) as i32;
    let miny = (a[1].min(b[1]).min(c[1]).floor().max(y0 as f32) as i32).max(0);
    let maxy = a[1].max(b[1]).max(c[1]).ceil().min((y0 + nrows) as f32 - 1.0).min(th as f32 - 1.0) as i32;
    if minx > maxx || miny > maxy {
        return;
    }
    let inv_area = 1.0 / area;
    // attributes divided by w for perspective-correct texturing
    let ua = [t.uv[0][0] * a[2], t.uv[1][0] * b[2], t.uv[2][0] * c[2]];
    let va = [t.uv[0][1] * a[2], t.uv[1][1] * b[2], t.uv[2][1] * c[2]];
    let iw = [a[2], b[2], c[2]];
    for y in miny..=maxy {
        let py = y as f32 + 0.5;
        let row = (y as usize - y0) * tw;
        for x in minx..=maxx {
            let px = x as f32 + 0.5;
            let w0 = ((b[0] - px) * (c[1] - py) - (b[1] - py) * (c[0] - px)) * inv_area;
            let w1 = ((c[0] - px) * (a[1] - py) - (c[1] - py) * (a[0] - px)) * inv_area;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let q = w0 * iw[0] + w1 * iw[1] + w2 * iw[2];
            if q <= 0.0 {
                continue;
            }
            let u = (w0 * ua[0] + w1 * ua[1] + w2 * ua[2]) / q;
            let v = (w0 * va[0] + w1 * va[1] + w2 * va[2]) / q;
            let mut s = img.sample(u, v);
            if s[3] <= 0.0 {
                continue;
            }
            for k in 0..4 {
                s[k] *= t.op;
            }
            if t.fog > 0.0 {
                for k in 0..3 {
                    s[k] += (fogc[k] * s[3] - s[k]) * t.fog;
                }
            }
            let d = &mut rows[row + x as usize];
            *d = over(*d, s);
        }
    }
}
