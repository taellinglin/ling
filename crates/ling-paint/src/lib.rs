//! # ling-paint: the LingAnime engine
//!
//! A host (the Ling runtime) owns one [`Studio`] and calls it by name with plain values ([`Studio::call`]);
//! the few calls that draw into the host's framebuffer go through [`Studio::draw`] with an [`Fb`].
//!
//! * documents of tiled 16-bit premultiplied layers (raster or vector), 27 blend modes, clipping, alpha lock,
//!   screentone layers, opacity; copy-on-write undo of the whole document
//! * the brush engine ([`brush`]): procedural tips shaped by pressure, tilt, azimuth, twist and speed
//! * vector paths, gradients, halftone / dither / filters, selections, fill, free transform
//! * 2.5D scenes with bone rigs, projections and keyframe animation ([`scene`]), video / sprite-sheet export
//! * Stable Diffusion in the selection through a local WebUI API ([`sd`]); WinTab pens ([`tablet`])

// Without `std` (the LingOS kernel) the engine is no_std + alloc; see `os` for what the host supplies.
#![cfg_attr(not(feature = "std"), no_std)]
#[cfg(not(feature = "std"))]
#[macro_use]
extern crate alloc;

/// alloc types and libm float maths for the no_std build (empty with `std`, whose prelude has them)
#[allow(unused_imports)]
mod prelude {
    #[cfg(not(feature = "std"))]
    pub use crate::os::{FloatExt, SeqIntoPar, SeqSlice};
    #[cfg(not(feature = "std"))]
    pub use alloc::{
        borrow::ToOwned,
        boxed::Box,
        string::{String, ToString},
        vec::Vec,
    };
    #[cfg(not(feature = "std"))]
    pub use num_traits::Float;
}
#[allow(unused_imports)]
use prelude::*;

#[cfg(not(feature = "std"))]
pub mod os;

pub mod blend;
pub mod brush;
#[cfg(feature = "std")]
pub mod clipboard;
#[cfg(not(feature = "std"))]
#[path = "os/clipboard.rs"]
pub mod clipboard;
pub mod filters;
pub mod gradient;
#[cfg(feature = "std")]
mod io;
#[cfg(not(feature = "std"))]
#[path = "os/io.rs"]
mod io;
pub mod px;
pub mod scene;
#[cfg(feature = "std")]
pub mod sd;
#[cfg(not(feature = "std"))]
#[path = "os/sd.rs"]
pub mod sd;
pub mod split;
pub mod tablet;
pub mod vector;

use brush::{Brush, Ctx, Sample, Stroke};
use gradient::Gradient;
#[cfg(not(feature = "std"))]
use os::image;
#[cfg(not(feature = "std"))]
use os::sync::Mutex;
use px::*;
#[cfg(feature = "std")]
use rayon::prelude::*;
use scene::{Img, Part, Scene};
#[cfg(not(feature = "std"))]
use sd::Receiver;
use serde::{Deserialize, Serialize};
#[cfg(feature = "std")]
use std::sync::mpsc::Receiver;
#[cfg(feature = "std")]
use std::sync::Mutex;
#[cfg(not(feature = "std"))]
use alloc::sync::Arc;
#[cfg(feature = "std")]
use std::sync::Arc;
use vector::VPath;

/// the file name of `p` without its folder and extension
fn file_stem(p: &str) -> Option<String> {
    #[cfg(feature = "std")]
    {
        std::path::Path::new(p).file_stem().map(|s| s.to_string_lossy().to_string())
    }
    #[cfg(not(feature = "std"))]
    {
        let base = p.rsplit(['/', '\\']).next().unwrap_or(p);
        let stem = match base.rfind('.') {
            Some(0) | None => base,
            Some(i) => &base[..i],
        };
        if stem.is_empty() { None } else { Some(stem.to_string()) }
    }
}

/// a value crossing the host boundary
#[derive(Clone, Debug)]
pub enum Val {
    Num(f64),
    Str(String),
    List(Vec<Val>),
    Unit,
}
impl Val {
    fn nums(v: Vec<f32>) -> Val {
        Val::List(v.into_iter().map(|x| Val::Num(x as f64)).collect())
    }
    fn strs<S: AsRef<str>>(v: &[S]) -> Val {
        Val::List(v.iter().map(|s| Val::Str(s.as_ref().to_string())).collect())
    }
}

/// the host framebuffer: 0x00RRGGBB pixels, a clip rectangle [x0, y0, x1, y1) and bits OR-ed into every pixel written
pub struct Fb<'a> {
    pub buf: &'a mut [u32],
    pub w: usize,
    pub h: usize,
    pub clip: [i32; 4],
    pub tag: u32,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerMeta {
    pub name: String,
    /// 0 raster, 1 vector
    pub kind: u8,
    pub visible: f32,
    pub locked: f32,
    pub alpha_lock: f32,
    pub clip: f32,
    pub opacity: f32,
    pub blend: u8,
    pub tone: f32,
    pub tone_lpi: f32,
    pub tone_angle: f32,
    pub tone_shape: u8,
    pub tone_col: [f32; 3],
    pub paths: Vec<VPath>,
}
impl Default for LayerMeta {
    fn default() -> Self {
        LayerMeta {
            name: "Layer".into(),
            kind: 0,
            visible: 1.0,
            locked: 0.0,
            alpha_lock: 0.0,
            clip: 0.0,
            opacity: 1.0,
            blend: 0,
            tone: 0.0,
            tone_lpi: 60.0,
            tone_angle: 45.0,
            tone_shape: 0,
            tone_col: [0.0, 0.0, 0.0],
            paths: vec![],
        }
    }
}

#[derive(Clone)]
pub struct Layer {
    pub m: LayerMeta,
    pub t: Tiles,
}
impl Layer {
    fn new(name: &str, kind: u8, w: usize, h: usize) -> Layer {
        Layer { m: LayerMeta { name: name.into(), kind, ..Default::default() }, t: Tiles::new(w, h) }
    }
    pub fn reraster(&mut self) {
        if self.m.kind == 1 {
            self.t.clear();
            for p in &self.m.paths {
                p.raster(&mut self.t);
            }
        }
    }
}

#[derive(Clone)]
pub struct Doc {
    pub w: usize,
    pub h: usize,
    pub dpi: f32,
    pub name: String,
    pub paper: [f32; 3],
    pub paper_on: bool,
    pub meta: String,
    pub layers: Vec<Layer>,
    pub active: usize,
    pub sel: Option<Mask>,
    pub scene: Scene,
}
impl Doc {
    fn new(w: usize, h: usize, dpi: f32, name: &str) -> Doc {
        let mut scene = Scene { w: w as f32, h: h as f32, ..Default::default() };
        scene.fit_camera();
        Doc { w, h, dpi, name: name.into(), paper: [1.0, 1.0, 1.0], paper_on: true, meta: String::new(), layers: vec![Layer::new("Layer 1", 0, w, h)], active: 0, sel: None, scene }
    }
}

struct Comp {
    w: usize,
    h: usize,
    tw: usize,
    /// premultiplied RGBA8 packed a<<24 | r<<16 | g<<8 | b
    px: Vec<u32>,
    dirty: Vec<bool>,
}
impl Comp {
    fn new(w: usize, h: usize) -> Comp {
        let tw = w.div_ceil(TS);
        let th = h.div_ceil(TS);
        Comp { w, h, tw, px: vec![0; w * h], dirty: vec![true; tw * th] }
    }
    fn mark(&mut self, r: [i32; 4]) {
        let th = self.dirty.len() / self.tw.max(1);
        let x0 = (r[0].max(0) as usize) / TS;
        let y0 = (r[1].max(0) as usize) / TS;
        if r[2] <= 0 || r[3] <= 0 {
            return;
        }
        let x1 = ((r[2] as usize).min(self.w).saturating_sub(1)) / TS;
        let y1 = ((r[3] as usize).min(self.h).saturating_sub(1)) / TS;
        for ty in y0..=y1.min(th.saturating_sub(1)) {
            for tx in x0..=x1.min(self.tw - 1) {
                self.dirty[ty * self.tw + tx] = true;
            }
        }
    }
    fn all(&mut self) {
        self.dirty.iter_mut().for_each(|d| *d = true);
    }
}

/// a floating selection being moved / scaled / rotated
#[derive(Clone)]
struct Xf {
    layer: usize,
    src: Tiles,
    bbox: [f32; 4],
    tx: f32,
    ty: f32,
    sx: f32,
    sy: f32,
    rot: f32,
    vector: bool,
}
impl Xf {
    fn center(&self) -> (f32, f32) {
        ((self.bbox[0] + self.bbox[2]) * 0.5, (self.bbox[1] + self.bbox[3]) * 0.5)
    }
    /// forward map source -> canvas
    fn fwd(&self, x: f32, y: f32) -> (f32, f32) {
        let (cx, cy) = self.center();
        let (s, c) = self.rot.to_radians().sin_cos();
        let lx = (x - cx) * self.sx;
        let ly = (y - cy) * self.sy;
        (cx + self.tx + lx * c - ly * s, cy + self.ty + lx * s + ly * c)
    }
    fn inv(&self, x: f32, y: f32) -> (f32, f32) {
        let (cx, cy) = self.center();
        let (s, c) = self.rot.to_radians().sin_cos();
        let dx = x - cx - self.tx;
        let dy = y - cy - self.ty;
        let lx = dx * c + dy * s;
        let ly = -dx * s + dy * c;
        (cx + lx / if self.sx.abs() < 1e-4 { 1e-4 } else { self.sx }, cy + ly / if self.sy.abs() < 1e-4 { 1e-4 } else { self.sy })
    }
    fn out_bbox(&self) -> [i32; 4] {
        let b = self.bbox;
        let pts = [self.fwd(b[0], b[1]), self.fwd(b[2], b[1]), self.fwd(b[2], b[3]), self.fwd(b[0], b[3])];
        let mut o = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for (x, y) in pts {
            o[0] = o[0].min(x);
            o[1] = o[1].min(y);
            o[2] = o[2].max(x);
            o[3] = o[3].max(y);
        }
        [o[0].floor() as i32 - 2, o[1].floor() as i32 - 2, o[2].ceil() as i32 + 2, o[3].ceil() as i32 + 2]
    }
    #[inline]
    fn sample(&self, x: i32, y: i32) -> [f32; 4] {
        let (u, v) = self.inv(x as f32 + 0.5, y as f32 + 0.5);
        if u < self.bbox[0] - 1.0 || v < self.bbox[1] - 1.0 || u > self.bbox[2] + 1.0 || v > self.bbox[3] + 1.0 {
            return [0.0; 4];
        }
        self.src.sample(u, v)
    }
}

#[derive(Clone, Copy)]
pub struct View {
    pub cx: f32,
    pub cy: f32,
    pub zoom: f32,
    pub rot: f32,
    pub flip: bool,
    pub vx: f32,
    pub vy: f32,
    pub vw: f32,
    pub vh: f32,
    pub grid: bool,
}
impl View {
    fn s2c(&self, sx: f32, sy: f32) -> (f32, f32) {
        let dx = sx - (self.vx + self.vw * 0.5);
        let dy = sy - (self.vy + self.vh * 0.5);
        let (s, c) = (-self.rot.to_radians()).sin_cos();
        let mut x = (dx * c - dy * s) / self.zoom;
        let y = (dx * s + dy * c) / self.zoom;
        if self.flip {
            x = -x;
        }
        (self.cx + x, self.cy + y)
    }
    fn c2s(&self, x: f32, y: f32) -> (f32, f32) {
        let mut dx = (x - self.cx) * self.zoom;
        let dy = (y - self.cy) * self.zoom;
        if self.flip {
            dx = -dx;
        }
        let (s, c) = self.rot.to_radians().sin_cos();
        (self.vx + self.vw * 0.5 + dx * c - dy * s, self.vy + self.vh * 0.5 + dx * s + dy * c)
    }
}

#[derive(Clone, Copy, Default)]
pub struct PenSample {
    pub x: f32,
    pub y: f32,
    pub p: f32,
    pub tilt: f32,
    pub az: f32,
    pub twist: f32,
    pub eraser: bool,
}

struct StrokeState {
    stroke: Stroke,
    orig: Tiles,
    layer: usize,
    vec_raw: Option<Vec<(f32, f32, f32)>>,
    vec_erase: bool,
    last_mouse: (f32, f32),
    sim_p: f32,
}

/// a picture split running on a worker thread
struct SplitJob {
    prog: Arc<core::sync::atomic::AtomicU32>,
    res: Arc<Mutex<Option<Result<split::SplitResult, String>>>>,
    layer: usize,
    x0: i32,
    y0: i32,
    to_stage: bool,
    spread: f32,
}

#[derive(Default)]
struct Sd {
    url: String,
    models: Vec<String>,
    samplers: Vec<String>,
    state: u8,
    progress: f32,
    msg: String,
    rx: Option<Receiver<sd::SdMsg>>,
    /// where the result goes: x, y, w, h and the generation mode
    place: Option<(i32, i32, usize, usize, u8, String)>,
}

#[derive(Default)]
struct Export {
    done: usize,
    total: usize,
    running: bool,
    msg: String,
}

pub struct Studio {
    doc: Doc,
    undo: Vec<Doc>,
    redo: Vec<Doc>,
    comp: Comp,
    view: View,
    brush: Brush,
    presets: Vec<Brush>,
    fg: [f32; 3],
    bg: [f32; 3],
    grad: Gradient,
    stroke: Option<StrokeState>,
    xf: Option<Xf>,
    filt: Option<(String, usize)>,
    clip: Option<(Tiles, [i32; 4])>,
    clip_sig: Option<u64>,
    pen_q: Vec<PenSample>,
    pen_last: PenSample,
    pen_t: f64,
    tablet: tablet::Tablet,
    sim_mode: u8,
    sim_p: f32,
    sim_tilt: f32,
    sim_az: f32,
    sim_follow: bool,
    sd: Sd,
    split: Option<SplitJob>,
    split_msg: String,
    /// where the last split filled in its background, and that background layer
    split_hole: Option<(Mask, usize)>,
    export: Arc<Mutex<Export>>,
    err: String,
    dirty: bool,
    #[cfg(feature = "std")]
    t0: std::time::Instant,
    #[cfg(not(feature = "std"))]
    t0: os::Instant,
    sc_map: (f32, f32, f32, [f32; 4]),
    brush_prev: Option<(String, usize, usize, Vec<[f32; 4]>)>,
    seed: u64,
}

impl Default for Studio {
    fn default() -> Self {
        Self::new()
    }
}

// ── argument helpers ─────────────────────────────────────────────────────────
fn an(a: &[Val], i: usize, d: f64) -> f64 {
    match a.get(i) {
        Some(Val::Num(n)) => *n,
        Some(Val::Str(s)) => s.trim().parse().unwrap_or(d),
        _ => d,
    }
}
fn af(a: &[Val], i: usize, d: f32) -> f32 {
    an(a, i, d as f64) as f32
}
fn as_(a: &[Val], i: usize) -> String {
    match a.get(i) {
        Some(Val::Str(s)) => s.clone(),
        Some(Val::Num(n)) => format!("{n}"),
        _ => String::new(),
    }
}
fn al(a: &[Val], i: usize) -> Vec<f32> {
    match a.get(i) {
        Some(Val::List(l)) => l
            .iter()
            .map(|v| match v {
                Val::Num(n) => *n as f32,
                Val::Str(s) => s.parse().unwrap_or(0.0),
                _ => 0.0,
            })
            .collect(),
        _ => vec![],
    }
}
fn num(v: f64) -> Val {
    Val::Num(v)
}
fn b2n(b: bool) -> Val {
    Val::Num(if b { 1.0 } else { 0.0 })
}

#[inline(always)]
fn pack_pre(c: [f32; 4]) -> u32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    (q(c[3]) << 24) | (q(c[0]) << 16) | (q(c[1]) << 8) | q(c[2])
}
#[inline(always)]
fn unpack_pre(p: u32) -> [f32; 4] {
    const K: f32 = 1.0 / 255.0;
    [((p >> 16) & 255) as f32 * K, ((p >> 8) & 255) as f32 * K, (p & 255) as f32 * K, (p >> 24) as f32 * K]
}

/// composite one 64x64 tile of the document (f32 premultiplied)
fn comp_tile(doc: &Doc, xf: Option<&Xf>, ti: usize, tw: usize) -> Vec<[f32; 4]> {
    let bx = ((ti % tw) * TS) as i32;
    let by = ((ti / tw) * TS) as i32;
    let paper = if doc.paper_on { [doc.paper[0], doc.paper[1], doc.paper[2], 1.0] } else { [0.0; 4] };
    let mut acc = vec![paper; TN];
    let mut base = vec![1.0f32; TN];
    for (li, l) in doc.layers.iter().enumerate() {
        let has_xf = xf.is_some_and(|x| x.layer == li);
        let tile = l.t.t.get(ti).and_then(|t| t.as_ref());
        if l.m.visible < 0.5 || (tile.is_none() && !has_xf) {
            if l.m.clip < 0.5 {
                base.iter_mut().for_each(|b| *b = 0.0);
            }
            continue;
        }
        let op = l.m.opacity;
        let tone = l.m.tone > 0.5;
        let cell = (doc.dpi / l.m.tone_lpi.max(1.0)).max(2.0);
        let (sa, ca) = l.m.tone_angle.to_radians().sin_cos();
        for i in 0..TN {
            let mut s = match tile {
                Some(t) => tof(t[i]),
                None => [0.0; 4],
            };
            if has_xf {
                let f = xf.unwrap().sample(bx + (i % TS) as i32, by + (i / TS) as i32);
                if f[3] > 0.0 {
                    s = over(s, f);
                }
            }
            if tone && s[3] > 0.0 {
                let u = unpre(s);
                let k = u[3] * (1.0 - lum(u[0], u[1], u[2]));
                let c = filters::tone_cov(l.m.tone_shape, cell, sa, ca, (bx + (i % TS) as i32) as f32 + 0.5, (by + (i / TS) as i32) as f32 + 0.5, k);
                let tc = l.m.tone_col;
                s = [tc[0] * c, tc[1] * c, tc[2] * c, c];
            }
            if l.m.clip > 0.5 {
                let k = op * base[i];
                s = [s[0] * k, s[1] * k, s[2] * k, s[3] * k];
            } else {
                s = [s[0] * op, s[1] * op, s[2] * op, s[3] * op];
                base[i] = s[3];
            }
            if s[3] > 0.0 {
                acc[i] = blend::blend(acc[i], s, l.m.blend);
            }
        }
    }
    acc
}

fn flood(w: usize, h: usize, x: i32, y: i32, tol: f32, get: &dyn Fn(usize, usize) -> [f32; 4]) -> Vec<bool> {
    let mut m = vec![false; w * h];
    if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
        return m;
    }
    let seed = get(x as usize, y as usize);
    let close = |c: [f32; 4]| (0..4).all(|k| (c[k] - seed[k]).abs() <= tol + 1e-4);
    let mut stack = vec![(x as usize, y as usize)];
    while let Some((sx, sy)) = stack.pop() {
        if m[sy * w + sx] || !close(get(sx, sy)) {
            continue;
        }
        let mut l = sx;
        while l > 0 && !m[sy * w + l - 1] && close(get(l - 1, sy)) {
            l -= 1;
        }
        let mut r = sx;
        while r + 1 < w && !m[sy * w + r + 1] && close(get(r + 1, sy)) {
            r += 1;
        }
        for xx in l..=r {
            m[sy * w + xx] = true;
        }
        for ny in [sy.wrapping_sub(1), sy + 1] {
            if ny >= h {
                continue;
            }
            let mut xx = l;
            while xx <= r {
                if !m[ny * w + xx] && close(get(xx, ny)) {
                    stack.push((xx, ny));
                    while xx <= r && close(get(xx, ny)) {
                        xx += 1;
                    }
                }
                xx += 1;
            }
        }
    }
    m
}

impl Studio {
    pub fn new() -> Studio {
        let doc = Doc::new(1920, 1080, 144.0, "Untitled");
        let presets = brush::presets();
        let brush = presets[0].clone();
        let comp = Comp::new(doc.w, doc.h);
        let mut s = Studio {
            doc,
            undo: vec![],
            redo: vec![],
            comp,
            view: View { cx: 960.0, cy: 540.0, zoom: 0.5, rot: 0.0, flip: false, vx: 0.0, vy: 0.0, vw: 800.0, vh: 600.0, grid: true },
            brush,
            presets,
            fg: [0.07, 0.06, 0.09],
            bg: [1.0, 1.0, 1.0],
            grad: Gradient::default(),
            stroke: None,
            xf: None,
            filt: None,
            clip: None,
            clip_sig: None,
            pen_q: vec![],
            pen_last: PenSample { p: 1.0, ..Default::default() },
            pen_t: -10.0,
            tablet: tablet::Tablet::default(),
            sim_mode: 0,
            sim_p: 0.8,
            sim_tilt: 0.0,
            sim_az: 0.0,
            sim_follow: false,
            sd: Sd { url: "http://127.0.0.1:7860".into(), ..Default::default() },
            split: None,
            split_msg: String::new(),
            split_hole: None,
            export: Arc::new(Mutex::new(Export::default())),
            err: String::new(),
            dirty: false,
            #[cfg(feature = "std")]
            t0: std::time::Instant::now(),
            #[cfg(not(feature = "std"))]
            t0: os::Instant::now(),
            sc_map: (1.0, 0.0, 0.0, [0.0; 4]),
            brush_prev: None,
            seed: 0x9E3779B97F4A7C15,
        };
        s.grad = Gradient::preset("FG to BG", s.fgc(), [s.bg[0], s.bg[1], s.bg[2], 1.0]);
        s
    }
    fn now(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }
    fn fgc(&self) -> [f32; 4] {
        [self.fg[0], self.fg[1], self.fg[2], 1.0]
    }
    fn fail(&mut self, e: impl Into<String>) -> Val {
        self.err = e.into();
        num(0.0)
    }
    pub fn push_undo(&mut self) {
        self.undo.push(self.doc.clone());
        if self.undo.len() > 60 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
    }
    fn reset_comp(&mut self) {
        if self.comp.w != self.doc.w || self.comp.h != self.doc.h {
            self.comp = Comp::new(self.doc.w, self.doc.h);
        } else {
            self.comp.all();
        }
    }
    fn act(&self) -> usize {
        self.doc.active.min(self.doc.layers.len().saturating_sub(1))
    }
    fn editable(&self, i: usize) -> bool {
        self.doc.layers.get(i).is_some_and(|l| l.m.locked < 0.5)
    }

    /// bring the composite cache up to date
    fn update_comp(&mut self) {
        let tw = self.comp.tw;
        let list: Vec<usize> = self.comp.dirty.iter().enumerate().filter(|(_, d)| **d).map(|(i, _)| i).collect();
        if list.is_empty() {
            return;
        }
        let doc = &self.doc;
        let xf = self.xf.as_ref();
        let res: Vec<(usize, Vec<[f32; 4]>)> = list.par_iter().map(|&ti| (ti, comp_tile(doc, xf, ti, tw))).collect();
        let (w, h) = (self.comp.w, self.comp.h);
        for (ti, px) in res {
            let bx = (ti % tw) * TS;
            let by = (ti / tw) * TS;
            for y in 0..TS {
                let yy = by + y;
                if yy >= h {
                    break;
                }
                for x in 0..TS {
                    let xx = bx + x;
                    if xx >= w {
                        break;
                    }
                    self.comp.px[yy * w + xx] = pack_pre(px[y * TS + x]);
                }
            }
            self.comp.dirty[ti] = false;
        }
    }

    /// full-precision flattened image (straight RGBA16), optionally on the paper colour
    pub(crate) fn flatten16(&self, paper: bool) -> Vec<u16> {
        let mut doc = self.doc.clone();
        doc.paper_on = paper && doc.paper_on;
        let tw = doc.w.div_ceil(TS);
        let th = doc.h.div_ceil(TS);
        let tiles: Vec<Vec<[f32; 4]>> = (0..tw * th).into_par_iter().map(|ti| comp_tile(&doc, None, ti, tw)).collect();
        let mut out = vec![0u16; doc.w * doc.h * 4];
        for (ti, px) in tiles.iter().enumerate() {
            let bx = (ti % tw) * TS;
            let by = (ti / tw) * TS;
            for y in 0..TS {
                for x in 0..TS {
                    let (xx, yy) = (bx + x, by + y);
                    if xx < doc.w && yy < doc.h {
                        let c = unpre(px[y * TS + x]);
                        let o = (yy * doc.w + xx) * 4;
                        for k in 0..4 {
                            out[o + k] = (c[k] * 65535.0 + 0.5) as u16;
                        }
                    }
                }
            }
        }
        out
    }

    // ── pen input ────────────────────────────────────────────────────────────
    /// the host maps tablet packets into window coordinates and hands them over here
    pub fn pen_feed(&mut self, pk: Vec<PenSample>) {
        if pk.is_empty() {
            return;
        }
        self.pen_t = self.now();
        self.pen_last = *pk.last().unwrap();
        if self.stroke.is_some() {
            self.pen_q.extend(pk);
        } else {
            self.pen_q.clear();
        }
    }
    pub fn tablet_poll(&mut self) -> Vec<tablet::Packet> {
        self.tablet.poll()
    }
    pub fn tablet_open(&mut self, hwnd: isize) -> Val {
        match self.tablet.open(hwnd) {
            Ok(n) => {
                self.err = n;
                num(1.0)
            }
            Err(e) => self.fail(e),
        }
    }
    fn pen_recent(&self) -> bool {
        self.now() - self.pen_t < 0.3
    }

    fn mk_sample(&self, sx: f32, sy: f32, p: f32, tilt: f32, az: f32, twist: f32) -> Sample {
        let (x, y) = self.view.s2c(sx, sy);
        // the view's rotation turns the pen's azimuth too
        let az = az - self.view.rot.to_radians();
        Sample { x, y, p, tilt, az, twist, t: self.now() as f32 }
    }

    fn stroke_begin(&mut self, sx: f32, sy: f32, erase: bool) -> Val {
        if self.stroke.is_some() {
            self.stroke_end();
        }
        let li = self.act();
        if !self.editable(li) || self.doc.layers[li].m.visible < 0.5 {
            return self.fail("the layer is locked or hidden");
        }
        self.push_undo();
        let pen = self.pen_recent();
        let eraser = erase || (pen && self.pen_last.eraser) || self.brush.mode == 1;
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let stroke = Stroke::new(self.brush.clone(), self.fg, eraser && self.brush.mode != 2 && self.brush.mode != 3, self.seed);
        let is_vec = self.doc.layers[li].m.kind == 1;
        let orig = self.doc.layers[li].t.clone();
        self.stroke = Some(StrokeState { stroke, orig, layer: li, vec_raw: if is_vec && !eraser { Some(vec![]) } else { None }, vec_erase: is_vec && eraser, last_mouse: (sx, sy), sim_p: 0.15 });
        self.pen_q.clear();
        let p = if pen { self.pen_last.p } else { self.sim_pressure(0.0, true) };
        let (tilt, az, tw) = if pen { (self.pen_last.tilt, self.pen_last.az, self.pen_last.twist) } else { (self.sim_tilt, self.sim_az.to_radians(), 0.0) };
        let (px, py) = if pen && self.pen_aligned(sx, sy) { (self.pen_last.x, self.pen_last.y) } else { (sx, sy) };
        let s = self.mk_sample(px, py, p, tilt, az, tw);
        self.feed(s);
        num(1.0)
    }
    fn pen_aligned(&self, mx: f32, my: f32) -> bool {
        (self.pen_last.x - mx).abs() < 40.0 && (self.pen_last.y - my).abs() < 40.0
    }
    fn sim_pressure(&mut self, speed: f32, start: bool) -> f32 {
        match self.sim_mode {
            1 => self.sim_p,
            2 => {
                // ramp: strokes start light and swell, like a pen pressed down
                if let Some(st) = self.stroke.as_mut() {
                    if !start {
                        st.sim_p = (st.sim_p + 0.06).min(self.sim_p);
                    }
                    st.sim_p
                } else {
                    0.15
                }
            }
            3 => 1.0,
            _ => {
                // velocity: fast strokes thin out (ink-like)
                let v = (speed / 2500.0).clamp(0.0, 1.0);
                let target = (self.sim_p * (1.0 - 0.75 * v)).max(0.08);
                if let Some(st) = self.stroke.as_mut() {
                    st.sim_p += (target - st.sim_p) * 0.35;
                    st.sim_p
                } else {
                    target * 0.5
                }
            }
        }
    }
    fn stroke_to(&mut self, sx: f32, sy: f32) {
        if self.stroke.is_none() {
            return;
        }
        let pen = self.pen_recent();
        if pen && !self.pen_q.is_empty() {
            let q = core::mem::take(&mut self.pen_q);
            let aligned = self.pen_aligned(sx, sy);
            let n = q.len();
            for (i, p) in q.into_iter().enumerate() {
                let (px, py) = if aligned { (p.x, p.y) } else if i + 1 == n { (sx, sy) } else { continue };
                let s = self.mk_sample(px, py, p.p, p.tilt, p.az, p.twist);
                self.feed(s);
            }
            self.stroke.as_mut().unwrap().last_mouse = (sx, sy);
            return;
        }
        if pen {
            return;
        }
        let (lx, ly) = self.stroke.as_ref().unwrap().last_mouse;
        if (lx - sx).abs() < 0.01 && (ly - sy).abs() < 0.01 {
            return;
        }
        let speed = ((sx - lx).hypot(sy - ly) / self.view.zoom) * 60.0;
        let p = self.sim_pressure(speed, false);
        let az = if self.sim_follow { (sy - ly).atan2(sx - lx) + core::f32::consts::FRAC_PI_2 + self.view.rot.to_radians() } else { self.sim_az.to_radians() };
        let s = self.mk_sample(sx, sy, p, self.sim_tilt, az, 0.0);
        self.stroke.as_mut().unwrap().last_mouse = (sx, sy);
        self.feed(s);
    }
    fn feed(&mut self, s: Sample) {
        let Some(st) = self.stroke.as_mut() else { return };
        let li = st.layer;
        if st.vec_erase {
            let r = (self.brush.size * 0.5).max(3.0);
            let l = &mut self.doc.layers[li];
            let before = l.m.paths.len();
            l.m.paths.retain(|p| p.dist(s.x, s.y) > r);
            if l.m.paths.len() != before {
                l.reraster();
                self.comp.all();
            }
            return;
        }
        if let Some(raw) = st.vec_raw.as_mut() {
            let b = &self.brush;
            let pr = s.p.clamp(0.0, 1.0).powf(b.gamma);
            let w = 1.0 + (b.size_min + (1.0 - b.size_min) * pr - 1.0) * b.p_size;
            let prev = raw.last().copied();
            raw.push((s.x, s.y, w));
            // live preview: rasterise the newest segment straight onto the layer
            let seg = VPath {
                a: vec![
                    vector::Anchor { x: prev.map_or(s.x, |p| p.0), y: prev.map_or(s.y, |p| p.1), w: prev.map_or(w, |p| p.2), ..Default::default() },
                    vector::Anchor { x: s.x, y: s.y, w, ..Default::default() },
                ],
                width: b.size,
                color: [self.fg[0], self.fg[1], self.fg[2], 1.0],
                opacity: b.opacity,
                ..Default::default()
            };
            seg.raster(&mut self.doc.layers[li].t);
            let bb = seg.bbox();
            self.comp.mark([bb[0] as i32 - 2, bb[1] as i32 - 2, bb[2] as i32 + 2, bb[3] as i32 + 2]);
            return;
        }
        let StrokeState { stroke, orig, .. } = st;
        let layer = &mut self.doc.layers[li];
        let mut ctx = Ctx { tiles: &mut layer.t, orig, sel: self.doc.sel.as_ref(), alpha_lock: layer.m.alpha_lock > 0.5 };
        stroke.add(&mut ctx, s);
        let d = stroke.dirty;
        stroke.dirty = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
        if d[0] <= d[2] {
            self.comp.mark(d);
        }
    }
    fn stroke_end(&mut self) {
        let Some(mut st) = self.stroke.take() else { return };
        let li = st.layer;
        if let Some(raw) = st.vec_raw.take() {
            if !raw.is_empty() {
                let b = &self.brush;
                let path = VPath {
                    a: vector::fit(&raw, 0.6),
                    width: b.size,
                    color: [self.fg[0], self.fg[1], self.fg[2], 1.0],
                    opacity: b.opacity,
                    taper_in: b.taper,
                    taper_out: b.taper,
                    ..Default::default()
                };
                let l = &mut self.doc.layers[li];
                l.m.paths.push(path);
                l.reraster();
            }
            self.comp.all();
            return;
        }
        if st.vec_erase {
            return;
        }
        {
            let StrokeState { stroke, orig, .. } = &mut st;
            let layer = &mut self.doc.layers[li];
            let mut ctx = Ctx { tiles: &mut layer.t, orig, sel: self.doc.sel.as_ref(), alpha_lock: layer.m.alpha_lock > 0.5 };
            stroke.finish(&mut ctx);
            let d = stroke.dirty;
            if d[0] <= d[2] {
                self.comp.mark(d);
            }
        }
        self.doc.layers[li].t.prune();
    }

    // ── selection ────────────────────────────────────────────────────────────
    fn sel_combine(&mut self, newm: Vec<f32>, mode: u8) {
        let (w, h) = (self.doc.w, self.doc.h);
        let old = self.doc.sel.clone();
        let mut m = vec![0u8; w * h];
        for i in 0..w * h {
            let n = newm[i].clamp(0.0, 1.0);
            let o = match &old {
                Some(s) => s.m[i] as f32 / 255.0,
                None => {
                    if mode == 2 || mode == 3 {
                        1.0
                    } else {
                        0.0
                    }
                }
            };
            let v = match mode {
                1 => o.max(n),
                2 => o * (1.0 - n),
                3 => o.min(n),
                _ => n,
            };
            m[i] = (v * 255.0 + 0.5) as u8;
        }
        let mask = Mask { w, h, m: Arc::new(m) };
        self.doc.sel = if mask.bbox().is_some() { Some(mask) } else { None };
    }
    fn poly_mask(&self, pts: &[(f32, f32)]) -> Vec<f32> {
        let (w, h) = (self.doc.w, self.doc.h);
        let mut tiles = Tiles::new(w, h);
        let p = VPath { a: pts.iter().map(|&(x, y)| vector::Anchor { x, y, w: 1.0, ..Default::default() }).collect(), closed: true, stroke: false, fill: Some([1.0; 4]), ..Default::default() };
        p.raster(&mut tiles);
        let mut out = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                out[y * w + x] = tiles.getf(x as i32, y as i32)[3];
            }
        }
        out
    }
    fn sel_or_canvas(&self) -> [i32; 4] {
        match &self.doc.sel {
            Some(s) => s.bbox().unwrap_or([0, 0, 0, 0]),
            None => [0, 0, self.doc.w as i32, self.doc.h as i32],
        }
    }

    // ── layers ───────────────────────────────────────────────────────────────
    /// A straight-RGBA picture as a new raster layer, centred, shrunk to fit the canvas when `fit`.
    fn place_image(&mut self, mut img: image::RgbaImage, name: &str, fit: bool) -> usize {
        let (dw, dh) = (self.doc.w as u32, self.doc.h as u32);
        if fit && (img.width() > dw || img.height() > dh) {
            let k = (dw as f32 / img.width() as f32).min(dh as f32 / img.height() as f32);
            img = image::imageops::resize(&img, ((img.width() as f32 * k) as u32).max(1), ((img.height() as f32 * k) as u32).max(1), image::imageops::FilterType::Lanczos3);
        }
        let li = self.layer_add(0, name);
        let (w, h) = (img.width() as usize, img.height() as usize);
        let x0 = (dw as i32 - w as i32) / 2;
        let y0 = (dh as i32 - h as i32) / 2;
        self.doc.layers[li].t.put_rgba8(x0, y0, w, h, &img.into_raw());
        self.comp.all();
        li
    }
    fn layer_add(&mut self, kind: u8, name: &str) -> usize {
        self.push_undo();
        let at = if self.doc.layers.is_empty() { 0 } else { self.act() + 1 };
        let n = if name.is_empty() { format!("{} {}", if kind == 1 { "Vector" } else { "Layer" }, self.doc.layers.len() + 1) } else { name.to_string() };
        self.doc.layers.insert(at, Layer::new(&n, kind, self.doc.w, self.doc.h));
        self.doc.active = at;
        self.comp.all();
        at
    }

    fn layer_get(&self, i: usize, k: &str) -> Val {
        // a stale index (the stack changed this frame) reads as an empty, hidden layer rather than Unit
        let Some(l) = self.doc.layers.get(i) else { return if k == "name" { Val::Str(String::new()) } else { num(0.0) } };
        let m = &l.m;
        match k {
            "name" => Val::Str(m.name.clone()),
            "kind" => num(m.kind as f64),
            "visible" => num(m.visible as f64),
            "locked" => num(m.locked as f64),
            "alpha_lock" => num(m.alpha_lock as f64),
            "clip" => num(m.clip as f64),
            "opacity" => num(m.opacity as f64),
            "blend" => num(m.blend as f64),
            "tone" => num(m.tone as f64),
            "tone_lpi" => num(m.tone_lpi as f64),
            "tone_angle" => num(m.tone_angle as f64),
            "tone_shape" => num(m.tone_shape as f64),
            "tone_col" => num(pack_rgb(m.tone_col)),
            "paths" => num(m.paths.len() as f64),
            "empty" => b2n(l.t.is_empty()),
            _ => Val::Unit,
        }
    }
    fn layer_set(&mut self, i: usize, k: &str, v: &Val) {
        if i >= self.doc.layers.len() {
            return;
        }
        let f = match v {
            Val::Num(n) => *n as f32,
            Val::Str(s) => s.parse().unwrap_or(0.0),
            _ => 0.0,
        };
        let m = &mut self.doc.layers[i].m;
        match k {
            "name" => {
                if let Val::Str(s) = v {
                    m.name = s.clone();
                }
            }
            "visible" => m.visible = f,
            "locked" => m.locked = f,
            "alpha_lock" => m.alpha_lock = f,
            "clip" => m.clip = f,
            "opacity" => m.opacity = f.clamp(0.0, 1.0),
            "blend" => m.blend = (f.max(0.0) as u8).min(blend::BLEND_NAMES.len() as u8 - 1),
            "tone" => m.tone = f,
            "tone_lpi" => m.tone_lpi = f.clamp(5.0, 300.0),
            "tone_angle" => m.tone_angle = f,
            "tone_shape" => m.tone_shape = (f.max(0.0) as u8).min(6),
            "tone_col" => m.tone_col = unpack_rgb(f as f64),
            _ => return,
        }
        self.dirty = true;
        self.comp.all();
    }

    // ── fill / gradient ──────────────────────────────────────────────────────
    fn fill(&mut self, x: f32, y: f32, tol: f32, all: bool, expand: f32) -> Val {
        let li = self.act();
        if !self.editable(li) {
            return self.fail("the layer is locked");
        }
        let (w, h) = (self.doc.w, self.doc.h);
        if all {
            self.update_comp();
        }
        let region = {
            let comp = &self.comp.px;
            let lt = &self.doc.layers[li].t;
            let get = |xx: usize, yy: usize| -> [f32; 4] {
                if all {
                    unpack_pre(comp[yy * w + xx])
                } else {
                    lt.getf(xx as i32, yy as i32)
                }
            };
            flood(w, h, x as i32, y as i32, tol, &get)
        };
        let cov: Vec<f32> = if expand > 0.0 {
            let d = filters::edt(&region, w, h);
            d.iter().map(|&dd| (expand + 0.5 - dd).clamp(0.0, 1.0)).collect()
        } else {
            region.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect()
        };
        self.push_undo();
        let col = self.fg;
        let sel = self.doc.sel.clone();
        let layer = &mut self.doc.layers[li];
        let alock = layer.m.alpha_lock > 0.5;
        let b = cov.iter().enumerate().filter(|(_, c)| **c > 0.0).fold([i32::MAX, i32::MAX, i32::MIN, i32::MIN], |b, (i, _)| {
            let (xx, yy) = ((i % w) as i32, (i / w) as i32);
            [b[0].min(xx), b[1].min(yy), b[2].max(xx + 1), b[3].max(yy + 1)]
        });
        if b[0] > b[2] {
            return num(0.0);
        }
        layer.t.for_rect(b[0], b[1], b[2], b[3], |xx, yy, p| {
            let mut c = cov[yy as usize * w + xx as usize];
            if let Some(s) = &sel {
                c *= s.at(xx, yy);
            }
            if c <= 0.0 {
                return;
            }
            let d = tof(*p);
            *p = fromf(if alock { [d[0] + (col[0] * d[3] - d[0]) * c, d[1] + (col[1] * d[3] - d[1]) * c, d[2] + (col[2] * d[3] - d[2]) * c, d[3]] } else { over(d, [col[0] * c, col[1] * c, col[2] * c, c]) });
        });
        layer.t.prune();
        self.comp.mark(b);
        num(1.0)
    }

    #[allow(clippy::too_many_arguments)]
    fn grad_apply(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, kind: u8, rep: u8, opacity: f32, dither: f32) -> Val {
        let li = self.act();
        if !self.editable(li) {
            return self.fail("the layer is locked");
        }
        if self.doc.layers[li].m.kind == 1 {
            return self.fail("gradients on vector layers: select a path and use Fill with Gradient");
        }
        self.push_undo();
        let r = self.sel_or_canvas();
        let g = self.grad.clone();
        let sel = self.doc.sel.clone();
        let layer = &mut self.doc.layers[li];
        let alock = layer.m.alpha_lock > 0.5;
        layer.t.for_rect(r[0], r[1], r[2], r[3], |x, y, p| {
            let k = match &sel {
                Some(s) => s.at(x, y),
                None => 1.0,
            };
            if k <= 0.0 {
                return;
            }
            let t = gradient::param(kind, rep, x0, y0, x1, y1, x as f32 + 0.5, y as f32 + 0.5);
            let mut c = g.eval(t);
            if dither > 0.0 {
                let n = (52.982_918 * (0.067_110_56 * x as f32 + 0.005_837_15 * y as f32).fract()).fract() - 0.5;
                for v in c.iter_mut().take(3) {
                    *v += n * dither / 128.0;
                }
            }
            let a = c[3] * opacity * k;
            let d = tof(*p);
            *p = fromf(if alock { [d[0] + (c[0] * d[3] - d[0]) * a, d[1] + (c[1] * d[3] - d[1]) * a, d[2] + (c[2] * d[3] - d[2]) * a, d[3]] } else { over(d, [c[0] * a, c[1] * a, c[2] * a, a]) });
        });
        layer.t.prune();
        self.comp.mark(r);
        num(1.0)
    }

    // ── free transform ───────────────────────────────────────────────────────
    fn xf_begin(&mut self) -> Val {
        if self.xf.is_some() {
            return num(1.0);
        }
        let li = self.act();
        if !self.editable(li) {
            return self.fail("the layer is locked");
        }
        self.push_undo();
        let (w, h) = (self.doc.w, self.doc.h);
        let sel = self.doc.sel.clone();
        let layer = &mut self.doc.layers[li];
        let vector = layer.m.kind == 1;
        let mut src = Tiles::new(w, h);
        if vector {
            src = layer.t.clone();
            layer.t.clear();
        } else {
            let r = match &sel {
                Some(s) => s.bbox().unwrap_or([0, 0, 0, 0]),
                None => layer.t.bbox().unwrap_or([0, 0, 0, 0]),
            };
            let lt = layer.t.clone();
            src.for_rect(r[0], r[1], r[2], r[3], |x, y, p| {
                let k = sel.as_ref().map_or(1.0, |s| s.at(x, y));
                if k > 0.0 {
                    let c = lt.getf(x, y);
                    *p = fromf([c[0] * k, c[1] * k, c[2] * k, c[3] * k]);
                }
            });
            layer.t.for_rect(r[0], r[1], r[2], r[3], |x, y, p| {
                let k = sel.as_ref().map_or(1.0, |s| s.at(x, y));
                if k > 0.0 {
                    let c = tof(*p);
                    let q = 1.0 - k;
                    *p = fromf([c[0] * q, c[1] * q, c[2] * q, c[3] * q]);
                }
            });
            src.prune();
            layer.t.prune();
        }
        let Some(b) = src.bbox() else {
            self.undo.pop();
            return self.fail("nothing to transform on this layer");
        };
        self.xf = Some(Xf { layer: li, src, bbox: [b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32], tx: 0.0, ty: 0.0, sx: 1.0, sy: 1.0, rot: 0.0, vector });
        self.comp.mark(b);
        num(1.0)
    }
    fn xf_commit(&mut self) {
        let Some(xf) = self.xf.take() else { return };
        let layer = &mut self.doc.layers[xf.layer];
        if xf.vector {
            let (cx, cy) = xf.center();
            let (s, c) = xf.rot.to_radians().sin_cos();
            let lin = |x: f32, y: f32| {
                let lx = x * xf.sx;
                let ly = y * xf.sy;
                (lx * c - ly * s, lx * s + ly * c)
            };
            let k = (xf.sx.abs() * xf.sy.abs()).sqrt();
            for p in layer.m.paths.iter_mut() {
                for a in p.a.iter_mut() {
                    let (nx, ny) = xf.fwd(a.x, a.y);
                    let (ix, iy) = lin(a.ix, a.iy);
                    let (ox, oy) = lin(a.ox, a.oy);
                    *a = vector::Anchor { x: nx, y: ny, ix, iy, ox, oy, w: a.w };
                }
                p.width *= k;
                let _ = (cx, cy);
            }
            layer.reraster();
        } else {
            let ob = xf.out_bbox();
            layer.t.for_rect(ob[0], ob[1], ob[2], ob[3], |x, y, p| {
                let f = xf.sample(x, y);
                if f[3] > 0.0 {
                    *p = fromf(over(tof(*p), f));
                }
            });
            layer.t.prune();
            // the selection travels with the pixels
            if let Some(sel) = self.doc.sel.clone() {
                let (w, h) = (sel.w, sel.h);
                let mut m = vec![0u8; w * h];
                for y in ob[1].max(0)..ob[3].min(h as i32) {
                    for x in ob[0].max(0)..ob[2].min(w as i32) {
                        let (u, v) = xf.inv(x as f32 + 0.5, y as f32 + 0.5);
                        m[y as usize * w + x as usize] = (sel.at(u as i32, v as i32) * 255.0) as u8;
                    }
                }
                let nm = Mask { w, h, m: Arc::new(m) };
                self.doc.sel = if nm.bbox().is_some() { Some(nm) } else { None };
            }
        }
        self.comp.all();
    }
    fn xf_cancel(&mut self) {
        if self.xf.take().is_some() {
            if let Some(d) = self.undo.pop() {
                self.doc = d;
            }
            self.comp.all();
        }
    }

    // ── scene glue ───────────────────────────────────────────────────────────
    fn img_from_layer(&self, li: usize) -> Option<(Img, [i32; 4])> {
        let l = self.doc.layers.get(li)?;
        let b = l.t.bbox()?;
        let (w, h) = ((b[2] - b[0]) as usize, (b[3] - b[1]) as usize);
        let mut px = Vec::with_capacity(w * h);
        let cell = (self.doc.dpi / l.m.tone_lpi.max(1.0)).max(2.0);
        let (sa, ca) = l.m.tone_angle.to_radians().sin_cos();
        for y in 0..h {
            for x in 0..w {
                let mut c = l.t.getf(b[0] + x as i32, b[1] + y as i32);
                if l.m.tone > 0.5 && c[3] > 0.0 {
                    // screentone layers become parts as their dots, the way they look on the canvas
                    let u = unpre(c);
                    let k = u[3] * (1.0 - lum(u[0], u[1], u[2]));
                    let cv = filters::tone_cov(l.m.tone_shape, cell, sa, ca, (b[0] + x as i32) as f32 + 0.5, (b[1] + y as i32) as f32 + 0.5, k);
                    let tc = l.m.tone_col;
                    c = [tc[0] * cv, tc[1] * cv, tc[2] * cv, cv];
                }
                let op = l.m.opacity;
                px.push([(c[0] * op * 255.0 + 0.5) as u8, (c[1] * op * 255.0 + 0.5) as u8, (c[2] * op * 255.0 + 0.5) as u8, (c[3] * op * 255.0 + 0.5) as u8]);
            }
        }
        Some((Img { w, h, px }, b))
    }
    fn split_apply(&mut self, job: SplitJob, r: split::SplitResult) {
        self.push_undo();
        let (dw, dh) = (self.doc.w, self.doc.h);
        let li = job.layer.min(self.doc.layers.len().saturating_sub(1));
        let base = self.doc.layers.get(li).map(|l| l.m.name.clone()).unwrap_or_default();
        let mut at = li + 1;
        let mut bg_layer = None;
        let n = r.layers.len();
        for ol in &r.layers {
            let mut l = Layer::new(&ol.name, 0, dw, dh);
            l.m.blend = ol.blend;
            let (x0, y0, w) = (job.x0 + ol.x0, job.y0 + ol.y0, ol.w);
            let px = &ol.px;
            l.t.for_rect(x0, y0, x0 + ol.w as i32, y0 + ol.h as i32, |x, y, p| *p = px[(y - y0) as usize * w + (x - x0) as usize]);
            l.t.prune();
            if ol.group == 0 && bg_layer.is_none() {
                bg_layer = Some(at);
            }
            self.doc.layers.insert(at, l);
            at += 1;
        }
        if let Some(src) = self.doc.layers.get_mut(li) {
            src.m.visible = 0.0;
        }
        self.doc.active = at - 1;
        // the filled-in area, as a document-size selection mask
        self.split_hole = match (&r.hole, bg_layer) {
            (Some(hole), Some(bl)) => {
                let (rw, rh) = (r.region_w, r.region_h);
                let mut m = vec![0u8; dw * dh];
                for y in 0..rh {
                    for x in 0..rw {
                        let (dx, dy) = (job.x0 + x as i32, job.y0 + y as i32);
                        if dx >= 0 && dy >= 0 && (dx as usize) < dw && (dy as usize) < dh && hole[y * rw + x] > 0.5 {
                            m[dy as usize * dw + dx as usize] = 255;
                        }
                    }
                }
                Some((Mask { w: dw, h: dh, m: Arc::new(m) }, bl))
            }
            _ => None,
        };
        let mut parts = 0;
        if job.to_stage && !r.parts.is_empty() {
            let maxd = r.parts.iter().map(|p| p.depth).max().unwrap_or(0) as f32;
            for p in &r.parts {
                let img = Img::from_straight(p.w, p.h, &p.rgba);
                let cx = (job.x0 + p.x0) as f32 + p.w as f32 * 0.5;
                let cy = (job.y0 + p.y0) as f32 + p.h as f32 * 0.5;
                let name = if base.is_empty() { p.name.clone() } else { format!("{base} - {}", p.name) };
                self.doc.scene.parts.push(Part::new(&name, img, cx, cy));
                let pi = self.doc.scene.parts.len() - 1;
                // background furthest, nearer objects closer to the camera
                let z = job.spread * (maxd - p.depth as f32) - job.spread * maxd * 0.5;
                self.doc.scene.set_depth_keep(pi, z);
                parts += 1;
            }
        }
        self.comp.all();
        self.dirty = true;
        self.split_msg = format!("{n} layers{} - {}", if parts > 0 { format!(", {parts} stage parts") } else { String::new() }, r.note);
    }

    fn part_from_layer(&mut self, li: usize) -> i32 {
        let Some((img, b)) = self.img_from_layer(li) else { return -1 };
        let name = self.doc.layers[li].m.name.clone();
        let p = Part::new(&name, img, (b[0] + b[2]) as f32 * 0.5, (b[1] + b[3]) as f32 * 0.5);
        self.doc.scene.parts.push(p);
        (self.doc.scene.parts.len() - 1) as i32
    }

    fn sd_poll(&mut self) -> Val {
        let mut msgs = Vec::new();
        if let Some(rx) = &self.sd.rx {
            while let Ok(m) = rx.try_recv() {
                msgs.push(m);
            }
        }
        for m in msgs {
            match m {
                sd::SdMsg::Models(v) => {
                    self.sd.models = v;
                    self.sd.state = 0;
                    self.sd.msg = format!("{} checkpoints on the server", self.sd.models.len());
                }
                sd::SdMsg::Samplers(v) => self.sd.samplers = v,
                sd::SdMsg::Progress(p, s) => {
                    if self.sd.state == 1 {
                        self.sd.progress = p;
                        self.sd.msg = s;
                    }
                }
                sd::SdMsg::Error(e) => {
                    self.sd.state = 3;
                    self.sd.msg = e;
                    self.sd.rx = None;
                }
                sd::SdMsg::Images(imgs) => {
                    self.sd.rx = None;
                    self.sd.state = 2;
                    self.sd.progress = 1.0;
                    if let Some((x, y, w, h, _mode, name)) = self.sd.place.take() {
                        match image::load_from_memory(&imgs[0]) {
                            Ok(img) => {
                                let img = image::imageops::resize(&img.to_rgba8(), w as u32, h as u32, image::imageops::FilterType::Lanczos3);
                                let li = self.layer_add(0, &name);
                                let sel = self.doc.sel.clone();
                                let raw = img.into_raw();
                                let t = &mut self.doc.layers[li].t;
                                t.put_rgba8(x, y, w, h, &raw);
                                if let Some(s) = sel {
                                    t.for_rect(x, y, x + w as i32, y + h as i32, |xx, yy, p| {
                                        let k = s.at(xx, yy);
                                        let c = tof(*p);
                                        *p = fromf([c[0] * k, c[1] * k, c[2] * k, c[3] * k]);
                                    });
                                    t.prune();
                                }
                                self.comp.all();
                                self.sd.msg = format!("generated into the new layer \"{name}\"");
                            }
                            Err(e) => {
                                self.sd.state = 3;
                                self.sd.msg = format!("could not decode the image: {e}");
                            }
                        }
                    }
                }
            }
        }
        Val::nums(vec![self.sd.state as f32, self.sd.progress])
    }

    /// LingOS: no Stable Diffusion server to talk to
    #[cfg(not(feature = "std"))]
    #[allow(clippy::too_many_arguments)]
    fn sd_generate(&mut self, _mode: u8, _prompt: String, _neg: String, _steps: u32, _cfg: f32, _denoise: f32, _seed: i64, _sampler: String, _model: String, _maxside: f32) -> Val {
        self.sd.state = 3;
        self.sd.msg = sd::UNAVAILABLE.into();
        self.fail(sd::UNAVAILABLE)
    }

    #[cfg(feature = "std")]
    #[allow(clippy::too_many_arguments)]
    fn sd_generate(&mut self, mode: u8, prompt: String, neg: String, steps: u32, cfg: f32, denoise: f32, seed: i64, sampler: String, model: String, maxside: f32) -> Val {
        if self.sd.state == 1 {
            return self.fail("a generation is already running");
        }
        let (w, h) = (self.doc.w as i32, self.doc.h as i32);
        let r = self.sel_or_canvas();
        let pad = if self.doc.sel.is_some() { 12 } else { 0 };
        let r = [(r[0] - pad).max(0), (r[1] - pad).max(0), (r[2] + pad).min(w), (r[3] + pad).min(h)];
        let (rw, rh) = ((r[2] - r[0]).max(8) as usize, (r[3] - r[1]).max(8) as usize);
        let k = maxside / rw.max(rh) as f32;
        let gw = (((rw as f32 * k) / 8.0).round() as u32 * 8).max(64);
        let gh = (((rh as f32 * k) / 8.0).round() as u32 * 8).max(64);
        let mut init = None;
        let mut mask = None;
        if mode >= 1 {
            self.update_comp();
            let mut rgb = image::RgbImage::new(rw as u32, rh as u32);
            for y in 0..rh {
                for x in 0..rw {
                    let c = unpack_pre(self.comp.px[(r[1] as usize + y) * self.doc.w + r[0] as usize + x]);
                    // flattened over white so transparent areas do not read as black
                    let q = |v: f32| ((v + (1.0 - c[3])) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
                    rgb.put_pixel(x as u32, y as u32, image::Rgb([q(c[0]), q(c[1]), q(c[2])]));
                }
            }
            let rgb = image::imageops::resize(&rgb, gw, gh, image::imageops::FilterType::Lanczos3);
            let mut buf = std::io::Cursor::new(Vec::new());
            if let Err(e) = rgb.write_to(&mut buf, image::ImageFormat::Png) {
                return self.fail(e.to_string());
            }
            init = Some(buf.into_inner());
            if mode == 2 {
                let mut m = image::GrayImage::new(rw as u32, rh as u32);
                for y in 0..rh {
                    for x in 0..rw {
                        let v = match &self.doc.sel {
                            Some(s) => s.at(r[0] + x as i32, r[1] + y as i32),
                            None => 1.0,
                        };
                        m.put_pixel(x as u32, y as u32, image::Luma([(v * 255.0) as u8]));
                    }
                }
                let m = image::imageops::resize(&m, gw, gh, image::imageops::FilterType::Triangle);
                let mut buf = std::io::Cursor::new(Vec::new());
                if m.write_to(&mut buf, image::ImageFormat::Png).is_ok() {
                    mask = Some(buf.into_inner());
                }
            }
        }
        let short: String = prompt.chars().take(28).collect();
        self.sd.place = Some((r[0], r[1], rw, rh, mode, format!("AI: {short}")));
        self.sd.rx = Some(sd::generate(sd::GenReq { base: self.sd.url.clone(), prompt, negative: neg, steps, cfg, denoise, seed, sampler, model, w: gw, h: gh, init, mask }));
        self.sd.state = 1;
        self.sd.progress = 0.0;
        self.sd.msg = format!("generating {gw}x{gh}...");
        num(1.0)
    }

    /// LingOS: frames and sprite sheets render here and now (no threads); video needs ffmpeg, which LingOS has not
    #[cfg(not(feature = "std"))]
    fn export_start(&mut self, dir: String, mode: u8, fps: f32, t0: f32, t1: f32, scale: f32) -> Val {
        if mode != 0 && mode != 3 {
            let mut e = self.export.lock().unwrap();
            *e = Export { done: 0, total: 0, running: false, msg: "export failed: video export (MP4 / WebM / GIF / MOV) needs ffmpeg, which is not available on LingOS - export PNG frames or a sprite sheet instead".into() };
            return num(1.0);
        }
        let mut sc = self.doc.scene.clone();
        let n = (((t1 - t0) * fps).round() as usize).max(1);
        let res: Result<String, String> = (|| {
            os::mkdir_all(&dir);
            let mut sheet: Vec<(usize, usize, Vec<u8>)> = Vec::new();
            for i in 0..n {
                let t = t0 + i as f32 / fps;
                let (w, h, rgba) = sc.frame(t, scale);
                if mode == 3 {
                    sheet.push((w, h, rgba));
                } else {
                    let p = os::join(&dir, &format!("frame_{:05}.png", i + 1));
                    image::save_buffer(&p, &rgba, w as u32, h as u32, image::ExtendedColorType::Rgba8).map_err(|e| e.to_string())?;
                }
            }
            if mode == 0 {
                return Ok(format!("{n} PNG frames in {dir}"));
            }
            let (fw, fh) = (sheet[0].0, sheet[0].1);
            let cols = (n as f32).sqrt().ceil() as usize;
            let rows = n.div_ceil(cols);
            let mut img = image::RgbaImage::new((fw * cols) as u32, (fh * rows) as u32);
            let mut json = String::from("{\"frames\":[");
            for (i, (_, _, rgba)) in sheet.iter().enumerate() {
                let (cx, cy) = ((i % cols) * fw, (i / cols) * fh);
                for y in 0..fh {
                    for x in 0..fw {
                        let o = (y * fw + x) * 4;
                        img.put_pixel((cx + x) as u32, (cy + y) as u32, image::Rgba([rgba[o], rgba[o + 1], rgba[o + 2], rgba[o + 3]]));
                    }
                }
                json += &format!("{}{{\"x\":{cx},\"y\":{cy},\"w\":{fw},\"h\":{fh},\"t\":{:.4}}}", if i > 0 { "," } else { "" }, i as f32 / fps);
            }
            json += &format!("],\"fps\":{fps},\"cols\":{cols},\"rows\":{rows}}}");
            img.save(&os::join(&dir, "sheet.png")).map_err(|e| e.to_string())?;
            os::write(&os::join(&dir, "sheet.json"), json.as_bytes())?;
            Ok(format!("sprite sheet {cols}x{rows} in {dir}"))
        })();
        let mut e = self.export.lock().unwrap();
        *e = Export { done: n, total: n, running: false, msg: match res {
            Ok(m) => m,
            Err(m) => format!("export failed: {m}"),
        } };
        num(1.0)
    }

    #[cfg(feature = "std")]
    fn export_start(&mut self, dir: String, mode: u8, fps: f32, t0: f32, t1: f32, scale: f32) -> Val {
        let running = self.export.lock().unwrap().running;
        if running {
            return self.fail("an export is already running");
        }
        let mut sc = self.doc.scene.clone();
        let st = self.export.clone();
        let n = (((t1 - t0) * fps).round() as usize).max(1);
        {
            let mut e = st.lock().unwrap();
            *e = Export { done: 0, total: n, running: true, msg: "rendering...".into() };
        }
        std::thread::spawn(move || {
            let res: Result<String, String> = (|| {
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let frames_dir = if mode == 0 || mode == 3 { std::path::PathBuf::from(&dir) } else { std::path::Path::new(&dir).join("frames") };
                std::fs::create_dir_all(&frames_dir).map_err(|e| e.to_string())?;
                let mut sheet: Vec<(usize, usize, Vec<u8>)> = Vec::new();
                for i in 0..n {
                    let t = t0 + i as f32 / fps;
                    let (w, h, rgba) = sc.frame(t, scale);
                    if mode == 3 {
                        sheet.push((w, h, rgba));
                    } else {
                        let p = frames_dir.join(format!("frame_{:05}.png", i + 1));
                        image::save_buffer(&p, &rgba, w as u32, h as u32, image::ExtendedColorType::Rgba8).map_err(|e| e.to_string())?;
                    }
                    st.lock().unwrap().done = i + 1;
                }
                match mode {
                    0 => Ok(format!("{n} PNG frames in {dir}")),
                    3 => {
                        let (fw, fh) = (sheet[0].0, sheet[0].1);
                        let cols = (n as f32).sqrt().ceil() as usize;
                        let rows = n.div_ceil(cols);
                        let mut img = image::RgbaImage::new((fw * cols) as u32, (fh * rows) as u32);
                        let mut json = String::from("{\"frames\":[");
                        for (i, (_, _, rgba)) in sheet.iter().enumerate() {
                            let (cx, cy) = ((i % cols) * fw, (i / cols) * fh);
                            for y in 0..fh {
                                for x in 0..fw {
                                    let o = (y * fw + x) * 4;
                                    img.put_pixel((cx + x) as u32, (cy + y) as u32, image::Rgba([rgba[o], rgba[o + 1], rgba[o + 2], rgba[o + 3]]));
                                }
                            }
                            json += &format!("{}{{\"x\":{cx},\"y\":{cy},\"w\":{fw},\"h\":{fh},\"t\":{:.4}}}", if i > 0 { "," } else { "" }, i as f32 / fps);
                        }
                        json += &format!("],\"fps\":{fps},\"cols\":{cols},\"rows\":{rows}}}");
                        img.save(std::path::Path::new(&dir).join("sheet.png")).map_err(|e| e.to_string())?;
                        std::fs::write(std::path::Path::new(&dir).join("sheet.json"), json).map_err(|e| e.to_string())?;
                        Ok(format!("sprite sheet {cols}x{rows} in {dir}"))
                    }
                    _ => {
                        let (out, args): (&str, Vec<&str>) = match mode {
                            2 => ("animation.webm", vec!["-c:v", "libvpx-vp9", "-pix_fmt", "yuva420p", "-b:v", "0", "-crf", "24"]),
                            4 => ("animation.gif", vec!["-vf", "split[a][b];[a]palettegen=reserve_transparent=1[p];[b][p]paletteuse"]),
                            5 => ("animation.mov", vec!["-c:v", "prores_ks", "-profile:v", "4", "-pix_fmt", "yuva444p10le"]),
                            _ => ("animation.mp4", vec!["-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "16", "-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2"]),
                        };
                        let outp = std::path::Path::new(&dir).join(out);
                        let mut cmd = std::process::Command::new("ffmpeg");
                        cmd.arg("-y").arg("-framerate").arg(format!("{fps}")).arg("-i").arg(frames_dir.join("frame_%05d.png"));
                        cmd.args(&args).arg(&outp);
                        #[cfg(windows)]
                        {
                            use std::os::windows::process::CommandExt;
                            cmd.creation_flags(0x0800_0000);
                        }
                        let o = cmd.output().map_err(|_| format!("ffmpeg was not found on PATH - the PNG frames are in {}", frames_dir.display()))?;
                        if o.status.success() {
                            Ok(format!("wrote {}", outp.display()))
                        } else {
                            Err(format!("ffmpeg failed: {}", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("")))
                        }
                    }
                }
            })();
            let mut e = st.lock().unwrap();
            e.running = false;
            e.msg = match res {
                Ok(m) => m,
                Err(m) => format!("export failed: {m}"),
            };
        });
        num(1.0)
    }

    // ── drawing into the host framebuffer ───────────────────────────────────
    /// calls that paint pixels: art_view_draw, art_sc_draw, art_layer_thumb, art_part_thumb, art_draw_sv,
    /// art_draw_hue, art_draw_grad, art_draw_brush
    pub fn draw(&mut self, name: &str, a: &[Val], fb: &mut Fb) -> Option<Result<Val, String>> {
        let rect = |i: usize| -> [i32; 4] {
            let x = an(a, i, 0.0) as i32;
            let y = an(a, i + 1, 0.0) as i32;
            [x, y, x + an(a, i + 2, 0.0) as i32, y + an(a, i + 3, 0.0) as i32]
        };
        let clipr = |r: [i32; 4], fb: &Fb| [r[0].max(fb.clip[0]).max(0), r[1].max(fb.clip[1]).max(0), r[2].min(fb.clip[2]).min(fb.w as i32), r[3].min(fb.clip[3]).min(fb.h as i32)];
        let tag = fb.tag;
        match name {
            "art_view_draw" => {
                self.update_comp();
                let v = self.view;
                let r = clipr([v.vx as i32, v.vy as i32, (v.vx + v.vw) as i32, (v.vy + v.vh) as i32], fb);
                if r[0] >= r[2] || r[1] >= r[3] {
                    return Some(Ok(Val::Unit));
                }
                let (cw, ch) = (self.comp.w as i32, self.comp.h as i32);
                let comp = &self.comp.px;
                let (x0c, y0c) = v.s2c(0.0, 0.0);
                let (x1c, y1c) = v.s2c(1.0, 0.0);
                let (x2c, y2c) = v.s2c(0.0, 1.0);
                let (dux, dvx) = (x1c - x0c, y1c - y0c);
                let (duy, dvy) = (x2c - x0c, y2c - y0c);
                let zoom = v.zoom;
                let sel = self.doc.sel.as_ref();
                let ants = ((self.now() * 8.0) as i32) & 7;
                let paper_bg = 0u32;
                let _ = paper_bg;
                let grid = v.grid && zoom >= 10.0;
                let fbw = fb.w;
                let sample = |u: f32, vv: f32| -> Option<[f32; 4]> {
                    if u < 0.0 || vv < 0.0 || u >= cw as f32 || vv >= ch as f32 {
                        return None;
                    }
                    if zoom >= 1.0 {
                        return Some(unpack_pre(comp[vv as usize * cw as usize + u as usize]));
                    }
                    let bil = |u: f32, vv: f32| {
                        let fx = (u - 0.5).max(0.0);
                        let fy = (vv - 0.5).max(0.0);
                        let xi = (fx as i32).min(cw - 1);
                        let yi = (fy as i32).min(ch - 1);
                        let xj = (xi + 1).min(cw - 1);
                        let yj = (yi + 1).min(ch - 1);
                        let tx = fx - xi as f32;
                        let ty = fy - yi as f32;
                        let g = |x: i32, y: i32| unpack_pre(comp[y as usize * cw as usize + x as usize]);
                        lerp4(lerp4(g(xi, yi), g(xj, yi), tx), lerp4(g(xi, yj), g(xj, yj), tx), ty)
                    };
                    if zoom < 0.5 {
                        // 2x2 supersampling keeps zoomed-out line art from shimmering
                        let (ox, oy) = ((dux + duy) * 0.25, (dvx + dvy) * 0.25);
                        let a = bil(u - ox, vv - oy);
                        let b = bil(u + ox, vv - oy);
                        let c = bil(u - ox, vv + oy);
                        let d = bil(u + ox, vv + oy);
                        Some([(a[0] + b[0] + c[0] + d[0]) * 0.25, (a[1] + b[1] + c[1] + d[1]) * 0.25, (a[2] + b[2] + c[2] + d[2]) * 0.25, (a[3] + b[3] + c[3] + d[3]) * 0.25])
                    } else {
                        Some(bil(u, vv))
                    }
                };
                let draw_row = |y: usize, row: &mut [u32]| {
                    let sy = y as f32 + 0.5;
                    for x in r[0]..r[2] {
                        let sx = x as f32 + 0.5;
                        let u = x0c + dux * sx + duy * sy;
                        let vv = y0c + dvx * sx + dvy * sy;
                        let out = match sample(u, vv) {
                            None => 0x0e0f12,
                            Some(c) => {
                                let chk = if ((x >> 3) + (y as i32 >> 3)) & 1 == 0 { 0.80 } else { 0.92 };
                                let k = 1.0 - c[3];
                                let mut rgb = [c[0] + chk * k, c[1] + chk * k, c[2] + chk * k];
                                if grid {
                                    let fu = u.fract();
                                    let fv = vv.fract();
                                    if fu * zoom < 1.0 || fv * zoom < 1.0 {
                                        for c in rgb.iter_mut() {
                                            *c = *c * 0.82 + 0.09;
                                        }
                                    }
                                }
                                if let Some(s) = sel {
                                    let m0 = s.at(u as i32, vv as i32) > 0.5;
                                    let m1 = s.at((u + dux) as i32, (vv + dvx) as i32) > 0.5;
                                    let m2 = s.at((u + duy) as i32, (vv + dvy) as i32) > 0.5;
                                    if m0 != m1 || m0 != m2 {
                                        let on = ((x + y as i32 + ants) >> 2) & 1 == 0;
                                        rgb = if on { [0.0, 0.0, 0.0] } else { [1.0, 1.0, 1.0] };
                                    }
                                }
                                let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
                                (q(rgb[0]) << 16) | (q(rgb[1]) << 8) | q(rgb[2])
                            }
                        };
                        row[x as usize] = out | tag;
                    }
                };
                #[cfg(feature = "std")]
                fb.buf.par_chunks_mut(fbw).enumerate().skip(r[1] as usize).take((r[3] - r[1]) as usize).for_each(|(y, row)| draw_row(y, row));
                #[cfg(not(feature = "std"))]
                os::par_rows(fb.buf, fbw, r[1] as usize, r[3] as usize, &draw_row);
                Some(Ok(Val::Unit))
            }
            "art_sc_draw" => {
                let r0 = rect(0);
                let (vw, vh) = ((r0[2] - r0[0]).max(1) as usize, (r0[3] - r0[1]).max(1) as usize);
                let sc = &self.doc.scene;
                let k = (vw as f32 / sc.w).min(vh as f32 / sc.h) * af(a, 4, 1.0).max(0.05);
                let ox = (vw as f32 - sc.w * k) * 0.5 + af(a, 5, 0.0);
                let oy = (vh as f32 - sc.h * k) * 0.5 + af(a, 6, 0.0);
                self.sc_map = (k, ox + r0[0] as f32, oy + r0[1] as f32, [r0[0] as f32, r0[1] as f32, vw as f32, vh as f32]);
                let mut buf = vec![[0.0f32; 4]; vw * vh];
                sc.render(&mut buf, vw, vh, k, ox, oy);
                let r = clipr(r0, fb);
                let (fx0, fy0, fx1, fy1) = (ox, oy, ox + sc.w * k, oy + sc.h * k);
                for y in r[1]..r[3] {
                    let ly = (y - r0[1]) as usize;
                    for x in r[0]..r[2] {
                        let lx = (x - r0[0]) as usize;
                        let c = buf[ly * vw + lx];
                        let inside = (lx as f32) >= fx0 && (lx as f32) < fx1 && (ly as f32) >= fy0 && (ly as f32) < fy1;
                        let chk = if inside {
                            if ((x >> 3) + (y >> 3)) & 1 == 0 {
                                0.30
                            } else {
                                0.36
                            }
                        } else {
                            0.06
                        };
                        let kk = 1.0 - c[3];
                        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
                        fb.buf[y as usize * fb.w + x as usize] = (q(c[0] + chk * kk) << 16) | (q(c[1] + chk * kk) << 8) | q(c[2] + chk * kk) | tag;
                    }
                }
                Some(Ok(Val::Unit))
            }
            "art_layer_thumb" | "art_part_thumb" => {
                let i = an(a, 0, 0.0).max(0.0) as usize;
                let r0 = rect(1);
                let (tw, th) = ((r0[2] - r0[0]).max(1) as f32, (r0[3] - r0[1]).max(1) as f32);
                let r = clipr(r0, fb);
                if name == "art_layer_thumb" {
                    let Some(l) = self.doc.layers.get(i) else { return Some(Ok(Val::Unit)) };
                    let (dw, dh) = (self.doc.w as f32, self.doc.h as f32);
                    let k = (tw / dw).min(th / dh);
                    let (ox, oy) = ((tw - dw * k) * 0.5, (th - dh * k) * 0.5);
                    for y in r[1]..r[3] {
                        for x in r[0]..r[2] {
                            let lx = (x - r0[0]) as f32 - ox;
                            let ly = (y - r0[1]) as f32 - oy;
                            let inside = lx >= 0.0 && ly >= 0.0 && lx < dw * k && ly < dh * k;
                            if !inside {
                                continue;
                            }
                            let c = l.t.getf((lx / k) as i32, (ly / k) as i32);
                            let chk = if ((x >> 2) + (y >> 2)) & 1 == 0 { 0.82 } else { 0.95 };
                            let kk = 1.0 - c[3];
                            let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
                            fb.buf[y as usize * fb.w + x as usize] = (q(c[0] + chk * kk) << 16) | (q(c[1] + chk * kk) << 8) | q(c[2] + chk * kk) | tag;
                        }
                    }
                } else {
                    let Some(p) = self.doc.scene.parts.get(i) else { return Some(Ok(Val::Unit)) };
                    let img = p.img.clone();
                    let (dw, dh) = (img.w.max(1) as f32, img.h.max(1) as f32);
                    let k = (tw / dw).min(th / dh);
                    let (ox, oy) = ((tw - dw * k) * 0.5, (th - dh * k) * 0.5);
                    for y in r[1]..r[3] {
                        for x in r[0]..r[2] {
                            let lx = (x - r0[0]) as f32 - ox;
                            let ly = (y - r0[1]) as f32 - oy;
                            if lx < 0.0 || ly < 0.0 || lx >= dw * k || ly >= dh * k {
                                continue;
                            }
                            let c = img.sample((lx / k) / dw, (ly / k) / dh);
                            let d = unpack_pre(0xff00_0000 | (fb.buf[y as usize * fb.w + x as usize] & 0xffffff));
                            let o = over(d, c);
                            fb.buf[y as usize * fb.w + x as usize] = (pack_pre(o) & 0xffffff) | tag;
                        }
                    }
                }
                Some(Ok(Val::Unit))
            }
            "art_draw_sv" | "art_draw_hue" | "art_draw_grad" => {
                let r0 = rect(0);
                let r = clipr(r0, fb);
                let (w, h) = ((r0[2] - r0[0]).max(1) as f32, (r0[3] - r0[1]).max(1) as f32);
                let hue = af(a, 4, 0.0);
                for y in r[1]..r[3] {
                    for x in r[0]..r[2] {
                        let u = (x - r0[0]) as f32 / w;
                        let v = (y - r0[1]) as f32 / h;
                        let rgb = match name {
                            "art_draw_sv" => hsv2rgb([hue, u, 1.0 - v]),
                            "art_draw_hue" => hsv2rgb([v, 1.0, 1.0]),
                            _ => {
                                let c = self.grad.eval(u);
                                let chk = if ((x >> 3) + (y >> 3)) & 1 == 0 { 0.55 } else { 0.75 };
                                [c[0] * c[3] + chk * (1.0 - c[3]), c[1] * c[3] + chk * (1.0 - c[3]), c[2] * c[3] + chk * (1.0 - c[3])]
                            }
                        };
                        fb.buf[y as usize * fb.w + x as usize] = pack_pre([rgb[0], rgb[1], rgb[2], 1.0]) & 0xffffff | tag;
                    }
                }
                Some(Ok(Val::Unit))
            }
            "art_draw_brush" => {
                let r0 = rect(0);
                let (w, h) = ((r0[2] - r0[0]).max(1) as usize, (r0[3] - r0[1]).max(1) as usize);
                let col = unpack_rgb(an(a, 4, 16777215.0));
                let key = format!("{}|{:?}", serde_json::to_string(&self.brush).unwrap_or_default(), col);
                let fresh = !matches!(&self.brush_prev, Some((k, pw, ph, _)) if *k == key && *pw == w && *ph == h);
                if fresh {
                    let mut b = self.brush.clone();
                    if b.mode == 1 || b.mode >= 2 {
                        b.mode = 0;
                    }
                    let t = brush::preview(&b, w, h, col);
                    let mut px = vec![[0.0f32; 4]; w * h];
                    for y in 0..h {
                        for x in 0..w {
                            px[y * w + x] = t.getf(x as i32, y as i32);
                        }
                    }
                    self.brush_prev = Some((key, w, h, px));
                }
                let px = &self.brush_prev.as_ref().unwrap().3;
                let r = clipr(r0, fb);
                for y in r[1]..r[3] {
                    for x in r[0]..r[2] {
                        let c = px[(y - r0[1]) as usize * w + (x - r0[0]) as usize];
                        if c[3] <= 0.0 {
                            continue;
                        }
                        let i = y as usize * fb.w + x as usize;
                        let d = unpack_pre(0xff00_0000 | (fb.buf[i] & 0xffffff));
                        fb.buf[i] = (pack_pre(over(d, c)) & 0xffffff) | tag;
                    }
                }
                Some(Ok(Val::Unit))
            }
            _ => None,
        }
    }

    pub fn last_error(&self) -> &str {
        &self.err
    }

    // ── the named calls ──────────────────────────────────────────────────────
    pub fn call(&mut self, name: &str, a: &[Val]) -> Option<Result<Val, String>> {
        let unit = Val::Unit;
        let r: Val = match name {
            // documents
            "art_doc_new" => {
                let w = an(a, 0, 1920.0).clamp(16.0, 20000.0) as usize;
                let h = an(a, 1, 1080.0).clamp(16.0, 20000.0) as usize;
                if w * h > 220_000_000 {
                    return Some(Ok(self.fail("that canvas is too large (more than 220 megapixels)")));
                }
                let mut d = Doc::new(w, h, af(a, 2, 144.0).max(1.0), &as_(a, 3));
                d.paper = unpack_rgb(an(a, 4, 16777215.0));
                d.paper_on = an(a, 5, 1.0) > 0.5;
                self.doc = d;
                self.undo.clear();
                self.redo.clear();
                self.xf = None;
                self.stroke = None;
                self.reset_comp();
                self.view.cx = w as f32 * 0.5;
                self.view.cy = h as f32 * 0.5;
                self.fit_view();
                self.dirty = false;
                num(1.0)
            }
            "art_doc_info" => Val::nums(vec![self.doc.w as f32, self.doc.h as f32, self.doc.dpi, self.doc.layers.len() as f32, self.act() as f32, if self.dirty { 1.0 } else { 0.0 }]),
            "art_doc_name" => Val::Str(self.doc.name.clone()),
            "art_doc_rename" => {
                self.doc.name = as_(a, 0);
                unit
            }
            "art_doc_meta_get" => Val::Str(self.doc.meta.clone()),
            "art_doc_meta_set" => {
                self.doc.meta = as_(a, 0);
                unit
            }
            "art_doc_dpi" => {
                self.doc.dpi = af(a, 0, 144.0).max(1.0);
                self.comp.all();
                unit
            }
            "art_dirty" => b2n(self.dirty),
            "art_set_clean" => {
                self.dirty = false;
                unit
            }
            "art_error" => Val::Str(self.err.clone()),
            "art_doc_save" => {
                let p = as_(a, 0);
                match io::save(self, &p) {
                    Ok(()) => {
                        self.dirty = false;
                        num(1.0)
                    }
                    Err(e) => self.fail(e),
                }
            }
            "art_doc_save_copy" => match io::save(self, &as_(a, 0)) {
                // autosave / backups: the document keeps its unsaved state
                Ok(()) => num(1.0),
                Err(e) => self.fail(e),
            },
            "art_doc_open" => {
                let p = as_(a, 0);
                match io::open(&p) {
                    Ok(d) => {
                        self.doc = d;
                        self.undo.clear();
                        self.redo.clear();
                        self.xf = None;
                        self.reset_comp();
                        self.fit_view();
                        self.dirty = false;
                        num(1.0)
                    }
                    Err(e) => self.fail(e),
                }
            }
            "art_export_png" => {
                let p = as_(a, 0);
                match io::export_flat(self, &p, af(a, 1, 1.0), an(a, 2, 1.0) > 0.5) {
                    Ok(()) => num(1.0),
                    Err(e) => self.fail(e),
                }
            }
            "art_export_layers" => match io::export_layers(self, &as_(a, 0)) {
                Ok(n) => num(n as f64),
                Err(e) => self.fail(e),
            },
            "art_import_image" => {
                let p = as_(a, 0);
                match image::open(&p) {
                    Ok(img) => {
                        let stem = file_stem(&p).unwrap_or_else(|| "Image".into());
                        num(self.place_image(img.to_rgba8(), &stem, an(a, 1, 1.0) > 0.5) as f64)
                    }
                    Err(e) => {
                        self.err = format!("could not open {p}: {e}");
                        num(-1.0)
                    }
                }
            }
            "art_image_size" => match image::image_dimensions(as_(a, 0)) {
                Ok((w, h)) => Val::nums(vec![w as f32, h as f32]),
                Err(_) => Val::nums(vec![]),
            },
            "art_paper_set" => {
                self.push_undo();
                self.doc.paper = unpack_rgb(an(a, 0, 16777215.0));
                self.doc.paper_on = an(a, 1, 1.0) > 0.5;
                self.comp.all();
                unit
            }
            "art_paper_get" => Val::nums(vec![pack_rgb(self.doc.paper) as f32, if self.doc.paper_on { 1.0 } else { 0.0 }]),

            // undo
            "art_undo" => {
                if self.stroke.is_some() {
                    self.stroke_end();
                }
                self.xf = None;
                if let Some(d) = self.undo.pop() {
                    self.redo.push(core::mem::replace(&mut self.doc, d));
                    self.reset_comp();
                    self.dirty = true;
                    num(1.0)
                } else {
                    num(0.0)
                }
            }
            "art_redo" => {
                if let Some(d) = self.redo.pop() {
                    self.undo.push(core::mem::replace(&mut self.doc, d));
                    self.reset_comp();
                    self.dirty = true;
                    num(1.0)
                } else {
                    num(0.0)
                }
            }
            "art_undo_push" => {
                self.push_undo();
                unit
            }
            "art_undo_count" => Val::nums(vec![self.undo.len() as f32, self.redo.len() as f32]),

            // view
            "art_view_set" => {
                let (ox, oy) = (self.view.vx + self.view.vw * 0.5, self.view.vy + self.view.vh * 0.5);
                self.view.vx = af(a, 0, 0.0);
                self.view.vy = af(a, 1, 0.0);
                self.view.vw = af(a, 2, 100.0).max(1.0);
                self.view.vh = af(a, 3, 100.0).max(1.0);
                let _ = (ox, oy);
                unit
            }
            "art_view_pan" => {
                let (x0, y0) = self.view.s2c(0.0, 0.0);
                let (x1, y1) = self.view.s2c(af(a, 0, 0.0), af(a, 1, 0.0));
                self.view.cx -= x1 - x0;
                self.view.cy -= y1 - y0;
                unit
            }
            "art_view_zoom_at" => {
                let (sx, sy) = (af(a, 0, 0.0), af(a, 1, 0.0));
                let p = self.view.s2c(sx, sy);
                self.view.zoom = (self.view.zoom * af(a, 2, 1.0)).clamp(0.01, 64.0);
                let q = self.view.s2c(sx, sy);
                self.view.cx += p.0 - q.0;
                self.view.cy += p.1 - q.1;
                unit
            }
            "art_view_set_zoom" => {
                self.view.zoom = af(a, 0, 1.0).clamp(0.01, 64.0);
                unit
            }
            "art_view_rotate" => {
                self.view.rot = (self.view.rot + af(a, 0, 0.0)).rem_euclid(360.0);
                unit
            }
            "art_view_set_rot" => {
                self.view.rot = af(a, 0, 0.0).rem_euclid(360.0);
                unit
            }
            "art_view_flip" => {
                self.view.flip = !self.view.flip;
                unit
            }
            "art_view_grid" => {
                self.view.grid = an(a, 0, 1.0) > 0.5;
                unit
            }
            "art_view_fit" => {
                self.fit_view();
                unit
            }
            "art_view_center" => {
                self.view.cx = self.doc.w as f32 * 0.5;
                self.view.cy = self.doc.h as f32 * 0.5;
                unit
            }
            "art_view_get" => Val::nums(vec![self.view.cx, self.view.cy, self.view.zoom, self.view.rot, if self.view.flip { 1.0 } else { 0.0 }]),
            "art_s2c" => {
                let (x, y) = self.view.s2c(af(a, 0, 0.0), af(a, 1, 0.0));
                Val::nums(vec![x, y])
            }
            "art_c2s" => {
                let (x, y) = self.view.c2s(af(a, 0, 0.0), af(a, 1, 0.0));
                Val::nums(vec![x, y])
            }

            // layers
            "art_layer_count" => num(self.doc.layers.len() as f64),
            "art_layer_active" => num(self.act() as f64),
            "art_layer_select" => {
                let i = an(a, 0, 0.0).max(0.0) as usize;
                if i < self.doc.layers.len() {
                    self.xf_commit();
                    self.doc.active = i;
                }
                unit
            }
            "art_layer_add" => {
                let i = self.layer_add(an(a, 0, 0.0) as u8, &as_(a, 1));
                num(i as f64)
            }
            "art_layer_del" => {
                let i = an(a, 0, self.act() as f64) as usize;
                if i < self.doc.layers.len() && self.doc.layers.len() > 1 {
                    self.push_undo();
                    self.doc.layers.remove(i);
                    self.doc.active = self.doc.active.min(self.doc.layers.len() - 1);
                    self.comp.all();
                }
                unit
            }
            "art_layer_dup" => {
                let i = an(a, 0, self.act() as f64) as usize;
                if i < self.doc.layers.len() {
                    self.push_undo();
                    let mut l = self.doc.layers[i].clone();
                    l.m.name = format!("{} copy", l.m.name);
                    self.doc.layers.insert(i + 1, l);
                    self.doc.active = i + 1;
                    self.comp.all();
                }
                unit
            }
            "art_layer_move" => {
                let i = an(a, 0, 0.0) as usize;
                let j = an(a, 1, 0.0).max(0.0) as usize;
                let n = self.doc.layers.len();
                if i < n && j < n && i != j {
                    self.push_undo();
                    let l = self.doc.layers.remove(i);
                    self.doc.layers.insert(j, l);
                    self.doc.active = j;
                    self.comp.all();
                }
                unit
            }
            "art_layer_merge_down" => {
                let i = an(a, 0, self.act() as f64) as usize;
                if i >= 1 && i < self.doc.layers.len() {
                    self.push_undo();
                    let top = self.doc.layers.remove(i);
                    let base_alpha_lock = self.doc.layers[i - 1].m.clip;
                    let _ = base_alpha_lock;
                    let below = &mut self.doc.layers[i - 1];
                    if below.m.kind == 1 {
                        below.m.kind = 0;
                        below.m.paths.clear();
                    }
                    let op = top.m.opacity;
                    let bm = top.m.blend;
                    let tt = top.t.clone();
                    let (w, h) = (tt.w as i32, tt.h as i32);
                    below.t.for_rect(0, 0, w, h, |x, y, p| {
                        let s = tt.getf(x, y);
                        if s[3] <= 0.0 {
                            return;
                        }
                        *p = fromf(blend::blend(tof(*p), [s[0] * op, s[1] * op, s[2] * op, s[3] * op], bm));
                    });
                    below.t.prune();
                    self.doc.active = i - 1;
                    self.comp.all();
                }
                unit
            }
            "art_layer_get" => self.layer_get(an(a, 0, 0.0) as usize, &as_(a, 1)),
            "art_layer_set" => {
                let v = a.get(2).cloned().unwrap_or(Val::Num(0.0));
                self.layer_set(an(a, 0, 0.0) as usize, &as_(a, 1), &v);
                unit
            }
            "art_layer_clear" => {
                let i = an(a, 0, self.act() as f64) as usize;
                if self.editable(i) {
                    self.push_undo();
                    let sel = self.doc.sel.clone();
                    let l = &mut self.doc.layers[i];
                    match sel {
                        None => {
                            l.t.clear();
                            l.m.paths.clear();
                        }
                        Some(s) => {
                            let b = s.bbox().unwrap_or([0, 0, 0, 0]);
                            l.t.for_rect(b[0], b[1], b[2], b[3], |x, y, p| {
                                let k = 1.0 - s.at(x, y);
                                let c = tof(*p);
                                *p = fromf([c[0] * k, c[1] * k, c[2] * k, c[3] * k]);
                            });
                            l.t.prune();
                        }
                    }
                    self.comp.all();
                }
                unit
            }
            "art_layer_fill" => {
                let i = an(a, 0, self.act() as f64) as usize;
                if self.editable(i) && self.doc.layers[i].m.kind == 0 {
                    self.push_undo();
                    let r = self.sel_or_canvas();
                    let sel = self.doc.sel.clone();
                    let col = if an(a, 1, 0.0) > 0.5 { self.bg } else { self.fg };
                    let l = &mut self.doc.layers[i];
                    let alock = l.m.alpha_lock > 0.5;
                    l.t.for_rect(r[0], r[1], r[2], r[3], |x, y, p| {
                        let k = sel.as_ref().map_or(1.0, |s| s.at(x, y));
                        if k <= 0.0 {
                            return;
                        }
                        let d = tof(*p);
                        *p = fromf(if alock { [d[0] + (col[0] * d[3] - d[0]) * k, d[1] + (col[1] * d[3] - d[1]) * k, d[2] + (col[2] * d[3] - d[2]) * k, d[3]] } else { over(d, [col[0] * k, col[1] * k, col[2] * k, k]) });
                    });
                    l.t.prune();
                    self.comp.mark(r);
                }
                unit
            }
            "art_layer_rasterize" => {
                let i = an(a, 0, self.act() as f64) as usize;
                if i < self.doc.layers.len() {
                    self.push_undo();
                    let l = &mut self.doc.layers[i];
                    l.m.kind = 0;
                    l.m.paths.clear();
                }
                unit
            }
            "art_layer_flip" => {
                let i = an(a, 0, self.act() as f64) as usize;
                let vert = an(a, 1, 0.0) > 0.5;
                if self.editable(i) {
                    self.push_undo();
                    let (w, h) = (self.doc.w as i32, self.doc.h as i32);
                    let l = &mut self.doc.layers[i];
                    if l.m.kind == 1 {
                        for p in l.m.paths.iter_mut() {
                            for an in p.a.iter_mut() {
                                if vert {
                                    an.y = h as f32 - an.y;
                                    an.iy = -an.iy;
                                    an.oy = -an.oy;
                                } else {
                                    an.x = w as f32 - an.x;
                                    an.ix = -an.ix;
                                    an.ox = -an.ox;
                                }
                            }
                        }
                        l.reraster();
                    } else {
                        let src = l.t.clone();
                        l.t.clear();
                        let mut dst = Tiles::new(src.w, src.h);
                        dst.for_rect(0, 0, w, h, |x, y, p| {
                            *p = if vert { src.get(x, h - 1 - y) } else { src.get(w - 1 - x, y) };
                        });
                        dst.prune();
                        l.t = dst;
                    }
                    self.comp.all();
                }
                unit
            }
            "art_layer_export" => {
                let i = an(a, 0, self.act() as f64) as usize;
                match io::export_layer(self, i, &as_(a, 1)) {
                    Ok(()) => num(1.0),
                    Err(e) => self.fail(e),
                }
            }
            "art_blend_names" => Val::strs(&blend::BLEND_NAMES),

            // canvas
            "art_canvas_flip" => {
                let vert = an(a, 0, 0.0) > 0.5;
                self.push_undo();
                let n = self.doc.layers.len();
                let keep = self.doc.active;
                for i in 0..n {
                    let l = &mut self.doc.layers[i];
                    let (w, h) = (l.t.w as i32, l.t.h as i32);
                    if l.m.kind == 1 {
                        for p in l.m.paths.iter_mut() {
                            for an in p.a.iter_mut() {
                                if vert {
                                    an.y = h as f32 - an.y;
                                    an.iy = -an.iy;
                                    an.oy = -an.oy;
                                } else {
                                    an.x = w as f32 - an.x;
                                    an.ix = -an.ix;
                                    an.ox = -an.ox;
                                }
                            }
                        }
                        l.reraster();
                    } else {
                        let src = l.t.clone();
                        let mut dst = Tiles::new(src.w, src.h);
                        dst.for_rect(0, 0, w, h, |x, y, p| *p = if vert { src.get(x, h - 1 - y) } else { src.get(w - 1 - x, y) });
                        dst.prune();
                        l.t = dst;
                    }
                }
                self.doc.active = keep;
                self.doc.sel = None;
                self.comp.all();
                unit
            }
            "art_canvas_resize" => {
                let nw = an(a, 0, self.doc.w as f64).clamp(16.0, 20000.0) as usize;
                let nh = an(a, 1, self.doc.h as f64).clamp(16.0, 20000.0) as usize;
                // anchor 0..8 (3x3 grid, 4 = centre)
                let anc = an(a, 2, 4.0) as i32;
                let ox = match anc % 3 {
                    0 => 0,
                    1 => (nw as i32 - self.doc.w as i32) / 2,
                    _ => nw as i32 - self.doc.w as i32,
                };
                let oy = match anc / 3 {
                    0 => 0,
                    1 => (nh as i32 - self.doc.h as i32) / 2,
                    _ => nh as i32 - self.doc.h as i32,
                };
                self.push_undo();
                for l in self.doc.layers.iter_mut() {
                    let src = l.t.clone();
                    let mut dst = Tiles::new(nw, nh);
                    dst.for_rect(0, 0, nw as i32, nh as i32, |x, y, p| *p = src.get(x - ox, y - oy));
                    dst.prune();
                    l.t = dst;
                    for p in l.m.paths.iter_mut() {
                        p.translate(ox as f32, oy as f32);
                    }
                }
                self.doc.w = nw;
                self.doc.h = nh;
                self.doc.sel = None;
                self.reset_comp();
                self.fit_view();
                unit
            }
            "art_crop_to_selection" => {
                if let Some(b) = self.doc.sel.as_ref().and_then(|s| s.bbox()) {
                    self.push_undo();
                    let (nw, nh) = ((b[2] - b[0]) as usize, (b[3] - b[1]) as usize);
                    for l in self.doc.layers.iter_mut() {
                        let src = l.t.clone();
                        let mut dst = Tiles::new(nw, nh);
                        dst.for_rect(0, 0, nw as i32, nh as i32, |x, y, p| *p = src.get(x + b[0], y + b[1]));
                        dst.prune();
                        l.t = dst;
                        for p in l.m.paths.iter_mut() {
                            p.translate(-b[0] as f32, -b[1] as f32);
                        }
                    }
                    self.doc.w = nw;
                    self.doc.h = nh;
                    self.doc.sel = None;
                    self.reset_comp();
                    self.fit_view();
                }
                unit
            }

            // brushes
            "art_brush_set" => {
                let k = as_(a, 0);
                if k == "name" {
                    self.brush.name = as_(a, 1);
                } else {
                    self.brush.set(&k, af(a, 1, 0.0));
                }
                unit
            }
            "art_brush_get" => {
                let k = as_(a, 0);
                if k == "name" {
                    Val::Str(self.brush.name.clone())
                } else {
                    num(self.brush.get(&k) as f64)
                }
            }
            "art_brush_keys" => {
                let mut v = Vec::new();
                for (k, l, lo, hi) in brush::BRUSH_KEYS {
                    v.push(Val::Str(k.to_string()));
                    v.push(Val::Str(l.to_string()));
                    v.push(num(*lo as f64));
                    v.push(num(*hi as f64));
                }
                Val::List(v)
            }
            "art_brush_presets" => Val::List(self.presets.iter().map(|b| Val::Str(b.name.clone())).collect()),
            "art_brush_load" => {
                let i = an(a, 0, 0.0) as usize;
                if let Some(b) = self.presets.get(i) {
                    self.brush = b.clone();
                }
                unit
            }
            "art_brush_save_preset" => {
                let nm = as_(a, 0);
                let mut b = self.brush.clone();
                if !nm.is_empty() {
                    b.name = nm;
                }
                if let Some(i) = self.presets.iter().position(|p| p.name == b.name) {
                    self.presets[i] = b;
                    num(i as f64)
                } else {
                    self.presets.push(b);
                    num((self.presets.len() - 1) as f64)
                }
            }
            "art_brush_del_preset" => {
                let i = an(a, 0, -1.0);
                if i >= 0.0 && (i as usize) < self.presets.len() && self.presets.len() > 1 {
                    self.presets.remove(i as usize);
                }
                unit
            }
            "art_brush_reset_presets" => {
                self.presets = brush::presets();
                unit
            }
            #[cfg(feature = "std")]
            "art_brush_save_file" => match serde_json::to_string_pretty(&self.presets).map_err(|e| e.to_string()).and_then(|t| std::fs::write(as_(a, 0), t).map_err(|e| e.to_string())) {
                Ok(()) => num(1.0),
                Err(e) => self.fail(e),
            },
            #[cfg(not(feature = "std"))]
            "art_brush_save_file" => match serde_json::to_string_pretty(&self.presets).map_err(|e| e.to_string()).and_then(|t| os::write(&as_(a, 0), t.as_bytes())) {
                Ok(()) => num(1.0),
                Err(e) => self.fail(e),
            },
            #[cfg(feature = "std")]
            "art_brush_load_file" => match std::fs::read_to_string(as_(a, 0)).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<Vec<Brush>>(&t).map_err(|e| e.to_string())) {
                Ok(v) if !v.is_empty() => {
                    self.presets = v;
                    num(1.0)
                }
                Ok(_) => num(0.0),
                Err(e) => self.fail(e),
            },
            #[cfg(not(feature = "std"))]
            "art_brush_load_file" => match os::read(&as_(a, 0)).and_then(|t| serde_json::from_slice::<Vec<Brush>>(&t).map_err(|e| e.to_string())) {
                Ok(v) if !v.is_empty() => {
                    self.presets = v;
                    num(1.0)
                }
                Ok(_) => num(0.0),
                Err(e) => self.fail(e),
            },
            "art_tip_names" => Val::strs(&brush::TIP_NAMES),
            "art_mode_names" => Val::strs(&brush::MODE_NAMES),

            // colours
            "art_color" => {
                self.fg = unpack_rgb(an(a, 0, 0.0));
                unit
            }
            "art_color_get" => num(pack_rgb(self.fg)),
            "art_bg" => {
                self.bg = unpack_rgb(an(a, 0, 16777215.0));
                unit
            }
            "art_bg_get" => num(pack_rgb(self.bg)),
            "art_swap_colors" => {
                core::mem::swap(&mut self.fg, &mut self.bg);
                unit
            }
            "art_hsv_to" => num(pack_rgb(hsv2rgb([af(a, 0, 0.0), af(a, 1, 0.0), af(a, 2, 0.0)]))),
            "art_to_hsv" => {
                let c = rgb2hsv(unpack_rgb(an(a, 0, 0.0)));
                Val::nums(c.to_vec())
            }
            "art_pick" => {
                let (x, y) = self.view.s2c(af(a, 0, 0.0), af(a, 1, 0.0));
                let c = if an(a, 2, 1.0) > 0.5 {
                    self.update_comp();
                    if x < 0.0 || y < 0.0 || x as usize >= self.comp.w || y as usize >= self.comp.h {
                        [0.0; 4]
                    } else {
                        unpack_pre(self.comp.px[y as usize * self.comp.w + x as usize])
                    }
                } else {
                    self.doc.layers.get(self.act()).map_or([0.0; 4], |l| l.t.getf(x as i32, y as i32))
                };
                if c[3] < 0.02 {
                    num(-1.0)
                } else {
                    let u = unpre(c);
                    num(pack_rgb([u[0], u[1], u[2]]))
                }
            }

            // pen
            "art_pen" => {
                let p = self.pen_last;
                Val::nums(vec![p.p, p.tilt, p.az.to_degrees(), p.twist.to_degrees(), if p.eraser { 1.0 } else { 0.0 }, if self.tablet.is_open() { 1.0 } else { 0.0 }, if self.pen_recent() { 1.0 } else { 0.0 }])
            }
            "art_tablet_close" => {
                self.tablet.close();
                unit
            }
            "art_tablet_name" => Val::Str(self.tablet_name()),
            "art_sim_set" => {
                let v = af(a, 1, 0.0);
                match as_(a, 0).as_str() {
                    "mode" => self.sim_mode = (v.max(0.0) as u8).min(3),
                    "p" => self.sim_p = v.clamp(0.0, 1.0),
                    "tilt" => self.sim_tilt = v.clamp(0.0, 1.0),
                    "az" => self.sim_az = v,
                    "follow" => self.sim_follow = v > 0.5,
                    _ => {}
                }
                unit
            }
            "art_sim_get" => num(match as_(a, 0).as_str() {
                "mode" => self.sim_mode as f64,
                "p" => self.sim_p as f64,
                "tilt" => self.sim_tilt as f64,
                "az" => self.sim_az as f64,
                "follow" => self.sim_follow as i32 as f64,
                _ => 0.0,
            }),
            "art_stroke_begin" => self.stroke_begin(af(a, 0, 0.0), af(a, 1, 0.0), an(a, 2, 0.0) > 0.5),
            "art_stroke_to" => {
                self.stroke_to(af(a, 0, 0.0), af(a, 1, 0.0));
                unit
            }
            "art_stroke_end" => {
                self.stroke_end();
                unit
            }
            "art_stroking" => b2n(self.stroke.is_some()),

            // selection
            "art_sel_rect" | "art_sel_ellipse" => {
                let (x0, y0, x1, y1) = (af(a, 0, 0.0), af(a, 1, 0.0), af(a, 2, 0.0), af(a, 3, 0.0));
                let (xa, xb) = (x0.min(x1), x0.max(x1));
                let (ya, yb) = (y0.min(y1), y0.max(y1));
                let pts: Vec<(f32, f32)> = if name == "art_sel_rect" {
                    vec![(xa, ya), (xb, ya), (xb, yb), (xa, yb)]
                } else {
                    let (cx, cy, rx, ry) = ((xa + xb) * 0.5, (ya + yb) * 0.5, (xb - xa) * 0.5, (yb - ya) * 0.5);
                    (0..96).map(|i| {
                        let t = i as f32 / 96.0 * core::f32::consts::TAU;
                        (cx + t.cos() * rx, cy + t.sin() * ry)
                    }).collect()
                };
                self.push_undo();
                let m = self.poly_mask(&pts);
                self.sel_combine(m, an(a, 4, 0.0) as u8);
                unit
            }
            "art_sel_poly" => {
                let l = al(a, 0);
                let pts: Vec<(f32, f32)> = l.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1])).collect();
                if pts.len() >= 3 {
                    self.push_undo();
                    let m = self.poly_mask(&pts);
                    self.sel_combine(m, an(a, 1, 0.0) as u8);
                }
                unit
            }
            "art_sel_wand" => {
                let (x, y) = (af(a, 0, 0.0), af(a, 1, 0.0));
                let tol = af(a, 2, 0.1);
                let mode = an(a, 3, 0.0) as u8;
                let all = an(a, 4, 1.0) > 0.5;
                let (w, h) = (self.doc.w, self.doc.h);
                if all {
                    self.update_comp();
                }
                let region = {
                    let comp = &self.comp.px;
                    let li = self.act();
                    let lt = &self.doc.layers[li].t;
                    let get = |xx: usize, yy: usize| -> [f32; 4] {
                        if all {
                            unpack_pre(comp[yy * w + xx])
                        } else {
                            lt.getf(xx as i32, yy as i32)
                        }
                    };
                    flood(w, h, x as i32, y as i32, tol, &get)
                };
                self.push_undo();
                self.sel_combine(region.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect(), mode);
                unit
            }
            "art_sel_layer" => {
                let i = an(a, 0, self.act() as f64) as usize;
                if let Some(l) = self.doc.layers.get(i) {
                    let (w, h) = (self.doc.w, self.doc.h);
                    let mut m = vec![0.0f32; w * h];
                    for y in 0..h {
                        for x in 0..w {
                            m[y * w + x] = l.t.getf(x as i32, y as i32)[3];
                        }
                    }
                    self.push_undo();
                    self.sel_combine(m, an(a, 1, 0.0) as u8);
                }
                unit
            }
            "art_sel_all" => {
                self.push_undo();
                self.doc.sel = Some(Mask::new(self.doc.w, self.doc.h, 255));
                unit
            }
            "art_sel_none" => {
                if self.doc.sel.is_some() {
                    self.push_undo();
                    self.doc.sel = None;
                }
                unit
            }
            "art_sel_invert" => {
                self.push_undo();
                let (w, h) = (self.doc.w, self.doc.h);
                let m: Vec<u8> = match &self.doc.sel {
                    Some(s) => s.m.iter().map(|v| 255 - v).collect(),
                    None => vec![0; w * h],
                };
                let mk = Mask { w, h, m: Arc::new(m) };
                self.doc.sel = if mk.bbox().is_some() { Some(mk) } else { None };
                unit
            }
            "art_sel_grow" => {
                let px = af(a, 0, 2.0);
                if let Some(s) = self.doc.sel.clone() {
                    self.push_undo();
                    let (w, h) = (s.w, s.h);
                    let m: Vec<u8> = if px >= 0.0 {
                        let inside: Vec<bool> = s.m.iter().map(|&v| v >= 128).collect();
                        let d = filters::edt(&inside, w, h);
                        d.iter().map(|&dd| ((px + 0.5 - dd).clamp(0.0, 1.0) * 255.0) as u8).collect()
                    } else {
                        let outside: Vec<bool> = s.m.iter().map(|&v| v < 128).collect();
                        let d = filters::edt(&outside, w, h);
                        d.iter().map(|&dd| ((dd - (-px) - 0.5).clamp(0.0, 1.0) * 255.0) as u8).collect()
                    };
                    let mk = Mask { w, h, m: Arc::new(m) };
                    self.doc.sel = if mk.bbox().is_some() { Some(mk) } else { None };
                }
                unit
            }
            "art_sel_feather" => {
                let r = af(a, 0, 4.0);
                if let Some(s) = self.doc.sel.clone() {
                    self.push_undo();
                    let (w, h) = (s.w, s.h);
                    let mut px: Vec<[f32; 4]> = s.m.iter().map(|&v| {
                        let f = v as f32 / 255.0;
                        [f, f, f, f]
                    }).collect();
                    filters::gauss(&mut px, w, h, r);
                    let m: Vec<u8> = px.iter().map(|c| (c[3].clamp(0.0, 1.0) * 255.0) as u8).collect();
                    self.doc.sel = Some(Mask { w, h, m: Arc::new(m) });
                }
                unit
            }
            "art_sel_bbox" => match self.doc.sel.as_ref().and_then(|s| s.bbox()) {
                Some(b) => Val::nums(vec![b[0] as f32, b[1] as f32, (b[2] - b[0]) as f32, (b[3] - b[1]) as f32]),
                None => Val::nums(vec![]),
            },
            "art_sel_any" => b2n(self.doc.sel.is_some()),

            // fill & gradient
            "art_fill" => self.fill(af(a, 0, 0.0), af(a, 1, 0.0), af(a, 2, 0.05), an(a, 3, 1.0) > 0.5, af(a, 4, 1.0)),
            "art_grad_stops" => {
                let mut v = Vec::new();
                for (p, c) in &self.grad.stops {
                    v.push(*p);
                    v.push(pack_rgb([c[0], c[1], c[2]]) as f32);
                    v.push(c[3]);
                }
                Val::nums(v)
            }
            "art_grad_set_stops" => {
                let l = al(a, 0);
                let stops: Vec<(f32, [f32; 4])> = l.chunks(3).filter(|c| c.len() == 3).map(|c| {
                    let rgb = unpack_rgb(c[1] as f64);
                    (c[0].clamp(0.0, 1.0), [rgb[0], rgb[1], rgb[2], c[2].clamp(0.0, 1.0)])
                }).collect();
                if stops.len() >= 2 {
                    self.grad.stops = stops;
                    self.grad.sort();
                }
                unit
            }
            "art_grad_preset" => {
                self.grad = Gradient::preset(&as_(a, 0), self.fgc(), [self.bg[0], self.bg[1], self.bg[2], 1.0]);
                unit
            }
            "art_grad_reverse" => {
                for s in self.grad.stops.iter_mut() {
                    s.0 = 1.0 - s.0;
                }
                self.grad.sort();
                unit
            }
            "art_grad_presets" => Val::strs(&gradient::GRAD_PRESETS),
            "art_grad_kinds" => Val::strs(&gradient::GRAD_KINDS),
            "art_grad_repeats" => Val::strs(&gradient::GRAD_REPEAT),
            "art_grad_apply" => self.grad_apply(af(a, 0, 0.0), af(a, 1, 0.0), af(a, 2, 1.0), af(a, 3, 1.0), an(a, 4, 0.0) as u8, an(a, 5, 0.0) as u8, af(a, 6, 1.0), af(a, 7, 1.0)),

            // filters
            "art_filters" => {
                let mut v = Vec::new();
                for (id, label, ps) in filters::FILTERS {
                    v.push(Val::Str(id.to_string()));
                    v.push(Val::Str(label.to_string()));
                    v.push(num(ps.len() as f64));
                }
                Val::List(v)
            }
            "art_filter_params" => {
                let id = as_(a, 0);
                let mut v = Vec::new();
                if let Some((_, _, ps)) = filters::FILTERS.iter().find(|f| f.0 == id) {
                    for (l, lo, hi, d) in ps.iter() {
                        v.push(Val::Str(l.to_string()));
                        v.push(num(*lo as f64));
                        v.push(num(*hi as f64));
                        v.push(num(*d as f64));
                    }
                }
                Val::List(v)
            }
            "art_spot_names" => Val::strs(&filters::SPOT_NAMES),
            "art_dither_names" => Val::strs(&filters::DITHER_NAMES),
            "art_filter_begin" => {
                let li = self.act();
                if !self.editable(li) || self.doc.layers[li].m.kind == 1 {
                    return Some(Ok(self.fail("filters need an unlocked raster layer")));
                }
                self.push_undo();
                self.filt = Some((as_(a, 0), li));
                num(1.0)
            }
            "art_filter_preview" | "art_filter_apply" => {
                let (id, li) = if name == "art_filter_apply" {
                    let li = self.act();
                    if !self.editable(li) || self.doc.layers[li].m.kind == 1 {
                        return Some(Ok(self.fail("filters need an unlocked raster layer")));
                    }
                    self.push_undo();
                    (as_(a, 0), li)
                } else {
                    match self.filt.clone() {
                        Some(f) => f,
                        None => return Some(Ok(num(0.0))),
                    }
                };
                let off = if name == "art_filter_apply" { 1 } else { 0 };
                let params = al(a, off);
                let pal: Vec<[f32; 3]> = al(a, off + 1).iter().map(|&c| unpack_rgb(c as f64)).collect();
                // preview always starts from the layer as it was when the dialog opened
                if name == "art_filter_preview" {
                    if let Some(base) = self.undo.last() {
                        if let Some(l) = base.layers.get(li) {
                            self.doc.layers[li].t = l.t.clone();
                        }
                    }
                }
                let rect = match &self.doc.sel {
                    Some(s) => s.bbox().unwrap_or([0, 0, 0, 0]),
                    None => {
                        let grow = matches!(id.as_str(), "outline" | "glow" | "blur");
                        if grow {
                            [0, 0, self.doc.w as i32, self.doc.h as i32]
                        } else {
                            self.doc.layers[li].t.bbox().unwrap_or([0, 0, 0, 0])
                        }
                    }
                };
                let env = filters::FilterEnv { fg: self.fg, bg: self.bg, grad: &self.grad, palette: &pal };
                let sel = self.doc.sel.clone();
                filters::run(&id, &params, &mut self.doc.layers[li].t, sel.as_ref(), rect, &env);
                self.comp.all();
                num(1.0)
            }
            "art_filter_commit" => {
                self.filt = None;
                unit
            }
            "art_filter_cancel" => {
                if self.filt.take().is_some() {
                    if let Some(d) = self.undo.pop() {
                        self.doc = d;
                    }
                    self.comp.all();
                }
                unit
            }

            // free transform
            "art_xf_begin" => self.xf_begin(),
            "art_xf_set" => {
                if let Some(xf) = self.xf.as_mut() {
                    let before = xf.out_bbox();
                    xf.tx = af(a, 0, 0.0);
                    xf.ty = af(a, 1, 0.0);
                    xf.sx = af(a, 2, 1.0);
                    xf.sy = af(a, 3, 1.0);
                    xf.rot = af(a, 4, 0.0);
                    let after = xf.out_bbox();
                    self.comp.mark(before);
                    self.comp.mark(after);
                }
                unit
            }
            "art_xf_get" => match &self.xf {
                Some(x) => Val::nums(vec![x.tx, x.ty, x.sx, x.sy, x.rot, x.bbox[0], x.bbox[1], x.bbox[2], x.bbox[3]]),
                None => Val::nums(vec![]),
            },
            "art_xf_corners" => match &self.xf {
                Some(x) => {
                    let b = x.bbox;
                    let mut v = Vec::new();
                    for (px, py) in [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])] {
                        let (cx, cy) = x.fwd(px, py);
                        let (sx, sy) = self.view.c2s(cx, cy);
                        v.push(sx);
                        v.push(sy);
                    }
                    Val::nums(v)
                }
                None => Val::nums(vec![]),
            },
            "art_xf_commit" => {
                self.xf_commit();
                unit
            }
            "art_xf_cancel" => {
                self.xf_cancel();
                unit
            }
            "art_xf_active" => b2n(self.xf.is_some()),

            // clipboard
            "art_copy" | "art_cut" => {
                let li = self.act();
                let Some(l) = self.doc.layers.get(li) else { return Some(Ok(unit)) };
                let r = match &self.doc.sel {
                    Some(s) => s.bbox().unwrap_or([0, 0, 0, 0]),
                    None => l.t.bbox().unwrap_or([0, 0, 0, 0]),
                };
                let sel = self.doc.sel.clone();
                let mut t = Tiles::new(self.doc.w, self.doc.h);
                let lt = l.t.clone();
                t.for_rect(r[0], r[1], r[2], r[3], |x, y, p| {
                    let k = sel.as_ref().map_or(1.0, |s| s.at(x, y));
                    let c = lt.getf(x, y);
                    if k > 0.0 && c[3] > 0.0 {
                        *p = fromf([c[0] * k, c[1] * k, c[2] * k, c[3] * k]);
                    }
                });
                t.prune();
                // also onto the system clipboard, so other apps (and our own paste) can take it
                let (cw, ch) = ((r[2] - r[0]).max(0) as usize, (r[3] - r[1]).max(0) as usize);
                if cw > 0 && ch > 0 && cw * ch <= 80_000_000 {
                    self.clip_sig = clipboard::set(cw, ch, t.to_rgba8(r[0], r[1], cw, ch));
                }
                self.clip = Some((t, r));
                if name == "art_cut" && self.editable(li) {
                    self.push_undo();
                    let l = &mut self.doc.layers[li];
                    l.t.for_rect(r[0], r[1], r[2], r[3], |x, y, p| {
                        let k = 1.0 - sel.as_ref().map_or(1.0, |s| s.at(x, y));
                        let c = tof(*p);
                        *p = fromf([c[0] * k, c[1] * k, c[2] * k, c[3] * k]);
                    });
                    l.t.prune();
                    self.comp.mark(r);
                }
                num(1.0)
            }
            // [w, h] of a picture on the system clipboard, [] when there is none
            "art_clip_image_size" => match clipboard::get() {
                Some(c) => Val::nums(vec![c.w as f32, c.h as f32]),
                None => Val::nums(vec![]),
            },
            // a picture another app copied wins over our own copy; ours keeps its place on the canvas
            "art_paste" => {
                let ext = clipboard::get().filter(|c| self.clip.is_none() || Some(clipboard::sig(c.w, c.h, &c.px)) != self.clip_sig);
                if let Some(c) = ext {
                    return Some(Ok(match image::RgbaImage::from_raw(c.w as u32, c.h as u32, c.px) {
                        Some(img) => num(self.place_image(img, c.name.as_deref().unwrap_or("Pasted image"), an(a, 0, 1.0) > 0.5) as f64),
                        None => num(-1.0),
                    }));
                }
                match self.clip.clone() {
                    Some((t, _)) => {
                        let li = self.layer_add(0, "Pasted");
                        let mut t2 = Tiles::new(self.doc.w, self.doc.h);
                        let (w, h) = (self.doc.w as i32, self.doc.h as i32);
                        t2.for_rect(0, 0, w, h, |x, y, p| *p = t.get(x, y));
                        t2.prune();
                        self.doc.layers[li].t = t2;
                        self.comp.all();
                        num(li as f64)
                    }
                    None => num(-1.0),
                }
            }

            // vectors
            "art_vec_shape" => {
                let mut li = self.act();
                if self.doc.layers.get(li).is_none_or(|l| l.m.kind != 1) {
                    li = self.layer_add(1, "");
                } else {
                    self.push_undo();
                }
                let (anchors, closed) = vector::shape(an(a, 0, 0.0) as u8, af(a, 1, 0.0), af(a, 2, 0.0), af(a, 3, 0.0), af(a, 4, 0.0), an(a, 5, 5.0) as u32);
                let fill = an(a, 6, 0.0) > 0.5;
                let stroke = an(a, 7, 1.0) > 0.5 || !closed;
                let p = VPath { a: anchors, closed, stroke, width: self.brush.size.min(200.0), color: self.fgc(), fill: if fill { Some([self.bg[0], self.bg[1], self.bg[2], 1.0]) } else { None }, opacity: 1.0, ..Default::default() };
                let l = &mut self.doc.layers[li];
                l.m.paths.push(p);
                l.reraster();
                self.comp.all();
                num((self.doc.layers[li].m.paths.len() - 1) as f64)
            }
            "art_vec_path" => {
                let l = al(a, 0);
                let anchors: Vec<vector::Anchor> = l.chunks(7).filter(|c| c.len() == 7).map(|c| vector::Anchor { x: c[0], y: c[1], ix: c[2], iy: c[3], ox: c[4], oy: c[5], w: c[6] }).collect();
                if anchors.len() < 2 {
                    return Some(Ok(num(-1.0)));
                }
                let mut li = self.act();
                if self.doc.layers.get(li).is_none_or(|l| l.m.kind != 1) {
                    li = self.layer_add(1, "");
                } else {
                    self.push_undo();
                }
                let closed = an(a, 1, 0.0) > 0.5;
                let fill = an(a, 2, 0.0) > 0.5 && closed;
                let p = VPath { a: anchors, closed, width: self.brush.size.min(200.0), color: self.fgc(), fill: if fill { Some([self.bg[0], self.bg[1], self.bg[2], 1.0]) } else { None }, ..Default::default() };
                let lay = &mut self.doc.layers[li];
                lay.m.paths.push(p);
                lay.reraster();
                self.comp.all();
                num((self.doc.layers[li].m.paths.len() - 1) as f64)
            }
            "art_vec_count" => num(self.doc.layers.get(self.act()).map_or(0, |l| l.m.paths.len()) as f64),
            "art_vec_pick" => {
                let (x, y, tol) = (af(a, 0, 0.0), af(a, 1, 0.0), af(a, 2, 6.0));
                let l = &self.doc.layers[self.act()];
                let mut best = -1i32;
                let mut bd = tol;
                for (i, p) in l.m.paths.iter().enumerate().rev() {
                    let d = p.dist(x, y);
                    if d < bd {
                        bd = d;
                        best = i as i32;
                        if d < 0.0 {
                            break;
                        }
                    }
                }
                num(best as f64)
            }
            "art_vec_get" => {
                let li = self.act();
                let i = an(a, 0, 0.0) as usize;
                match self.doc.layers.get(li).and_then(|l| l.m.paths.get(i)) {
                    Some(p) => match as_(a, 1).as_str() {
                        "width" => num(p.width as f64),
                        "opacity" => num(p.opacity as f64),
                        "closed" => b2n(p.closed),
                        "stroke" => b2n(p.stroke),
                        "fill" => b2n(p.fill.is_some() || p.gfill.is_some()),
                        "color" => num(pack_rgb([p.color[0], p.color[1], p.color[2]])),
                        "fill_color" => num(p.fill.map_or(-1.0, |c| pack_rgb([c[0], c[1], c[2]]))),
                        "taper_in" => num(p.taper_in as f64),
                        "taper_out" => num(p.taper_out as f64),
                        "anchors" => num(p.a.len() as f64),
                        "bbox" => Val::nums(p.bbox().to_vec()),
                        _ => unit,
                    },
                    None => unit,
                }
            }
            "art_vec_set" => {
                let li = self.act();
                let i = an(a, 0, 0.0) as usize;
                let v = af(a, 2, 0.0);
                if let Some(p) = self.doc.layers.get_mut(li).and_then(|l| l.m.paths.get_mut(i)) {
                    match as_(a, 1).as_str() {
                        "width" => p.width = v.max(0.0),
                        "opacity" => p.opacity = v.clamp(0.0, 1.0),
                        "closed" => p.closed = v > 0.5,
                        "stroke" => p.stroke = v > 0.5,
                        "fill" => {
                            if v > 0.5 {
                                if p.fill.is_none() {
                                    p.fill = Some([self.bg[0], self.bg[1], self.bg[2], 1.0]);
                                }
                            } else {
                                p.fill = None;
                                p.gfill = None;
                            }
                        }
                        "color" => {
                            let c = unpack_rgb(v as f64);
                            p.color = [c[0], c[1], c[2], 1.0];
                        }
                        "fill_color" => {
                            let c = unpack_rgb(v as f64);
                            p.fill = Some([c[0], c[1], c[2], 1.0]);
                            p.gfill = None;
                        }
                        "taper_in" => p.taper_in = v.max(0.0),
                        "taper_out" => p.taper_out = v.max(0.0),
                        _ => {}
                    }
                    self.doc.layers[li].reraster();
                    self.comp.all();
                    self.dirty = true;
                }
                unit
            }
            "art_vec_grad_fill" => {
                let li = self.act();
                let i = an(a, 0, 0.0) as usize;
                let g = self.grad.clone();
                if let Some(p) = self.doc.layers.get_mut(li).and_then(|l| l.m.paths.get_mut(i)) {
                    p.closed = true;
                    p.gfill = Some(vector::GradFill { g, kind: an(a, 5, 0.0) as u8, rep: an(a, 6, 0.0) as u8, x0: af(a, 1, 0.0), y0: af(a, 2, 0.0), x1: af(a, 3, 1.0), y1: af(a, 4, 1.0) });
                    self.doc.layers[li].reraster();
                    self.comp.all();
                }
                unit
            }
            "art_vec_anchors" => {
                let li = self.act();
                let i = an(a, 0, 0.0) as usize;
                match self.doc.layers.get(li).and_then(|l| l.m.paths.get(i)) {
                    Some(p) => Val::nums(p.a.iter().flat_map(|q| [q.x, q.y, q.ix, q.iy, q.ox, q.oy, q.w]).collect()),
                    None => Val::nums(vec![]),
                }
            }
            "art_vec_set_anchor" => {
                let li = self.act();
                let i = an(a, 0, 0.0) as usize;
                let k = an(a, 1, 0.0) as usize;
                if let Some(p) = self.doc.layers.get_mut(li).and_then(|l| l.m.paths.get_mut(i)) {
                    if let Some(q) = p.a.get_mut(k) {
                        *q = vector::Anchor { x: af(a, 2, q.x), y: af(a, 3, q.y), ix: af(a, 4, q.ix), iy: af(a, 5, q.iy), ox: af(a, 6, q.ox), oy: af(a, 7, q.oy), w: af(a, 8, q.w) };
                    }
                    self.doc.layers[li].reraster();
                    self.comp.all();
                }
                unit
            }
            "art_vec_del_anchor" => {
                let li = self.act();
                let i = an(a, 0, 0.0) as usize;
                let k = an(a, 1, 0.0) as usize;
                if let Some(p) = self.doc.layers.get_mut(li).and_then(|l| l.m.paths.get_mut(i)) {
                    if p.a.len() > 2 && k < p.a.len() {
                        p.a.remove(k);
                    }
                    self.doc.layers[li].reraster();
                    self.comp.all();
                }
                unit
            }
            "art_vec_move" => {
                let li = self.act();
                let i = an(a, 0, 0.0) as usize;
                if let Some(p) = self.doc.layers.get_mut(li).and_then(|l| l.m.paths.get_mut(i)) {
                    p.translate(af(a, 1, 0.0), af(a, 2, 0.0));
                    self.doc.layers[li].reraster();
                    self.comp.all();
                }
                unit
            }
            "art_vec_del" => {
                let li = self.act();
                let i = an(a, 0, 0.0) as usize;
                if self.doc.layers.get(li).is_some_and(|l| i < l.m.paths.len()) {
                    self.push_undo();
                    self.doc.layers[li].m.paths.remove(i);
                    self.doc.layers[li].reraster();
                    self.comp.all();
                }
                unit
            }
            "art_vec_order" => {
                let li = self.act();
                let i = an(a, 0, 0.0) as usize;
                let up = an(a, 1, 1.0) > 0.5;
                if let Some(l) = self.doc.layers.get_mut(li) {
                    let n = l.m.paths.len();
                    if i < n {
                        let j = if up { (i + 1).min(n - 1) } else { i.saturating_sub(1) };
                        l.m.paths.swap(i, j);
                        l.reraster();
                        self.comp.all();
                        return Some(Ok(num(j as f64)));
                    }
                }
                num(-1.0)
            }

            // 2.5D scene
            "art_sc_reset" => {
                self.push_undo();
                let (w, h) = (self.doc.w as f32, self.doc.h as f32);
                self.doc.scene = Scene { w, h, ..Default::default() };
                self.doc.scene.fit_camera();
                unit
            }
            "art_sc_info" => {
                let s = &self.doc.scene;
                Val::nums(vec![s.parts.len() as f32, s.bones.len() as f32, s.fps, s.len, s.w, s.h, s.tracks.len() as f32])
            }
            "art_sc_frame" => {
                let s = &mut self.doc.scene;
                s.fps = af(a, 0, s.fps).clamp(1.0, 240.0);
                s.len = af(a, 1, s.len).clamp(0.05, 3600.0);
                s.w = af(a, 2, s.w).clamp(16.0, 16384.0);
                s.h = af(a, 3, s.h).clamp(16.0, 16384.0);
                unit
            }
            "art_sc_add_png" => {
                let p = as_(a, 0);
                match image::open(&p) {
                    Ok(img) => {
                        let img = img.to_rgba8();
                        let (w, h) = (img.width() as usize, img.height() as usize);
                        let im = Img::from_straight(w, h, &img.into_raw());
                        let stem = file_stem(&p).unwrap_or_else(|| "Part".into());
                        self.push_undo();
                        let sc = &mut self.doc.scene;
                        let part = Part::new(&stem, im, sc.w * 0.5, sc.h * 0.5);
                        sc.parts.push(part);
                        num((sc.parts.len() - 1) as f64)
                    }
                    Err(e) => {
                        self.err = format!("could not open {p}: {e}");
                        num(-1.0)
                    }
                }
            }
            // the clipboard's picture as a 2.5D part, centred on the stage
            "art_sc_add_clip" => match clipboard::get() {
                Some(c) => {
                    let im = Img::from_straight(c.w, c.h, &c.px);
                    self.push_undo();
                    let sc = &mut self.doc.scene;
                    let part = Part::new(c.name.as_deref().unwrap_or("Pasted part"), im, sc.w * 0.5, sc.h * 0.5);
                    sc.parts.push(part);
                    num((sc.parts.len() - 1) as f64)
                }
                None => num(-1.0),
            },
            "art_sc_add_layer" => {
                self.push_undo();
                let li = an(a, 0, self.act() as f64) as usize;
                let r = self.part_from_layer(li);
                if r < 0 {
                    self.undo.pop();
                    self.err = "that layer is empty".into();
                }
                num(r as f64)
            }
            "art_sc_layers_to_parts" => {
                self.push_undo();
                let spread = af(a, 0, 120.0);
                let keep = an(a, 1, 1.0) > 0.5;
                let n = self.doc.layers.len();
                let mut made = 0;
                let vis: Vec<usize> = (0..n).filter(|&i| self.doc.layers[i].m.visible > 0.5 && !self.doc.layers[i].t.is_empty()).collect();
                let m = vis.len();
                for (k, &li) in vis.iter().enumerate() {
                    let pi = self.part_from_layer(li);
                    if pi >= 0 {
                        // bottom layer furthest away, top layer nearest
                        let z = spread * (m as f32 - 1.0 - k as f32) - spread * (m as f32 - 1.0) * 0.5;
                        if keep {
                            self.doc.scene.set_depth_keep(pi as usize, z);
                        } else {
                            self.doc.scene.parts[pi as usize].z = z;
                        }
                        made += 1;
                    }
                }
                num(made as f64)
            }
            "art_sc_get" => match self.doc.scene.get(&as_(a, 0)) {
                Some(v) => num(v as f64),
                None => unit,
            },
            "art_sc_set" => {
                self.doc.scene.set(&as_(a, 0), af(a, 1, 0.0));
                self.dirty = true;
                unit
            }
            "art_sc_name" => {
                let i = an(a, 0, 0.0) as usize;
                Val::Str(self.doc.scene.parts.get(i).map(|p| p.name.clone()).unwrap_or_default())
            }
            "art_sc_rename" => {
                let i = an(a, 0, 0.0) as usize;
                if let Some(p) = self.doc.scene.parts.get_mut(i) {
                    p.name = as_(a, 1);
                }
                unit
            }
            "art_sc_del_part" => {
                self.push_undo();
                self.doc.scene.del_part(an(a, 0, 0.0) as usize);
                unit
            }
            "art_sc_dup_part" => {
                let i = an(a, 0, 0.0) as usize;
                if i < self.doc.scene.parts.len() {
                    self.push_undo();
                    let mut p = self.doc.scene.parts[i].clone();
                    p.name = format!("{} copy", p.name);
                    p.x += 20.0;
                    p.y += 20.0;
                    self.doc.scene.parts.push(p);
                    num((self.doc.scene.parts.len() - 1) as f64)
                } else {
                    num(-1.0)
                }
            }
            "art_sc_order" => {
                self.push_undo();
                self.doc.scene.reorder_part(an(a, 0, 0.0) as usize, an(a, 1, 0.0).max(0.0) as usize);
                unit
            }
            "art_sc_set_depth" => {
                self.doc.scene.set_depth_keep(an(a, 0, 0.0) as usize, af(a, 1, 0.0));
                unit
            }
            "art_sc_part_to_layer" => {
                let i = an(a, 0, 0.0) as usize;
                if let Some(p) = self.doc.scene.parts.get(i) {
                    let img = p.img.clone();
                    let (x0, y0) = ((p.x - p.px) as i32, (p.y - p.py) as i32);
                    let nm = p.name.clone();
                    let li = self.layer_add(0, &nm);
                    let raw = img.to_straight();
                    self.doc.layers[li].t.put_rgba8(x0, y0, img.w, img.h, &raw);
                    self.comp.all();
                    num(li as f64)
                } else {
                    num(-1.0)
                }
            }
            "art_bone_add" => {
                self.push_undo();
                let (x0, y0, x1, y1) = (af(a, 1, 0.0), af(a, 2, 0.0), af(a, 3, 10.0), af(a, 4, 0.0));
                let sc = &mut self.doc.scene;
                let n = sc.bones.len();
                let parent = an(a, 0, -1.0) as i32;
                let b = scene::Bone { name: format!("Bone {}", n + 1), parent: if parent >= 0 && (parent as usize) < n { parent } else { -1 }, hx: x0, hy: y0, ang: (y1 - y0).atan2(x1 - x0).to_degrees(), len: (x1 - x0).hypot(y1 - y0).max(2.0), ..Default::default() };
                sc.bones.push(b);
                num(n as f64)
            }
            "art_bone_name" => {
                let i = an(a, 0, 0.0) as usize;
                Val::Str(self.doc.scene.bones.get(i).map(|b| b.name.clone()).unwrap_or_default())
            }
            "art_bone_rename" => {
                let i = an(a, 0, 0.0) as usize;
                if let Some(b) = self.doc.scene.bones.get_mut(i) {
                    b.name = as_(a, 1);
                }
                unit
            }
            "art_bone_parent" => {
                let i = an(a, 0, 0.0) as usize;
                let p = an(a, 1, -1.0) as i32;
                let n = self.doc.scene.bones.len();
                if i < n && p != i as i32 && p < n as i32 {
                    // refuse cycles
                    let mut q = p;
                    let mut ok = true;
                    let mut guard = 0;
                    while q >= 0 && guard < 128 {
                        if q == i as i32 {
                            ok = false;
                            break;
                        }
                        q = self.doc.scene.bones[q as usize].parent;
                        guard += 1;
                    }
                    if ok {
                        self.doc.scene.bones[i].parent = p.max(-1);
                    }
                }
                unit
            }
            "art_bone_del" => {
                self.push_undo();
                self.doc.scene.del_bone(an(a, 0, 0.0) as usize);
                unit
            }
            "art_sc_bind" => {
                self.push_undo();
                let bones: Vec<i32> = al(a, 1).iter().map(|&b| b as i32).collect();
                self.doc.scene.bind(an(a, 0, 0.0) as usize, &bones, an(a, 2, 0.0) > 0.5);
                unit
            }
            "art_sc_part_bones" => {
                let i = an(a, 0, 0.0) as usize;
                Val::nums(self.doc.scene.parts.get(i).map(|p| p.bones.iter().map(|&b| b as f32).collect()).unwrap_or_default())
            }
            "art_sc_project" => {
                let (k, ox, oy, _) = self.sc_map;
                let pj = scene::Proj::new(&self.doc.scene, k, ox, oy);
                match pj.project([af(a, 0, 0.0), af(a, 1, 0.0), af(a, 2, 0.0)]) {
                    Some(p) => Val::nums(vec![p[0], p[1]]),
                    None => Val::nums(vec![]),
                }
            }
            "art_sc_unproject" => {
                let (k, ox, oy, _) = self.sc_map;
                let pj = scene::Proj::new(&self.doc.scene, k, ox, oy);
                match pj.unproject(af(a, 0, 0.0), af(a, 1, 0.0), af(a, 2, 0.0)) {
                    Some((x, y)) => Val::nums(vec![x, y]),
                    None => Val::nums(vec![]),
                }
            }
            "art_sc_pick" => {
                let (k, ox, oy, _) = self.sc_map;
                num(self.doc.scene.pick(af(a, 0, 0.0), af(a, 1, 0.0), k, ox, oy) as f64)
            }
            "art_sc_bones_screen" => {
                let (k, ox, oy, _) = self.sc_map;
                Val::nums(self.doc.scene.bone_screen(k, ox, oy))
            }
            "art_sc_map" => {
                let (k, ox, oy, r) = self.sc_map;
                Val::nums(vec![k, ox, oy, r[0], r[1], r[2], r[3]])
            }
            "art_sc_eval" => {
                self.doc.scene.eval(af(a, 0, 0.0));
                unit
            }
            "art_sc_time_set" => {
                self.doc.scene.time = af(a, 0, 0.0);
                unit
            }
            "art_sc_cam_fit" => {
                self.doc.scene.fit_camera();
                unit
            }
            "art_proj_names" => Val::strs(&scene::PROJ_NAMES),
            "art_ease_names" => Val::strs(&scene::EASE_NAMES),
            "art_key_set" => {
                self.doc.scene.key_set(&as_(a, 0), af(a, 1, 0.0), af(a, 2, 0.0), an(a, 3, -1.0) as i32);
                self.dirty = true;
                unit
            }
            "art_key_del" => {
                self.doc.scene.key_del(&as_(a, 0), af(a, 1, 0.0), af(a, 2, 0.0005));
                unit
            }
            "art_key_ease" => {
                let tgt = as_(a, 0);
                let t = af(a, 1, 0.0);
                let e = an(a, 2, 0.0) as u8;
                if let Some(tr) = self.doc.scene.tracks.iter_mut().find(|tr| tr.target == tgt) {
                    for k in tr.keys.iter_mut() {
                        if (k.t - t).abs() < 0.0005 {
                            k.e = e;
                        }
                    }
                }
                unit
            }
            "art_key_move" => {
                let tgt = as_(a, 0);
                let (t0, t1) = (af(a, 1, 0.0), af(a, 2, 0.0).max(0.0));
                if let Some(tr) = self.doc.scene.tracks.iter_mut().find(|tr| tr.target == tgt) {
                    if let Some(k) = tr.keys.iter_mut().find(|k| (k.t - t0).abs() < 0.0005) {
                        k.t = t1;
                    }
                    tr.keys.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(core::cmp::Ordering::Equal));
                }
                unit
            }
            "art_keys" => {
                let tgt = as_(a, 0);
                match self.doc.scene.tracks.iter().find(|tr| tr.target == tgt) {
                    Some(tr) => Val::nums(tr.keys.iter().flat_map(|k| [k.t, k.v, k.e as f32]).collect()),
                    None => Val::nums(vec![]),
                }
            }
            "art_tracks" => Val::List(self.doc.scene.tracks.iter().map(|t| Val::Str(t.target.clone())).collect()),
            "art_track_del" => {
                let tgt = as_(a, 0);
                self.doc.scene.tracks.retain(|t| t.target != tgt);
                unit
            }
            "art_sc_render_png" => {
                let p = as_(a, 0);
                let mut sc = self.doc.scene.clone();
                let (w, h, rgba) = sc.frame(af(a, 1, 0.0), af(a, 2, 1.0).clamp(0.05, 8.0));
                match image::save_buffer(&p, &rgba, w as u32, h as u32, image::ExtendedColorType::Rgba8) {
                    Ok(()) => num(1.0),
                    Err(e) => self.fail(e.to_string()),
                }
            }
            "art_sc_export" => self.export_start(as_(a, 0), an(a, 1, 0.0) as u8, af(a, 2, 24.0).max(1.0), af(a, 3, 0.0), af(a, 4, 1.0), af(a, 5, 1.0).clamp(0.05, 8.0)),
            "art_sc_export_poll" => {
                let e = self.export.lock().unwrap();
                Val::nums(vec![e.done as f32, e.total as f32, if e.running { 1.0 } else { 0.0 }])
            }
            "art_sc_export_msg" => Val::Str(self.export.lock().unwrap().msg.clone()),
            // LingOS has no ffmpeg to run
            #[cfg(not(feature = "std"))]
            "art_ffmpeg" => num(0.0),
            #[cfg(feature = "std")]
            "art_ffmpeg" => {
                let mut c = std::process::Command::new("ffmpeg");
                c.arg("-version");
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    c.creation_flags(0x0800_0000);
                }
                b2n(c.output().is_ok_and(|o| o.status.success()))
            }

            // stable diffusion
            "art_sd_url" => {
                self.sd.url = as_(a, 0);
                unit
            }
            "art_sd_url_get" => Val::Str(self.sd.url.clone()),
            "art_sd_refresh" => {
                self.sd.rx = Some(sd::fetch_lists(self.sd.url.clone()));
                self.sd.state = 1;
                self.sd.msg = "asking the server for its checkpoints...".into();
                unit
            }
            "art_sd_models" => Val::strs(&self.sd.models),
            "art_sd_samplers" => Val::strs(&self.sd.samplers),
            "art_sd_generate" => self.sd_generate(an(a, 0, 0.0) as u8, as_(a, 1), as_(a, 2), an(a, 3, 24.0) as u32, af(a, 4, 7.0), af(a, 5, 0.6), an(a, 6, -1.0) as i64, as_(a, 7), as_(a, 8), af(a, 9, 768.0).clamp(256.0, 2048.0)),
            "art_sd_poll" => self.sd_poll(),
            // split a layer into lineart / flats / shading and / or background + objects (worker thread)
            // args: layer, objects, components, per_colour, colours, line_sens, line_width, max_objects, method, to_stage, spread
            "art_split_start" => {
                if self.split.is_some() {
                    return Some(Ok(self.fail("a split is already running")));
                }
                let li = an(a, 0, self.act() as f64) as usize;
                let Some(l) = self.doc.layers.get(li) else { return Some(Ok(self.fail("no such layer"))) };
                let Some(r) = l.t.bbox() else { return Some(Ok(self.fail("that layer is empty"))) };
                let reg = filters::Region::read(&l.t, r);
                let o = split::SplitOpts {
                    objects: an(a, 1, 1.0) > 0.5,
                    components: an(a, 2, 1.0) > 0.5,
                    per_color: an(a, 3, 0.0) > 0.5,
                    colors: an(a, 4, 12.0) as usize,
                    line_sens: af(a, 5, 0.5),
                    line_width: af(a, 6, 4.0),
                    max_objects: an(a, 7, 6.0) as usize,
                    method: an(a, 8, 0.0) as u8,
                    sd_url: self.sd.url.clone(),
                };
                if !o.objects && !o.components {
                    return Some(Ok(self.fail("pick objects, components or both")));
                }
                let prog = Arc::new(core::sync::atomic::AtomicU32::new(0));
                let res = Arc::new(Mutex::new(None));
                let (p2, r2) = (prog.clone(), res.clone());
                #[cfg(feature = "std")]
                std::thread::spawn(move || {
                    let out = split::split(&reg.px, reg.w, reg.h, &o, &p2);
                    *r2.lock().unwrap() = Some(out);
                });
                // LingOS: no threads - split now; art_split_poll picks the result up on the next frame
                #[cfg(not(feature = "std"))]
                {
                    let out = split::split(&reg.px, reg.w, reg.h, &o, &p2);
                    *r2.lock().unwrap() = Some(out);
                }
                self.split = Some(SplitJob { prog, res, layer: li, x0: r[0], y0: r[1], to_stage: an(a, 9, 1.0) > 0.5, spread: af(a, 10, 160.0) });
                self.split_msg = "splitting...".into();
                num(1.0)
            }
            // -1 idle, 0..1 running, 2 finished (layers added), 3 failed (see art_split_msg)
            "art_split_poll" => {
                let Some(job) = &self.split else { return Some(Ok(num(-1.0))) };
                let done = job.res.lock().unwrap().take();
                match done {
                    None => num((job.prog.load(core::sync::atomic::Ordering::Relaxed) as f64 / 1000.0).min(0.999)),
                    Some(Err(e)) => {
                        self.split = None;
                        self.split_msg = e;
                        num(3.0)
                    }
                    Some(Ok(r)) => {
                        let job = self.split.take().unwrap();
                        self.split_apply(job, r);
                        num(2.0)
                    }
                }
            }
            "art_split_msg" => Val::Str(self.split_msg.clone()),
            // select where the background was filled in and make that layer active (for an AI inpaint touch-up)
            "art_split_hole_select" => match self.split_hole.clone() {
                Some((m, li)) if li < self.doc.layers.len() => {
                    self.doc.sel = Some(m);
                    self.doc.active = li;
                    num(1.0)
                }
                _ => num(0.0),
            },
            "art_sd_msg" => Val::Str(self.sd.msg.clone()),
            "art_sd_cancel" => {
                sd::interrupt(self.sd.url.clone());
                self.sd.msg = "interrupting...".into();
                unit
            }
            _ => return None,
        };
        Some(Ok(r))
    }

    fn tablet_name(&self) -> String {
        self.tablet.name.clone()
    }
    fn fit_view(&mut self) {
        let v = &mut self.view;
        v.cx = self.doc.w as f32 * 0.5;
        v.cy = self.doc.h as f32 * 0.5;
        v.rot = 0.0;
        v.flip = false;
        v.zoom = ((v.vw - 40.0) / self.doc.w as f32).min((v.vh - 40.0) / self.doc.h as f32).clamp(0.01, 16.0);
    }
}
