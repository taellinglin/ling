//! Native 3-D for compiled `.ling` games on LingOS: the host's game builtins
//! (`ling/src/runtime/game3d.rs`, the shape builtins and the camera / light /
//! fog state of `ling/src/gfx`) rendered straight into the app window's
//! `0xRRGGBB` back buffer, so LingDev's players run their world through the
//! kernel instead of the Ling-side software renderer (`rt_sw3d.ling`).
//!
//! What is here: the host camera (Y-then-X rotation, `zdist`, focal length /
//! `tm_fov`), a per-window z-buffer, near-plane clipping, a fixed-point span
//! rasterizer (nearest texel, perspective-correct every 8 pixels, per-vertex
//! colour, point lights evaluated per 8-pixel block for `tm_draw_lit`, fog,
//! normal / additive blending, the see-through "hole"), the primitive shapes
//! (cube, sphere, cylinder, cone, capsule, torus; cel-lit per vertex, filled
//! or wireframe), `.ltm` meshes with their groups, textured quads, billboards,
//! 2-D sprites, glyph textures from the app's fonts, the particle system and
//! the mesh collision grid (`coll_*`) Soul Symphony's physics asks for.
//!
//! Opaque triangles draw immediately; anything translucent or additive is
//! kept until `flush_3d` and drawn back-to-front. Textures are power-of-two
//! squares of `0xAARRGGBB` texels (PNG with alpha, SVG rendered twice to
//! recover coverage, BMP), resampled like the host's `tm_tex_load`.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::lingrt::{is_list, list_from, list_items, num, str_of, to_f64, to_string, UNIT};

type V3 = [f32; 3];

// ── small math ───────────────────────────────────────────────────────────────

#[inline]
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
#[inline]
fn sqrt(x: f32) -> f32 {
    libm::sqrtf(x)
}
#[inline]
fn norm(a: V3) -> V3 {
    let l = sqrt(dot(a, a));
    if l > 1e-8 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0; 3]
    }
}
#[inline]
fn lerp3(a: V3, b: V3, t: f32) -> V3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
#[inline]
fn sin(x: f32) -> f32 {
    libm::sinf(x)
}
#[inline]
fn cos(x: f32) -> f32 {
    libm::cosf(x)
}
#[inline]
fn floor(x: f32) -> f32 {
    libm::floorf(x)
}
#[inline]
fn ceil(x: f32) -> f32 {
    libm::ceilf(x)
}
const PI: f32 = core::f32::consts::PI;
const TAU: f32 = core::f32::consts::TAU;

fn pow2(n: u32) -> u32 {
    let mut p = 8;
    while p < n {
        p <<= 1;
    }
    p
}

// ── Ling values ──────────────────────────────────────────────────────────────

#[inline]
fn f(args: &[u64], i: usize, d: f32) -> f32 {
    match args.get(i) {
        Some(&v) => {
            let x = to_f64(v) as f32;
            if x.is_finite() {
                x
            } else {
                d
            }
        },
        None => d,
    }
}
#[inline]
fn n(args: &[u64], i: usize, d: f64) -> f64 {
    args.get(i).map(|&v| to_f64(v)).unwrap_or(d)
}
fn rgb(c: f64) -> u32 {
    (c.max(0.0) as u64 & 0x00FF_FFFF) as u32
}
fn col3(c: u32) -> V3 {
    [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0]
}
fn pack3(c: V3) -> u32 {
    let q = |x: f32| (x * 255.0).clamp(0.0, 255.0) as u32;
    (q(c[0]) << 16) | (q(c[1]) << 8) | q(c[2])
}
fn lerp_rgb(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| -> u32 {
        let x = ((a >> s) & 255) as f32;
        let y = ((b >> s) & 255) as f32;
        ((x + (y - x) * t).clamp(0.0, 255.0) as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}
fn nums(v: &[f32]) -> u64 {
    list_from(v.iter().map(|&x| num(x as f64)).collect())
}
/// A list argument as f32s (empty when it is not a list).
fn flist(args: &[u64], i: usize) -> Vec<f32> {
    match args.get(i) {
        Some(&v) if is_list(v) => list_items(v).iter().map(|&x| to_f64(x) as f32).collect(),
        _ => Vec::new(),
    }
}
/// `rot` argument: a yaw angle or a row-major 3x3 list.
fn rot_from(args: &[u64], i: usize) -> [f32; 9] {
    if let Some(&v) = args.get(i) {
        if is_list(v) {
            let l = list_items(v);
            if l.len() >= 9 {
                let mut m = [0.0f32; 9];
                for k in 0..9 {
                    m[k] = to_f64(l[k]) as f32;
                }
                return m;
            }
        }
    }
    let yaw = f(args, i, 0.0);
    let (c, s) = (cos(yaw), sin(yaw));
    [c, 0.0, s, 0.0, 1.0, 0.0, -s, 0.0, c]
}
#[inline]
fn xf(m: &[f32; 9], p: V3, s: f32, o: V3) -> V3 {
    [
        o[0] + s * (m[0] * p[0] + m[1] * p[1] + m[2] * p[2]),
        o[1] + s * (m[3] * p[0] + m[4] * p[1] + m[5] * p[2]),
        o[2] + s * (m[6] * p[0] + m[7] * p[1] + m[8] * p[2]),
    ]
}
#[inline]
fn rotn(m: &[f32; 9], v: V3) -> V3 {
    [
        m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
        m[3] * v[0] + m[4] * v[1] + m[5] * v[2],
        m[6] * v[0] + m[7] * v[1] + m[8] * v[2],
    ]
}

// ── textures ─────────────────────────────────────────────────────────────────

/// A square power-of-two texture of `0xAARRGGBB` texels.
pub struct Tex {
    w: u32,
    sh: u32,
    px: Vec<u32>,
}

impl Tex {
    fn new(w: u32, px: Vec<u32>) -> Tex {
        let mut sh = 0;
        while (1u32 << sh) < w {
            sh += 1;
        }
        Tex { w, sh, px }
    }
}

/// Box-filter `src` (any size, `0xAARRGGBB`) into a `size` x `size` square,
/// alpha-weighted so transparent texels do not bleed black.
fn resample(src: &[u32], sw: u32, sh: u32, size: u32) -> Vec<u32> {
    let mut out = vec![0u32; (size * size) as usize];
    if sw == 0 || sh == 0 {
        return out;
    }
    let fx = sw as f32 / size as f32;
    let fy = sh as f32 / size as f32;
    let taps = (ceil(if fx > fy { fx } else { fy }) as u32).clamp(1, 8);
    for y in 0..size {
        for x in 0..size {
            let mut acc = [0u64; 4];
            for j in 0..taps {
                for i in 0..taps {
                    let sx = (((x as f32 + (i as f32 + 0.5) / taps as f32) * fx) as u32).min(sw - 1);
                    let sy = (((y as f32 + (j as f32 + 0.5) / taps as f32) * fy) as u32).min(sh - 1);
                    let c = src[(sy * sw + sx) as usize];
                    let a = (c >> 24) as u64;
                    acc[0] += ((c >> 16) & 255) as u64 * a;
                    acc[1] += ((c >> 8) & 255) as u64 * a;
                    acc[2] += (c & 255) as u64 * a;
                    acc[3] += a;
                }
            }
            let nt = (taps * taps) as u64;
            let a = acc[3] / nt;
            let (r, g, b) = if acc[3] > 0 { (acc[0] / acc[3], acc[1] / acc[3], acc[2] / acc[3]) } else { (0, 0, 0) };
            out[(y * size + x) as usize] = ((a.min(255) as u32) << 24) | ((r.min(255) as u32) << 16) | ((g.min(255) as u32) << 8) | b.min(255) as u32;
        }
    }
    out
}

fn load_tex(path: &str, size: u32) -> Option<Tex> {
    use crate::drivers::image;
    let data = crate::vfs::read(path)?;
    let data = image::gunzip(&data).unwrap_or(data);
    let l = path.to_ascii_lowercase();
    let size = pow2(size.clamp(8, 1024));
    let img = if l.ends_with(".svg") || l.ends_with(".svgz") || image::looks_svg(&data) {
        image::decode_svg_alpha(&data, size)?
    } else if l.ends_with(".bmp") {
        image::decode_bmp_rgba(&data)?
    } else {
        image::decode_png_rgba(&data)?
    };
    Some(Tex::new(size, resample(&img.px, img.w, img.h, size)))
}

fn glow_tex() -> Tex {
    let n = 64u32;
    let mut px = vec![0u32; (n * n) as usize];
    for y in 0..n {
        for x in 0..n {
            let dx = (x as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let dy = (y as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let d = sqrt(dx * dx + dy * dy);
            let core = (1.0 - d / 0.35).clamp(0.0, 1.0);
            let halo = (1.0 - d).clamp(0.0, 1.0);
            let a = (core * core * 0.9 + halo * halo * halo * 0.55).clamp(0.0, 1.0);
            px[(y * n + x) as usize] = (((a * 255.0) as u32) << 24) | 0x00FF_FFFF;
        }
    }
    Tex::new(n, px)
}

fn soft_tex() -> Tex {
    let n = 64u32;
    let mut px = vec![0u32; (n * n) as usize];
    for y in 0..n {
        for x in 0..n {
            let dx = (x as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let dy = (y as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let t = (1.0 - sqrt(dx * dx + dy * dy)).clamp(0.0, 1.0);
            let a = t * t * (3.0 - 2.0 * t);
            px[(y * n + x) as usize] = (((a * 255.0) as u32) << 24) | 0x00FF_FFFF;
        }
    }
    Tex::new(n, px)
}

// ── meshes, lights, particles, collision ─────────────────────────────────────

struct Group {
    name: String,
    pos: Vec<V3>,
    uv: Vec<[f32; 2]>,
    nrm: Vec<V3>,
    idx: Vec<[u32; 3]>,
}

struct Mesh {
    groups: Vec<Group>,
}

fn load_ltm(path: &str) -> Option<Mesh> {
    let b = crate::vfs::read(path)?;
    if b.len() < 8 || &b[0..4] != b"LTM1" {
        return None;
    }
    let mut o = 4usize;
    let u32_at = |o: &mut usize| -> Option<u32> {
        if *o + 4 > b.len() {
            return None;
        }
        let v = u32::from_le_bytes([b[*o], b[*o + 1], b[*o + 2], b[*o + 3]]);
        *o += 4;
        Some(v)
    };
    let f32_at = |o: &mut usize| -> Option<f32> { u32_at(o).map(f32::from_bits) };
    let ng = u32_at(&mut o)?;
    let mut groups = Vec::new();
    for _ in 0..ng {
        if o >= b.len() {
            return None;
        }
        let nl = b[o] as usize;
        o += 1;
        let name = String::from_utf8_lossy(&b[o..(o + nl).min(b.len())]).into_owned();
        o += nl;
        let nv = u32_at(&mut o)? as usize;
        let nt = u32_at(&mut o)? as usize;
        if o + nv * 32 + nt * 12 > b.len() {
            return None;
        }
        let mut g = Group { name, pos: Vec::with_capacity(nv), uv: Vec::with_capacity(nv), nrm: Vec::with_capacity(nv), idx: Vec::with_capacity(nt) };
        for _ in 0..nv {
            let x = f32_at(&mut o)?;
            let y = f32_at(&mut o)?;
            let z = f32_at(&mut o)?;
            let u = f32_at(&mut o)?;
            let v = f32_at(&mut o)?;
            let nx = f32_at(&mut o)?;
            let ny = f32_at(&mut o)?;
            let nz = f32_at(&mut o)?;
            g.pos.push([x, y, z]);
            g.uv.push([u, v]);
            g.nrm.push([nx, ny, nz]);
        }
        for _ in 0..nt {
            let a = u32_at(&mut o)?;
            let c = u32_at(&mut o)?;
            let d = u32_at(&mut o)?;
            if (a as usize) < nv && (c as usize) < nv && (d as usize) < nv {
                g.idx.push([a, c, d]);
            }
        }
        groups.push(g);
    }
    Some(Mesh { groups })
}

#[derive(Clone, Copy)]
struct Light {
    p: V3,
    c: V3,
    i: f32,
    r: f32,
}

struct Part {
    p: V3,
    v: V3,
    life: f32,
    max: f32,
    s0: f32,
    s1: f32,
    c0: u32,
    c1: u32,
    kind: u8,
    tex: u32,
    grav: f32,
    spin: f32,
    alpha: f32,
}

/// A triangle soup in a uniform grid, for `coll_sphere` / `coll_ray`.
struct Coll {
    tris: Vec<[V3; 3]>,
    nrm: Vec<V3>,
    lo: V3,
    cell: f32,
    dims: [i32; 3],
    cells: Vec<Vec<u32>>,
    stamp: Vec<u32>,
    q: u32,
}

impl Coll {
    fn build(tris: Vec<[V3; 3]>, nrm: Vec<V3>, cell: f32) -> Coll {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for t in &tris {
            for p in t {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
        }
        if tris.is_empty() {
            lo = [0.0; 3];
            hi = [1.0; 3];
        }
        let mut cell = cell.max(0.5);
        let mut dims;
        loop {
            dims = [0i32; 3];
            for k in 0..3 {
                dims[k] = (((hi[k] - lo[k]) / cell) as i32 + 1).max(1);
            }
            if (dims[0] as i64) * (dims[1] as i64) * (dims[2] as i64) <= 1 << 18 {
                break;
            }
            cell *= 2.0;
        }
        let total = (dims[0] * dims[1] * dims[2]) as usize;
        let mut cells: Vec<Vec<u32>> = (0..total).map(|_| Vec::new()).collect();
        let idx = |c: &Coll, i: i32, j: i32, k: i32| -> usize { ((k * c.dims[1] + j) * c.dims[0] + i) as usize };
        let mut c = Coll { stamp: vec![0; tris.len()], tris, nrm, lo, cell, dims, cells: Vec::new(), q: 0 };
        for (ti, t) in c.tris.iter().enumerate() {
            let (a, b) = c.range(t);
            for k in a[2]..=b[2] {
                for j in a[1]..=b[1] {
                    for i in a[0]..=b[0] {
                        cells[idx(&c, i, j, k)].push(ti as u32);
                    }
                }
            }
        }
        c.cells = cells;
        c
    }
    fn cell_of(&self, p: V3) -> [i32; 3] {
        let mut r = [0i32; 3];
        for k in 0..3 {
            r[k] = (((p[k] - self.lo[k]) / self.cell) as i32).clamp(0, self.dims[k] - 1);
        }
        r
    }
    fn range(&self, t: &[V3; 3]) -> ([i32; 3], [i32; 3]) {
        let mut lo = t[0];
        let mut hi = t[0];
        for p in &t[1..] {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (self.cell_of(lo), self.cell_of(hi))
    }
    fn query(&mut self, lo: V3, hi: V3, out: &mut Vec<u32>) {
        out.clear();
        self.q = self.q.wrapping_add(1);
        if self.q == 0 {
            self.stamp.fill(0);
            self.q = 1;
        }
        let a = self.cell_of(lo);
        let b = self.cell_of(hi);
        for k in a[2]..=b[2] {
            for j in a[1]..=b[1] {
                for i in a[0]..=b[0] {
                    let ci = ((k * self.dims[1] + j) * self.dims[0] + i) as usize;
                    for &t in &self.cells[ci] {
                        if self.stamp[t as usize] != self.q {
                            self.stamp[t as usize] = self.q;
                            out.push(t);
                        }
                    }
                }
            }
        }
    }
}

/// Closest point on triangle `abc` to `p` (Ericson, Real-Time Collision Detection).
fn closest_on_tri(p: V3, a: V3, b: V3, c: V3) -> V3 {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return [a[0] + ab[0] * v, a[1] + ab[1] * v, a[2] + ab[2] * v];
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return [a[0] + ac[0] * w, a[1] + ac[1] * w, a[2] + ac[2] * w];
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return [b[0] + (c[0] - b[0]) * w, b[1] + (c[1] - b[1]) * w, b[2] + (c[2] - b[2]) * w];
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    [a[0] + ab[0] * v + ac[0] * w, a[1] + ab[1] * v + ac[1] * w, a[2] + ab[2] * v + ac[2] * w]
}

// ── camera ───────────────────────────────────────────────────────────────────

struct Cam {
    cry: f32,
    sry: f32,
    crx: f32,
    srx: f32,
    /// Focal length in pixels; 0 = the window height (the host default).
    focal: f32,
    zdist: f32,
    t: V3,
    /// Screen centre override (`set_camera_center`); None = the window centre.
    center: Option<(f32, f32)>,
    /// `set_camera_hyper(r, strength, hx, hy, hz)`: r 0 = off.
    hyp: [f32; 5],
    /// tan of the half view angle (centre x / focal), for the warp's view cone.
    tan_h: f32,
}

impl Cam {
    /// The host's hyperbolic warp (`gfx/camera.rs` `hyper`): around the hub
    /// (hx, hz) distance compresses like a Poincare disc and the ground
    /// saddles; only points in front of the lens and near the view bend.
    #[inline]
    fn warp(&self, p: V3) -> V3 {
        let r = self.hyp[0];
        if r <= 0.0 {
            return p;
        }
        let (vx, vy, vz) = (p[0] - self.t[0], p[1] - self.t[1], p[2] - self.t[2]);
        let qx = vx * self.cry - vz * self.sry;
        let qz1 = vx * self.sry + vz * self.cry;
        let qy = vy * self.crx - qz1 * self.srx;
        let qz = vy * self.srx + qz1 * self.crx;
        let eye = qz + self.zdist;
        let k_front = ((eye - 1.0) / 6.0).clamp(0.0, 1.0);
        if k_front <= 0.0 {
            return p;
        }
        let q = sqrt(qx * qx + qy * qy) / (eye.max(0.1) * self.tan_h.max(0.05));
        let k = k_front * ((1.8 - q) / 0.6).clamp(0.0, 1.0);
        if k <= 0.0 {
            return p;
        }
        let (hx, hy, hz) = (self.hyp[2], self.hyp[3], self.hyp[4]);
        let (mut dx, mut dz) = (p[0] - hx, p[2] - hz);
        let d = sqrt(dx * dx + dz * dz);
        if d > 1e-4 {
            let sc = r * libm::tanhf(d / r) / d;
            dx *= sc;
            dz *= sc;
        }
        let u = dx * self.cry - dz * self.sry;
        let v = dx * self.sry + dz * self.cry;
        let raw = self.hyp[1] * (v * v - u * u) / r;
        let fy = ((p[1] - hy) / 12.0).clamp(-1.0, 1.0);
        let wy2 = p[1] + 9.0 * libm::tanhf(raw / 9.0) * fy;
        [p[0] + (hx + dx - p[0]) * k, p[1] + (wy2 - p[1]) * k, p[2] + (hz + dz - p[2]) * k]
    }
    /// The edge midpoints (ab, bc, ca) of a triangle the lens would visibly bend between its
    /// corners, else `None` -- see the host's game3d `hyper_split` for the error bound.
    fn hyper_split(&self, wp: &[V3; 3]) -> Option<[V3; 3]> {
        let r = self.hyp[0];
        if r <= 0.0 {
            return None;
        }
        let e2 = |i: usize, j: usize| {
            let (dx, dz) = (wp[i][0] - wp[j][0], wp[i][2] - wp[j][2]);
            dx * dx + dz * dz
        };
        let longest = e2(0, 1).max(e2(1, 2)).max(e2(2, 0));
        let s = self.hyp[1].abs().max(0.01);
        if longest <= (0.6 * r / s).max(4.0) || longest > 64.0 * 64.0 {
            return None;
        }
        let cx = (wp[0][0] + wp[1][0] + wp[2][0]) / 3.0 - self.hyp[2];
        let cz = (wp[0][2] + wp[1][2] + wp[2][2]) / 3.0 - self.hyp[4];
        let reach = r * 2.5 + 64.0;
        if cx * cx + cz * cz > reach * reach {
            return None;
        }
        Some([lerp3(wp[0], wp[1], 0.5), lerp3(wp[1], wp[2], 0.5), lerp3(wp[2], wp[0], 0.5)])
    }
    #[inline]
    fn depth(&self, p: V3) -> f32 {
        let p = self.warp(p);
        let wx = p[0] - self.t[0];
        let wy = p[1] - self.t[1];
        let wz = p[2] - self.t[2];
        let rz1 = wx * self.sry + wz * self.cry;
        wy * self.srx + rz1 * self.crx
    }
    /// (screen x, screen y, camera depth) with the view's centre and focal length.
    #[inline]
    fn project(&self, p: V3, cx: f32, cy: f32, focal: f32) -> (f32, f32, f32) {
        let p = self.warp(p);
        let wx = p[0] - self.t[0];
        let wy = p[1] - self.t[1];
        let wz = p[2] - self.t[2];
        let rx = wx * self.cry - wz * self.sry;
        let rz1 = wx * self.sry + wz * self.cry;
        let ry = wy * self.crx - rz1 * self.srx;
        let rz = wy * self.srx + rz1 * self.crx;
        let d = rz + self.zdist;
        (cx + focal * rx / d, cy + focal * ry / d, rz)
    }
}

// ── rasterizer ───────────────────────────────────────────────────────────────

/// A projected vertex: screen position, 1/w and the w-divided attributes.
#[derive(Clone, Copy, Default)]
struct A {
    x: f32,
    y: f32,
    iw: f32,
    uw: f32,
    vw: f32,
    wx: f32,
    wy: f32,
    wz: f32,
    /// Vertex colour (0..1): the lit colour of a shape, or the tint of a textured face.
    r: f32,
    g: f32,
    b: f32,
}

impl A {
    #[inline]
    fn lerp(&self, o: &A, t: f32) -> A {
        A {
            x: self.x + (o.x - self.x) * t,
            y: self.y + (o.y - self.y) * t,
            iw: self.iw + (o.iw - self.iw) * t,
            uw: self.uw + (o.uw - self.uw) * t,
            vw: self.vw + (o.vw - self.vw) * t,
            wx: self.wx + (o.wx - self.wx) * t,
            wy: self.wy + (o.wy - self.wy) * t,
            wz: self.wz + (o.wz - self.wz) * t,
            r: self.r + (o.r - self.r) * t,
            g: self.g + (o.g - self.g) * t,
            b: self.b + (o.b - self.b) * t,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum TexId {
    None,
    Glow,
    Soft,
    Solid,
    Id(u32),
}

#[derive(Clone, Copy)]
struct Params {
    tex: TexId,
    alpha: f32,
    blend: u8,
    zwrite: bool,
    ztest: bool,
    /// Point lights per 8-pixel block (`tm_draw_lit`), with this ambient.
    lit: bool,
    amb: f32,
    fog: bool,
    /// Projected see-through circle: [sx, sy, radius, depth threshold]; radius 0 = off.
    hole: [f32; 4],
    /// 2-D (`tm_sprite`): affine, no depth.
    sprite: bool,
}

struct Deferred {
    v: [A; 3],
    p: Params,
    /// Sort key: the nearest 1/w of the triangle (small = far = drawn first).
    key: f32,
}

#[inline]
fn light_at(lights: &[Light], amb: f32, p: V3) -> V3 {
    let mut l = [amb, amb, amb];
    for li in lights {
        let dx = p[0] - li.p[0];
        let dy = p[1] - li.p[1];
        let dz = p[2] - li.p[2];
        let d2 = dx * dx + dy * dy + dz * dz;
        let r2 = li.r * li.r;
        if d2 < r2 {
            let k = 1.0 - sqrt(d2 / r2);
            let e = k * k * li.i;
            l[0] += li.c[0] * e;
            l[1] += li.c[1] * e;
            l[2] += li.c[2] * e;
        }
    }
    l
}

/// A pixel / depth plane the span loop indexes. A raw pointer, not a slice,
/// because several cores rasterize into it at once -- each into its own rows
/// (`Env::y_lo..y_hi`), so no pixel is ever written by two of them.
#[derive(Clone, Copy)]
struct Px<T> {
    p: *mut T,
    len: usize,
}

impl<T> core::ops::Index<usize> for Px<T> {
    type Output = T;
    #[inline(always)]
    fn index(&self, i: usize) -> &T {
        debug_assert!(i < self.len);
        unsafe { &*self.p.add(i) }
    }
}

impl<T> core::ops::IndexMut<usize> for Px<T> {
    #[inline(always)]
    fn index_mut(&mut self, i: usize) -> &mut T {
        debug_assert!(i < self.len);
        unsafe { &mut *self.p.add(i) }
    }
}

/// Everything the span loop needs besides the triangle itself.
struct Env<'a> {
    buf: Px<u32>,
    bw: usize,
    bh: usize,
    zb: Px<f32>,
    /// The rows this call may touch: a band of the target when the cores split a batch.
    y_lo: i32,
    y_hi: i32,
    tex: Option<&'a Tex>,
    lights: &'a [Light],
    fog_color: u32,
    fog_start: f32,
    fog_end: f32,
    zdist: f32,
}

const BLOCK: i32 = 8;
const UOFF: f32 = 4096.0;

/// Scanline-fill one screen-space triangle.
fn raster(env: &mut Env, v: &[A; 3], p: &Params) {
    let (bw, bh) = (env.bw as i32, env.bh as i32);
    if bw <= 0 || bh <= 0 {
        return;
    }
    for a in v {
        if !(a.x.is_finite() && a.y.is_finite() && a.iw.is_finite()) {
            return;
        }
    }
    // sort by y
    let mut s = [v[0], v[1], v[2]];
    if s[1].y < s[0].y {
        s.swap(0, 1);
    }
    if s[2].y < s[1].y {
        s.swap(1, 2);
    }
    if s[1].y < s[0].y {
        s.swap(0, 1);
    }
    let (t0, t1, t2) = (s[0], s[1], s[2]);
    if t2.y - t0.y < 1e-6 {
        return;
    }
    let y_start = (ceil(t0.y - 0.5) as i32).max(0).max(env.y_lo);
    let y_end = (ceil(t2.y - 0.5) as i32).min(bh).min(env.y_hi); // exclusive
    if y_start >= y_end {
        return;
    }
    let alpha8 = (p.alpha.clamp(0.0, 1.0) * 256.0) as u32;
    if alpha8 == 0 {
        return;
    }
    let fog_on = p.fog && env.fog_end > env.fog_start && env.fog_end > 0.0;
    let (fcr, fcg, fcb) = (((env.fog_color >> 16) & 255) as i32, ((env.fog_color >> 8) & 255) as i32, (env.fog_color & 255) as i32);
    let fog_inv = if fog_on { 1.0 / (env.fog_end - env.fog_start) } else { 0.0 };
    let hole_on = p.hole[2] > 0.0;
    let (tw, mask, tsh) = match env.tex {
        Some(t) => (t.w as f32, t.w - 1, t.sh),
        None => (1.0, 0, 0),
    };
    let texpx: &[u32] = env.tex.map(|t| &t.px[..]).unwrap_or(&[]);
    let zd = env.zdist;
    let ztest = p.ztest && !p.sprite;
    let zwrite = p.zwrite && !p.sprite;
    let blend = p.blend;
    let path: u8 = if alpha8 >= 256 && blend == 0 && !fog_on && !hole_on {
        if texpx.is_empty() {
            1
        } else {
            2
        }
    } else {
        0
    };

    for py in y_start..y_end {
        let fy = py as f32 + 0.5;
        // the long edge t0→t2 and the short edge for this row
        let tl = (fy - t0.y) / (t2.y - t0.y);
        let la = t0.lerp(&t2, tl);
        let sa = if fy < t1.y {
            let d = t1.y - t0.y;
            if d < 1e-9 {
                t1
            } else {
                t0.lerp(&t1, (fy - t0.y) / d)
            }
        } else {
            let d = t2.y - t1.y;
            if d < 1e-9 {
                t1
            } else {
                t1.lerp(&t2, (fy - t1.y) / d)
            }
        };
        let (l, r) = if la.x <= sa.x { (la, sa) } else { (sa, la) };
        let span = r.x - l.x;
        if span < 1e-6 {
            continue;
        }
        let x_start = (ceil(l.x - 0.5) as i32).max(0);
        let x_end = (ceil(r.x - 0.5) as i32).min(bw); // exclusive
        if x_start >= x_end {
            continue;
        }
        let inv_span = 1.0 / span;
        let row = py as usize * env.bw;
        // attribute at pixel centre x + 0.5: l + (x + 0.5 - l.x) * d
        let at = |x: i32| -> A { l.lerp(&r, ((x as f32 + 0.5) - l.x) * inv_span) };

        let mut x = x_start;
        let mut a0 = at(x);
        let mut inv0 = if a0.iw > 1e-12 { 1.0 / a0.iw } else { 1e12 };
        let mut l0 = if p.lit { light_at(env.lights, p.amb, [a0.wx * inv0, a0.wy * inv0, a0.wz * inv0]) } else { [1.0; 3] };
        while x < x_end {
            let bl = (x_end - x).min(BLOCK);
            let xe = x + bl;
            let a1 = at(xe);
            let inv1 = if a1.iw > 1e-12 { 1.0 / a1.iw } else { 1e12 };
            let l1 = if p.lit { light_at(env.lights, p.amb, [a1.wx * inv1, a1.wy * inv1, a1.wz * inv1]) } else { [1.0; 3] };
            let z0 = inv0 - zd;
            let z1 = inv1 - zd;
            let blf = bl as f32;
            // texel coordinates, 16.16 fixed, offset to stay positive (wrap is a power of two)
            let u0 = ((a0.uw * inv0 + UOFF) * tw * 65536.0) as i64;
            let u1 = ((a1.uw * inv1 + UOFF) * tw * 65536.0) as i64;
            let v0 = ((a0.vw * inv0 + UOFF) * tw * 65536.0) as i64;
            let v1 = ((a1.vw * inv1 + UOFF) * tw * 65536.0) as i64;
            let du = (u1 - u0) / bl as i64;
            let dv = (v1 - v0) / bl as i64;
            // colour x light, 16.16 (65536 = 1.0)
            let m0 = [(a0.r * l0[0] * 65536.0) as i32, (a0.g * l0[1] * 65536.0) as i32, (a0.b * l0[2] * 65536.0) as i32];
            let m1 = [(a1.r * l1[0] * 65536.0) as i32, (a1.g * l1[1] * 65536.0) as i32, (a1.b * l1[2] * 65536.0) as i32];
            let dm = [(m1[0] - m0[0]) / bl, (m1[1] - m0[1]) / bl, (m1[2] - m0[2]) / bl];
            let (mut mr, mut mg, mut mb) = (m0[0], m0[1], m0[2]);
            // fog factor, 16.16
            let (mut ff, dff) = if fog_on {
                let f0 = (((z0 - env.fog_start) * fog_inv).clamp(0.0, 1.0) * 65536.0) as i32;
                let f1 = (((z1 - env.fog_start) * fog_inv).clamp(0.0, 1.0) * 65536.0) as i32;
                (f0, (f1 - f0) / bl)
            } else {
                (0, 0)
            };
            let mut iw = a0.iw;
            let diw = (a1.iw - a0.iw) / blf;
            let dz = (z1 - z0) / blf;
            let mut z = z0;
            let (mut u, mut vv) = (u0, v0);
            match path {
                // opaque, untextured, no fog / hole: shapes
                1 => {
                    for px in x..xe {
                        let idx = row + px as usize;
                        if !ztest || iw >= env.zb[idx] {
                            let sr = ((mr >> 8).clamp(0, 256) * 255) >> 8;
                            let sg = ((mg >> 8).clamp(0, 256) * 255) >> 8;
                            let sb = ((mb >> 8).clamp(0, 256) * 255) >> 8;
                            env.buf[idx] = ((sr as u32) << 16) | ((sg as u32) << 8) | sb as u32;
                            if zwrite {
                                env.zb[idx] = iw;
                            }
                        }
                        iw += diw;
                        mr += dm[0];
                        mg += dm[1];
                        mb += dm[2];
                    }
                },
                // opaque textured, no fog / hole: meshes and cut-out textures
                2 => {
                    for px in x..xe {
                        let idx = row + px as usize;
                        if !ztest || iw >= env.zb[idx] {
                            let t = texpx[((((vv >> 16) as u32) & mask) << tsh | (((u >> 16) as u32) & mask)) as usize];
                            let a = t >> 24;
                            if a > 0 {
                                let sr = ((((t >> 16) & 255) as i32 * (mr >> 8)) >> 8).clamp(0, 255);
                                let sg = ((((t >> 8) & 255) as i32 * (mg >> 8)) >> 8).clamp(0, 255);
                                let sb = (((t & 255) as i32 * (mb >> 8)) >> 8).clamp(0, 255);
                                if a >= 255 {
                                    env.buf[idx] = ((sr as u32) << 16) | ((sg as u32) << 8) | sb as u32;
                                    if zwrite {
                                        env.zb[idx] = iw;
                                    }
                                } else {
                                    let d = env.buf[idx];
                                    let (dr, dg, db) = (((d >> 16) & 255) as i32, ((d >> 8) & 255) as i32, (d & 255) as i32);
                                    let a = a as i32;
                                    let r = dr + (((sr - dr) * a) >> 8);
                                    let g = dg + (((sg - dg) * a) >> 8);
                                    let b = db + (((sb - db) * a) >> 8);
                                    env.buf[idx] = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
                                    if zwrite && a > 247 {
                                        env.zb[idx] = iw;
                                    }
                                }
                            }
                        }
                        iw += diw;
                        u += du;
                        vv += dv;
                        mr += dm[0];
                        mg += dm[1];
                        mb += dm[2];
                    }
                },
                _ => {
                    for px in x..xe {
                        let idx = row + px as usize;
                        if !ztest || iw >= env.zb[idx] {
                            let t = if texpx.is_empty() { 0xFFFF_FFFFu32 } else { texpx[((((vv >> 16) as u32) & mask) << tsh | (((u >> 16) as u32) & mask)) as usize] };
                            let mut a = ((t >> 24) * alpha8) >> 8;
                            if hole_on && a > 0 && z < p.hole[3] {
                                let dx = px as f32 + 0.5 - p.hole[0];
                                let dy = py as f32 + 0.5 - p.hole[1];
                                let d = sqrt(dx * dx + dy * dy);
                                let k = ((d - p.hole[2] * 0.55) / (p.hole[2] * 0.45)).clamp(0.0, 1.0);
                                let k = k * k * (3.0 - 2.0 * k);
                                a = (a as f32 * (0.12 + 0.88 * k)) as u32;
                            }
                            if a > 0 {
                                let mut sr = ((((t >> 16) & 255) as i32 * (mr >> 8)) >> 8).clamp(0, 255);
                                let mut sg = ((((t >> 8) & 255) as i32 * (mg >> 8)) >> 8).clamp(0, 255);
                                let mut sb = (((t & 255) as i32 * (mb >> 8)) >> 8).clamp(0, 255);
                                if fog_on {
                                    sr += ((fcr - sr) * (ff >> 8)) >> 8;
                                    sg += ((fcg - sg) * (ff >> 8)) >> 8;
                                    sb += ((fcb - sb) * (ff >> 8)) >> 8;
                                }
                                if a >= 255 && blend == 0 {
                                    env.buf[idx] = ((sr as u32) << 16) | ((sg as u32) << 8) | sb as u32;
                                } else {
                                    let d = env.buf[idx];
                                    let (dr, dg, db) = (((d >> 16) & 255) as i32, ((d >> 8) & 255) as i32, (d & 255) as i32);
                                    let a = a.min(255) as i32;
                                    let (r, g, b) = if blend == 1 {
                                        ((dr + ((sr * a) >> 8)).min(255), (dg + ((sg * a) >> 8)).min(255), (db + ((sb * a) >> 8)).min(255))
                                    } else {
                                        (dr + (((sr - dr) * a) >> 8), dg + (((sg - dg) * a) >> 8), db + (((sb - db) * a) >> 8))
                                    };
                                    env.buf[idx] = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
                                }
                                if zwrite && a > 247 {
                                    env.zb[idx] = iw;
                                }
                            }
                        }
                        iw += diw;
                        z += dz;
                        u += du;
                        vv += dv;
                        mr += dm[0];
                        mg += dm[1];
                        mb += dm[2];
                        ff += dff;
                    }
                },
            }
            x = xe;
            a0 = a1;
            inv0 = inv1;
            l0 = l1;
        }
    }
}

/// A 2-D line in the window (wireframe shapes); no depth.
fn line2d(buf: &mut [u32], bw: usize, bh: usize, x0: f32, y0: f32, x1: f32, y1: f32, c: u32) {
    if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
        return;
    }
    let dx = x1 - x0;
    let dy = y1 - y0;
    let steps = (if dx.abs() > dy.abs() { dx.abs() } else { dy.abs() }).min(4096.0) as i32 + 1;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = (x0 + dx * t) as i32;
        let y = (y0 + dy * t) as i32;
        if x >= 0 && y >= 0 && (x as usize) < bw && (y as usize) < bh {
            buf[y as usize * bw + x as usize] = c;
        }
    }
}

// ── primitive shapes ─────────────────────────────────────────────────────────

struct Base {
    verts: Vec<V3>,
    nrm: Vec<V3>,
    tris: Vec<[u32; 3]>,
    edges: Vec<[u32; 2]>,
}

impl Base {
    fn finish(mut self) -> Base {
        // area-weighted smooth normals
        let mut nn = vec![[0.0f32; 3]; self.verts.len()];
        for t in &self.tris {
            let a = self.verts[t[0] as usize];
            let b = self.verts[t[1] as usize];
            let c = self.verts[t[2] as usize];
            let fnr = cross(sub(b, a), sub(c, a));
            for &i in t {
                let v = &mut nn[i as usize];
                v[0] += fnr[0];
                v[1] += fnr[1];
                v[2] += fnr[2];
            }
        }
        self.nrm = nn.into_iter().map(norm).collect();
        if self.edges.is_empty() {
            let mut seen: alloc::collections::BTreeSet<(u32, u32)> = alloc::collections::BTreeSet::new();
            for t in &self.tris {
                for &(a, b) in &[(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    let k = if a < b { (a, b) } else { (b, a) };
                    if seen.insert(k) {
                        self.edges.push([k.0, k.1]);
                    }
                }
            }
        }
        self
    }
}

fn base_cube() -> Base {
    let mut verts = Vec::new();
    for i in 0..8 {
        verts.push([if i & 1 != 0 { 1.0 } else { -1.0 }, if i & 2 != 0 { 1.0 } else { -1.0 }, if i & 4 != 0 { 1.0 } else { -1.0 }]);
    }
    // faces as index quads (outward winding is not needed: shapes draw uncullled)
    let faces: [[u32; 4]; 6] = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    let mut tris = Vec::new();
    let mut edges = Vec::new();
    for q in &faces {
        tris.push([q[0], q[1], q[2]]);
        tris.push([q[0], q[2], q[3]]);
        for k in 0..4 {
            let (a, b) = (q[k], q[(k + 1) % 4]);
            let e = if a < b { [a, b] } else { [b, a] };
            if !edges.contains(&e) {
                edges.push(e);
            }
        }
    }
    Base { verts, nrm: Vec::new(), tris, edges }.finish()
}

fn base_sphere(seg: i32, rings: i32) -> Base {
    let seg = seg.clamp(3, 96);
    let rings = rings.clamp(2, 64);
    let mut verts = Vec::new();
    let mut tris = Vec::new();
    for r in 0..=rings {
        let phi = PI * r as f32 / rings as f32;
        let (y, rad) = (cos(phi), sin(phi));
        for s in 0..seg {
            let th = TAU * s as f32 / seg as f32;
            verts.push([rad * cos(th), y, rad * sin(th)]);
        }
    }
    for r in 0..rings {
        for s in 0..seg {
            let a = (r * seg + s) as u32;
            let b = (r * seg + (s + 1) % seg) as u32;
            let c = ((r + 1) * seg + s) as u32;
            let d = ((r + 1) * seg + (s + 1) % seg) as u32;
            if r > 0 {
                tris.push([a, b, c]);
            }
            if r < rings - 1 {
                tris.push([b, d, c]);
            }
        }
    }
    Base { verts, nrm: Vec::new(), tris, edges: Vec::new() }.finish()
}

fn base_capsule(seg: i32, rings: i32) -> Base {
    // a sphere whose upper hemisphere is lifted by 1 and lower lowered by 1 (height 2 cylinder + caps),
    // normalised afterwards to span [-1, 1] in y
    let mut b = base_sphere(seg, rings * 2);
    for v in &mut b.verts {
        v[1] = if v[1] > 0.0 { (v[1] + 1.0) * 0.5 } else { (v[1] - 1.0) * 0.5 };
        v[0] *= 0.5;
        v[2] *= 0.5;
    }
    Base { verts: b.verts, nrm: Vec::new(), tris: b.tris, edges: Vec::new() }.finish()
}

fn base_tube(seg: i32, top_r: f32) -> Base {
    let seg = seg.clamp(3, 96);
    let mut verts = Vec::new();
    let mut tris = Vec::new();
    for s in 0..seg {
        let th = TAU * s as f32 / seg as f32;
        verts.push([cos(th), 1.0, sin(th)]); // bottom ring (y down)
    }
    for s in 0..seg {
        let th = TAU * s as f32 / seg as f32;
        verts.push([top_r * cos(th), -1.0, top_r * sin(th)]); // top ring
    }
    let cb = verts.len() as u32;
    verts.push([0.0, 1.0, 0.0]);
    let ct = verts.len() as u32;
    verts.push([0.0, -1.0, 0.0]);
    for s in 0..seg {
        let a = s as u32;
        let b = ((s + 1) % seg) as u32;
        let c = seg as u32 + a;
        let d = seg as u32 + b;
        tris.push([a, b, c]);
        if top_r > 1e-4 {
            tris.push([b, d, c]);
        }
        tris.push([a, cb, b]);
        if top_r > 1e-4 {
            tris.push([c, d, ct]);
        }
    }
    Base { verts, nrm: Vec::new(), tris, edges: Vec::new() }.finish()
}

fn base_torus(seg: i32, sides: i32, tube: f32) -> Base {
    let seg = seg.clamp(3, 96);
    let sides = sides.clamp(3, 48);
    let mut verts = Vec::new();
    let mut tris = Vec::new();
    for i in 0..seg {
        let th = TAU * i as f32 / seg as f32;
        let (c, s) = (cos(th), sin(th));
        for j in 0..sides {
            let ph = TAU * j as f32 / sides as f32;
            let r = 1.0 - tube + tube * cos(ph);
            verts.push([r * c, tube * sin(ph), r * s]);
        }
    }
    for i in 0..seg {
        for j in 0..sides {
            let a = (i * sides + j) as u32;
            let b = (i * sides + (j + 1) % sides) as u32;
            let c = (((i + 1) % seg) * sides + j) as u32;
            let d = (((i + 1) % seg) * sides + (j + 1) % sides) as u32;
            tris.push([a, b, c]);
            tris.push([b, d, c]);
        }
    }
    Base { verts, nrm: Vec::new(), tris, edges: Vec::new() }.finish()
}

fn cel_ramp(d: f32) -> f32 {
    let lo = 0.30;
    let hi = 0.55;
    if d < lo {
        0.20
    } else if d > hi {
        1.0
    } else {
        0.20 + 0.80 * (d - lo) / (hi - lo)
    }
}

fn posterize(c: V3, bands: u32) -> V3 {
    let bands = bands.max(2) as f32;
    let lum = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
    if lum < 1e-5 {
        return c;
    }
    let band = floor(lum * bands).min(bands - 1.0);
    let q = (band + 0.5) / bands;
    let k = (q / lum).clamp(0.0, 4.0);
    [(c[0] * k).min(1.0), (c[1] * k).min(1.0), (c[2] * k).min(1.0)]
}

// ── state ────────────────────────────────────────────────────────────────────

struct State {
    cam: Cam,
    lights: Vec<Light>,
    ambient: f32,
    fog_color: u32,
    fog_start: f32,
    fog_end: f32,
    shade_mode: u8,
    rim: f32,
    rim_color: V3,
    bands: u32,
    blend: u8,
    depth_test: bool,
    /// `set_cull(mode)`, as the host's: 0 off (double-sided) · 1 drop triangles wound clockwise on
    /// screen · 2 drop counter-clockwise · 3 auto (meshes keep the cull their draw call asks for).
    cull: u8,
    zbuf: Vec<f32>,
    zw: usize,
    zh: usize,
    zclear: bool,
    texs: Vec<Tex>,
    glow: Tex,
    soft: Tex,
    solid: Tex,
    meshes: Vec<Mesh>,
    colls: Vec<Coll>,
    parts: Vec<Part>,
    max_parts: usize,
    hole: [f32; 4],
    deferred: Vec<Deferred>,
    bases: BTreeMap<(u8, i32, i32), Base>,
    rng: u32,
    scratch: Vec<u32>,
    pub tris: usize,
    /// Render scale: 1 = the window, 0.5 = a quarter of the pixels, upscaled at flush.
    scale: f32,
    auto_scale: bool,
    rbuf: Vec<u32>,
    rw: usize,
    rh: usize,
    frame_open: bool,
    /// The window's pixels at the start of a layered frame (decimated like `rbuf`): a 3-D pixel is one
    /// that differs from it, and only those are written back to the window.
    rback: Vec<u32>,
    /// 8-row bands of the layer already filled from the window; the rect (x0, y0, x1, y1) 3-D drew on.
    band_ok: Vec<bool>,
    dirty: [usize; 4],
    rmask: Vec<u8>,
    /// `flush_3d_clip` was used: 3-D then renders in a layer even at full scale, so the clip can crop it.
    clip_seen: bool,
    /// The program that set the render scale; another program starting resets it.
    app: String,
    win_w: usize,
    win_h: usize,
    /// Microseconds spent rasterizing / post-processing / upscaling this frame.
    raster_us: u64,
    slow_frames: u32,
    fast_frames: u32,
    /// Toon post: ink outline half-width in pixels (0 = off), colour, depth threshold.
    toon_px: f32,
    toon_color: u32,
    toon_thresh: f32,
    /// The host's 3-band tone ramp as a brightness-scale table (8.8) per luminance.
    ramp_lut: [u16; 256],
    ramp_on: bool,
    drew_3d: bool,
    post_done: bool,
    /// `.lmesh` colour meshes (`mesh_load` / `mesh_draw`).
    cmeshes: Vec<CMesh>,
    /// glTF (model, mesh) -> texture id in `texs`, built on first draw.
    gltex: BTreeMap<(usize, usize), u32>,
    /// `tm_lens_guard`: triangles with a corner this close to the near plane are dropped.
    lens_guard: f32,
    /// Finished `terrain_grid_start` jobs by id (computed at once; polled later).
    tgrids: Vec<(String, TGrid)>,
    tgrid_seq: u32,
    /// Triangles projected but not rasterized yet, all for `batch_target` (pixels, width, height):
    /// rasterized together by `sync`, split over the cores in bands of rows.
    batch: Vec<([A; 3], Params)>,
    batch_target: (usize, usize, usize),
    /// Their summed bounding-box area in pixels (decides whether splitting is worth it).
    batch_area: f32,
}

/// Triangles are waiting in the batch. `lingrt::ling_builtin` reads this before every builtin and
/// rasterizes them first unless the builtin is one that only adds more (`batch_ok`), so nothing
/// else ever sees the target before the triangles drawn earlier are on it.
pub static mut PENDING: bool = false;

/// Builtins that may run while triangles wait: they only queue more, move the camera, set the pen,
/// or compute -- they never read or write pixels, resize a buffer, or change what rasterizing reads
/// (lights, fog, depth, textures, the render scale). Anything not listed draws the batch first.
pub fn batch_ok(name: &[u8]) -> bool {
    matches!(
        name,
        b"draw_triangle_3d"
            | b"triangle3d"
            | b"mesh_draw"
            | b"txm_draw"
            | b"txm_quad"
            | b"txm_tri"
            | b"gltf_draw"
            | b"gltf_anim_draw"
            | b"gltf_anim_draw_tex"
            | b"gltf_pose_draw"
            | b"gltf_pose_draw_tex"
            | b"gltf_mesh_show"
            | b"gltf_mesh_show_list"
            | b"image_pixel_u16"
            | b"image_height_tri"
            | b"gltf_mesh_tint"
            | b"gltf_bounds"
            | b"gltf_tris"
            | b"gltf_anim_count"
            | b"gltf_anim_dur"
            | b"terrain_draw_grid"
            | b"terrain_elev"
            | b"project_3d"
            | b"hyper_warp"
            | b"mouse_ray"
            | b"set_camera"
            | b"set_camera_pos"
            | b"move_camera"
            | b"set_camera_hyper"
            | b"cube"
            | b"box"
            | b"sphere"
            | b"cylinder"
            | b"cone"
            | b"capsule"
            | b"torus"
            | b"frustum"
            | b"tm_draw"
            | b"tm_draw_lit"
            | b"tm_quad"
            | b"tm_billboard"
            | b"coll_sphere"
            | b"coll_ray"
            | b"set_color"
            | b"set_alpha"
            | b"set_blend"
            | b"sin"
            | b"cos"
            | b"tan"
            | b"sqrt"
            | b"abs"
            | b"floor"
            | b"ceil"
            | b"round"
            | b"min"
            | b"max"
            | b"clamp"
            | b"atan2"
            | b"pow"
            | b"exp"
            | b"len"
            | b"list_get"
            | b"list_set"
            | b"list_push"
            | b"st_get"
            | b"st_set"
            | b"lerp"
    )
}

/// Rasterize whatever waits in the batch (see [`PENDING`]).
pub fn sync_pending() {
    if unsafe { PENDING } {
        st().sync();
    }
}

/// An `.lmesh`: unlit triangles with one colour each.
struct CMesh {
    pos: Vec<V3>,
    col: Vec<[u8; 3]>,
}

/// One `terrain_grid_start` result (the host's TerrainGridResult).
struct TGrid {
    bx: f64,
    bz: f64,
    corners: Vec<f64>,
    nx: Vec<f64>,
    ny: Vec<f64>,
    nz: Vec<f64>,
}

static mut G: Option<State> = None;

fn st() -> &'static mut State {
    let g = unsafe { &mut *(&raw mut G) };
    if g.is_none() {
        *g = Some(State {
            cam: Cam { cry: 1.0, sry: 0.0, crx: 1.0, srx: 0.0, focal: 0.0, zdist: 5.0, t: [0.0; 3], center: None, hyp: [0.0; 5], tan_h: 1.0 },
            lights: Vec::new(),
            ambient: 0.15,
            fog_color: 0,
            fog_start: 0.0,
            fog_end: 0.0,
            shade_mode: 1,
            rim: 0.6,
            rim_color: [115.0 / 255.0, 217.0 / 255.0, 1.0],
            bands: 4,
            blend: 0,
            depth_test: true,
            cull: 0,
            zbuf: Vec::new(),
            zw: 0,
            zh: 0,
            zclear: true,
            texs: Vec::new(),
            glow: glow_tex(),
            soft: soft_tex(),
            solid: Tex::new(8, vec![0xFFFF_FFFF; 64]),
            meshes: Vec::new(),
            colls: Vec::new(),
            parts: Vec::new(),
            max_parts: 6000,
            hole: [0.0; 4],
            deferred: Vec::new(),
            bases: BTreeMap::new(),
            rng: 0x9E37_79B9,
            scratch: Vec::new(),
            tris: 0,
            scale: 1.0,
            auto_scale: true,
            rbuf: Vec::new(),
            rw: 0,
            rh: 0,
            frame_open: false,
            rback: Vec::new(),
            band_ok: Vec::new(),
            dirty: [usize::MAX, usize::MAX, 0, 0],
            rmask: Vec::new(),
            clip_seen: false,
            app: String::new(),
            win_w: 0,
            win_h: 0,
            raster_us: 0,
            slow_frames: 0,
            fast_frames: 0,
            toon_px: 0.0,
            toon_color: 0,
            toon_thresh: 0.05,
            ramp_lut: build_ramp_lut(),
            ramp_on: true,
            drew_3d: false,
            post_done: false,
            cmeshes: Vec::new(),
            gltex: BTreeMap::new(),
            lens_guard: 0.0,
            tgrids: Vec::new(),
            tgrid_seq: 0,
            batch: Vec::new(),
            batch_target: (0, 0, 0),
            batch_area: 0.0,
        });
    }
    g.as_mut().unwrap()
}

/// Triangles rasterized since the last `take_stats` (for the frame-timing overlay).
pub fn take_stats() -> usize {
    let s = st();
    let n = s.tris;
    s.tris = 0;
    n
}

/// " 3d x0.5" while rendering below the window's resolution, "" otherwise.
pub fn scale_note() -> String {
    let s = st();
    if s.scale < 0.999 {
        alloc::format!(" 3d x{:.2}", s.scale)
    } else {
        String::new()
    }
}

// ── toon post: the host's tone ramp (gfx/toon.rs) as a table ─────────────────

fn smoothstep01(f: f32) -> f32 {
    let f = f.clamp(0.0, 1.0);
    f * f * (3.0 - 2.0 * f)
}

/// The default 3-band cel ramp: shadow 0.08 below 0.25, mid 0.5 below 0.6, lit 1.0;
/// soft 0.32 band edges, sheen 0.65 above 0.72.
fn sample_ramp(t: f32) -> f32 {
    const STOPS: [(f32, f32); 4] = [(0.0, 0.08), (0.25, 0.50), (0.60, 1.00), (1.0, 1.0)];
    const SOFT: f32 = 0.32;
    let t = t.clamp(0.0, 1.0);
    if t <= STOPS[0].0 {
        return STOPS[0].1;
    }
    let last = STOPS.len() - 1;
    if t >= STOPS[last].0 {
        return STOPS[last].1;
    }
    for i in 0..last {
        if t < STOPS[i + 1].0 {
            let v = STOPS[i].1;
            let gap_u = STOPS[i + 1].0 - STOPS[i].0;
            let gap_u2 = if i + 2 <= last { STOPS[i + 2].0 - STOPS[i + 1].0 } else { gap_u };
            let hw_u = 0.5 * SOFT * gap_u.min(gap_u2);
            if hw_u > 1e-6 && t > STOPS[i + 1].0 - hw_u {
                let s = smoothstep01((t - (STOPS[i + 1].0 - hw_u)) / (2.0 * hw_u));
                return v + s * (STOPS[i + 1].1 - v);
            }
            if i > 0 {
                let gap_l = STOPS[i].0 - STOPS[i - 1].0;
                let hw_l = 0.5 * SOFT * gap_l.min(gap_u);
                if hw_l > 1e-6 && t < STOPS[i].0 + hw_l {
                    let s = smoothstep01((t - (STOPS[i].0 - hw_l)) / (2.0 * hw_l));
                    return STOPS[i - 1].1 + s * (v - STOPS[i - 1].1);
                }
            }
            return v;
        }
    }
    STOPS[last].1
}

fn build_ramp_lut() -> [u16; 256] {
    let mut lut = [256u16; 256];
    for (lum, e) in lut.iter_mut().enumerate() {
        let l = lum as f32;
        if l < 1.5 {
            continue;
        }
        let t = l / 255.0;
        let mut v = sample_ramp(t);
        let k = smoothstep01((t - 0.72) / 0.20) * 0.65;
        v += (t - v) * k;
        let maxs = if l < 14.0 { 2.2 } else { 4.0 };
        let scale = (v * 255.0 / l).clamp(0.0, maxs);
        *e = (scale * 256.0) as u16;
    }
    lut
}

/// Luminance-preserving-hue tone ramp over the whole buffer (the host applies it every frame).
fn apply_ramp(buf: &mut [u32], lut: &[u16; 256]) {
    for p in buf.iter_mut() {
        let c = *p;
        let (r, g, b) = ((c >> 16) & 255, (c >> 8) & 255, c & 255);
        let lum = ((77 * r + 150 * g + 29 * b) >> 8) as usize;
        let s = lut[lum.min(255)] as u32;
        if s == 256 {
            continue;
        }
        let r2 = ((r * s) >> 8).min(255);
        let g2 = ((g * s) >> 8).min(255);
        let b2 = ((b * s) >> 8).min(255);
        *p = (r2 << 16) | (g2 << 8) | b2;
    }
}

/// Ink where the depth buffer (1/w) jumps: silhouettes and creases. `thresh` is in
/// camera depth units like the host's; a neighbour's depth difference is taken from
/// the 1/w difference scaled by w^2 (one divide per pixel).
#[allow(clippy::too_many_arguments)]
fn draw_outlines(buf: &mut [u32], zb: &[f32], w: usize, h: usize, thick: f32, color: u32, thresh: f32) {
    if w < 3 || h < 3 || zb.len() < w * h || buf.len() < w * h {
        return;
    }
    let t = thick.clamp(0.5, 6.0);
    let ti = ceil(t) as i32;
    let t2 = t * t;
    // coverage of the stamp per offset
    let kw = (2 * ti + 1) as usize;
    let mut cov = vec![0u8; kw * kw];
    for dy in -ti..=ti {
        for dx in -ti..=ti {
            let d2 = (dx * dx + dy * dy) as f32;
            if d2 <= t2 {
                let c = (sqrt(t2 - d2) / if t > 1.0 { t } else { 1.0 }).clamp(0.0, 1.0);
                cov[((dy + ti) as usize) * kw + (dx + ti) as usize] = (c * 255.0) as u8;
            }
        }
    }
    let (ir, ig, ib) = ((color >> 16) & 255, (color >> 8) & 255, color & 255);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let idx = y * w + x;
            let iw = zb[idx];
            if iw <= 0.0 {
                continue;
            }
            let inv = 1.0 / iw;
            let k = inv * inv;
            let d = |j: usize| (zb[j] - iw).abs() * k;
            let dmax = d(idx - w).max(d(idx + w)).max(d(idx - 1)).max(d(idx + 1));
            if dmax < thresh {
                continue;
            }
            for dy in -ti..=ti {
                let ny = y as i32 + dy;
                if ny < 0 || ny >= h as i32 {
                    continue;
                }
                for dx in -ti..=ti {
                    let c = cov[((dy + ti) as usize) * kw + (dx + ti) as usize] as u32;
                    if c == 0 {
                        continue;
                    }
                    let nx = x as i32 + dx;
                    if nx < 0 || nx >= w as i32 {
                        continue;
                    }
                    let ni = ny as usize * w + nx as usize;
                    if c >= 255 {
                        buf[ni] = color;
                    } else {
                        let dst = buf[ni];
                        let (dr, dg, db) = ((dst >> 16) & 255, (dst >> 8) & 255, dst & 255);
                        let r = (ir * c + dr * (255 - c)) / 255;
                        let g = (ig * c + dg * (255 - c)) / 255;
                        let b = (ib * c + db * (255 - c)) / 255;
                        buf[ni] = (r << 16) | (g << 8) | b;
                    }
                }
            }
        }
    }
}

#[inline]
fn avg2(p: u32, q: u32) -> u32 {
    (((p ^ q) & 0x00FE_FEFE) >> 1) + (p & q)
}

/// The wireframe primitives of the host's `tm_wire`: unit-size vertices and edges.
fn wire_shape(kind: i32, detail: usize) -> (Vec<V3>, Vec<(usize, usize)>) {
    let mut v: Vec<V3> = Vec::new();
    let mut e: Vec<(usize, usize)> = Vec::new();
    fn by_length(v: &[V3], e: &mut Vec<(usize, usize)>) {
        let d2 = |a: V3, b: V3| dot(sub(a, b), sub(a, b));
        let mut best = f32::MAX;
        for i in 0..v.len() {
            for j in i + 1..v.len() {
                best = best.min(d2(v[i], v[j]));
            }
        }
        for i in 0..v.len() {
            for j in i + 1..v.len() {
                if d2(v[i], v[j]) < best * 1.01 {
                    e.push((i, j));
                }
            }
        }
    }
    match kind {
        1 => {
            let k = 1.0 / sqrt(3.0);
            v = vec![[k, k, k], [k, -k, -k], [-k, k, -k], [-k, -k, k]];
            by_length(&v, &mut e);
        },
        2 => {
            v = vec![[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0]];
            by_length(&v, &mut e);
        },
        3 => {
            let g = (1.0 + sqrt(5.0)) / 2.0;
            let nn = sqrt(1.0 + g * g);
            for &(a, b) in &[(1.0f32, g), (-1.0, g), (1.0, -g), (-1.0, -g)] {
                v.push([0.0, a / nn, b / nn]);
                v.push([a / nn, b / nn, 0.0]);
                v.push([b / nn, 0.0, a / nn]);
            }
            by_length(&v, &mut e);
        },
        4 => {
            let (maj, min) = (detail, (detail / 2).max(4));
            for i in 0..maj {
                let a = i as f32 / maj as f32 * TAU;
                for j in 0..min {
                    let b = j as f32 / min as f32 * TAU;
                    let r = 0.72 + 0.28 * cos(b);
                    v.push([r * cos(a), 0.28 * sin(b), r * sin(a)]);
                    let here = i * min + j;
                    e.push((here, i * min + (j + 1) % min));
                    e.push((here, ((i + 1) % maj) * min + j));
                }
            }
        },
        5 => {
            let (rings, segs) = ((detail / 2).max(3), detail);
            for i in 1..rings {
                let p = i as f32 / rings as f32 * PI;
                for j in 0..segs {
                    let a = j as f32 / segs as f32 * TAU;
                    v.push([sin(p) * cos(a), cos(p), sin(p) * sin(a)]);
                    let here = (i - 1) * segs + j;
                    e.push((here, (i - 1) * segs + (j + 1) % segs));
                    if i + 1 < rings {
                        e.push((here, i * segs + j));
                    }
                }
            }
        },
        6 => {
            for j in 0..detail {
                let a = j as f32 / detail as f32 * TAU;
                v.push([cos(a), sin(a), 0.0]);
                e.push((j, (j + 1) % detail));
            }
        },
        7 => {
            for layer in 0..2 {
                let z = if layer == 0 { -0.25 } else { 0.25 };
                for j in 0..10 {
                    let a = j as f32 / 10.0 * TAU - PI * 0.5;
                    let r = if j % 2 == 0 { 1.0 } else { 0.42 };
                    v.push([r * cos(a), r * sin(a), z]);
                    e.push((layer * 10 + j, layer * 10 + (j + 1) % 10));
                }
            }
            for j in 0..10 {
                e.push((j, 10 + j));
            }
        },
        8 => {
            let n = detail;
            for i in 0..=n {
                let t = i as f32 / n as f32 * 2.0 - 1.0;
                v.push([t, 0.0, -1.0]);
                v.push([t, 0.0, 1.0]);
                v.push([-1.0, 0.0, t]);
                v.push([1.0, 0.0, t]);
                let k = i * 4;
                e.push((k, k + 1));
                e.push((k + 2, k + 3));
            }
        },
        _ => {
            let k = 1.0 / sqrt(3.0);
            for &x in &[-1.0f32, 1.0] {
                for &y in &[-1.0f32, 1.0] {
                    for &z in &[-1.0f32, 1.0] {
                        v.push([x * k, y * k, z * k]);
                    }
                }
            }
            by_length(&v, &mut e);
        },
    }
    (v, e)
}

impl State {
    fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / 16_777_216.0
    }

    /// The window's size (None without a window), remembered for the render target.
    fn win_dims(&mut self) -> Option<(usize, usize)> {
        let (_, w, h) = crate::lingapp::surface()?;
        self.win_w = w;
        self.win_h = h;
        Some((w, h))
    }

    /// The render target's size: the window's scaled by the render scale.
    fn tdims(&self) -> (usize, usize) {
        if self.scale < 0.999 {
            (((self.win_w as f32 * self.scale) as usize).max(1), ((self.win_h as f32 * self.scale) as usize).max(1))
        } else {
            (self.win_w, self.win_h)
        }
    }

    /// The view's centre and focal length in render-target pixels.
    fn view(&self) -> (f32, f32, f32) {
        let (tw, th) = self.tdims();
        let sc = if self.scale < 0.999 { self.scale } else { 1.0 };
        let (cx, cy) = match self.cam.center {
            Some((x, y)) => (x * sc, y * sc),
            None => (tw as f32 * 0.5, th as f32 * 0.5),
        };
        let focal = if self.cam.focal > 0.0 { self.cam.focal * sc } else { th as f32 };
        (cx, cy, focal)
    }

    /// Where 3-D pixels go for code that writes anywhere (lines, wire shapes): the whole layer, backdrop
    /// included (a layer is scaled below scale 1, or whenever `flush_3d_clip` is in use so the clip can
    /// crop it), else the window itself.
    fn target(&mut self) -> Option<(*mut u32, usize, usize)> {
        self.win_dims()?;
        if self.layered() {
            self.open_frame();
            self.ensure_rows(0, self.rh);
            self.dirty = [0, 0, self.rw, self.rh];
            Some((self.rbuf.as_mut_ptr(), self.rw, self.rh))
        } else {
            let (buf, w, h) = crate::lingapp::surface()?;
            Some((buf.as_mut_ptr(), w, h))
        }
    }

    /// The open layer without preparing any of it (the caller prepares the rows it draws on).
    fn target_layer(&mut self) -> Option<(*mut u32, usize, usize)> {
        self.win_dims()?;
        self.open_frame();
        Some((self.rbuf.as_mut_ptr(), self.rw, self.rh))
    }

    /// The open layer, or the window when 3-D is not layered (the post pass works on what exists).
    fn target_layer_or_window(&mut self) -> Option<(*mut u32, usize, usize)> {
        if self.layered() {
            self.target_layer()
        } else {
            self.target()
        }
    }

    fn layered(&self) -> bool {
        self.scale < 0.999 || self.clip_seen
    }

    /// Start a layered frame. Nothing is copied yet: the window's pixels (the backdrop 3-D blends over)
    /// are decimated into the layer in 8-row bands as drawing reaches them, so a small batch (a portrait)
    /// costs what it touches, not the whole window.
    fn open_frame(&mut self) {
        if self.frame_open {
            return;
        }
        let Some((_, _, _)) = crate::lingapp::surface() else { return };
        let (rw, rh) = self.tdims();
        if self.rbuf.len() != rw * rh {
            self.rbuf = vec![0u32; rw * rh];
            self.rback = vec![0u32; rw * rh];
        }
        self.rw = rw;
        self.rh = rh;
        self.band_ok.clear();
        self.band_ok.resize(rh.div_ceil(8), false);
        self.rmask.clear();
        self.rmask.resize(rw * rh, 0);
        self.dirty = [usize::MAX, usize::MAX, 0, 0];
        self.frame_open = true;
    }

    /// Make rows `y0..y1` of the layer hold the window's pixels (and the same in `rback`), once.
    fn ensure_rows(&mut self, y0: usize, y1: usize) {
        let t0 = crate::arch::timer::now_us();
        let Some((win, ww, wh)) = crate::lingapp::surface() else { return };
        let (rw, rh) = (self.rw, self.rh);
        let y1 = y1.min(rh);
        if y0 >= y1 || rw == 0 {
            return;
        }
        let step = ((ww << 16) / rw) as u32;
        for b in y0 / 8..=(y1 - 1) / 8 {
            if self.band_ok[b] {
                continue;
            }
            self.band_ok[b] = true;
            for y in b * 8..((b + 1) * 8).min(rh) {
                let sy = (y * wh / rh).min(wh - 1);
                let src = &win[sy * ww..sy * ww + ww];
                {
                    let row = &mut self.rbuf[y * rw..y * rw + rw];
                    if rw == ww {
                        row.copy_from_slice(src);
                    } else {
                        let mut fx = 0u32;
                        for px in row.iter_mut() {
                            *px = src[((fx >> 16) as usize).min(ww - 1)];
                            fx += step;
                        }
                    }
                }
                self.rback[y * rw..y * rw + rw].copy_from_slice(&self.rbuf[y * rw..y * rw + rw]);
            }
        }
        crate::prof::add(crate::prof::LAYER, crate::arch::timer::now_us().saturating_sub(t0));
    }

    /// Which layer pixels (inside the rect drawn on) 3-D changed from the backdrop; `keep` adds to the mask.
    /// Rows this frame never prepared are skipped: nothing was drawn there, and they still hold an older
    /// batch's pixels (the portrait's body below its well), which differ from an equally old backdrop.
    fn make_mask(&mut self, keep: bool) {
        self.sync();
        let [x0, y0, x1, y1] = self.dirty;
        if x0 >= x1 || y0 >= y1 || self.rmask.len() != self.rbuf.len() {
            return;
        }
        let rw = self.rw;
        for y in y0..y1.min(self.rh) {
            if !self.band_ok.get(y / 8).copied().unwrap_or(false) {
                continue;
            }
            let r = y * rw + x0..y * rw + x1.min(rw);
            for ((m, a), b) in self.rmask[r.clone()].iter_mut().zip(&self.rbuf[r.clone()]).zip(&self.rback[r]) {
                let d = (a != b) as u8;
                *m = if keep { *m | d } else { d };
            }
        }
    }

    /// End a layered frame: the pixels 3-D drew (inside `clip`, window pixels [x, y, w, h], when given) go
    /// back onto the window -- a smooth 2x expansion at half scale, nearest otherwise. Everything else on
    /// the window (the HUD and text drawn before this batch, the sky) is left at its native pixels.
    fn close_frame(&mut self, clip: Option<[i32; 4]>) {
        self.sync();
        if !self.frame_open {
            return;
        }
        self.frame_open = false;
        let Some((win, ww, wh)) = crate::lingapp::surface() else { return };
        let (rw, rh) = (self.rw, self.rh);
        if rw == 0 || rh == 0 || self.rbuf.len() < rw * rh || self.rmask.len() < rw * rh {
            return;
        }
        let (cx0, cy0, cx1, cy1) = match clip {
            Some([x, y, w, h]) => (x.max(0) as usize, y.max(0) as usize, ((x + w).max(0) as usize).min(ww), ((y + h).max(0) as usize).min(wh)),
            None => (0, 0, ww, wh),
        };
        if cx0 >= cx1 || cy0 >= cy1 {
            return;
        }
        // only the rect 3-D drew on (a portrait batch is tiny; the whole layer is not walked)
        let [dx0, dy0, dx1, dy1] = self.dirty;
        if dx0 >= dx1 || dy0 >= dy1 {
            return;
        }
        let (dx1, dy1) = (dx1.min(rw), dy1.min(rh));
        let (src, mask, ok) = (&self.rbuf, &self.rmask, &self.band_ok);
        if ww + 1 >= 2 * rw && wh + 1 >= 2 * rh && ww <= 2 * rw && wh <= 2 * rh {
            let put = |win: &mut [u32], x: usize, y: usize, v: u32| {
                if x >= cx0 && x < cx1 && y >= cy0 && y < cy1 {
                    win[y * ww + x] = v;
                }
            };
            for sy in (cy0 / 2).max(dy0)..((cy1 + 1) / 2).min(rh).min(dy1) {
                let s = &src[sy * rw..sy * rw + rw];
                // never blend with a row this frame did not prepare (it holds stale pixels)
                let ny = if sy + 1 < rh && ok.get((sy + 1) / 8).copied().unwrap_or(false) { sy + 1 } else { sy };
                let n = &src[ny * rw..ny * rw + rw];
                let y0 = 2 * sy;
                let y1 = (y0 + 1).min(wh - 1);
                for sx in (cx0 / 2).max(dx0)..((cx1 + 1) / 2).min(rw).min(dx1) {
                    if mask[sy * rw + sx] == 0 {
                        continue;
                    }
                    let a = s[sx];
                    let b = s[(sx + 1).min(rw - 1)];
                    let c = n[sx];
                    let d = n[(sx + 1).min(rw - 1)];
                    let x0 = 2 * sx;
                    let x1 = (x0 + 1).min(ww - 1);
                    let ab = avg2(a, b);
                    put(win, x0, y0, a);
                    put(win, x1, y0, ab);
                    put(win, x0, y1, avg2(a, c));
                    put(win, x1, y1, avg2(ab, avg2(c, d)));
                }
            }
        } else {
            let step = ((rw << 16) / ww) as u32;
            for y in cy0..cy1 {
                let sy = (y * rh / wh).min(rh - 1);
                if sy < dy0 || sy >= dy1 {
                    continue;
                }
                let s = &src[sy * rw..sy * rw + rw];
                let m = &mask[sy * rw..sy * rw + rw];
                let row = &mut win[y * ww..y * ww + ww];
                let mut fx = step.wrapping_mul(cx0 as u32);
                for px in row[cx0..cx1].iter_mut() {
                    let sx = ((fx >> 16) as usize).min(rw - 1);
                    if m[sx] != 0 {
                        *px = s[sx];
                    }
                    fx = fx.wrapping_add(step);
                }
            }
        }
    }

    /// The toon post pass over the render target, once per frame: tone ramp, then ink.
    fn post(&mut self) {
        self.sync();
        if self.post_done || !self.drew_3d {
            return;
        }
        // a layer that is not open has nothing drawn in it (opening one would only re-blur the window)
        if self.layered() && !self.frame_open {
            return;
        }
        self.post_done = true;
        let t0 = crate::arch::timer::now_us();
        if self.frame_open && self.toon_px > 0.0 {
            // the ink lands a few pixels past the shapes, possibly in a band no triangle touched:
            // prepare those bands first, or make_mask would leave the ink out
            let px = if self.scale < 0.999 { self.toon_px * self.scale } else { self.toon_px };
            let reach = (libm::ceilf(px) as usize + 2).div_ceil(8);
            let nb = self.band_ok.len();
            let near: Vec<usize> = (0..nb).filter(|&b| !self.band_ok[b] && (b.saturating_sub(reach)..(b + reach + 1).min(nb)).any(|k| self.band_ok[k])).collect();
            for b in near {
                self.ensure_rows(b * 8, b * 8 + 8);
            }
        }
        let Some((ptr, w, h)) = self.target_layer_or_window() else { return };
        let buf = unsafe { core::slice::from_raw_parts_mut(ptr, w * h) };
        if self.ramp_on {
            if self.frame_open && self.rmask.len() == buf.len() {
                // layered: tone only what 3-D drew, never the backdrop (the HUD / text under it)
                let [x0, y0, x1, y1] = self.dirty;
                if x0 < x1 && y0 < y1 {
                    for y in y0..y1.min(h) {
                        for x in x0..x1.min(w) {
                            let i = y * w + x;
                            if self.rmask[i] != 0 {
                                apply_ramp(core::slice::from_mut(&mut buf[i]), &self.ramp_lut);
                            }
                        }
                    }
                }
            } else {
                apply_ramp(buf, &self.ramp_lut);
            }
        }
        if self.toon_px > 0.0 && self.zw == w && self.zh == h {
            // the ink is specified in window pixels: thinner on a scaled target
            let px = if self.scale < 0.999 { self.toon_px * self.scale } else { self.toon_px };
            draw_outlines(buf, &self.zbuf, w, h, px, self.toon_color, self.toon_thresh);
            if self.frame_open {
                // the ink reaches a little past the shapes
                let m = libm::ceilf(px) as usize + 2;
                let [x0, y0, x1, y1] = self.dirty;
                if x0 < x1 && y0 < y1 {
                    self.dirty = [x0.saturating_sub(m), y0.saturating_sub(m), (x1 + m).min(w), (y1 + m).min(h)];
                }
            }
        }
        let dt = crate::arch::timer::now_us().saturating_sub(t0);
        self.raster_us += dt;
        crate::prof::add(crate::prof::POST, dt);
    }

    /// Pick the render scale for the next frame from this frame's pixel work
    /// (auto mode): below the window's resolution when the 3-D takes more than
    /// 14 ms, back to full when it takes less than 3 ms for a second.
    fn adapt(&mut self) {
        let t = self.raster_us;
        self.raster_us = 0;
        if !self.auto_scale || !self.drew_3d {
            return;
        }
        if t > 14_000 {
            self.slow_frames += 1;
            self.fast_frames = 0;
            if self.slow_frames >= 3 && self.scale > 0.5 {
                self.scale = 0.5;
                self.slow_frames = 0;
            }
        } else if t < 3_000 && self.scale < 1.0 {
            self.fast_frames += 1;
            self.slow_frames = 0;
            if self.fast_frames >= 60 {
                self.scale = 1.0;
                self.fast_frames = 0;
            }
        } else {
            self.slow_frames = 0;
            self.fast_frames = 0;
        }
    }

    fn ensure_z(&mut self, bw: usize, bh: usize) {
        if self.zw != bw || self.zh != bh {
            // (keeps its allocation: a UI layer at display size flips the target every frame)
            self.zbuf.clear();
            self.zbuf.resize(bw * bh, 0.0);
            self.zw = bw;
            self.zh = bh;
            self.zclear = false;
        } else if self.zclear {
            self.zbuf.fill(0.0);
            self.zclear = false;
        }
    }

    /// Draw one screen-space triangle now (2-D sprites go to the window, 3-D to the target).
    fn draw_now(&mut self, v: &[A; 3], p: &Params) {
        let t0 = crate::arch::timer::now_us();
        let got = if p.sprite {
            crate::lingapp::surface().map(|(b, w, h)| (b.as_mut_ptr(), w, h))
        } else if self.layered() {
            // a layer: prepare only the rows this triangle covers
            let r = self.target_layer();
            if let Some((_, bw, bh)) = r {
                if v.iter().all(|a| a.x.is_finite() && a.y.is_finite()) {
                    let lo = |f: fn(f32, f32) -> f32, g: fn(&A) -> f32, lim: usize| f(f(g(&v[0]), g(&v[1])), g(&v[2])).max(0.0).min(lim as f32);
                    let x0 = lo(f32::min, |a| a.x, bw) as usize;
                    let x1 = (lo(f32::max, |a| a.x, bw) as usize + 1).min(bw);
                    let y0 = lo(f32::min, |a| a.y, bh) as usize;
                    let y1 = (lo(f32::max, |a| a.y, bh) as usize + 1).min(bh);
                    if x0 < x1 && y0 < y1 {
                        self.ensure_rows(y0, y1);
                        let d = &mut self.dirty;
                        *d = [d[0].min(x0), d[1].min(y0), d[2].max(x1), d[3].max(y1)];
                    }
                }
            }
            r
        } else {
            self.target()
        };
        let Some((ptr, bw, bh)) = got else { return };
        let key = (ptr as usize, bw, bh);
        if !self.batch.is_empty() && self.batch_target != key {
            self.sync();
        }
        if !p.sprite {
            self.ensure_z(bw, bh);
            self.drew_3d = true;
        }
        if let TexId::Id(i) = p.tex {
            if i as usize >= self.texs.len() {
                return;
            }
        }
        self.tris += 1;
        self.batch.push((*v, *p));
        self.batch_target = key;
        if v.iter().all(|a| a.x.is_finite() && a.y.is_finite()) {
            let (x0, x1) = (v[0].x.min(v[1].x).min(v[2].x).max(0.0), v[0].x.max(v[1].x).max(v[2].x).min(bw as f32));
            let (y0, y1) = (v[0].y.min(v[1].y).min(v[2].y).max(0.0), v[0].y.max(v[1].y).max(v[2].y).min(bh as f32));
            self.batch_area += (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
        }
        unsafe { PENDING = true };
        crate::prof::count(crate::prof::C_TRIS, 1);
        let dt = crate::arch::timer::now_us().saturating_sub(t0);
        self.raster_us += dt;
        if self.batch.len() >= 8192 {
            self.sync();
        }
    }

    /// Rasterize the batch onto its target. With more than one core and enough pixels to share,
    /// the target is cut into bands of rows and every core draws all of the batch's triangles that
    /// reach its bands, in order -- each pixel sees the same triangles in the same order as one
    /// core drawing them one by one, so the image is identical.
    fn sync(&mut self) {
        unsafe { PENDING = false };
        if self.batch.is_empty() {
            return;
        }
        let t0 = crate::arch::timer::now_us();
        let (ptr, bw, bh) = self.batch_target;
        let list = core::mem::take(&mut self.batch);
        let area = core::mem::replace(&mut self.batch_area, 0.0);
        let (zp, zlen) = (self.zbuf.as_mut_ptr(), self.zbuf.len());
        let zok = zlen >= bw * bh;
        {
            let State { texs, glow, soft, solid, lights, fog_color, fog_start, fog_end, cam, .. } = &*self;
            let buf = crate::par::Shared(ptr as *mut u32);
            let zb = crate::par::Shared(zp);
            let (fc, fs, fe, zd) = (*fog_color, *fog_start, *fog_end, cam.zdist);
            let lights: &[Light] = &lights[..];
            let band = |y_lo: usize, y_hi: usize| {
                let mut env = Env {
                    buf: Px { p: buf.get(), len: bw * bh },
                    bw,
                    bh,
                    // sprites never test or write depth; 3-D targets always have a matching plane
                    zb: Px { p: zb.get(), len: if zok { zlen } else { 0 } },
                    y_lo: y_lo as i32,
                    y_hi: y_hi as i32,
                    tex: None,
                    lights,
                    fog_color: fc,
                    fog_start: fs,
                    fog_end: fe,
                    zdist: zd,
                };
                for (v, p) in &list {
                    if !zok && !p.sprite {
                        continue;
                    }
                    env.tex = match p.tex {
                        TexId::None => None,
                        TexId::Glow => Some(glow),
                        TexId::Soft => Some(soft),
                        TexId::Solid => Some(solid),
                        TexId::Id(i) => match texs.get(i as usize) {
                            Some(t) => Some(t),
                            None => continue,
                        },
                    };
                    raster(&mut env, v, p);
                }
            };
            // splitting costs a wake-up and every core walking the whole list: only for real work
            let (chunks, per) = if area >= 24_000.0 { crate::par::row_chunks(bh, 8) } else { (1, bh.max(1)) };
            if chunks <= 1 {
                band(0, bh);
            } else {
                crate::par::for_each(chunks, &|c: usize| band(c * per, ((c + 1) * per).min(bh)));
            }
        }
        let mut list = list;
        list.clear();
        self.batch = list;
        let dt = crate::arch::timer::now_us().saturating_sub(t0);
        self.raster_us += dt;
        crate::prof::add(crate::prof::RASTER, dt);
    }

    /// The pen's cull for a draw that does not choose its own: modes 1 / 2 apply, 0 and 3 (auto) do not.
    fn pen_cull(&self) -> u8 {
        if self.cull == 1 || self.cull == 2 { self.cull } else { 0 }
    }

    /// Clip a world-space triangle against the near plane, project it and
    /// draw it (opaque) or keep it for `flush` (translucent). `col` is the
    /// per-vertex colour; `cull` 1 / 2 drops one screen winding.
    #[allow(clippy::too_many_arguments)]
    fn push_tri(&mut self, wp: [V3; 3], uv: [[f32; 2]; 3], col: [V3; 3], p: Params, cull: u8, bias: f32) -> usize {
        if self.win_dims().is_none() {
            return 0;
        }
        let near = -self.cam.zdist + 0.05;
        let d = [self.cam.depth(wp[0]), self.cam.depth(wp[1]), self.cam.depth(wp[2])];
        if d[0] <= near && d[1] <= near && d[2] <= near {
            return 0;
        }
        // the hyperbolic lens bends vertices: a long triangle inside it is split at its edge
        // midpoints so the ground bends smoothly (the host's game3d `hyper_split`)
        if let Some(kids) = self.cam.hyper_split(&wp) {
            let m2 = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
            let (tab, tbc, tca) = (m2(uv[0], uv[1]), m2(uv[1], uv[2]), m2(uv[2], uv[0]));
            let (cab, cbc, cca) = (lerp3(col[0], col[1], 0.5), lerp3(col[1], col[2], 0.5), lerp3(col[2], col[0], 0.5));
            let [ab, bc, ca] = kids;
            return self.push_tri([wp[0], ab, ca], [uv[0], tab, tca], [col[0], cab, cca], p, cull, bias)
                + self.push_tri([ab, wp[1], bc], [tab, uv[1], tbc], [cab, col[1], cbc], p, cull, bias)
                + self.push_tri([ca, bc, wp[2]], [tca, tbc, uv[2]], [cca, cbc, col[2]], p, cull, bias)
                + self.push_tri([ab, bc, ca], [tab, tbc, tca], [cab, cbc, cca], p, cull, bias);
        }
        let mut cp: [(V3, [f32; 2], V3); 4] = [([0.0; 3], [0.0; 2], [0.0; 3]); 4];
        let mut cn = 0usize;
        for i in 0..3 {
            let j = (i + 1) % 3;
            let ain = d[i] > near;
            let bin = d[j] > near;
            if ain && cn < 4 {
                cp[cn] = (wp[i], uv[i], col[i]);
                cn += 1;
            }
            if ain != bin && cn < 4 {
                let t = (near - d[i]) / (d[j] - d[i]);
                cp[cn] = (
                    lerp3(wp[i], wp[j], t),
                    [uv[i][0] + (uv[j][0] - uv[i][0]) * t, uv[i][1] + (uv[j][1] - uv[i][1]) * t],
                    lerp3(col[i], col[j], t),
                );
                cn += 1;
            }
        }
        if cn < 3 {
            return 0;
        }
        let (cx, cy, focal) = self.view();
        let zd = self.cam.zdist;
        let mut tv = [A::default(); 4];
        for k in 0..cn {
            let (pp, t, c) = cp[k];
            let (sx, sy, z) = self.cam.project(pp, cx, cy, focal);
            let iw = 1.0 / (z - bias + zd).max(1e-4);
            tv[k] = A { x: sx, y: sy, iw, uw: t[0] * iw, vw: t[1] * iw, wx: pp[0] * iw, wy: pp[1] * iw, wz: pp[2] * iw, r: c[0], g: c[1], b: c[2] };
        }
        if cull != 0 {
            let area = (tv[1].x - tv[0].x) * (tv[2].y - tv[0].y) - (tv[1].y - tv[0].y) * (tv[2].x - tv[0].x);
            if (cull == 1 && area > 0.0) || (cull == 2 && area < 0.0) {
                return 0;
            }
        }
        // with the depth test off (set_depth_test(0)) opaque triangles are painter-sorted at flush,
        // like the host's depth queue: drawn in submission order, a model's back faces (hair, the
        // back of the head) would paint over its front (the face in the HUD portrait / doll)
        let immediate = p.ztest && p.zwrite && p.alpha > 0.97 && p.blend == 0;
        let mut count = 0;
        for k in 1..cn - 1 {
            let tri = [tv[0], tv[k], tv[k + 1]];
            if immediate {
                self.draw_now(&tri, &p);
            } else {
                let key = tri[0].iw.max(tri[1].iw).max(tri[2].iw);
                self.deferred.push(Deferred { v: tri, p, key });
            }
            count += 1;
        }
        count
    }

    /// Draw the translucent triangles kept since the last flush, far to near, run
    /// the post pass and put a scaled frame back onto the window.
    fn flush(&mut self) {
        self.flush_in(None);
    }

    /// `flush`, putting only what lies inside `clip` ([x, y, w, h] in window pixels) onto the window.
    fn flush_in(&mut self, clip: Option<[i32; 4]>) {
        if !self.deferred.is_empty() {
            let mut list = core::mem::take(&mut self.deferred);
            list.sort_by(|a, b| a.key.partial_cmp(&b.key).unwrap_or(core::cmp::Ordering::Equal));
            for d in &list {
                self.draw_now(&d.v, &d.p);
            }
            list.clear();
            self.deferred = list;
            self.sync();
        }
        let tm = crate::arch::timer::now_us();
        if self.frame_open {
            self.make_mask(false);
        }
        crate::prof::add(crate::prof::LAYER, crate::arch::timer::now_us().saturating_sub(tm));
        self.post();
        let t0 = crate::arch::timer::now_us();
        if self.frame_open {
            // the ink outline also lands on pixels just outside the shapes: take them in
            self.make_mask(true);
        }
        self.close_frame(clip);
        let dt = crate::arch::timer::now_us().saturating_sub(t0);
        self.raster_us += dt;
        crate::prof::add(crate::prof::LAYER, dt);
    }

    /// `tm_wire(kind, x, y, z, size, rx, ry, rz, colour, detail, thick)`: a wireframe
    /// primitive (1 tetrahedron, 2 octahedron, 3 icosahedron, 4 torus, 5 sphere,
    /// 6 ring, 7 star prism, 8 grid, else cube), fogged lines, no depth.
    fn tm_wire(&mut self, args: &[u64]) {
        let kind = n(args, 0, 0.0) as i32;
        let c = [f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)];
        let size = f(args, 4, 1.0);
        let (rx, ry, rz) = (f(args, 5, 0.0), f(args, 6, 0.0), f(args, 7, 0.0));
        let col = rgb(n(args, 8, 16777215.0));
        let detail = (n(args, 9, 12.0) as usize).clamp(3, 64);
        let thick = n(args, 10, 0.0) > 0.5;
        self.sync();
        let Some((ptr, bw, bh)) = self.target() else { return };
        let buf = unsafe { core::slice::from_raw_parts_mut(ptr, bw * bh) };
        let (verts, edges) = wire_shape(kind, detail);
        let (sx, cx) = (sin(rx), cos(rx));
        let (sy, cy) = (sin(ry), cos(ry));
        let (sz, cz) = (sin(rz), cos(rz));
        let world: Vec<V3> = verts
            .iter()
            .map(|v| {
                let (x, y, z) = (v[0], v[1] * cx - v[2] * sx, v[1] * sx + v[2] * cx);
                let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
                let (x, y) = (x * cz - y * sz, x * sz + y * cz);
                [c[0] + x * size, c[1] + y * size, c[2] + z * size]
            })
            .collect();
        let (vcx, vcy, focal) = self.view();
        let near = -self.cam.zdist + 0.05;
        for (i, j) in edges {
            let (mut a, mut b) = (world[i], world[j]);
            let da = self.cam.depth(a);
            let db = self.cam.depth(b);
            if da <= near && db <= near {
                continue;
            }
            if da <= near {
                a = lerp3(a, b, (near - da) / (db - da));
            } else if db <= near {
                b = lerp3(a, b, (near - da) / (db - da));
            }
            let (ax, ay, az) = self.cam.project(a, vcx, vcy, focal);
            let (bx, by, bz) = self.cam.project(b, vcx, vcy, focal);
            if ax.abs() > 20000.0 || ay.abs() > 20000.0 || bx.abs() > 20000.0 || by.abs() > 20000.0 {
                continue;
            }
            let lc = self.fog_apply(col, (az + bz) * 0.5);
            line2d(buf, bw, bh, ax, ay, bx, by, lc);
            if thick {
                line2d(buf, bw, bh, ax + 1.0, ay, bx + 1.0, by, lc);
                line2d(buf, bw, bh, ax, ay + 1.0, bx, by + 1.0, lc);
            }
        }
        self.drew_3d = true;
    }

    fn fog_apply(&self, color: u32, depth: f32) -> u32 {
        if self.fog_end <= 0.0 || self.fog_end <= self.fog_start {
            return color;
        }
        let f = ((depth - self.fog_start) / (self.fog_end - self.fog_start)).clamp(0.0, 1.0);
        lerp_rgb(color, self.fog_color, f)
    }

    fn base(&mut self, kind: u8, e0: i32, e1: i32) -> &Base {
        let key = (kind, e0, e1);
        if !self.bases.contains_key(&key) {
            let b = match kind {
                0 => base_cube(),
                1 => base_sphere(if e0 > 0 { e0 } else { 20 }, if e1 > 0 { e1 } else { 10 }),
                2 => base_tube(if e0 > 0 { e0 } else { 20 }, 1.0),
                3 => base_tube(if e0 > 0 { e0 } else { 20 }, 0.0),
                4 => base_capsule(if e0 > 0 { e0 } else { 14 }, if e1 > 0 { e1 } else { 4 }),
                6 => base_tube(if e0 > 0 { e0 } else { 24 }, e1 as f32 / 1000.0),
                _ => base_torus(if e0 > 0 { e0 } else { 20 }, if e1 > 0 { e1 } else { 10 }, 0.3),
            };
            self.bases.insert(key, b);
        }
        self.bases.get(&key).unwrap()
    }

    /// The shape builtins: scale → rotate (X, Y, Z) → translate, cel-lit per
    /// vertex in the pen colour; mode 0 filled, 1 wireframe, 2 both.
    #[allow(clippy::too_many_arguments)]
    fn shape(&mut self, kind: u8, c: [f32; 9], mode: i32, e0: i32, e1: i32) {
        let color = col3(crate::lingapp::gfx_color());
        let (cx, cy, cz) = (c[0], c[1], c[2]);
        let (sx, sy, sz) = (c[3], c[4], c[5]);
        let (srx, crx) = (sin(c[6]), cos(c[6]));
        let (sry, cry) = (sin(c[7]), cos(c[7]));
        let (srz, crz) = (sin(c[8]), cos(c[8]));
        let rot = |x: f32, y: f32, z: f32| -> V3 {
            let (y1, z1) = (y * crx - z * srx, y * srx + z * crx);
            let (x2, z2) = (x * cry + z1 * sry, -x * sry + z1 * cry);
            let (x3, y3) = (x2 * crz - y1 * srz, x2 * srz + y1 * crz);
            [x3, y3, z2]
        };
        let (nverts, ntris, nedges) = {
            let b = self.base(kind, e0, e1);
            (b.verts.len(), b.tris.len(), b.edges.len())
        };
        let _ = (ntris, nedges);
        // transformed vertices and normals
        let mut wv: Vec<V3> = Vec::with_capacity(nverts);
        let mut wn: Vec<V3> = Vec::with_capacity(nverts);
        {
            let b = self.bases.get(&(kind, e0, e1)).unwrap();
            let inv = [if sx.abs() > 1e-6 { 1.0 / sx } else { 0.0 }, if sy.abs() > 1e-6 { 1.0 / sy } else { 0.0 }, if sz.abs() > 1e-6 { 1.0 / sz } else { 0.0 }];
            for (v, nr) in b.verts.iter().zip(b.nrm.iter()) {
                let p = rot(v[0] * sx, v[1] * sy, v[2] * sz);
                wv.push([p[0] + cx, p[1] + cy, p[2] + cz]);
                wn.push(norm(rot(nr[0] * inv[0], nr[1] * inv[1], nr[2] * inv[2])));
            }
        }
        let want_fill = mode == 0 || mode == 2;
        let want_wire = mode == 1 || mode == 2;
        if want_fill {
            // per-vertex cel lighting (ling-graphics shading::lit_vertex)
            let amb = self.ambient;
            let shadow = [0.10f32, 0.13, 0.30];
            let eye = self.cam.t;
            let rim = if self.shade_mode == 1 { self.rim * 0.4 } else { self.rim };
            let holo = self.shade_mode == 2;
            let mut lit: Vec<V3> = Vec::with_capacity(nverts);
            for (p, nr) in wv.iter().zip(wn.iter()) {
                let mut acc = [
                    color[0] * amb + shadow[0] * (1.0 - amb),
                    color[1] * amb + shadow[1] * (1.0 - amb),
                    color[2] * amb + shadow[2] * (1.0 - amb),
                ];
                if self.shade_mode != 0 {
                    for l in &self.lights {
                        let d = sub(l.p, *p);
                        let dist = sqrt(dot(d, d)).max(1e-6);
                        let atten = if l.r > 0.0 { (1.0 - dist / l.r).max(0.0) } else { 1.0 };
                        if atten <= 0.0 {
                            continue;
                        }
                        let ldir = [d[0] / dist, d[1] / dist, d[2] / dist];
                        let diff = dot(*nr, ldir).max(0.0);
                        let sh = cel_ramp(diff) * l.i * atten;
                        acc[0] += color[0] * sh * l.c[0];
                        acc[1] += color[1] * sh * l.c[1];
                        acc[2] += color[2] * sh * l.c[2];
                    }
                    if rim > 0.0 {
                        let vd = norm(sub(eye, *p));
                        let fr = (1.0 - dot(*nr, vd).max(0.0)).clamp(0.0, 1.0);
                        let k = fr * fr * fr * rim;
                        acc[0] += self.rim_color[0] * k;
                        acc[1] += self.rim_color[1] * k;
                        acc[2] += self.rim_color[2] * k;
                    }
                    if holo {
                        let s = 0.07;
                        acc[0] += (0.5 + 0.5 * nr[0]) * s * (0.4 + 0.6 * color[0]);
                        acc[1] += (0.5 + 0.5 * nr[1]) * s * (0.4 + 0.6 * color[1]);
                        acc[2] += (0.5 + 0.5 * nr[2]) * s * (0.4 + 0.6 * color[2]);
                    }
                } else {
                    // flat: a fixed key light from the camera's upper left
                    let d = norm([0.35, -0.8, -0.45]);
                    let l = dot(*nr, d).abs();
                    let k = 0.35 + 0.65 * l;
                    acc = [color[0] * k, color[1] * k, color[2] * k];
                }
                let c = [acc[0].min(1.0), acc[1].min(1.0), acc[2].min(1.0)];
                lit.push(if self.shade_mode != 0 { posterize(c, self.bands) } else { c });
            }
            let p = Params {
                tex: TexId::None,
                alpha: 1.0,
                blend: self.blend,
                zwrite: self.blend == 0,
                ztest: self.depth_test,
                lit: false,
                amb: 0.0,
                fog: self.fog_end > 0.0,
                hole: [0.0; 4],
                sprite: false,
            };
            let tris: Vec<[u32; 3]> = self.bases.get(&(kind, e0, e1)).unwrap().tris.clone();
            for t in &tris {
                let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
                self.push_tri([wv[a], wv[b], wv[c]], [[0.0; 2]; 3], [lit[a], lit[b], lit[c]], p, 0, 0.0);
            }
        }
        if want_wire {
            self.sync();
            let Some((ptr, bw, bh)) = self.target() else { return };
            let buf = unsafe { core::slice::from_raw_parts_mut(ptr, bw * bh) };
            let (vcx, vcy, focal) = self.view();
            let near = -self.cam.zdist + 0.05;
            let col = pack3(color);
            let edges: Vec<[u32; 2]> = self.bases.get(&(kind, e0, e1)).unwrap().edges.clone();
            for e in &edges {
                let (a, b) = (wv[e[0] as usize], wv[e[1] as usize]);
                let (ax, ay, az) = self.cam.project(a, vcx, vcy, focal);
                let (bx, by, bz) = self.cam.project(b, vcx, vcy, focal);
                if az > near && bz > near {
                    line2d(buf, bw, bh, ax, ay, bx, by, col);
                }
            }
        }
    }

    /// `tm_draw(mesh, group, tex, x, y, z, scale, rot, tint, alpha, uoff, voff, uscale,
    ///          vscale, shade, blend, cull[, ambient])` → triangles queued
    fn tm_draw(&mut self, args: &[u64]) -> usize {
        let mi = n(args, 0, -1.0);
        let gi = n(args, 1, -1.0);
        let ti = n(args, 2, -1.0);
        if mi < 0.0 || mi as usize >= self.meshes.len() {
            return 0;
        }
        let tex = if ti < 0.0 {
            TexId::Glow
        } else if (ti as usize) < self.texs.len() {
            TexId::Id(ti as u32)
        } else {
            return 0;
        };
        let o = [f(args, 3, 0.0), f(args, 4, 0.0), f(args, 5, 0.0)];
        let sc = f(args, 6, 1.0);
        let m = rot_from(args, 7);
        let tint = rgb(n(args, 8, 16777215.0));
        let alpha = f(args, 9, 1.0).clamp(0.0, 1.0);
        let (uo, vo) = (f(args, 10, 0.0), f(args, 11, 0.0));
        let (us, vs) = (f(args, 12, 1.0), f(args, 13, 1.0));
        let shade = f(args, 14, 0.0).clamp(0.0, 1.0);
        let blend = n(args, 15, 0.0) as u8;
        let cull = match n(args, 16, 0.0) as u8 {
            0 => self.pen_cull(),
            c => c,
        };
        let lit_amb = f(args, 17, -1.0);
        if self.win_dims().is_none() {
            return 0;
        }
        let hole = if self.hole[3] > 0.0 {
            let (cx, cy, focal) = self.view();
            let (hx, hy, hd) = self.cam.project([self.hole[0], self.hole[1], self.hole[2]], cx, cy, focal);
            let r = focal * self.hole[3] / (hd + self.cam.zdist).max(0.1);
            [hx, hy, r, hd - self.hole[3] * 0.2]
        } else {
            [0.0; 4]
        };
        let p = Params {
            tex,
            alpha,
            blend,
            zwrite: alpha > 0.97 && blend == 0,
            ztest: self.depth_test,
            lit: lit_amb >= 0.0,
            amb: lit_amb.max(0.0),
            fog: self.fog_end > 0.0,
            hole,
            sprite: false,
        };
        let light = norm([0.35, -0.8, -0.45]);
        let tint3 = col3(tint);
        let mesh = core::mem::replace(&mut self.meshes[mi as usize], Mesh { groups: Vec::new() });
        let mut count = 0;
        for (gidx, g) in mesh.groups.iter().enumerate() {
            if gi >= 0.0 && gidx != gi as usize {
                continue;
            }
            for t in &g.idx {
                let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
                let wp = [xf(&m, g.pos[a], sc, o), xf(&m, g.pos[b], sc, o), xf(&m, g.pos[c], sc, o)];
                let uv = [
                    [g.uv[a][0] * us + uo, g.uv[a][1] * vs + vo],
                    [g.uv[b][0] * us + uo, g.uv[b][1] * vs + vo],
                    [g.uv[c][0] * us + uo, g.uv[c][1] * vs + vo],
                ];
                let tt = if shade > 0.0 {
                    let fnrm = norm(cross(sub(wp[1], wp[0]), sub(wp[2], wp[0])));
                    let vn = rotn(&m, g.nrm[a]);
                    let nn = if dot(vn, vn) > 0.25 { norm(vn) } else { fnrm };
                    let l = dot(nn, light).abs();
                    let k = 1.0 - shade * (1.0 - (0.35 + 0.65 * l));
                    [tint3[0] * k, tint3[1] * k, tint3[2] * k]
                } else {
                    tint3
                };
                count += self.push_tri(wp, uv, [tt, tt, tt], p, cull, 0.0);
            }
        }
        self.meshes[mi as usize] = mesh;
        count
    }

    /// A camera-facing textured square at `p`, `size` world units wide.
    #[allow(clippy::too_many_arguments)]
    fn billboard(&mut self, tex: TexId, p: V3, size: f32, rot: f32, tint: u32, alpha: f32, blend: u8) {
        if self.win_dims().is_none() {
            return;
        }
        let (cx, cy, focal) = self.view();
        let near = -self.cam.zdist + 0.05;
        let (sx, sy, z) = self.cam.project(p, cx, cy, focal);
        if z <= near {
            return;
        }
        let w = z + self.cam.zdist;
        let r = focal * size * 0.5 / w.max(1e-3);
        if r < 0.3 || r > 4000.0 {
            return;
        }
        let tint = if blend == 1 {
            let fog_k = if self.fog_end > self.fog_start && self.fog_end > 0.0 { ((z - self.fog_start) / (self.fog_end - self.fog_start)).clamp(0.0, 1.0) } else { 0.0 };
            lerp_rgb(tint, 0, fog_k)
        } else {
            self.fog_apply(tint, z)
        };
        let c3 = col3(tint);
        let iw = 1.0 / w.max(1e-4);
        let (c, s) = (cos(rot), sin(rot));
        let corner = |dx: f32, dy: f32, u: f32, v: f32| A {
            x: sx + (dx * c - dy * s) * r,
            y: sy + (dx * s + dy * c) * r,
            iw,
            uw: u * iw,
            vw: v * iw,
            wx: 0.0,
            wy: 0.0,
            wz: 0.0,
            r: c3[0],
            g: c3[1],
            b: c3[2],
        };
        let q = [corner(-1.0, -1.0, 0.0, 0.0), corner(1.0, -1.0, 0.999, 0.0), corner(1.0, 1.0, 0.999, 0.999), corner(-1.0, 1.0, 0.0, 0.999)];
        let p = Params { tex, alpha, blend, zwrite: false, ztest: self.depth_test, lit: false, amb: 0.0, fog: false, hole: [0.0; 4], sprite: false };
        self.deferred.push(Deferred { v: [q[0], q[1], q[2]], p, key: iw });
        self.deferred.push(Deferred { v: [q[0], q[2], q[3]], p, key: iw });
    }

    fn pfx_draw(&mut self) {
        let parts = core::mem::take(&mut self.parts);
        for p in &parts {
            let t = 1.0 - (p.life / p.max).clamp(0.0, 1.0);
            let size = p.s0 + (p.s1 - p.s0) * t;
            let col = lerp_rgb(p.c0, p.c1, t);
            let fade = ((p.life / p.max) * 3.0).clamp(0.0, 1.0);
            match p.kind {
                1 => {
                    // streak: a thin additive quad from the particle back along its velocity
                    if self.win_dims().is_some() {
                        let (cx, cy, focal) = self.view();
                        let near = -self.cam.zdist + 0.05;
                        let tail = [p.p[0] - p.v[0] * 0.06, p.p[1] - p.v[1] * 0.06, p.p[2] - p.v[2] * 0.06];
                        let (ax, ay, az) = self.cam.project(p.p, cx, cy, focal);
                        let (bx, by, bz) = self.cam.project(tail, cx, cy, focal);
                        if az > near && bz > near {
                            let w = (focal * size * 0.5 / (az + self.cam.zdist).max(1e-3)).max(0.6);
                            let (dx, dy) = (bx - ax, by - ay);
                            let l = sqrt(dx * dx + dy * dy).max(1e-3);
                            let (nx, ny) = (-dy / l * w, dx / l * w);
                            let iwa = 1.0 / (az + self.cam.zdist).max(1e-4);
                            let iwb = 1.0 / (bz + self.cam.zdist).max(1e-4);
                            let c3 = col3(col);
                            let v = |x: f32, y: f32, iw: f32| A { x, y, iw, uw: 0.5 * iw, vw: 0.5 * iw, wx: 0.0, wy: 0.0, wz: 0.0, r: c3[0], g: c3[1], b: c3[2] };
                            let q = [v(ax + nx, ay + ny, iwa), v(ax - nx, ay - ny, iwa), v(bx - nx * 0.3, by - ny * 0.3, iwb), v(bx + nx * 0.3, by + ny * 0.3, iwb)];
                            let pp = Params { tex: TexId::Solid, alpha: fade * p.alpha, blend: 1, zwrite: false, ztest: self.depth_test, lit: false, amb: 0.0, fog: false, hole: [0.0; 4], sprite: false };
                            let key = iwa.max(iwb);
                            self.deferred.push(Deferred { v: [q[0], q[1], q[2]], p: pp, key });
                            self.deferred.push(Deferred { v: [q[0], q[2], q[3]], p: pp, key });
                        }
                    }
                    self.billboard(TexId::Glow, p.p, size * 1.6, 0.0, col, fade * p.alpha, 1);
                },
                2 => {
                    if (p.tex as usize) < self.texs.len() {
                        self.billboard(TexId::Id(p.tex), p.p, size, p.spin * (p.max - p.life), col, p.alpha * fade.min(1.0), 0);
                    }
                },
                3..=7 => self.pixel_spark(p, size, col, fade * p.alpha),
                _ => self.billboard(TexId::Glow, p.p, size, 0.0, col, fade * p.alpha, 1),
            }
        }
        self.parts = parts;
    }

    /// The pixel kinds (3 spark · 4 twinkle · 5 crackle · 6 trail · 7 flicker):
    /// a crisp additive square `size` pixels across at 720p, snapped to the grid.
    /// Same look as the desktop port (ling/src/gfx/particles.rs).
    fn pixel_spark(&mut self, p: &Part, size: f32, col: u32, alpha: f32) {
        if self.win_dims().is_none() {
            return;
        }
        let hs = |x: u32| -> u32 {
            let mut x = x ^ (x >> 16);
            x = x.wrapping_mul(0x7feb_352d);
            x ^= x >> 15;
            x = x.wrapping_mul(0x846c_a68b);
            x ^ (x >> 16)
        };
        let seed = p.max.to_bits() ^ p.v[0].to_bits().rotate_left(7);
        let t = 1.0 - (p.life / p.max).clamp(0.0, 1.0);
        let age = p.max - p.life;
        let mut col = col;
        let mut a = alpha;
        match p.kind {
            4 if t > 0.5 && hs(seed ^ (age * 24.0) as u32) & 3 == 0 => a *= 0.15,
            5 if t > 0.35 => {
                if hs(seed ^ (age * 30.0) as u32) % 100 < 30 {
                    col = 0xfff0c8;
                } else {
                    a *= 0.05;
                }
            },
            7 => a *= 0.6 + 0.4 * ((hs(seed ^ (age * 40.0) as u32) >> 8) as f32 / 16_777_216.0),
            _ => {},
        }
        let (cx, cy, focal) = self.view();
        let near = -self.cam.zdist + 0.05;
        let (sx, sy, z) = self.cam.project(p.p, cx, cy, focal);
        if z <= near + 0.05 {
            return;
        }
        let fog_k = if p.kind == 7 && self.fog_end > self.fog_start && self.fog_end > 0.0 { ((z - self.fog_start) / (self.fog_end - self.fog_start)).clamp(0.0, 1.0) } else { 0.0 };
        a *= 1.0 - fog_k;
        if a <= 0.02 {
            return;
        }
        let th = self.tdims().1 as f32;
        let px = (th / 720.0).max(1.0);
        let w = z + self.cam.zdist;
        let boost = (14.0 / w.max(1e-3)).clamp(1.0, 2.0);
        let half = libm::roundf(size * px * boost).max(1.0) * 0.5;
        let (qx, qy) = (libm::roundf(sx), libm::roundf(sy));
        let iw = 1.0 / w.max(1e-4);
        let c3 = col3(col);
        let tail = [p.p[0] - p.v[0] * 0.05, p.p[1] - p.v[1] * 0.05, p.p[2] - p.v[2] * 0.05];
        let (tx, ty, tz) = self.cam.project(tail, cx, cy, focal);
        let mut sq = |x: f32, y: f32, hh: f32, al: f32| {
            let v = |x: f32, y: f32| A { x, y, iw, uw: 0.5 * iw, vw: 0.5 * iw, wx: 0.0, wy: 0.0, wz: 0.0, r: c3[0], g: c3[1], b: c3[2] };
            let q = [v(x - hh, y - hh), v(x + hh, y - hh), v(x + hh, y + hh), v(x - hh, y + hh)];
            let pp = Params { tex: TexId::Solid, alpha: al, blend: 1, zwrite: false, ztest: self.depth_test, lit: false, amb: 0.0, fog: false, hole: [0.0; 4], sprite: false };
            self.deferred.push(Deferred { v: [q[0], q[1], q[2]], p: pp, key: iw });
            self.deferred.push(Deferred { v: [q[0], q[2], q[3]], p: pp, key: iw });
        };
        sq(qx, qy, half, a);
        if size >= 2.0 {
            sq(qx, qy, half + px, a * 0.22);
        }
        if p.kind == 6 {
            if tz > near + 0.05 {
                for (f, m) in [(0.5f32, 0.55f32), (1.0, 0.3)] {
                    sq(libm::roundf(sx + (tx - sx) * f), libm::roundf(sy + (ty - sy) * f), (half * 0.75).max(0.5), a * m);
                }
            }
        }
    }

    fn pfx_push(&mut self, p: Part) {
        if self.parts.len() >= self.max_parts {
            self.parts.remove(0);
        }
        self.parts.push(p);
    }

    fn coll_build(&mut self, args: &[u64]) -> f64 {
        let mi = n(args, 0, -1.0);
        if mi < 0.0 || mi as usize >= self.meshes.len() {
            return -1.0;
        }
        let gsel = n(args, 1, -1.0);
        let o = [f(args, 2, 0.0), f(args, 3, 0.0), f(args, 4, 0.0)];
        let s = f(args, 5, 1.0);
        let m = rot_from(args, 6);
        let cell = f(args, 7, 8.0);
        let mut tris = Vec::new();
        let mut nrm = Vec::new();
        for (gi, g) in self.meshes[mi as usize].groups.iter().enumerate() {
            if gsel >= 0.0 && gi != gsel as usize {
                continue;
            }
            for t in &g.idx {
                let a = xf(&m, g.pos[t[0] as usize], s, o);
                let b = xf(&m, g.pos[t[1] as usize], s, o);
                let c = xf(&m, g.pos[t[2] as usize], s, o);
                let fnrm = norm(cross(sub(b, a), sub(c, a)));
                let vn = rotn(&m, g.nrm[t[0] as usize]);
                let nn = if dot(fnrm, vn) < 0.0 { [-fnrm[0], -fnrm[1], -fnrm[2]] } else { fnrm };
                tris.push([a, b, c]);
                nrm.push(nn);
            }
        }
        self.colls.push(Coll::build(tris, nrm, cell));
        (self.colls.len() - 1) as f64
    }

    fn coll_sphere(&mut self, args: &[u64]) -> u64 {
        let id = n(args, 0, -1.0);
        let mut p = [f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)];
        let r = f(args, 4, 1.0).max(1e-3);
        if id < 0.0 || id as usize >= self.colls.len() {
            return nums(&[p[0], p[1], p[2], 0.0, 0.0, 0.0, 0.0, 0.0]);
        }
        let mut cand = core::mem::take(&mut self.scratch);
        let coll = &mut self.colls[id as usize];
        let mut nsum = [0.0f32; 3];
        let mut contacts = 0.0f32;
        let mut deepest = 0.0f32;
        for _ in 0..4 {
            coll.query([p[0] - r, p[1] - r, p[2] - r], [p[0] + r, p[1] + r, p[2] + r], &mut cand);
            let mut moved = false;
            for &ti in &cand {
                let t = coll.tris[ti as usize];
                let q = closest_on_tri(p, t[0], t[1], t[2]);
                let d = sub(p, q);
                let dist2 = dot(d, d);
                if dist2 < r * r {
                    let dist = sqrt(dist2);
                    let nn = if dist > 1e-5 { [d[0] / dist, d[1] / dist, d[2] / dist] } else { coll.nrm[ti as usize] };
                    let pen = r - dist;
                    p = [p[0] + nn[0] * pen, p[1] + nn[1] * pen, p[2] + nn[2] * pen];
                    nsum = [nsum[0] + nn[0] * pen, nsum[1] + nn[1] * pen, nsum[2] + nn[2] * pen];
                    contacts += 1.0;
                    deepest = deepest.max(pen);
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
        self.scratch = cand;
        let nn = if contacts > 0.0 { norm(nsum) } else { [0.0; 3] };
        nums(&[p[0], p[1], p[2], nn[0], nn[1], nn[2], contacts, deepest])
    }

    fn coll_ray(&mut self, args: &[u64]) -> u64 {
        let id = n(args, 0, -1.0);
        let o = [f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)];
        let dir = norm([f(args, 4, 0.0), f(args, 5, 0.0), f(args, 6, 1.0)]);
        let maxt = f(args, 7, 1000.0);
        if id < 0.0 || id as usize >= self.colls.len() {
            return nums(&[-1.0, 0.0, 0.0, 0.0]);
        }
        let mut cand = core::mem::take(&mut self.scratch);
        let coll = &mut self.colls[id as usize];
        let e = [o[0] + dir[0] * maxt, o[1] + dir[1] * maxt, o[2] + dir[2] * maxt];
        let lo = [o[0].min(e[0]), o[1].min(e[1]), o[2].min(e[2])];
        let hi = [o[0].max(e[0]), o[1].max(e[1]), o[2].max(e[2])];
        coll.query(lo, hi, &mut cand);
        let mut best = (maxt, [0.0f32; 3], false);
        for &ti in &cand {
            let t = coll.tris[ti as usize];
            let e1 = sub(t[1], t[0]);
            let e2 = sub(t[2], t[0]);
            let pv = cross(dir, e2);
            let det = dot(e1, pv);
            if det.abs() < 1e-9 {
                continue;
            }
            let inv = 1.0 / det;
            let tv = sub(o, t[0]);
            let u = dot(tv, pv) * inv;
            if !(0.0..=1.0).contains(&u) {
                continue;
            }
            let qv = cross(tv, e1);
            let v = dot(dir, qv) * inv;
            if v < 0.0 || u + v > 1.0 {
                continue;
            }
            let tt = dot(e2, qv) * inv;
            if tt > 1e-4 && tt < best.0 {
                best = (tt, coll.nrm[ti as usize], true);
            }
        }
        self.scratch = cand;
        if best.2 {
            nums(&[best.0, best.1[0], best.1[1], best.1[2]])
        } else {
            nums(&[-1.0, 0.0, 0.0, 0.0])
        }
    }

    /// `tm_glyph(font, "c", px)` → [tex, span]: the glyph's coverage as the alpha
    /// of a white `px` x `px` texture centred on its ink; `span` is the square's
    /// side in ems.
    fn glyph(&mut self, args: &[u64]) -> u64 {
        let fid = n(args, 0, -1.0);
        let ch = args.get(1).map(|&v| to_string(v)).unwrap_or_default().chars().next().unwrap_or(' ');
        let px = pow2((n(args, 2, 256.0) as u32).clamp(16, 1024));
        let Some(font) = crate::lingapp::font_fd_for(fid, ch) else { return nums(&[-1.0, 0.0]) };
        // measure at a probe size, then rasterize so the ink spans px / 1.12
        let probe = px as f32 * 0.5;
        let (m0, _) = font.rasterize(ch, probe);
        let ink = m0.width.max(m0.height) as f32;
        if ink < 1.0 {
            return nums(&[-1.0, 0.0]);
        }
        let size = probe * (px as f32 / 1.12) / ink;
        let (m, cov) = font.rasterize(ch, size);
        if m.width == 0 || m.height == 0 || m.width > px as usize || m.height > px as usize {
            return nums(&[-1.0, 0.0]);
        }
        let mut data = vec![0x00FF_FFFFu32; (px * px) as usize];
        let ox = (px as usize - m.width) / 2;
        let oy = (px as usize - m.height) / 2;
        for y in 0..m.height {
            for x in 0..m.width {
                let a = cov[y * m.width + x] as u32;
                data[(oy + y) * px as usize + ox + x] = (a << 24) | 0x00FF_FFFF;
            }
        }
        self.texs.push(Tex::new(px, data));
        nums(&[(self.texs.len() - 1) as f32, px as f32 / size])
    }
}

/// `bench3d [frames] [triangles]`: rasterize a fixed scene of lit, depth-tested
/// triangles into an offscreen 1920x1080 target with the same code path FLY for
/// FUN and LingDev games use (`draw_now` -> batch -> banded `sync`), at 1 core
/// and then at every power of two up to all of them, and report the frame times.
/// The image must come out the same at every core count; a checksum says so.
pub fn bench(arg: &str) -> String {
    use alloc::format;
    let mut it = arg.split_whitespace();
    let frames: usize = it.next().and_then(|s| s.parse().ok()).unwrap_or(30).clamp(1, 600);
    let ntri: usize = it.next().and_then(|s| s.parse().ok()).unwrap_or(20_000).clamp(1, 200_000);
    let (w, h) = (1920usize, 1080usize);
    let s = st();
    s.sync();
    // the scene: a deterministic field of triangles, near and far, overlapping
    let mut seed = 0x1234_5678u32;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed >> 8) as f32 / 16_777_216.0
    };
    let mut tris: Vec<[A; 3]> = Vec::with_capacity(ntri);
    for _ in 0..ntri {
        let (cx, cy) = (rnd() * w as f32, rnd() * h as f32);
        let size = 8.0 + rnd() * 70.0;
        let iw = 0.05 + rnd() * 0.9;
        let col = [0.2 + rnd() * 0.8, 0.2 + rnd() * 0.8, 0.2 + rnd() * 0.8];
        let mut t = [A::default(); 3];
        for k in 0..3 {
            let a = (k as f32) * 2.094 + rnd() * 0.6;
            t[k] = A { x: cx + libm::cosf(a) * size, y: cy + libm::sinf(a) * size, iw, uw: 0.0, vw: 0.0, wx: 0.0, wy: 0.0, wz: 0.0, r: col[0], g: col[1], b: col[2] };
        }
        tris.push(t);
    }
    let p = Params { tex: TexId::None, alpha: 1.0, blend: 0, zwrite: true, ztest: true, lit: false, amb: 0.0, fog: false, hole: [0.0; 4], sprite: false };
    let mut buf = alloc::vec![0u32; w * h];
    let online = crate::par::cores();
    let mut counts = alloc::vec![1usize];
    let mut c = 2;
    while c < online {
        counts.push(c);
        c *= 2;
    }
    if online > 1 {
        counts.push(online);
    }
    let mut out = format!("bench3d: {frames} frames of {ntri} lit, depth-tested triangles at {w}x{h} (offscreen)\n");
    let mut base_ms = 0.0f64;
    let mut base_sum = 0u64;
    #[cfg(target_arch = "x86_64")]
    let before = crate::arch::smp::cores();
    for &n in &counts {
        #[cfg(target_arch = "x86_64")]
        crate::arch::smp::set_cores(n);
        let t0 = crate::arch::timer::now_us();
        for _ in 0..frames {
            buf.fill(0x0010_1018);
            s.ensure_z(w, h);
            s.zbuf.fill(0.0);
            for t in &tris {
                s.batch.push((*t, p));
            }
            s.batch_target = (buf.as_mut_ptr() as usize, w, h);
            s.batch_area = (w * h) as f32;
            unsafe { PENDING = true };
            s.sync();
        }
        let ms = crate::arch::timer::now_us().saturating_sub(t0) as f64 / 1000.0 / frames as f64;
        let sum = buf.iter().fold(0u64, |a, &v| a.wrapping_mul(31).wrapping_add(v as u64));
        if n == 1 {
            base_ms = ms;
            base_sum = sum;
        }
        out.push_str(&format!(
            "  {n:>2} core{}  {ms:>7.2} ms/frame  {:>6.1} fps  x{:.2}  {:.1} Mtri/s  image {}\n",
            if n == 1 { " " } else { "s" },
            1000.0 / ms.max(0.001),
            base_ms / ms.max(0.001),
            ntri as f64 / ms.max(0.001) / 1000.0,
            if sum == base_sum { "identical" } else { "DIFFERENT" }
        ));
    }
    #[cfg(target_arch = "x86_64")]
    crate::arch::smp::set_cores(before);
    // the next frame of a real program must not see this target or depth
    s.batch.clear();
    s.batch_target = (0, 0, 0);
    s.zclear = true;
    out
}

/// The app window's drawing surface is about to change (a UI layer opens or closes,
/// `lingapp::ui_begin` / `ui_end`): the 3-D drawn so far lands on the surface it was
/// drawn for first. `post`: also run the frame's tone pass if it has not run yet
/// (opening the HUD layer); closing one never tones the UI drawn in it.
pub fn surface_switch(post: bool) {
    let g = unsafe { &mut *(&raw mut G) };
    if let Some(s) = g.as_mut() {
        s.sync();
        let tone = post && s.drew_3d && !s.post_done;
        if !s.deferred.is_empty() || s.frame_open || tone {
            let done = s.post_done;
            if !post {
                s.post_done = true;
            }
            s.flush();
            if !post {
                s.post_done = done;
            }
        }
    }
}

/// Draw whatever is still queued when the program presents a frame without a
/// `flush_3d` (the host would hold it until the next flush; here it would be lost).
pub fn end_frame() {
    let g = unsafe { &mut *(&raw mut G) };
    if let Some(s) = g.as_mut() {
        s.sync();
        if !s.deferred.is_empty() || s.frame_open || (s.drew_3d && !s.post_done) {
            s.flush();
        }
        s.adapt();
        // the render scale / clip mode is the running program's: another program starts at the defaults
        let app = crate::lingapp::active_app_name();
        if app != s.app {
            s.app = app;
            s.scale = 1.0;
            s.auto_scale = true;
            s.clip_seen = false;
            s.slow_frames = 0;
            s.fast_frames = 0;
        }
        s.post_done = false;
        // the host clears depth at every present: a model that holds still
        // must not fail the strict test against its own last frame
        if s.drew_3d {
            s.zclear = true;
        }
        s.drew_3d = false;
    }
}

// ── builtins ─────────────────────────────────────────────────────────────────

fn log(s: &str) {
    crate::console_write(s.as_bytes());
}

fn tex_arg(s: &State, id: f64, soft: bool) -> Option<TexId> {
    if id <= -1.5 && soft {
        Some(TexId::Soft)
    } else if id < 0.0 {
        Some(TexId::Glow)
    } else if (id as usize) < s.texs.len() {
        Some(TexId::Id(id as u32))
    } else {
        None
    }
}

include!("g3d_host.rs");

/// The 3-D builtins; `None` when `name` is not one of them.
pub fn dispatch(name: &[u8], args: &[u64]) -> Option<u64> {
    if let Some(v) = dispatch_host(st(), name, args) {
        return Some(v);
    }
    let shape_kind: Option<u8> = match name {
        b"cube" | b"box" => Some(0),
        b"sphere" => Some(1),
        b"cylinder" => Some(2),
        b"cone" => Some(3),
        b"capsule" => Some(4),
        b"torus" => Some(5),
        b"frustum" => Some(6),
        _ => None,
    };
    if let Some(kind) = shape_kind {
        let c = [f(args, 0, 0.0), f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 1.0), f(args, 4, 1.0), f(args, 5, 1.0), f(args, 6, 0.0), f(args, 7, 0.0), f(args, 8, 0.0)];
        let mode = n(args, 9, 0.0) as i32;
        let e0 = n(args, 10, 0.0) as i32;
        let e1 = if kind == 6 { (f(args, 11, 0.5).clamp(0.0, 4.0) * 1000.0) as i32 } else { n(args, 11, 0.0) as i32 };
        st().shape(kind, c, mode, e0, e1);
        return Some(UNIT);
    }
    let s = st();
    Some(match name {
        // ── camera ──
        b"set_camera" => {
            s.cam.cry = f(args, 0, 1.0);
            s.cam.sry = f(args, 1, 0.0);
            s.cam.crx = f(args, 2, 1.0);
            s.cam.srx = f(args, 3, 0.0);
            UNIT
        },
        b"set_camera_pos" => {
            s.cam.t = [f(args, 0, 0.0), f(args, 1, 0.0), f(args, 2, 0.0)];
            UNIT
        },
        b"move_camera" => {
            s.cam.t[0] += f(args, 0, 0.0);
            s.cam.t[1] += f(args, 1, 0.0);
            s.cam.t[2] += f(args, 2, 0.0);
            UNIT
        },
        b"set_zdist" => {
            s.cam.zdist = f(args, 0, 5.0);
            UNIT
        },
        b"set_camera_center" => {
            s.cam.center = Some((f(args, 0, 0.0), f(args, 1, 0.0)));
            UNIT
        },
        b"set_focal" => {
            s.cam.focal = f(args, 0, 0.0);
            UNIT
        },
        b"tm_fov" => {
            let deg = f(args, 0, 0.0);
            let bw = s.win_dims().map(|(w, _)| w as f32).unwrap_or(1280.0);
            let vw = f(args, 1, bw);
            s.cam.focal = if deg > 1.0 && deg < 179.0 { (vw * 0.5) / libm::tanf(deg.to_radians() * 0.5) } else { 0.0 };
            UNIT
        },
        // ── lights, depth, fog, shading ──
        b"set_ambient" => {
            s.ambient = f(args, 0, 0.15);
            UNIT
        },
        // light_pool(x, y, z, radius, r, g, b, strength): a soft additive glow disc lying flat on the
        // ground (y-down world), for a campfire / lamp. Two triangles with the glow texture.
        b"light_pool" => {
            let (x, y, z) = (f(args, 0, 0.0), f(args, 1, 0.0), f(args, 2, 0.0));
            let rad = f(args, 3, 1.0).max(0.01);
            let col = (n(args, 4, 255.0).clamp(0.0, 255.0) as u32) << 16 | (n(args, 5, 255.0).clamp(0.0, 255.0) as u32) << 8 | n(args, 6, 255.0).clamp(0.0, 255.0) as u32;
            let alpha = f(args, 7, 0.3).clamp(0.0, 1.0);
            let t = col3(col);
            let q = [[x - rad, y, z - rad], [x + rad, y, z - rad], [x + rad, y, z + rad], [x - rad, y, z + rad]];
            let p = Params { tex: TexId::Glow, alpha, blend: 1, zwrite: false, ztest: s.depth_test, lit: false, amb: 0.0, fog: false, hole: [0.0; 4], sprite: false };
            s.push_tri([q[0], q[1], q[2]], [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], [t; 3], p, 0, 0.0);
            s.push_tri([q[0], q[2], q[3]], [[0.0, 0.0], [1.0, 1.0], [0.0, 1.0]], [t; 3], p, 0, 0.0);
            UNIT
        },
        b"clear_lights" => {
            s.lights.clear();
            UNIT
        },
        b"add_light" => {
            let (mut r, mut g, mut b) = (f(args, 3, 1.0), f(args, 4, 1.0), f(args, 5, 1.0));
            if r > 1.5 || g > 1.5 || b > 1.5 {
                r /= 255.0;
                g /= 255.0;
                b /= 255.0;
            }
            let radius = f(args, 7, 0.0);
            s.lights.push(Light { p: [f(args, 0, 0.0), f(args, 1, -3.0), f(args, 2, 3.0)], c: [r, g, b], i: f(args, 6, 1.0), r: if radius > 0.0 { radius } else { 1.0e6 } });
            UNIT
        },
        b"clear_depth" => {
            s.zclear = true;
            UNIT
        },
        b"set_depth_test" => {
            s.depth_test = n(args, 0, 1.0) as i64 != 0;
            UNIT
        },
        // set_cull(mode): backface culling for draw_triangle_3d and mesh draws (see State::cull)
        b"set_cull" => {
            s.cull = n(args, 0, 1.0).clamp(0.0, 3.0) as u8;
            UNIT
        },
        b"set_fog" => {
            let (r, g, b) = (f(args, 0, 0.0).clamp(0.0, 255.0) as u32, f(args, 1, 0.0).clamp(0.0, 255.0) as u32, f(args, 2, 0.0).clamp(0.0, 255.0) as u32);
            s.fog_color = (r << 16) | (g << 8) | b;
            s.fog_start = f(args, 3, 0.0);
            s.fog_end = f(args, 4, 0.0);
            UNIT
        },
        b"set_shade_mode" => {
            s.shade_mode = n(args, 0, 2.0) as u8;
            UNIT
        },
        b"set_cel_bands" => {
            s.bands = (n(args, 0, 4.0) as u32).max(2);
            UNIT
        },
        b"set_rim" => {
            s.rim = f(args, 0, 0.6);
            s.rim_color = [f(args, 1, 115.0) / 255.0, f(args, 2, 217.0) / 255.0, f(args, 3, 255.0) / 255.0];
            UNIT
        },
        b"set_blend" => {
            s.blend = n(args, 0, 0.0) as u8;
            UNIT
        },
        b"flush_3d" | b"render_3d" | b"flush_post" | b"post_now" => {
            s.flush();
            UNIT
        },
        b"toon_outlines" => {
            s.toon_px = f(args, 0, 0.0);
            s.toon_color = rgb(n(args, 1, 0.0));
            s.toon_thresh = f(args, 2, 0.05);
            UNIT
        },
        b"set_tone_ramp" => {
            s.ramp_on = n(args, 0, 1.0) > 0.5;
            UNIT
        },
        // g3d_scale(s): 0 = automatic (full resolution while it is fast enough, else half),
        // 0.25..1 = fixed
        b"g3d_scale" => {
            let v = f(args, 0, 0.0);
            s.app = crate::lingapp::active_app_name();
            if v <= 0.0 {
                s.auto_scale = true;
                s.scale = 1.0;
            } else {
                s.auto_scale = false;
                s.scale = v.clamp(0.25, 1.0);
                log(&alloc::format!("g3d: {} renders 3-D at x{:.2} (fixed); UI and text stay native\n", s.app, s.scale));
            }
            UNIT
        },
        b"tm_wire" => {
            s.tm_wire(args);
            UNIT
        },
        // post-processing the kernel does not do (yet): accepted, no effect
        b"set_bloom" | b"set_fxaa" | b"set_ssao" | b"set_frame_blur" | b"screen_distort" | b"set_depth_blur" | b"set_antialias" | b"tm_ripple" | b"tm_ripple_use" | b"tm_ripple_clear" | b"tm_voice" | b"font_text_3d" | b"set_normal_override" | b"set_flat_shade" => UNIT,

        // ── textures and meshes ──
        b"tm_tex_load" => {
            let path = crate::lingapp::path_arg(args, 0);
            match load_tex(&path, n(args, 1, 256.0) as u32) {
                Some(t) => {
                    s.texs.push(t);
                    num((s.texs.len() - 1) as f64)
                },
                None => {
                    log(&alloc::format!("g3d: tm_tex_load {path}: not readable\n"));
                    num(-1.0)
                },
            }
        },
        b"tm_tex_count" => num(s.texs.len() as f64),
        b"tm_load" => {
            let path = crate::lingapp::path_arg(args, 0);
            match load_ltm(&path) {
                Some(m) => {
                    s.meshes.push(m);
                    num((s.meshes.len() - 1) as f64)
                },
                None => {
                    // once per file (games retry a missing mesh every half second): missing vs bad
                    let seen = unsafe { &mut *(&raw mut GLTF_REPORTED) };
                    if !seen.iter().any(|p| *p == path) {
                        seen.push(path.clone());
                        let why = if crate::vfs::exists(&path) { "not an LTM1 mesh (or unreadable)" } else { "no such file (not copied to the card?)" };
                        log(&alloc::format!("g3d: tm_load {path}: {why}\n"));
                    }
                    num(-1.0)
                },
            }
        },
        b"tm_groups" => num(s.meshes.get(n(args, 0, -1.0).max(0.0) as usize).map(|m| m.groups.len()).unwrap_or(0) as f64),
        b"tm_group" => {
            let key = args.get(1).map(|&v| to_string(v)).unwrap_or_default();
            let r = s
                .meshes
                .get(n(args, 0, -1.0).max(0.0) as usize)
                .filter(|_| n(args, 0, -1.0) >= 0.0)
                .and_then(|m| m.groups.iter().position(|g| g.name == key).or_else(|| m.groups.iter().position(|g| g.name.contains(key.as_str()))));
            num(r.map(|i| i as f64).unwrap_or(-1.0))
        },
        b"tm_group_name" => {
            let r = s.meshes.get(n(args, 0, -1.0).max(0.0) as usize).and_then(|m| m.groups.get(n(args, 1, -1.0).max(0.0) as usize)).map(|g| g.name.clone()).unwrap_or_default();
            crate::lingrt::mk_str(&r)
        },
        b"tm_group_tris" => num(s.meshes.get(n(args, 0, -1.0).max(0.0) as usize).and_then(|m| m.groups.get(n(args, 1, -1.0).max(0.0) as usize)).map(|g| g.idx.len()).unwrap_or(0) as f64),
        b"tm_bounds" => {
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            if let Some(m) = s.meshes.get(n(args, 0, -1.0).max(0.0) as usize) {
                let gsel = n(args, 1, -1.0);
                for (gi, g) in m.groups.iter().enumerate() {
                    if gsel >= 0.0 && gi != gsel as usize {
                        continue;
                    }
                    for p in &g.pos {
                        for k in 0..3 {
                            lo[k] = lo[k].min(p[k]);
                            hi[k] = hi[k].max(p[k]);
                        }
                    }
                }
            }
            if !lo[0].is_finite() {
                lo = [0.0; 3];
                hi = [0.0; 3];
            }
            nums(&[lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]])
        },
        b"tm_edges" => {
            let mut out: Vec<f32> = Vec::new();
            let mi = n(args, 0, -1.0);
            if mi >= 0.0 {
                if let Some(m) = s.meshes.get(mi as usize) {
                    let gsel = n(args, 1, -1.0);
                    let maxe = n(args, 2, 4000.0).max(1.0) as usize;
                    let q = |p: V3| ((p[0] * 20.0 + 0.5 * p[0].signum()) as i64, (p[1] * 20.0 + 0.5 * p[1].signum()) as i64, (p[2] * 20.0 + 0.5 * p[2].signum()) as i64);
                    let mut count: BTreeMap<((i64, i64, i64), (i64, i64, i64)), (u32, V3, V3)> = BTreeMap::new();
                    for (gi, g) in m.groups.iter().enumerate() {
                        if gsel >= 0.0 && gi != gsel as usize {
                            continue;
                        }
                        for t in &g.idx {
                            for k in 0..3 {
                                let a = g.pos[t[k] as usize];
                                let b = g.pos[t[(k + 1) % 3] as usize];
                                let (ka, kb) = (q(a), q(b));
                                let key = if ka < kb { (ka, kb) } else { (kb, ka) };
                                count.entry(key).or_insert((0, a, b)).0 += 1;
                            }
                        }
                    }
                    for (_, (c, a, b)) in count {
                        if c == 1 && out.len() / 6 < maxe {
                            out.extend_from_slice(&[a[0], a[1], a[2], b[0], b[1], b[2]]);
                        }
                    }
                }
            }
            nums(&out)
        },
        b"tm_wall_spots" => {
            let mut out: Vec<f32> = Vec::new();
            let mi = n(args, 0, -1.0);
            if mi >= 0.0 {
                if let Some(m) = s.meshes.get(mi as usize) {
                    let gsel = n(args, 1, -1.0);
                    let spacing = f(args, 2, 30.0).max(1.0);
                    let up = f(args, 3, 6.0);
                    let mut chosen: Vec<V3> = Vec::new();
                    for (gi, g) in m.groups.iter().enumerate() {
                        if gsel >= 0.0 && gi != gsel as usize {
                            continue;
                        }
                        for t in &g.idx {
                            let a = g.pos[t[0] as usize];
                            let b = g.pos[t[1] as usize];
                            let c = g.pos[t[2] as usize];
                            let fnrm = norm(cross(sub(b, a), sub(c, a)));
                            if fnrm[1].abs() > 0.3 {
                                continue;
                            }
                            let vn = g.nrm[t[0] as usize];
                            let nn = if dot(fnrm, vn) < 0.0 { [-fnrm[0], -fnrm[1], -fnrm[2]] } else { fnrm };
                            let bottom = a[1].max(b[1]).max(c[1]);
                            let top = a[1].min(b[1]).min(c[1]);
                            if bottom - top < up * 1.2 {
                                continue;
                            }
                            let p = [(a[0] + b[0] + c[0]) / 3.0, bottom - up, (a[2] + b[2] + c[2]) / 3.0];
                            if chosen.iter().any(|q| {
                                let d = sub(*q, p);
                                d[0] * d[0] + d[2] * d[2] < spacing * spacing && d[1].abs() < spacing
                            }) {
                                continue;
                            }
                            chosen.push(p);
                            out.extend_from_slice(&[p[0], p[1], p[2], nn[0], nn[1], nn[2]]);
                        }
                    }
                }
            }
            nums(&out)
        },
        b"tm_hole" => {
            s.hole = [f(args, 0, 0.0), f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)];
            UNIT
        },
        b"tm_draw" => num(s.tm_draw(args) as f64),
        b"tm_sprite" => {
            let Some(tex) = tex_arg(s, n(args, 0, -1.0), false) else { return Some(UNIT) };
            let (cx, cy) = (f(args, 1, 0.0), f(args, 2, 0.0));
            let (w, h) = (f(args, 3, 64.0) * 0.5, f(args, 4, 64.0) * 0.5);
            let rot = f(args, 5, 0.0);
            let c3 = col3(rgb(n(args, 6, 16777215.0)));
            let alpha = f(args, 7, 1.0);
            let blend = n(args, 8, 0.0) as u8;
            let (c, sn) = (cos(rot), sin(rot));
            let corner = |dx: f32, dy: f32, u: f32, v: f32| A { x: cx + dx * w * c - dy * h * sn, y: cy + dx * w * sn + dy * h * c, iw: 1.0, uw: u, vw: v, wx: 0.0, wy: 0.0, wz: 0.0, r: c3[0], g: c3[1], b: c3[2] };
            let q = [corner(-1.0, -1.0, 0.0, 0.0), corner(1.0, -1.0, 0.999, 0.0), corner(1.0, 1.0, 0.999, 0.999), corner(-1.0, 1.0, 0.0, 0.999)];
            let p = Params { tex, alpha, blend, zwrite: false, ztest: false, lit: false, amb: 0.0, fog: false, hole: [0.0; 4], sprite: true };
            s.draw_now(&[q[0], q[1], q[2]], &p);
            s.draw_now(&[q[0], q[2], q[3]], &p);
            UNIT
        },
        b"tm_billboard" => {
            if let Some(tex) = tex_arg(s, n(args, 0, -1.0), false) {
                let p = [f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)];
                s.billboard(tex, p, f(args, 4, 1.0), f(args, 5, 0.0), rgb(n(args, 6, 16777215.0)), f(args, 7, 1.0), n(args, 8, 0.0) as u8);
            }
            UNIT
        },
        b"tm_glyph" => s.glyph(args),
        b"tm_quad" => {
            if let Some(tex) = tex_arg(s, n(args, 0, -1.0), true) {
                let c = [f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)];
                let u = [f(args, 4, 1.0), f(args, 5, 0.0), f(args, 6, 0.0)];
                let v = [f(args, 7, 0.0), f(args, 8, 0.0), f(args, 9, 1.0)];
                let tint = col3(rgb(n(args, 10, 16777215.0)));
                let alpha = f(args, 11, 1.0);
                let blend = n(args, 12, 0.0) as u8;
                let bias = f(args, 13, 0.0);
                let at = |a: f32, b: f32| [c[0] + u[0] * a + v[0] * b, c[1] + u[1] * a + v[1] * b, c[2] + u[2] * a + v[2] * b];
                let q = [at(-1.0, -1.0), at(1.0, -1.0), at(1.0, 1.0), at(-1.0, 1.0)];
                let t = [[0.0, 0.0], [0.999, 0.0], [0.999, 0.999], [0.0, 0.999]];
                let p = Params { tex, alpha, blend, zwrite: false, ztest: s.depth_test, lit: false, amb: 0.0, fog: s.fog_end > 0.0, hole: [0.0; 4], sprite: false };
                s.push_tri([q[0], q[1], q[2]], [t[0], t[1], t[2]], [tint; 3], p, 0, bias);
                s.push_tri([q[0], q[2], q[3]], [t[0], t[2], t[3]], [tint; 3], p, 0, bias);
            }
            UNIT
        },
        // tm_field_tex(id, size, x0, z0, span, t, wells, contrast, gamma, bands, lights, alpha) -> id
        //   The host's thermal map of a gravity field's cross section (runtime/game3d.rs), written
        //   into texture `id` (a new one when id < 0): texel (i, j) is the world point
        //   (x0 + span (i + .5) / size, z0 + span (j + .5) / size); the potential is a slow level
        //   field plus every well in `wells` = [x, z, mass, radius, ...], mapped through a ROYGBIV
        //   thermal ramp with bright equipotential contour lines every 1 / bands, and vivid pools
        //   under `lights` = [x, z, radius, colour, ...] over a see-through base of `alpha`.
        //   One core rebuilds this every frame, so the texel loop does no trig and no powf: the
        //   contrast / gamma / ramp chain is a 257-entry LUT, and each mixed sinusoid is split by
        //   sin(fx + gz) = sin fx cos gz + cos fx sin gz into per-column and per-row parts.
        b"tm_field_tex" => {
            let id = n(args, 0, -1.0);
            let size = pow2((n(args, 1, 128.0) as u32).clamp(8, 512));
            let (x0, z0, span, t) = (f(args, 2, 0.0), f(args, 3, 0.0), f(args, 4, 64.0), f(args, 5, 0.0));
            let wells = flist(args, 6);
            let (contrast, gamma, bands) = (f(args, 7, 1.35), f(args, 8, 0.85).max(0.05), f(args, 9, 9.0).max(1.0));
            let lights = flist(args, 10);
            let base_alpha = f(args, 11, 1.0).clamp(0.0, 1.0);
            const RAMP: [[f32; 3]; 9] = [
                [0.0, 0.0, 0.05],
                [0.28, 0.0, 0.5],
                [0.0, 0.2, 1.0],
                [0.0, 0.9, 1.0],
                [0.1, 1.0, 0.3],
                [1.0, 1.0, 0.0],
                [1.0, 0.5, 0.0],
                [1.0, 0.0, 0.12],
                [1.0, 1.0, 1.0],
            ];
            let mut lut = [[0.0f32; 3]; 257];
            for (i, e) in lut.iter_mut().enumerate() {
                let d = libm::powf(i as f32 / 256.0, gamma);
                let d = d * d * (3.0 - 2.0 * d);
                let u = d * 7.999;
                let (a, b, fr) = (RAMP[u as usize], RAMP[u as usize + 1], u - floor(u));
                *e = [a[0] + (b[0] - a[0]) * fr, a[1] + (b[1] - a[1]) * fr, a[2] + (b[2] - a[2]) * fr];
            }
            let nn = size as usize;
            let k = span / size as f32;
            // per column: sin / cos of the x parts of the three mixed sinusoids, the x-only
            // sinusoid, and x itself
            let mut cols = vec![[0.0f32; 8]; nn];
            for (i, c) in cols.iter_mut().enumerate() {
                let x = x0 + (i as f32 + 0.5) * k;
                *c = [sin(x * 0.018), cos(x * 0.018), sin(x * 0.006), cos(x * 0.006), sin(x * 0.043), cos(x * 0.043), sin(x * 0.11 - t * 0.3), x];
            }
            let mut px = vec![0u32; nn * nn];
            for j in 0..nn {
                let z = z0 + (j as f32 + 0.5) * k;
                let (sg1, cg1) = (sin(z * 0.021 + t * 0.07), cos(z * 0.021 + t * 0.07));
                let (sg2, cg2) = (sin(z * 0.017 + t * 0.05), cos(z * 0.017 + t * 0.05));
                let (sg3, cg3) = (sin(z * 0.043 + t * 0.11), cos(z * 0.043 + t * 0.11));
                let e5 = sin(z * 0.09 + t * 0.23);
                for (cl, out) in cols.iter().zip(px[j * nn..j * nn + nn].iter_mut()) {
                    let x = cl[7];
                    // 0.22 sin(.018x + .021z + .07t) cos(.017z - .006x + .05t)
                    // + 0.1 sin(.043(x + z) + .11t) + 0.05 sin(.11x - .3t) sin(.09z + .23t)
                    let s1 = cl[0] * cg1 + cl[1] * sg1;
                    let c2 = cl[3] * cg2 + cl[2] * sg2;
                    let s3 = cl[4] * cg3 + cl[5] * sg3;
                    let mut phi = 0.5 + 0.22 * s1 * c2 + 0.1 * s3 + 0.05 * cl[6] * e5;
                    for w in wells.chunks_exact(4) {
                        let (dx, dz, r2) = (x - w[0], z - w[1], (w[3] * w[3]).max(0.01));
                        phi += w[2] * r2 / (dx * dx + dz * dz + r2);
                    }
                    let v = ((phi - 0.5) * contrast + 0.5).clamp(0.0, 1.0) * 256.0;
                    let vi = (v as usize).min(255);
                    let vf = v - vi as f32;
                    let (lo, hi) = (lut[vi], lut[vi + 1]);
                    let mut c = [lo[0] + (hi[0] - lo[0]) * vf, lo[1] + (hi[1] - lo[1]) * vf, lo[2] + (hi[2] - lo[2]) * vf];
                    // the cross-section slices: thin bright lines on the equipotentials
                    let q = phi * bands;
                    let q = q - floor(q);
                    let line = (1.0 - (if q < 1.0 - q { q } else { 1.0 - q }) / 0.09).clamp(0.0, 1.0);
                    let line = line * line;
                    for ch in c.iter_mut() {
                        *ch = (*ch * 0.78 * (1.0 - 0.5 * line) + 0.95 * line).clamp(0.0, 1.0);
                    }
                    let mut al = base_alpha + (1.0 - base_alpha) * line * 0.45;
                    for l in lights.chunks_exact(4) {
                        let (dx, dz, r) = (x - l[0], z - l[1], l[2].max(0.1));
                        let d2 = (dx * dx + dz * dz) / (r * r);
                        if d2 < 1.0 {
                            let g = (1.0 - d2) * (1.0 - d2);
                            let lc = l[3] as u32;
                            let lr = [((lc >> 16) & 255) as f32 / 255.0, ((lc >> 8) & 255) as f32 / 255.0, (lc & 255) as f32 / 255.0];
                            for (ch, v) in c.iter_mut().zip(lr) {
                                *ch = (*ch * (1.0 - 0.75 * g) + v * 1.3 * g).clamp(0.0, 1.0);
                            }
                            al += g;
                        }
                    }
                    let al = al.clamp(0.0, 1.0);
                    *out = ((al * 255.0) as u32) << 24 | ((c[0] * 255.0) as u32) << 16 | ((c[1] * 255.0) as u32) << 8 | (c[2] * 255.0) as u32;
                }
            }
            let tex = Tex::new(size, px);
            if id >= 0.0 && (id as usize) < s.texs.len() {
                s.texs[id as usize] = tex;
                num(id)
            } else {
                s.texs.push(tex);
                num((s.texs.len() - 1) as f64)
            }
        },
        // tm_wave_grid(tex, x0, z0, span, n, t, waves, ybase, sign, tint, alpha, blend, u0, v0, ru, rv)
        //   The host's sea surface (runtime/game3d.rs): an n x n textured grid over
        //   [x0, x0 + span] x [z0, z0 + span] whose vertices ride
        //   y = ybase + sign * sum(a sin(kx x + kz z + w t + p)) for waves = [kx, kz, w, a, p, ...];
        //   unlit, fogged, no z-write (ordered with the other translucent triangles at flush), and
        //   the texture runs from (u0, v0) over (ru, rv) across the grid and wraps.
        b"tm_wave_grid" => {
            let id = n(args, 0, -1.0);
            let tex = if id < 0.0 {
                Some(TexId::Glow)
            } else if (id as usize) < s.texs.len() {
                Some(TexId::Id(id as u32))
            } else {
                None
            };
            let Some(tex) = tex else { return Some(UNIT) };
            let (x0, z0, span) = (f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 64.0));
            let gn = (n(args, 4, 32.0) as usize).clamp(1, 128);
            let t = f(args, 5, 0.0);
            let waves = flist(args, 6);
            let (ybase, sign) = (f(args, 7, 0.0), f(args, 8, -1.0));
            let tint = col3(rgb(n(args, 9, 16777215.0)));
            let alpha = f(args, 10, 1.0);
            let blend = n(args, 11, 0.0) as u8;
            let (u0, v0, ru, rv) = (f(args, 12, 0.0), f(args, 13, 0.0), f(args, 14, 1.0), f(args, 15, 1.0));
            let h = |x: f32, z: f32| -> f32 {
                let mut sum = 0.0;
                for w in waves.chunks_exact(5) {
                    sum += w[3] * sin(w[0] * x + w[1] * z + w[2] * t + w[4]);
                }
                ybase + sign * sum
            };
            let m = gn + 1;
            let cell = span / gn as f32;
            let mut grid = vec![[0.0f32; 3]; m * m];
            let mut uvs = vec![[0.0f32; 2]; m * m];
            for j in 0..m {
                for i in 0..m {
                    let (x, z) = (x0 + i as f32 * cell, z0 + j as f32 * cell);
                    grid[j * m + i] = [x, h(x, z), z];
                    uvs[j * m + i] = [u0 + ru * i as f32 / gn as f32, v0 + rv * j as f32 / gn as f32];
                }
            }
            let p = Params { tex, alpha, blend, zwrite: false, ztest: s.depth_test, lit: false, amb: 0.0, fog: s.fog_end > 0.0, hole: [0.0; 4], sprite: false };
            for j in 0..gn {
                for i in 0..gn {
                    let a = j * m + i;
                    let (b, c, d) = (a + 1, a + m + 1, a + m);
                    s.push_tri([grid[a], grid[b], grid[c]], [uvs[a], uvs[b], uvs[c]], [tint; 3], p, 0, 0.0);
                    s.push_tri([grid[a], grid[c], grid[d]], [uvs[a], uvs[c], uvs[d]], [tint; 3], p, 0, 0.0);
                }
            }
            UNIT
        },

        // ── collision ──
        b"coll_build" => num(s.coll_build(args)),
        b"coll_tris" => num(s.colls.get(n(args, 0, -1.0).max(0.0) as usize).map(|c| c.tris.len()).unwrap_or(0) as f64),
        b"coll_sphere" => s.coll_sphere(args),
        b"coll_ray" => s.coll_ray(args),

        // ── particles ──
        b"pfx_emit" => {
            let c0 = rgb(n(args, 9, 16777215.0));
            let pt = Part {
                p: [f(args, 0, 0.0), f(args, 1, 0.0), f(args, 2, 0.0)],
                v: [f(args, 3, 0.0), f(args, 4, 0.0), f(args, 5, 0.0)],
                life: f(args, 6, 1.0),
                max: f(args, 6, 1.0).max(1e-3),
                s0: f(args, 7, 0.2),
                s1: f(args, 8, 0.0),
                c0,
                c1: rgb(n(args, 10, c0 as f64)),
                kind: n(args, 11, 0.0) as u8,
                tex: n(args, 12, 0.0).max(0.0) as u32,
                grav: f(args, 13, 1.0),
                spin: 0.0,
                alpha: f(args, 14, 1.0),
            };
            s.pfx_push(pt);
            UNIT
        },
        b"pfx_burst" => {
            let count = n(args, 3, 16.0).clamp(0.0, 2000.0) as usize;
            let (sp, life) = (f(args, 4, 5.0), f(args, 5, 1.0));
            let base = [f(args, 12, 0.0), f(args, 13, 0.0), f(args, 14, 0.0)];
            let c0 = rgb(n(args, 8, 16777215.0));
            let c1 = rgb(n(args, 9, c0 as f64));
            for _ in 0..count {
                let z = s.rand() * 2.0 - 1.0;
                let a = s.rand() * TAU;
                let r = sqrt((1.0 - z * z).max(0.0));
                let sv = sp * (0.55 + 0.45 * s.rand());
                let l = life * (0.6 + 0.4 * s.rand());
                let pt = Part {
                    p: [f(args, 0, 0.0), f(args, 1, 0.0), f(args, 2, 0.0)],
                    v: [base[0] + r * cos(a) * sv, base[1] + z * sv, base[2] + r * sin(a) * sv],
                    life: l,
                    max: l.max(1e-3),
                    s0: f(args, 6, 0.2),
                    s1: f(args, 7, 0.0),
                    c0,
                    c1,
                    kind: n(args, 10, 0.0) as u8,
                    tex: 0,
                    grav: f(args, 11, 1.0),
                    spin: 0.0,
                    alpha: 1.0,
                };
                s.pfx_push(pt);
            }
            UNIT
        },
        b"pfx_rain" => {
            let count = n(args, 5, 1.0).clamp(0.0, 2000.0) as usize;
            let ntex = n(args, 11, 1.0).max(1.0) as u32;
            let col = rgb(n(args, 13, 16777215.0));
            for _ in 0..count {
                let x = f(args, 0, 0.0) + (f(args, 2, 0.0) - f(args, 0, 0.0)) * s.rand();
                let z = f(args, 1, 0.0) + (f(args, 3, 0.0) - f(args, 1, 0.0)) * s.rand();
                let y = f(args, 4, 0.0) + (s.rand() - 0.5) * 2.0 * f(args, 15, 0.0);
                let l = f(args, 7, 3.0) * (0.25 + 0.75 * s.rand());
                let t = n(args, 10, 0.0).max(0.0) as u32 + ((s.rand() * ntex as f32) as u32).min(ntex - 1);
                let spin = (s.rand() - 0.5) * 2.0;
                let pt = Part { p: [x, y, z], v: [0.0, f(args, 6, 90.0), 0.0], life: l, max: l.max(1e-3), s0: f(args, 8, 6.0), s1: f(args, 9, 1.0), c0: col, c1: col, kind: 2, tex: t, grav: 0.0, spin, alpha: f(args, 12, 0.5) };
                s.pfx_push(pt);
            }
            UNIT
        },
        b"pfx_step" => {
            let dt = f(args, 0, 1.0 / 60.0);
            let g = f(args, 1, 9.8);
            let drag = (1.0 - f(args, 2, 0.0) * dt).clamp(0.0, 1.0);
            let floor_y = f(args, 3, 1e9);
            let bounce = f(args, 4, 0.4);
            s.parts.retain_mut(|p| {
                p.life -= dt;
                if p.life <= 0.0 {
                    return false;
                }
                p.v[1] += g * p.grav * dt;
                for k in 0..3 {
                    p.v[k] *= drag;
                    p.p[k] += p.v[k] * dt;
                }
                if p.p[1] > floor_y {
                    p.p[1] = floor_y;
                    p.v[1] = -p.v[1].abs() * bounce;
                    p.v[0] *= 0.7;
                    p.v[2] *= 0.7;
                }
                true
            });
            UNIT
        },
        b"pfx_draw" => {
            s.pfx_draw();
            UNIT
        },
        b"pfx_clear" => {
            s.parts.clear();
            UNIT
        },
        b"pfx_count" => num(s.parts.len() as f64),
        b"pfx_max" => {
            s.max_parts = n(args, 0, 6000.0).clamp(16.0, 100000.0) as usize;
            UNIT
        },
        _ => {
            let _ = str_of;
            return None;
        },
    })
}
