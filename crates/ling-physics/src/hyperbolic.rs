//! Poincaré ball model of hyperbolic geometry.
//!
//! A point `p` in the unit open ball (|p| < 1) represents a position in ℍ³.
//!
//! Useful for:
//! - Hyperbolic Minecraft-style rooms on the inside of a sphere
//! - {5,4} pentagonal tilings
//! - Gravity pointing outward on a sphere's interior
//! - Long-exposure trails (geodesic paths look psychedelic)

use glam::Vec3;

const EPSILON: f32 = 1e-6;

// ── Core operations ───────────────────────────────────────────────────────────

/// Conformal factor at point `p` in the Poincaré ball.
#[inline]
pub fn lambda(p: Vec3) -> f32 {
    let r2 = p.length_squared();
    2.0 / (1.0 - r2).max(EPSILON)
}

/// Hyperbolic distance between two Poincaré-ball points.
pub fn distance(a: Vec3, b: Vec3) -> f32 {
    let diff2 = (a - b).length_squared();
    let a2    = a.length_squared();
    let b2    = b.length_squared();
    let denom = ((1.0 - a2) * (1.0 - b2)).max(EPSILON);
    let arg   = 1.0 + 2.0 * diff2 / denom;
    arg.max(1.0).acosh()
}

/// Möbius addition (translation by `y` in hyperbolic space).
pub fn mobius_add(x: Vec3, y: Vec3) -> Vec3 {
    let x2  = x.length_squared();
    let y2  = y.length_squared();
    let xy  = x.dot(y);
    let denom = 1.0 + 2.0 * xy + x2 * y2;
    if denom.abs() < EPSILON { return x; }
    ((1.0 + 2.0 * xy + y2) * x + (1.0 - x2) * y) / denom
}

/// Exponential map: geodesic starting at `base` in the direction `v` (in tangent space).
pub fn exp_map(base: Vec3, v: Vec3) -> Vec3 {
    let lam = lambda(base);
    let vn  = v.length();
    if vn < EPSILON { return base; }
    let tanh_arg = (lam * vn * 0.5).tanh();
    let dir = v / vn;
    mobius_add(base, dir * tanh_arg)
}

/// Logarithmic map: tangent vector from `base` to `target`.
pub fn log_map(base: Vec3, target: Vec3) -> Vec3 {
    let minus_base = mobius_add(-base, target);
    let norm = minus_base.length();
    if norm < EPSILON { return Vec3::ZERO; }
    let lam = lambda(base);
    let scale = (2.0 / lam) * norm.atanh();
    minus_base * (scale / norm)
}

/// Parallel transport of tangent vector `v` along geodesic from `a` to `b`.
pub fn parallel_transport(a: Vec3, b: Vec3, v: Vec3) -> Vec3 {
    let gyrovec = mobius_add(-a, b);
    let norm = gyrovec.length();
    if norm < EPSILON { return v; }
    let cos = gyrovec.dot(v) / norm;
    let proj = gyrovec * (cos / norm);
    let perp = v - proj;
    perp + proj // simplified — for full transport use the gyrovector formula
}

// ── Hyperbolic sphere world ───────────────────────────────────────────────────

/// A spherical world whose interior is hyperbolic.
/// Players walk on the *inside* of the sphere; gravity points outward.
#[derive(Clone, Debug)]
pub struct HyperbolicSphereWorld {
    /// Sphere radius in world units.
    pub radius:    f32,
    /// Hyperbolic curvature (negative, default -1.0).
    pub curvature: f32,
    /// Gravitational acceleration magnitude.
    pub gravity:   f32,
}

impl Default for HyperbolicSphereWorld {
    fn default() -> Self {
        Self { radius: 100.0, curvature: -1.0, gravity: 9.81 }
    }
}

impl HyperbolicSphereWorld {
    /// Gravity direction for a player at world position `pos` (Euclidean coords).
    /// Points *away* from sphere centre so the player is pressed against the inside shell.
    pub fn gravity_dir(&self, pos: Vec3) -> Vec3 {
        let dir = pos.normalize_or(Vec3::Y);
        dir // outward
    }

    /// Gravity force vector.
    pub fn gravity_force(&self, pos: Vec3, mass: f32) -> Vec3 {
        self.gravity_dir(pos) * self.gravity * mass
    }

    /// Convert a position in Euclidean world space to a Poincaré ball coordinate.
    pub fn to_poincare(&self, world_pos: Vec3) -> Vec3 {
        world_pos / self.radius
    }

    /// Convert Poincaré coordinate back to world space.
    pub fn from_poincare(&self, p: Vec3) -> Vec3 {
        p * self.radius
    }

    /// Hyperbolic distance between two world-space points.
    pub fn world_distance(&self, a: Vec3, b: Vec3) -> f32 {
        distance(self.to_poincare(a), self.to_poincare(b))
    }

    /// "Up" direction at `pos` — points inward toward sphere centre.
    pub fn up_at(&self, pos: Vec3) -> Vec3 {
        -pos.normalize_or(Vec3::Y)
    }
}

// ── Pentagonal (5-sided) tiling helpers ──────────────────────────────────────

/// Vertices of a regular hyperbolic pentagon centred at the origin in ℍ².
/// In the {5,4} tiling (4 pentagons per vertex), the circumradius is:
///   r = acosh( cos(π/5) / sin(π/4) ) / acosh( cos(π/5) / sin(2π/5) )
/// We just return unit-disc positions for the 5 vertices.
pub fn pentagon_vertices() -> [[f32; 2]; 5] {
    // circumradius in Poincaré disk for {5,4}
    let r: f32 = {
        let cos_pi5 = (std::f32::consts::PI / 5.0).cos();
        let sin_pi4 = (std::f32::consts::PI / 4.0).sin();
        let sin_2pi5 = (2.0 * std::f32::consts::PI / 5.0).sin();
        let cosh_r = cos_pi5 / sin_pi4;
        let _cosh_r2 = cos_pi5 / sin_2pi5;
        // r = arccosh(cosh_r) but we need Poincaré radius = tanh(d/2)
        let d = cosh_r.max(1.0).acosh();
        d.tanh()
    };
    let mut verts = [[0.0f32; 2]; 5];
    for i in 0..5 {
        let angle = i as f32 * 2.0 * std::f32::consts::PI / 5.0 - std::f32::consts::FRAC_PI_2;
        verts[i] = [r * angle.cos(), r * angle.sin()];
    }
    verts
}

/// Translate a pentagon centred at origin to a Poincaré ball position `centre`.
pub fn translate_pentagon(centre: Vec3, vertices: &[[f32; 2]; 5]) -> Vec<Vec3> {
    vertices.iter().map(|&[x, y]| {
        let p = Vec3::new(x, 0.0, y);
        let wp = mobius_add(centre, p);
        wp
    }).collect()
}

/// Generate the first `depth` rings of a {5,4} tiling centred at the origin.
pub fn pentagon_tiling(depth: u32) -> Vec<Vec3> {
    let mut centres = vec![Vec3::ZERO];
    let base_verts  = pentagon_vertices();
    // Each adjacent pentagon is centred at a vertex translated outward.
    for _ in 0..depth {
        let mut new_centres = Vec::new();
        for &c in &centres {
            for &[vx, vy] in &base_verts {
                let v = Vec3::new(vx, 0.0, vy);
                let adjacent = mobius_add(c, v * 0.8); // approx neighbour centre
                if adjacent.length() < 0.99 {
                    new_centres.push(adjacent);
                }
            }
        }
        centres.extend(new_centres);
        centres.dedup_by(|a, b| (*a - *b).length() < 0.01);
    }
    centres
}
