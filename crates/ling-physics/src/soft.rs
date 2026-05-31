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

        // Structural springs along rings and sectors.
        for r in 0..(rings - 1) {
            for s in 0..sectors {
                let cur  = offset + (r * sectors + s) as usize;
                let next = offset + (r * sectors + (s + 1) % sectors) as usize;
                let down = offset + ((r + 1) * sectors + s) as usize;
                let rest_h = (nodes[cur].pos - nodes[next].pos).length();
                let rest_v = (nodes[cur].pos - nodes[down].pos).length();
                if rest_h > 0.0 {
                    springs.push(Spring { a: cur, b: next, rest_length: rest_h, stiffness: 0.9 });
                }
                if rest_v > 0.0 && r + 1 < rings - 1 {
                    springs.push(Spring { a: cur, b: down, rest_length: rest_v, stiffness: 0.9 });
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

        Self { nodes, springs, damping: 0.98 }
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
}
