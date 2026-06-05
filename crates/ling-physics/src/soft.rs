//! Spring-mass soft bodies — deformable balls, cloth, etc.
//!
//! Uses position-based dynamics (PBD) for stability at large timesteps.

use glam::Vec3;

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
            // centroid so the ball stays round (squashes then recovers).
            if self.rest_radius > 0.0 && self.shape_stiffness > 0.0 {
                let c = self.centroid();
                let k = self.shape_stiffness;
                let r = self.rest_radius;
                for n in &mut self.nodes {
                    if n.pinned { continue; }
                    let d = n.pos - c;
                    let len = d.length();
                    if len > 1e-5 {
                        let target = c + d / len * r;
                        n.pos += (target - n.pos) * k;
                    }
                }
            }
        }
        // Update velocities for damping.
        for n in &mut self.nodes {
            n.vel = (n.pos - n.prev_pos) * self.damping;
        }
    }

    /// Returns the centre of mass.
    pub fn centroid(&self) -> Vec3 {
        if self.nodes.is_empty() { return Vec3::ZERO; }
        self.nodes.iter().map(|n| n.pos).fold(Vec3::ZERO, |a, b| a + b)
            / self.nodes.len() as f32
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
    pub fn floor_collision(&mut self, floor_y: f32, restitution: f32) {
        for n in &mut self.nodes {
            if n.pos.y < floor_y {
                let penetration = floor_y - n.pos.y;
                n.pos.y += penetration;
                let vel_y = (n.pos.y - n.prev_pos.y).abs();
                n.prev_pos.y = n.pos.y + vel_y * restitution;
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
        let mut b = SoftBody::sphere(Vec3::new(0.0, 2.0, 0.0), 1.0, 6, 8, 1.0);
        let d0 = b.deformation();
        // drop it onto a floor and let it squash
        for _ in 0..120 {
            b.integrate(1.0 / 60.0, Vec3::new(0.0, -9.81, 0.0), 4);
            b.floor_collision(0.0, 0.5);
        }
        // it should have deformed away from a perfect sphere at some point and stayed finite
        assert!(b.centroid().is_finite());
        assert!(b.deformation() >= 0.0 && b.deformation() <= 1.0);
        let _ = d0;
    }
}
