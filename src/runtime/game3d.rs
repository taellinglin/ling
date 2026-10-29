//! `tm_*`, `coll_*` and `pfx_*` builtins: textured meshes, triangle-mesh collision and a
//! native particle system for 3-D games (LingDev's Soul Symphony port and Ling Breaker).
//!
//! * `tm_tex_load(path, size)` loads an SVG / SVGZ (rendered with resvg) or PNG into a
//!   power-of-two texture. `tm_load(path)` reads an `.ltm` mesh (groups of textured
//!   triangles). `tm_draw(...)` queues a group into the depth queue as perspective-correct
//!   textured triangles, near-clipped, with tint, alpha, UV scroll / scale, simple shading
//!   and the see-through circle set by `tm_hole`. `tm_sprite(...)` draws a rotated textured
//!   quad straight onto the 2-D framebuffer (title cards, HUD).
//! * `coll_build(mesh, ...)` turns a mesh into a uniform-grid collision soup;
//!   `coll_sphere(id, x, y, z, r)` pushes a sphere out of it and returns the contact normal;
//!   `coll_ray(...)` casts a ray (camera collision, line of sight).
//! * `pfx_emit / pfx_burst / pfx_rain / pfx_step / pfx_draw / pfx_clear / pfx_count`:
//!   glow dots, streaks and textured sprites, simulated and drawn natively.
//!
//! The `.ltm` format (little endian): b"LTM1", u32 group count, then per group:
//! u8 name length, name bytes, u32 vertex count, u32 triangle count,
//! vertices as 8 f32 (x y z u v nx ny nz), triangles as 3 u32.
use super::{EvalResult, Interpreter, Value};
use crate::gfx::depth::{TexData, TexShade, TexVert};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

pub(super) struct TmGroup {
    name: String,
    pos: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    nrm: Vec<[f32; 3]>,
    idx: Vec<[u32; 3]>,
}

pub(super) struct TmMesh {
    groups: Vec<TmGroup>,
}

pub(super) struct Coll {
    tris: Vec<[[f32; 3]; 3]>,
    nrm: Vec<[f32; 3]>,
    cell: f32,
    grid: HashMap<(i32, i32, i32), Vec<u32>>,
    stamp: Vec<u32>,
    frame: u32,
}

#[derive(Clone, Copy)]
struct Part {
    p: [f32; 3],
    v: [f32; 3],
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

pub(super) struct G3dState {
    texs: Vec<Arc<TexData>>,
    meshes: Vec<TmMesh>,
    colls: Vec<Coll>,
    parts: Vec<Part>,
    hole: [f32; 4],
    glow: Option<Arc<TexData>>,
    rng: u64,
    max_parts: usize,
    ripples: Vec<Ripple>,
    ripple_on: bool,
    trail: Vec<u32>,
    lens: Vec<u32>,
}

/// A floor shockwave started by `tm_ripple`.
struct Ripple {
    x: f32,
    z: f32,
    t0: std::time::Instant,
    amp: f32,
    speed: f32,
    wl: f32,
    life: f32,
}

impl Default for G3dState {
    fn default() -> Self {
        Self {
            texs: Vec::new(),
            meshes: Vec::new(),
            colls: Vec::new(),
            parts: Vec::new(),
            hole: [0.0; 4],
            glow: None,
            ripples: Vec::new(),
            ripple_on: false,
            trail: Vec::new(),
            lens: Vec::new(),
            rng: 0x9E37_79B9_7F4A_7C15,
            max_parts: 6000,
        }
    }
}

impl G3dState {
    fn rand(&mut self) -> f32 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        ((x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32) / ((1u64 << 24) as f32)
    }
    fn glow_tex(&mut self) -> Arc<TexData> {
        if let Some(g) = &self.glow {
            return g.clone();
        }
        let n = 64u32;
        let mut px = vec![0u32; (n * n) as usize];
        for y in 0..n {
            for x in 0..n {
                let dx = (x as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                let dy = (y as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                let d = (dx * dx + dy * dy).sqrt();
                let core = (1.0 - d / 0.35).clamp(0.0, 1.0);
                let halo = (1.0 - d).clamp(0.0, 1.0);
                let a = (core * core * 0.9 + halo * halo * halo * 0.55).clamp(0.0, 1.0);
                let a8 = (a * 255.0) as u32;
                px[(y * n + x) as usize] = (a8 << 24) | 0x00FF_FFFF;
            }
        }
        let t = Arc::new(TexData { w: n, h: n, px });
        self.glow = Some(t.clone());
        t
    }
}

/// Vertices (unit radius) and edges of a `tm_wire` shape.
fn wire_shape(kind: i32, detail: usize) -> (Vec<[f32; 3]>, Vec<(usize, usize)>) {
    let mut v: Vec<[f32; 3]> = Vec::new();
    let mut e: Vec<(usize, usize)> = Vec::new();
    // every pair of vertices at the polyhedron's edge length
    fn by_length(v: &[[f32; 3]], e: &mut Vec<(usize, usize)>) {
        let d2 = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2);
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
    let tau = std::f32::consts::TAU;
    match kind {
        1 => {
            let k = 1.0 / 3f32.sqrt();
            v = vec![[k, k, k], [k, -k, -k], [-k, k, -k], [-k, -k, k]];
            by_length(&v, &mut e);
        },
        2 => {
            v = vec![[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0]];
            by_length(&v, &mut e);
        },
        3 => {
            let g = (1.0 + 5f32.sqrt()) / 2.0;
            let n = (1.0 + g * g).sqrt();
            for &(a, b) in &[(1.0f32, g), (-1.0, g), (1.0, -g), (-1.0, -g)] {
                v.push([0.0, a / n, b / n]);
                v.push([a / n, b / n, 0.0]);
                v.push([b / n, 0.0, a / n]);
            }
            by_length(&v, &mut e);
        },
        4 => {
            let (maj, min) = (detail, (detail / 2).max(4));
            for i in 0..maj {
                let a = i as f32 / maj as f32 * tau;
                for j in 0..min {
                    let b = j as f32 / min as f32 * tau;
                    let r = 0.72 + 0.28 * b.cos();
                    v.push([r * a.cos(), 0.28 * b.sin(), r * a.sin()]);
                    let here = i * min + j;
                    e.push((here, i * min + (j + 1) % min));
                    e.push((here, ((i + 1) % maj) * min + j));
                }
            }
        },
        5 => {
            let (rings, segs) = ((detail / 2).max(3), detail);
            for i in 1..rings {
                let p = i as f32 / rings as f32 * std::f32::consts::PI;
                for j in 0..segs {
                    let a = j as f32 / segs as f32 * tau;
                    v.push([p.sin() * a.cos(), p.cos(), p.sin() * a.sin()]);
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
                let a = j as f32 / detail as f32 * tau;
                v.push([a.cos(), a.sin(), 0.0]);
                e.push((j, (j + 1) % detail));
            }
        },
        7 => {
            for layer in 0..2 {
                let z = if layer == 0 { -0.25 } else { 0.25 };
                for j in 0..10 {
                    let a = j as f32 / 10.0 * tau - std::f32::consts::FRAC_PI_2;
                    let r = if j % 2 == 0 { 1.0 } else { 0.42 };
                    v.push([r * a.cos(), r * a.sin(), z]);
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
            for &x in &[-1.0f32, 1.0] {
                for &y in &[-1.0f32, 1.0] {
                    for &z in &[-1.0f32, 1.0] {
                        let k = 1.0 / 3f32.sqrt();
                        v.push([x * k, y * k, z * k]);
                    }
                }
            }
            by_length(&v, &mut e);
        },
    }
    (v, e)
}

/// Vertices (unit radius) and triangle faces of a `tm_solid` shape.
fn solid_shape(kind: i32) -> (Vec<[f32; 3]>, Vec<[usize; 3]>) {
    if kind == 0 {
        let k = 1.0 / 3f32.sqrt();
        let v: Vec<[f32; 3]> = (0..8).map(|i| [if i & 1 == 0 { -k } else { k }, if i & 2 == 0 { -k } else { k }, if i & 4 == 0 { -k } else { k }]).collect();
        let quads = [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]];
        let mut f = Vec::new();
        for q in quads {
            f.push([q[0], q[1], q[2]]);
            f.push([q[0], q[2], q[3]]);
        }
        return (v, f);
    }
    // triangular-faced polyhedra: every triple of mutually connected vertices is a face
    let (v, e) = wire_shape(if kind == 9 { 2 } else { kind }, 12);
    let has = |i: usize, j: usize| e.iter().any(|&(a, b)| (a == i && b == j) || (a == j && b == i));
    let mut f = Vec::new();
    for i in 0..v.len() {
        for j in i + 1..v.len() {
            if !has(i, j) {
                continue;
            }
            for k in j + 1..v.len() {
                if has(i, k) && has(j, k) {
                    f.push([i, j, k]);
                }
            }
        }
    }
    (v, f)
}

/// One near-clipped, projected, depth-sorted 3-D line (twice, a pixel apart, when `thick`).
fn wire_line(gfx: &mut crate::gfx::GfxState, a: [f32; 3], b: [f32; 3], col: u32, thick: bool) {
    let near = -gfx.camera.zdist + 0.05;
    let (mut a, mut b) = (a, b);
    let da = gfx.camera.depth(a[0], a[1], a[2]);
    let db = gfx.camera.depth(b[0], b[1], b[2]);
    if da <= near && db <= near {
        return;
    }
    if da <= near {
        let t = (near - da) / (db - da);
        a = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
    } else if db <= near {
        let t = (near - da) / (db - da);
        b = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
    }
    let (ax, ay, az) = gfx.camera.project(a[0], a[1], a[2]);
    let (bx, by, bz) = gfx.camera.project(b[0], b[1], b[2]);
    if !(ax.is_finite() && ay.is_finite() && bx.is_finite() && by.is_finite()) {
        return;
    }
    let lim = 20000.0;
    if ax.abs() > lim || ay.abs() > lim || bx.abs() > lim || by.abs() > lim {
        return;
    }
    let col = gfx.fog_apply(col, (az + bz) * 0.5);
    let depth = (az + bz) * 0.5;
    gfx.depth_queue.push_line(depth, col, ax, ay, bx, by);
    if thick {
        gfx.depth_queue.push_line(depth, col, ax + 1.0, ay, bx + 1.0, by);
        gfx.depth_queue.push_line(depth, col, ax, ay + 1.0, bx, by + 1.0);
    }
}

/// Start `script` where the user can see it (see `tm_sys_run`).
#[cfg(not(target_arch = "wasm32"))]
fn sys_run_script(script: &std::path::Path, dir: &std::path::Path) -> bool {
    use std::process::Command;
    let sp = script.to_string_lossy().into_owned();
    if cfg!(windows) {
        return Command::new("cmd").args(["/C", "start", "", &sp.replace('/', "\\")]).current_dir(dir).spawn().is_ok();
    }
    if cfg!(target_os = "macos") {
        let _ = Command::new("chmod").args(["+x", &sp]).status();
        return Command::new("open").args(["-a", "Terminal", &sp]).spawn().is_ok();
    }
    let _ = Command::new("chmod").args(["+x", &sp]).status();
    // Raspberry Pi OS ships lxterminal; the rest are common desktop terminals
    let hold = format!("sh '{}'; echo; echo '[press Enter to close]'; read _", sp.replace('\'', ""));
    for (term, flag) in [("x-terminal-emulator", "-e"), ("lxterminal", "-e"), ("gnome-terminal", "--"), ("konsole", "-e"), ("xfce4-terminal", "-x"), ("xterm", "-e")] {
        let ok = Command::new(term).arg(flag).args(["sh", "-c", &hold]).current_dir(dir).spawn().is_ok();
        if ok {
            return true;
        }
    }
    let log = std::fs::File::create(dir.join("build.log"));
    match log {
        Ok(f) => {
            let err = f.try_clone().ok();
            let mut c = Command::new("sh");
            c.arg(&sp).current_dir(dir).stdout(f);
            if let Some(e) = err {
                c.stderr(e);
            }
            c.spawn().is_ok()
        },
        Err(_) => Command::new("sh").arg(&sp).current_dir(dir).spawn().is_ok(),
    }
}

/// A soft round shadow: white with alpha falling smoothly from 1 at the centre to 0 at the rim.
fn soft_tex() -> Arc<TexData> {
    static SOFT: std::sync::OnceLock<Arc<TexData>> = std::sync::OnceLock::new();
    SOFT.get_or_init(|| {
        let n = 64u32;
        let mut px = vec![0u32; (n * n) as usize];
        for y in 0..n {
            for x in 0..n {
                let dx = (x as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                let dy = (y as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                let t = (1.0 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
                let a = t * t * (3.0 - 2.0 * t);
                px[(y * n + x) as usize] = (((a * 255.0) as u32) << 24) | 0x00FF_FFFF;
            }
        }
        Arc::new(TexData { w: n, h: n, px })
    })
    .clone()
}

// ── argument helpers ─────────────────────────────────────────────────────────
fn num(args: &[Value], i: usize, d: f64) -> f64 {
    match args.get(i) {
        Some(Value::Number(n)) => *n,
        Some(Value::Bool(b)) => *b as i32 as f64,
        Some(Value::Str(s)) => s.trim().parse().unwrap_or(d),
        _ => d,
    }
}
fn f(args: &[Value], i: usize, d: f32) -> f32 {
    num(args, i, d as f64) as f32
}
fn text(args: &[Value], i: usize) -> String {
    match args.get(i) {
        Some(Value::Str(s)) => s.to_string(),
        Some(v) => v.to_string(),
        None => String::new(),
    }
}
fn list_f32(args: &[Value], i: usize) -> Option<Vec<f32>> {
    match args.get(i) {
        Some(Value::List(l)) => Some(
            l.iter()
                .map(|v| match v {
                    Value::Number(n) => *n as f32,
                    _ => 0.0,
                })
                .collect(),
        ),
        _ => None,
    }
}
fn nums(v: &[f32]) -> Value {
    Value::List(Rc::new(v.iter().map(|&x| Value::Number(x as f64)).collect()))
}
fn rgb(c: f64) -> u32 {
    (c.max(0.0) as u64 & 0x00FF_FFFF) as u32
}
fn lerp_rgb(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| -> u32 {
        let x = ((a >> s) & 255) as f32;
        let y = ((b >> s) & 255) as f32;
        ((x + (y - x) * t).clamp(0.0, 255.0) as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

// ── texture loading ──────────────────────────────────────────────────────────
fn pow2(n: u32) -> u32 {
    let mut p = 1u32;
    while p < n.max(1) && p < 4096 {
        p <<= 1;
    }
    p
}

/// Resample an RGBA8 image (premultiplied = false) to `size`×`size` with a box /
/// bilinear filter so big sources shrink without aliasing.
fn resample(src: &[u8], sw: u32, sh: u32, size: u32) -> Vec<u32> {
    let mut out = vec![0u32; (size * size) as usize];
    let fx = sw as f32 / size as f32;
    let fy = sh as f32 / size as f32;
    let taps = (fx.max(fy).ceil() as u32).clamp(1, 8);
    for y in 0..size {
        for x in 0..size {
            let mut acc = [0.0f32; 4];
            let mut n = 0.0f32;
            for j in 0..taps {
                for i in 0..taps {
                    let sx = (((x as f32 + (i as f32 + 0.5) / taps as f32) * fx) as u32).min(sw - 1);
                    let sy = (((y as f32 + (j as f32 + 0.5) / taps as f32) * fy) as u32).min(sh - 1);
                    let o = ((sy * sw + sx) * 4) as usize;
                    let a = src[o + 3] as f32;
                    // alpha-weighted so transparent texels do not bleed black
                    acc[0] += src[o] as f32 * a;
                    acc[1] += src[o + 1] as f32 * a;
                    acc[2] += src[o + 2] as f32 * a;
                    acc[3] += a;
                    n += 1.0;
                }
            }
            let a = acc[3] / n;
            let (r, g, b) = if acc[3] > 0.0 {
                (acc[0] / acc[3], acc[1] / acc[3], acc[2] / acc[3])
            } else {
                (0.0, 0.0, 0.0)
            };
            out[(y * size + x) as usize] =
                ((a as u32).min(255) << 24) | ((r as u32).min(255) << 16) | ((g as u32).min(255) << 8) | (b as u32).min(255);
        }
    }
    out
}

fn load_texture(path: &str, size: u32) -> Result<TexData, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let lower = path.to_lowercase();
    let size = pow2(size.clamp(8, 2048));
    if lower.ends_with(".svg") || lower.ends_with(".svgz") {
        let opt = resvg::usvg::Options::default();
        let tree = resvg::usvg::Tree::from_data(&bytes, &opt).map_err(|e| format!("{path}: {e}"))?;
        let ts = tree.size();
        let mut pm = resvg::tiny_skia::Pixmap::new(size, size).ok_or("pixmap")?;
        let tf = resvg::tiny_skia::Transform::from_scale(size as f32 / ts.width(), size as f32 / ts.height());
        resvg::render(&tree, tf, &mut pm.as_mut());
        // tiny-skia pixels are premultiplied RGBA
        let mut px = Vec::with_capacity((size * size) as usize);
        for c in pm.pixels() {
            let a = c.alpha() as u32;
            let un = |v: u8| if a == 0 { 0 } else { ((v as u32 * 255 + a / 2) / a).min(255) };
            px.push((a << 24) | (un(c.red()) << 16) | (un(c.green()) << 8) | un(c.blue()));
        }
        return Ok(TexData { w: size, h: size, px });
    }
    let img = image::load_from_memory(&bytes).map_err(|e| format!("{path}: {e}"))?.to_rgba8();
    let (w, h) = img.dimensions();
    Ok(TexData { w: size, h: size, px: resample(img.as_raw(), w, h, size) })
}

// ── .ltm meshes ──────────────────────────────────────────────────────────────
fn load_ltm(path: &str) -> Result<TmMesh, String> {
    let b = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if b.len() < 8 || &b[0..4] != b"LTM1" {
        return Err(format!("{path}: not an LTM1 mesh"));
    }
    let mut o = 4usize;
    let u32_at = |b: &[u8], o: &mut usize| -> Result<u32, String> {
        if *o + 4 > b.len() {
            return Err("truncated".into());
        }
        let v = u32::from_le_bytes([b[*o], b[*o + 1], b[*o + 2], b[*o + 3]]);
        *o += 4;
        Ok(v)
    };
    let f32_at = |b: &[u8], o: &mut usize| -> Result<f32, String> {
        if *o + 4 > b.len() {
            return Err("truncated".into());
        }
        let v = f32::from_le_bytes([b[*o], b[*o + 1], b[*o + 2], b[*o + 3]]);
        *o += 4;
        Ok(v)
    };
    let ng = u32_at(&b, &mut o)?;
    let mut groups = Vec::new();
    for _ in 0..ng {
        if o >= b.len() {
            return Err("truncated".into());
        }
        let nl = b[o] as usize;
        o += 1;
        let name = String::from_utf8_lossy(&b[o..(o + nl).min(b.len())]).to_string();
        o += nl;
        let nv = u32_at(&b, &mut o)? as usize;
        let nt = u32_at(&b, &mut o)? as usize;
        let mut g = TmGroup { name, pos: Vec::with_capacity(nv), uv: Vec::with_capacity(nv), nrm: Vec::with_capacity(nv), idx: Vec::with_capacity(nt) };
        for _ in 0..nv {
            let x = f32_at(&b, &mut o)?;
            let y = f32_at(&b, &mut o)?;
            let z = f32_at(&b, &mut o)?;
            let u = f32_at(&b, &mut o)?;
            let v = f32_at(&b, &mut o)?;
            let nx = f32_at(&b, &mut o)?;
            let ny = f32_at(&b, &mut o)?;
            let nz = f32_at(&b, &mut o)?;
            g.pos.push([x, y, z]);
            g.uv.push([u, v]);
            g.nrm.push([nx, ny, nz]);
        }
        for _ in 0..nt {
            let a = u32_at(&b, &mut o)?;
            let c = u32_at(&b, &mut o)?;
            let d = u32_at(&b, &mut o)?;
            if (a as usize) < nv && (c as usize) < nv && (d as usize) < nv {
                g.idx.push([a, c, d]);
            }
        }
        groups.push(g);
    }
    Ok(TmMesh { groups })
}

// ── transforms ───────────────────────────────────────────────────────────────
/// Row-major 3×3 rotation from a yaw number or a 9-element list.
fn rot_from(args: &[Value], i: usize) -> [f32; 9] {
    if let Some(l) = list_f32(args, i) {
        if l.len() >= 9 {
            return [l[0], l[1], l[2], l[3], l[4], l[5], l[6], l[7], l[8]];
        }
    }
    let yaw = f(args, i, 0.0);
    let (c, s) = (yaw.cos(), yaw.sin());
    // same convention as mesh_draw / body_draw: x' = x c + z s, z' = z c - x s
    [c, 0.0, s, 0.0, 1.0, 0.0, -s, 0.0, c]
}
#[inline]
fn xf(m: &[f32; 9], p: [f32; 3], s: f32, o: [f32; 3]) -> [f32; 3] {
    [
        o[0] + s * (m[0] * p[0] + m[1] * p[1] + m[2] * p[2]),
        o[1] + s * (m[3] * p[0] + m[4] * p[1] + m[5] * p[2]),
        o[2] + s * (m[6] * p[0] + m[7] * p[1] + m[8] * p[2]),
    ]
}
#[inline]
fn rotn(m: &[f32; 9], n: [f32; 3]) -> [f32; 3] {
    [
        m[0] * n[0] + m[1] * n[1] + m[2] * n[2],
        m[3] * n[0] + m[4] * n[1] + m[5] * n[2],
        m[6] * n[0] + m[7] * n[1] + m[8] * n[2],
    ]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    if l > 1e-9 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0, -1.0, 0.0]
    }
}

/// Closest point on triangle abc to p (Ericson, Real-Time Collision Detection 5.1.5).
fn closest_on_tri(p: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
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

impl Coll {
    fn key(&self, p: [f32; 3]) -> (i32, i32, i32) {
        ((p[0] / self.cell).floor() as i32, (p[1] / self.cell).floor() as i32, (p[2] / self.cell).floor() as i32)
    }
    /// Triangle indices whose cells overlap the box [lo, hi], each once.
    fn query(&mut self, lo: [f32; 3], hi: [f32; 3], out: &mut Vec<u32>) {
        out.clear();
        self.frame = self.frame.wrapping_add(1);
        let a = self.key(lo);
        let b = self.key(hi);
        if (b.0 - a.0) as i64 * (b.1 - a.1) as i64 * (b.2 - a.2) as i64 > 4096 {
            for i in 0..self.tris.len() as u32 {
                out.push(i);
            }
            return;
        }
        for x in a.0..=b.0 {
            for y in a.1..=b.1 {
                for z in a.2..=b.2 {
                    if let Some(l) = self.grid.get(&(x, y, z)) {
                        for &t in l {
                            if self.stamp[t as usize] != self.frame {
                                self.stamp[t as usize] = self.frame;
                                out.push(t);
                            }
                        }
                    }
                }
            }
        }
    }
}

impl Interpreter {
    pub(super) fn game3d_builtin(&mut self, name: &str, args: &[Value]) -> Option<EvalResult> {
        if !(name.starts_with("tm_")
            || name.starts_with("coll_")
            || name.starts_with("pfx_")
            || matches!(name, "str_to_hex" | "hex_to_str" | "net_local_ip" | "rand_seed" | "clipboard_set" | "clipboard_get" | "random_hex"))
        {
            return None;
        }
        Some(self.game3d_call(name, args))
    }

    fn game3d_call(&mut self, name: &str, args: &[Value]) -> EvalResult {
        match name {
            // tm_collide(ax, ay, ar, bx, by, br) -> [i0, j0, i1, j1, ...]: every pair of overlapping
            //   circles between set A and set B (lists of x, y; radius a number or a list per item).
            //   Native, so a shooter can test every bullet against every rock in one call.
            "tm_collide" => {
                let fl = |v: Option<&Value>| -> Vec<f32> {
                    match v {
                        Some(Value::List(l)) => l.iter().map(|x| if let Value::Number(n) = x { *n as f32 } else { 0.0 }).collect(),
                        _ => Vec::new(),
                    }
                };
                let rad = |v: Option<&Value>, n: usize| -> Vec<f32> {
                    match v {
                        Some(Value::Number(r)) => vec![*r as f32; n],
                        Some(Value::List(l)) => l.iter().map(|x| if let Value::Number(n) = x { *n as f32 } else { 0.0 }).collect(),
                        _ => vec![0.0; n],
                    }
                };
                let (ax, ay) = (fl(args.first()), fl(args.get(1)));
                let (bx, by) = (fl(args.get(3)), fl(args.get(4)));
                let ar = rad(args.get(2), ax.len());
                let br = rad(args.get(5), bx.len());
                let mut out = Vec::new();
                for i in 0..ax.len().min(ay.len()).min(ar.len()) {
                    for j in 0..bx.len().min(by.len()).min(br.len()) {
                        let (dx, dy, r) = (ax[i] - bx[j], ay[i] - by[j], ar[i] + br[j]);
                        if dx * dx + dy * dy < r * r {
                            out.push(Value::Number(i as f64));
                            out.push(Value::Number(j as f64));
                        }
                    }
                }
                Ok(Value::List(Rc::new(out)))
            },
            // tm_wave_grid(tex, x0, z0, span, n, t, waves, ybase, sign, tint, alpha, blend, u0, v0, ru, rv)
            //   An n x n textured grid over [x0, x0 + span] x [z0, z0 + span] whose vertices ride
            //   y = ybase + sign * sum(a sin(kx x + kz z + w t + p)) for waves = [kx, kz, w, a, p, ...]:
            //   a sea surface with real wave geometry (fogged like tm_quad, unlit). The texture runs
            //   from (u0, v0) over (ru, rv) across the grid and wraps.
            "tm_wave_grid" => {
                let id = num(args, 0, -1.0);
                let tex = if id < 0.0 { Some(self.g3d.glow_tex()) } else { self.g3d.texs.get(id as usize).cloned() };
                let Some(tex) = tex else { return Ok(Value::Unit) };
                let (x0, z0, span) = (f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 64.0));
                let n = (num(args, 4, 32.0) as usize).clamp(1, 128);
                let t = f(args, 5, 0.0);
                let waves: Vec<f32> = match args.get(6) {
                    Some(Value::List(l)) => l.iter().map(|x| if let Value::Number(n) = x { *n as f32 } else { 0.0 }).collect(),
                    _ => Vec::new(),
                };
                let (ybase, sign) = (f(args, 7, 0.0), f(args, 8, -1.0));
                let tint = rgb(num(args, 9, 16777215.0));
                let alpha = f(args, 10, 1.0);
                let blend = num(args, 11, 0.0) as u8;
                let (u0, v0, ru, rv) = (f(args, 12, 0.0), f(args, 13, 0.0), f(args, 14, 1.0), f(args, 15, 1.0));
                let h = |x: f32, z: f32| -> f32 {
                    let mut s = 0.0;
                    for w in waves.chunks_exact(5) {
                        s += w[3] * (w[0] * x + w[1] * z + w[2] * t + w[4]).sin();
                    }
                    ybase + sign * s
                };
                let cell = span / n as f32;
                let mut grid = vec![[0.0f32; 3]; (n + 1) * (n + 1)];
                let mut uvs = vec![[0.0f32; 2]; (n + 1) * (n + 1)];
                for j in 0..=n {
                    for i in 0..=n {
                        let (x, z) = (x0 + i as f32 * cell, z0 + j as f32 * cell);
                        grid[j * (n + 1) + i] = [x, h(x, z), z];
                        uvs[j * (n + 1) + i] = [u0 + ru * i as f32 / n as f32, v0 + rv * j as f32 / n as f32];
                    }
                }
                let mut gfx = self.gfx.borrow_mut();
                let fog = TexShade { amb: 1.0, lights: Vec::new(), fog_start: gfx.fog_start, fog_end: gfx.fog_end, fog_color: gfx.fog_color, lit: false, ripples: Vec::new() };
                let shade = if fog.fog_end > 0.0 { Some(&fog) } else { None };
                let (pm, pa) = (gfx.blend, gfx.alpha);
                gfx.depth_queue.set_state(blend, alpha);
                for j in 0..n {
                    for i in 0..n {
                        let a = j * (n + 1) + i;
                        let (b, c, d) = (a + 1, a + n + 2, a + n + 1);
                        Self::g3d_push_poly(&mut gfx, [grid[a], grid[b], grid[c]], [uvs[a], uvs[b], uvs[c]], &tex, tint, [0.0; 4], false, 0, shade);
                        Self::g3d_push_poly(&mut gfx, [grid[a], grid[c], grid[d]], [uvs[a], uvs[c], uvs[d]], &tex, tint, [0.0; 4], false, 0, shade);
                    }
                }
                gfx.depth_queue.set_state(pm, pa);
                Ok(Value::Unit)
            },
            // tm_field_tex(id, size, x0, z0, span, t, wells, contrast=1.35, gamma=0.85, bands=9) -> id
            //   A thermal map of a gravity field's cross section, written into texture `id` (a new
            //   one when id < 0): texel (i, j) is the world point (x0 + span (i + .5) / size,
            //   z0 + span (j + .5) / size). The potential is a slow level field plus every well in
            //   `wells` = [x, z, mass, radius, ...] (mass r^2 / (d^2 + r^2); negative mass is a
            //   hill). Mapped through a ROYGBIV thermal ramp (contrast, gamma) with bright
            //   equipotential contour lines every 1 / bands.
            "tm_field_tex" => {
                let id = num(args, 0, -1.0);
                let n = pow2((num(args, 1, 128.0) as u32).clamp(8, 512));
                let (x0, z0, span, t) = (f(args, 2, 0.0), f(args, 3, 0.0), f(args, 4, 64.0), f(args, 5, 0.0));
                let wells: Vec<f32> = match args.get(6) {
                    Some(Value::List(l)) => l.iter().map(|x| if let Value::Number(n) = x { *n as f32 } else { 0.0 }).collect(),
                    _ => Vec::new(),
                };
                let (contrast, gamma, bands) = (f(args, 7, 1.35), f(args, 8, 0.85).max(0.05), f(args, 9, 9.0).max(1.0));
                // lights = [x, z, radius, colour, ...]: vivid glows painted over the map; base_alpha =
                // how opaque the bare map is (the glows and contour lines add to it)
                let lights: Vec<f32> = match args.get(10) {
                    Some(Value::List(l)) => l.iter().map(|x| if let Value::Number(n) = x { *n as f32 } else { 0.0 }).collect(),
                    _ => Vec::new(),
                };
                let base_alpha = f(args, 11, 1.0).clamp(0.0, 1.0);
                const RAMP: [[f32; 3]; 9] = [
                    [0.0, 0.0, 0.05], [0.28, 0.0, 0.5], [0.0, 0.2, 1.0], [0.0, 0.9, 1.0], [0.1, 1.0, 0.3],
                    [1.0, 1.0, 0.0], [1.0, 0.5, 0.0], [1.0, 0.0, 0.12], [1.0, 1.0, 1.0],
                ];
                let mut px = vec![0u32; (n * n) as usize];
                let k = span / n as f32;
                for j in 0..n {
                    let z = z0 + (j as f32 + 0.5) * k;
                    for i in 0..n {
                        let x = x0 + (i as f32 + 0.5) * k;
                        let mut phi = 0.5
                            + 0.22 * (x * 0.018 + z * 0.021 + t * 0.07).sin() * (z * 0.017 - x * 0.006 + t * 0.05).cos()
                            + 0.1 * ((x + z) * 0.043 + t * 0.11).sin()
                            + 0.05 * (x * 0.11 - t * 0.3).sin() * (z * 0.09 + t * 0.23).sin();
                        for w in wells.chunks_exact(4) {
                            let (dx, dz, r2) = (x - w[0], z - w[1], (w[3] * w[3]).max(0.01));
                            phi += w[2] * r2 / (dx * dx + dz * dz + r2);
                        }
                        let d = (((phi - 0.5) * contrast + 0.5).clamp(0.0, 1.0)).powf(gamma);
                        let d = d * d * (3.0 - 2.0 * d);
                        let u = d * 7.999;
                        let (a, b, fr) = (RAMP[u as usize], RAMP[u as usize + 1], u.fract());
                        let mut c = [a[0] + (b[0] - a[0]) * fr, a[1] + (b[1] - a[1]) * fr, a[2] + (b[2] - a[2]) * fr];
                        // the cross-section slices: thin bright lines on the equipotentials
                        let q = (phi * bands).rem_euclid(1.0);
                        let line = (1.0 - (q.min(1.0 - q) / 0.09)).clamp(0.0, 1.0);
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
                        px[(j * n + i) as usize] = ((al * 255.0) as u32) << 24 | ((c[0] * 255.0) as u32) << 16 | ((c[1] * 255.0) as u32) << 8 | (c[2] * 255.0) as u32;
                    }
                }
                let tex = Arc::new(TexData { w: n, h: n, px });
                if id >= 0.0 && (id as usize) < self.g3d.texs.len() {
                    self.g3d.texs[id as usize] = tex;
                    Ok(Value::Number(id))
                } else {
                    self.g3d.texs.push(tex);
                    Ok(Value::Number((self.g3d.texs.len() - 1) as f64))
                }
            },
            // tm_screen_lens(k): a hyperbolic lens over the frame (the Poincare disk's look): the
            //   middle swells, the edges crowd in, the corners stay put. Output radius r (0 at the
            //   centre, 1 at a corner) shows the frame at atanh(r tanh k) / k. 0 = off.
            "tm_screen_lens" => {
                let k = f(args, 0, 0.0).clamp(0.0, 3.0);
                if k < 0.01 {
                    return Ok(Value::Unit);
                }
                let mut gfx = self.gfx.borrow_mut();
                let (w, h) = (gfx.width, gfx.height);
                if gfx.buffer.len() < w * h || w < 4 || h < 4 {
                    return Ok(Value::Unit);
                }
                let src = &mut self.g3d.lens;
                src.clear();
                src.extend_from_slice(&gfx.buffer[..w * h]);
                let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
                let rmax = (cx * cx + cy * cy).sqrt();
                let tk = k.tanh();
                let src: &Vec<u32> = src;
                use rayon::prelude::*;
                gfx.buffer[..w * h].par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                    let dy = y as f32 + 0.5 - cy;
                    for (x, px) in row.iter_mut().enumerate() {
                        let dx = x as f32 + 0.5 - cx;
                        let r = (dx * dx + dy * dy).sqrt() / rmax;
                        if r < 1e-4 {
                            *px = src[y * w + x];
                            continue;
                        }
                        let rs = ((r * tk).clamp(0.0, 0.999_999)).atanh() / k;
                        let s = rs / r;
                        let sx = (cx + dx * s) as i32;
                        let sy = (cy + dy * s) as i32;
                        *px = if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h { src[sy as usize * w + sx as usize] } else { 0 };
                    }
                });
                Ok(Value::Unit)
            },
            // tm_screen_trail(k): blend the framebuffer with the last trailed frame (k 0..0.95), a
            //   smeared, syrupy persistence; call after the 3-D flush, before the UI.
            "tm_screen_trail" => {
                let k = f(args, 0, 0.5).clamp(0.0, 0.95);
                let mut gfx = self.gfx.borrow_mut();
                let n = gfx.width * gfx.height;
                if gfx.buffer.len() < n {
                    return Ok(Value::Unit);
                }
                let tr = &mut self.g3d.trail;
                if tr.len() != n || k <= 0.0 {
                    tr.clear();
                    tr.extend_from_slice(&gfx.buffer[..n]);
                    return Ok(Value::Unit);
                }
                let kb = (k * 256.0) as u32;
                let ka = 256 - kb;
                for (c, p) in gfx.buffer[..n].iter_mut().zip(tr.iter_mut()) {
                    let (a, b) = (*c, *p);
                    let rb = (((a & 0xFF00FF) * ka + (b & 0xFF00FF) * kb) >> 8) & 0xFF00FF;
                    let g = (((a & 0x00FF00) * ka + (b & 0x00FF00) * kb) >> 8) & 0x00FF00;
                    let o = (a & 0xFF00_0000) | rb | g;
                    *c = o;
                    *p = o;
                }
                Ok(Value::Unit)
            },
            // str_to_hex(s) / hex_to_str(h): text <-> hex (the p2p_* payloads are hex)
            "str_to_hex" => Ok(Value::Str(text(args, 0).bytes().map(|b| format!("{b:02x}")).collect())),
            "hex_to_str" => {
                let h = text(args, 0);
                let b: Vec<u8> = (0..h.len() / 2).filter_map(|i| u8::from_str_radix(h.get(i * 2..i * 2 + 2)?, 16).ok()).collect();
                Ok(Value::Str(String::from_utf8_lossy(&b).into_owned()))
            },
            // net_local_ip() -> this machine's LAN address ("" offline): the route a datagram would take
            "net_local_ip" => {
                let ip = std::net::UdpSocket::bind("0.0.0.0:0")
                    .and_then(|s| s.connect("8.8.8.8:80").map(|_| s))
                    .and_then(|s| s.local_addr())
                    .map(|a| a.ip().to_string())
                    .unwrap_or_default();
                Ok(Value::Str(ip))
            },
            // random_hex(bytes): bytes of OS randomness as hex (keys and seeds; rand() is not for secrets)
            "random_hex" => {
                use rand::RngCore;
                let n = (num(args, 0, 32.0) as usize).clamp(1, 256);
                let mut b = vec![0u8; n];
                rand::rngs::OsRng.fill_bytes(&mut b);
                Ok(Value::Str(b.iter().map(|x| format!("{x:02x}")).collect()))
            },
            // rand_seed(n): reseed rand(), so two machines can build the same world
            "rand_seed" => {
                self.rand_state = (num(args, 0, 1.0) as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
                Ok(Value::Unit)
            },
            // clipboard_set(s) / clipboard_get(): the system clipboard (through the OS's own tools)
            "clipboard_set" => {
                let s = text(args, 0);
                Ok(Value::Bool(clipboard_set(&s)))
            },
            "clipboard_get" => Ok(Value::Str(clipboard_get())),
            // tm_lens_guard(d): skip solids, quads and 3-D triangles with a corner within d of the
            //   eye (0 = off); the sea grid is exempt (its cells under the camera reach behind it)
            "tm_lens_guard" => {
                self.gfx.borrow_mut().lens_guard = f(args, 0, 0.0).max(0.0);
                Ok(Value::Unit)
            },
            // tm_sys_os() -> "windows" | "linux" | "macos" | ...; tm_sys_arch() -> "x86_64" | "aarch64" | ...
            "tm_sys_os" => Ok(Value::Str(std::env::consts::OS.into())),
            "tm_sys_arch" => Ok(Value::Str(std::env::consts::ARCH.into())),
            // tm_sys_run(script) - run a build / run script in a terminal window the user can watch:
            //   a .bat in its own console on Windows, Terminal on macOS, on Linux (a Raspberry Pi
            //   desktop included) the first terminal found, else `sh script` in the background with
            //   its output in build.log next to it. The script's folder is the working directory.
            //   -> true when something was started.
            #[cfg(not(target_arch = "wasm32"))]
            "tm_sys_run" => {
                let script = text(args, 0);
                let path = std::path::Path::new(&script);
                let dir = path.parent().map(|d| d.to_path_buf()).unwrap_or_default();
                let started = sys_run_script(path, &dir);
                Ok(Value::Bool(started))
            },
            // tm_tex_load(path, size=256) -> id or -1
            "tm_tex_load" => {
                let path = self.g3d_path(&text(args, 0));
                match load_texture(&path, f(args, 1, 256.0) as u32) {
                    Ok(t) => {
                        self.g3d.texs.push(Arc::new(t));
                        Ok(Value::Number((self.g3d.texs.len() - 1) as f64))
                    },
                    Err(e) => {
                        eprintln!("tm_tex_load: {e}");
                        Ok(Value::Number(-1.0))
                    },
                }
            },
            // tm_fov(horizontal degrees, viewport width px = window width): sets the focal
            // length so the view spans that angle; 0 restores the default (focal = height)
            "tm_fov" => {
                let mut gfx = self.gfx.borrow_mut();
                let deg = f(args, 0, 0.0);
                let vw = f(args, 1, gfx.width as f32);
                gfx.camera.focal = if deg > 1.0 && deg < 179.0 {
                    (vw * 0.5) / (deg.to_radians() * 0.5).tan()
                } else {
                    gfx.height as f32
                };
                Ok(Value::Unit)
            },
            // tm_voice(voice, vol, pitch): retune / re-level a playing sample voice
            "tm_voice" => {
                if let Some(a) = &self.audio {
                    a.set_sample_voice(num(args, 0, 0.0) as u32, f(args, 1, -1.0), f(args, 2, -1.0));
                }
                Ok(Value::Unit)
            },
            "tm_tex_count" => Ok(Value::Number(self.g3d.texs.len() as f64)),
            // tm_load(path) -> mesh id or -1
            "tm_load" => {
                let path = self.g3d_path(&text(args, 0));
                match load_ltm(&path) {
                    Ok(m) => {
                        self.g3d.meshes.push(m);
                        Ok(Value::Number((self.g3d.meshes.len() - 1) as f64))
                    },
                    Err(e) => {
                        eprintln!("tm_load: {e}");
                        Ok(Value::Number(-1.0))
                    },
                }
            },
            "tm_groups" => {
                let n = self.g3d.meshes.get(num(args, 0, -1.0) as usize).map(|m| m.groups.len()).unwrap_or(0);
                Ok(Value::Number(n as f64))
            },
            // tm_group(mesh, name) -> index of the first group whose name contains `name`, or -1
            "tm_group" => {
                let key = text(args, 1);
                let r = self
                    .g3d
                    .meshes
                    .get(num(args, 0, -1.0) as usize)
                    .and_then(|m| m.groups.iter().position(|g| g.name == key).or_else(|| m.groups.iter().position(|g| g.name.contains(&key))));
                Ok(Value::Number(r.map(|i| i as f64).unwrap_or(-1.0)))
            },
            "tm_group_name" => {
                let r = self
                    .g3d
                    .meshes
                    .get(num(args, 0, -1.0) as usize)
                    .and_then(|m| m.groups.get(num(args, 1, -1.0) as usize))
                    .map(|g| g.name.clone())
                    .unwrap_or_default();
                Ok(Value::Str(r.into()))
            },
            "tm_group_tris" => {
                let r = self
                    .g3d
                    .meshes
                    .get(num(args, 0, -1.0) as usize)
                    .and_then(|m| m.groups.get(num(args, 1, -1.0) as usize))
                    .map(|g| g.idx.len())
                    .unwrap_or(0);
                Ok(Value::Number(r as f64))
            },
            // tm_bounds(mesh, group=-1) -> [minx, miny, minz, maxx, maxy, maxz]
            "tm_bounds" => {
                let mut lo = [f32::INFINITY; 3];
                let mut hi = [f32::NEG_INFINITY; 3];
                if let Some(m) = self.g3d.meshes.get(num(args, 0, -1.0) as usize) {
                    let gsel = num(args, 1, -1.0);
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
                Ok(nums(&[lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]]))
            },
            // tm_edges(mesh, group=-1, max=4000) -> flat [x0,y0,z0, x1,y1,z1, ...]: the outline
            // edges (used by one triangle only) plus sharp creases, for minimaps / wireframes
            "tm_edges" => {
                let mut out: Vec<f32> = Vec::new();
                if let Some(m) = self.g3d.meshes.get(num(args, 0, -1.0).max(0.0) as usize).filter(|_| num(args, 0, -1.0) >= 0.0) {
                    let gsel = num(args, 1, -1.0);
                    let maxe = num(args, 2, 4000.0).max(1.0) as usize;
                    let q = |p: [f32; 3]| ((p[0] * 20.0).round() as i64, (p[1] * 20.0).round() as i64, (p[2] * 20.0).round() as i64);
                    let mut count: HashMap<((i64, i64, i64), (i64, i64, i64)), (u32, [f32; 3], [f32; 3])> = HashMap::new();
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
                                let e = count.entry(key).or_insert((0, a, b));
                                e.0 += 1;
                            }
                        }
                    }
                    for (_, (n, a, b)) in count {
                        if n == 1 && out.len() / 6 < maxe {
                            out.extend_from_slice(&[a[0], a[1], a[2], b[0], b[1], b[2]]);
                        }
                    }
                }
                Ok(nums(&out))
            },
            // tm_wall_spots(mesh, group, spacing, up) -> flat [x, y, z, nx, ny, nz, ...]: points on the
            // vertical faces of a group at least `spacing` apart (torches along corridors). y is
            // `up` units above the face's bottom edge (+y is down); n is the face normal.
            "tm_wall_spots" => {
                let mut out: Vec<f32> = Vec::new();
                let mi = num(args, 0, -1.0);
                if mi >= 0.0 {
                    if let Some(m) = self.g3d.meshes.get(mi as usize) {
                        let gsel = num(args, 1, -1.0);
                        let spacing = f(args, 2, 30.0).max(1.0);
                        let up = f(args, 3, 6.0);
                        let mut chosen: Vec<[f32; 3]> = Vec::new();
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
                                // face normal oriented like the exported vertex normals
                                let vn = g.nrm[t[0] as usize];
                                let n = if dot(fnrm, vn) < 0.0 { [-fnrm[0], -fnrm[1], -fnrm[2]] } else { fnrm };
                                let bottom = a[1].max(b[1]).max(c[1]);
                                let top = a[1].min(b[1]).min(c[1]);
                                if bottom - top < up * 1.2 {
                                    continue;
                                }
                                let p = [(a[0] + b[0] + c[0]) / 3.0, bottom - up, (a[2] + b[2] + c[2]) / 3.0];
                                if chosen.iter().any(|q| {
                                    let d = sub(*q, p);
                                    d[0] * d[0] + d[2] * d[2] < spacing * spacing && (d[1]).abs() < spacing
                                }) {
                                    continue;
                                }
                                chosen.push(p);
                                out.extend_from_slice(&[p[0], p[1], p[2], n[0], n[1], n[2]]);
                            }
                        }
                    }
                }
                Ok(nums(&out))
            },
            // tm_hole(x, y, z, radius): soft see-through circle around a world point for
            // the tm_draw surfaces in front of it; radius 0 turns it off
            "tm_hole" => {
                self.g3d.hole = [f(args, 0, 0.0), f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)];
                Ok(Value::Unit)
            },
            // tm_draw(mesh, group, tex, x, y, z, scale, rot, tint, alpha, uoff, voff, uscale,
            //         vscale, shade, blend, cull) -> triangles queued
            //   rot: yaw angle or a row-major 3x3 list; tint 0xRRGGBB; shade 0..1 darkens faces
            //   turned from the light; blend 0 normal 1 additive; cull 0 none, 1 / 2 one winding
            "tm_draw" => Ok(Value::Number(self.tm_draw(args) as f64)),
            // tm_sprite(tex, cx, cy, w, h, rot, tint, alpha, blend): 2-D rotated textured quad
            // drawn straight onto the framebuffer (use after flush_3d)
            "tm_sprite" => {
                self.tm_sprite(args);
                Ok(Value::Unit)
            },
            // tm_billboard(tex, x, y, z, size, rot, tint, alpha, blend): camera-facing quad
            "tm_billboard" => {
                let id = num(args, 0, -1.0);
                let tex = if id < 0.0 { Some(self.g3d.glow_tex()) } else { self.g3d.texs.get(id as usize).cloned() };
                if let Some(tex) = tex {
                    let p = [f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)];
                    let size = f(args, 4, 1.0);
                    let mut alpha = f(args, 7, 1.0);
                    // under the lens guard a glow fades out as the eye comes within its own size and
                    // is skipped on top of the lens (its flat core would wash over the whole view)
                    {
                        let g = self.gfx.borrow();
                        if g.lens_guard > 0.0 {
                            let eye = g.camera.depth(p[0], p[1], p[2]) + g.camera.zdist;
                            let k = ((eye - g.lens_guard - size * 0.35) / (size * 0.9 + 0.5)).clamp(0.0, 1.0);
                            if k <= 0.02 {
                                return Ok(Value::Unit);
                            }
                            alpha *= k;
                        }
                    }
                    self.g3d_billboard(&tex, p, size, f(args, 5, 0.0), rgb(num(args, 6, 16777215.0)), alpha, num(args, 8, 0.0) as u8);
                }
                Ok(Value::Unit)
            },

            // tm_wire(kind, x, y, z, size, rx, ry, rz, colour, detail=12, thick=0) - a wireframe shape
            //   drawn natively as depth-sorted 3-D lines in the current blend mode (set_blend(1) for
            //   additive neon). kind: 0 cube, 1 tetrahedron, 2 octahedron, 3 icosahedron, 4 torus,
            //   5 sphere (detail rings), 6 ring (detail sides, in the XY plane), 7 star prism,
            //   8 grid (detail x detail in the XZ plane). size = radius; rx, ry, rz in radians.
            //   thick 1 doubles every stroke one pixel over, for a bolder line under bloom.
            "tm_wire" => {
                let kind = num(args, 0, 0.0) as i32;
                let c = [f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)];
                let size = f(args, 4, 1.0);
                let (rx, ry, rz) = (f(args, 5, 0.0), f(args, 6, 0.0), f(args, 7, 0.0));
                let col = rgb(num(args, 8, 16777215.0));
                let detail = (num(args, 9, 12.0) as usize).clamp(3, 64);
                let thick = num(args, 10, 0.0) > 0.5;
                let (verts, edges) = wire_shape(kind, detail);
                let (sx, cx) = rx.sin_cos();
                let (sy, cy) = ry.sin_cos();
                let (sz, cz) = rz.sin_cos();
                let world: Vec<[f32; 3]> = verts
                    .iter()
                    .map(|v| {
                        // Rz * Ry * Rx
                        let (x, y, z) = (v[0], v[1] * cx - v[2] * sx, v[1] * sx + v[2] * cx);
                        let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
                        let (x, y) = (x * cz - y * sz, x * sz + y * cz);
                        [c[0] + x * size, c[1] + y * size, c[2] + z * size]
                    })
                    .collect();
                let mut gfx = self.gfx.borrow_mut();
                for (i, j) in edges {
                    wire_line(&mut gfx, world[i], world[j], col, thick);
                }
                Ok(Value::Unit)
            },
            // tm_solid(kind, x, y, z, size, rx, ry, rz, colour, alpha=1, blend=0, stretch=1) - a faceted,
            //   flat-shaded low-poly solid: every face gets its own tone from a fixed key light (the
            //   crystal / shard look). kind: 0 cube, 1 tetrahedron, 2 octahedron, 3 icosahedron,
            //   9 shard (an octahedron stretched along its local y by `stretch`). alpha < 1 is
            //   translucent; blend 1 adds (glowing glass).
            "tm_solid" => {
                let kind = num(args, 0, 3.0) as i32;
                let c = [f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)];
                let size = f(args, 4, 1.0);
                let (rx, ry, rz) = (f(args, 5, 0.0), f(args, 6, 0.0), f(args, 7, 0.0));
                let col = rgb(num(args, 8, 16777215.0));
                let alpha = f(args, 9, 1.0);
                let blend = num(args, 10, 0.0) as u8;
                let stretch = f(args, 11, if kind == 9 { 2.4 } else { 1.0 });
                let (verts, faces) = solid_shape(kind);
                let (sx, cx) = rx.sin_cos();
                let (sy, cy) = ry.sin_cos();
                let (sz, cz) = rz.sin_cos();
                let world: Vec<[f32; 3]> = verts
                    .iter()
                    .map(|v| {
                        let (x, y, z) = (v[0], v[1] * stretch, v[2]);
                        let (x, y, z) = (x, y * cx - z * sx, y * sx + z * cx);
                        let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
                        let (x, y) = (x * cz - y * sz, x * sz + y * cz);
                        [c[0] + x * size, c[1] + y * size, c[2] + z * size]
                    })
                    .collect();
                let mut gfx = self.gfx.borrow_mut();
                let near = -gfx.camera.zdist + 0.05;
                let (pm, pa) = (gfx.blend, gfx.alpha);
                gfx.depth_queue.set_state(blend, alpha);
                // key light from up-left-front (y is down), a little ambient
                let l = { let v = [0.45f32, -0.75, -0.5]; let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt(); [v[0] / n, v[1] / n, v[2] / n] };
                for f3 in &faces {
                    let (a, b, d) = (world[f3[0]], world[f3[1]], world[f3[2]]);
                    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                    let v = [d[0] - a[0], d[1] - a[1], d[2] - a[2]];
                    let mut n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
                    let nl = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
                    n = [n[0] / nl, n[1] / nl, n[2] / nl];
                    // face normals point outward from the centre
                    let fc = [(a[0] + b[0] + d[0]) / 3.0 - c[0], (a[1] + b[1] + d[1]) / 3.0 - c[1], (a[2] + b[2] + d[2]) / 3.0 - c[2]];
                    if n[0] * fc[0] + n[1] * fc[1] + n[2] * fc[2] < 0.0 {
                        n = [-n[0], -n[1], -n[2]];
                    }
                    let k = 0.28 + 0.72 * (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.0) + 0.12 * (-n[2]).max(0.0);
                    let shade = |ch: u32| ((ch as f32 * k).min(255.0)) as u32;
                    let fcol = (shade((col >> 16) & 255) << 16) | (shade((col >> 8) & 255) << 8) | shade(col & 255);
                    let da = gfx.camera.depth(a[0], a[1], a[2]);
                    let db = gfx.camera.depth(b[0], b[1], b[2]);
                    let dd = gfx.camera.depth(d[0], d[1], d[2]);
                    let lg = near + gfx.lens_guard;
                    if da <= lg || db <= lg || dd <= lg {
                        continue;
                    }
                    let (ax, ay, az) = gfx.camera.project(a[0], a[1], a[2]);
                    let (bx, by, bz) = gfx.camera.project(b[0], b[1], b[2]);
                    let (dx, dy, dz) = gfx.camera.project(d[0], d[1], d[2]);
                    let fcol = gfx.fog_apply(fcol, (az + bz + dz) / 3.0);
                    gfx.depth_queue.push_triangle_zv(fcol, ax, ay, az, bx, by, bz, dx, dy, dz);
                }
                gfx.depth_queue.set_state(pm, pa);
                Ok(Value::Unit)
            },
            // tm_ripple(x, z, strength, speed=60, wavelength=5, life=1.8): a shockwave ring spreading
            //   over the floor from (x, z) at `speed` units/s; strength ~0..1.5 is the wave's slope.
            //   Up to 12 live rings (the oldest gives way). Only draws made while tm_ripple_use(1)
            //   show them - the floor, not the walls.
            "tm_ripple" => {
                if self.g3d.ripples.len() >= 12 {
                    self.g3d.ripples.remove(0);
                }
                self.g3d.ripples.push(Ripple {
                    x: f(args, 0, 0.0),
                    z: f(args, 1, 0.0),
                    t0: std::time::Instant::now(),
                    amp: f(args, 2, 1.0).clamp(0.0, 3.0),
                    speed: f(args, 3, 60.0).max(1.0),
                    wl: f(args, 4, 5.0).max(0.5),
                    life: f(args, 5, 1.8).max(0.05),
                });
                Ok(Value::Unit)
            },
            "tm_ripple_use" => {
                self.g3d.ripple_on = num(args, 0, 1.0) > 0.5;
                Ok(Value::Unit)
            },
            "tm_ripple_clear" => {
                self.g3d.ripples.clear();
                Ok(Value::Unit)
            },
            // tm_glyph(font, "c", px=256) -> [tex, span]: the glyph filled solid (non-zero winding,
            //   anti-aliased) into a px x px white texture whose alpha is the coverage, centred on
            //   the glyph's ink. `span` is the square's side in ems, so a tm_quad with half-extents
            //   span * size / 2 draws the letter at `size` world units per em. -1 when the font or
            //   glyph has no outline (a space).
            #[cfg(not(target_arch = "wasm32"))]
            "tm_glyph" => {
                let fid = num(args, 0, -1.0);
                let ch = text(args, 1).chars().next().unwrap_or(' ');
                let px = pow2((num(args, 2, 256.0) as u32).clamp(16, 2048));
                if fid < 0.0 || fid as usize >= self.fonts.len() {
                    return Ok(nums(&[-1.0, 0.0]));
                }
                let go = self.font_glyph_fb(fid as usize, ch, 0.002);
                let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for pl in &go.polylines {
                    for p in pl {
                        x0 = x0.min(p[0]);
                        x1 = x1.max(p[0]);
                        y0 = y0.min(p[1]);
                        y1 = y1.max(p[1]);
                    }
                }
                if x1 <= x0 || y1 <= y0 {
                    return Ok(nums(&[-1.0, 0.0]));
                }
                // a square around the ink with a 6% margin; glyph y is up, texture rows go down
                let span = (x1 - x0).max(y1 - y0) * 1.12;
                let (mx, my) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
                let k = px as f32 / span;
                let mut pb = resvg::tiny_skia::PathBuilder::new();
                for pl in &go.polylines {
                    if pl.len() < 3 {
                        continue;
                    }
                    for (i, p) in pl.iter().enumerate() {
                        let (x, y) = ((p[0] - mx) * k + px as f32 * 0.5, (my - p[1]) * k + px as f32 * 0.5);
                        if i == 0 {
                            pb.move_to(x, y);
                        } else {
                            pb.line_to(x, y);
                        }
                    }
                    pb.close();
                }
                let (Some(path), Some(mut pm)) = (pb.finish(), resvg::tiny_skia::Pixmap::new(px, px)) else {
                    return Ok(nums(&[-1.0, 0.0]));
                };
                let mut paint = resvg::tiny_skia::Paint::default();
                paint.set_color_rgba8(255, 255, 255, 255);
                paint.anti_alias = true;
                pm.fill_path(&path, &paint, resvg::tiny_skia::FillRule::Winding, resvg::tiny_skia::Transform::identity(), None);
                let data: Vec<u32> = pm.pixels().iter().map(|c| ((c.alpha() as u32) << 24) | 0x00FF_FFFF).collect();
                self.g3d.texs.push(Arc::new(TexData { w: px, h: px, px: data }));
                Ok(nums(&[(self.g3d.texs.len() - 1) as f32, span]))
            },
            // tm_quad(tex, x, y, z, ux, uy, uz, vx, vy, vz, tint, alpha, blend, bias=0, u0=0, v0=0, ru=1, rv=1) - a textured quad in
            //   any orientation: centre (x, y, z), half-extent vectors u and v (texture u runs along
            //   u, v along v). tex -1 = the glow dot, -2 = a soft round shadow (tint 0 = black).
            //   Depth-tested and fogged like the meshes; for decals such as a blob shadow laid on
            //   the ground along its normal. `bias` (world units) sorts it that much nearer, so a
            //   decal draws after the large translucent triangles under it.
            "tm_quad" => {
                let id = num(args, 0, -1.0);
                let tex = if id <= -1.5 {
                    Some(soft_tex())
                } else if id < 0.0 {
                    Some(self.g3d.glow_tex())
                } else {
                    self.g3d.texs.get(id as usize).cloned()
                };
                if let Some(tex) = tex {
                    let c = [f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)];
                    let u = [f(args, 4, 1.0), f(args, 5, 0.0), f(args, 6, 0.0)];
                    let v = [f(args, 7, 0.0), f(args, 8, 0.0), f(args, 9, 1.0)];
                    let tint = rgb(num(args, 10, 16777215.0));
                    let alpha = f(args, 11, 1.0);
                    let blend = num(args, 12, 0.0) as u8;
                    let at = |a: f32, b: f32| [c[0] + u[0] * a + v[0] * b, c[1] + u[1] * a + v[1] * b, c[2] + u[2] * a + v[2] * b];
                    let q = [at(-1.0, -1.0), at(1.0, -1.0), at(1.0, 1.0), at(-1.0, 1.0)];
                    {
                        let g = self.gfx.borrow();
                        if g.lens_guard > 0.0 {
                            let lg = -g.camera.zdist + 0.05 + g.lens_guard;
                            if q.iter().any(|p| g.camera.depth(p[0], p[1], p[2]) < lg) {
                                return Ok(Value::Unit);
                            }
                        }
                    }
                    // optional uv rect (u0, v0, repeat u, repeat v): the texture fetch wraps
                    let (u0, v0) = (f(args, 14, 0.0), f(args, 15, 0.0));
                    let (us, vs) = (f(args, 16, 0.999), f(args, 17, 0.999));
                    let t = [[u0, v0], [u0 + us, v0], [u0 + us, v0 + vs], [u0, v0 + vs]];
                    let mut gfx = self.gfx.borrow_mut();
                    let fog = TexShade { amb: 1.0, lights: Vec::new(), fog_start: gfx.fog_start, fog_end: gfx.fog_end, fog_color: gfx.fog_color, lit: false, ripples: Vec::new() };
                    let shade = if fog.fog_end > 0.0 { Some(&fog) } else { None };
                    let (pm, pa) = (gfx.blend, gfx.alpha);
                    gfx.depth_queue.set_state(blend, alpha);
                    let n = Self::g3d_push_poly(&mut gfx, [q[0], q[1], q[2]], [t[0], t[1], t[2]], &tex, tint, [0.0; 4], false, 0, shade)
                        + Self::g3d_push_poly(&mut gfx, [q[0], q[2], q[3]], [t[0], t[2], t[3]], &tex, tint, [0.0; 4], false, 0, shade);
                    let bias = f(args, 13, 0.0);
                    if bias != 0.0 {
                        gfx.depth_queue.bias_last(n, bias);
                    }
                    gfx.depth_queue.set_state(pm, pa);
                }
                Ok(Value::Unit)
            },

            // ── collision ──
            // coll_build(mesh, group=-1, x, y, z, scale, rot, cell=8) -> id
            "coll_build" => Ok(Value::Number(self.coll_build(args))),
            "coll_tris" => Ok(Value::Number(self.g3d.colls.get(num(args, 0, -1.0) as usize).map(|c| c.tris.len()).unwrap_or(0) as f64)),
            // coll_sphere(id, x, y, z, r) -> [x, y, z, nx, ny, nz, contacts, deepest]
            "coll_sphere" => Ok(self.coll_sphere(args)),
            // coll_ray(id, ox, oy, oz, dx, dy, dz, maxt) -> [t, nx, ny, nz] (t = -1: no hit)
            "coll_ray" => Ok(self.coll_ray(args)),

            // ── particles ──
            // pfx_emit(x, y, z, vx, vy, vz, life, size0, size1, col0, col1, kind, tex, grav, alpha)
            //   kind 0 glow dot (additive), 1 streak (additive), 2 sprite (tex, normal blend)
            "pfx_emit" => {
                let pt = Part {
                    p: [f(args, 0, 0.0), f(args, 1, 0.0), f(args, 2, 0.0)],
                    v: [f(args, 3, 0.0), f(args, 4, 0.0), f(args, 5, 0.0)],
                    life: f(args, 6, 1.0),
                    max: f(args, 6, 1.0).max(1e-3),
                    s0: f(args, 7, 0.2),
                    s1: f(args, 8, 0.0),
                    c0: rgb(num(args, 9, 16777215.0)),
                    c1: rgb(num(args, 10, num(args, 9, 16777215.0))),
                    kind: num(args, 11, 0.0) as u8,
                    tex: num(args, 12, 0.0).max(0.0) as u32,
                    grav: f(args, 13, 1.0),
                    spin: 0.0,
                    alpha: f(args, 14, 1.0),
                };
                self.g3d_push(pt);
                Ok(Value::Unit)
            },
            // pfx_burst(x, y, z, n, speed, life, size0, size1, col0, col1, kind, grav, vx, vy, vz)
            //   n particles in random directions (a firework shell), speeds 0.55..1 x speed,
            //   lives 0.6..1 x life, inheriting (vx, vy, vz)
            "pfx_burst" => {
                let n = num(args, 3, 16.0).clamp(0.0, 2000.0) as usize;
                let (sp, life) = (f(args, 4, 5.0), f(args, 5, 1.0));
                let base = [f(args, 12, 0.0), f(args, 13, 0.0), f(args, 14, 0.0)];
                for _ in 0..n {
                    let z = self.g3d.rand() * 2.0 - 1.0;
                    let a = self.g3d.rand() * std::f32::consts::TAU;
                    let r = (1.0 - z * z).max(0.0).sqrt();
                    let s = sp * (0.55 + 0.45 * self.g3d.rand());
                    let l = life * (0.6 + 0.4 * self.g3d.rand());
                    let pt = Part {
                        p: [f(args, 0, 0.0), f(args, 1, 0.0), f(args, 2, 0.0)],
                        v: [base[0] + r * a.cos() * s, base[1] + z * s, base[2] + r * a.sin() * s],
                        life: l,
                        max: l.max(1e-3),
                        s0: f(args, 6, 0.2),
                        s1: f(args, 7, 0.0),
                        c0: rgb(num(args, 8, 16777215.0)),
                        c1: rgb(num(args, 9, num(args, 8, 16777215.0))),
                        kind: num(args, 10, 0.0) as u8,
                        tex: 0,
                        grav: f(args, 11, 1.0),
                        spin: 0.0,
                        alpha: 1.0,
                    };
                    self.g3d_push(pt);
                }
                Ok(Value::Unit)
            },
            // pfx_rain(x0, z0, x1, z1, y, n, vy, life, size0, size1, tex0, ntex, alpha, col)
            //   textured sprites born at random over a box top (falling stars)
            "pfx_rain" => {
                let n = num(args, 5, 1.0).clamp(0.0, 2000.0) as usize;
                let ntex = num(args, 11, 1.0).max(1.0) as u32;
                for _ in 0..n {
                    let x = f(args, 0, 0.0) + (f(args, 2, 0.0) - f(args, 0, 0.0)) * self.g3d.rand();
                    let z = f(args, 1, 0.0) + (f(args, 3, 0.0) - f(args, 1, 0.0)) * self.g3d.rand();
                    let y = f(args, 4, 0.0) + (self.g3d.rand() - 0.5) * 2.0 * f(args, 15, 0.0);
                    let l = f(args, 7, 3.0) * (0.25 + 0.75 * self.g3d.rand());
                    let t = num(args, 10, 0.0).max(0.0) as u32 + ((self.g3d.rand() * ntex as f32) as u32).min(ntex - 1);
                    let spin = (self.g3d.rand() - 0.5) * 2.0;
                    let pt = Part {
                        p: [x, y, z],
                        v: [0.0, f(args, 6, 90.0), 0.0],
                        life: l,
                        max: l.max(1e-3),
                        s0: f(args, 8, 6.0),
                        s1: f(args, 9, 1.0),
                        c0: rgb(num(args, 13, 16777215.0)),
                        c1: rgb(num(args, 13, 16777215.0)),
                        kind: 2,
                        tex: t,
                        grav: 0.0,
                        spin,
                        alpha: f(args, 12, 0.5),
                    };
                    self.g3d_push(pt);
                }
                Ok(Value::Unit)
            },
            // pfx_step(dt, gravity=9.8, drag=0, floor_y=1e9, bounce=0.4)  (+y is down)
            "pfx_step" => {
                let dt = f(args, 0, 1.0 / 60.0);
                let g = f(args, 1, 9.8);
                let drag = (1.0 - f(args, 2, 0.0) * dt).clamp(0.0, 1.0);
                let floor = f(args, 3, 1e9);
                let bounce = f(args, 4, 0.4);
                self.g3d.parts.retain_mut(|p| {
                    p.life -= dt;
                    if p.life <= 0.0 {
                        return false;
                    }
                    p.v[1] += g * p.grav * dt;
                    for k in 0..3 {
                        p.v[k] *= drag;
                        p.p[k] += p.v[k] * dt;
                    }
                    if p.p[1] > floor {
                        p.p[1] = floor;
                        p.v[1] = -p.v[1].abs() * bounce;
                        p.v[0] *= 0.7;
                        p.v[2] *= 0.7;
                    }
                    true
                });
                Ok(Value::Unit)
            },
            "pfx_draw" => {
                self.pfx_draw();
                Ok(Value::Unit)
            },
            "pfx_clear" => {
                self.g3d.parts.clear();
                Ok(Value::Unit)
            },
            "pfx_count" => Ok(Value::Number(self.g3d.parts.len() as f64)),
            "pfx_max" => {
                self.g3d.max_parts = num(args, 0, 6000.0).clamp(16.0, 100000.0) as usize;
                Ok(Value::Unit)
            },
            _ => Err(format!("unknown game3d builtin '{name}'").into()),
        }
    }

    fn g3d_path(&self, p: &str) -> String {
        if std::path::Path::new(p).exists() {
            return p.to_string();
        }
        if let Some(d) = &self.source_dir {
            return d.join(p).to_string_lossy().into_owned();
        }
        p.to_string()
    }

    fn g3d_push(&mut self, p: Part) {
        if self.g3d.parts.len() >= self.g3d.max_parts {
            // drop the oldest-born (front) particle
            self.g3d.parts.remove(0);
        }
        self.g3d.parts.push(p);
    }

    /// Clip a polygon of (world pos, uv) against the near plane and queue it.
    #[allow(clippy::too_many_arguments)]
    fn g3d_push_poly(
        gfx: &mut crate::gfx::GfxState,
        wp: [[f32; 3]; 3],
        uv: [[f32; 2]; 3],
        tex: &Arc<TexData>,
        tint: u32,
        hole: [f32; 4],
        zwrite: bool,
        cull: u8,
        light: Option<&TexShade>,
    ) -> usize {
        let near = -gfx.camera.zdist + 0.05;
        let d = [
            gfx.camera.depth(wp[0][0], wp[0][1], wp[0][2]),
            gfx.camera.depth(wp[1][0], wp[1][1], wp[1][2]),
            gfx.camera.depth(wp[2][0], wp[2][1], wp[2][2]),
        ];
        if d[0] <= near && d[1] <= near && d[2] <= near {
            return 0;
        }
        let mut cp: [([f32; 3], [f32; 2]); 4] = [([0.0; 3], [0.0; 2]); 4];
        let mut cn = 0usize;
        for i in 0..3 {
            let j = (i + 1) % 3;
            let ain = d[i] > near;
            let bin = d[j] > near;
            if ain && cn < 4 {
                cp[cn] = (wp[i], uv[i]);
                cn += 1;
            }
            if ain != bin && cn < 4 {
                let t = (near - d[i]) / (d[j] - d[i]);
                cp[cn] = (
                    [
                        wp[i][0] + (wp[j][0] - wp[i][0]) * t,
                        wp[i][1] + (wp[j][1] - wp[i][1]) * t,
                        wp[i][2] + (wp[j][2] - wp[i][2]) * t,
                    ],
                    [uv[i][0] + (uv[j][0] - uv[i][0]) * t, uv[i][1] + (uv[j][1] - uv[i][1]) * t],
                );
                cn += 1;
            }
        }
        if cn < 3 {
            return 0;
        }
        let zd = gfx.camera.zdist;
        let mut tv = [TexVert::default(); 4];
        for k in 0..cn {
            let (p, t) = cp[k];
            let (sx, sy, z) = gfx.camera.project(p[0], p[1], p[2]);
            let iw = 1.0 / (z + zd).max(1e-4);
            tv[k] = TexVert { x: sx, y: sy, z, iw, uw: t[0] * iw, vw: t[1] * iw, wx: p[0] * iw, wy: p[1] * iw, wz: p[2] * iw };
        }
        if cull != 0 {
            let area = (tv[1].x - tv[0].x) * (tv[2].y - tv[0].y) - (tv[1].y - tv[0].y) * (tv[2].x - tv[0].x);
            if (cull == 1 && area > 0.0) || (cull == 2 && area < 0.0) {
                return 0;
            }
        }
        // the lights that reach this triangle (bounding sphere vs light radius)
        let shade = light.map(|base| {
            let c = [(wp[0][0] + wp[1][0] + wp[2][0]) / 3.0, (wp[0][1] + wp[1][1] + wp[2][1]) / 3.0, (wp[0][2] + wp[1][2] + wp[2][2]) / 3.0];
            let mut rad = 0.0f32;
            for q in &wp {
                rad = rad.max(dot(sub(*q, c), sub(*q, c)).sqrt());
            }
            let mut sh = TexShade { amb: base.amb, lights: Vec::new(), fog_start: base.fog_start, fog_end: base.fog_end, fog_color: base.fog_color, lit: base.lit, ripples: Vec::new() };
            for r in &base.ripples {
                let d = ((c[0] - r[0]) * (c[0] - r[0]) + (c[2] - r[1]) * (c[2] - r[1])).sqrt();
                if (d - r[2]).abs() < rad + r[5] * 3.0 {
                    sh.ripples.push(*r);
                }
            }
            if base.lit {
                for l in &base.lights {
                    let d = sub([l[0], l[1], l[2]], c);
                    let reach = rad + l[7];
                    if dot(d, d) < reach * reach {
                        sh.lights.push(*l);
                    }
                }
            }
            Arc::new(sh)
        });
        let mut n = 0;
        for j in 1..cn - 1 {
            gfx.depth_queue.push_triangle_tex_shaded([tv[0], tv[j], tv[j + 1]], tex.clone(), tint, hole, zwrite, shade.clone());
            n += 1;
        }
        n
    }

    fn tm_draw(&mut self, args: &[Value]) -> usize {
        let mi = num(args, 0, -1.0);
        let gi = num(args, 1, -1.0);
        let ti = num(args, 2, -1.0);
        if mi < 0.0 || mi as usize >= self.g3d.meshes.len() {
            return 0;
        }
        let tex = if ti < 0.0 { self.g3d.glow_tex() } else {
            match self.g3d.texs.get(ti as usize) {
                Some(t) => t.clone(),
                None => return 0,
            }
        };
        let o = [f(args, 3, 0.0), f(args, 4, 0.0), f(args, 5, 0.0)];
        let sc = f(args, 6, 1.0);
        let m = rot_from(args, 7);
        let tint = rgb(num(args, 8, 16777215.0));
        let alpha = f(args, 9, 1.0).clamp(0.0, 1.0);
        let (uo, vo) = (f(args, 10, 0.0), f(args, 11, 0.0));
        let (us, vs) = (f(args, 12, 1.0), f(args, 13, 1.0));
        let shade = f(args, 14, 0.0).clamp(0.0, 1.0);
        let blend = num(args, 15, 0.0) as u8;
        let cull = num(args, 16, 0.0) as u8;
        let lit_amb = f(args, 17, -1.0);
        let hole_w = self.g3d.hole;
        // the live shockwaves, for draws made while tm_ripple_use(1) is on
        let ripples: Vec<[f32; 6]> = if self.g3d.ripple_on {
            let now = std::time::Instant::now();
            self.g3d.ripples.retain(|r| now.duration_since(r.t0).as_secs_f32() < r.life);
            self.g3d
                .ripples
                .iter()
                .map(|r| {
                    let age = now.duration_since(r.t0).as_secs_f32();
                    let front = r.speed * age;
                    let k = 1.0 - age / r.life;
                    [r.x, r.z, front, r.amp * k * k / (1.0 + front * 0.03), r.wl * (1.0 + age * 0.6), r.wl * 0.9 + front * 0.12]
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut gfx = self.gfx.borrow_mut();
        let base = TexShade {
            amb: lit_amb.max(0.0),
            lights: if lit_amb >= 0.0 {
                gfx.lights.iter().map(|l| [l.x, l.y, l.z, l.r, l.g, l.b, l.intensity, if l.radius > 0.0 { l.radius } else { 1.0e6 }]).collect()
            } else {
                Vec::new()
            },
            fog_start: gfx.fog_start,
            fog_end: gfx.fog_end,
            fog_color: gfx.fog_color,
            lit: lit_amb >= 0.0,
            ripples,
        };
        let shade_opt = if base.lit || base.fog_end > 0.0 || !base.ripples.is_empty() { Some(&base) } else { None };
        let hole = if hole_w[3] > 0.0 {
            let (hx, hy, hd) = gfx.camera.project(hole_w[0], hole_w[1], hole_w[2]);
            let r = gfx.camera.focal * hole_w[3] / (hd + gfx.camera.zdist).max(0.1);
            // only what is clearly in front of the point fades: a margin of a fifth of the radius
            // keeps the ground the point rests on (slopes included) out of the circle
            [hx, hy, r, hd - hole_w[3] * 0.2]
        } else {
            [0.0; 4]
        };
        let (pm, pa) = (gfx.blend, gfx.alpha);
        gfx.depth_queue.set_state(blend, alpha);
        let zwrite = alpha > 0.97 && blend == 0;
        let light = norm([0.35, -0.8, -0.45]);
        let mut count = 0;
        let mesh = &self.g3d.meshes[mi as usize];
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
                    let n = if dot(vn, vn) > 0.25 { norm(vn) } else { fnrm };
                    let l = dot(n, light).abs();
                    let k = 1.0 - shade * (1.0 - (0.35 + 0.65 * l));
                    lerp_rgb(0, tint, k)
                } else {
                    tint
                };
                count += Self::g3d_push_poly(&mut gfx, wp, uv, &tex, tt, hole, zwrite, cull, shade_opt);
            }
        }
        gfx.depth_queue.set_state(pm, pa);
        count
    }

    #[allow(clippy::too_many_arguments)]
    fn g3d_billboard(&mut self, tex: &Arc<TexData>, p: [f32; 3], size: f32, rot: f32, tint: u32, alpha: f32, blend: u8) {
        let mut gfx = self.gfx.borrow_mut();
        let near = -gfx.camera.zdist + 0.05;
        let (sx, sy, z) = gfx.camera.project(p[0], p[1], p[2]);
        if z <= near {
            return;
        }
        let w = z + gfx.camera.zdist;
        let r = gfx.camera.focal * size * 0.5 / w.max(1e-3);
        if r < 0.3 || r > 4000.0 {
            return;
        }
        let tint = if blend == 1 {
            // additive glow fades out into the fog instead of toward its colour
            let fog_k = if gfx.fog_end > gfx.fog_start && gfx.fog_end > 0.0 { ((z - gfx.fog_start) / (gfx.fog_end - gfx.fog_start)).clamp(0.0, 1.0) } else { 0.0 };
            lerp_rgb(tint, 0, fog_k)
        } else {
            gfx.fog_apply(tint, z)
        };
        let (pm, pa) = (gfx.blend, gfx.alpha);
        gfx.depth_queue.set_state(blend, alpha);
        let iw = 1.0 / w.max(1e-4);
        let (c, s) = (rot.cos(), rot.sin());
        let corner = |dx: f32, dy: f32, u: f32, v: f32| TexVert {
            x: sx + (dx * c - dy * s) * r,
            y: sy + (dx * s + dy * c) * r,
            z,
            iw,
            uw: u * iw,
            vw: v * iw,
            ..Default::default()
        };
        let q = [corner(-1.0, -1.0, 0.0, 0.0), corner(1.0, -1.0, 0.999, 0.0), corner(1.0, 1.0, 0.999, 0.999), corner(-1.0, 1.0, 0.0, 0.999)];
        gfx.depth_queue.push_triangle_tex([q[0], q[1], q[2]], tex.clone(), tint, [0.0; 4], false);
        gfx.depth_queue.push_triangle_tex([q[0], q[2], q[3]], tex.clone(), tint, [0.0; 4], false);
        gfx.depth_queue.set_state(pm, pa);
    }

    fn pfx_draw(&mut self) {
        let glow = self.g3d.glow_tex();
        let parts = std::mem::take(&mut self.g3d.parts);
        for p in &parts {
            let t = 1.0 - (p.life / p.max).clamp(0.0, 1.0);
            let size = p.s0 + (p.s1 - p.s0) * t;
            let col = lerp_rgb(p.c0, p.c1, t);
            let fade = ((p.life / p.max) * 3.0).clamp(0.0, 1.0);
            match p.kind {
                1 => {
                    // streak: a thin additive quad from the particle back along its velocity
                    let tail = [p.p[0] - p.v[0] * 0.06, p.p[1] - p.v[1] * 0.06, p.p[2] - p.v[2] * 0.06];
                    let mut gfx = self.gfx.borrow_mut();
                    let near = -gfx.camera.zdist + 0.05;
                    let (ax, ay, az) = gfx.camera.project(p.p[0], p.p[1], p.p[2]);
                    let (bx, by, bz) = gfx.camera.project(tail[0], tail[1], tail[2]);
                    if az > near && bz > near {
                        let w = gfx.camera.focal * size * 0.5 / (az + gfx.camera.zdist).max(1e-3);
                        let w = w.max(0.6);
                        let (dx, dy) = (bx - ax, by - ay);
                        let l = (dx * dx + dy * dy).sqrt().max(1e-3);
                        let (nx, ny) = (-dy / l * w, dx / l * w);
                        let (pm, pa) = (gfx.blend, gfx.alpha);
                        gfx.depth_queue.set_state(1, fade * p.alpha);
                        let iwa = 1.0 / (az + gfx.camera.zdist).max(1e-4);
                        let iwb = 1.0 / (bz + gfx.camera.zdist).max(1e-4);
                        let v = |x: f32, y: f32, z: f32, iw: f32, u: f32| TexVert { x, y, z, iw, uw: u * iw, vw: 0.5 * iw, ..Default::default() };
                        let q = [
                            v(ax + nx, ay + ny, az, iwa, 0.5),
                            v(ax - nx, ay - ny, az, iwa, 0.5),
                            v(bx - nx * 0.3, by - ny * 0.3, bz, iwb, 0.5),
                            v(bx + nx * 0.3, by + ny * 0.3, bz, iwb, 0.5),
                        ];
                        let solid = solid_tex();
                        gfx.depth_queue.push_triangle_tex([q[0], q[1], q[2]], solid.clone(), col, [0.0; 4], false);
                        gfx.depth_queue.push_triangle_tex([q[0], q[2], q[3]], solid, col, [0.0; 4], false);
                        gfx.depth_queue.set_state(pm, pa);
                    }
                    drop(gfx);
                    // a small glowing head on the streak
                    self.g3d_billboard(&glow, p.p, size * 1.6, 0.0, col, fade * p.alpha, 1);
                },
                2 => {
                    if let Some(tex) = self.g3d.texs.get(p.tex as usize).cloned() {
                        self.g3d_billboard(&tex, p.p, size, p.spin * (p.max - p.life), col, p.alpha * fade.min(1.0), 0);
                    }
                },
                _ => self.g3d_billboard(&glow, p.p, size, 0.0, col, fade * p.alpha, 1),
            }
        }
        // keep particles emitted meanwhile (none: drawing does not emit) and restore
        self.g3d.parts = parts;
    }

    fn tm_sprite(&mut self, args: &[Value]) {
        let id = num(args, 0, -1.0);
        let tex = if id < 0.0 { self.g3d.glow_tex() } else {
            match self.g3d.texs.get(id as usize) {
                Some(t) => t.clone(),
                None => return,
            }
        };
        let (cx, cy) = (f(args, 1, 0.0), f(args, 2, 0.0));
        let (w, h) = (f(args, 3, 64.0) * 0.5, f(args, 4, 64.0) * 0.5);
        let rot = f(args, 5, 0.0);
        let tint = rgb(num(args, 6, 16777215.0));
        let alpha = f(args, 7, 1.0);
        let blend = num(args, 8, 0.0) as u8;
        let (c, s) = (rot.cos(), rot.sin());
        let corner = |dx: f32, dy: f32, u: f32, v: f32| TexVert {
            x: cx + dx * w * c - dy * h * s,
            y: cy + dx * w * s + dy * h * c,
            z: 0.0,
            iw: 1.0,
            uw: u,
            vw: v,
            ..Default::default()
        };
        let q = [corner(-1.0, -1.0, 0.0, 0.0), corner(1.0, -1.0, 0.999, 0.0), corner(1.0, 1.0, 0.999, 0.999), corner(-1.0, 1.0, 0.0, 0.999)];
        let mut gfx = self.gfx.borrow_mut();
        let (bw, bh) = (gfx.width, gfx.height);
        let buf = &mut gfx.buffer;
        if buf.len() < bw * bh || bw == 0 {
            return;
        }
        // rows in parallel bands: a full-screen sprite is ~1M textured pixels
        use rayon::prelude::*;
        let bands = rayon::current_num_threads().clamp(1, 16);
        let rows = bh.div_ceil(bands).max(1);
        let t1 = [q[0], q[1], q[2]];
        let t2 = [q[0], q[2], q[3]];
        buf[..bw * bh].par_chunks_mut(rows * bw).enumerate().for_each(|(b, chunk)| {
            let ysh = (b * rows) as f32;
            let h = chunk.len() / bw;
            crate::gfx::raster::fill_triangle_tex(chunk, None, bw, h, ysh, &t1, &tex, tint, [0.0; 4], false, blend, alpha);
            crate::gfx::raster::fill_triangle_tex(chunk, None, bw, h, ysh, &t2, &tex, tint, [0.0; 4], false, blend, alpha);
        });
    }

    fn coll_build(&mut self, args: &[Value]) -> f64 {
        let mi = num(args, 0, -1.0);
        let Some(mesh) = self.g3d.meshes.get(mi.max(0.0) as usize).filter(|_| mi >= 0.0) else { return -1.0 };
        let gsel = num(args, 1, -1.0);
        let o = [f(args, 2, 0.0), f(args, 3, 0.0), f(args, 4, 0.0)];
        let sc = f(args, 5, 1.0);
        let m = rot_from(args, 6);
        let cell = f(args, 7, 8.0).max(0.5);
        let mut tris = Vec::new();
        let mut nrm = Vec::new();
        for (gi, g) in mesh.groups.iter().enumerate() {
            if gsel >= 0.0 && gi != gsel as usize {
                continue;
            }
            for t in &g.idx {
                let a = xf(&m, g.pos[t[0] as usize], sc, o);
                let b = xf(&m, g.pos[t[1] as usize], sc, o);
                let c = xf(&m, g.pos[t[2] as usize], sc, o);
                let n = cross(sub(b, a), sub(c, a));
                if dot(n, n) < 1e-12 {
                    continue;
                }
                tris.push([a, b, c]);
                nrm.push(norm(n));
            }
        }
        let mut coll = Coll { tris, nrm, cell, grid: HashMap::new(), stamp: Vec::new(), frame: 0 };
        coll.stamp = vec![0; coll.tris.len()];
        for (i, t) in coll.tris.iter().enumerate() {
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            for p in t {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            let a = coll.key(lo);
            let b = coll.key(hi);
            for x in a.0..=b.0 {
                for y in a.1..=b.1 {
                    for z in a.2..=b.2 {
                        coll.grid.entry((x, y, z)).or_default().push(i as u32);
                    }
                }
            }
        }
        self.g3d.colls.push(coll);
        (self.g3d.colls.len() - 1) as f64
    }

    fn coll_sphere(&mut self, args: &[Value]) -> Value {
        let id = num(args, 0, -1.0);
        let mut p = [f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)];
        let r = f(args, 4, 1.0).max(1e-3);
        if id < 0.0 || id as usize >= self.g3d.colls.len() {
            return nums(&[p[0], p[1], p[2], 0.0, 0.0, 0.0, 0.0, 0.0]);
        }
        let coll = &mut self.g3d.colls[id as usize];
        let mut cand = Vec::new();
        let mut nsum = [0.0f32; 3];
        let mut contacts = 0.0f32;
        let mut deepest = 0.0f32;
        for _iter in 0..4 {
            coll.query([p[0] - r, p[1] - r, p[2] - r], [p[0] + r, p[1] + r, p[2] + r], &mut cand);
            let mut moved = false;
            for &ti in &cand {
                let t = coll.tris[ti as usize];
                let q = closest_on_tri(p, t[0], t[1], t[2]);
                let d = sub(p, q);
                let dist2 = dot(d, d);
                if dist2 < r * r {
                    let dist = dist2.sqrt();
                    let n = if dist > 1e-5 { [d[0] / dist, d[1] / dist, d[2] / dist] } else { coll.nrm[ti as usize] };
                    let pen = r - dist;
                    p = [p[0] + n[0] * pen, p[1] + n[1] * pen, p[2] + n[2] * pen];
                    nsum = [nsum[0] + n[0] * pen, nsum[1] + n[1] * pen, nsum[2] + n[2] * pen];
                    contacts += 1.0;
                    deepest = deepest.max(pen);
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
        let n = if contacts > 0.0 { norm(nsum) } else { [0.0; 3] };
        nums(&[p[0], p[1], p[2], n[0], n[1], n[2], contacts, deepest])
    }

    fn coll_ray(&mut self, args: &[Value]) -> Value {
        let id = num(args, 0, -1.0);
        let o = [f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)];
        let dir = norm([f(args, 4, 0.0), f(args, 5, 0.0), f(args, 6, 1.0)]);
        let maxt = f(args, 7, 1000.0);
        if id < 0.0 || id as usize >= self.g3d.colls.len() {
            return nums(&[-1.0, 0.0, 0.0, 0.0]);
        }
        let coll = &mut self.g3d.colls[id as usize];
        let e = [o[0] + dir[0] * maxt, o[1] + dir[1] * maxt, o[2] + dir[2] * maxt];
        let lo = [o[0].min(e[0]), o[1].min(e[1]), o[2].min(e[2])];
        let hi = [o[0].max(e[0]), o[1].max(e[1]), o[2].max(e[2])];
        let mut cand = Vec::new();
        coll.query(lo, hi, &mut cand);
        let mut best = (maxt, [0.0f32; 3], false);
        for &ti in &cand {
            let t = coll.tris[ti as usize];
            // Möller–Trumbore
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
        if best.2 {
            nums(&[best.0, best.1[0], best.1[1], best.1[2]])
        } else {
            nums(&[-1.0, 0.0, 0.0, 0.0])
        }
    }
}

/// A 1×1 opaque white texture (flat-colour quads through the textured path).
fn solid_tex() -> Arc<TexData> {
    thread_local! {
        static SOLID: Arc<TexData> = Arc::new(TexData { w: 1, h: 1, px: vec![0xFFFF_FFFF] });
    }
    SOLID.with(|s| s.clone())
}


/// Copies text to the system clipboard with the platform's own tool (clip / pbcopy / wl-copy, xclip).
fn clipboard_set(s: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let tries: &[(&str, &[&str])] = if cfg!(windows) {
        &[("powershell", &["-NoProfile", "-Command", "$input | Set-Clipboard"])]
    } else if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else {
        &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"]), ("xsel", &["-b", "-i"])]
    };
    for (cmd, a) in tries {
        let mut c = Command::new(cmd);
        c.args(*a).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        if let Ok(mut ch) = c.spawn() {
            if let Some(mut i) = ch.stdin.take() {
                let _ = i.write_all(s.as_bytes());
            }
            if ch.wait().map(|st| st.success()).unwrap_or(false) {
                return true;
            }
        }
    }
    false
}

/// Reads the system clipboard as text ("" when there is none).
fn clipboard_get() -> String {
    use std::process::Command;
    let tries: &[(&str, &[&str])] = if cfg!(windows) {
        &[("powershell", &["-NoProfile", "-Command", "Get-Clipboard -Raw"])]
    } else if cfg!(target_os = "macos") {
        &[("pbpaste", &[])]
    } else {
        &[("wl-paste", &["-n"]), ("xclip", &["-selection", "clipboard", "-o"]), ("xsel", &["-b", "-o"])]
    };
    for (cmd, a) in tries {
        let mut c = Command::new(cmd);
        c.args(*a);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        if let Ok(o) = c.output() {
            if o.status.success() {
                return String::from_utf8_lossy(&o.stdout).trim().to_string();
            }
        }
    }
    String::new()
}
