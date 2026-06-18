// ling-gpu — GPU compute for Ling with a transparent CPU fallback.
//
// The rest of the engine calls `ling_gpu::backend()` and gets the best available
// device: a CUDA backend when the `cuda` feature is enabled *and* an NVIDIA
// device is present, otherwise the always-available CPU backend. Callers never
// branch on the device — fallback is automatic, so a single code path works on
// every machine and target.
//
// Accelerated ops are deliberately the data-parallel hotspots shared by
// ling-physics and ling-graphics:
//   • `nbody_accel`    — O(n²) gravitational/charge acceleration (physics).
//   • `project_points` — batch perspective projection of vertices (graphics).

use std::sync::OnceLock;

mod cpu;
#[cfg(feature = "cuda")]
mod cuda;

/// Camera parameters for [`Backend::project_points`] — mirrors
/// `ling::gfx::camera::Camera3D` so a whole vertex buffer can be projected at once.
#[derive(Clone, Copy, Debug)]
pub struct CameraParams {
    pub cry: f32,
    pub sry: f32, // cos/sin yaw
    pub crx: f32,
    pub srx: f32, // cos/sin pitch
    pub cx: f32,
    pub cy: f32, // screen centre (px)
    pub focal: f32,
    pub zdist: f32,
    pub tx: f32,
    pub ty: f32,
    pub tz: f32, // camera world position
}

/// A compute device. Implementations must be cheap to share (`Send + Sync`).
pub trait Backend: Send + Sync {
    /// Human-readable device name, e.g. "CPU" or "CUDA: NVIDIA GeForce RTX 4090".
    fn name(&self) -> &str;

    /// Whether this backend runs on a GPU (vs. the CPU fallback).
    fn is_gpu(&self) -> bool {
        false
    }

    /// 3-D N-body acceleration. `pos` and `out` are interleaved xyz of length
    /// `3*n`; `mass` has length `n`. `out[i] = Σ_{j≠i} g·m_j·(p_j−p_i)/(|Δ|²+ε²)^{3/2}`.
    fn nbody_accel(&self, pos: &[f32], mass: &[f32], g: f32, soften: f32, out: &mut [f32]);

    /// Batch perspective projection. `world` is interleaved xyz (len `3*n`);
    /// `out` receives interleaved (screen_x, screen_y, camera_depth) (len `3*n`).
    fn project_points(&self, world: &[f32], cam: &CameraParams, out: &mut [f32]);
}

/// The process-wide best-available backend (initialised once).
pub fn backend() -> &'static dyn Backend {
    static B: OnceLock<Box<dyn Backend>> = OnceLock::new();
    let b = B.get_or_init(|| {
        #[cfg(feature = "cuda")]
        {
            if let Some(c) = cuda::CudaBackend::new() {
                return Box::new(c) as Box<dyn Backend>;
            }
        }
        Box::new(cpu::CpuBackend::new()) as Box<dyn Backend>
    });
    &**b
}

/// Convenience: the active device's name (for logging / `--version` banners).
pub fn device_name() -> &'static str {
    backend().name()
}

/// Whether a GPU backend is actually active.
pub fn gpu_active() -> bool {
    backend().is_gpu()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nbody_directions_and_symmetry() {
        let b = backend();
        // Two equal masses on the x-axis: each accelerates toward the other.
        let pos = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let mass = [1.0, 1.0];
        let mut out = [0.0f32; 6];
        b.nbody_accel(&pos, &mass, 1.0, 0.01, &mut out);
        assert!(out[0] > 0.0, "body0 should pull toward +x");
        assert!(out[3] < 0.0, "body1 should pull toward -x");
        assert!(
            (out[0] + out[3]).abs() < 1e-3,
            "equal-mass pair is symmetric"
        );
        assert!(
            out[1].abs() < 1e-6 && out[2].abs() < 1e-6,
            "no off-axis accel"
        );
    }

    #[test]
    fn project_matches_reference() {
        let b = backend();
        // Identity-ish camera: no rotation, focal 100, zdist 5, centre (0,0).
        let cam = CameraParams {
            cry: 1.0,
            sry: 0.0,
            crx: 1.0,
            srx: 0.0,
            cx: 0.0,
            cy: 0.0,
            focal: 100.0,
            zdist: 5.0,
            tx: 0.0,
            ty: 0.0,
            tz: 0.0,
        };
        let world = [1.0, 2.0, 0.0]; // depth rz=0 → d=5
        let mut out = [0.0f32; 3];
        b.project_points(&world, &cam, &mut out);
        assert!(
            (out[0] - 20.0).abs() < 1e-3,
            "sx = focal*x/d = 100*1/5 = 20"
        );
        assert!(
            (out[1] - 40.0).abs() < 1e-3,
            "sy = focal*y/d = 100*2/5 = 40"
        );
        assert!((out[2] - 0.0).abs() < 1e-6, "depth rz = 0");
    }
}
