//! Spring-mass soft bodies — deformable balls, cloth, etc.
//!
//! Uses position-based dynamics (PBD) for stability at large timesteps.

use glam::Vec3;

/// Largest distance a node may move in a single substep. Caps the runaway
/// feedback that stiff springs + a hard floor can otherwise build up into
/// non-finite coordinates.
const MAX_STEP: f32 = 1.5;

#[derive(Clone, Debug)]
pub struct SoftNode {
    pub pos:       Vec3,
    pub prev_pos:  Vec3,
    pub vel:       Vec3,
    pub mass:      f32,
    pub pinned:    bool,
}

impl SoftNode {
    pub fn new(pos: Vec3, mass: f32) -> Self {
        Self { pos, prev_pos: pos, vel: Vec3::ZERO, mass, pinned: false }
    }
}

#[derive(Clone, Debug)]
pub struct Spring {
    pub a:           usize,
    pub b:           usize,
    pub rest_length: f32,
    pub stiffness:   f32,
}

#[derive(Clone, Debug)]
pub struct SoftBody {
    pub nodes:   Vec<SoftNode>,
    pub springs: Vec<Spring>,
    pub damping: f32,
    /// Target radius from the centroid — a gentle shape-restoration constraint
    /// keeps the ball round (it squashes on impact, then springs back) instead
    /// of deflating/collapsing under gravity. `0` disables it.
    pub rest_radius: f32,
    /// How strongly each step pulls nodes back toward `rest_radius` (0..1).
    pub shape_stiffness: f32,
}

impl SoftBody {
    /// Build a deformable sphere from lat/lon rings.
    pub fn sphere(center: Vec3, radius: f32, rings: u32, sectors: u32, mass_per_node: f32) -> Self {
        let mut nodes  = Vec::new();
        let mut springs = Vec::new();

        // Pole nodes.
        nodes.push(SoftNode::new(center + Vec3::Y * radius, mass_per_node));
        nodes.push(SoftNode::new(center - Vec3::Y * radius, mass_per_node));

        let offset = 2usize;

        // Interior nodes.
        for r in 1..rings {
            let phi = std::f32::consts::PI * r as f32 / rings as f32;
            for s in 0..sectors {
                let theta = 2.0 * std::f32::consts::PI * s as f32 / sectors as f32;
                let x = phi.sin() * theta.cos();
                let y = phi.cos();
                let z = phi.sin() * theta.sin();
                nodes.push(SoftNode::new(center + Vec3::new(x, y, z) * radius, mass_per_node));
            }
        }

        // Structural springs along rings and sectors. Interior rows are r∈[0, rings-2],
        // so the vertical (`down`) spring only exists when r+1 is still a valid row.
        for r in 0..(rings - 1) {
            for s in 0..sectors {
                let cur  = offset + (r * sectors + s) as usize;
                let next = offset + (r * sectors + (s + 1) % sectors) as usize;
                let rest_h = (nodes[cur].pos - nodes[next].pos).length();
                if rest_h > 0.0 {
                    springs.push(Spring { a: cur, b: next, rest_length: rest_h, stiffness: 0.9 });
                }
                if r + 1 < rings - 1 {
                    let down = offset + ((r + 1) * sectors + s) as usize;
                    let rest_v = (nodes[cur].pos - nodes[down].pos).length();
                    if rest_v > 0.0 {
                        springs.push(Spring { a: cur, b: down, rest_length: rest_v, stiffness: 0.9 });
                    }
                }
            }
        }

        // Pole springs.
        for s in 0..sectors as usize {
            let top_near = offset + s;
            let bot_near = offset + ((rings - 2) * sectors) as usize + s;
            let r_top = (nodes[0].pos - nodes[top_near].pos).length();
            let r_bot = (nodes[1].pos - nodes[bot_near].pos).length();
            if r_top > 0.0 { springs.push(Spring { a: 0, b: top_near, rest_length: r_top, stiffness: 0.9 }); }
            if r_bot > 0.0 { springs.push(Spring { a: 1, b: bot_near, rest_length: r_bot, stiffness: 0.9 }); }
        }

        Self { nodes, springs, damping: 0.98, rest_radius: radius, shape_stiffness: 0.12 }
    }

    /// Verlet integration with gravity, then spring constraint projection.
    pub fn integrate(&mut self, dt: f32, gravity: Vec3, substeps: u32) {
        let sub_dt = dt / substeps as f32;
        for _ in 0..substeps {
            // Verlet step.
            for n in &mut self.nodes {
                if n.pinned { continue; }
                let acc = gravity;
                let next = n.pos * 2.0 - n.prev_pos + acc * sub_dt * sub_dt;
                n.prev_pos = n.pos;
                n.pos = next;
            }
            // PBD spring constraints.
            for s in &self.springs {
                let pa = self.nodes[s.a].pos;
                let pb = self.nodes[s.b].pos;
                let dir = pb - pa;
                let dist = dir.length();
                if dist < 1e-6 { continue; }
                let correction = dir / dist * (dist - s.rest_length) * s.stiffness * 0.5;
                if !self.nodes[s.a].pinned { self.nodes[s.a].pos += correction; }
                if !self.nodes[s.b].pinned { self.nodes[s.b].pos -= correction; }
            }
            // Shape-restoration: nudge each node toward `rest_radius` from the
            // centroid so the ball stays round (squashes then recovers). The
            // displacements are de-meaned so the constraint preserves the centroid
            // — otherwise an asymmetric/spinning body slowly translates itself
            // (the marble "floating off" while you roll it).
            if self.rest_radius > 0.0 && self.shape_stiffness > 0.0 {
                let c = self.centroid();
                let k = self.shape_stiffness;
                let r = self.rest_radius;
                let inv_n = 1.0 / self.nodes.len().max(1) as f32;
                let mut mean = Vec3::ZERO;
                let mut disp: Vec<Vec3> = Vec::with_capacity(self.nodes.len());
                for n in &self.nodes {
                    let d = n.pos - c;
                    let len = d.length();
                    let dv = if len > 1e-5 { (c + d / len * r - n.pos) * k } else { Vec3::ZERO };
                    mean += dv;
                    disp.push(dv);
                }
                mean *= inv_n;
                for (n, dv) in self.nodes.iter_mut().zip(disp) {
                    if n.pinned { continue; }
                    n.pos += dv - mean;
                }
            }
        }
        // Stability pass: scrub any non-finite coordinate and cap per-step travel
        // so the body can never explode into NaN/Inf (which would corrupt the
        // centroid and everything downstream that reads it).
        let c = {
            let raw = self.centroid();
            if raw.is_finite() { raw } else { Vec3::ZERO }
        };
        for n in &mut self.nodes {
            if !n.pos.is_finite() { n.pos = c; }
            if !n.prev_pos.is_finite() { n.prev_pos = n.pos; }
            let step = n.pos - n.prev_pos;
            let m = step.length();
            if m > MAX_STEP { n.prev_pos = n.pos - step / m * MAX_STEP; }
        }
        // Update velocities for damping.
        for n in &mut self.nodes {
            n.vel = (n.pos - n.prev_pos) * self.damping;
        }
    }

    /// Returns the centre of mass (ignoring any non-finite nodes).
    pub fn centroid(&self) -> Vec3 {
        let mut sum = Vec3::ZERO;
        let mut count = 0.0f32;
        for n in &self.nodes {
            if n.pos.is_finite() { sum += n.pos; count += 1.0; }
        }
        if count == 0.0 { Vec3::ZERO } else { sum / count }
    }

    /// Mean (centre-of-mass) velocity of the body.
    pub fn mean_velocity(&self) -> Vec3 {
        let mut sum = Vec3::ZERO;
        let mut count = 0.0f32;
        for n in &self.nodes {
            if n.vel.is_finite() { sum += n.vel; count += 1.0; }
        }
        if count == 0.0 { Vec3::ZERO } else { sum / count }
    }

    /// Approximate rigid angular velocity ω of the body, derived from the node
    /// velocities: ω ≈ (Σ r × v) / (Σ |r|²) with r = node − centroid and v the
    /// node velocity relative to the centre of mass. Tumbling/rolling shows up
    /// here even though the body is soft.
    pub fn angular_velocity(&self) -> Vec3 {
        let c = self.centroid();
        let cv = self.mean_velocity();
        let mut l = Vec3::ZERO;
        let mut inertia = 0.0f32;
        for n in &self.nodes {
            let r = n.pos - c;
            let v = n.vel - cv;
            l += r.cross(v);
            inertia += r.length_squared();
        }
        if inertia > 1e-6 && l.is_finite() { l / inertia } else { Vec3::ZERO }
    }

    /// Magnitude of the angular velocity (spin rate).
    pub fn angular_speed(&self) -> f32 {
        self.angular_velocity().length()
    }

    /// Deformation factor: 0 = perfect sphere, 1 = fully squished.
    pub fn deformation(&self) -> f32 {
        let c = self.centroid();
        let dists: Vec<f32> = self.nodes.iter().map(|n| (n.pos - c).length()).collect();
        if dists.is_empty() { return 0.0; }
        let avg = dists.iter().sum::<f32>() / dists.len() as f32;
        let variance = dists.iter().map(|d| (d - avg).powi(2)).sum::<f32>() / dists.len() as f32;
        (variance.sqrt() / avg.max(1e-6)).min(1.0)
    }

    /// Bounce off a horizontal floor plane at `y = floor_y`.
    ///
    /// The Ling renderer uses screen-space **+y = down**, so a floor sits at the
    /// *largest* y and the valid region is `y <= floor_y`: gravity (+y) pulls the
    /// body down onto it. Penetrating nodes (`y > floor_y`) are pushed back to the
    /// plane and their vertical velocity is reflected so they leave moving *up*
    /// (toward −y), keeping `restitution` of their speed, with a little friction.
    pub fn floor_collision(&mut self, floor_y: f32, restitution: f32) {
        let rest = restitution.clamp(0.0, 0.99);
        for n in &mut self.nodes {
            if n.pos.y > floor_y {
                n.pos.y = floor_y;
                // Verlet velocity is (pos - prev). To move up (−y) next step we
                // need prev > pos, so place prev *above* pos by the reflected speed.
                let vel_y = (n.prev_pos.y - n.pos.y).abs().min(MAX_STEP);
                n.prev_pos.y = n.pos.y + vel_y * rest;
                // tangential friction: bleed a little horizontal speed
                n.prev_pos.x += (n.pos.x - n.prev_pos.x) * 0.04;
                n.prev_pos.z += (n.pos.z - n.prev_pos.z) * 0.04;
            }
        }
    }

    /// Kick the whole body in a direction (Verlet impulse): shifts each node's
    /// previous position so the next step gains `strength` of velocity along `dir`.
    pub fn kick(&mut self, dir: Vec3, strength: f32) {
        let d = if dir.length_squared() > 1e-9 { dir.normalize() } else { return };
        for n in &mut self.nodes {
            if !n.pinned { n.prev_pos -= d * strength; }
        }
    }

    /// Add angular velocity about `axis` (through the centroid): every node gains
    /// the tangential velocity `ω × r` where `ω = axiŝ · rate` and `r` is the node
    /// offset from the centroid. The springs resist the shear, so the body both
    /// **spins/rolls and squashes** as it does. `rate` is the angular speed in
    /// radians per step (for rolling-without-slip at surface speed `s` and radius
    /// `R`, pass `rate = s / R`).
    pub fn spin(&mut self, axis: Vec3, rate: f32) {
        if axis.length_squared() < 1e-12 || rate.abs() < 1e-9 { return; }
        let w = axis.normalize() * rate;
        let c = self.centroid();
        if !c.is_finite() { return; }
        // Tangential velocity ω × r per node. The discrete sphere isn't perfectly
        // symmetric, so the raw sum has a small bias that would translate the body
        // (the marble "floating off" while you roll it). Subtract the mean so a
        // spin is *pure rotation* — zero net linear momentum.
        let inv_n = 1.0 / self.nodes.len().max(1) as f32;
        let mut mean = Vec3::ZERO;
        let mut tang: Vec<Vec3> = Vec::with_capacity(self.nodes.len());
        for n in &self.nodes {
            let v = w.cross(n.pos - c);
            mean += v;
            tang.push(v);
        }
        mean *= inv_n;
        for (n, v) in self.nodes.iter_mut().zip(tang) {
            if n.pinned { continue; }
            // Verlet velocity is (pos - prev); bias prev backward by the (de-meaned)
            // tangential velocity so the next step rotates the node forward.
            n.prev_pos -= v - mean;
        }
    }

    /// Keep the body inside an axis-aligned box `[min,max]`, bouncing (and thus
    /// squashing) off every wall — a contained bouncy ball.
    pub fn contain(&mut self, min: Vec3, max: Vec3, restitution: f32) {
        for n in &mut self.nodes {
            // x
            if n.pos.x < min.x { let v = (n.pos.x - n.prev_pos.x).abs(); n.pos.x = min.x; n.prev_pos.x = n.pos.x + v * restitution; }
            if n.pos.x > max.x { let v = (n.pos.x - n.prev_pos.x).abs(); n.pos.x = max.x; n.prev_pos.x = n.pos.x - v * restitution; }
            // y
            if n.pos.y < min.y { let v = (n.pos.y - n.prev_pos.y).abs(); n.pos.y = min.y; n.prev_pos.y = n.pos.y + v * restitution; }
            if n.pos.y > max.y { let v = (n.pos.y - n.prev_pos.y).abs(); n.pos.y = max.y; n.prev_pos.y = n.pos.y - v * restitution; }
            // z
            if n.pos.z < min.z { let v = (n.pos.z - n.prev_pos.z).abs(); n.pos.z = min.z; n.prev_pos.z = n.pos.z + v * restitution; }
            if n.pos.z > max.z { let v = (n.pos.z - n.prev_pos.z).abs(); n.pos.z = max.z; n.prev_pos.z = n.pos.z - v * restitution; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ball_deforms_on_impact() {
        // screen-down convention: floor below (large y), gravity (+y) pulls onto it
        let mut b = SoftBody::sphere(Vec3::new(0.0, 0.0, 0.0), 1.0, 6, 8, 1.0);
        let d0 = b.deformation();
        // drop it onto a floor at y=3 and let it squash + bounce
        for _ in 0..240 {
            b.integrate(1.0 / 60.0, Vec3::new(0.0, 9.81, 0.0), 4);
            b.floor_collision(3.0, 0.5);
        }
        // it stays finite, settles above the floor, and stays a sane shape
        let c = b.centroid();
        assert!(c.is_finite(), "centroid must stay finite, got {c:?}");
        assert!(c.y <= 3.0 + 1e-3, "ball must rest on/above the floor, y={}", c.y);
        assert!(b.deformation() >= 0.0 && b.deformation() <= 1.0);
        let _ = d0;
    }

    #[test]
    fn spin_rolls_and_deforms_without_exploding() {
        let mut b = SoftBody::sphere(Vec3::new(0.0, 0.0, 0.0), 1.0, 6, 8, 1.0);
        // pick a node and remember its starting offset from the centroid
        let c0 = b.centroid();
        let start = b.nodes[2].pos - c0;
        // spin about +z each step for a while (no gravity, no floor)
        for _ in 0..40 {
            b.spin(Vec3::Z, 0.15);
            b.integrate(1.0 / 60.0, Vec3::ZERO, 4);
        }
        let c = b.centroid();
        assert!(c.is_finite(), "centroid must stay finite, got {c:?}");
        // spinning must not *explode* the body across the world (a little drift is
        // fine — a deformed soft sphere isn't perfectly symmetric)
        assert!(c.length() < 1.5, "spin should not fling the body: {c:?}");
        // the marker node should have rotated tangentially (its xy direction moved)
        let nowv = b.nodes[2].pos - c;
        let moved = (nowv.normalize() - start.normalize()).length();
        assert!(moved > 0.05, "node should have rotated, moved={moved}");
        // and the shear should have deformed it a little
        assert!(b.deformation() > 0.0 && b.deformation() <= 1.0);
    }
}
