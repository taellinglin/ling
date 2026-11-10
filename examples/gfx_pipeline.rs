// examples/gfx_pipeline.rs — headless FPS bottleneck hunt for the software renderer.
//
//   cargo run --release --example gfx_pipeline                       # CPU paths
//   cargo run --release --example gfx_pipeline --features gpu-cuda   # + CUDA projection
//   LING_BENCH_WINDOW=1 …   + the real minifb blit (opens a window)
//   LING_BENCH_RES=1280x720 / LING_BENCH_FILTER=toon   narrow the run
//   LING_GPU_MIN_PROJECT=0  force the CUDA path for every projection size
//
// Every number is the MEDIAN ms of N runs of the real `ling::gfx` code (no
// mocks). A 120 FPS frame budget is 8.33 ms; the report flags anything that
// alone eats more than a quarter of it.

use ling::gfx::depth::{DepthQueue, TexData, TexShade, TexVert};
use ling::gfx::light::{compute_lit_color, Light};
use ling::gfx::{camera::Camera3D, raster, toon, LingMaterial};
use std::sync::Arc;
use std::time::Instant;

const BUDGET_120: f64 = 1000.0 / 120.0;

struct Rng(u64);
impl Rng {
    fn f(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    fn col(&mut self) -> u32 {
        ((self.f() * 255.0) as u32) << 16 | ((self.f() * 255.0) as u32) << 8 | (self.f() * 255.0) as u32
    }
}

fn filter_ok(name: &str) -> bool {
    match std::env::var("LING_BENCH_FILTER") {
        Ok(f) if !f.is_empty() => name.contains(&f),
        _ => true,
    }
}

/// Median ms over `iters` runs (after one warm-up). `setup` runs untimed before each.
fn bench<S, F: FnMut(&mut S)>(name: &str, iters: usize, mut setup: impl FnMut() -> S, mut f: F) -> Option<f64> {
    if !filter_ok(name) {
        return None;
    }
    let mut s = setup();
    f(&mut s);
    let mut v = Vec::with_capacity(iters);
    for _ in 0..iters {
        let mut s = setup();
        let t = Instant::now();
        f(&mut s);
        v.push(t.elapsed().as_secs_f64() * 1e3);
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = v[v.len() / 2];
    let flag = if med > BUDGET_120 {
        "  !!! over the whole 120 FPS budget"
    } else if med > BUDGET_120 / 4.0 {
        "  !  >25% of 120 FPS budget"
    } else {
        ""
    };
    println!("  {name:<58} {med:>9.3} ms  (min {:.3}){flag}", v[0]);
    Some(med)
}

// ── scene builders ──────────────────────────────────────────────────────────

/// `n` small screen-space triangles (~`size` px across) at random depth.
fn small_tris(w: usize, h: usize, n: usize, size: f32, seed: u64) -> Vec<[f32; 10]> {
    let mut r = Rng(seed);
    (0..n)
        .map(|_| {
            let (cx, cy, z) = (r.f() * w as f32, r.f() * h as f32, 1.0 + r.f() * 50.0);
            let mut t = [0f32; 10];
            for k in 0..3 {
                t[k * 3] = cx + (r.f() - 0.5) * size;
                t[k * 3 + 1] = cy + (r.f() - 0.5) * size;
                t[k * 3 + 2] = z + r.f() * 0.5;
            }
            t[9] = f32::from_bits(r.col());
            t
        })
        .collect()
}

fn queue_flat(tris: &[[f32; 10]]) -> DepthQueue {
    let mut q = DepthQueue::default();
    for t in tris {
        q.push_triangle_zv(t[9].to_bits(), t[0], t[1], t[2], t[3], t[4], t[5], t[6], t[7], t[8]);
    }
    q
}

fn queue_gouraud(tris: &[[f32; 10]], bands: u32) -> DepthQueue {
    let mut q = DepthQueue::default();
    for t in tris {
        let c = t[9].to_bits();
        q.push_triangle_g_zv(t[0], t[1], t[2], c, t[3], t[4], t[5], c ^ 0x3030, t[6], t[7], t[8], c ^ 0x303000, bands, false);
    }
    q
}

fn checker_tex(n: u32) -> Arc<TexData> {
    let px = (0..n * n)
        .map(|i| {
            let (x, y) = (i % n, i / n);
            if ((x / 16) ^ (y / 16)) & 1 == 0 { 0xFF_E0_C0_A0 } else { 0xFF_30_50_80 }
        })
        .collect();
    Arc::new(TexData::new(n, n, px))
}

/// `n` screen-space textured quads of `size` px with perspective-ish 1/w.
fn queue_tex(w: usize, h: usize, n: usize, size: f32, tex: &Arc<TexData>, shade: Option<Arc<TexShade>>) -> DepthQueue {
    let mut r = Rng(7);
    let mut q = DepthQueue::default();
    for _ in 0..n {
        let (x, y, z) = (r.f() * (w as f32 - size), r.f() * (h as f32 - size), 2.0 + r.f() * 30.0);
        let iw = 1.0 / (z + 5.0);
        let mk = |px: f32, py: f32, u: f32, v: f32| TexVert {
            x: px, y: py, z, iw, uw: u * iw, vw: v * iw, wx: px * 0.01 * iw, wy: 0.0, wz: py * 0.01 * iw,
        };
        let a = mk(x, y, 0.0, 0.0);
        let b = mk(x + size, y, 1.0, 0.0);
        let c = mk(x + size, y + size, 1.0, 1.0);
        let d = mk(x, y + size, 0.0, 1.0);
        q.push_triangle_tex_shaded([a, b, c], tex.clone(), 0xFFFFFF, [0.0; 4], true, shade.clone());
        q.push_triangle_tex_shaded([a, c, d], tex.clone(), 0xFFFFFF, [0.0; 4], true, shade.clone());
    }
    q
}

fn lights(k: usize) -> Vec<Light> {
    (0..k)
        .map(|i| Light {
            x: i as f32 * 3.0 - 4.0, y: -3.0, z: 2.0 + i as f32,
            r: 1.0, g: 0.9, b: 0.8, intensity: 1.0, radius: 30.0,
        })
        .collect()
}

/// A z-buffer that looks like a real scene (overlapping objects at varied depth).
fn scene_zbuf(w: usize, h: usize) -> (Vec<u32>, Vec<f32>) {
    let mut buf = vec![0x202830u32; w * h];
    let mut z = vec![f32::INFINITY; w * h];
    raster::set_persp_zd(5.0);
    queue_flat(&small_tris(w, h, 4000, 60.0, 3)).flush(&mut buf, Some(&mut z), true, w, h, false);
    (buf, z)
}

fn main() {
    let threads = rayon::current_num_threads();
    let res: Vec<(usize, usize)> = match std::env::var("LING_BENCH_RES") {
        Ok(s) if s.contains('x') => {
            let (a, b) = s.split_once('x').unwrap();
            vec![(a.parse().unwrap(), b.parse().unwrap())]
        },
        _ => vec![(1280, 720), (1920, 1080)],
    };
    println!("ling gfx pipeline bench — {threads} rayon threads, 120 FPS budget = {BUDGET_120:.2} ms, size_of DrawCall = {} B", std::mem::size_of::<ling::gfx::depth::DrawCall>());
    raster::set_persp_zd(5.0);

    for &(w, h) in &res {
        println!("\n══ {w}x{h} ══════════════════════════════════════════════════════════════");
        let n = w * h;

        println!("── framebuffer");
        bench("clear (fill)", 50, || fresh(n), |b| b.fill(0x102030));
        bench("z-buffer reset to +inf", 50, || { let mut z = vec![1f32; n]; z[0] = 0.0; z }, |z| z.iter_mut().for_each(|v| *v = f32::INFINITY));

        println!("── many small dynamic objects (60k tris ≈ 5k cubes, ~12 px)");
        let small = small_tris(w, h, 60_000, 12.0, 1);
        bench("queue build: push 60k flat tris", 30, || (), |_| { std::hint::black_box(queue_flat(&small)); });
        bench("flush 60k flat, painter (sort+raster)", 20, || (queue_flat(&small), fresh(n)), |(q, b)| {
            std::mem::take(q).flush(b, None, false, w, h, false)
        });
        bench("flush 60k flat, z-buffer", 20, || (queue_flat(&small), fresh(n), vec![f32::INFINITY; n]), |(q, b, z)| {
            std::mem::take(q).flush(b, Some(z), false, w, h, false)
        });
        bench("flush 60k gouraud smooth-cel (4 bands), z-buffer", 20, || (queue_gouraud(&small, 4), fresh(n), vec![f32::INFINITY; n]), |(q, b, z)| {
            std::mem::take(q).flush(b, Some(z), false, w, h, false)
        });

        println!("── retained meshes: 5000 objects x 12-tri cube via GfxState::mesh_draw (4 lights)");
        for (label, hyper, dt) in [("z-buffer", false, true), ("z-buffer + hyperbolic", true, true), ("painter", false, false)] {
            bench(&format!("mesh_draw x5000 submit ({label})"), 15, || game_state(w, h, hyper, dt), |g| {
                draw_objects(g, 5000);
            });
            bench(&format!("mesh_draw x5000 submit + flush + toon ({label})"), 15, || game_state(w, h, hyper, dt), |g| {
                draw_objects(g, 5000);
                present_like(g);
            });
        }
        println!("── fill-bound: 300 big tris (~1/6 screen each, ~25x overdraw)");
        let big = small_tris(w, h, 300, (w.min(h) as f32) * 0.8, 2);
        bench("flush 300 big flat, painter", 20, || (queue_flat(&big), fresh(n)), |(q, b)| {
            std::mem::take(q).flush(b, None, false, w, h, false)
        });
        bench("flush 300 big flat, z-buffer", 20, || (queue_flat(&big), fresh(n), vec![f32::INFINITY; n]), |(q, b, z)| {
            std::mem::take(q).flush(b, Some(z), false, w, h, false)
        });
        bench("flush 300 big gouraud smooth-cel, z-buffer", 20, || (queue_gouraud(&big, 4), fresh(n), vec![f32::INFINITY; n]), |(q, b, z)| {
            std::mem::take(q).flush(b, Some(z), false, w, h, false)
        });

        println!("── textured raster (2k quads of 96 px, 256² tex)");
        let tex = checker_tex(256);
        let sh_lit = Arc::new(TexShade {
            amb: 0.3,
            lights: (0..4).map(|i| [i as f32 * 3.0, 0.0, 4.0, 1.0, 0.9, 0.8, 1.2, 12.0]).collect(),
            fog_start: 10.0, fog_end: 40.0, fog_color: 0x203040, lit: true, ripples: vec![],
        });
        bench("flush tex unlit, z-buffer", 15, || (queue_tex(w, h, 2000, 96.0, &tex, None), fresh(n), vec![f32::INFINITY; n]), |(q, b, z)| {
            std::mem::take(q).flush(b, Some(z), false, w, h, false)
        });
        bench("flush tex + 4 per-pixel lights + fog, z-buffer", 15, || (queue_tex(w, h, 2000, 96.0, &tex, Some(sh_lit.clone())), fresh(n), vec![f32::INFINITY; n]), |(q, b, z)| {
            std::mem::take(q).flush(b, Some(z), false, w, h, false)
        });

        println!("── edges / vertices / vector shapes");
        let mut r = Rng(9);
        let segs: Vec<[f32; 4]> = (0..20_000).map(|_| {
            let (x, y, a) = (r.f() * w as f32, r.f() * h as f32, r.f() * 6.28);
            [x, y, x + a.cos() * 50.0, y + a.sin() * 50.0]
        }).collect();
        bench("20k edges via depth queue (aliased)", 20, || {
            let mut q = DepthQueue::default();
            for (i, s) in segs.iter().enumerate() { q.push_line(i as f32, 0xFFFFFF, s[0], s[1], s[2], s[3]); }
            (q, fresh(n))
        }, |(q, b)| std::mem::take(q).flush(b, None, false, w, h, false));
        bench("20k edges via depth queue (antialias on)", 20, || {
            let mut q = DepthQueue::default();
            for (i, s) in segs.iter().enumerate() { q.push_line(i as f32, 0xFFFFFF, s[0], s[1], s[2], s[3]); }
            (q, fresh(n))
        }, |(q, b)| std::mem::take(q).flush(b, None, false, w, h, true));
        bench("20k edges draw_line_aa direct", 20, || fresh(n), |b| {
            for s in &segs { raster::draw_line_aa(b, w, h, 0xFFFFFF, false, s[0], s[1], s[2], s[3]); }
        });
        let circles: Vec<Vec<[f32; 2]>> = (0..500).map(|i| {
            let (cx, cy) = ((i * 97 % w) as f32, (i * 61 % h) as f32);
            (0..64).map(|k| { let a = k as f32 / 64.0 * 6.2832; [cx + a.cos() * 20.0, cy + a.sin() * 20.0] }).collect()
        }).collect();
        bench("vector: 500 circles fill_contours (aliased)", 20, || fresh(n), |b| {
            for c in &circles { raster::fill_contours(b, w, h, 0x40A0FF, false, std::slice::from_ref(c)); }
        });
        bench("vector: 500 circles fill_contours_aa", 20, || fresh(n), |b| {
            for c in &circles { raster::fill_contours_aa(b, w, h, 0x40A0FF, false, std::slice::from_ref(c)); }
        });
        bench("vector: 1 full-screen path fill_contours_aa", 20, || fresh(n), |b| {
            let big = vec![(0..256).map(|k| { let a = k as f32 / 256.0 * 6.2832; [w as f32 * 0.5 + a.cos() * w as f32 * 0.48, h as f32 * 0.5 + a.sin() * h as f32 * 0.48] }).collect::<Vec<_>>()];
            raster::fill_contours_aa(b, w, h, 0x40A0FF, false, &big);
        });
        bench("vertex dots: 20k 3x3 markers", 20, || fresh(n), |b| {
            for s in &segs { raster::fill_disc_soft(b, w, h, s[0], s[1], 1.5, 1.5, 0xFFFF00, 1.0, 0.3, 0, false); }
        });

        println!("── toon / post-process chain (full screen)");
        let (sbuf, sz) = scene_zbuf(w, h);
        let cfg_default = toon::ToonConfig::default();
        bench("apply_ramp (default 3-band — runs EVERY frame)", 30, || sbuf.clone(), |b| toon::apply_ramp(b, w, h, &cfg_default.ramp));
        bench("draw_outlines 1px", 15, || sbuf.clone(), |b| toon::draw_outlines(b, &sz, w, h, 1.0, 0, 0.05));
        bench("draw_outlines 2px (anime thick)", 15, || sbuf.clone(), |b| toon::draw_outlines(b, &sz, w, h, 2.0, 0, 0.05));
        bench("apply_ssao", 15, || sbuf.clone(), |b| toon::apply_ssao(b, &sz, w, h, 0.5, 6.0, 12.0));
        bench("apply_bloom", 15, || sbuf.clone(), |b| toon::apply_bloom(b, w, h, 0.6, 0.74));
        bench("apply_fxaa", 15, || sbuf.clone(), |b| toon::apply_fxaa(b, w, h));
        bench("toon::apply default config (= every present)", 30, || sbuf.clone(), |b| toon::apply(&cfg_default, b, &sz, w, h));
        let mut cfg_all = toon::ToonConfig::default();
        cfg_all.outline_px = 2.0;
        cfg_all.ao_strength = 0.5;
        cfg_all.bloom_strength = 0.6;
        cfg_all.fxaa = true;
        bench("depth_of_field (game: depth_blur r=2)", 15, || sbuf.clone(), |b| raster::depth_of_field(b, &sz, w, h, 20.0, 22.0, 2, 0.0));
        bench("depth_of_field r=8", 15, || sbuf.clone(), |b| raster::depth_of_field(b, &sz, w, h, 20.0, 22.0, 8, 0.0));
        bench("toon::apply everything on (AO+outline+ramp+bloom+fxaa)", 15, || sbuf.clone(), |b| toon::apply(&cfg_all, b, &sz, w, h));
    }

    println!("\n══ per-face lighting (resolution independent) ════════════════════════════");
    let mut r = Rng(5);
    let faces: Vec<([f32; 3], [f32; 3])> = (0..60_000).map(|_| ([r.f() - 0.5, r.f() - 0.5, r.f() - 0.5], [r.f() * 10.0, r.f() * 10.0, r.f() * 10.0])).collect();
    for k in [0usize, 1, 4, 16] {
        let ls = lights(k);
        bench(&format!("compute_lit_color 60k faces, {k} lights"), 20, || (), |_| {
            let mut acc = 0u32;
            for (nn, c) in &faces { acc = acc.wrapping_add(compute_lit_color(0xC08060, *nn, *c, &ls, 0.15)); }
            std::hint::black_box(acc);
        });
    }
    let mat = LingMaterial::default();
    let ls4 = lights(4);
    bench("material::shade (BSDF) 60k faces, 4 lights", 20, || (), |_| {
        let mut acc = 0u32;
        for (nn, c) in &faces { acc = acc.wrapping_add(ling::gfx::material::shade(&mat, *nn, [0.0, 0.0, -1.0], *c, &ls4, 0.15)); }
        std::hint::black_box(acc);
    });

    println!("\n══ vertex projection / hyperbolic warp ═══════════════════════════════════");
    let mut cam = Camera3D::default();
    cam.cx = 960.0; cam.cy = 540.0; cam.focal = 1080.0; cam.zdist = 5.0;
    let (a, b) = (0.6f32, 0.3f32);
    cam.cry = a.cos(); cam.sry = a.sin(); cam.crx = b.cos(); cam.srx = b.sin();
    let mut r = Rng(11);
    for nv in [2_000usize, 50_000, 1_000_000] {
        let world: Vec<f32> = (0..nv * 3).map(|_| (r.f() - 0.5) * 20.0).collect();
        let mut out = vec![0f32; nv * 3];
        let it = if nv >= 1_000_000 { 10 } else { 40 };
        cam.hyp_r = 0.0;
        bench(&format!("camera.project serial, {nv} verts, hyper OFF"), it, || (), |_| {
            for i in 0..nv { let p = cam.project(world[i * 3], world[i * 3 + 1], world[i * 3 + 2]); out[i * 3] = p.0; out[i * 3 + 1] = p.1; out[i * 3 + 2] = p.2; }
        });
        let mut hc = cam.clone();
        hc.hyp_r = 8.0; hc.hyp_s = 0.6;
        bench(&format!("camera.project serial, {nv} verts, hyper ON"), it, || (), |_| {
            for i in 0..nv { let p = hc.project(world[i * 3], world[i * 3 + 1], world[i * 3 + 2]); out[i * 3] = p.0; out[i * 3 + 1] = p.1; out[i * 3 + 2] = p.2; }
        });
        let cp = ling_gpu::CameraParams {
            cry: cam.cry, sry: cam.sry, crx: cam.crx, srx: cam.srx, cx: cam.cx, cy: cam.cy,
            focal: cam.focal, zdist: cam.zdist, tx: 0.0, ty: 0.0, tz: 0.0,
        };
        let be = ling_gpu::backend();
        bench(&format!("ling_gpu project_points [{}], {nv} verts (no hyper)", be.name()), it, || (), |_| be.project_points(&world, &cp, &mut out));
        bench(&format!("OLD w/ hyper: batch + serial re-project, {nv} verts"), it, || (), |_| {
            be.project_points(&world, &cp, &mut out);
            for i in 0..nv { let p = hc.project(world[i * 3], world[i * 3 + 1], world[i * 3 + 2]); out[i * 3] = p.0; out[i * 3 + 1] = p.1; out[i * 3 + 2] = p.2; }
        });
        let (hcp, hw) = hc.gpu_params();
        let cpu = ling_gpu::cpu_backend();
        bench(&format!("NEW w/ hyper: warped batch, CPU rayon, {nv} verts"), it, || (), |_| cpu.project_points_warped(&world, &hcp, &hw, &mut out));
        if be.is_gpu() {
            bench(&format!("NEW w/ hyper: warped batch, CUDA kernel, {nv} verts"), it, || (), |_| be.project_points_warped(&world, &hcp, &hw, &mut out));
        }
        bench(&format!("NEW w/ hyper: Camera3D::project_batch (auto), {nv} verts"), it, || (), |_| hc.project_batch(&world, &mut out));
    }
    println!();

    if std::env::var("LING_BENCH_WINDOW").is_ok() {
        println!("\n══ present: minifb update_with_buffer (real window) ══════════════════════");
        for &(w, h) in &res {
            let mut win = minifb::Window::new("ling bench", w, h, minifb::WindowOptions::default()).expect("window");
            win.set_target_fps(0);
            let buf = vec![0x336699u32; w * h];
            bench(&format!("update_with_buffer {w}x{h} (1:1)"), 60, || (), |_| { win.update_with_buffer(&buf, w, h).unwrap(); });
            let (hw, hh) = (w / 2, h / 2);
            let half = vec![0x336699u32; hw * hh];
            bench(&format!("update_with_buffer {hw}x{hh} → {w}x{h} window (stretch)"), 60, || (), |_| { win.update_with_buffer(&half, hw, hh).unwrap(); });
        }
    }
}

/// A framebuffer whose pages are already faulted in (like the engine's reused
/// `GfxState::buffer`); `vec![0; n]` would hand back lazily-zeroed OS pages and
/// bill the page faults to whatever runs first.
fn fresh(n: usize) -> Vec<u32> {
    let mut b = Vec::with_capacity(n);
    b.resize(n, 0x0010_2030);
    b
}

fn cube_tris() -> Vec<([f32; 9], u32)> {
    let v = |i: u32| -> [f32; 3] { [if i & 1 != 0 { 0.5 } else { -0.5 }, if i & 2 != 0 { 0.5 } else { -0.5 }, if i & 4 != 0 { 0.5 } else { -0.5 }] };
    let faces = [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]];
    let mut out = Vec::new();
    for (k, f) in faces.iter().enumerate() {
        for t in [[f[0], f[1], f[2]], [f[0], f[2], f[3]]] {
            let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
            out.push(([a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2]], 0x406080 + k as u32 * 0x101010));
        }
    }
    out
}

/// A headless GfxState configured like a ling-game scene.
fn game_state(w: usize, h: usize, hyper: bool, depth_test: bool) -> ling::gfx::GfxState {
    let mut g = ling::gfx::GfxState::new();
    g.width = w;
    g.height = h;
    g.buffer = fresh(w * h);
    g.depth_buf = vec![f32::INFINITY; w * h];
    g.depth_test = depth_test;
    g.zbuf_needs_clear = true;
    g.camera.cx = w as f32 * 0.5;
    g.camera.cy = h as f32 * 0.5;
    g.camera.focal = h as f32;
    g.camera.zdist = 5.0;
    let (a, b) = (0.5f32, 0.35f32);
    g.camera.cry = a.cos(); g.camera.sry = a.sin(); g.camera.crx = b.cos(); g.camera.srx = b.sin();
    g.camera.tz = -20.0;
    if hyper { g.camera.hyp_r = 30.0; g.camera.hyp_s = 0.35; }
    g.lights = lights(4);
    let id = g.mesh_register(cube_tris());
    assert_eq!(id, 0);
    g
}

fn draw_objects(g: &mut ling::gfx::GfxState, n: usize) {
    let mut r = Rng(42);
    for i in 0..n {
        let (x, y, z) = ((r.f() - 0.5) * 60.0, (r.f() - 0.5) * 30.0, r.f() * 60.0);
        let a = i as f32 * 0.37;
        g.mesh_draw(0, x, y, z, a.cos(), 0.0, a.sin(), 0.0, 1.0, 0.0, 0.8, true);
    }
}

/// What `present` does minus the window blit: flush the queue, then the toon chain.
fn present_like(g: &mut ling::gfx::GfxState) {
    let (w, h) = (g.width, g.height);
    raster::set_persp_zd(g.camera.zdist);
    let q = std::mem::take(&mut g.depth_queue);
    let reset = g.zbuf_needs_clear;
    let z = if g.depth_test { Some(&mut g.depth_buf) } else { None };
    q.flush(&mut g.buffer, z, reset, w, h, false);
    g.toon_post_process();
}
