//! Minimal built-in 2-D physics (no external deps required).
//! Avian integration is available via the `physics2d` / `physics3d` features.

#[derive(Debug, Clone, Copy, Default)]
pub struct Vec2 { pub x: f32, pub y: f32 }

impl Vec2 {
    pub fn new(x: f32, y: f32) -> Self { Self { x, y } }
    pub fn len(self) -> f32 { (self.x * self.x + self.y * self.y).sqrt() }
    pub fn normalise(self) -> Self {
        let l = self.len();
        if l < 1e-9 { Self::default() } else { Self::new(self.x / l, self.y / l) }
    }
    pub fn dot(self, rhs: Self) -> f32 { self.x * rhs.x + self.y * rhs.y }
}

impl std::ops::Add for Vec2 { type Output = Self; fn add(self, r: Self) -> Self { Self::new(self.x + r.x, self.y + r.y) } }
impl std::ops::Sub for Vec2 { type Output = Self; fn sub(self, r: Self) -> Self { Self::new(self.x - r.x, self.y - r.y) } }
impl std::ops::Mul<f32> for Vec2 { type Output = Self; fn mul(self, s: f32) -> Self { Self::new(self.x * s, self.y * s) } }

#[derive(Debug, Clone, Copy, Default)]
pub struct RigidBody {
    pub pos:      Vec2,
    pub vel:      Vec2,
    pub mass:     f32,
    pub restitution: f32,
}

impl RigidBody {
    pub fn new(x: f32, y: f32, mass: f32) -> Self {
        Self { pos: Vec2::new(x, y), vel: Vec2::default(), mass: mass.max(1e-6), restitution: 0.5 }
    }

    pub fn apply_force(&mut self, force: Vec2, dt: f32) {
        let accel = Vec2::new(force.x / self.mass, force.y / self.mass);
        self.vel  = self.vel + accel * dt;
    }

    pub fn integrate(&mut self, dt: f32) {
        self.pos = self.pos + self.vel * dt;
    }
}

/// Axis-aligned bounding box collision test.
#[derive(Debug, Clone, Copy)]
pub struct Aabb { pub min: Vec2, pub max: Vec2 }

impl Aabb {
    pub fn new(cx: f32, cy: f32, hw: f32, hh: f32) -> Self {
        Self { min: Vec2::new(cx - hw, cy - hh), max: Vec2::new(cx + hw, cy + hh) }
    }
    pub fn overlaps(self, other: Self) -> bool {
        self.min.x < other.max.x && self.max.x > other.min.x
        && self.min.y < other.max.y && self.max.y > other.min.y
    }
}
