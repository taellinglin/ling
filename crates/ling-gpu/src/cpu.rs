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

/// Software z-buffer (the default `Backend::raster_tris`): the CPU twin of the CUDA `raster_tris` kernel. Depth is 1/d
/// interpolated linearly in screen space (exact for perspective); the nearest triangle wins, a later one wins a tie.
pub(crate) fn raster_cpu(scene: &crate::RasterScene, out: &mut [u32]) {
    let (w, h) = (scene.width, scene.height);
    if w == 0 || h == 0 || out.len() < w * h {
        return;
    }
    let mut depth = vec![0.0f32; w * h];
    for v in out[..w * h].iter_mut() {
        *v = 0;
    }
    let nt = scene.colors.len().min(scene.tris.len() / 9);
    for t in 0..nt {
        let p = &scene.tris[t * 9..t * 9 + 9];
        let (ax, ay, az) = (p[0] - scene.x0, p[1] - scene.y0, p[2]);
        let (bx, by, bz) = (p[3] - scene.x0, p[4] - scene.y0, p[5]);
        let (cx, cy, cz) = (p[6] - scene.x0, p[7] - scene.y0, p[8]);
        let area = (bx - ax) * (cy - ay) - (cx - ax) * (by - ay);
        if area.abs() < 1e-12 {
            continue;
        }
        let inv = 1.0 / area;
        let x0 = ((ax.min(bx).min(cx) - 0.5).floor().max(0.0)) as usize;
        let y0 = ((ay.min(by).min(cy) - 0.5).floor().max(0.0)) as usize;
        let x1 = (((ax.max(bx).max(cx) - 0.5).ceil()).min(w as f32 - 1.0)).max(-1.0) as i64;
        let y1 = (((ay.max(by).max(cy) - 0.5).ceil()).min(h as f32 - 1.0)).max(-1.0) as i64;
        for y in y0 as i64..=y1 {
            let py = y as f32 + 0.5;
            for x in x0 as i64..=x1 {
                let px = x as f32 + 0.5;
                let w0 = ((bx - px) * (cy - py) - (cx - px) * (by - py)) * inv;
                let w1 = ((cx - px) * (ay - py) - (ax - px) * (cy - py)) * inv;
                let w2 = 1.0 - w0 - w1;
                if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                    continue;
                }
                let z = w0 * az + w1 * bz + w2 * cz;
                if !(z > 0.0) {
                    continue;
                }
                let i = y as usize * w + x as usize;
                if z >= depth[i] {
                    depth[i] = z;
                    out[i] = 0xFF00_0000 | scene.colors[t];
                }
            }
        }
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
