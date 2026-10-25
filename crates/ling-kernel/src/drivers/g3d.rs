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
}

impl Cam {
    #[inline]
    fn depth(&self, p: V3) -> f32 {
        let wx = p[0] - self.t[0];
        let wy = p[1] - self.t[1];
        let wz = p[2] - self.t[2];
        let rz1 = wx * self.sry + wz * self.cry;
        wy * self.srx + rz1 * self.crx
    }
    /// (screen x, screen y, camera depth) with the view's centre and focal length.
    #[inline]
    fn project(&self, p: V3, cx: f32, cy: f32, focal: f32) -> (f32, f32, f32) {
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

/// Everything the span loop needs besides the triangle itself.
struct Env<'a> {
    buf: &'a mut [u32],
    bw: usize,
    bh: usize,
    zb: &'a mut [f32],
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
    let y_start = (ceil(t0.y - 0.5) as i32).max(0);
    let y_end = (ceil(t2.y - 0.5) as i32).min(bh); // exclusive
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
            for px in x..xe {
                let idx = row + px as usize;
                if !ztest || iw > env.zb[idx] {
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
}

static mut G: Option<State> = None;

fn st() -> &'static mut State {
    let g = unsafe { &mut *(&raw mut G) };
    if g.is_none() {
        *g = Some(State {
            cam: Cam { cry: 1.0, sry: 0.0, crx: 1.0, srx: 0.0, focal: 0.0, zdist: 5.0, t: [0.0; 3], center: None },
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

impl State {
    fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / 16_777_216.0
    }

    /// The window's centre and focal length for this frame.
    fn view(&self, bw: usize, bh: usize) -> (f32, f32, f32) {
        let (cx, cy) = self.cam.center.unwrap_or((bw as f32 * 0.5, bh as f32 * 0.5));
        let focal = if self.cam.focal > 0.0 { self.cam.focal } else { bh as f32 };
        (cx, cy, focal)
    }

    fn ensure_z(&mut self, bw: usize, bh: usize) {
        if self.zw != bw || self.zh != bh {
            self.zbuf = vec![0.0; bw * bh];
            self.zw = bw;
            self.zh = bh;
            self.zclear = false;
        } else if self.zclear {
            self.zbuf.fill(0.0);
            self.zclear = false;
        }
    }

    /// Draw one screen-space triangle now.
    fn draw_now(&mut self, v: &[A; 3], p: &Params) {
        let Some((buf, bw, bh)) = crate::lingapp::surface() else { return };
        if !p.sprite {
            self.ensure_z(bw, bh);
        } else if self.zw != bw || self.zh != bh {
            self.ensure_z(bw, bh);
        }
        self.tris += 1;
        let State { zbuf, texs, glow, soft, solid, lights, fog_color, fog_start, fog_end, cam, .. } = self;
        let tex = match p.tex {
            TexId::None => None,
            TexId::Glow => Some(&*glow),
            TexId::Soft => Some(&*soft),
            TexId::Solid => Some(&*solid),
            TexId::Id(i) => texs.get(i as usize),
        };
        if p.tex != TexId::None && tex.is_none() {
            return;
        }
        let mut env = Env { buf, bw, bh, zb: &mut zbuf[..], tex, lights: &lights[..], fog_color: *fog_color, fog_start: *fog_start, fog_end: *fog_end, zdist: cam.zdist };
        raster(&mut env, v, p);
    }

    /// Clip a world-space triangle against the near plane, project it and
    /// draw it (opaque) or keep it for `flush` (translucent). `col` is the
    /// per-vertex colour; `cull` 1 / 2 drops one screen winding.
    #[allow(clippy::too_many_arguments)]
    fn push_tri(&mut self, wp: [V3; 3], uv: [[f32; 2]; 3], col: [V3; 3], p: Params, cull: u8, bias: f32) -> usize {
        let Some((_, bw, bh)) = crate::lingapp::surface() else { return 0 };
        let near = -self.cam.zdist + 0.05;
        let d = [self.cam.depth(wp[0]), self.cam.depth(wp[1]), self.cam.depth(wp[2])];
        if d[0] <= near && d[1] <= near && d[2] <= near {
            return 0;
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
        let (cx, cy, focal) = self.view(bw, bh);
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
        let immediate = p.zwrite && p.alpha > 0.97 && p.blend == 0;
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

    /// Draw the translucent triangles kept since the last flush, far to near.
    fn flush(&mut self) {
        if self.deferred.is_empty() {
            return;
        }
        let mut list = core::mem::take(&mut self.deferred);
        list.sort_by(|a, b| a.key.partial_cmp(&b.key).unwrap_or(core::cmp::Ordering::Equal));
        for d in &list {
            self.draw_now(&d.v, &d.p);
        }
        list.clear();
        self.deferred = list;
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
            let Some((buf, bw, bh)) = crate::lingapp::surface() else { return };
            let (vcx, vcy, focal) = self.view(bw, bh);
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
        let cull = n(args, 16, 0.0) as u8;
        let lit_amb = f(args, 17, -1.0);
        let Some((_, bw, bh)) = crate::lingapp::surface() else { return 0 };
        let hole = if self.hole[3] > 0.0 {
            let (cx, cy, focal) = self.view(bw, bh);
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
        let Some((_, bw, bh)) = crate::lingapp::surface() else { return };
        let (cx, cy, focal) = self.view(bw, bh);
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
                    if let Some((_, bw, bh)) = crate::lingapp::surface() {
                        let (cx, cy, focal) = self.view(bw, bh);
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
                _ => self.billboard(TexId::Glow, p.p, size, 0.0, col, fade * p.alpha, 1),
            }
        }
        self.parts = parts;
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
        let Some(font) = crate::lingapp::font_fd(fid) else { return nums(&[-1.0, 0.0]) };
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

/// Draw whatever is still queued when the program presents a frame without a
/// `flush_3d` (the host would hold it until the next flush; here it would be lost).
pub fn end_frame() {
    let g = unsafe { &mut *(&raw mut G) };
    if let Some(s) = g.as_mut() {
        if !s.deferred.is_empty() {
            s.flush();
        }
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

/// The 3-D builtins; `None` when `name` is not one of them.
pub fn dispatch(name: &[u8], args: &[u64]) -> Option<u64> {
    let shape_kind: Option<u8> = match name {
        b"cube" | b"box" => Some(0),
        b"sphere" => Some(1),
        b"cylinder" => Some(2),
        b"cone" => Some(3),
        b"capsule" => Some(4),
        b"torus" => Some(5),
        _ => None,
    };
    if let Some(kind) = shape_kind {
        let c = [f(args, 0, 0.0), f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 1.0), f(args, 4, 1.0), f(args, 5, 1.0), f(args, 6, 0.0), f(args, 7, 0.0), f(args, 8, 0.0)];
        let mode = n(args, 9, 0.0) as i32;
        let e0 = n(args, 10, 0.0) as i32;
        let e1 = n(args, 11, 0.0) as i32;
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
            let bw = crate::lingapp::surface().map(|(_, w, _)| w as f32).unwrap_or(1280.0);
            let vw = f(args, 1, bw);
            s.cam.focal = if deg > 1.0 && deg < 179.0 { (vw * 0.5) / libm::tanf(deg.to_radians() * 0.5) } else { 0.0 };
            UNIT
        },
        // ── lights, depth, fog, shading ──
        b"set_ambient" => {
            s.ambient = f(args, 0, 0.15);
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
        // post-processing the kernel does not do (yet): accepted, no effect
        b"mesh_hue" | b"toon_outlines" | b"set_bloom" | b"set_frame_blur" | b"screen_distort" | b"set_depth_blur" | b"set_antialias" | b"tm_ripple" | b"tm_ripple_use" | b"tm_ripple_clear" | b"tm_voice" | b"font_text_3d" | b"set_normal_override" | b"set_flat_shade" => UNIT,

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
                    log(&alloc::format!("g3d: tm_load {path}: not an LTM1 mesh\n"));
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
