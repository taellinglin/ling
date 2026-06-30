// CPU backend — the always-available fallback. Pure Rust + rayon.
//
// Both hotspots are embarrassingly parallel over their *output* rows, so we map
// each output triple to a rayon task. On a many-core box this keeps the no-GPU
// path (and small workloads that aren't worth a PCIe round-trip) saturating
// every core instead of running on a single thread.

use crate::{Backend, CameraParams};
use rayon::prelude::*;

pub struct CpuBackend {
    name: String,
}

impl CpuBackend {
    pub fn new() -> Self {
        CpuBackend { name: "CPU".to_string() }
    }
}

impl Default for CpuBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for CpuBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn nbody_accel(&self, pos: &[f32], mass: &[f32], g: f32, soften: f32, out: &mut [f32]) {
        let n = mass.len();
        debug_assert_eq!(pos.len(), 3 * n);
        debug_assert_eq!(out.len(), 3 * n);
        nbody_range_cpu(pos, mass, g, soften, 0, n, out);
    }

    fn project_points(&self, world: &[f32], cam: &CameraParams, out: &mut [f32]) {
        debug_assert_eq!(out.len(), world.len());
        // Each point projects independently; parallelise across points.
        out.par_chunks_mut(3).enumerate().for_each(|(i, dst)| {
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
            dst[0] = cam.cx + cam.focal * rx / d;
            dst[1] = cam.cy + cam.focal * ry / d;
            dst[2] = rz;
        });
    }
}

/// Compute N-body acceleration for the output rows `[row0, row0+count)` against
/// the full body set, writing `count` triples into `out_local` (length
/// `3*count`, indexed locally from 0). This is the CPU twin of the CUDA
/// `nbody_accel_range` kernel: the multi-GPU path uses it as a per-device
/// fallback so each device's slice is always filled even if its launch fails.
pub(crate) fn nbody_range_cpu(
    pos: &[f32],
    mass: &[f32],
    g: f32,
    soften: f32,
    row0: usize,
    count: usize,
    out_local: &mut [f32],
) {
    if count == 0 {
        return;
    }
    let n = mass.len();
    debug_assert_eq!(pos.len(), 3 * n);
    debug_assert!(out_local.len() >= 3 * count);
    debug_assert!(row0 + count <= n);
    let eps2 = soften * soften;
    out_local[..3 * count]
        .par_chunks_mut(3)
        .enumerate()
        .for_each(|(t, acc)| {
            let i = row0 + t;
            let (xi, yi, zi) = (pos[3 * i], pos[3 * i + 1], pos[3 * i + 2]);
            let (mut ax, mut ay, mut az) = (0.0f32, 0.0, 0.0);
            for j in 0..n {
                if j == i {
                    continue;
                }
                let dx = pos[3 * j] - xi;
                let dy = pos[3 * j + 1] - yi;
                let dz = pos[3 * j + 2] - zi;
                let r2 = dx * dx + dy * dy + dz * dz + eps2;
                let inv = r2.sqrt().recip();
                let inv3 = inv * inv * inv;
                let s = g * mass[j] * inv3;
                ax += s * dx;
                ay += s * dy;
                az += s * dz;
            }
            acc[0] = ax;
            acc[1] = ay;
            acc[2] = az;
        });
}
