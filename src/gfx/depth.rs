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
    ) {
        self.calls.push(DrawCall {
            depth,
            color: c0,
            mode: self.cur_mode,
            alpha: self.cur_alpha,
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
    ) {
        let depth = (z0 + z1 + z2) / 3.0;
        self.calls.push(DrawCall {
            depth,
            color: c0,
            mode: self.cur_mode,
            alpha: self.cur_alpha,
            kind: DrawKind::TriangleG { x0, y0, z0, c0, x1, y1, z1, c1, x2, y2, z2, c2, bands },
        });
    }

    /// Queue a line segment (flat per-vertex depth).
    pub fn push_line(&mut self, depth: f32, color: u32, x0: f32, y0: f32, x1: f32, y1: f32) {
        self.calls.push(DrawCall {
            depth,
            color,
            mode: self.cur_mode,
            alpha: self.cur_alpha,
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
        width: usize,
        height: usize,
    ) {
        // Sort largest depth first (furthest → painted first, nearest on top).
        // With a z-buffer the sort still helps transparency + reduces overdraw.
        self.calls.sort_unstable_by(|a, b| {
            b.depth
                .partial_cmp(&a.depth)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        match zbuf {
            Some(z) => {
                // Reset the depth buffer to "infinitely far" for this frame.
                if z.len() != width * height {
                    z.clear();
                    z.resize(width * height, f32::INFINITY);
                } else {
                    for v in z.iter_mut() {
                        *v = f32::INFINITY;
                    }
                }
                for call in &self.calls {
                    let blended = call.mode != 0 || call.alpha < 0.999;
                    match call.kind {
                        DrawKind::Triangle { x0, y0, z0, x1, y1, z1, x2, y2, z2 } => {
                            if blended {
                                raster::fill_triangle_z_blend(
                                    buf, z, width, height, call.color, call.mode, call.alpha, x0,
                                    y0, z0, x1, y1, z1, x2, y2, z2,
                                );
                            } else {
                                raster::fill_triangle_z(
                                    buf, z, width, height, call.color, x0, y0, z0, x1, y1, z1, x2,
                                    y2, z2,
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
                        } => raster::fill_triangle_gouraud_z(
                            buf, z, width, height, x0, y0, z0, c0, x1, y1, z1, c1, x2, y2, z2, c2,
                            bands,
                        ),
                        DrawKind::Line { x0, y0, x1, y1, .. } => {
                            if blended {
                                raster::draw_line_blend(
                                    buf, width, height, call.color, call.mode, call.alpha, x0, y0,
                                    x1, y1,
                                );
                            } else {
                                raster::draw_line(buf, width, height, call.color, x0, y0, x1, y1);
                            }
                        },
                    }
                }
            },
            None => {
                for call in &self.calls {
                    let blended = call.mode != 0 || call.alpha < 0.999;
                    match call.kind {
                        DrawKind::Triangle { x0, y0, x1, y1, x2, y2, .. } => {
                            if blended {
                                raster::fill_triangle_blend(
                                    buf, width, height, call.color, call.mode, call.alpha, x0, y0,
                                    x1, y1, x2, y2,
                                );
                            } else {
                                raster::fill_triangle(
                                    buf, width, height, call.color, x0, y0, x1, y1, x2, y2,
                                );
                            }
                        },
                        DrawKind::TriangleG {
                            x0, y0, c0, x1, y1, c1, x2, y2, c2, bands, ..
                        } => raster::fill_triangle_gouraud(
                            buf, width, height, x0, y0, c0, x1, y1, c1, x2, y2, c2, bands,
                        ),
                        DrawKind::Line { x0, y0, x1, y1, .. } => {
                            if blended {
                                raster::draw_line_blend(
                                    buf, width, height, call.color, call.mode, call.alpha, x0, y0,
                                    x1, y1,
                                );
                            } else {
                                raster::draw_line(buf, width, height, call.color, x0, y0, x1, y1);
                            }
                        },
                    }
                }
            },
        }
        // `self` dropped here — no need to clear explicitly
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
}
