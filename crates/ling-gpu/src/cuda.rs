// CUDA backend (feature = "cuda"). Kernels are compiled at runtime with NVRTC,
// so there is no build-time dependency on `nvcc` — only the CUDA driver at run
// time. `CudaBackend::new()` returns `None` when no device is usable, and the
// caller falls back to the CPU backend.
//
// NOTE: `cudarc` selects the CUDA API version via a Cargo feature. If enabling
// `--features cuda` fails to build, match cudarc's CUDA-version feature to your
// installed toolkit (this machine has CUDA 12.8).

use crate::{Backend, CameraParams};
use cudarc::driver::{CudaDevice, LaunchAsync, LaunchConfig};
use cudarc::nvrtc::compile_ptx;
use std::sync::Arc;

const KERNELS: &str = r#"
extern "C" __global__
void nbody_accel(const float* pos, const float* mass, float* out,
                 int n, float g, float soften) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n) return;
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
    out[3*i] = ax; out[3*i+1] = ay; out[3*i+2] = az;
}

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

pub struct CudaBackend {
    dev: Arc<CudaDevice>,
    name: String,
}

impl CudaBackend {
    pub fn new() -> Option<Self> {
        let dev = CudaDevice::new(0).ok()?;
        let ptx = compile_ptx(KERNELS).ok()?;
        dev.load_ptx(ptx, "ling_gpu", &["nbody_accel", "project_points"]).ok()?;
        let name = format!("CUDA: {}", dev.name().unwrap_or_else(|_| "NVIDIA GPU".into()));
        Some(CudaBackend { dev, name })
    }
}

impl Backend for CudaBackend {
    fn name(&self) -> &str { &self.name }
    fn is_gpu(&self) -> bool { true }

    fn nbody_accel(&self, pos: &[f32], mass: &[f32], g: f32, soften: f32, out: &mut [f32]) {
        let n = mass.len();
        let mut run = || -> Result<(), cudarc::driver::DriverError> {
            let pos_d = self.dev.htod_sync_copy(pos)?;
            let mass_d = self.dev.htod_sync_copy(mass)?;
            let mut out_d = self.dev.alloc_zeros::<f32>(3 * n)?;
            let f = self.dev.get_func("ling_gpu", "nbody_accel").unwrap();
            let cfg = LaunchConfig::for_num_elems(n as u32);
            unsafe { f.launch(cfg, (&pos_d, &mass_d, &mut out_d, n as i32, g, soften))?; }
            self.dev.dtoh_sync_copy_into(&out_d, out)?;
            Ok(())
        };
        if run().is_err() {
            crate::cpu::CpuBackend::new().nbody_accel(pos, mass, g, soften, out);
        }
    }

    fn project_points(&self, world: &[f32], cam: &CameraParams, out: &mut [f32]) {
        let n = world.len() / 3;
        let cam_v = [
            cam.cry, cam.sry, cam.crx, cam.srx,
            cam.cx, cam.cy, cam.focal, cam.zdist,
            cam.tx, cam.ty, cam.tz,
        ];
        let mut run = || -> Result<(), cudarc::driver::DriverError> {
            let world_d = self.dev.htod_sync_copy(world)?;
            let cam_d = self.dev.htod_sync_copy(&cam_v)?;
            let mut out_d = self.dev.alloc_zeros::<f32>(3 * n)?;
            let f = self.dev.get_func("ling_gpu", "project_points").unwrap();
            let cfg = LaunchConfig::for_num_elems(n as u32);
            unsafe { f.launch(cfg, (&world_d, &mut out_d, &cam_d, n as i32))?; }
            self.dev.dtoh_sync_copy_into(&out_d, out)?;
            Ok(())
        };
        if run().is_err() {
            crate::cpu::CpuBackend::new().project_points(world, cam, out);
        }
    }
}
