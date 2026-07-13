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

    /// Number of physical GPUs this backend drives. `0` for the CPU fallback,
    /// `1` for a single CUDA device, `N` for the multi-GPU backend.
    fn gpu_count(&self) -> usize {
        0
    }

    /// 3-D N-body acceleration. `pos` and `out` are interleaved xyz of length
    /// `3*n`; `mass` has length `n`. `out[i] = Σ_{j≠i} g·m_j·(p_j−p_i)/(|Δ|²+ε²)^{3/2}`.
    fn nbody_accel(&self, pos: &[f32], mass: &[f32], g: f32, soften: f32, out: &mut [f32]);

    /// Batch perspective projection. `world` is interleaved xyz (len `3*n`);
    /// `out` receives interleaved (screen_x, screen_y, camera_depth) (len `3*n`).
    fn project_points(&self, world: &[f32], cam: &CameraParams, out: &mut [f32]);
}

/// Partition `n` output rows into `k` contiguous, near-equal `[lo, hi)` ranges
/// (one per device). The remainder is spread over the first `n % k` ranges so
/// the largest and smallest differ by at most one row — keeping two GPUs of
/// equal speed finishing at the same time. With `k == 0` (or `1`) the whole
/// range is returned as a single chunk; when `n < k` the trailing ranges are
/// empty (`lo == hi`), which the kernels treat as a no-op.
#[cfg(any(feature = "cuda", test))]
pub(crate) fn split_ranges(n: usize, k: usize) -> Vec<(usize, usize)> {
    let k = k.max(1);
    let base = n / k;
    let rem = n % k;
    let mut ranges = Vec::with_capacity(k);
    let mut lo = 0;
    for d in 0..k {
        let extra = usize::from(d < rem);
        let hi = (lo + base + extra).min(n);
        ranges.push((lo, hi));
        lo = hi;
    }
    ranges
}

/// The process-wide best-available backend (initialised once).
pub fn backend() -> &'static dyn Backend {
    static B: OnceLock<Box<dyn Backend>> = OnceLock::new();
    let b = B.get_or_init(|| {
        #[cfg(feature = "cuda")]
        {
            // Prefer every visible CUDA device (e.g. dual RTX 4090); the
            // multi-GPU backend collapses to a single device when only one is
            // present, and to the CPU when none initialise.
            if let Some(c) = cuda::MultiCudaBackend::new() {
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

/// How many physical GPUs the active backend is driving (0 on the CPU path).
/// Use this to size work batches or report "2× RTX 4090" in a startup banner.
pub fn gpu_count() -> usize {
    backend().gpu_count()
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

    // ── Multi-GPU partition logic ────────────────────────────────────────────
    // `split_ranges` drives the work distribution across N devices, so its
    // correctness is what guarantees two RTX 4090s each get half the rows with
    // nothing dropped or double-counted. Tested on every machine (no GPU needed).

    #[test]
    fn split_ranges_covers_everything_once() {
        for &(n, k) in &[(0, 2), (1, 2), (7, 2), (8, 2), (1000, 2), (1000, 3), (5, 8)] {
            let ranges = split_ranges(n, k);
            assert_eq!(
                ranges.len(),
                k.max(1),
                "one range per device (n={n}, k={k})"
            );
            // Contiguous, non-overlapping, starting at 0 and ending at n.
            assert_eq!(ranges[0].0, 0);
            assert_eq!(ranges.last().unwrap().1, n);
            let mut covered = 0;
            for w in ranges.windows(2) {
                assert_eq!(w[0].1, w[1].0, "ranges must be contiguous (n={n}, k={k})");
            }
            for &(lo, hi) in &ranges {
                assert!(lo <= hi, "range must be ordered");
                covered += hi - lo;
            }
            assert_eq!(covered, n, "every row covered exactly once (n={n}, k={k})");
        }
    }

    #[test]
    fn split_ranges_is_balanced() {
        // With n=1000 over 3 devices the largest/smallest chunk differ by ≤1.
        let ranges = split_ranges(1000, 3);
        let sizes: Vec<usize> = ranges.iter().map(|&(lo, hi)| hi - lo).collect();
        let max = *sizes.iter().max().unwrap();
        let min = *sizes.iter().min().unwrap();
        assert!(max - min <= 1, "chunks should be near-equal, got {sizes:?}");
    }

    #[test]
    fn split_ranges_handles_zero_devices() {
        // Defensive: k==0 must not divide-by-zero; treat as a single chunk.
        let ranges = split_ranges(10, 0);
        assert_eq!(ranges, vec![(0, 10)]);
    }

    #[test]
    fn cpu_range_matches_full_compute() {
        // The per-device CPU fallback (`nbody_range_cpu`) must agree with a full
        // single-shot computation on the rows it owns — this is what keeps a
        // mixed GPU/CPU multi-device result seamless.
        let n = 64usize;
        let mut pos = Vec::with_capacity(3 * n);
        let mut mass = Vec::with_capacity(n);
        for i in 0..n {
            let f = i as f32;
            pos.push((f * 0.3).sin() * 4.0);
            pos.push((f * 0.2).cos() * 4.0);
            pos.push((f * 0.1).sin() * 4.0);
            mass.push(1.0 + (f * 0.05).fract());
        }
        let mut full = vec![0.0f32; 3 * n];
        cpu::CpuBackend::new().nbody_accel(&pos, &mass, 0.7, 0.05, &mut full);

        // Compute rows [20, 50) into a local buffer and compare.
        let (row0, count) = (20usize, 30usize);
        let mut part = vec![0.0f32; 3 * count];
        cpu::nbody_range_cpu(&pos, &mass, 0.7, 0.05, row0, count, &mut part);
        for t in 0..count {
            for c in 0..3 {
                let a = part[3 * t + c];
                let b = full[3 * (row0 + t) + c];
                assert!(
                    (a - b).abs() < 1e-4,
                    "row {} chan {c}: {a} vs {b}",
                    row0 + t
                );
            }
        }
    }

    #[test]
    fn gpu_count_is_consistent_with_active() {
        // On CI/no-GPU this is the CPU path: not a GPU, zero devices.
        let b = backend();
        if b.is_gpu() {
            assert!(b.gpu_count() >= 1, "an active GPU backend drives ≥1 device");
        } else {
            assert_eq!(b.gpu_count(), 0, "CPU fallback drives no GPUs");
        }
        assert_eq!(gpu_active(), backend().is_gpu());
        assert_eq!(gpu_count(), backend().gpu_count());
    }
}
