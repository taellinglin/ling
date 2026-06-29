// src/gfx/poly.rs — Polygon fan triangulation + per-frame shared-edge deduplication.
//
// Design:
//   Quads, pentagons, hexagons (and arbitrary convex n-gons) are split into
//   (n−2) triangles anchored at vertex 0 — the standard "fan" decomposition.
//   Zero heap allocation: the caller supplies a fixed-size array.
//
//   EdgeSet deduplicates draw_line_3d calls so shared edges between adjacent
//   faces are drawn exactly once per frame.  Keys are quantised world-space
//   endpoint pairs → hash-set of sorted (u64, u64) tuples.  Clear at frame
//   start (clear_screen).

use std::collections::HashSet;

// ── Edge deduplication ────────────────────────────────────────────────────────

/// Canonical edge: always (min, max) so direction doesn't matter.
#[inline]
fn canonical(a: u64, b: u64) -> (u64, u64) {
    if a <= b { (a, b) } else { (b, a) }
}

/// Encode a 3-D world-space point to a u64 key quantised to 1/64 unit.
/// Two points within ~0.016 world units hash the same — enough for shared
/// edges that are geometrically coincident but computed separately.
#[inline]
pub fn world_id(x: f32, y: f32, z: f32) -> u64 {
    let xi = (x * 64.0).round() as i16;
    let yi = (y * 64.0).round() as i16;
    let zi = (z * 64.0).round() as i16;
    ((xi as u16 as u64) << 32) | ((yi as u16 as u64) << 16) | (zi as u16 as u64)
}

/// Per-frame edge table.  Call `clear()` whenever the screen is cleared.
pub struct EdgeSet {
    set: HashSet<(u64, u64)>,
}

impl Default for EdgeSet {
    fn default() -> Self {
        Self { set: HashSet::with_capacity(1024) }
    }
}

impl EdgeSet {
    /// Returns `true` if this world-space edge is new this frame (and marks it).
    /// Returns `false` when the shared edge has already been emitted — caller skips.
    #[inline]
    pub fn try_insert(&mut self, x0: f32, y0: f32, z0: f32, x1: f32, y1: f32, z1: f32) -> bool {
        let a = world_id(x0, y0, z0);
        let b = world_id(x1, y1, z1);
        self.set.insert(canonical(a, b))
    }

    #[inline]
    pub fn clear(&mut self) {
        self.set.clear();
    }
}

// ── Fan triangulation — 3-D world space ──────────────────────────────────────

/// Fan-triangulate a 3-D polygon given as a parallel array of world-space
/// (x, y, z) components and per-vertex lit colours.
///
/// Calls `emit(x0,y0,z0,c0, x1,y1,z1,c1, x2,y2,z2,c2)` for each triangle.
/// At least 3 vertices required; silently does nothing for fewer.
///
/// The arrays must all be at least `n` elements; `n` must be ≤ the array length.
#[inline]
pub fn fan_emit_3d<F>(
    xs: &[f32],
    ys: &[f32],
    zs: &[f32],
    cs: &[u32],
    n: usize,
    mut emit: F,
)
where
    F: FnMut(f32, f32, f32, u32, f32, f32, f32, u32, f32, f32, f32, u32),
{
    if n < 3 {
        return;
    }
    let ax = xs[0]; let ay = ys[0]; let az = zs[0]; let ac = cs[0];
    for i in 1..n - 1 {
        emit(
            ax, ay, az, ac,
            xs[i],     ys[i],     zs[i],     cs[i],
            xs[i + 1], ys[i + 1], zs[i + 1], cs[i + 1],
        );
    }
}

// ── Projected-polygon fan triangulation (screen space) ───────────────────────

/// Fan-triangulate an already-projected polygon.  Each element is
/// `(screen_x, screen_y, camera_z, colour)`.
///
/// Calls `emit(sx0,sy0,sz0,c0, sx1,sy1,sz1,c1, sx2,sy2,sz2,c2)`.
#[inline]
pub fn fan_emit_proj<F>(poly: &[(f32, f32, f32, u32)], n: usize, mut emit: F)
where
    F: FnMut(f32, f32, f32, u32, f32, f32, f32, u32, f32, f32, f32, u32),
{
    if n < 3 {
        return;
    }
    let (ax, ay, az, ac) = poly[0];
    for i in 1..n - 1 {
        let (bx, by, bz, bc) = poly[i];
        let (cx, cy, cz, cc) = poly[i + 1];
        emit(ax, ay, az, ac, bx, by, bz, bc, cx, cy, cz, cc);
    }
}

// ── Near-plane Sutherland–Hodgman clip ────────────────────────────────────────

/// Maximum polygon size after near-plane clip: input n-gon → at most n+1 vertices.
pub const MAX_CLIP_VERTS: usize = 9; // sufficient for hex (6) + 3 extra

/// Clip an n-gon against the near plane `near_depth`.  Input and output are
/// `(wx, wy, wz, cam_depth, colour)` tuples in a fixed-size stack array.
/// Returns the number of output vertices (0..=MAX_CLIP_VERTS).
///
/// Uses `lerp_color` for interpolated vertex colours at clip edges.
#[allow(clippy::too_many_arguments)]
pub fn clip_near(
    input:      &[(f32, f32, f32, f32, u32)], // (wx, wy, wz, cam_depth, color)
    n_in:       usize,
    near:       f32,
    output:     &mut [(f32, f32, f32, f32, u32); MAX_CLIP_VERTS],
) -> usize {
    let mut n_out = 0usize;
    for ei in 0..n_in {
        let a = input[ei];
        let b = input[(ei + 1) % n_in];
        let a_in = a.3 > near;
        let b_in = b.3 > near;
        if a_in && n_out < MAX_CLIP_VERTS {
            output[n_out] = a;
            n_out += 1;
        }
        if a_in != b_in && n_out < MAX_CLIP_VERTS {
            let t = (near - a.3) / (b.3 - a.3);
            output[n_out] = (
                a.0 + (b.0 - a.0) * t,
                a.1 + (b.1 - a.1) * t,
                a.2 + (b.2 - a.2) * t,
                near,
                lerp_color(a.4, b.4, t),
            );
            n_out += 1;
        }
    }
    n_out
}

/// Linear-interpolate two 0x00RRGGBB colours.
#[inline]
pub fn lerp_color(a: u32, b: u32, t: f32) -> u32 {
    let ar = ((a >> 16) & 0xFF) as f32;
    let ag = ((a >> 8) & 0xFF) as f32;
    let ab = (a & 0xFF) as f32;
    let br = ((b >> 16) & 0xFF) as f32;
    let bg = ((b >> 8) & 0xFF) as f32;
    let bb = (b & 0xFF) as f32;
    let r = (ar + (br - ar) * t).clamp(0.0, 255.0) as u32;
    let g = (ag + (bg - ag) * t).clamp(0.0, 255.0) as u32;
    let bl = (ab + (bb - ab) * t).clamp(0.0, 255.0) as u32;
    (r << 16) | (g << 8) | bl
}

// ── Face-normal helper ────────────────────────────────────────────────────────

/// World-space face normal (B−A) × (C−A).  Not normalised.
#[inline]
pub fn face_normal(
    ax: f32, ay: f32, az: f32,
    bx: f32, by: f32, bz: f32,
    cx: f32, cy: f32, cz: f32,
) -> [f32; 3] {
    let ux = bx - ax; let uy = by - ay; let uz = bz - az;
    let vx = cx - ax; let vy = cy - ay; let vz = cz - az;
    [uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx]
}
