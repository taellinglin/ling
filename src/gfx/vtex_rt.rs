// src/gfx/vtex_rt.rs — Real-time vector texture with dirty-tracking cache.
//
// Model: "FBO with sleeping"
// ══════════════════════════
// A VtexRT is a resolution-independent vector texture backed by a CPU pixel
// buffer (Arc<Vec<u32>>).  It is built from a queue of vector operations:
// fill, circle, rect, line, gradient, ripple.  Each op is composited over
// the previous result in order (painters algorithm at the texture level).
//
// Dirty tracking
// ──────────────
// All op parameters are hashed (FNV-1a over f32 bits).  If the hash matches
// the previous render, `ensure_rendered()` returns immediately — zero cost.
// If any parameter changed, the texture re-renders.  This means:
//   • Dynamic animation (ripple phase, ball position) → one re-render/frame
//   • Static texture                                  → zero re-renders/frame
//   • Stopped animation                               → automatically sleeps
//
// The rendered buffer is Arc<Vec<u32>> so DrawKind::TriangleT can clone the
// Arc at push time (O(1) — no copy) and the depth queue deref's it at flush.
//
// Shape rendering
// ───────────────
// All ops use SDF (Signed Distance Fields) with a 1-pixel anti-aliased edge.
// Coordinates are in UV [0,1] space; the px_size = 1/min(w,h) converts to
// pixel units for the AA fringe.  This makes shapes look identical at any
// texture resolution.

use std::sync::Arc;

// ── Operation types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum VtexOp {
    Fill   { color: u32 },
    Circle { cx: f32, cy: f32, radius: f32, fill: u32, stroke_w: f32, stroke: u32 },
    Rect   { x: f32, y: f32, w: f32, h: f32, fill: u32, stroke_w: f32, stroke: u32 },
    RoundRect { x: f32, y: f32, w: f32, h: f32, corner: f32, fill: u32, stroke_w: f32, stroke: u32 },
    Line   { x0: f32, y0: f32, x1: f32, y1: f32, width: f32, color: u32 },
    Gradient { x0: f32, y0: f32, x1: f32, y1: f32, c0: u32, c1: u32 },
    /// sin(dist*freq + phase) → blend c0↔c1 concentrically.
    Ripple { cx: f32, cy: f32, freq: f32, phase: f32, c0: u32, c1: u32 },
}

// ── VtexRT ───────────────────────────────────────────────────────────────────

pub struct VtexRT {
    pub width:  u32,
    pub height: u32,
    /// Rendered pixel buffer (shared with DrawKind::TriangleT via Arc).
    buffer: Arc<Vec<u32>>,
    /// Ordered op queue — composited in submission order.
    ops:    Vec<VtexOp>,
    /// Hash of `ops` at the last successful render.  If `hash(ops)` matches
    /// this, skip re-rendering.
    last_hash: u64,
    /// True when ops changed since last render.
    dirty: bool,
}

impl VtexRT {
    pub fn new(width: u32, height: u32) -> Self {
        let n = (width as usize) * (height as usize);
        Self {
            width,
            height,
            buffer:    Arc::new(vec![0u32; n]),
            ops:       Vec::new(),
            last_hash: u64::MAX, // impossible hash → first render always runs
            dirty:     true,
        }
    }

    // ── Op submission ─────────────────────────────────────────────────────────

    #[inline]
    fn push(&mut self, op: VtexOp) {
        self.ops.push(op);
        self.dirty = true;
    }

    pub fn fill(&mut self, color: u32)                                 { self.push(VtexOp::Fill { color }); }
    pub fn circle(&mut self, cx: f32, cy: f32, radius: f32, fill: u32){ self.push(VtexOp::Circle { cx, cy, radius, fill, stroke_w: 0.0, stroke: 0 }); }
    pub fn circle_stroke(&mut self, cx: f32, cy: f32, radius: f32, fill: u32, stroke_w: f32, stroke: u32) {
        self.push(VtexOp::Circle { cx, cy, radius, fill, stroke_w, stroke });
    }
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, fill: u32) {
        self.push(VtexOp::Rect { x, y, w, h, fill, stroke_w: 0.0, stroke: 0 });
    }
    pub fn rect_stroke(&mut self, x: f32, y: f32, w: f32, h: f32, fill: u32, stroke_w: f32, stroke: u32) {
        self.push(VtexOp::Rect { x, y, w, h, fill, stroke_w, stroke });
    }
    pub fn round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, corner: f32, fill: u32) {
        self.push(VtexOp::RoundRect { x, y, w, h, corner, fill, stroke_w: 0.0, stroke: 0 });
    }
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, color: u32) {
        self.push(VtexOp::Line { x0, y0, x1, y1, width, color });
    }
    pub fn gradient(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c0: u32, c1: u32) {
        self.push(VtexOp::Gradient { x0, y0, x1, y1, c0, c1 });
    }
    pub fn ripple(&mut self, cx: f32, cy: f32, freq: f32, phase: f32, c0: u32, c1: u32) {
        self.push(VtexOp::Ripple { cx, cy, freq, phase, c0, c1 });
    }

    /// Remove all ops and mark dirty (texture will clear on next ensure_rendered).
    pub fn clear_ops(&mut self) {
        self.ops.clear();
        self.dirty = true;
        self.last_hash = u64::MAX;
    }

    // ── Dirty tracking + render ───────────────────────────────────────────────

    /// Render if the op queue changed since the last render.  Returns an Arc
    /// clone of the pixel buffer — O(1) regardless of texture size.
    pub fn ensure_rendered(&mut self) -> Arc<Vec<u32>> {
        if self.dirty {
            let h = Self::hash_ops(&self.ops);
            if h != self.last_hash {
                self.render();
                self.last_hash = h;
            }
            self.dirty = false;
        }
        Arc::clone(&self.buffer)
    }

    /// Force-render without dirty check.
    pub fn force_render(&mut self) -> Arc<Vec<u32>> {
        self.render();
        self.last_hash = Self::hash_ops(&self.ops);
        self.dirty = false;
        Arc::clone(&self.buffer)
    }

    // ── FNV-1a hash of ops ────────────────────────────────────────────────────

    fn hash_ops(ops: &[VtexOp]) -> u64 {
        let mut h: u64 = 14695981039346656037;
        for op in ops {
            // Push discriminant
            h = fnv(h, op.discriminant());
            // Push float bits + u32 color fields
            for word in op.words() {
                h = fnv(h, (word >> 24) as u8);
                h = fnv(h, (word >> 16) as u8);
                h = fnv(h, (word >> 8) as u8);
                h = fnv(h, word as u8);
            }
        }
        h
    }

    // ── Renderer ──────────────────────────────────────────────────────────────

    fn render(&mut self) {
        let w = self.width as usize;
        let h = self.height as usize;
        let n = w * h;
        if n == 0 { return; }

        // Make the buffer writable (COW via Arc::make_mut).
        let buf = Arc::make_mut(&mut self.buffer);
        if buf.len() != n { buf.resize(n, 0); }
        buf.fill(0); // start transparent-black

        let inv_w  = 1.0 / w as f32;
        let inv_h  = 1.0 / h as f32;
        // One pixel in UV units — used for anti-aliasing fringe width
        let px_uv = inv_w.min(inv_h);

        for row in 0..h {
            let pv = (row as f32 + 0.5) * inv_h;
            let base = row * w;
            for col in 0..w {
                let pu = (col as f32 + 0.5) * inv_w;
                let idx = base + col;
                // Evaluate ops in order (painter's: later ops on top)
                for op in &self.ops {
                    buf[idx] = eval_op(op, pu, pv, px_uv, buf[idx]);
                }
            }
        }
    }
}

// ── Per-op evaluation ─────────────────────────────────────────────────────────

fn eval_op(op: &VtexOp, pu: f32, pv: f32, px_uv: f32, dst: u32) -> u32 {
    match op {
        VtexOp::Fill { color } => *color,

        VtexOp::Circle { cx, cy, radius, fill, stroke_w, stroke } => {
            let dist = ((pu - cx).powi(2) + (pv - cy).powi(2)).sqrt();
            // Filled interior
            let fill_sdf = dist - radius;
            let fill_cov = aa_cov(fill_sdf, px_uv);
            let out = blend_over(dst, *fill, fill_cov);
            // Stroke ring
            if *stroke_w > 0.0 {
                let stroke_sdf = fill_sdf.abs() - stroke_w * 0.5;
                let stroke_cov = aa_cov(stroke_sdf, px_uv);
                blend_over(out, *stroke, stroke_cov)
            } else {
                out
            }
        },

        VtexOp::Rect { x, y, w, h, fill, stroke_w, stroke } => {
            let sdf = rect_sdf(pu, pv, *x, *y, *w, *h);
            let fill_cov = aa_cov(sdf, px_uv);
            let out = blend_over(dst, *fill, fill_cov);
            if *stroke_w > 0.0 {
                let stroke_sdf = sdf.abs() - stroke_w * 0.5;
                let stroke_cov = aa_cov(stroke_sdf, px_uv);
                blend_over(out, *stroke, stroke_cov)
            } else {
                out
            }
        },

        VtexOp::RoundRect { x, y, w, h, corner, fill, stroke_w, stroke } => {
            let sdf = round_rect_sdf(pu, pv, *x, *y, *w, *h, *corner);
            let fill_cov = aa_cov(sdf, px_uv);
            let out = blend_over(dst, *fill, fill_cov);
            if *stroke_w > 0.0 {
                let stroke_sdf = sdf.abs() - stroke_w * 0.5;
                let stroke_cov = aa_cov(stroke_sdf, px_uv);
                blend_over(out, *stroke, stroke_cov)
            } else {
                out
            }
        },

        VtexOp::Line { x0, y0, x1, y1, width, color } => {
            let sdf = segment_sdf(pu, pv, *x0, *y0, *x1, *y1) - width * 0.5;
            let cov = aa_cov(sdf, px_uv);
            blend_over(dst, *color, cov)
        },

        VtexOp::Gradient { x0, y0, x1, y1, c0, c1 } => {
            let ax = x1 - x0; let ay = y1 - y0;
            let len2 = ax * ax + ay * ay;
            if len2 < 1e-9 { return dst; }
            let t = ((pu - x0) * ax + (pv - y0) * ay) / len2;
            let t = t.clamp(0.0, 1.0);
            lerp_u32(*c0, *c1, t)
        },

        VtexOp::Ripple { cx, cy, freq, phase, c0, c1 } => {
            let dist = ((pu - cx).powi(2) + (pv - cy).powi(2)).sqrt();
            let t = (dist * freq + phase).sin() * 0.5 + 0.5;
            lerp_u32(*c0, *c1, t)
        },
    }
}

// ── SDF helpers ───────────────────────────────────────────────────────────────

/// Anti-alias coverage: 1 when inside (sdf < 0), 0 when outside.
/// Transition happens over one pixel (px_uv).
#[inline]
fn aa_cov(sdf: f32, px_uv: f32) -> f32 {
    (0.5 - sdf / px_uv).clamp(0.0, 1.0)
}

/// SDF for an axis-aligned rectangle (x,y = top-left corner, w,h = size).
#[inline]
fn rect_sdf(pu: f32, pv: f32, x: f32, y: f32, w: f32, h: f32) -> f32 {
    let qx = (pu - x - w * 0.5).abs() - w * 0.5;
    let qy = (pv - y - h * 0.5).abs() - h * 0.5;
    let outer = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outer + qx.min(0.0).max(qy.min(0.0))
}

/// SDF for a rounded rectangle (corner = UV-space corner radius).
#[inline]
fn round_rect_sdf(pu: f32, pv: f32, x: f32, y: f32, w: f32, h: f32, corner: f32) -> f32 {
    let r = corner.min(w * 0.5).min(h * 0.5);
    let qx = (pu - x - w * 0.5).abs() - w * 0.5 + r;
    let qy = (pv - y - h * 0.5).abs() - h * 0.5 + r;
    let outer = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outer + qx.min(0.0).max(qy.min(0.0)) - r
}

/// SDF from point (pu,pv) to line segment (ax,ay)→(bx,by).
#[inline]
fn segment_sdf(pu: f32, pv: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let abx = bx - ax; let aby = by - ay;
    let len2 = abx * abx + aby * aby;
    if len2 < 1e-9 {
        return ((pu - ax).powi(2) + (pv - ay).powi(2)).sqrt();
    }
    let t = ((pu - ax) * abx + (pv - ay) * aby) / len2;
    let t = t.clamp(0.0, 1.0);
    let cx = ax + t * abx; let cy = ay + t * aby;
    ((pu - cx).powi(2) + (pv - cy).powi(2)).sqrt()
}

// ── Color helpers ─────────────────────────────────────────────────────────────

/// Porter-Duff "over": composite `src` with coverage `cov` over `dst`.
#[inline]
fn blend_over(dst: u32, src: u32, cov: f32) -> u32 {
    if cov <= 0.0 { return dst; }
    if cov >= 1.0 { return src; }
    let dr = ((dst >> 16) & 0xFF) as f32;
    let dg = ((dst >> 8)  & 0xFF) as f32;
    let db = (dst & 0xFF) as f32;
    let sr = ((src >> 16) & 0xFF) as f32;
    let sg = ((src >> 8)  & 0xFF) as f32;
    let sb = (src & 0xFF) as f32;
    let r = (sr * cov + dr * (1.0 - cov)) as u32;
    let g = (sg * cov + dg * (1.0 - cov)) as u32;
    let b = (sb * cov + db * (1.0 - cov)) as u32;
    (r << 16) | (g << 8) | b
}

/// Linear interpolate two 0x00RRGGBB colours.
#[inline]
fn lerp_u32(a: u32, b: u32, t: f32) -> u32 {
    let ar = ((a >> 16) & 0xFF) as f32;
    let ag = ((a >> 8)  & 0xFF) as f32;
    let ab = (a & 0xFF) as f32;
    let br = ((b >> 16) & 0xFF) as f32;
    let bg = ((b >> 8)  & 0xFF) as f32;
    let bb = (b & 0xFF) as f32;
    let r = (ar + (br - ar) * t) as u32;
    let g = (ag + (bg - ag) * t) as u32;
    let bl= (ab + (bb - ab) * t) as u32;
    (r << 16) | (g << 8) | bl
}

// ── FNV-1a step ───────────────────────────────────────────────────────────────

#[inline]
fn fnv(h: u64, b: u8) -> u64 {
    (h ^ b as u64).wrapping_mul(1099511628211u64)
}

// ── Op serialisation for hashing ──────────────────────────────────────────────

impl VtexOp {
    fn discriminant(&self) -> u8 {
        match self {
            VtexOp::Fill { .. }      => 0,
            VtexOp::Circle { .. }    => 1,
            VtexOp::Rect { .. }      => 2,
            VtexOp::RoundRect { .. } => 3,
            VtexOp::Line { .. }      => 4,
            VtexOp::Gradient { .. }  => 5,
            VtexOp::Ripple { .. }    => 6,
        }
    }

    /// Flatten all fields to u32 words (float bits + color values).
    fn words(&self) -> Vec<u32> {
        match self {
            VtexOp::Fill { color } => vec![*color],
            VtexOp::Circle { cx, cy, radius, fill, stroke_w, stroke } =>
                vec![cx.to_bits(), cy.to_bits(), radius.to_bits(), *fill,
                     stroke_w.to_bits(), *stroke],
            VtexOp::Rect { x, y, w, h, fill, stroke_w, stroke } =>
                vec![x.to_bits(), y.to_bits(), w.to_bits(), h.to_bits(), *fill,
                     stroke_w.to_bits(), *stroke],
            VtexOp::RoundRect { x, y, w, h, corner, fill, stroke_w, stroke } =>
                vec![x.to_bits(), y.to_bits(), w.to_bits(), h.to_bits(),
                     corner.to_bits(), *fill, stroke_w.to_bits(), *stroke],
            VtexOp::Line { x0, y0, x1, y1, width, color } =>
                vec![x0.to_bits(), y0.to_bits(), x1.to_bits(), y1.to_bits(),
                     width.to_bits(), *color],
            VtexOp::Gradient { x0, y0, x1, y1, c0, c1 } =>
                vec![x0.to_bits(), y0.to_bits(), x1.to_bits(), y1.to_bits(), *c0, *c1],
            VtexOp::Ripple { cx, cy, freq, phase, c0, c1 } =>
                vec![cx.to_bits(), cy.to_bits(), freq.to_bits(), phase.to_bits(), *c0, *c1],
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_renders_solid_color() {
        let mut t = VtexRT::new(4, 4);
        t.fill(0x00FF_0000); // red
        let buf = t.ensure_rendered();
        assert!(buf.iter().all(|&p| p == 0x00FF_0000),
            "fill should paint every pixel red");
    }

    #[test]
    fn dirty_tracking_skips_re_render() {
        let mut t = VtexRT::new(8, 8);
        t.fill(0x0000_FF00);
        let arc1 = t.ensure_rendered();
        // Call again without changing ops — should return same Arc (no re-render)
        let arc2 = t.ensure_rendered();
        assert!(Arc::ptr_eq(&arc1, &arc2), "same ops → same Arc, no re-render");
    }

    #[test]
    fn op_change_triggers_re_render() {
        let mut t = VtexRT::new(8, 8);
        t.fill(0x0000_00FF);
        let arc1 = t.ensure_rendered();
        // Change phase on ripple → different hash → re-render
        t.ripple(0.5, 0.5, 10.0, 0.0, 0x00FF_0000, 0x0000_FF00);
        let arc2 = t.ensure_rendered();
        // Buffer must have changed
        assert_ne!(arc1[0], arc2[0], "changed ops should produce different pixels");
    }

    #[test]
    fn ripple_changes_when_phase_changes() {
        let mut t = VtexRT::new(32, 32);
        t.ripple(0.5, 0.5, 20.0, 0.0,  0x00FF_0000, 0x0000_FF00);
        let b1 = t.ensure_rendered();
        let s1 = b1[16 * 32 + 16]; // centre pixel

        t.clear_ops();
        t.ripple(0.5, 0.5, 20.0, 1.0, 0x00FF_0000, 0x0000_FF00);
        let b2 = t.ensure_rendered();
        let s2 = b2[16 * 32 + 16];
        assert_ne!(s1, s2, "different phase → different pixel at centre");
    }

    #[test]
    fn circle_centre_is_filled() {
        let mut t = VtexRT::new(64, 64);
        t.fill(0x00_00_00_00);  // black bg
        t.circle(0.5, 0.5, 0.4, 0x00FF_FFFF); // cyan circle
        let buf = t.ensure_rendered();
        // Centre pixel should be cyan
        let centre = buf[32 * 64 + 32];
        let r = (centre >> 16) & 0xFF;
        let g = (centre >> 8) & 0xFF;
        let b = centre & 0xFF;
        assert!(r > 200 && g > 200 && b > 200, "centre should be near-white (cyan at full coverage)");
    }

    #[test]
    fn gradient_transitions() {
        let mut t = VtexRT::new(64, 4);
        t.gradient(0.0, 0.5, 1.0, 0.5, 0x00FF_0000, 0x0000_00FF); // red→blue horizontal
        let buf = t.ensure_rendered();
        let left  = buf[0];        // ~red
        let right = buf[63];       // ~blue
        let lr = (left  >> 16) & 0xFF;
        let lb = left  & 0xFF;
        let rr = (right >> 16) & 0xFF;
        let rb = right & 0xFF;
        assert!(lr > rb, "left should be more red");
        assert!(rb > lb, "right should be more blue");
    }
}
