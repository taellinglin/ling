// CUDA backend (feature = "cuda"). Kernels are compiled at runtime with NVRTC,
// so there is no build-time dependency on `nvcc` — only the CUDA driver at run
// time. `CudaBackend::new()` returns `None` when no device is usable, and the
// caller falls back to the CPU backend.
//
// Multi-GPU: `MultiCudaBackend::new()` builds one `CudaBackend` per visible
// device (e.g. both RTX 4090s) and, for large workloads, splits the output
// range across them — each device runs on its own thread and CUDA context, so
// the cards compute concurrently ("full blast"). Results are gathered host-side;
// there is no inter-GPU communication, so it scales near-linearly. Any device
// whose launch fails transparently falls back to the CPU for *its* slice only,
// so a partial failure never corrupts the result.
//
// NOTE: `cudarc` selects the CUDA API version via a Cargo feature. If enabling
// `--features cuda` fails to build, match cudarc's CUDA-version feature to your
// installed toolkit (this machine has CUDA 12.8).

use crate::{Backend, CameraParams};
use cudarc::driver::{CudaDevice, DriverError, LaunchAsync, LaunchConfig};
use cudarc::nvrtc::compile_ptx;
use std::sync::Arc;

const KERNELS: &str = r#"
// N-body acceleration for output rows [row0, row0+count) against all n bodies.
// `out` is the device-local slice (length 3*count), indexed from 0.
extern "C" __global__
void nbody_accel_range(const float* pos, const float* mass, float* out,
                       int n, int row0, int count, float g, float soften) {
    int t = blockIdx.x * blockDim.x + threadIdx.x;
    if (t >= count) return;
    int i = row0 + t;
    float xi = pos[3*i], yi = pos[3*i+1], zi = pos[3*i+2];
    float ax = 0.f, ay = 0.f, az = 0.f;
    float eps2 = soften * soften;
    for (int j = 0; j < n; ++j) {
        if (j == i) continue;
        float dx = pos[3*j]   - xi;
        float dy = pos[3*j+1] - yi;
        float dz = pos[3*j+2] - zi;
        float r2 = dx*dx + dy*dy + dz*dz + eps2;
        float inv = rsqrtf(r2);
        float inv3 = inv * inv * inv;
        float s = g * mass[j] * inv3;
        ax += s*dx; ay += s*dy; az += s*dz;
    }
    out[3*t] = ax; out[3*t+1] = ay; out[3*t+2] = az;
}

// Perspective projection of `n` points. `world`/`out` are the device's own
// slice (already offset on the host), so indices are local.
extern "C" __global__
void project_points(const float* world, float* out, const float* cam, int n) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n) return;
    // cam = [cry, sry, crx, srx, cx, cy, focal, zdist, tx, ty, tz]
    float cry=cam[0], sry=cam[1], crx=cam[2], srx=cam[3];
    float cx=cam[4], cy=cam[5], focal=cam[6], zdist=cam[7];
    float tx=cam[8], ty=cam[9], tz=cam[10];
    float wx = world[3*i]   - tx;
    float wy = world[3*i+1] - ty;
    float wz = world[3*i+2] - tz;
    float rx  = wx*cry - wz*sry;
    float rz1 = wx*sry + wz*cry;
    float ry  = wy*crx - rz1*srx;
    float rz  = wy*srx + rz1*crx;
    float d = rz + zdist;
    out[3*i]   = cx + focal * rx / d;
    out[3*i+1] = cy + focal * ry / d;
    out[3*i+2] = rz;
}
"#;

/// Below these element counts the multi-GPU split costs more (extra host↔device
/// copies, kernel launches, thread sync) than it saves, so we run on a single
/// device. N-body is O(n²) so its break-even is far smaller than projection's.
const MULTI_GPU_MIN_NBODY: usize = 2_048;
const MULTI_GPU_MIN_PROJECT: usize = 200_000;

/// A single CUDA device with the Ling kernels loaded into its context.
pub struct CudaBackend {
    dev: Arc<CudaDevice>,
    name: String,
}

impl CudaBackend {
    /// Build a backend for the default device (ordinal 0).
    pub fn new() -> Option<Self> {
        Self::on_ordinal(0)
    }

    /// Build a backend bound to a specific device ordinal.
    pub fn on_ordinal(ordinal: usize) -> Option<Self> {
        let dev = CudaDevice::new(ordinal).ok()?;
        let ptx = compile_ptx(KERNELS).ok()?;
        dev.load_ptx(ptx, "ling_gpu", &["nbody_accel_range", "project_points"])
            .ok()?;
        let name = format!(
            "CUDA[{ordinal}]: {}",
            dev.name().unwrap_or_else(|_| "NVIDIA GPU".into())
        );
        Some(CudaBackend { dev, name })
    }

    fn device_name(&self) -> &str {
        // Strip the "CUDA[n]: " prefix for the aggregate multi-GPU label.
        self.name.split_once(": ").map(|(_, n)| n).unwrap_or(&self.name)
    }

    /// Accelerate output rows `[row0, row0+count)` into `out_local` (len
    /// `3*count`). Falls back to the CPU for this slice on any driver error.
    fn nbody_range(
        &self,
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
        let mut run = || -> Result<(), DriverError> {
            let pos_d = self.dev.htod_sync_copy(pos)?;
            let mass_d = self.dev.htod_sync_copy(mass)?;
            let mut out_d = self.dev.alloc_zeros::<f32>(3 * count)?;
            let f = self.dev.get_func("ling_gpu", "nbody_accel_range").unwrap();
            let cfg = LaunchConfig::for_num_elems(count as u32);
            unsafe {
                f.launch(
                    cfg,
                    (
                        &pos_d,
                        &mass_d,
                        &mut out_d,
                        n as i32,
                        row0 as i32,
                        count as i32,
                        g,
                        soften,
                    ),
                )?;
            }
            self.dev.dtoh_sync_copy_into(&out_d, &mut out_local[..3 * count])?;
            Ok(())
        };
        if run().is_err() {
            crate::cpu::nbody_range_cpu(pos, mass, g, soften, row0, count, out_local);
        }
    }

    /// Project the point slice `world_slice` (len `3*m`) into `out_slice` (len
    /// `3*m`). Falls back to the CPU for this slice on any driver error.
    fn project_slice(&self, world_slice: &[f32], cam: &CameraParams, out_slice: &mut [f32]) {
        let m = world_slice.len() / 3;
        if m == 0 {
            return;
        }
        let cam_v = [
            cam.cry, cam.sry, cam.crx, cam.srx, cam.cx, cam.cy, cam.focal, cam.zdist, cam.tx,
            cam.ty, cam.tz,
        ];
        let mut run = || -> Result<(), DriverError> {
            let world_d = self.dev.htod_sync_copy(world_slice)?;
            let cam_d = self.dev.htod_sync_copy(&cam_v)?;
            let mut out_d = self.dev.alloc_zeros::<f32>(3 * m)?;
            let f = self.dev.get_func("ling_gpu", "project_points").unwrap();
            let cfg = LaunchConfig::for_num_elems(m as u32);
            unsafe {
                f.launch(cfg, (&world_d, &mut out_d, &cam_d, m as i32))?;
            }
            self.dev.dtoh_sync_copy_into(&out_d, &mut out_slice[..3 * m])?;
            Ok(())
        };
        if run().is_err() {
            crate::cpu::CpuBackend::new().project_points(world_slice, cam, out_slice);
        }
    }
}

impl Backend for CudaBackend {
    fn name(&self) -> &str {
        &self.name
    }
    fn is_gpu(&self) -> bool {
        true
    }
    fn gpu_count(&self) -> usize {
        1
    }

    fn nbody_accel(&self, pos: &[f32], mass: &[f32], g: f32, soften: f32, out: &mut [f32]) {
        let n = mass.len();
        self.nbody_range(pos, mass, g, soften, 0, n, out);
    }

    fn project_points(&self, world: &[f32], cam: &CameraParams, out: &mut [f32]) {
        self.project_slice(world, cam, out);
    }
}

/// Two-or-more CUDA devices driven concurrently. For large batches the output
/// range is partitioned across the devices and each runs on its own thread, so
/// e.g. two RTX 4090s execute the same kernel on different data at the same time.
pub struct MultiCudaBackend {
    devs: Vec<CudaBackend>,
    name: String,
}

impl MultiCudaBackend {
    /// Enumerate every usable CUDA device and load the kernels on each. Returns
    /// `None` if no device initialises (caller then uses the CPU backend).
    pub fn new() -> Option<Self> {
        let count = CudaDevice::count().ok()?.max(0) as usize;
        let mut devs = Vec::new();
        for ord in 0..count {
            if let Some(b) = CudaBackend::on_ordinal(ord) {
                devs.push(b);
            }
        }
        if devs.is_empty() {
            return None;
        }
        let name = if devs.len() == 1 {
            devs[0].name.clone()
        } else {
            let list = devs
                .iter()
                .map(|d| d.device_name().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!("CUDA x{}: {}", devs.len(), list)
        };
        Some(MultiCudaBackend { devs, name })
    }
}

impl Backend for MultiCudaBackend {
    fn name(&self) -> &str {
        &self.name
    }
    fn is_gpu(&self) -> bool {
        true
    }
    fn gpu_count(&self) -> usize {
        self.devs.len()
    }

    fn nbody_accel(&self, pos: &[f32], mass: &[f32], g: f32, soften: f32, out: &mut [f32]) {
        let n = mass.len();
        let k = self.devs.len();
        // Single device or small batch: no split, run directly on device 0.
        if k <= 1 || n < MULTI_GPU_MIN_NBODY {
            self.devs[0].nbody_range(pos, mass, g, soften, 0, n, out);
            return;
        }
        let ranges = crate::split_ranges(n, k);
        // Carve `out` into disjoint per-device row sub-slices.
        let mut rest: &mut [f32] = out;
        let mut subs: Vec<&mut [f32]> = Vec::with_capacity(k);
        for &(lo, hi) in &ranges {
            let (a, b) = rest.split_at_mut(3 * (hi - lo));
            subs.push(a);
            rest = b;
        }
        let devs = &self.devs;
        std::thread::scope(|s| {
            for ((dev, &(lo, hi)), sub) in devs.iter().zip(ranges.iter()).zip(subs) {
                s.spawn(move || {
                    dev.nbody_range(pos, mass, g, soften, lo, hi - lo, sub);
                });
            }
        });
    }

    fn project_points(&self, world: &[f32], cam: &CameraParams, out: &mut [f32]) {
        let n = world.len() / 3;
        let k = self.devs.len();
        if k <= 1 || n < MULTI_GPU_MIN_PROJECT {
            self.devs[0].project_slice(world, cam, out);
            return;
        }
        let ranges = crate::split_ranges(n, k);
        let mut rest: &mut [f32] = out;
        let mut subs: Vec<&mut [f32]> = Vec::with_capacity(k);
        for &(lo, hi) in &ranges {
            let (a, b) = rest.split_at_mut(3 * (hi - lo));
            subs.push(a);
            rest = b;
        }
        let devs = &self.devs;
        std::thread::scope(|s| {
            for ((dev, &(lo, hi)), sub) in devs.iter().zip(ranges.iter()).zip(subs) {
                let world_slice = &world[3 * lo..3 * hi];
                s.spawn(move || {
                    dev.project_slice(world_slice, cam, sub);
                });
            }
        });
    }
}

