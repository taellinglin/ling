// src/gfx/depth.rs — deferred depth-sorted draw queue (painter's algorithm).
//
// All 3-D draw calls (`วาดสามเหลี่ยม3มิติ`, `วาดเส้น3มิติ`) push a `DrawCall`
// into this queue instead of rasterising immediately.  When `แสดงผล` / `present`
// is called, the queue is sorted back-to-front by the depth tag and then
// flushed into the pixel buffer.
//
// Painter's algorithm is exact for convex non-intersecting geometry and
// produces plausible results for the Sierpiński fractal + tesseract wireframe.

use crate::gfx::raster;

/// Tagged draw call stored in the queue.
#[derive(Debug, Clone)]
pub struct DrawCall {
    /// Camera-space z of the face/edge centroid — larger = further away.
    pub depth: f32,
    /// Pre-lit 0x00RRGGBB colour.
    pub color: u32,
    pub kind:  DrawKind,
}

#[derive(Debug, Clone)]
pub enum DrawKind {
    Triangle { x0:f32, y0:f32, x1:f32, y1:f32, x2:f32, y2:f32 },
    Line     { x0:f32, y0:f32, x1:f32, y1:f32 },
}

/// Deferred depth-sorted draw queue.
#[derive(Default, Debug)]
pub struct DepthQueue {
    calls: Vec<DrawCall>,
}

impl DepthQueue {
    /// Queue a filled triangle.
    pub fn push_triangle(
        &mut self, depth: f32, color: u32,
        x0:f32, y0:f32, x1:f32, y1:f32, x2:f32, y2:f32,
    ) {
        self.calls.push(DrawCall {
            depth, color,
            kind: DrawKind::Triangle { x0, y0, x1, y1, x2, y2 },
        });
    }

    /// Queue a line segment.
    pub fn push_line(
        &mut self, depth: f32, color: u32,
        x0:f32, y0:f32, x1:f32, y1:f32,
    ) {
        self.calls.push(DrawCall {
            depth, color,
            kind: DrawKind::Line { x0, y0, x1, y1 },
        });
    }

    /// Sort back-to-front and rasterise everything into `buf`.
    /// Consumes `self` — call site does `mem::take` to avoid borrow conflict.
    pub fn flush(mut self, buf: &mut Vec<u32>, width: usize, height: usize) {
        // Sort largest depth first (furthest → painted first, nearest on top)
        self.calls.sort_unstable_by(|a, b| {
            b.depth.partial_cmp(&a.depth).unwrap_or(std::cmp::Ordering::Equal)
        });
        for call in &self.calls {
            match call.kind {
                DrawKind::Triangle { x0, y0, x1, y1, x2, y2 } =>
                    raster::fill_triangle(buf, width, height, call.color,
                                         x0, y0, x1, y1, x2, y2),
                DrawKind::Line { x0, y0, x1, y1 } =>
                    raster::draw_line(buf, width, height, call.color,
                                      x0, y0, x1, y1),
            }
        }
        // `self` dropped here — no need to clear explicitly
    }

    pub fn is_empty(&self) -> bool { self.calls.is_empty() }
}
