// src/gfx/depth.rs — deferred depth-sorted draw queue (painter's algorithm).
//
// All 3-D draw calls (`วาดสามเหลี่ยม3มิติ`, `วาดเส้น3มิติ`) push a `DrawCall`
// into this queue instead of rasterising immediately.  When `แสดงผล` / `present`
// is called, the queue is sorted back-to-front by the depth tag and then
// flushed into the pixel buffer.
//
// Painter's algorithm is exact for convex non-intersecting geometry and
// produces plausible results for the Sierpiński fractal + tesseract wireframe.

// `raster` is wasm-safe (pure CPU); the software-framebuffer flush runs on web too.
use crate::gfx::raster;

/// Tagged draw call stored in the queue.
#[derive(Debug, Clone)]
pub struct DrawCall {
    /// Camera-space z of the face/edge centroid — larger = further away.
    pub depth: f32,
    /// Pre-lit 0x00RRGGBB colour.
    pub color: u32,
    pub kind: DrawKind,
}

#[derive(Debug, Clone)]
pub enum DrawKind {
    Triangle {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    },
    /// Gouraud-interpolated + per-pixel posterised triangle (smooth cel).
    TriangleG {
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
    },
    Line {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    },
}

/// Deferred depth-sorted draw queue.
#[derive(Default, Debug)]
pub struct DepthQueue {
    calls: Vec<DrawCall>,
}

impl DepthQueue {
    /// Queue a filled triangle.
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
            kind: DrawKind::Triangle { x0, y0, x1, y1, x2, y2 },
        });
    }

    /// Queue a Gouraud + posterised triangle (smooth cel).
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
            kind: DrawKind::TriangleG { x0, y0, c0, x1, y1, c1, x2, y2, c2, bands },
        });
    }

    /// Queue a line segment.
    pub fn push_line(&mut self, depth: f32, color: u32, x0: f32, y0: f32, x1: f32, y1: f32) {
        self.calls
            .push(DrawCall { depth, color, kind: DrawKind::Line { x0, y0, x1, y1 } });
    }

    /// Sort back-to-front and rasterise everything into `buf`.
    /// Consumes `self` — call site does `mem::take` to avoid borrow conflict.
    pub fn flush(mut self, buf: &mut Vec<u32>, width: usize, height: usize) {
        // Sort largest depth first (furthest → painted first, nearest on top)
        self.calls.sort_unstable_by(|a, b| {
            b.depth
                .partial_cmp(&a.depth)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for call in &self.calls {
            match call.kind {
                DrawKind::Triangle { x0, y0, x1, y1, x2, y2 } => {
                    raster::fill_triangle(buf, width, height, call.color, x0, y0, x1, y1, x2, y2)
                },
                DrawKind::TriangleG { x0, y0, c0, x1, y1, c1, x2, y2, c2, bands } => {
                    raster::fill_triangle_gouraud(
                        buf, width, height, x0, y0, c0, x1, y1, c1, x2, y2, c2, bands,
                    )
                },
                DrawKind::Line { x0, y0, x1, y1 } => {
                    raster::draw_line(buf, width, height, call.color, x0, y0, x1, y1)
                },
            }
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
                DrawKind::Triangle { x0, y0, x1, y1, x2, y2 } => {
                    crate::gfx::webgl::push_triangle(call.color, x0, y0, x1, y1, x2, y2, call.depth)
                },
                DrawKind::TriangleG { x0, y0, c0, x1, y1, c1, x2, y2, c2, bands: _ } => {
                    // WebGL path: approximate with the averaged vertex colour.
                    let avg = {
                        let r = ((c0 >> 16 & 0xFF) + (c1 >> 16 & 0xFF) + (c2 >> 16 & 0xFF)) / 3;
                        let g = ((c0 >> 8 & 0xFF) + (c1 >> 8 & 0xFF) + (c2 >> 8 & 0xFF)) / 3;
                        let b = ((c0 & 0xFF) + (c1 & 0xFF) + (c2 & 0xFF)) / 3;
                        (r << 16) | (g << 8) | b
                    };
                    crate::gfx::webgl::push_triangle(avg, x0, y0, x1, y1, x2, y2, call.depth);
                },
                DrawKind::Line { x0, y0, x1, y1 } => {
                    crate::gfx::webgl::push_line(call.color, x0, y0, x1, y1, call.depth)
                },
            }
        }
        crate::gfx::webgl::flush(fill_r, fill_g, fill_b, width, height);
    }
}
