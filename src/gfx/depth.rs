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

/// Number of horizontal bands to rasterise a flush across. 1 = serial.
///
/// Banding pays off only when fill (pixels written) dominates: each band re-runs
/// per-triangle setup, so a flush of many *tiny* triangles (e.g. text glyphs)
/// would just multiply that setup. Gate on estimated covered area, not call count.
#[cfg(not(target_arch = "wasm32"))]
fn render_bands(width: usize, height: usize, est_pixels: usize) -> usize {
    let screen = width * height;
    if width == 0 || height < 256 || est_pixels < screen {
        return 1;
    }
    let by_rows = height / 96; // keep bands ≥ ~96 rows tall
    let by_fill = est_pixels / screen; // more overdraw → more bands worth it
    rayon::current_num_threads()
        .min(by_rows)
        .min(by_fill.max(1) + 1)
        .max(1)
}

#[cfg(not(target_arch = "wasm32"))]
fn estimate_fill(calls: &[DrawCall]) -> usize {
    let mut px = 0.0f32;
    for c in calls {
        let (a, b) = match c.kind {
            DrawKind::Triangle { x0, y0, x1, y1, x2, y2, .. }
            | DrawKind::TriangleG { x0, y0, x1, y1, x2, y2, .. } => {
                let w = x0.max(x1).max(x2) - x0.min(x1).min(x2);
                let h = y0.max(y1).max(y2) - y0.min(y1).min(y2);
                (w, h)
            },
            DrawKind::Line { .. } => (0.0, 0.0),
        };
        px += 0.5 * a * b;
    }
    px.max(0.0) as usize
}

#[cfg(target_arch = "wasm32")]
fn render_bands(_w: usize, _h: usize, _n: usize) -> usize {
    1
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
            DrawKind::TriangleG { x0, y0, z0, c0, x1, y1, z1, c1, x2, y2, z2, c2, bands } => {
                raster::fill_triangle_gouraud_z(
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
            DrawKind::TriangleG { x0, y0, c0, x1, y1, c1, x2, y2, c2, bands, .. } => {
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
        },
    }
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
    },
    Line {
        x0: f32,
        y0: f32,
        z0: f32,
        x1: f32,
        y1: f32,
        z1: f32,
    },
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
        let depth = (z0 + z1 + z2) / 3.0;
        self.calls.push(DrawCall {
            depth,
            color: c0,
            mode: self.cur_mode,
            alpha: self.cur_alpha,
            unlit,
            kind: DrawKind::TriangleG { x0, y0, z0, c0, x1, y1, z1, c1, x2, y2, z2, c2, bands },
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
        #[cfg(not(target_arch = "wasm32"))]
        let _s = std::time::Instant::now();
        self.calls.sort_by(|a, b| {
            b.depth
                .partial_cmp(&a.depth)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        #[cfg(not(target_arch = "wasm32"))]
        crate::runtime::ling_phase_add(crate::runtime::phase::SORT, _s.elapsed().as_nanos());
        #[cfg(not(target_arch = "wasm32"))]
        let _r = std::time::Instant::now();
        #[cfg(not(target_arch = "wasm32"))]
        let _guard = FlushTimer(_r);
        let calls = &self.calls;
        // Split the framebuffer into horizontal bands rasterised in parallel.
        // Each band owns a disjoint slice of `buf`/`zbuf`, so a pixel is touched
        // by exactly one thread; processing the sorted call list inside every
        // band preserves the painter's per-pixel order. Worth the thread hop only
        // when there is enough fill to amortise it.
        #[cfg(not(target_arch = "wasm32"))]
        let bands = render_bands(width, height, estimate_fill(calls));
        #[cfg(target_arch = "wasm32")]
        let bands = render_bands(width, height, 0);
        match zbuf {
            Some(z) => {
                if z.len() != width * height {
                    z.clear();
                    z.resize(width * height, f32::INFINITY);
                } else if reset_z {
                    z.iter_mut().for_each(|v| *v = f32::INFINITY);
                }
                #[cfg(not(target_arch = "wasm32"))]
                if bands > 1 {
                    let rows = height.div_ceil(bands);
                    buf.par_chunks_mut(rows * width)
                        .zip(z.par_chunks_mut(rows * width))
                        .enumerate()
                        .for_each(|(b, (bbuf, bz))| {
                            let ysh = (b * rows) as f32;
                            let bh = bbuf.len() / width;
                            for call in calls {
                                rasterize_call(call, bbuf, Some(bz), width, bh, ysh, aa);
                            }
                        });
                    return;
                }
                let _ = bands;
                for call in calls {
                    rasterize_call(call, buf, Some(z), width, height, 0.0, aa);
                }
            },
            None => {
                #[cfg(not(target_arch = "wasm32"))]
                if bands > 1 {
                    let rows = height.div_ceil(bands);
                    buf.par_chunks_mut(rows * width)
                        .enumerate()
                        .for_each(|(b, bbuf)| {
                            let ysh = (b * rows) as f32;
                            let bh = bbuf.len() / width;
                            for call in calls {
                                rasterize_call(call, bbuf, None, width, bh, ysh, aa);
                            }
                        });
                    return;
                }
                let _ = bands;
                for call in calls {
                    rasterize_call(call, buf, None, width, height, 0.0, aa);
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
            }
        }
        if buf.len() < width * height {
            buf.resize(width * height, 0);
        }
        crate::gfx::wgpu_raster::raster(&verts, width, height, clear, buf)
    }
}
