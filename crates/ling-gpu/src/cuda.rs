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

use crate::{Backend, CameraParams, ViewWarp};
use cudarc::driver::{CudaDevice, CudaSlice, DeviceRepr, DeviceSlice, DriverError, LaunchAsync, LaunchConfig};
use std::sync::Mutex;
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

// Full camera projection incl. view roll and the hyperbolic warp (set_camera_hyper): the device twin of
// ling_gpu::project_warped / Camera3D::project. One thread per vertex; the camera rides in a by-value struct
// (kernel-param constant bank), so a frame costs no extra host->device copy for it.
struct CamPack {
    float cry, sry, crx, srx, cx, cy, focal, zdist, tx, ty, tz;
    float crz, srz, hyp_r, hyp_s, hx, hy, hz;
};
extern "C" __global__
void project_points_warped(const float* __restrict__ world, float* __restrict__ out, CamPack c, int n) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n) return;
    float wx = world[3*i], wy = world[3*i+1], wz = world[3*i+2];
    if (c.hyp_r > 0.0f) {
        float vx = wx - c.tx, vy = wy - c.ty, vz = wz - c.tz;
        float qx = vx*c.cry - vz*c.sry;
        float qz1 = vx*c.sry + vz*c.cry;
        float qy = vy*c.crx - qz1*c.srx;
        float qz = vy*c.srx + qz1*c.crx;
        float eye = qz + c.zdist;
        float k_front = fminf(fmaxf((eye - 1.0f) / 6.0f, 0.0f), 1.0f);
        float tan_h = fmaxf(c.cx / fmaxf(c.focal, 1.0f), 0.05f);
        float e = fmaxf(eye, 0.1f) * tan_h;
        float r2 = qx*qx + qy*qy;
        float lim = 1.8f * e;
        if (k_front > 0.0f && r2 < lim*lim) {
            float q = sqrtf(r2) / e;
            float k = k_front * fminf(fmaxf((1.8f - q) / 0.6f, 0.0f), 1.0f);
            if (k > 0.0f) {
                float dx = wx - c.hx, dz = wz - c.hz;
                float d = sqrtf(dx*dx + dz*dz);
                if (d > 1e-4f) { float s = c.hyp_r * tanhf(d / c.hyp_r) / d; dx *= s; dz *= s; }
                float u = dx*c.cry - dz*c.sry;
                float v = dx*c.sry + dz*c.cry;
                float raw = c.hyp_s * (v*v - u*u) / c.hyp_r;
                float f = fminf(fmaxf((wy - c.hy) / 12.0f, -1.0f), 1.0f);
                float wy2 = wy + 9.0f * tanhf(raw / 9.0f) * f;
                wx = wx + (c.hx + dx - wx) * k;
                wy = wy + (wy2 - wy) * k;
                wz = wz + (c.hz + dz - wz) * k;
            }
        }
    }
    wx -= c.tx; wy -= c.ty; wz -= c.tz;
    float rx  = wx*c.cry - wz*c.sry;
    float rz1 = wx*c.sry + wz*c.cry;
    float ry  = wy*c.crx - rz1*c.srx;
    float rz  = wy*c.srx + rz1*c.crx;
    float rrx = rx*c.crz - ry*c.srz;
    float rry = rx*c.srz + ry*c.crz;
    float dd = rz + c.zdist;
    out[3*i]   = c.cx + c.focal * rrx / dd;
    out[3*i+1] = c.cy + c.focal * rry / dd;
    out[3*i+2] = rz;
}

// Depth-buffered triangle raster. One thread per triangle walks its bounding box; for every covered pixel centre it
// interpolates 1/depth and keeps the nearest triangle with a 64-bit atomicMax on (depth bits << 32 | triangle + 1) -
// positive floats order like their bit patterns, so a larger key is nearer, and a later triangle wins a tie.
extern "C" __global__
void raster_tris(const float* tri, unsigned long long* vis, int ntri, int w, int h, float ox, float oy) {
    int t = blockIdx.x * blockDim.x + threadIdx.x;
    if (t >= ntri) return;
    const float* p = tri + 9 * t;
    float ax = p[0] - ox, ay = p[1] - oy, az = p[2];
    float bx = p[3] - ox, by = p[4] - oy, bz = p[5];
    float cx = p[6] - ox, cy = p[7] - oy, cz = p[8];
    float area = (bx - ax) * (cy - ay) - (cx - ax) * (by - ay);
    if (fabsf(area) < 1e-12f) return;
    float inv = 1.0f / area;
    int x0 = max(0, (int)floorf(fminf(ax, fminf(bx, cx)) - 0.5f));
    int y0 = max(0, (int)floorf(fminf(ay, fminf(by, cy)) - 0.5f));
    int x1 = min(w - 1, (int)ceilf(fmaxf(ax, fmaxf(bx, cx)) - 0.5f));
    int y1 = min(h - 1, (int)ceilf(fmaxf(ay, fmaxf(by, cy)) - 0.5f));
    for (int y = y0; y <= y1; ++y) {
        float py = y + 0.5f;
        for (int x = x0; x <= x1; ++x) {
            float px = x + 0.5f;
            float w0 = ((bx - px) * (cy - py) - (cx - px) * (by - py)) * inv;
            float w1 = ((cx - px) * (ay - py) - (ax - px) * (cy - py)) * inv;
            float w2 = 1.0f - w0 - w1;
            if (w0 < -1e-4f || w1 < -1e-4f || w2 < -1e-4f) continue;
            float z = w0 * az + w1 * bz + w2 * cz;
            if (!(z > 0.0f)) continue;
            unsigned long long key = ((unsigned long long)__float_as_uint(z) << 32) | (unsigned int)(t + 1);
            atomicMax(vis + (size_t)y * w + x, key);
        }
    }
}

// visibility buffer -> 0xFF000000 | rgb (0 where no triangle reached the pixel)
extern "C" __global__
void raster_resolve(const unsigned long long* vis, const unsigned int* col, unsigned int* out, int npix) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= npix) return;
    unsigned long long k = vis[i];
    out[i] = (k == 0ULL) ? 0u : (0xFF000000u | col[(unsigned int)(k & 0xFFFFFFFFULL) - 1u]);
}
"#;

/// Below these element counts the multi-GPU split costs more (extra host↔device
/// copies, kernel launches, thread sync) than it saves, so we run on a single
/// device. N-body is O(n²) so its break-even is far smaller than projection's.
const MULTI_GPU_MIN_NBODY: usize = 2_048;
const MULTI_GPU_MIN_PROJECT: usize = 200_000;
/// Below this many vertices a projection stays on the (rayon) CPU. Measured on
/// 2x RTX 4090 + 32 threads (`cuda::tests::breakdown`, `benches/gfx_pipeline`):
/// the warped kernel itself does 4M vertices in ~0.5 ms, but cudarc 0.12 only
/// copies from pageable memory (~3 GB/s), so upload + download of the 24 bytes
/// per vertex cost ~8 ms per million — while all CPU cores project a million
/// warped vertices in ~2 ms. With per-frame transfers the GPU never wins, so
/// the crossover sits above any real scene; it pays off only once vertices stay
/// resident on the device (see README). `LING_GPU_MIN_PROJECT` overrides it.
const CUDA_MIN_PROJECT: usize = 8_000_000;

/// The CPU/GPU crossover for projections; `LING_GPU_MIN_PROJECT` overrides it
/// (0 = always GPU) for tuning.
fn min_project() -> usize {
    static MIN: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *MIN.get_or_init(|| std::env::var("LING_GPU_MIN_PROJECT").ok().and_then(|v| v.parse().ok()).unwrap_or(CUDA_MIN_PROJECT))
}

/// Kernel-argument mirror of the device `CamPack` (18 floats, by value).
#[repr(C)]
#[derive(Clone, Copy)]
struct CamPack {
    cam: [f32; 11],
    warp: [f32; 7],
}
// SAFETY: plain-old-data, #[repr(C)], layout matches the CUDA struct exactly.
unsafe impl DeviceRepr for CamPack {}

impl CamPack {
    fn new(c: &CameraParams, w: &ViewWarp) -> Self {
        CamPack {
            cam: [c.cry, c.sry, c.crx, c.srx, c.cx, c.cy, c.focal, c.zdist, c.tx, c.ty, c.tz],
            warp: [w.crz, w.srz, w.hyp_r, w.hyp_s, w.hx, w.hy, w.hz],
        }
    }
}

/// Device buffers kept across frames: a projection used to allocate (and
/// free) its input and output on every call, which cost more than the kernel.
#[derive(Default)]
struct ProjBufs {
    world: Option<CudaSlice<f32>>,
    out: Option<CudaSlice<f32>>,
}

/// A single CUDA device with the Ling kernels loaded into its context.
pub struct CudaBackend {
    dev: Arc<CudaDevice>,
    name: String,
    bufs: Mutex<ProjBufs>,
}

impl CudaBackend {
    /// Build a backend bound to a specific device ordinal.
    pub fn on_ordinal(ordinal: usize) -> Option<Self> {
        let dev = CudaDevice::new(ordinal).ok()?;
        let ptx = compile_ptx(KERNELS).ok()?;
        dev.load_ptx(
            ptx,
            "ling_gpu",
            &["nbody_accel_range", "project_points", "project_points_warped", "raster_tris", "raster_resolve"],
        )
        .ok()?;
        let name = format!(
            "CUDA[{ordinal}]: {}",
            dev.name().unwrap_or_else(|_| "NVIDIA GPU".into())
        );
        Some(CudaBackend { dev, name, bufs: Mutex::new(ProjBufs::default()) })
    }

    fn device_name(&self) -> &str {
        // Strip the "CUDA[n]: " prefix for the aggregate multi-GPU label.
        self.name
            .split_once(": ")
            .map(|(_, n)| n)
            .unwrap_or(&self.name)
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
            self.dev
                .dtoh_sync_copy_into(&out_d, &mut out_local[..3 * count])?;
            Ok(())
        };
        if run().is_err() {
            crate::cpu::nbody_range_cpu(pos, mass, g, soften, row0, count, out_local);
        }
    }

    /// Project the point slice `world_slice` (len `3*m`) into `out_slice` (len
    /// `3*m`), with the view warp when `warp` is given. Reuses this device's
    /// cached buffers. Falls back to the CPU for this slice on any driver error.
    fn project_slice_ex(&self, world_slice: &[f32], cam: &CameraParams, warp: Option<&ViewWarp>, out_slice: &mut [f32]) {
        let m = world_slice.len() / 3;
        if m == 0 {
            return;
        }
        let len = 3 * m;
        let mut run = || -> Result<(), DriverError> {
            let mut guard = self.bufs.lock().unwrap_or_else(|e| e.into_inner());
            let bufs = &mut *guard;
            if bufs.world.as_ref().map_or(true, |b| b.len() < len) {
                // grow with headroom so a slowly growing scene doesn't realloc every frame
                let cap = (len + len / 2).max(3 * 4096);
                bufs.world = Some(unsafe { self.dev.alloc::<f32>(cap)? });
                bufs.out = Some(unsafe { self.dev.alloc::<f32>(cap)? });
            }
            let world_d = bufs.world.as_mut().unwrap();
            self.dev.htod_sync_copy_into(&world_slice[..len], &mut world_d.slice_mut(0..len))?;
            let world_d = bufs.world.as_ref().unwrap();
            let out_d = bufs.out.as_mut().unwrap();
            let cfg = LaunchConfig::for_num_elems(m as u32);
            match warp {
                Some(w) => {
                    let f = self.dev.get_func("ling_gpu", "project_points_warped").unwrap();
                    unsafe { f.launch(cfg, (world_d, &mut *out_d, CamPack::new(cam, w), m as i32))? };
                },
                None => {
                    let cam_v = [cam.cry, cam.sry, cam.crx, cam.srx, cam.cx, cam.cy, cam.focal, cam.zdist, cam.tx, cam.ty, cam.tz];
                    let cam_d = self.dev.htod_sync_copy(&cam_v)?;
                    let f = self.dev.get_func("ling_gpu", "project_points").unwrap();
                    unsafe { f.launch(cfg, (world_d, &mut *out_d, &cam_d, m as i32))? };
                },
            }
            self.dev.dtoh_sync_copy_into(&out_d.slice(0..len), &mut out_slice[..len])?;
            Ok(())
        };
        if run().is_err() {
            match warp {
                Some(w) => crate::cpu::project_warped_cpu(world_slice, cam, w, out_slice),
                None => crate::cpu::CpuBackend::new().project_points(world_slice, cam, out_slice),
            }
        }
    }

    fn project_slice(&self, world_slice: &[f32], cam: &CameraParams, out_slice: &mut [f32]) {
        // a typical mesh (a few thousand vertices) projects in microseconds on
        // the CPU; the PCIe round trip alone costs ~0.1 ms, so stay on the host
        if world_slice.len() / 3 < min_project() {
            crate::cpu::CpuBackend::new().project_points(world_slice, cam, out_slice);
        } else {
            self.project_slice_ex(world_slice, cam, None, out_slice)
        }
    }

    fn project_warped(&self, world: &[f32], cam: &CameraParams, warp: &ViewWarp, out: &mut [f32]) {
        if world.len() / 3 < min_project() {
            crate::cpu::project_warped_cpu(world, cam, warp, out);
        } else {
            self.project_slice_ex(world, cam, Some(warp), out);
        }
    }
}

impl CudaBackend {
    /// z-buffered raster of one scene on this device; CPU z-buffer on any driver error
    fn raster(&self, scene: &crate::RasterScene, out: &mut [u32]) {
        let npix = scene.width * scene.height;
        let nt = scene.colors.len().min(scene.tris.len() / 9);
        if npix == 0 || out.len() < npix {
            return;
        }
        if nt == 0 {
            out[..npix].fill(0);
            return;
        }
        let mut run = || -> Result<(), DriverError> {
            let tri_d = self.dev.htod_sync_copy(&scene.tris[..nt * 9])?;
            let col_d = self.dev.htod_sync_copy(&scene.colors[..nt])?;
            let mut vis_d = self.dev.alloc_zeros::<u64>(npix)?;
            let mut out_d = self.dev.alloc_zeros::<u32>(npix)?;
            let f = self.dev.get_func("ling_gpu", "raster_tris").unwrap();
            unsafe {
                f.launch(
                    LaunchConfig::for_num_elems(nt as u32),
                    (&tri_d, &mut vis_d, nt as i32, scene.width as i32, scene.height as i32, scene.x0, scene.y0),
                )?;
            }
            let g = self.dev.get_func("ling_gpu", "raster_resolve").unwrap();
            unsafe {
                g.launch(LaunchConfig::for_num_elems(npix as u32), (&vis_d, &col_d, &mut out_d, npix as i32))?;
            }
            self.dev.dtoh_sync_copy_into(&out_d, &mut out[..npix])?;
            Ok(())
        };
        if run().is_err() {
            crate::cpu::raster_cpu(scene, out);
        }
    }
}

impl Backend for CudaBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn raster_tris(&self, scene: &crate::RasterScene, out: &mut [u32]) {
        self.raster(scene, out);
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

    fn project_points_warped(&self, world: &[f32], cam: &CameraParams, warp: &ViewWarp, out: &mut [f32]) {
        self.project_warped(world, cam, warp, out);
    }
}

/// Two-or-more CUDA devices driven concurrently. For large batches the output
/// range is partitioned across the devices and each runs on its own thread, so
/// e.g. two RTX 4090s execute the same kernel on different data at the same time.
pub struct MultiCudaBackend {
    devs: Vec<CudaBackend>,
    name: String,
}

/// The shared-library names cudarc 0.12 tries for `lib` (its
/// `get_lib_name_candidates`, which is crate-private), for API 12.6 -- kept in
/// step with the `cuda-12060` feature in Cargo.toml.
fn lib_candidates(lib: &str) -> Vec<String> {
    use std::env::consts::{DLL_PREFIX as P, DLL_SUFFIX as S};
    let (major, minor) = ("12", "6");
    vec![
        format!("{P}{lib}{S}"),
        format!("{P}{lib}64{S}"),
        format!("{P}{lib}64_{major}{S}"),
        format!("{P}{lib}64_{major}{minor}{S}"),
        format!("{P}{lib}64_{major}{minor}_0{S}"),
        format!("{P}{lib}64_{major}0_{minor}{S}"),
        format!("{P}{lib}64_10{S}"),
        format!("{P}{lib}64_{major}0_0{S}"),
        format!("{P}{lib}64_9{S}"),
        format!("{P}{lib}{S}.{major}"),
        format!("{P}{lib}{S}.11"),
        format!("{P}{lib}{S}.10"),
    ]
}

fn loadable(names: &[String]) -> bool {
    // SAFETY: loading a vendor library runs its initialisers; these are the
    // same libraries cudarc would load next.
    names.iter().any(|n| unsafe { libloading::Library::new(n) }.is_ok())
}

/// Ok when both the driver (libcuda / nvcuda) and NVRTC can be loaded.
fn cuda_libraries_present() -> Result<(), &'static str> {
    let mut driver = lib_candidates("cuda");
    driver.extend(lib_candidates("nvcuda"));
    if !loadable(&driver) {
        return Err("no NVIDIA driver library (libcuda)");
    }
    if !loadable(&lib_candidates("nvrtc")) {
        return Err("NVIDIA driver found but no NVRTC (libnvrtc): install the CUDA toolkit to use the GPU");
    }
    Ok(())
}

impl MultiCudaBackend {
    /// Enumerate every usable CUDA device and load the kernels on each. Returns
    /// `None` if no device initialises (caller then uses the CPU backend).
    pub fn new() -> Option<Self> {
        // cudarc panics (and release builds abort) when libcuda or libnvrtc
        // cannot be loaded, so probe both first: a machine with no NVIDIA
        // driver, or with the driver but no CUDA toolkit (a common gaming
        // setup: libcuda yes, libnvrtc no), falls back to the CPU instead.
        if let Err(missing) = cuda_libraries_present() {
            eprintln!("ling-gpu: {missing}; using the CPU backend");
            return None;
        }
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

    // one frame is one z-buffer: it is rasterised on the first card (splitting it would need a depth merge)
    fn raster_tris(&self, scene: &crate::RasterScene, out: &mut [u32]) {
        self.devs[0].raster(scene, out);
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

    fn project_points_warped(&self, world: &[f32], cam: &CameraParams, warp: &ViewWarp, out: &mut [f32]) {
        let n = world.len() / 3;
        let k = self.devs.len();
        if k <= 1 || n < MULTI_GPU_MIN_PROJECT {
            self.devs[0].project_warped(world, cam, warp, out);
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
                s.spawn(move || dev.project_warped(world_slice, cam, warp, sub));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test -p ling-gpu --release --features cuda -- --ignored --nocapture breakdown`
    /// Where the time goes in one warped projection: upload / kernel / download.
    #[test]
    #[ignore]
    fn breakdown() {
        let Some(b) = CudaBackend::on_ordinal(0) else { return };
        let (cam, warp) = (
            CameraParams { cry: 0.8, sry: 0.6, crx: 0.95, srx: 0.31, cx: 960.0, cy: 540.0, focal: 1080.0, zdist: 5.0, tx: 0.0, ty: 0.0, tz: -6.0 },
            ViewWarp { crz: 1.0, srz: 0.0, hyp_r: 8.0, hyp_s: 0.6, hx: 0.0, hy: 0.0, hz: 0.0 },
        );
        for n in [50_000usize, 1_000_000, 4_000_000] {
            let world: Vec<f32> = (0..3 * n).map(|i| ((i as f32 * 0.618).fract() - 0.5) * 30.0).collect();
            let mut out = vec![0f32; 3 * n];
            let mut w_d = b.dev.htod_sync_copy(&world).unwrap();
            let mut o_d = b.dev.alloc_zeros::<f32>(3 * n).unwrap();
            let f = b.dev.get_func("ling_gpu", "project_points_warped").unwrap();
            let (mut up, mut k, mut down) = (0f64, 0f64, 0f64);
            let reps = 20;
            for _ in 0..reps {
                let t = std::time::Instant::now();
                b.dev.htod_sync_copy_into(&world, &mut w_d).unwrap();
                up += t.elapsed().as_secs_f64();
                let t = std::time::Instant::now();
                unsafe { f.clone().launch(LaunchConfig::for_num_elems(n as u32), (&w_d, &mut o_d, CamPack::new(&cam, &warp), n as i32)).unwrap() };
                b.dev.synchronize().unwrap();
                k += t.elapsed().as_secs_f64();
                let t = std::time::Instant::now();
                b.dev.dtoh_sync_copy_into(&o_d, &mut out).unwrap();
                down += t.elapsed().as_secs_f64();
            }
            let ms = |x: f64| x / reps as f64 * 1e3;
            let gbs = |x: f64| (12 * n) as f64 / (x / reps as f64) / 1e9;
            eprintln!(
                "n={n:>8}: upload {:.3} ms ({:.1} GB/s) | kernel {:.3} ms | download {:.3} ms ({:.1} GB/s)",
                ms(up), gbs(up), ms(k), ms(down), gbs(down)
            );
        }
    }
}
