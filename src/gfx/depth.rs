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

/// Number of horizontal bands to rasterise `nrows` rows across. 1 = serial.
///
/// Calls are binned by their y-extent (see [`raster_rows`]), so a band only
/// re-runs setup for the triangles that actually touch it; that makes many
/// thin bands cheap and keeps every core busy (work stealing evens out bands
/// that hold more of the scene).
#[cfg(not(target_arch = "wasm32"))]
fn render_bands(nrows: usize, n_calls: usize) -> usize {
    if nrows < 48 || n_calls < 16 {
        return 1;
    }
    (rayon::current_num_threads() * 3).min(nrows / 24).max(1)
}

/// Conservative screen-y extent of a call (pixels, before any band shift);
/// `None` when a coordinate is not finite (such calls go to every band).
#[cfg(not(target_arch = "wasm32"))]
#[inline]
fn call_y_range(c: &DrawCall) -> Option<(f32, f32)> {
    let (a, b, d) = match c.kind {
        DrawKind::Triangle { y0, y1, y2, .. } | DrawKind::TriangleG { y0, y1, y2, .. } => (y0, y1, y2),
        DrawKind::Line { y0, y1, .. } => (y0, y1, y1),
        DrawKind::TriangleT { ref v, .. } => (v[0].y, v[1].y, v[2].y),
    };
    if !(a.is_finite() && b.is_finite() && d.is_finite()) {
        return None;
    }
    Some((a.min(b).min(d), a.max(b).max(d)))
}

/// Rasterise `calls` (already in draw order) into rows `row0 .. row0 + nrows`
/// of a `width`-wide framebuffer; `buf` / `zbuf` hold exactly those rows.
///
/// The rows are split into horizontal bands rasterised in parallel. Each band
/// owns a disjoint slice of `buf`/`zbuf`, so a pixel is touched by exactly one
/// thread, and each band walks its calls in the sorted order, preserving the
/// painter's per-pixel order. Calls are binned by y-extent first so a band only
/// visits the calls that can touch it.
fn raster_rows(
    calls: &[DrawCall],
    buf: &mut [u32],
    zbuf: Option<&mut [f32]>,
    width: usize,
    row0: usize,
    nrows: usize,
    aa: bool,
) {
    if width == 0 || nrows == 0 {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let bands = render_bands(nrows, calls.len());
        if bands > 1 {
            let rows = nrows.div_ceil(bands);
            let bands = nrows.div_ceil(rows);
            // Margin covers AA edges / line feathering past the vertex extent.
            const MARGIN: f32 = 4.0;
            let lo_lim = row0 as f32;
            let hi_lim = (row0 + nrows) as f32;
            let mut bins: Vec<Vec<u32>> = vec![Vec::new(); bands];
            for (i, c) in calls.iter().enumerate() {
                let (b0, b1) = match call_y_range(c) {
                    Some((lo, hi)) => {
                        let lo = lo - MARGIN;
                        let hi = hi + MARGIN;
                        if hi < lo_lim || lo >= hi_lim {
                            continue; // entirely outside these rows: draws nothing here
                        }
                        let b0 = ((lo - lo_lim).max(0.0) as usize / rows).min(bands - 1);
                        let b1 = ((hi - lo_lim).max(0.0) as usize / rows).min(bands - 1);
                        (b0, b1)
                    },
                    None => (0, bands - 1),
                };
                for bin in &mut bins[b0..=b1] {
                    bin.push(i as u32);
                }
            }
            let job = |b: usize, bbuf: &mut [u32], mut bz: Option<&mut [f32]>| {
                let ysh = (row0 + b * rows) as f32;
                let bh = bbuf.len() / width;
                for &i in &bins[b] {
                    rasterize_call(&calls[i as usize], bbuf, bz.as_deref_mut(), width, bh, ysh, aa);
                }
            };
            match zbuf {
                Some(z) => buf
                    .par_chunks_mut(rows * width)
                    .zip(z.par_chunks_mut(rows * width))
                    .enumerate()
                    .for_each(|(b, (bbuf, bz))| job(b, bbuf, Some(bz))),
                None => buf
                    .par_chunks_mut(rows * width)
                    .enumerate()
                    .for_each(|(b, bbuf)| job(b, bbuf, None)),
            }
            return;
        }
    }
    let ysh = row0 as f32;
    match zbuf {
        Some(z) => {
            for call in calls {
                rasterize_call(call, buf, Some(&mut *z), width, nrows, ysh, aa);
            }
        },
        None => {
            for call in calls {
                rasterize_call(call, buf, None, width, nrows, ysh, aa);
            }
        },
    }
}

/// Reset a z-buffer to "nothing drawn" (+∞), in parallel on native.
fn reset_depth(z: &mut [f32]) {
    #[cfg(not(target_arch = "wasm32"))]
    z.par_chunks_mut(1 << 16).for_each(|c| c.fill(f32::INFINITY));
    #[cfg(target_arch = "wasm32")]
    z.fill(f32::INFINITY);
}

/// Size `z` to `n` (filled with +∞) or reset it when `reset_z`.
fn prepare_depth(z: &mut Vec<f32>, n: usize, reset_z: bool) {
    if z.len() != n {
        z.clear();
        z.resize(n, f32::INFINITY);
    } else if reset_z {
        reset_depth(z);
    }
}

/// Sort key for "largest depth first": ascending u32 order = descending depth,
/// with NaN first and -0/+0 tied. Callers pack it above an index for stability.
#[inline]
fn desc_key(d: f32) -> u32 {
    if d.is_nan() {
        return 0;
    }
    let d = if d == 0.0 { 0.0 } else { d };
    let b = d.to_bits();
    let asc = if b >> 31 == 1 { !b } else { b | 0x8000_0000 };
    !asc
}

/// Order calls back-to-front (largest depth first), ties in submission order:
/// exactly the order of a stable sort on depth, but it sorts packed 8-byte
/// (depth, index) keys and then moves each ~170-byte call once, instead of
/// shuffling the calls themselves through a merge sort (50k calls: ~9 ms → <1 ms).
fn sort_back_to_front(calls: &mut Vec<DrawCall>) {
    let n = calls.len();
    if n < 2 || n > u32::MAX as usize {
        if n >= 2 {
            calls.sort_by(|a, b| b.depth.partial_cmp(&a.depth).unwrap_or(std::cmp::Ordering::Equal));
        }
        return;
    }
    let mut keys: Vec<u64> = SORT_KEYS.with(|k| std::mem::take(&mut *k.borrow_mut()));
    keys.clear();
    keys.extend(
        calls
            .iter()
            .enumerate()
            .map(|(i, c)| ((desc_key(c.depth) as u64) << 32) | i as u64),
    );
    if keys.windows(2).all(|w| w[0] <= w[1]) {
        SORT_KEYS.with(|k| *k.borrow_mut() = keys);
        return; // already in order
    }
    // Keys are unique (index in the low bits), so any sort gives one order.
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        if n >= 16384 {
            keys.par_sort_unstable();
        } else {
            keys.sort_unstable();
        }
    }
    #[cfg(target_arch = "wasm32")]
    keys.sort_unstable();
    // The gather target is a pooled buffer (a fresh multi-MB Vec per flush is
    // mostly page faults); the old `calls` buffer becomes the next target.
    let mut out: Vec<DrawCall> = SORT_OUT.with(|o| std::mem::take(&mut *o.borrow_mut()));
    out.clear();
    out.reserve(n);
    // SAFETY: `keys` holds every index in 0..n exactly once, so each call is
    // moved out of `calls` exactly once into a distinct slot of `out`; `calls`
    // is then emptied without dropping, so nothing is dropped twice. No code
    // between the moves can panic.
    unsafe {
        let src = calls.as_ptr() as usize;
        let dst = out.as_mut_ptr() as usize;
        let gather = |j0: usize, ks: &[u64]| {
            let src = src as *const DrawCall;
            let dst = dst as *mut DrawCall;
            for (j, k) in ks.iter().enumerate() {
                std::ptr::copy_nonoverlapping(src.add((*k & 0xFFFF_FFFF) as usize), dst.add(j0 + j), 1);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            use rayon::prelude::*;
            const CH: usize = 8192;
            if n >= 2 * CH {
                keys.par_chunks(CH).enumerate().for_each(|(c, ks)| gather(c * CH, ks));
            } else {
                gather(0, &keys);
            }
        }
        #[cfg(target_arch = "wasm32")]
        gather(0, &keys);
        calls.set_len(0);
        out.set_len(n);
    }
    std::mem::swap(calls, &mut out);
    SORT_OUT.with(|o| *o.borrow_mut() = out);
    SORT_KEYS.with(|k| *k.borrow_mut() = keys);
}

thread_local! {
    /// Reused sort keys / gather target for [`sort_back_to_front`].
    static SORT_KEYS: std::cell::RefCell<Vec<u64>> = const { std::cell::RefCell::new(Vec::new()) };
    static SORT_OUT: std::cell::RefCell<Vec<DrawCall>> = const { std::cell::RefCell::new(Vec::new()) };
}

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
    if let DrawKind::TriangleT { ref v, ref tex, tint, hole, zwrite, ref shade } = call.kind {
        raster::fill_triangle_tex_shaded(buf, zbuf, width, height, ysh, v, tex, tint, hole, zwrite, call.mode, call.alpha, shade.as_deref());
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

    // Flat verts carry camera-space z; the shader depth is 1/iw and textured verts use 1/(z+zdist),
    // so flat ones need the same offset or they win every depth test against textured geometry.
    fn vert(x: f32, y: f32, z: f32, uw: f32, vw: f32, c: [f32; 3], a: f32, zd: f32) -> ling_gl::Vertex {
        let iw = 1.0 / (z + zd).max(1e-3);
        ling_gl::Vertex { x, y, iw, uw: uw * iw, vw: vw * iw, tint: [c[0], c[1], c[2], a], w: [0.0; 3] }
    }

    /// The set of texture ids the GPU side already holds.
    pub trait TexKnown {
        /// Record `id` as used this frame; true when the GPU doesn't have it yet.
        fn mark(&mut self, id: u64) -> bool;
    }
    impl TexKnown for HashSet<u64> {
        fn mark(&mut self, id: u64) -> bool {
            self.insert(id)
        }
    }

    /// Pack calls into batches. Opaque calls group by texture + shading (the GPU
    /// depth-tests them, so their order is free); blended ones draw back-to-front
    /// (largest depth first, ties in submission order) whatever order `calls` is in,
    /// with consecutive ones sharing texture, blend and shading merged. Textures not
    /// yet in `known` are attached to the scene.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn build<K: TexKnown>(calls: &[DrawCall], known: &mut K) -> ling_gl::Scene {
        build_pooled(calls, known, &mut Vec::new())
    }

    /// Return a drawn scene's vertex Vecs to `pool` for [`build_pooled`], in reverse so
    /// the next frame's batches (usually the same shape) get back their own capacity.
    pub fn recycle(scene: ling_gl::Scene, pool: &mut Vec<Vec<ling_gl::Vertex>>) {
        for b in scene.batches.into_iter().rev() {
            if pool.len() < 4096 {
                pool.push(b.verts);
            }
        }
    }

    /// [`build`] taking batch vertex Vecs from `pool`, so steady frames don't
    /// allocate (and page-fault in) megabytes of fresh vertex memory.
    pub fn build_pooled<'a, K: TexKnown>(calls: &'a [DrawCall], known: &mut K, pool: &mut Vec<Vec<ling_gl::Vertex>>) -> ling_gl::Scene {
        let mut scene = ling_gl::Scene::new([0.0; 3]);
        scene.clear_alpha = 0.0;
        let zd = super::raster::persp_zd();
        let zdist = if zd.is_finite() { zd } else { 0.0 };
        scene.zdist = zdist;
        let mut textures: Vec<ling_gl::Texture> = Vec::new();
        // Texture id per Arc address. Within one build every call holds its Arc,
        // so an address names one live texture: hash its content once, not per call.
        let mut tex_ids: std::collections::HashMap<usize, u64> = std::collections::HashMap::new();
        let mut last_tex: (usize, u64) = (0, 0);
        // Fog + lighting of a call, only materialised when it opens a new batch.
        let shading = |sh: Option<&TexShade>| -> (ling_gl::Fog, ling_gl::Lighting) {
            match sh {
                Some(sh) => (
                    if sh.fog_end > 0.0 {
                        ling_gl::Fog { start: sh.fog_start, end: sh.fog_end, color: rgb(sh.fog_color) }
                    } else {
                        ling_gl::Fog::default()
                    },
                    ling_gl::Lighting { lit: sh.lit, amb: sh.amb, lights: sh.lights.clone() },
                ),
                None => (ling_gl::Fog::default(), ling_gl::Lighting::default()),
            }
        };
        let blend_of = |call: &DrawCall| -> ling_gl::Blend {
            match (call.mode, call.alpha.clamp(0.0, 1.0) < 0.999) {
                (1, _) => ling_gl::Blend::Additive,
                (0, false) => ling_gl::Blend::Opaque,
                _ => ling_gl::Blend::Alpha,
            }
        };
        // (texture, shading, shading key, vertices) of a triangle call.
        type Conv<'s> = (Option<u64>, Option<&'s TexShade>, Option<usize>, [ling_gl::Vertex; 3]);
        let mut convert = |call: &'a DrawCall| -> Option<Conv<'a>> {
            let a = call.alpha.clamp(0.0, 1.0);
            Some(match &call.kind {
                DrawKind::Triangle { x0, y0, z0, x1, y1, z1, x2, y2, z2 } => {
                    let c = rgb(call.color);
                    (None, None, None, [vert(*x0, *y0, *z0, 0.0, 0.0, c, a, zdist), vert(*x1, *y1, *z1, 0.0, 0.0, c, a, zdist), vert(*x2, *y2, *z2, 0.0, 0.0, c, a, zdist)])
                },
                DrawKind::TriangleG { x0, y0, z0, c0, x1, y1, z1, c1, x2, y2, z2, c2, .. } => (
                    None,
                    None,
                    None,
                    [
                        vert(*x0, *y0, *z0, 0.0, 0.0, rgb(*c0), a, zdist),
                        vert(*x1, *y1, *z1, 0.0, 0.0, rgb(*c1), a, zdist),
                        vert(*x2, *y2, *z2, 0.0, 0.0, rgb(*c2), a, zdist),
                    ],
                ),
                DrawKind::TriangleT { v, tex, tint, shade, .. } => {
                    let addr = std::sync::Arc::as_ptr(tex) as usize;
                    let id = if addr == last_tex.0 && addr != 0 {
                        last_tex.1
                    } else {
                        let id = match tex_ids.get(&addr) {
                            Some(id) => *id,
                            None => {
                                // Arc addresses get reused by short-lived textures (portraits, hair), so mix in a content sample.
                                let mut h = addr as u64 ^ ((tex.w as u64) << 32 | tex.h as u64);
                                let step = (tex.px.len() / 64).max(1);
                                for p in tex.px.iter().step_by(step) {
                                    h = (h ^ *p as u64).wrapping_mul(0x100000001b3);
                                }
                                tex_ids.insert(addr, h);
                                if known.mark(h) {
                                    textures.push(ling_gl::Texture { id: h, w: tex.w, h: tex.h, px: tex.px.clone() });
                                }
                                h
                            },
                        };
                        last_tex = (addr, id);
                        id
                    };
                    let (sh, key) = match shade {
                        Some(sh) => (Some(&**sh), Some(std::sync::Arc::as_ptr(sh) as usize)),
                        None => (None, None),
                    };
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
                    (Some(id), sh, key, [mk(&v[0]), mk(&v[1]), mk(&v[2])])
                },
                DrawKind::Line { .. } => return None,
            })
        };
        let fresh = |pool: &mut Vec<Vec<ling_gl::Vertex>>| -> Vec<ling_gl::Vertex> {
            let mut v = pool.pop().unwrap_or_default();
            v.clear();
            v
        };

        // The GPU depth-tests opaque triangles, so they group by texture + shading in
        // submission order, which collapses tens of thousands of calls into a few
        // hundred batches. Blended calls are only keyed here and sorted below.
        let mut opaque: Vec<ling_gl::Batch> = Vec::new();
        let mut opaque_ix: std::collections::HashMap<(Option<u64>, Option<usize>), usize> = std::collections::HashMap::new();
        let mut last_opaque: Option<((Option<u64>, Option<usize>), usize)> = None;
        let mut blended: Vec<u64> = Vec::new();
        for (i, call) in calls.iter().enumerate() {
            if blend_of(call) != ling_gl::Blend::Opaque {
                if !matches!(call.kind, DrawKind::Line { .. }) {
                    blended.push(((desc_key(call.depth) as u64) << 32) | i as u64);
                }
                continue;
            }
            let Some((tex_id, sh, key, verts)) = convert(call) else { continue };
            let k = (tex_id, key);
            let idx = match last_opaque {
                Some((lk, li)) if lk == k => li,
                _ => {
                    let idx = match opaque_ix.get(&k) {
                        Some(i) => *i,
                        None => {
                            let (fog, lighting) = shading(sh);
                            let verts = fresh(pool);
                            opaque.push(ling_gl::Batch { texture: tex_id, blend: ling_gl::Blend::Opaque, fog, lighting, verts });
                            opaque_ix.insert(k, opaque.len() - 1);
                            opaque.len() - 1
                        },
                    };
                    last_opaque = Some((k, idx));
                    idx
                },
            };
            opaque[idx].verts.extend_from_slice(&verts);
        }

        // Blended geometry draws after all opaque (which writes depth), back-to-front.
        blended.sort_unstable();
        let mut last_shade: Option<usize> = None;
        let first_blended = opaque.len();
        let mut batches = opaque;
        for k in &blended {
            let call = &calls[(k & 0xFFFF_FFFF) as usize];
            let blend = blend_of(call);
            let Some((tex_id, sh, key, verts)) = convert(call) else { continue };
            let merge = batches.len() > first_blended;
            match batches.last_mut() {
                Some(b) if merge && b.texture == tex_id && b.blend == blend && last_shade == key => b.verts.extend_from_slice(&verts),
                _ => {
                    let (fog, lighting) = shading(sh);
                    let mut v = fresh(pool);
                    v.extend_from_slice(&verts);
                    batches.push(ling_gl::Batch { texture: tex_id, blend, fog, lighting, verts: v })
                },
            }
            last_shade = key;
        }
        scene.batches = batches;
        scene.textures = textures;
        scene
    }
}

#[cfg(all(feature = "gl", not(target_arch = "wasm32")))]
mod gl_bridge {
    use super::*;

    pub struct Bridge {
        pub gpu: Option<ling_gl::Headless>,
        pub known: TexLru,
        pub frames: u32,
        pub slow: u32,
        /// Batch vertex Vecs recycled between flushes.
        pub pool: Vec<Vec<ling_gl::Vertex>>,
    }

    /// GPU-resident texture ids with the flush they were last drawn in.
    #[derive(Default)]
    pub struct TexLru {
        pub last: std::collections::HashMap<u64, u32>,
        pub frame: u32,
    }
    impl super::gl_scene::TexKnown for TexLru {
        fn mark(&mut self, id: u64) -> bool {
            self.last.insert(id, self.frame).is_none()
        }
    }
    /// Above this many cached textures, ones not drawn for a few flushes are freed.
    const TEX_SOFT_CAP: usize = 400;

    thread_local! {
        pub static BRIDGE: std::cell::RefCell<Option<Bridge>> = const { std::cell::RefCell::new(None) };
    }

    pub fn shutdown() {
        let bridge = BRIDGE.try_with(|c| c.borrow_mut().take()).ok().flatten();
        if let Some(gpu) = bridge.and_then(|b| b.gpu) {
            gpu.shutdown();
        }
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
                Bridge { gpu, known: TexLru::default(), frames: 0, slow: 0, pool: Vec::new() }
            });
            let Some(gpu) = br.gpu.as_mut() else { return false };
            // Tiny flushes (UI, portraits) cost more in readback than they save.
            if calls.len() < 1500 || w * h < 640 * 360 {
                return false;
            }
            // Short-lived textures (portraits, name plates, regenerated skins) get
            // fresh ids, so the cache grows; evict only the stale ones instead of
            // wiping it, which re-uploaded every live texture (multi-ms GPU stalls).
            br.known.frame = br.known.frame.wrapping_add(1);
            if br.known.last.len() > TEX_SOFT_CAP {
                let now = br.known.frame;
                let r = gpu.renderer_mut();
                br.known.last.retain(|id, seen| {
                    let keep = now.wrapping_sub(*seen) <= 2;
                    if !keep {
                        r.remove_texture(*id);
                    }
                    keep
                });
            }

            let t_build = std::time::Instant::now();
            let scene = super::gl_scene::build_pooled(calls, &mut br.known, &mut br.pool);
            let build_ms = t_build.elapsed().as_millis();
            crate::runtime::ling_phase_add(crate::runtime::phase::GL_BUILD, t_build.elapsed().as_nanos());
            let t0 = std::time::Instant::now();
            let n_batches = scene.batches.len();
            let n_tex = scene.textures.len();
            let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match zbuf {
                Some(z) => gpu.render_over_depth(&scene, w, h, buf, z.as_mut_slice()),
                None => gpu.render_over(&scene, w, h, buf),
            }));
            {
                use crate::runtime::{ling_phase_add, phase};
                let [e, wt, u] = gpu.timings;
                ling_phase_add(phase::GL_ENCODE, (e * 1e6) as u128);
                ling_phase_add(phase::GL_WAIT, (wt * 1e6) as u128);
                ling_phase_add(phase::GL_UNPACK, (u * 1e6) as u128);
                let [un, upx, ums] = gpu.renderer_mut().last_upload;
                ling_phase_add(phase::GL_UPLOAD, (ums * 1e6) as u128);
                ling_phase_add(phase::GL_UPLOADS, un as u128);
                ling_phase_add(phase::GL_UPLOAD_PX, upx as u128);
                ling_phase_add(phase::GL_TRIS, calls.len() as u128);
                crate::runtime::ling_phase_batches(n_batches);
                super::gl_scene::recycle(scene, &mut br.pool);
                let other = t0.elapsed().as_secs_f64() * 1000.0 - (e + wt + u);
                ling_phase_add(phase::GL_OTHER, (other.max(0.0) * 1e6) as u128);
            }
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

/// Orderly GPU teardown on this thread; call before the process exits. No-op without a GPU.
pub fn gpu_shutdown() {
    #[cfg(all(feature = "gl", not(target_arch = "wasm32")))]
    gl_bridge::shutdown();
}

#[cfg(not(target_arch = "wasm32"))]
struct FlushTimer(std::time::Instant);
#[cfg(not(target_arch = "wasm32"))]
impl Drop for FlushTimer {
    fn drop(&mut self) {
        crate::runtime::ling_phase_add(crate::runtime::phase::FLUSH, self.0.elapsed().as_nanos());
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
pub struct TexData {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u32>,
}

impl std::fmt::Debug for TexData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TexData({}x{})", self.w, self.h)
    }
}

/// Deferred depth-sorted draw queue.
#[derive(Debug)]
pub struct DepthQueue {
    calls: Vec<DrawCall>,
    /// Current blend mode applied to subsequent pushes (mirrors `gfx.blend`).
    cur_mode: u8,
    /// Current pen alpha applied to subsequent pushes (mirrors `gfx.alpha`).
    cur_alpha: f32,
}

impl Default for DepthQueue {
    fn default() -> Self {
        // Reuse a drained queue's buffer: a frame's queue is swapped out with
        // `mem::take` and consumed by `flush`, so without this every frame
        // regrows a fresh Vec to 100k+ calls (tens of MB of page faults).
        let calls = CALLS_POOL.with(|p| {
            let mut p = p.borrow_mut();
            let best = (0..p.len()).max_by_key(|&i| p[i].capacity());
            best.map(|i| p.swap_remove(i)).unwrap_or_default()
        });
        Self { calls, cur_mode: 0, cur_alpha: 1.0 }
    }
}

impl Drop for DepthQueue {
    fn drop(&mut self) {
        if self.calls.capacity() == 0 {
            return;
        }
        let mut v = std::mem::take(&mut self.calls);
        v.clear();
        let _ = CALLS_POOL.try_with(|p| {
            if let Ok(mut p) = p.try_borrow_mut() {
                p.push(v);
                if p.len() > 2 {
                    let small = (0..p.len()).min_by_key(|&i| p[i].capacity()).unwrap_or(0);
                    p.swap_remove(small);
                }
            }
        });
    }
}

thread_local! {
    /// Emptied draw-call buffers kept for the next [`DepthQueue::default`].
    static CALLS_POOL: std::cell::RefCell<Vec<Vec<DrawCall>>> =
        const { std::cell::RefCell::new(Vec::new()) };
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
        self.calls.push(DrawCall {
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
        self.calls.push(DrawCall {
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
        self.calls.push(DrawCall {
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
        self.calls.push(DrawCall {
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
        self.calls.push(DrawCall {
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
        self.calls.push(DrawCall {
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
        // With a z-buffer the sort still helps transparency + reduces overdraw.
        // STABLE sort: equal-depth calls keep submission order every frame, so
        // co-planar / same-depth overlapping primitives (e.g. adjacent boot/title
        // glyphs at z=0) never swap draw order frame-to-frame — kills the
        // "lit↔unlit" flicker that an unstable sort produced on ties.
        #[cfg(all(feature = "gl", not(target_arch = "wasm32")))]
        {
            // The GPU path keeps its own depth; still reset the CPU z-buffer so
            // geometry drawn later on the CPU (portraits, UI models) isn't occluded by last frame's depth.
            let mut zb = zbuf;
            let _tz = std::time::Instant::now();
            if let Some(z) = zb.as_deref_mut() {
                prepare_depth(z, width * height, reset_z);
            }
            let tz = _tz.elapsed().as_nanos();
            // Unsorted: the GPU depth-tests opaque triangles and gl_scene orders the
            // blended ones itself, so only the CPU fallback needs the full sort.
            if gl_flush(&self.calls, buf, zb.as_deref_mut(), width, height) {
                // The GPU drew the triangles; lines still go through the CPU, back-to-front.
                let _tl = std::time::Instant::now();
                let mut lines: Vec<u64> = self
                    .calls
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| matches!(c.kind, DrawKind::Line { .. }))
                    .map(|(i, c)| ((desc_key(c.depth) as u64) << 32) | i as u64)
                    .collect();
                lines.sort_unstable();
                for k in &lines {
                    rasterize_call(&self.calls[(k & 0xFFFF_FFFF) as usize], buf, None, width, height, 0.0, aa);
                }
                crate::runtime::ling_phase_add(crate::runtime::phase::LINES, tz + _tl.elapsed().as_nanos());
                if std::env::var("LING_GPU_LOG").is_ok() {
                    eprintln!("[ling-gl]   {} cpu lines {} ms", lines.len(), _tl.elapsed().as_millis());
                }
                return;
            }
            let _s = std::time::Instant::now();
            sort_back_to_front(&mut self.calls);
            crate::runtime::ling_phase_add(crate::runtime::phase::SORT, _s.elapsed().as_nanos());
            return self.flush_cpu(buf, zb, reset_z, width, height, aa);
        }
        #[cfg(not(all(feature = "gl", not(target_arch = "wasm32"))))]
        {
            #[cfg(not(target_arch = "wasm32"))]
            let _s = std::time::Instant::now();
            sort_back_to_front(&mut self.calls);
            #[cfg(not(target_arch = "wasm32"))]
            crate::runtime::ling_phase_add(crate::runtime::phase::SORT, _s.elapsed().as_nanos());
            // Web: the scene is drawn by WebGL2 over the finished CPU frame at present time.
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
                return;
            }
            self.flush_cpu(buf, zbuf, reset_z, width, height, aa)
        }
    }

    #[allow(clippy::ptr_arg)]
    fn flush_cpu(
        self,
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
        let n = (width * height).min(buf.len());
        let rows = if width == 0 { 0 } else { n / width };
        let n = rows * width;
        match zbuf {
            Some(z) => {
                prepare_depth(z, width * height, reset_z);
                raster_rows(&self.calls, &mut buf[..n], Some(&mut z[..n]), width, 0, rows, aa);
            },
            None => raster_rows(&self.calls, &mut buf[..n], None, width, 0, rows, aa),
        }
    }

    /// Sort and rasterise on the CPU into rows `y0..y1` only — the
    /// picture-in-picture path (`flush_3d_clip`). Never takes the GPU path: a
    /// small box isn't worth a full-frame upload + readback, and only the band
    /// of rows that holds the box is touched (the z-buffer is still reset in
    /// full when `reset_z`, as a full flush would).
    #[allow(clippy::too_many_arguments, clippy::ptr_arg)]
    pub fn flush_rows(
        mut self,
        buf: &mut Vec<u32>,
        zbuf: Option<&mut Vec<f32>>,
        reset_z: bool,
        width: usize,
        height: usize,
        y0: usize,
        y1: usize,
        aa: bool,
    ) {
        sort_back_to_front(&mut self.calls);
        let y1 = y1.min(height).min(buf.len() / width.max(1));
        let y0 = y0.min(y1);
        let rows = y0 * width..y1 * width;
        match zbuf {
            Some(z) => {
                prepare_depth(z, width * height, reset_z);
                raster_rows(&self.calls, &mut buf[rows.clone()], Some(&mut z[rows]), width, y0, y1 - y0, aa);
            },
            None => raster_rows(&self.calls, &mut buf[rows], None, width, y0, y1 - y0, aa),
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

#[cfg(all(test, feature = "gl-web"))]
mod gl_scene_tests {
    use super::*;

    fn tri(depth: f32, alpha: f32, x: f32) -> DrawCall {
        DrawCall {
            depth,
            color: 0x808080,
            mode: 0,
            alpha,
            unlit: false,
            kind: DrawKind::Triangle { x0: x, y0: 0.0, z0: depth, x1: x + 1.0, y1: 0.0, z1: depth, x2: x, y2: 1.0, z2: depth },
        }
    }

    #[test]
    fn unsorted_input_draws_blended_back_to_front() {
        // Submission order, as the GPU path now receives it.
        let calls = vec![tri(5.0, 1.0, 1.0), tri(3.0, 0.5, 2.0), tri(9.0, 1.0, 3.0), tri(10.0, 0.5, 4.0), tri(3.0, 0.5, 5.0)];
        let mut known = std::collections::HashSet::new();
        let mut pool = vec![vec![ling_gl::Vertex::default(); 7]];
        let scene = gl_scene::build_pooled(&calls, &mut known, &mut pool);
        assert_eq!(scene.batches.len(), 2);
        let xs = |b: &ling_gl::Batch| b.verts.iter().step_by(3).map(|v| v.x).collect::<Vec<_>>();
        assert_eq!(scene.batches[0].blend, ling_gl::Blend::Opaque);
        assert_eq!(xs(&scene.batches[0]), vec![1.0, 3.0]);
        // Furthest first; the depth-3 tie keeps submission order.
        assert_eq!(scene.batches[1].blend, ling_gl::Blend::Alpha);
        assert_eq!(xs(&scene.batches[1]), vec![4.0, 2.0, 5.0]);
        gl_scene::recycle(scene, &mut pool);
        assert_eq!(pool.len(), 2);
    }
}
