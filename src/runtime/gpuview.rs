// src/runtime/gpuview.rs — batches a frame's triangles for the GPU z-buffer rasteriser (ling-gpu) used by the LingModeler viewport.
// mesh3d_fill(.., gpu = 1) appends shaded polygons here instead of painting them; gpu_raster_end rasterises the whole batch in one
// depth-buffered pass (CUDA when a device is present, a CPU z-buffer otherwise) and composites it over the framebuffer.
use std::sync::Mutex;

struct Scene {
    tris: Vec<f32>,
    colors: Vec<u32>,
}
static SCENE: Mutex<Scene> = Mutex::new(Scene { tris: Vec::new(), colors: Vec::new() });

pub fn begin() {
    if let Ok(mut s) = SCENE.lock() {
        s.tris.clear();
        s.colors.clear();
    }
}

/// fan-triangulate one convex-ish polygon: screen points + 1/depth per corner, one flat colour
pub fn push_poly(pts: &[[f32; 2]], zs: &[f32], color: u32) {
    if pts.len() < 3 || zs.len() < pts.len() {
        return;
    }
    if let Ok(mut s) = SCENE.lock() {
        for i in 1..pts.len() - 1 {
            for &k in &[0usize, i, i + 1] {
                s.tris.push(pts[k][0]);
                s.tris.push(pts[k][1]);
                s.tris.push(zs[k]);
            }
            s.colors.push(color);
        }
    }
}

/// rasterise the batch inside the rectangle (x, y, w, h) ∩ clip ∩ framebuffer and write the covered pixels; returns the triangle count
pub fn end(x: i32, y: i32, w: i32, h: i32, clip: Option<[i32; 4]>, buf: &mut [u32], bw: usize, bh: usize) -> usize {
    let (mut x0, mut y0, mut x1, mut y1) = (x.max(0), y.max(0), (x + w).min(bw as i32), (y + h).min(bh as i32));
    if let Some([cx0, cy0, cx1, cy1]) = clip {
        x0 = x0.max(cx0);
        y0 = y0.max(cy0);
        x1 = x1.min(cx1);
        y1 = y1.min(cy1);
    }
    let guard = match SCENE.lock() {
        Ok(g) => g,
        Err(_) => return 0,
    };
    let nt = guard.colors.len();
    if x1 <= x0 || y1 <= y0 || nt == 0 {
        return 0;
    }
    let (rw, rh) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mut out = vec![0u32; rw * rh];
    let scene = ling_gpu::RasterScene { x0: x0 as f32, y0: y0 as f32, width: rw, height: rh, tris: &guard.tris, colors: &guard.colors };
    ling_gpu::backend().raster_tris(&scene, &mut out);
    for ry in 0..rh {
        let row = &out[ry * rw..(ry + 1) * rw];
        let dst = &mut buf[(y0 as usize + ry) * bw + x0 as usize..(y0 as usize + ry) * bw + x0 as usize + rw];
        for (d, &s) in dst.iter_mut().zip(row.iter()) {
            if s != 0 {
                *d = s & 0x00FF_FFFF;
            }
        }
    }
    nt
}
