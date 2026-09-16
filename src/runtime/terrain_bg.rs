//! Background terrain-grid generation — `terrain_grid_start`/`terrain_grid_poll`
//! builtins.
//!
//! Ports LingForFun's draw_ground/terrain_elev (originally hand-written in
//! .ling) to Rust so the expensive half of that computation — the
//! corner-elevation + per-corner-normal grid, ~(steps+1)^2 calls each
//! involving several trig-heavy noise lobes — can run on a plain OS thread
//! instead of blocking the single-threaded interpreter. Benchmarked at
//! ~110ms per grid under the tree-walking interpreter (several frame
//! budgets at 60fps), which showed up as a visible hitch every time the
//! player crossed a grid cell, even after caching made it rare instead of
//! constant.
//!
//! Uses a plain `std::thread::spawn`, not the tokio-backed `web::AsyncJobs`
//! used for HTTP jobs (`http_post_async`/`http_job_poll`): this is CPU-bound
//! compute, not I/O, and terrain generation should be available in every
//! desktop build, not just ones built with the optional `web` feature that
//! tokio/reqwest live behind.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

fn wrap_period(v: f64, period: f64) -> f64 {
    let k = (v / period).floor();
    v - k * period
}

fn torus_delta(a: f64, b: f64, period: f64) -> f64 {
    let d = (a - b).abs();
    let h = period * 0.5;
    if d > h {
        period - d
    } else {
        d
    }
}

fn island_lobe(u: f64, v: f64, cx: f64, cz: f64, sx: f64, sz: f64, amp: f64, period: f64) -> f64 {
    let dx = torus_delta(u, cx, period) / sx;
    let dz = torus_delta(v, cz, period) / sz;
    let r2 = dx * dx + dz * dz;
    let mut g = 1.0 / (1.0 + r2 * 1.25);
    g *= g;
    g *= g;
    amp * g
}

fn continent_lobe(u: f64, v: f64, cx: f64, cz: f64, sx: f64, sz: f64, amp: f64, period: f64) -> f64 {
    let dx = torus_delta(u, cx, period) / sx;
    let dz = torus_delta(v, cz, period) / sz;
    let r2 = dx * dx + dz * dz;
    amp * (1.0 / (1.0 + r2 * 0.7))
}

/// Exact port of 생성.灵's `terrain_elev` (see that file for the design
/// rationale of each term — three continents, sharp island/mountain lobes,
/// multi-octave coastline + hill noise, a clamped valley carve, biome
/// relief modulation). Must stay bit-for-bit in step with the .ling
/// version: draw_ground still computes a cell synchronously the first time
/// it's ever visited (before any background job could have completed), so
/// any drift between the two would show as a visible seam where the async
/// grid takes over.
pub fn terrain_elev(x: f64, z: f64) -> f64 {
    let p = 1440.0;
    let u = wrap_period(x + p * 0.5, p);
    let v = wrap_period(z + p * 0.5, p);

    let mut h = 0.0;
    h += continent_lobe(u, v, 720.0, 720.0, 210.0, 185.0, 1.50, p);
    h += continent_lobe(u, v, 300.0, 340.0, 180.0, 165.0, 1.36, p);
    h += continent_lobe(u, v, 1080.0, 1120.0, 205.0, 200.0, 1.46, p);
    h += island_lobe(u, v, 700.0, 760.0, 60.0, 52.0, 0.55, p);
    h += island_lobe(u, v, 1060.0, 1150.0, 66.0, 60.0, 0.60, p);
    h += island_lobe(u, v, 320.0, 300.0, 54.0, 58.0, 0.50, p);
    h += island_lobe(u, v, 500.0, 980.0, 62.0, 70.0, 0.50, p);
    h += island_lobe(u, v, 950.0, 520.0, 58.0, 54.0, 0.45, p);

    let nf = (u * 0.021 + v * 0.017).sin() * 0.16
        + (u * 0.041 - v * 0.033).sin() * 0.09
        + (u * 0.087 + v * 0.061).sin() * 0.045;

    let mut hills = (u * 0.055 + v * 0.041).sin() * 0.34;
    hills += (u * 0.031 - v * 0.049).sin() * 0.26;
    hills += (u * 0.108 + v * 0.089).sin() * 0.14;
    hills += (u * 0.190 - v * 0.160).sin() * 0.07;

    let mut valley =
        (u * 0.0072 + v * 0.0053).sin() * 0.85 + (u * 0.0140 - v * 0.0098).sin() * 0.42;
    if valley < -0.55 {
        valley = -0.55;
    }

    let mut landf = h - 0.5;
    if landf < 0.0 {
        landf = 0.0;
    }
    if landf > 1.0 {
        landf = 1.0;
    }

    let bmod = (u * 0.09 + v * 0.07).sin() * 0.40 + (u * 0.045 - v * 0.058).sin() * 0.25;

    h - 0.66 + nf + valley * 0.5 + (bmod + hills) * (landf * 0.7 + 0.3)
}

/// Plain-data result of one grid computation — `Send`, safe to move across
/// the thread boundary and store in the shared job table. Converted to real
/// `Value::List`s (which hold `Rc`, not `Send`) only in `poll`, back on the
/// calling (interpreter) thread.
#[derive(Clone)]
pub struct TerrainGridResult {
    pub bx: f64,
    pub bz: f64,
    pub corners: Vec<f64>,
    pub norm_nx: Vec<f64>,
    pub norm_ny: Vec<f64>,
    pub norm_nz: Vec<f64>,
}

/// Exact port of draw_ground's corner-elevation + per-corner-normal
/// computation — the expensive half of that function. The coloring/
/// draw_triangle_3d loop stays in .ling: only the pure-math half benefits
/// from running off-thread, since drawing has to happen on the main thread
/// against the real framebuffer/graphics state.
fn compute_grid(bx: f64, bz: f64, cell: f64, rad: f64) -> TerrainGridResult {
    let steps = (rad * 2.0 / cell).floor() + 1.0;
    let npts = steps + 1.0;
    let n = npts as usize;

    let mut corners = Vec::with_capacity(n * n);
    let mut ci = 0.0;
    while ci < npts {
        let ccx = bx - rad + ci * cell;
        let mut cj = 0.0;
        while cj < npts {
            corners.push(terrain_elev(ccx, bz - rad + cj * cell));
            cj += 1.0;
        }
        ci += 1.0;
    }

    let get = |idx: f64| -> f64 {
        let i = if idx < 0.0 { 0 } else { idx as usize };
        corners.get(i).copied().unwrap_or(0.0)
    };

    let mut norm_nx = Vec::with_capacity(n * n);
    let mut norm_ny = Vec::with_capacity(n * n);
    let mut norm_nz = Vec::with_capacity(n * n);
    let mut ni = 0.0;
    while ni < npts {
        let mut im1 = ni - 1.0;
        if im1 < 0.0 {
            im1 = 0.0;
        }
        let mut ip1 = ni + 1.0;
        if ip1 > steps {
            ip1 = steps;
        }
        let mut nj = 0.0;
        while nj < npts {
            let mut jm1 = nj - 1.0;
            if jm1 < 0.0 {
                jm1 = 0.0;
            }
            let mut jp1 = nj + 1.0;
            if jp1 > steps {
                jp1 = steps;
            }
            let e_l = get(im1 * npts + nj);
            let e_r = get(ip1 * npts + nj);
            let e_d = get(ni * npts + jm1);
            let e_u = get(ni * npts + jp1);
            let mut dx = (ip1 - im1) * cell;
            if dx < 0.001 {
                dx = cell;
            }
            let mut dz = (jp1 - jm1) * cell;
            if dz < 0.001 {
                dz = cell;
            }
            let de_dx = (e_r - e_l) / dx;
            let de_dz = (e_u - e_d) / dz;
            let nlen = (de_dx * de_dx + 1.0 + de_dz * de_dz).sqrt();
            norm_nx.push(de_dx / nlen);
            norm_ny.push(1.0 / nlen);
            norm_nz.push(de_dz / nlen);
            nj += 1.0;
        }
        ni += 1.0;
    }

    TerrainGridResult { bx, bz, corners, norm_nx, norm_ny, norm_nz }
}

type JobMap = Arc<Mutex<HashMap<String, Option<TerrainGridResult>>>>;

#[derive(Clone, Default)]
pub struct TerrainGridJobs(JobMap);

impl TerrainGridJobs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts computing the (bx,bz)-centered grid on a background OS
    /// thread; returns a job id immediately. Poll with `poll`.
    pub fn start(&self, bx: f64, bz: f64, cell: f64, rad: f64) -> String {
        let id = {
            use rand::RngCore;
            let mut buf = [0u8; 16];
            rand::rngs::OsRng.fill_bytes(&mut buf);
            buf.iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        self.0.lock().unwrap().insert(id.clone(), None);

        let jobs = self.0.clone();
        let job_id = id.clone();
        std::thread::spawn(move || {
            let result = compute_grid(bx, bz, cell, rad);
            jobs.lock().unwrap().insert(job_id, Some(result));
        });

        id
    }

    /// Non-blocking: `None` while still running (or for an unknown/already-
    /// consumed id), the result once it's ready. Does not remove the job
    /// from the table — callers poll once per frame and only act once,
    /// but a duplicate poll after completion still returns the same result.
    pub fn poll(&self, id: &str) -> Option<TerrainGridResult> {
        self.0.lock().unwrap().get(id).cloned().flatten()
    }
}
