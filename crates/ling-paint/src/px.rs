//! Pixels, tiles and masks.
//!
//! Layers are stored as 64x64 tiles of premultiplied 16-bit RGBA, allocated on first touch and shared through
//! `Arc` so an undo snapshot of the whole document is a cheap copy of tile pointers: only the tiles a stroke
//! changes get duplicated (`Arc::make_mut`).

#[allow(unused_imports)]
use crate::prelude::*;
#[cfg(not(feature = "std"))]
use alloc::sync::Arc;
#[cfg(feature = "std")]
use std::sync::Arc;

pub const TS: usize = 64;
pub const TN: usize = TS * TS;
/// premultiplied RGBA, 0..65535
pub type Px = [u16; 4];
pub type Tile = Vec<Px>;
const K16: f32 = 1.0 / 65535.0;

#[inline(always)]
pub fn tof(p: Px) -> [f32; 4] {
    [p[0] as f32 * K16, p[1] as f32 * K16, p[2] as f32 * K16, p[3] as f32 * K16]
}
#[inline(always)]
pub fn fromf(c: [f32; 4]) -> Px {
    let a = c[3].clamp(0.0, 1.0);
    let q = |v: f32| (v.clamp(0.0, a) * 65535.0 + 0.5) as u16;
    [q(c[0]), q(c[1]), q(c[2]), (a * 65535.0 + 0.5) as u16]
}
/// premultiplied -> straight
#[inline(always)]
pub fn unpre(c: [f32; 4]) -> [f32; 4] {
    if c[3] <= 1e-6 {
        return [0.0; 4];
    }
    let k = 1.0 / c[3];
    [(c[0] * k).min(1.0), (c[1] * k).min(1.0), (c[2] * k).min(1.0), c[3]]
}
#[inline(always)]
pub fn pre(c: [f32; 4]) -> [f32; 4] {
    [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]]
}
/// source-over of two premultiplied colours
#[inline(always)]
pub fn over(dst: [f32; 4], src: [f32; 4]) -> [f32; 4] {
    let k = 1.0 - src[3];
    [src[0] + dst[0] * k, src[1] + dst[1] * k, src[2] + dst[2] * k, src[3] + dst[3] * k]
}
#[inline(always)]
pub fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}
#[inline(always)]
pub fn lum(r: f32, g: f32, b: f32) -> f32 {
    0.299 * r + 0.587 * g + 0.114 * b
}
#[inline(always)]
pub fn smooth01(x: f32) -> f32 {
    let t = x.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
#[inline(always)]
pub fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    smooth01((x - a) / (b - a))
}

/// packed 0xRRGGBB number -> straight rgb
pub fn unpack_rgb(c: f64) -> [f32; 3] {
    let c = c.max(0.0) as u32;
    [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0]
}
pub fn pack_rgb(c: [f32; 3]) -> f64 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    ((q(c[0]) << 16) | (q(c[1]) << 8) | q(c[2])) as f64
}

pub fn rgb2hsv(c: [f32; 3]) -> [f32; 3] {
    let (r, g, b) = (c[0], c[1], c[2]);
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn;
    let h = if d <= 1e-6 {
        0.0
    } else if mx == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if mx == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if mx <= 1e-6 { 0.0 } else { d / mx };
    [h, s, mx]
}
pub fn hsv2rgb(c: [f32; 3]) -> [f32; 3] {
    let h = c[0].rem_euclid(1.0) * 6.0;
    let (s, v) = (c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0));
    let i = h.floor();
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// tiled premultiplied RGBA16 image
#[derive(Clone)]
pub struct Tiles {
    pub w: usize,
    pub h: usize,
    pub tw: usize,
    pub th: usize,
    pub t: Vec<Option<Arc<Tile>>>,
}

impl Tiles {
    pub fn new(w: usize, h: usize) -> Self {
        let tw = w.div_ceil(TS);
        let th = h.div_ceil(TS);
        Tiles { w, h, tw, th, t: vec![None; tw * th] }
    }
    #[inline(always)]
    pub fn get(&self, x: i32, y: i32) -> Px {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return [0; 4];
        }
        let (x, y) = (x as usize, y as usize);
        match &self.t[(y / TS) * self.tw + x / TS] {
            Some(t) => t[(y % TS) * TS + x % TS],
            None => [0; 4],
        }
    }
    #[inline(always)]
    pub fn getf(&self, x: i32, y: i32) -> [f32; 4] {
        tof(self.get(x, y))
    }
    /// bilinear sample (premultiplied), pixel centres at +0.5
    pub fn sample(&self, x: f32, y: f32) -> [f32; 4] {
        let fx = x - 0.5;
        let fy = y - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = fx - x0;
        let ty = fy - y0;
        let (xi, yi) = (x0 as i32, y0 as i32);
        let a = self.getf(xi, yi);
        let b = self.getf(xi + 1, yi);
        let c = self.getf(xi, yi + 1);
        let d = self.getf(xi + 1, yi + 1);
        lerp4(lerp4(a, b, tx), lerp4(c, d, tx), ty)
    }
    pub fn tile_mut(&mut self, ti: usize) -> &mut Tile {
        let slot = &mut self.t[ti];
        if slot.is_none() {
            *slot = Some(Arc::new(vec![[0; 4]; TN]));
        }
        Arc::make_mut(slot.as_mut().unwrap())
    }
    pub fn set(&mut self, x: i32, y: i32, p: Px) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        if p[3] == 0 && self.t[(y / TS) * self.tw + x / TS].is_none() {
            return;
        }
        let tw = self.tw;
        self.tile_mut((y / TS) * tw + x / TS)[(y % TS) * TS + x % TS] = p;
    }
    /// visit every pixel of [x0,x1) x [y0,y1) (clipped to the image), tile by tile
    pub fn for_rect<F: FnMut(i32, i32, &mut Px)>(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, mut f: F) {
        let x0 = x0.max(0);
        let y0 = y0.max(0);
        let x1 = x1.min(self.w as i32);
        let y1 = y1.min(self.h as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let (tx0, ty0, tx1, ty1) = (x0 as usize / TS, y0 as usize / TS, (x1 as usize - 1) / TS, (y1 as usize - 1) / TS);
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let ti = ty * self.tw + tx;
                let t = self.tile_mut(ti);
                let bx = (tx * TS) as i32;
                let by = (ty * TS) as i32;
                let ya = y0.max(by);
                let yb = y1.min(by + TS as i32);
                let xa = x0.max(bx);
                let xb = x1.min(bx + TS as i32);
                for y in ya..yb {
                    let row = ((y - by) as usize) * TS;
                    for x in xa..xb {
                        f(x, y, &mut t[row + (x - bx) as usize]);
                    }
                }
            }
        }
    }
    /// drop tiles that became fully transparent
    pub fn prune(&mut self) {
        for slot in self.t.iter_mut() {
            if let Some(t) = slot {
                if t.iter().all(|p| p[3] == 0) {
                    *slot = None;
                }
            }
        }
    }
    pub fn clear(&mut self) {
        for s in self.t.iter_mut() {
            *s = None;
        }
    }
    pub fn is_empty(&self) -> bool {
        self.t.iter().all(|s| s.is_none())
    }
    /// bounding box of non-transparent pixels: x0, y0, x1, y1 (exclusive)
    pub fn bbox(&self) -> Option<[i32; 4]> {
        let mut b = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
        for ty in 0..self.th {
            for tx in 0..self.tw {
                if let Some(t) = &self.t[ty * self.tw + tx] {
                    for (i, p) in t.iter().enumerate() {
                        if p[3] > 0 {
                            let x = (tx * TS + i % TS) as i32;
                            let y = (ty * TS + i / TS) as i32;
                            b[0] = b[0].min(x);
                            b[1] = b[1].min(y);
                            b[2] = b[2].max(x + 1);
                            b[3] = b[3].max(y + 1);
                        }
                    }
                }
            }
        }
        if b[0] > b[2] {
            None
        } else {
            Some(b)
        }
    }
    /// straight-alpha 8-bit copy of a region
    pub fn to_rgba8(&self, x0: i32, y0: i32, w: usize, h: usize) -> Vec<u8> {
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let c = unpre(self.getf(x0 + x as i32, y0 + y as i32));
                let o = (y * w + x) * 4;
                for k in 0..4 {
                    out[o + k] = (c[k] * 255.0 + 0.5) as u8;
                }
            }
        }
        out
    }
    pub fn to_rgba16(&self) -> Vec<u16> {
        let mut out = vec![0u16; self.w * self.h * 4];
        for y in 0..self.h {
            for x in 0..self.w {
                let c = unpre(self.getf(x as i32, y as i32));
                let o = (y * self.w + x) * 4;
                for k in 0..4 {
                    out[o + k] = (c[k] * 65535.0 + 0.5) as u16;
                }
            }
        }
        out
    }
    /// write straight RGBA8 pixels at (x0, y0)
    pub fn put_rgba8(&mut self, x0: i32, y0: i32, w: usize, h: usize, px: &[u8]) {
        for y in 0..h {
            for x in 0..w {
                let o = (y * w + x) * 4;
                let a = px[o + 3] as f32 / 255.0;
                if a <= 0.0 {
                    continue;
                }
                let c = pre([px[o] as f32 / 255.0, px[o + 1] as f32 / 255.0, px[o + 2] as f32 / 255.0, a]);
                self.set(x0 + x as i32, y0 + y as i32, fromf(c));
            }
        }
    }
    pub fn put_rgba16(&mut self, px: &[u16]) {
        for y in 0..self.h {
            for x in 0..self.w {
                let o = (y * self.w + x) * 4;
                let a = px[o + 3] as f32 * K16;
                if a <= 0.0 {
                    continue;
                }
                let c = pre([px[o] as f32 * K16, px[o + 1] as f32 * K16, px[o + 2] as f32 * K16, a]);
                self.set(x as i32, y as i32, fromf(c));
            }
        }
    }
}

/// selection: one byte per canvas pixel, shared copy-on-write
#[derive(Clone)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub m: Arc<Vec<u8>>,
}
impl Mask {
    pub fn new(w: usize, h: usize, v: u8) -> Self {
        Mask { w, h, m: Arc::new(vec![v; w * h]) }
    }
    #[inline(always)]
    pub fn at(&self, x: i32, y: i32) -> f32 {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return 0.0;
        }
        self.m[y as usize * self.w + x as usize] as f32 * (1.0 / 255.0)
    }
    pub fn bbox(&self) -> Option<[i32; 4]> {
        let mut b = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
        for y in 0..self.h {
            let row = &self.m[y * self.w..(y + 1) * self.w];
            if let Some(x0) = row.iter().position(|&v| v > 0) {
                let x1 = row.iter().rposition(|&v| v > 0).unwrap();
                b[0] = b[0].min(x0 as i32);
                b[2] = b[2].max(x1 as i32 + 1);
                b[1] = b[1].min(y as i32);
                b[3] = b[3].max(y as i32 + 1);
            }
        }
        if b[0] > b[2] {
            None
        } else {
            Some(b)
        }
    }
}

/// tiny deterministic RNG (xorshift64*)
#[derive(Clone)]
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        ((x.wrapping_mul(0x2545F4914F6CDD1D) >> 40) as f32) / 16777216.0
    }
    /// -1..1
    pub fn sym(&mut self) -> f32 {
        self.next() * 2.0 - 1.0
    }
    /// roughly gaussian (sum of three uniforms), -1..1
    pub fn gauss(&mut self) -> f32 {
        (self.next() + self.next() + self.next()) / 1.5 - 1.0
    }
}

#[inline(always)]
fn hash2(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6b343) ^ (y as u32).wrapping_mul(0xd8163841);
    h ^= h >> 13;
    h = h.wrapping_mul(0x85ebca6b);
    h ^= h >> 16;
    (h & 0xffffff) as f32 / 16777215.0
}
/// value noise 0..1
pub fn vnoise(x: f32, y: f32) -> f32 {
    let xi = x.floor();
    let yi = y.floor();
    let fx = smooth01(x - xi);
    let fy = smooth01(y - yi);
    let (ix, iy) = (xi as i32, yi as i32);
    let a = hash2(ix, iy);
    let b = hash2(ix + 1, iy);
    let c = hash2(ix, iy + 1);
    let d = hash2(ix + 1, iy + 1);
    let top = a + (b - a) * fx;
    let bot = c + (d - c) * fx;
    top + (bot - top) * fy
}
/// paper grain: two octaves of value noise plus a little white noise, canvas-anchored
pub fn grain(x: f32, y: f32, scale: f32) -> f32 {
    let s = 1.0 / scale.max(0.2);
    let n = vnoise(x * s, y * s) * 0.55 + vnoise(x * s * 2.7 + 17.0, y * s * 2.7 + 5.0) * 0.3 + hash2(x as i32, y as i32) * 0.15;
    n.clamp(0.0, 1.0)
}
