//! Rigid-body physics: integration, AABB collision, impulse response.

use glam::Vec3;

#[derive(Clone, Debug)]
pub struct RigidBody {
    pub pos:         Vec3,
    pub vel:         Vec3,
    pub acc:         Vec3,
    pub mass:        f32,
    pub restitution: f32,   // bounciness 0..1
    pub friction:    f32,   // 0..1
    pub half_extents: Vec3, // AABB half-size
    pub gravity_scale: f32,
    pub is_static:   bool,
}

impl RigidBody {
    pub fn new(pos: Vec3, mass: f32) -> Self {
        Self {
            pos,
            vel: Vec3::ZERO,
            acc: Vec3::ZERO,
            mass,
            restitution: 0.5,
            friction: 0.3,
            half_extents: Vec3::splat(0.5),
            gravity_scale: 1.0,
            is_static: false,
        }
    }

    pub fn apply_force(&mut self, force: Vec3) {
        if !self.is_static && self.mass > 0.0 {
            self.acc += force / self.mass;
        }
    }

    pub fn apply_impulse(&mut self, impulse: Vec3) {
        if !self.is_static && self.mass > 0.0 {
            self.vel += impulse / self.mass;
        }
    }

    /// Semi-implicit Euler integration with gravity.
    pub fn integrate(&mut self, dt: f32, gravity: Vec3) {
        if self.is_static { return; }
        self.acc += gravity * self.gravity_scale;
        self.vel += self.acc * dt;
        self.pos += self.vel * dt;
        self.acc = Vec3::ZERO;
    }

    pub fn aabb_min(&self) -> Vec3 { self.pos - self.half_extents }
    pub fn aabb_max(&self) -> Vec3 { self.pos + self.half_extents }

    pub fn overlaps(&self, other: &RigidBody) -> bool {
        let a_min = self.aabb_min();
        let a_max = self.aabb_max();
        let b_min = other.aabb_min();
        let b_max = other.aabb_max();
        a_min.x <= b_max.x && a_max.x >= b_min.x
            && a_min.y <= b_max.y && a_max.y >= b_min.y
            && a_min.z <= b_max.z && a_max.z >= b_min.z
    }
}

/// Resolve an AABB collision between two bodies (impulse method).
pub fn resolve_collision(a: &mut RigidBody, b: &mut RigidBody) {
    if !a.overlaps(b) { return; }

    let a_min = a.aabb_min(); let a_max = a.aabb_max();
    let b_min = b.aabb_min(); let b_max = b.aabb_max();

    // Find smallest overlap axis.
    let overlaps = [
        (b_max.x - a_min.x, Vec3::X),
        (a_max.x - b_min.x, Vec3::NEG_X),
        (b_max.y - a_min.y, Vec3::Y),
        (a_max.y - b_min.y, Vec3::NEG_Y),
        (b_max.z - a_min.z, Vec3::Z),
        (a_max.z - b_min.z, Vec3::NEG_Z),
    ];

    let (depth, normal) = overlaps.iter()
        .min_by(|x, y| x.0.partial_cmp(&y.0).unwrap())
        .cloned()
        .unwrap();

    let restitution = (a.restitution + b.restitution) * 0.5;
    let rel_vel = (a.vel - b.vel).dot(normal);

    if rel_vel > 0.0 { return; } // already separating

    let inv_a = if a.is_static { 0.0 } else { 1.0 / a.mass };
    let inv_b = if b.is_static { 0.0 } else { 1.0 / b.mass };
    let j = -(1.0 + restitution) * rel_vel / (inv_a + inv_b).max(1e-6);

    let impulse = normal * j;
    a.apply_impulse(impulse);
    b.apply_impulse(-impulse);

    // Positional correction (Baumgarte).
    let correction = normal * (depth * 0.8 / (inv_a + inv_b).max(1e-6));
    if !a.is_static { a.pos += correction * inv_a; }
    if !b.is_static { b.pos -= correction * inv_b; }
}

/// Simple physics world holding all rigid bodies.
pub struct PhysicsWorld {
    pub bodies:  Vec<RigidBody>,
    pub gravity: Vec3,
}

impl PhysicsWorld {
    pub fn new() -> Self {
        Self { bodies: Vec::new(), gravity: Vec3::new(0.0, -9.81, 0.0) }
    }

    pub fn add(&mut self, body: RigidBody) -> usize {
        let id = self.bodies.len();
        self.bodies.push(body);
        id
    }

    pub fn step(&mut self, dt: f32) {
        for b in &mut self.bodies {
            b.integrate(dt, self.gravity);
        }
        // Naive O(n²) broadphase — fine for small scenes.
        for i in 0..self.bodies.len() {
            for j in (i + 1)..self.bodies.len() {
                let (left, right) = self.bodies.split_at_mut(j);
                resolve_collision(&mut left[i], &mut right[0]);
            }
        }
    }
}

impl Default for PhysicsWorld {
    fn default() -> Self { Self::new() }
}
