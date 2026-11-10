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
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
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
pub fn fan_emit_3d<F>(xs: &[f32], ys: &[f32], zs: &[f32], cs: &[u32], n: usize, mut emit: F)
where
    F: FnMut(f32, f32, f32, u32, f32, f32, f32, u32, f32, f32, f32, u32),
{
    if n < 3 {
        return;
    }
    let ax = xs[0];
    let ay = ys[0];
    let az = zs[0];
    let ac = cs[0];
    for i in 1..n - 1 {
        emit(
            ax,
            ay,
            az,
            ac,
            xs[i],
            ys[i],
            zs[i],
            cs[i],
            xs[i + 1],
            ys[i + 1],
            zs[i + 1],
            cs[i + 1],
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
    input: &[(f32, f32, f32, f32, u32)], // (wx, wy, wz, cam_depth, color)
    n_in: usize,
    near: f32,
    output: &mut [(f32, f32, f32, f32, u32); MAX_CLIP_VERTS],
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
#[allow(clippy::too_many_arguments)]
pub fn face_normal(
    ax: f32,
    ay: f32,
    az: f32,
    bx: f32,
    by: f32,
    bz: f32,
    cx: f32,
    cy: f32,
    cz: f32,
) -> [f32; 3] {
    let ux = bx - ax;
    let uy = by - ay;
    let uz = bz - az;
    let vx = cx - ax;
    let vy = cy - ay;
    let vz = cz - az;
    [uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx]
}

/// Is this triangle soup a closed, consistently wound surface — and which way
/// round? `+1` closed with positive signed volume (outward-facing by the
/// right-hand rule), `-1` closed and inside-out, `0` open / inconsistent (a
/// card, a cloth or hair sheet, a house without a floor) — such meshes must
/// stay double-sided. Vertices are welded by position (meshes split vertices at
/// UV seams); up to 1% unmatched edges are tolerated for tiny cracks. Used by
/// `set_cull(3)` (auto backface culling).
pub fn closed_mesh_orientation<I: IntoIterator<Item = [[f32; 3]; 3]>>(tris: I) -> i8 {
    use std::collections::HashMap;
    let tris: Vec<[[f32; 3]; 3]> = tris.into_iter().collect();
    if tris.len() < 4 {
        return 0;
    }
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for t in &tris {
        for p in t {
            for k in 0..3 {
                if !p[k].is_finite() {
                    return 0;
                }
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    let diag = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt();
    if diag <= 0.0 {
        return 0;
    }
    let eps = diag * 1e-5;
    let mut ids: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut id = |p: [f32; 3]| -> u32 {
        let key = ((p[0] / eps).round() as i64, (p[1] / eps).round() as i64, (p[2] / eps).round() as i64);
        let n = ids.len() as u32;
        *ids.entry(key).or_insert(n)
    };
    let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
    let mut vol = 0.0f64;
    for t in &tris {
        let (a, b, c) = (id(t[0]), id(t[1]), id(t[2]));
        if a == b || b == c || c == a {
            continue; // degenerate
        }
        for e in [(a, b), (b, c), (c, a)] {
            *edges.entry(e).or_insert(0) += 1;
        }
        let (p, q, r) = (t[0], t[1], t[2]);
        let (p0, p1, p2) = (p[0] as f64, p[1] as f64, p[2] as f64);
        let (q0, q1, q2) = (q[0] as f64, q[1] as f64, q[2] as f64);
        let (r0, r1, r2) = (r[0] as f64, r[1] as f64, r[2] as f64);
        vol += p0 * (q1 * r2 - q2 * r1) - p1 * (q0 * r2 - q2 * r0) + p2 * (q0 * r1 - q1 * r0);
    }
    if edges.is_empty() {
        return 0;
    }
    let bad = edges.iter().filter(|(&(a, b), &n)| n != 1 || edges.get(&(b, a)).copied() != Some(1)).count();
    if bad * 100 > edges.len() {
        return 0;
    }
    if vol.abs() < (diag as f64).powi(3) * 1e-9 {
        return 0;
    }
    if vol > 0.0 { 1 } else { -1 }
}

/// The `cull` mode (1 = drop clockwise-on-screen, 2 = counter-clockwise) that
/// removes the back faces of a closed mesh with orientation `orient` (see
/// [`closed_mesh_orientation`]) drawn with a scale of sign `mirror` (< 0
/// flips the winding). 0 = don't cull.
pub fn auto_cull_mode(orient: i8, mirror: f32) -> u8 {
    let o = if mirror < 0.0 { -orient } else { orient };
    match o {
        1 => AUTO_CULL_POSITIVE,
        -1 => 3 - AUTO_CULL_POSITIVE,
        _ => 0,
    }
}

/// Screen-winding cull mode that hides the back faces of a positive-volume
/// (outward, right-hand-rule) mesh. Pinned by `gfx::cull_tests`.
pub const AUTO_CULL_POSITIVE: u8 = 1;
