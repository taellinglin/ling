// src/gfx/depth.rs — deferred depth-sorted draw queue (painter's algorithm).
//
// All 3-D draw calls (`วาดสามเหลี่ยม3มิติ`, `วาดเส้น3มิติ`) push a `DrawCall`
// into this queue instead of rasterising immediately.  When `แสดงผล` / `present`
// is called, the queue is sorted back-to-front by the depth tag and then
// flushed into the pixel buffer.
//
// Painter's algorithm is exact for convex non-intersecting geometry and
// produces plausible results for the Sierpiński fractal + tesseract wireframe.
//
// Each call also captures the current blend `mode` (0 normal · 1 add · 2 mul ·
// 3 screen · 4 subtract · 5 overlay) and pen `alpha` so translucent 3-D FX
// (sword slashes, ring trails, liquid orbs) composite over the scene instead of
// painting opaque black where they fade out.

// `raster` is wasm-safe (pure CPU); the software-framebuffer flush runs on web too.
use crate::gfx::raster;
#[cfg(not(target_arch = "wasm32"))]
use rayon::prelude::*;


/// Rasterise one queued call into a band starting `ysh` rows down: every y
/// coordinate is shifted into band-local space and the band's own slices are
/// indexed as a standalone `width × bh` framebuffer.
#[inline]
fn rasterize_call(
    call: &DrawCall,
    buf: &mut [u32],
    zbuf: Option<&mut [f32]>,
    width: usize,
    height: usize,
    ysh: f32,
    aa: bool,
) {
    rasterize_call_z(call, buf, zbuf, width, height, ysh, aa, false)
}

/// Can this call be depth-prepassed (early-z)? Only opaque calls that write
/// depth at every covered pixel: non-blended gouraud triangles, and textured
/// ones with a fully opaque texture, full pen alpha, depth writes on and no
/// see-through hole. Flat triangles are cheap to shade, so they skip it.
#[inline]
fn prepass_eligible(call: &DrawCall) -> bool {
    if call.mode != 0 || call.alpha < 0.999 {
        return false;
    }
    match call.kind {
        DrawKind::TriangleG { .. } => true,
        DrawKind::TriangleT { ref tex, hole, zwrite, .. } => zwrite && tex.opaque && hole[2] <= 0.0,
        _ => false,
    }
}

/// Depth-only raster of a [`prepass_eligible`] call (band-local, like `rasterize_call`).
#[inline]
fn prepass_call(call: &DrawCall, zbuf: &mut [f32], width: usize, height: usize, ysh: f32) {
    match call.kind {
        DrawKind::TriangleG { x0, y0, z0, x1, y1, z1, x2, y2, z2, .. } => {
            // the gouraud fill computes depth exactly like the flat one
            raster::fill_triangle_z_mode::<{ raster::Z_PREPASS }>(&mut [], zbuf, width, height, 0, x0, y0 - ysh, z0, x1, y1 - ysh, z1, x2, y2 - ysh, z2)
        },
        DrawKind::TriangleT { ref v, ref tex, tint, hole, zwrite, ref shade } => {
            raster::fill_triangle_tex_shaded_mode::<{ raster::Z_PREPASS }>(
                &mut [], Some(zbuf), width, height, ysh, v, tex, tint, hole, zwrite, call.mode, call.alpha, shade.as_deref(),
            )
        },
        _ => {},
    }
}

/// Rough pixels a call covers inside rows `[r0, r1)` (half its clipped bbox).
#[inline]
fn call_area_in_rows(call: &DrawCall, width: usize, r0: f32, r1: f32) -> f32 {
    let (xa, xb, ya, yb) = match call.kind {
        DrawKind::Triangle { x0, y0, x1, y1, x2, y2, .. } | DrawKind::TriangleG { x0, y0, x1, y1, x2, y2, .. } => {
            (x0.min(x1).min(x2), x0.max(x1).max(x2), y0.min(y1).min(y2), y0.max(y1).max(y2))
        },
        DrawKind::TriangleT { ref v, .. } => (
            v[0].x.min(v[1].x).min(v[2].x),
            v[0].x.max(v[1].x).max(v[2].x),
            v[0].y.min(v[1].y).min(v[2].y),
            v[0].y.max(v[1].y).max(v[2].y),
        ),
        DrawKind::Line { .. } => return 0.0,
    };
    let wdt = xb.min(width as f32) - xa.max(0.0);
    let hgt = yb.min(r1) - ya.max(r0);
    if wdt > 0.0 && hgt > 0.0 { 0.5 * wdt * hgt } else { 0.0 }
}

/// `rasterize_call`; with `after_prepass` the call's depth was already recorded
/// by [`prepass_call`], so it shades where its depth is `<=` the stored one.
#[inline]
#[allow(clippy::too_many_arguments)]
fn rasterize_call_z(
    call: &DrawCall,
    buf: &mut [u32],
    zbuf: Option<&mut [f32]>,
    width: usize,
    height: usize,
    ysh: f32,
    aa: bool,
    after_prepass: bool,
) {
    if let DrawKind::TriangleT { ref v, ref tex, tint, hole, zwrite, ref shade } = call.kind {
        if after_prepass {
            raster::fill_triangle_tex_shaded_mode::<{ raster::Z_LE }>(buf, zbuf, width, height, ysh, v, tex, tint, hole, zwrite, call.mode, call.alpha, shade.as_deref());
        } else {
            raster::fill_triangle_tex_shaded(buf, zbuf, width, height, ysh, v, tex, tint, hole, zwrite, call.mode, call.alpha, shade.as_deref());
        }
        return;
    }
    if after_prepass {
        if let (Some(z), DrawKind::TriangleG { x0, y0, z0, c0, x1, y1, z1, c1, x2, y2, z2, c2, bands, softness }) = (zbuf, &call.kind) {
            raster::fill_triangle_gouraud_z_mode::<{ raster::Z_LE }>(
                buf, z, width, height, *x0, *y0 - ysh, *z0, *c0, *x1, *y1 - ysh, *z1, *c1, *x2, *y2 - ysh, *z2, *c2, *bands, *softness, call.alpha, call.mode, call.unlit,
            );
        }
        return;
    }
    let blended = call.mode != 0 || call.alpha < 0.999;
    match zbuf {
        Some(z) => match call.kind {
            DrawKind::Triangle { x0, y0, z0, x1, y1, z1, x2, y2, z2 } => {
                if blended {
                    raster::fill_triangle_z_blend(
                        buf,
                        z,
                        width,
                        height,
                        call.color,
                        call.mode,
                        call.alpha,
                        x0,
                        y0 - ysh,
                        z0,
                        x1,
                        y1 - ysh,
                        z1,
                        x2,
                        y2 - ysh,
                        z2,
                    );
                } else {
                    raster::fill_triangle_z(
                        buf,
                        z,
                        width,
                        height,
                        call.color,
                        x0,
                        y0 - ysh,
                        z0,
                        x1,
                        y1 - ysh,
                        z1,
                        x2,
                        y2 - ysh,
                        z2,
                    );
                }
            },
            DrawKind::TriangleG {
                x0,
                y0,
                z0,
                c0,
                x1,
                y1,
                z1,
                c1,
                x2,
                y2,
                z2,
                c2,
                bands,
                softness,
            } => raster::fill_triangle_gouraud_z(
                buf,
                z,
                width,
                height,
                x0,
                y0 - ysh,
                z0,
                c0,
                x1,
                y1 - ysh,
                z1,
                c1,
                x2,
                y2 - ysh,
                z2,
                c2,
                bands,
                softness,
                call.alpha,
                call.mode,
                call.unlit,
            ),
            DrawKind::Line { x0, y0, x1, y1, .. } => {
                if aa {
                    raster::draw_line_aa(
                        buf,
                        width,
                        height,
                        call.color,
                        call.mode == 1,
                        x0,
                        y0 - ysh,
                        x1,
                        y1 - ysh,
                    );
                } else if blended {
                    raster::draw_line_blend(
                        buf,
                        width,
                        height,
                        call.color,
                        call.mode,
                        call.alpha,
                        x0,
                        y0 - ysh,
                        x1,
                        y1 - ysh,
                    );
                } else {
                    raster::draw_line(buf, width, height, call.color, x0, y0 - ysh, x1, y1 - ysh);
                }
            },
            DrawKind::TriangleT { .. } => {},
        },
        None => match call.kind {
            DrawKind::Triangle { x0, y0, x1, y1, x2, y2, .. } => {
                if blended {
                    raster::fill_triangle_blend(
                        buf,
                        width,
                        height,
                        call.color,
                        call.mode,
                        call.alpha,
                        x0,
                        y0 - ysh,
                        x1,
                        y1 - ysh,
                        x2,
                        y2 - ysh,
                    );
                } else {
                    raster::fill_triangle(
                        buf,
                        width,
                        height,
                        call.color,
                        x0,
                        y0 - ysh,
                        x1,
                        y1 - ysh,
                        x2,
                        y2 - ysh,
                    );
                }
            },
            DrawKind::TriangleG { x0, y0, c0, x1, y1, c1, x2, y2, c2, bands, softness, .. } => {
                raster::fill_triangle_gouraud(
                    buf,
                    width,
                    height,
                    x0,
                    y0 - ysh,
                    c0,
                    x1,
                    y1 - ysh,
                    c1,
                    x2,
                    y2 - ysh,
                    c2,
                    bands,
                    softness,
                    call.alpha,
                    call.mode,
                    call.unlit,
                )
            },
            DrawKind::Line { x0, y0, x1, y1, .. } => {
                if aa {
                    raster::draw_line_aa(
                        buf,
                        width,
                        height,
                        call.color,
                        call.mode == 1,
                        x0,
                        y0 - ysh,
                        x1,
                        y1 - ysh,
                    );
                } else if blended {
                    raster::draw_line_blend(
                        buf,
                        width,
                        height,
                        call.color,
                        call.mode,
                        call.alpha,
                        x0,
                        y0 - ysh,
                        x1,
                        y1 - ysh,
                    );
                } else {
                    raster::draw_line(buf, width, height, call.color, x0, y0 - ysh, x1, y1 - ysh);
                }
            },
            DrawKind::TriangleT { .. } => {},
        },
    }
}

#[cfg(feature = "gl-web")]
pub(crate) mod gl_scene {
    use super::*;
    use std::collections::HashSet;

    fn rgb(c: u32) -> [f32; 3] {
        [((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0]
    }

    fn vert(x: f32, y: f32, z: f32, uw: f32, vw: f32, c: [f32; 3], a: f32) -> ling_gl::Vertex {
        let iw = 1.0 / z.max(1e-3);
        ling_gl::Vertex { x, y, iw, uw: uw * iw, vw: vw * iw, tint: [c[0], c[1], c[2], a], w: [0.0; 3] }
    }

    /// Pack sorted calls into batches (consecutive calls sharing texture, blend and
    /// shading merge). Textures not yet in `known` are attached to the scene.
    pub fn build(calls: &[DrawCall], known: &mut HashSet<u64>) -> ling_gl::Scene {
        let mut scene = ling_gl::Scene::new([0.0; 3]);
        scene.clear_alpha = 0.0;
        let zd = super::raster::persp_zd();
        scene.zdist = if zd.is_finite() { zd } else { 0.0 };
        let mut last_shade: Option<usize> = None;
        let mut opaque: Vec<ling_gl::Batch> = Vec::new();
        let mut opaque_ix: std::collections::HashMap<(Option<u64>, Option<usize>), usize> = std::collections::HashMap::new();
        for call in calls {
            let a = call.alpha.clamp(0.0, 1.0);
            let blend = match (call.mode, a < 0.999) {
                (1, _) => ling_gl::Blend::Additive,
                (0, false) => ling_gl::Blend::Opaque,
                (0, true) => ling_gl::Blend::Alpha,
                _ => ling_gl::Blend::Alpha,
            };
            let mut fog = ling_gl::Fog::default();
            let mut lighting = ling_gl::Lighting::default();
            let mut shade_id = 0usize;
            let (tex_id, verts): (Option<u64>, [ling_gl::Vertex; 3]) = match &call.kind {
                DrawKind::Triangle { x0, y0, z0, x1, y1, z1, x2, y2, z2 } => {
                    let c = rgb(call.color);
                    (None, [vert(*x0, *y0, *z0, 0.0, 0.0, c, a), vert(*x1, *y1, *z1, 0.0, 0.0, c, a), vert(*x2, *y2, *z2, 0.0, 0.0, c, a)])
                },
                DrawKind::TriangleG { x0, y0, z0, c0, x1, y1, z1, c1, x2, y2, z2, c2, .. } => (
                    None,
                    [
                        vert(*x0, *y0, *z0, 0.0, 0.0, rgb(*c0), a),
                        vert(*x1, *y1, *z1, 0.0, 0.0, rgb(*c1), a),
                        vert(*x2, *y2, *z2, 0.0, 0.0, rgb(*c2), a),
                    ],
                ),
                DrawKind::TriangleT { v, tex, tint, shade, .. } => {
                    let id = {
                        // Arc addresses get reused by short-lived textures (portraits, hair), so mix in a content sample.
                        let mut h = std::sync::Arc::as_ptr(tex) as usize as u64 ^ ((tex.w as u64) << 32 | tex.h as u64);
                        let step = (tex.px.len() / 64).max(1);
                        for p in tex.px.iter().step_by(step) {
                            h = (h ^ *p as u64).wrapping_mul(0x100000001b3);
                        }
                        h
                    };
                    if known.insert(id) {
                        scene.textures.push(ling_gl::Texture { id, w: tex.w, h: tex.h, px: tex.px.clone() });
                    }
                    if let Some(sh) = shade {
                        shade_id = std::sync::Arc::as_ptr(sh) as usize;
                        if sh.fog_end > 0.0 {
                            fog = ling_gl::Fog { start: sh.fog_start, end: sh.fog_end, color: rgb(sh.fog_color) };
                        }
                        lighting = ling_gl::Lighting { lit: sh.lit, amb: sh.amb, lights: sh.lights.clone() };
                    }
                    let c = rgb(*tint);
                    let mk = |t: &TexVert| ling_gl::Vertex {
                        x: t.x,
                        y: t.y,
                        iw: t.iw.max(1e-6),
                        uw: t.uw,
                        vw: t.vw,
                        tint: [c[0], c[1], c[2], a],
                        w: [t.wx, t.wy, t.wz],
                    };
                    (Some(id), [mk(&v[0]), mk(&v[1]), mk(&v[2])])
                },
                DrawKind::Line { .. } => continue,
            };
            let key = if shade_id == 0 { None } else { Some(shade_id) };
            if blend == ling_gl::Blend::Opaque {
                // The GPU depth-tests, so opaque draw order is irrelevant: group by
                // texture + shading instead of interleaving them in depth order,
                // which collapses tens of thousands of batches into a few hundred.
                let idx = *opaque_ix.entry((tex_id, key)).or_insert_with(|| {
                    opaque.push(ling_gl::Batch { texture: tex_id, blend, fog, lighting, verts: Vec::new() });
                    opaque.len() - 1
                });
                opaque[idx].verts.extend_from_slice(&verts);
                continue;
            }
            match scene.batches.last_mut() {
                Some(b) if b.texture == tex_id && b.blend == blend && last_shade == key => b.verts.extend_from_slice(&verts),
                _ => scene.batches.push(ling_gl::Batch { texture: tex_id, blend, fog, lighting, verts: verts.to_vec() }),
            }
            last_shade = key;
        }
        // Opaque first (writes depth), then blended geometry back-to-front.
        opaque.append(&mut scene.batches);
        scene.batches = opaque;
        scene
    }
}

#[cfg(all(feature = "gl", not(target_arch = "wasm32")))]
mod gl_bridge {
    use super::*;
    use std::collections::HashSet;

    pub struct Bridge {
        pub gpu: Option<ling_gl::Headless>,
        pub known: HashSet<u64>,
        pub frames: u32,
        pub slow: u32,
    }

    thread_local! {
        pub static BRIDGE: std::cell::RefCell<Option<Bridge>> = const { std::cell::RefCell::new(None) };
    }

    /// Enabled by `LING_GPU=1`. Sorted calls are packed into batches (consecutive
    /// calls sharing texture and blend merge) and composited over `buf`.
    pub fn flush(calls: &[DrawCall], buf: &mut [u32], zbuf: Option<&mut Vec<f32>>, w: usize, h: usize) -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*ON.get_or_init(|| std::env::var("LING_GPU").map(|v| v != "0").unwrap_or(false)) {
            return false;
        }
        BRIDGE.with(|cell| {
            let mut slot = cell.borrow_mut();
            let br = slot.get_or_insert_with(|| {
                let gpu = ling_gl::Headless::new();
                if let Some(g) = &gpu {
                    eprintln!("[ling-gl] GPU rasteriser on {}", g.adapter_name());
                } else {
                    eprintln!("[ling-gl] no GPU adapter; using the CPU rasteriser");
                }
                Bridge { gpu, known: HashSet::new(), frames: 0, slow: 0 }
            });
            let Some(gpu) = br.gpu.as_mut() else { return false };
            // Tiny flushes (UI, portraits) cost more in readback than they save.
            if calls.len() < 1500 || w * h < 640 * 360 {
                return false;
            }
            if br.known.len() > 400 {
                br.known.clear();
                gpu.renderer_mut().clear_textures();
            }

            let t_build = std::time::Instant::now();
            let scene = super::gl_scene::build(calls, &mut br.known);
            let build_ms = t_build.elapsed().as_millis();
            let t0 = std::time::Instant::now();
            let n_batches = scene.batches.len();
            let n_tex = scene.textures.len();
            let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match zbuf {
                Some(z) => gpu.render_over_depth(&scene, w, h, buf, z.as_mut_slice()),
                None => gpu.render_over(&scene, w, h, buf),
            }));
            if std::env::var("LING_GPU_LOG").is_ok() {
                eprintln!(
                    "[ling-gl] flush {}x{} calls={} batches={} new_tex={} build {} ms, render {} ms",
                    w, h, calls.len(), n_batches, n_tex, build_ms, t0.elapsed().as_millis()
                );
            }
            match ok {
                Ok(r) => {
                    // Shader/pipeline warm-up makes the first frames slow; only give up on sustained slowness.
                    br.frames += 1;
                    if br.frames > 4 && t0.elapsed().as_millis() > 250 {
                        br.slow += 1;
                        if br.slow >= 5 {
                            eprintln!("[ling-gl] GPU frames stay slow ({} ms); disabling", t0.elapsed().as_millis());
                            br.gpu = None;
                        }
                    } else if br.frames > 4 {
                        br.slow = 0;
                    }
                    r
                },
                Err(_) => {
                    eprintln!("[ling-gl] GPU render panicked; falling back to the CPU rasteriser");
                    br.gpu = None;
                    false
                },
            }
        })
    }
}

#[cfg(all(feature = "gl", not(target_arch = "wasm32")))]
use gl_bridge::flush as gl_flush;

#[cfg(not(target_arch = "wasm32"))]
struct FlushTimer(std::time::Instant);
#[cfg(not(target_arch = "wasm32"))]
impl Drop for FlushTimer {
    fn drop(&mut self) {
        crate::runtime::ling_phase_add(crate::runtime::phase::FLUSH, self.0.elapsed().as_nanos());
    }
}

/// Test/diagnostic switch: disable the early-z prepass.
pub(crate) static EARLY_Z_OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Sort key for back-to-front order: f32 depth → order-preserving u32, inverted
/// so larger depth sorts first; NaN sorts first (painted earliest).
#[inline]
fn depth_key(d: f32) -> u64 {
    let b = d.to_bits();
    let asc = if b >> 31 == 1 { !b } else { b | 0x8000_0000 };
    (!asc) as u64
}

// ── queue storage recycling + strip binning (used by DepthQueue::flush) ──────

thread_local! {
    /// Emptied call vectors kept for reuse (at most two: the queue's own and
    /// the sort's ping-pong target), so steady-state frames never re-grow them.
    static RECYCLED: std::cell::RefCell<Vec<Vec<DrawCall>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// An empty call vector, preferably one with at least `min_cap` capacity left
/// over from an earlier frame.
fn recycle_take(min_cap: usize) -> Vec<DrawCall> {
    RECYCLED
        .with(|r| {
            let mut r = r.borrow_mut();
            let pos = r.iter().position(|v| v.capacity() >= min_cap).or(if r.is_empty() { None } else { Some(r.len() - 1) })?;
            Some(r.swap_remove(pos))
        })
        .unwrap_or_default()
}

fn recycle_put(mut v: Vec<DrawCall>) {
    if v.capacity() == 0 {
        return;
    }
    v.clear();
    RECYCLED.with(|r| {
        let mut r = r.borrow_mut();
        if r.len() < 2 {
            r.push(v);
        } else if let Some(small) = r.iter_mut().min_by_key(|x| x.capacity()) {
            if small.capacity() < v.capacity() {
                *small = v;
            }
        }
    });
}

/// Calls bucketed by the row strips their bounding box touches (CSR layout):
/// strip `s` owns `idx[off[s]..off[s+1]]`, in queue (= painter) order.
#[cfg(not(target_arch = "wasm32"))]
struct Bins {
    off: Vec<u32>,
    idx: Vec<u32>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Bins {
    fn build(calls: &[DrawCall], order: Option<&[u32]>, height: usize, sh: usize, strips: usize) -> Self {
        // Conservative row range per call (+-2 rows: AA lines plot a pixel past
        // the ideal line; triangles round to pixel centres). Non-finite input is
        // rejected by every rasteriser, so it is simply left out of all bins.
        let hmax = height as f32 - 1.0;
        let span = |c: &DrawCall| -> Option<(u32, u32)> {
            let (lo, hi) = match c.kind {
                DrawKind::Triangle { y0, y1, y2, .. } | DrawKind::TriangleG { y0, y1, y2, .. } => {
                    (y0.min(y1).min(y2), y0.max(y1).max(y2))
                },
                DrawKind::Line { y0, y1, .. } => (y0.min(y1), y0.max(y1)),
                DrawKind::TriangleT { ref v, .. } => (v[0].y.min(v[1].y).min(v[2].y), v[0].y.max(v[1].y).max(v[2].y)),
            };
            if !(lo.is_finite() && hi.is_finite()) || hi < -2.0 || lo > hmax + 2.0 {
                return None;
            }
            let r0 = (lo - 2.0).clamp(0.0, hmax) as usize;
            let r1 = (hi + 2.0).clamp(0.0, hmax) as usize;
            Some(((r0 / sh) as u32, (r1 / sh) as u32))
        };
        // spans[k] belongs to the k-th call in painter order
        let at = |k: usize| order.map_or(k, |o| o[k] as usize);
        let spans: Vec<Option<(u32, u32)>> = (0..calls.len()).map(|k| span(&calls[at(k)])).collect();
        let mut off = vec![0u32; strips + 1];
        for (s0, s1) in spans.iter().flatten() {
            for s in *s0..=*s1 {
                off[s as usize + 1] += 1;
            }
        }
        for s in 0..strips {
            off[s + 1] += off[s];
        }
        let mut fill = off.clone();
        let mut idx = vec![0u32; off[strips] as usize];
        for (k, sp) in spans.iter().enumerate() {
            let i = at(k);
            if let Some((s0, s1)) = sp {
                for s in *s0..=*s1 {
                    let f = &mut fill[s as usize];
                    idx[*f as usize] = i as u32;
                    *f += 1;
                }
            }
        }
        Bins { off, idx }
    }

    #[inline]
    fn of(&self, s: usize) -> &[u32] {
        &self.idx[self.off[s] as usize..self.off[s + 1] as usize]
    }
}

/// Tagged draw call stored in the queue.
#[derive(Debug, Clone)]
pub struct DrawCall {
    /// Camera-space z of the face/edge centroid — larger = further away.
    pub depth: f32,
    /// Pre-lit 0x00RRGGBB colour.
    pub color: u32,
    /// Blend mode (0 normal · 1 add · 2 multiply · 3 screen · 4 subtract · 5 overlay).
    pub mode: u8,
    /// Pen opacity 0..1 (coverage for the composite).
    pub alpha: f32,
    /// Tag written pixels [`crate::gfx::UNLIT`] (flat-shaded Gouraud triangles
    /// only) so the toon post-process leaves them exact instead of re-shading
    /// colour that was deliberately drawn unlit.
    pub unlit: bool,
    pub kind: DrawKind,
}

#[derive(Debug, Clone)]
pub enum DrawKind {
    Triangle {
        x0: f32,
        y0: f32,
        z0: f32,
        x1: f32,
        y1: f32,
        z1: f32,
        x2: f32,
        y2: f32,
        z2: f32,
    },
    /// Gouraud-interpolated + per-pixel posterised triangle (smooth cel).
    /// `bands < 2` disables posterisation (smooth Gouraud, unchanged look for
    /// callers that never asked for toon bands). `softness` crossfades between
    /// the posterised and raw interpolated colour (0 = crisp bands, 1 = smooth).
    TriangleG {
        x0: f32,
        y0: f32,
        z0: f32,
        c0: u32,
        x1: f32,
        y1: f32,
        z1: f32,
        c1: u32,
        x2: f32,
        y2: f32,
        z2: f32,
        c2: u32,
        bands: u32,
        softness: f32,
    },
    Line {
        x0: f32,
        y0: f32,
        z0: f32,
        x1: f32,
        y1: f32,
        z1: f32,
    },
    /// Textured, perspective-correct triangle (game3d `tm_*` / `pfx_*` builtins):
    /// bilinear texel × `tint`, texel alpha × pen alpha, an optional soft see-through
    /// circle (`hole` = screen x, y, radius px, depth; radius <= 0 = off) that fades the
    /// pixels in front of `depth` around a point, and `zwrite` for opaque surfaces.
    TriangleT {
        v: [TexVert; 3],
        tex: std::sync::Arc<TexData>,
        tint: u32,
        hole: [f32; 4],
        zwrite: bool,
        shade: Option<std::sync::Arc<TexShade>>,
    },
}

/// One screen-space vertex of a textured triangle: pixel x/y, camera depth z (the
/// z-buffer key), and 1/w, u/w, v/w for perspective-correct interpolation.
#[derive(Debug, Clone, Copy, Default)]
pub struct TexVert {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub iw: f32,
    pub uw: f32,
    pub vw: f32,
    /// World position times 1/w (perspective-correct per-pixel lighting).
    pub wx: f32,
    pub wy: f32,
    pub wz: f32,
}

/// Per-pixel lighting and fog for a textured triangle: ambient level, the point
/// lights near it (x, y, z, r, g, b, intensity, radius) and distance fog
/// (start, end, colour; end <= 0 = off).
#[derive(Debug, Clone, Default)]
pub struct TexShade {
    pub amb: f32,
    pub lights: Vec<[f32; 8]>,
    pub fog_start: f32,
    pub fog_end: f32,
    pub fog_color: u32,
    pub lit: bool,
    /// Floor shockwaves, each `[cx, cz, front radius, amplitude, wavelength, ring width]` in world
    /// units on the XZ plane: per pixel they bend the texture lookup (refraction) and shade the
    /// wave's slopes (a bump map), see `fill_triangle_tex_shaded`.
    pub ripples: Vec<[f32; 6]>,
}

/// A texture for [`DrawKind::TriangleT`]: power-of-two size, 0xAARRGGBB texels.
/// Build with [`TexData::new`] so `opaque` is right.
pub struct TexData {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u32>,
    /// Every texel has alpha 255 — lets the z-buffered flush depth-prepass the
    /// texture (early-z) since none of its pixels can be see-through.
    pub opaque: bool,
}

impl TexData {
    pub fn new(w: u32, h: u32, px: Vec<u32>) -> Self {
        let opaque = !px.is_empty() && px.iter().all(|p| p >> 24 == 0xFF);
        TexData { w, h, px, opaque }
    }
}

impl std::fmt::Debug for TexData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TexData({}x{})", self.w, self.h)
    }
}

/// Deferred depth-sorted draw queue.
#[derive(Debug)]
pub struct DepthQueue {
    pub(crate) calls: Vec<DrawCall>,
    /// Current blend mode applied to subsequent pushes (mirrors `gfx.blend`).
    cur_mode: u8,
    /// Current pen alpha applied to subsequent pushes (mirrors `gfx.alpha`).
    cur_alpha: f32,
}

impl Default for DepthQueue {
    fn default() -> Self {
        Self { calls: Vec::new(), cur_mode: 0, cur_alpha: 1.0 }
    }
}

impl DepthQueue {
    /// Mirror the live pen blend mode + alpha so the next pushes capture them.
    /// Call after `std::mem::take` so an active blend survives a mid-frame flush.
    pub fn set_state(&mut self, mode: u8, alpha: f32) {
        self.cur_mode = mode;
        self.cur_alpha = alpha.clamp(0.0, 1.0);
    }

    /// Queue a filled triangle (flat per-vertex depth = the sort key).
    #[allow(clippy::too_many_arguments)]
    pub fn push_triangle(
        &mut self,
        depth: f32,
        color: u32,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    ) {
        self.push_call(DrawCall {
            depth,
            color,
            mode: self.cur_mode,
            alpha: self.cur_alpha,
            unlit: false,
            kind: DrawKind::Triangle { x0, y0, z0: depth, x1, y1, z1: depth, x2, y2, z2: depth },
        });
    }

    /// Queue a textured triangle (see [`DrawKind::TriangleT`]); the sort key is the
    /// mean vertex depth, blend mode and alpha come from the current pen state.
    pub fn push_triangle_tex(
        &mut self,
        v: [TexVert; 3],
        tex: std::sync::Arc<TexData>,
        tint: u32,
        hole: [f32; 4],
        zwrite: bool,
    ) {
        self.push_triangle_tex_shaded(v, tex, tint, hole, zwrite, None);
    }

    /// Move the last `n` queued calls `bias` units nearer in the back-to-front order, so a decal
    /// (a shadow on a translucent floor) draws after the big triangles it lies on.
    pub fn bias_last(&mut self, n: usize, bias: f32) {
        let k = self.calls.len().saturating_sub(n);
        for c in &mut self.calls[k..] {
            c.depth -= bias;
        }
    }

    /// As [`Self::push_triangle_tex`] with lighting / fog. Translucent triangles
    /// (no z-write) sort after every opaque call, so they are depth-tested
    /// against the finished opaque scene and blend over it: a translucent wall
    /// in front of the ball tints it, one behind it is hidden.
    #[allow(clippy::too_many_arguments)]
    pub fn push_triangle_tex_shaded(
        &mut self,
        v: [TexVert; 3],
        tex: std::sync::Arc<TexData>,
        tint: u32,
        hole: [f32; 4],
        zwrite: bool,
        shade: Option<std::sync::Arc<TexShade>>,
    ) {
        let mut depth = (v[0].z + v[1].z + v[2].z) / 3.0;
        if !zwrite {
            depth -= 1.0e6;
        }
        self.push_call(DrawCall {
            depth,
            color: tint,
            mode: self.cur_mode,
            alpha: self.cur_alpha,
            unlit: true,
            kind: DrawKind::TriangleT { v, tex, tint, hole, zwrite, shade },
        });
    }

    /// Queue a filled triangle with true per-vertex camera-space depth, so the
    /// per-pixel z-buffer can resolve interpenetration.
    #[allow(clippy::too_many_arguments)]
    pub fn push_triangle_zv(
        &mut self,
        color: u32,
        x0: f32,
        y0: f32,
        z0: f32,
        x1: f32,
        y1: f32,
        z1: f32,
        x2: f32,
        y2: f32,
        z2: f32,
    ) {
        let depth = (z0 + z1 + z2) / 3.0;
        self.push_call(DrawCall {
            depth,
            color,
            mode: self.cur_mode,
            alpha: self.cur_alpha,
            unlit: false,
            kind: DrawKind::Triangle { x0, y0, z0, x1, y1, z1, x2, y2, z2 },
        });
    }

    /// Queue a Gouraud + posterised triangle (smooth cel), flat per-vertex depth.
    #[allow(clippy::too_many_arguments)]
    pub fn push_triangle_g(
        &mut self,
        depth: f32,
        x0: f32,
        y0: f32,
        c0: u32,
        x1: f32,
        y1: f32,
        c1: u32,
        x2: f32,
        y2: f32,
        c2: u32,
        bands: u32,
        unlit: bool,
    ) {
        self.push_call(DrawCall {
            depth,
            color: c0,
            mode: self.cur_mode,
            alpha: self.cur_alpha,
            unlit,
            kind: DrawKind::TriangleG {
                x0,
                y0,
                z0: depth,
                c0,
                x1,
                y1,
                z1: depth,
                c1,
                x2,
                y2,
                z2: depth,
                c2,
                bands,
                softness: 0.0,
            },
        });
    }

    /// Gouraud triangle with true per-vertex depth (for the z-buffer path).
    #[allow(clippy::too_many_arguments)]
    pub fn push_triangle_g_zv(
        &mut self,
        x0: f32,
        y0: f32,
        z0: f32,
        c0: u32,
        x1: f32,
        y1: f32,
        z1: f32,
        c1: u32,
        x2: f32,
        y2: f32,
        z2: f32,
        c2: u32,
        bands: u32,
        unlit: bool,
    ) {
        self.push_triangle_g_zv_soft(
            x0, y0, z0, c0, x1, y1, z1, c1, x2, y2, z2, c2, bands, 0.0, unlit,
        );
    }

    /// `push_triangle_g_zv` with an explicit band-crossfade `softness`
    /// (0 = crisp bands, 1 = fully smooth). See `DrawKind::TriangleG`.
    #[allow(clippy::too_many_arguments)]
    pub fn push_triangle_g_zv_soft(
        &mut self,
        x0: f32,
        y0: f32,
        z0: f32,
        c0: u32,
        x1: f32,
        y1: f32,
        z1: f32,
        c1: u32,
        x2: f32,
        y2: f32,
        z2: f32,
        c2: u32,
        bands: u32,
        softness: f32,
        unlit: bool,
    ) {
        let depth = (z0 + z1 + z2) / 3.0;
        self.push_call(DrawCall {
            depth,
            color: c0,
            mode: self.cur_mode,
            alpha: self.cur_alpha,
            unlit,
            kind: DrawKind::TriangleG {
                x0,
                y0,
                z0,
                c0,
                x1,
                y1,
                z1,
                c1,
                x2,
                y2,
                z2,
                c2,
                bands,
                softness,
            },
        });
    }

    /// Queue a line segment (flat per-vertex depth).
    pub fn push_line(&mut self, depth: f32, color: u32, x0: f32, y0: f32, x1: f32, y1: f32) {
        // Global wireframe hue-cycle: when enabled, override every line stroke's
        // colour with a rapidly time-cycling rainbow. The screen position adds a
        // spatial phase so strokes spread across the spectrum instead of flashing
        // as one flat colour.
        let color = match crate::runtime::line_hue_phase() {
            Some(ph) => {
                let p = (ph + (x0 + y0) as f64 * 0.006) as f32;
                let r = ((p.sin() * 0.5 + 0.5) * 255.0) as u32;
                let g = (((p + 2.0944).sin() * 0.5 + 0.5) * 255.0) as u32;
                let b = (((p + 4.1888).sin() * 0.5 + 0.5) * 255.0) as u32;
                (r << 16) | (g << 8) | b
            },
            None => color,
        };
        self.push_call(DrawCall {
            depth,
            color,
            mode: self.cur_mode,
            alpha: self.cur_alpha,
            unlit: false,
            kind: DrawKind::Line { x0, y0, z0: depth, x1, y1, z1: depth },
        });
    }

    /// Sort back-to-front and rasterise everything into `buf`.
    ///
    /// `zbuf`: when `Some`, a per-pixel depth buffer (camera-space z, smaller =
    /// nearer) is used so interpenetrating triangles resolve correctly — a true
    /// z-buffer on top of the painter's sort. When `None`, pure painter's
    /// algorithm (the default/legacy path).
    ///
    /// Opaque calls (mode 0, alpha ≈ 1) take the fast direct-write path; calls
    /// with a blend mode or alpha < 1 composite via `composite_pixel`. In the
    /// z-buffer path, translucent calls test depth but do not write it, so they
    /// layer over the opaque scene (back-to-front sort handles their ordering).
    ///
    /// Consumes `self` — call site does `mem::take` to avoid borrow conflict.
    #[allow(clippy::ptr_arg)]
    pub fn flush(
        mut self,
        buf: &mut Vec<u32>,
        zbuf: Option<&mut Vec<f32>>,
        reset_z: bool,
        width: usize,
        height: usize,
        aa: bool,
    ) {
        // Sort largest depth first (furthest → painted first, nearest on top).
        // With a z-buffer the sort still helps transparency.
        // STABLE order: equal-depth calls keep submission order every frame, so
        // co-planar / same-depth overlapping primitives (e.g. adjacent boot/title
        // glyphs at z=0) never swap draw order frame-to-frame — kills the
        // "lit↔unlit" flicker that an unstable sort produced on ties.
        #[cfg(not(target_arch = "wasm32"))]
        let _s = std::time::Instant::now();
        #[cfg(all(feature = "gl", not(target_arch = "wasm32")))]
        {
            // the GPU bridge consumes a sorted slice
            self.sort_back_to_front();
            crate::runtime::ling_phase_add(crate::runtime::phase::SORT, _s.elapsed().as_nanos());
            // The GPU path keeps its own depth; still reset the CPU z-buffer so
            // geometry drawn later on the CPU (portraits, UI models) isn't occluded by last frame's depth.
            let mut zb = zbuf;
            if let Some(z) = zb.as_deref_mut() {
                if z.len() != width * height {
                    z.clear();
                    z.resize(width * height, f32::INFINITY);
                } else if reset_z {
                    z.iter_mut().for_each(|v| *v = f32::INFINITY);
                }
            }
            if gl_flush(&self.calls, buf, zb.as_deref_mut(), width, height) {
                // The GPU drew the triangles; lines still go through the CPU path.
                let _tl = std::time::Instant::now();
                let mut n_lines = 0usize;
                for call in &self.calls {
                    if matches!(call.kind, DrawKind::Line { .. }) {
                        n_lines += 1;
                        rasterize_call(call, buf, None, width, height, 0.0, aa);
                    }
                }
                if std::env::var("LING_GPU_LOG").is_ok() {
                    eprintln!("[ling-gl]   sort {} ms, {} cpu lines {} ms", _s.elapsed().as_millis() - _tl.elapsed().as_millis(), n_lines, _tl.elapsed().as_millis());
                }
                self.recycle();
                return;
            }
            // the z-buffer was already reset above
            self.flush_cpu(None, buf, zb, false, width, height, aa);
            self.recycle();
            return;
        }
        #[cfg(not(all(feature = "gl", not(target_arch = "wasm32"))))]
        {
            // Web: the scene is drawn by WebGL2 over the finished CPU frame at present time.
            #[cfg(all(feature = "gl-web", target_arch = "wasm32"))]
            if self.calls.len() >= 1500 && width * height >= 640 * 360 && crate::gfx::webgl_scene::ready() {
                self.sort_back_to_front();
            }
            #[cfg(all(feature = "gl-web", target_arch = "wasm32"))]
            if self.calls.len() >= 1500 && width * height >= 640 * 360 && crate::gfx::webgl_scene::ready() {
                let mut zb = zbuf;
                if let Some(z) = zb.as_deref_mut() {
                    if z.len() != width * height {
                        z.clear();
                        z.resize(width * height, f32::INFINITY);
                    } else if reset_z {
                        z.iter_mut().for_each(|v| *v = f32::INFINITY);
                    }
                }
                crate::gfx::webgl_scene::submit(&self.calls);
                for call in &self.calls {
                    if matches!(call.kind, DrawKind::Line { .. }) {
                        rasterize_call(call, buf, None, width, height, 0.0, aa);
                    }
                }
                self.recycle();
                return;
            }
            // CPU raster: walk the calls through a sorted index order — no need
            // to move them (each is 168 bytes; ~5 MB of copying for 30k calls)
            let order = self.sorted_order();
            #[cfg(not(target_arch = "wasm32"))]
            crate::runtime::ling_phase_add(crate::runtime::phase::SORT, _s.elapsed().as_nanos());
            self.flush_cpu(order.as_deref(), buf, zbuf, reset_z, width, height, aa);
            self.recycle();
        }
    }

    /// Back-to-front (stable) index order of the queued calls, or `None` when
    /// they are already in order (common for 2-D UI / text at one depth).
    fn sorted_order(&self) -> Option<Vec<u32>> {
        let n = self.calls.len();
        if n < 2 {
            return None;
        }
        let mut keys: Vec<u64> = self.calls.iter().enumerate().map(|(i, c)| depth_key(c.depth) << 32 | i as u64).collect();
        if keys.windows(2).all(|w| w[0] <= w[1]) {
            return None;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if n > 16_384 {
            keys.par_sort_unstable();
        } else {
            keys.sort_unstable();
        }
        #[cfg(target_arch = "wasm32")]
        keys.sort_unstable();
        Some(keys.into_iter().map(|k| k as u32).collect())
    }

    /// Stable back-to-front order without moving 168-byte calls n·log n times:
    /// sort packed `(depth key, submission index)` u64s, then move every call
    /// exactly once into a recycled vector in that order.
    #[cfg_attr(not(any(feature = "gl", target_arch = "wasm32")), allow(dead_code))] // GPU / WebGL bridges
    fn sort_back_to_front(&mut self) {
        let n = self.calls.len();
        if n < 2 {
            return;
        }
        if n < 64 {
            self.calls.sort_by(|a, b| b.depth.partial_cmp(&a.depth).unwrap_or(std::cmp::Ordering::Equal));
            return;
        }
        let mut keys: Vec<u64> = self.calls.iter().enumerate().map(|(i, c)| depth_key(c.depth) << 32 | i as u64).collect();
        // already in order (common for 2-D UI / text at one depth): nothing to move
        if keys.windows(2).all(|w| w[0] <= w[1]) {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if n > 16_384 {
            keys.par_sort_unstable();
        } else {
            keys.sort_unstable();
        }
        #[cfg(target_arch = "wasm32")]
        keys.sort_unstable();
        let mut sorted = recycle_take(n);
        sorted.reserve(n);
        let src = self.calls.as_ptr();
        // SAFETY: `keys` holds every index in 0..n exactly once, so each call is
        // read (moved) exactly once; `set_len(0)` then forgets the moved-from
        // originals so nothing is dropped twice. `sorted` has capacity >= n, so
        // the pushes cannot reallocate or panic in between.
        unsafe {
            for k in &keys {
                sorted.push(std::ptr::read(src.add((*k & 0xFFFF_FFFF) as usize)));
            }
            self.calls.set_len(0);
        }
        let old = std::mem::replace(&mut self.calls, sorted);
        recycle_put(old);
    }

    /// Return this queue's (now empty) storage to the per-thread pool, so next
    /// frame's queue starts with last frame's capacity instead of re-growing
    /// (and page-faulting) a multi-MB vector from zero.
    fn recycle(mut self) {
        self.calls.clear();
        recycle_put(std::mem::take(&mut self.calls));
    }

    #[inline]
    fn push_call(&mut self, c: DrawCall) {
        if self.calls.capacity() == 0 {
            self.calls = recycle_take(0);
        }
        self.calls.push(c);
    }

    #[allow(clippy::ptr_arg)]
    fn flush_cpu(
        &mut self,
        order: Option<&[u32]>,
        buf: &mut Vec<u32>,
        zbuf: Option<&mut Vec<f32>>,
        reset_z: bool,
        width: usize,
        height: usize,
        aa: bool,
    ) {
        #[cfg(not(target_arch = "wasm32"))]
        let _r = std::time::Instant::now();
        #[cfg(not(target_arch = "wasm32"))]
        let _guard = FlushTimer(_r);
        let calls = &self.calls;
        let n = width * height;
        if width == 0 || height == 0 || buf.len() < n {
            return;
        }
        let mut zb = zbuf;
        let mut reset_z = reset_z;
        if let Some(z) = zb.as_deref_mut() {
            if z.len() != n {
                z.clear();
                z.resize(n, f32::INFINITY);
                reset_z = false;
            }
        }
        // Binned parallel raster: the frame is cut into short row strips; every
        // call is assigned (once) to the strips its bounding rows touch, and the
        // strips rasterise concurrently, each walking only its own calls in the
        // sorted order — so painter's order per pixel is preserved, a pixel is
        // touched by exactly one thread, and no strip re-scans the whole queue.
        #[cfg(not(target_arch = "wasm32"))]
        if calls.len() >= 48 && height >= 64 && rayon::current_num_threads() > 1 {
            let threads = rayon::current_num_threads();
            let sh = (height / (threads * 3)).clamp(8, 64);
            let strips = height.div_ceil(sh);
            let bins = Bins::build(calls, order, height, sh, strips);
            let raster_strip = |s: usize, sbuf: &mut [u32], sz: Option<&mut [f32]>| {
                let bh = sbuf.len() / width;
                let ysh = (s * sh) as f32;
                let mut sz = sz;
                if reset_z {
                    if let Some(z) = sz.as_deref_mut() {
                        z.fill(f32::INFINITY);
                    }
                }
                let bin = bins.of(s);
                // Early-z: when opaque gouraud / textured calls overdraw this strip
                // (> ~1.5 layers), record their nearest depth first, so the ordered
                // pass below shades each visible pixel once. Below that the extra
                // depth-only pass costs more than the shading it saves.
                let mut prepass = false;
                if let Some(z) = sz.as_deref_mut() {
                    let (r0, r1) = (ysh, ysh + bh as f32);
                    let area: f32 = bin.iter().map(|&ci| &calls[ci as usize]).filter(|c| prepass_eligible(c)).map(|c| call_area_in_rows(c, width, r0, r1)).sum();
                    if area > 1.5 * (width * bh) as f32 && !EARLY_Z_OFF.load(std::sync::atomic::Ordering::Relaxed) {
                        prepass = true;
                        for &ci in bin {
                            let c = &calls[ci as usize];
                            if prepass_eligible(c) {
                                prepass_call(c, z, width, bh, ysh);
                            }
                        }
                    }
                }
                for &ci in bin {
                    let c = &calls[ci as usize];
                    rasterize_call_z(c, sbuf, sz.as_deref_mut(), width, bh, ysh, aa, prepass && prepass_eligible(c));
                }
            };
            match zb {
                Some(z) => {
                    buf[..n]
                        .par_chunks_mut(sh * width)
                        .zip(z[..n].par_chunks_mut(sh * width))
                        .enumerate()
                        .with_max_len(1)
                        .for_each(|(s, (b, z))| raster_strip(s, b, Some(z)));
                },
                None => {
                    buf[..n]
                        .par_chunks_mut(sh * width)
                        .enumerate()
                        .with_max_len(1)
                        .for_each(|(s, b)| raster_strip(s, b, None));
                },
            }
            return;
        }
        let n_calls = calls.len();
        let at = |k: usize| &calls[order.map_or(k, |o| o[k] as usize)];
        match zb {
            Some(z) => {
                if reset_z {
                    z.iter_mut().for_each(|v| *v = f32::INFINITY);
                }
                for k in 0..n_calls {
                    rasterize_call(at(k), buf, Some(z), width, height, 0.0, aa);
                }
            },
            None => {
                for k in 0..n_calls {
                    rasterize_call(at(k), buf, None, width, height, 0.0, aa);
                }
            },
        }
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// Consume the queue and send all draw calls to the WebGL backend.
    /// Only compiled for wasm32 targets.
    #[cfg(target_arch = "wasm32")]
    pub fn flush_to_webgl(
        mut self,
        fill_r: f32,
        fill_g: f32,
        fill_b: f32,
        width: usize,
        height: usize,
    ) {
        // Sort back-to-front (painter's algorithm) — same as the native path.
        self.calls.sort_unstable_by(|a, b| {
            b.depth
                .partial_cmp(&a.depth)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for call in &self.calls {
            match call.kind {
                DrawKind::Triangle { x0, y0, x1, y1, x2, y2, .. } => {
                    crate::gfx::webgl::push_triangle(call.color, x0, y0, x1, y1, x2, y2, call.depth)
                },
                DrawKind::TriangleG { x0, y0, c0, x1, y1, c1, x2, y2, c2, .. } => {
                    // WebGL path: approximate with the averaged vertex colour.
                    let avg = {
                        let r = ((c0 >> 16 & 0xFF) + (c1 >> 16 & 0xFF) + (c2 >> 16 & 0xFF)) / 3;
                        let g = ((c0 >> 8 & 0xFF) + (c1 >> 8 & 0xFF) + (c2 >> 8 & 0xFF)) / 3;
                        let b = ((c0 & 0xFF) + (c1 & 0xFF) + (c2 & 0xFF)) / 3;
                        (r << 16) | (g << 8) | b
                    };
                    crate::gfx::webgl::push_triangle(avg, x0, y0, x1, y1, x2, y2, call.depth);
                },
                DrawKind::Line { x0, y0, x1, y1, .. } => {
                    crate::gfx::webgl::push_line(call.color, x0, y0, x1, y1, call.depth)
                },
                DrawKind::TriangleT { ref v, tint, .. } => crate::gfx::webgl::push_triangle(
                    tint, v[0].x, v[0].y, v[1].x, v[1].y, v[2].x, v[2].y, call.depth,
                ),
            }
        }
        crate::gfx::webgl::flush(fill_r, fill_g, fill_b, width, height);
    }

    /// Consume the queue and rasterise it on the GPU (wgpu) into `buf`.
    /// Native analogue of `flush_to_webgl`: every call is already in screen
    /// space, so we expand triangles to a vertex list and let the GPU fill
    /// them, reading the result back into `buf`. Returns `false` if no GPU is
    /// available (caller falls back to the CPU `flush`). Lines are not yet
    /// emitted on this path.
    #[cfg(feature = "gpu")]
    pub fn flush_to_wgpu(
        mut self,
        buf: &mut Vec<u32>,
        width: usize,
        height: usize,
        clear: [f32; 3],
    ) -> bool {
        use crate::gfx::wgpu_raster::Vert;
        self.calls.sort_unstable_by(|a, b| {
            b.depth
                .partial_cmp(&a.depth)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let to_rgb = |c: u32| {
            [
                ((c >> 16) & 0xFF) as f32 / 255.0,
                ((c >> 8) & 0xFF) as f32 / 255.0,
                (c & 0xFF) as f32 / 255.0,
            ]
        };
        let mut verts: Vec<Vert> = Vec::with_capacity(self.calls.len() * 3);
        for call in &self.calls {
            match call.kind {
                DrawKind::Triangle { x0, y0, x1, y1, x2, y2, .. } => {
                    let c = to_rgb(call.color);
                    verts.push(Vert { pos: [x0, y0], color: c });
                    verts.push(Vert { pos: [x1, y1], color: c });
                    verts.push(Vert { pos: [x2, y2], color: c });
                },
                DrawKind::TriangleG { x0, y0, c0, x1, y1, c1, x2, y2, c2, .. } => {
                    verts.push(Vert { pos: [x0, y0], color: to_rgb(c0) });
                    verts.push(Vert { pos: [x1, y1], color: to_rgb(c1) });
                    verts.push(Vert { pos: [x2, y2], color: to_rgb(c2) });
                },
                DrawKind::Line { .. } => { /* TODO: emit lines as thin quads on the GPU path */ },
                DrawKind::TriangleT { ref v, tint, .. } => {
                    let c = to_rgb(tint);
                    for k in v {
                        verts.push(Vert { pos: [k.x, k.y], color: c });
                    }
                },
            }
        }
        if buf.len() < width * height {
            buf.resize(width * height, 0);
        }
        crate::gfx::wgpu_raster::raster(&verts, width, height, clear, buf)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct R(u64);
    impl R {
        fn f(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
    }

    /// A deliberately nasty mixed scene: heavy opaque overdraw (gouraud + opaque
    /// texture → early-z kicks in), translucent and additive triangles, a
    /// see-through texture, AA lines crossing strip seams, and off-screen junk.
    fn scene(w: usize, h: usize) -> DepthQueue {
        let mut r = R(0xD1B54A32D192ED03);
        let mut q = DepthQueue::default();
        let opaque = Arc::new(TexData::new(64, 64, (0..64 * 64).map(|i| 0xFF00_0000 | (i as u32 * 2654435761) & 0xFFFFFF).collect()));
        let seethru = Arc::new(TexData::new(64, 64, (0..64 * 64).map(|i| ((i as u32 & 255) << 24) | 0x8040C0).collect()));
        assert!(opaque.opaque && !seethru.opaque);
        for k in 0..900 {
            let s = [8.0, 40.0, 200.0][k % 3];
            let (cx, cy) = (r.f() * w as f32, r.f() * h as f32);
            let mut v = [0f32; 9];
            for j in 0..3 {
                v[j * 3] = cx + (r.f() - 0.5) * s;
                v[j * 3 + 1] = cy + (r.f() - 0.5) * s;
                v[j * 3 + 2] = 1.0 + r.f() * 40.0;
            }
            let col = (r.f() * 16777215.0) as u32;
            match k % 9 {
                0..=3 => q.push_triangle_g_zv(v[0], v[1], v[2], col, v[3], v[4], v[5], col ^ 0x404040, v[6], v[7], v[8], col ^ 0x202020, 4, false),
                4 => q.push_triangle_zv(col, v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8]),
                5 | 6 => {
                    let tv = |i: usize, u: f32, vv: f32| {
                        let iw = 1.0 / (v[i * 3 + 2] + 5.0);
                        TexVert { x: v[i * 3], y: v[i * 3 + 1], z: v[i * 3 + 2], iw, uw: u * iw, vw: vv * iw, wx: 0.0, wy: 0.0, wz: 0.0 }
                    };
                    let tex = if k % 9 == 5 { opaque.clone() } else { seethru.clone() };
                    q.push_triangle_tex([tv(0, 0.0, 0.0), tv(1, 1.0, 0.0), tv(2, 0.5, 1.0)], tex, 0xFFFFFF, [0.0; 4], true);
                },
                7 => {
                    q.set_state(if r.f() < 0.5 { 1 } else { 0 }, 0.5);
                    q.push_triangle_zv(col, v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8]);
                    q.set_state(0, 1.0);
                },
                _ => q.push_line(v[2], col, v[0], v[1], v[3] + (r.f() - 0.5) * 900.0, v[4] + (r.f() - 0.5) * 900.0),
            }
        }
        q.push_triangle_zv(0xFFFFFF, f32::NAN, 0.0, 1.0, 5.0, 5.0, 1.0, 9.0, 0.0, 1.0);
        q.push_line(1.0, 0xFF0000, -1e9, 10.0, 1e9, 20.0);
        q
    }

    fn render(threads: usize, zbuf: bool, aa: bool) -> (Vec<u32>, Vec<f32>) {
        let (w, h) = (517usize, 389usize);
        crate::gfx::raster::set_persp_zd(5.0);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        pool.install(|| {
            let mut buf = vec![0x0010_2030u32; w * h];
            let mut z = vec![f32::INFINITY; w * h];
            scene(w, h).flush(&mut buf, if zbuf { Some(&mut z) } else { None }, true, w, h, aa);
            (buf, z)
        })
    }

    #[test]
    fn binned_early_z_flush_matches_serial_flush() {
        for (zbuf, aa) in [(true, false), (true, true), (false, false), (false, true)] {
            let (b1, z1) = render(1, zbuf, aa); // serial path: no strips, no prepass
            EARLY_Z_OFF.store(true, std::sync::atomic::Ordering::Relaxed);
            let (bn, zn) = render(8, zbuf, aa);
            EARLY_Z_OFF.store(false, std::sync::atomic::Ordering::Relaxed);
            let (b8, z8) = render(8, zbuf, aa); // binned strips (+ early-z with a z-buffer)
            let zd = |a: &[f32], b: &[f32]| a.iter().zip(b).filter(|(x, y)| x.to_bits() != y.to_bits()).count();
            eprintln!("zbuf={zbuf} aa={aa}: serial vs binned(no early-z) {} px / {} z, binned vs binned+early-z {} px / {} z",
                b1.iter().zip(&bn).filter(|(a, b)| a != b).count(), zd(&z1, &zn), bn.iter().zip(&b8).filter(|(a, b)| a != b).count(), zd(&zn, &z8));
            let diff = b1.iter().zip(&b8).filter(|(a, b)| a != b).count();
            // band-local `y - ysh` arithmetic perturbs barycentrics in the last bits,
            // so depth is compared with a tolerance (colour above is the real check)
            let zdiff = z1.iter().zip(&z8).filter(|(a, b)| a != b && !((*a - *b).abs() <= 1e-4 * a.abs().max(1.0))).count();
            // exact-depth ties between different triangles may resolve either way
            assert!(diff * 1_000 < b1.len(), "zbuf={zbuf} aa={aa}: {diff} pixels differ");
            assert_eq!(zdiff, 0, "zbuf={zbuf} aa={aa}: depth differs at {zdiff} pixels");
        }
    }

    #[test]
    fn queue_storage_is_recycled_across_frames() {
        let mut q = DepthQueue::default();
        for i in 0..10_000 {
            q.push_line(i as f32, 0, 0.0, 0.0, 1.0, 1.0);
        }
        let mut buf = vec![0u32; 64 * 64];
        q.flush(&mut buf, None, false, 64, 64, false);
        let mut q2 = DepthQueue::default();
        q2.push_line(0.0, 0, 0.0, 0.0, 1.0, 1.0);
        assert!(q2.calls.capacity() >= 10_000, "next frame's queue should reuse last frame's storage");
    }
}
