//! Rigid-body physics: linear + **angular** integration, AABB collision, impulse
//! response, and a spin-inducing floor bounce (so bodies tumble and roll).

use glam::{Quat, Vec3};

#[derive(Clone, Debug)]
pub struct RigidBody {
    pub pos: Vec3,
    pub vel: Vec3,
    pub acc: Vec3,
    pub mass: f32,
    pub restitution: f32,   // bounciness 0..1
    pub friction: f32,      // 0..1
    pub half_extents: Vec3, // AABB half-size
    pub gravity_scale: f32,
    pub is_static: bool,
    // ── angular state ──
    pub orientation: Quat, // current rotation
    pub ang_vel: Vec3,     // angular velocity (rad/s, world axes)
    pub torque: Vec3,      // accumulated torque this step
    pub inv_inertia: f32,  // scalar inverse moment of inertia (sphere approx)
}

impl RigidBody {
    pub fn new(pos: Vec3, mass: f32) -> Self {
        let he = Vec3::splat(0.5);
        Self {
            pos,
            vel: Vec3::ZERO,
            acc: Vec3::ZERO,
            mass,
            restitution: 0.5,
            friction: 0.3,
            half_extents: he,
            gravity_scale: 1.0,
            is_static: false,
            orientation: Quat::IDENTITY,
            ang_vel: Vec3::ZERO,
            torque: Vec3::ZERO,
            inv_inertia: Self::inv_inertia_for(mass, he),
        }
    }

    /// Solid-sphere moment of inertia I = 2/5·m·r² (r = largest half-extent).
    fn inv_inertia_for(mass: f32, he: Vec3) -> f32 {
        let r = he.max_element().max(1e-3);
        let i = 0.4 * mass.max(1e-6) * r * r;
        if i > 1e-9 {
            1.0 / i
        } else {
            0.0
        }
    }

    /// Recompute the inertia after changing mass/half_extents.
    pub fn refresh_inertia(&mut self) {
        self.inv_inertia = Self::inv_inertia_for(self.mass, self.half_extents);
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

    /// Accumulate a torque (world axes) for this step.
    pub fn apply_torque(&mut self, t: Vec3) {
        if !self.is_static {
            self.torque += t;
        }
    }

    /// Instantly add to the angular velocity.
    pub fn apply_spin(&mut self, w: Vec3) {
        if !self.is_static {
            self.ang_vel += w;
        }
    }

    /// Apply an impulse at a world-space contact point — produces both linear
    /// and angular response (the lever arm `point - pos` creates spin).
    pub fn apply_impulse_at(&mut self, impulse: Vec3, point: Vec3) {
        if self.is_static || self.mass <= 0.0 {
            return;
        }
        self.vel += impulse / self.mass;
        let r = point - self.pos;
        self.ang_vel += r.cross(impulse) * self.inv_inertia;
    }

    /// Semi-implicit Euler integration with gravity, plus angular integration
    /// (quaternion derivative) so the body tumbles.
    pub fn integrate(&mut self, dt: f32, gravity: Vec3) {
        if self.is_static {
            return;
        }
        self.acc += gravity * self.gravity_scale;
        self.vel += self.acc * dt;
        self.pos += self.vel * dt;
        self.acc = Vec3::ZERO;

        // angular: ω += I⁻¹·τ·dt ; q += ½·(ω as quat)·q·dt ; renormalise
        self.ang_vel += self.torque * self.inv_inertia * dt;
        self.torque = Vec3::ZERO;
        if self.ang_vel.length_squared() > 1e-12 {
            let wq = Quat::from_xyzw(self.ang_vel.x, self.ang_vel.y, self.ang_vel.z, 0.0);
            let dq = wq * self.orientation;
            self.orientation = Quat::from_xyzw(
                self.orientation.x + 0.5 * dq.x * dt,
                self.orientation.y + 0.5 * dq.y * dt,
                self.orientation.z + 0.5 * dq.z * dt,
                self.orientation.w + 0.5 * dq.w * dt,
            )
            .normalize();
        }
    }

    /// Bounce off a horizontal floor at `floor_y`, converting some of the
    /// horizontal velocity into spin (rolling) via friction at the contact.
    pub fn bounce_floor(&mut self, floor_y: f32, restitution: f32, friction: f32) {
        let bottom = self.pos.y - self.half_extents.y;
        if bottom < floor_y && self.vel.y < 0.0 {
            self.pos.y = floor_y + self.half_extents.y;
            self.vel.y = -self.vel.y * restitution;
            // tangential velocity at the contact induces spin (rolling without slipping-ish)
            let r = self.half_extents.max_element();
            // rolling about Z from x-motion, about X from z-motion (right-hand rule)
            self.ang_vel.z += -self.vel.x / r.max(1e-3) * friction;
            self.ang_vel.x += self.vel.z / r.max(1e-3) * friction;
            self.vel.x *= 1.0 - friction * 0.3;
            self.vel.z *= 1.0 - friction * 0.3;
        }
    }

    pub fn aabb_min(&self) -> Vec3 {
        self.pos - self.half_extents
    }

    pub fn aabb_max(&self) -> Vec3 {
        self.pos + self.half_extents
    }

    pub fn overlaps(&self, other: &RigidBody) -> bool {
        let a_min = self.aabb_min();
        let a_max = self.aabb_max();
        let b_min = other.aabb_min();
        let b_max = other.aabb_max();
        a_min.x <= b_max.x
            && a_max.x >= b_min.x
            && a_min.y <= b_max.y
            && a_max.y >= b_min.y
            && a_min.z <= b_max.z
            && a_max.z >= b_min.z
    }
}

/// Resolve an AABB collision between two bodies (impulse method).
pub fn resolve_collision(a: &mut RigidBody, b: &mut RigidBody) {
    if !a.overlaps(b) {
        return;
    }

    let a_min = a.aabb_min();
    let a_max = a.aabb_max();
    let b_min = b.aabb_min();
    let b_max = b.aabb_max();

    // Find smallest overlap axis.
    let overlaps = [
        (b_max.x - a_min.x, Vec3::X),
        (a_max.x - b_min.x, Vec3::NEG_X),
        (b_max.y - a_min.y, Vec3::Y),
        (a_max.y - b_min.y, Vec3::NEG_Y),
        (b_max.z - a_min.z, Vec3::Z),
        (a_max.z - b_min.z, Vec3::NEG_Z),
    ];

    let (depth, normal) = overlaps
        .iter()
        .min_by(|x, y| x.0.partial_cmp(&y.0).unwrap())
        .cloned()
        .unwrap();

    let restitution = (a.restitution + b.restitution) * 0.5;
    let rel_vel = (a.vel - b.vel).dot(normal);

    if rel_vel > 0.0 {
        return;
    } // already separating

    let inv_a = if a.is_static { 0.0 } else { 1.0 / a.mass };
    let inv_b = if b.is_static { 0.0 } else { 1.0 / b.mass };
    let j = -(1.0 + restitution) * rel_vel / (inv_a + inv_b).max(1e-6);

    let impulse = normal * j;
    a.apply_impulse(impulse);
    b.apply_impulse(-impulse);

    // Positional correction (Baumgarte).
    let correction = normal * (depth * 0.8 / (inv_a + inv_b).max(1e-6));
    if !a.is_static {
        a.pos += correction * inv_a;
    }
    if !b.is_static {
        b.pos -= correction * inv_b;
    }
}

/// Simple physics world holding all rigid bodies.
pub struct PhysicsWorld {
    pub bodies: Vec<RigidBody>,
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
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn torque_induces_spin_and_rotation() {
        let mut b = RigidBody::new(Vec3::ZERO, 1.0);
        b.gravity_scale = 0.0;
        b.apply_torque(Vec3::new(0.0, 1.0, 0.0));
        b.integrate(1.0 / 60.0, Vec3::ZERO);
        assert!(b.ang_vel.y > 0.0, "torque should create angular velocity");
        let before = b.orientation;
        for _ in 0..60 {
            b.integrate(1.0 / 60.0, Vec3::ZERO);
        }
        assert!(
            (b.orientation.dot(before)).abs() < 0.999,
            "body should have rotated"
        );
        assert!(b.orientation.is_normalized());
    }
    #[test]
    fn off_center_impulse_spins_body() {
        let mut b = RigidBody::new(Vec3::ZERO, 1.0);
        b.apply_impulse_at(Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.5, 0.0));
        assert!(b.vel.x > 0.0);
        assert!(b.ang_vel.length() > 0.0, "off-centre hit should spin it");
    }
    #[test]
    fn floor_bounce_creates_roll() {
        let mut b = RigidBody::new(Vec3::new(0.0, 0.4, 0.0), 1.0);
        b.vel = Vec3::new(3.0, -2.0, 0.0);
        b.bounce_floor(0.0, 0.6, 0.8);
        assert!(b.vel.y > 0.0, "should bounce up");
        assert!(
            b.ang_vel.z.abs() > 0.0,
            "horizontal motion should induce roll"
        );
    }
}
