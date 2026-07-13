pub use glam::{IVec2, IVec3, Mat3, Mat4, Quat, UVec2, Vec2, Vec3, Vec4};

// ── 4D vector (spatial, not homogeneous) ─────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Vec4H {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Vec4H {
    pub const ONE: Self = Self { x: 1.0, y: 1.0, z: 1.0, w: 1.0 };
    pub const ZERO: Self = Self { x: 0.0, y: 0.0, z: 0.0, w: 0.0 };

    pub fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    pub fn dot(&self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }

    /// Minkowski inner product: -x₀y₀ + x₁y₁ + x₂y₂ + x₃y₃ (hyperboloid model)
    pub fn minkowski_dot(&self, other: Self) -> f32 {
        -self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }

    pub fn length(&self) -> f32 {
        self.dot(*self).sqrt()
    }

    pub fn normalize(&self) -> Self {
        let l = self.length();
        if l < 1e-8 {
            return *self;
        }
        Self::new(self.x / l, self.y / l, self.z / l, self.w / l)
    }

    pub fn lerp(&self, other: Self, t: f32) -> Self {
        Self::new(
            self.x + (other.x - self.x) * t,
            self.y + (other.y - self.y) * t,
            self.z + (other.z - self.z) * t,
            self.w + (other.w - self.w) * t,
        )
    }

    pub fn xyz(&self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }
}

impl std::ops::Add for Vec4H {
    type Output = Self;

    fn add(self, r: Self) -> Self {
        Self::new(self.x + r.x, self.y + r.y, self.z + r.z, self.w + r.w)
    }
}
impl std::ops::Sub for Vec4H {
    type Output = Self;

    fn sub(self, r: Self) -> Self {
        Self::new(self.x - r.x, self.y - r.y, self.z - r.z, self.w - r.w)
    }
}
impl std::ops::Mul<f32> for Vec4H {
    type Output = Self;

    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s, self.w * s)
    }
}
impl std::ops::Neg for Vec4H {
    type Output = Self;

    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z, -self.w)
    }
}

// ── 5×5 matrix for homogeneous 4D transforms ─────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Mat5(pub [[f32; 5]; 5]);

impl Mat5 {
    pub fn identity() -> Self {
        let mut m = [[0f32; 5]; 5];
        for i in 0..5 {
            m[i][i] = 1.0;
        }
        Self(m)
    }

    pub fn zero() -> Self {
        Self([[0f32; 5]; 5])
    }

    pub fn mul_vec(&self, v: [f32; 5]) -> [f32; 5] {
        let mut out = [0f32; 5];
        for i in 0..5 {
            for j in 0..5 {
                out[i] += self.0[i][j] * v[j];
            }
        }
        out
    }

    pub fn mul(&self, rhs: &Self) -> Self {
        let mut out = [[0f32; 5]; 5];
        for i in 0..5 {
            for j in 0..5 {
                for k in 0..5 {
                    out[i][j] += self.0[i][k] * rhs.0[k][j];
                }
            }
        }
        Self(out)
    }

    pub fn transpose(&self) -> Self {
        let mut out = [[0f32; 5]; 5];
        for i in 0..5 {
            for j in 0..5 {
                out[i][j] = self.0[j][i];
            }
        }
        Self(out)
    }

    /// 4D translation matrix (shifts along the w axis of hyperbolic space)
    pub fn translation_4d(delta: Vec4H) -> Self {
        let mut m = Self::identity();
        m.0[0][4] = delta.x;
        m.0[1][4] = delta.y;
        m.0[2][4] = delta.z;
        m.0[3][4] = delta.w;
        m
    }
}

// ── Axis-aligned bounding box ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    pub fn from_points(points: &[Vec3]) -> Self {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for &p in points {
            min = min.min(p);
            max = max.max(p);
        }
        Self { min, max }
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn half_extents(&self) -> Vec3 {
        (self.max - self.min) * 0.5
    }

    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    pub fn contains(&self, p: Vec3) -> bool {
        p.x >= self.min.x
            && p.x <= self.max.x
            && p.y >= self.min.y
            && p.y <= self.max.y
            && p.z >= self.min.z
            && p.z <= self.max.z
    }

    pub fn union(&self, other: &Self) -> Self {
        Self { min: self.min.min(other.min), max: self.max.max(other.max) }
    }

    pub fn expand(&self, by: f32) -> Self {
        Self {
            min: self.min - Vec3::splat(by),
            max: self.max + Vec3::splat(by),
        }
    }
}

// ── Ray ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray3 {
    pub origin: Vec3,
    pub direction: Vec3,
}

impl Ray3 {
    pub fn new(origin: Vec3, direction: Vec3) -> Self {
        Self { origin, direction: direction.normalize() }
    }

    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.direction * t
    }

    pub fn intersect_sphere(&self, center: Vec3, radius: f32) -> Option<f32> {
        let oc = self.origin - center;
        let b = oc.dot(self.direction);
        let c = oc.dot(oc) - radius * radius;
        let disc = b * b - c;
        if disc < 0.0 {
            return None;
        }
        let t = -b - disc.sqrt();
        if t > 0.0 {
            Some(t)
        } else {
            let t2 = -b + disc.sqrt();
            if t2 > 0.0 {
                Some(t2)
            } else {
                None
            }
        }
    }

    pub fn intersect_aabb(&self, aabb: &Aabb) -> Option<f32> {
        let inv_d = Vec3::ONE / self.direction;
        let t1 = (aabb.min - self.origin) * inv_d;
        let t2 = (aabb.max - self.origin) * inv_d;
        let tmin = t1.min(t2);
        let tmax = t1.max(t2);
        let enter = tmin.x.max(tmin.y).max(tmin.z);
        let exit = tmax.x.min(tmax.y).min(tmax.z);
        if exit >= enter && exit >= 0.0 {
            Some(enter.max(0.0))
        } else {
            None
        }
    }

    pub fn intersect_plane(&self, plane: &Plane) -> Option<f32> {
        let denom = plane.normal.dot(self.direction);
        if denom.abs() < 1e-8 {
            return None;
        }
        let t = -(plane.normal.dot(self.origin) + plane.d) / denom;
        if t >= 0.0 {
            Some(t)
        } else {
            None
        }
    }
}

// ── Plane ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub normal: Vec3,
    pub d: f32,
}

impl Plane {
    pub fn new(normal: Vec3, d: f32) -> Self {
        Self { normal: normal.normalize(), d }
    }

    pub fn from_point_normal(point: Vec3, normal: Vec3) -> Self {
        let n = normal.normalize();
        Self { normal: n, d: -n.dot(point) }
    }

    pub fn from_three_points(a: Vec3, b: Vec3, c: Vec3) -> Self {
        let n = (b - a).cross(c - a).normalize();
        Self::from_point_normal(a, n)
    }

    pub fn signed_distance(&self, p: Vec3) -> f32 {
        self.normal.dot(p) + self.d
    }
}

// ── View frustum ──────────────────────────────────────────────────────────────

pub struct Frustum {
    planes: [Plane; 6],
}

impl Frustum {
    /// Extract frustum planes from a combined view-projection matrix
    /// using the Gribb/Hartmann method.
    pub fn from_view_proj(vp: Mat4) -> Self {
        let cols = vp.to_cols_array_2d(); // [col][row]
        let get_row = |i: usize| Vec4::new(cols[0][i], cols[1][i], cols[2][i], cols[3][i]);
        let r0 = get_row(0);
        let r1 = get_row(1);
        let r2 = get_row(2);
        let r3 = get_row(3);

        let make = |v: Vec4| Plane::new(Vec3::new(v.x, v.y, v.z), v.w);

        Self {
            planes: [
                make(r3 + r0), // left
                make(r3 - r0), // right
                make(r3 + r1), // bottom
                make(r3 - r1), // top
                make(r3 + r2), // near
                make(r3 - r2), // far
            ],
        }
    }

    pub fn contains_point(&self, p: Vec3) -> bool {
        self.planes
            .iter()
            .all(|plane| plane.signed_distance(p) >= 0.0)
    }

    pub fn contains_aabb(&self, aabb: &Aabb) -> bool {
        for plane in &self.planes {
            let positive = Vec3::new(
                if plane.normal.x > 0.0 {
                    aabb.max.x
                } else {
                    aabb.min.x
                },
                if plane.normal.y > 0.0 {
                    aabb.max.y
                } else {
                    aabb.min.y
                },
                if plane.normal.z > 0.0 {
                    aabb.max.z
                } else {
                    aabb.min.z
                },
            );
            if plane.signed_distance(positive) < 0.0 {
                return false;
            }
        }
        true
    }

    pub fn contains_sphere(&self, center: Vec3, radius: f32) -> bool {
        self.planes
            .iter()
            .all(|p| p.signed_distance(center) >= -radius)
    }
}
