// CPU backend — the always-available fallback. Pure Rust, no dependencies.

use crate::{Backend, CameraParams};

pub struct CpuBackend {
    name: String,
}

impl CpuBackend {
    pub fn new() -> Self {
        CpuBackend { name: "CPU".to_string() }
    }
}

impl Backend for CpuBackend {
    fn name(&self) -> &str { &self.name }

    fn nbody_accel(&self, pos: &[f32], mass: &[f32], g: f32, soften: f32, out: &mut [f32]) {
        let n = mass.len();
        debug_assert_eq!(pos.len(), 3 * n);
        debug_assert_eq!(out.len(), 3 * n);
        let eps2 = soften * soften;
        for i in 0..n {
            let (xi, yi, zi) = (pos[3 * i], pos[3 * i + 1], pos[3 * i + 2]);
            let (mut ax, mut ay, mut az) = (0.0f32, 0.0, 0.0);
            for j in 0..n {
                if j == i { continue; }
                let dx = pos[3 * j] - xi;
                let dy = pos[3 * j + 1] - yi;
                let dz = pos[3 * j + 2] - zi;
                let r2 = dx * dx + dy * dy + dz * dz + eps2;
                let inv = r2.sqrt().recip();
                let inv3 = inv * inv * inv;
                let s = g * mass[j] * inv3;
                ax += s * dx; ay += s * dy; az += s * dz;
            }
            out[3 * i] = ax; out[3 * i + 1] = ay; out[3 * i + 2] = az;
        }
    }

    fn project_points(&self, world: &[f32], cam: &CameraParams, out: &mut [f32]) {
        let n = world.len() / 3;
        debug_assert_eq!(out.len(), 3 * n);
        for i in 0..n {
            let wx = world[3 * i] - cam.tx;
            let wy = world[3 * i + 1] - cam.ty;
            let wz = world[3 * i + 2] - cam.tz;
            // Y rotation
            let rx = wx * cam.cry - wz * cam.sry;
            let rz1 = wx * cam.sry + wz * cam.cry;
            // X rotation
            let ry = wy * cam.crx - rz1 * cam.srx;
            let rz = wy * cam.srx + rz1 * cam.crx;
            // Perspective
            let d = rz + cam.zdist;
            out[3 * i] = cam.cx + cam.focal * rx / d;
            out[3 * i + 1] = cam.cy + cam.focal * ry / d;
            out[3 * i + 2] = rz;
        }
    }
}
