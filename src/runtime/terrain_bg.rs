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
pub(crate) fn compute_grid(bx: f64, bz: f64, cell: f64, rad: f64) -> TerrainGridResult {
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
    #[cfg(not(target_arch = "wasm32"))]
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

// wasm has no OS threads, so "background" grid jobs compute synchronously: a
// one-off few-ms stall at a cell boundary instead of the frame loop spinning
// forever on a job that can never finish (which is what the gated-out native
// path produced -- the "joining world" freeze).
#[cfg(target_arch = "wasm32")]
thread_local! {
    static SYNC_JOBS: std::cell::RefCell<HashMap<String, TerrainGridResult>> =
        std::cell::RefCell::new(HashMap::new());
    static SYNC_SEQ: std::cell::Cell<u64> = std::cell::Cell::new(0);
}

#[cfg(target_arch = "wasm32")]
pub fn start_sync(bx: f64, bz: f64, cell: f64, rad: f64) -> String {
    let id = SYNC_SEQ.with(|c| {
        let n = c.get() + 1;
        c.set(n);
        format!("sync{n}")
    });
    let result = compute_grid(bx, bz, cell, rad);
    SYNC_JOBS.with(|j| j.borrow_mut().insert(id.clone(), result));
    id
}

#[cfg(target_arch = "wasm32")]
pub fn poll_sync(id: &str) -> Option<TerrainGridResult> {
    SYNC_JOBS.with(|j| j.borrow().get(id).cloned())
}

// ── draw_quad_grid: EXACT Rust port of 생성.灵's draw_ground quad loop ──────
// (terraced contour colours, drifting biome recolour, world-space wash,
// per-triangle grain, Wind-Waker water sheet). One builtin call replaces
// ~150k interpreted ops per frame, so the view radius no longer has to be
// traded against the frame rate. Rasterization itself stays in the depth
// queue's flush (rayon-parallel natively).
fn hash01(i: f64) -> f64 {
    let v = (i * 12.9898 + 7.13).sin() * 43758.5453;
    v - v.floor()
}

fn biome_theme(x: f64, z: f64, t: f64) -> i32 {
    let p = 1440.0;
    let u = wrap_period(x + p * 0.5, p);
    let v = wrap_period(z + p * 0.5, p);
    let a = (u * 0.0095 + v * 0.0061 + t * 0.012).sin();
    let b = (u * 0.0078 - v * 0.0110 + t * 0.009).sin();
    if a > 0.0 {
        if b > 0.0 { 0 } else { 2 }
    } else if b > 0.0 {
        1
    } else {
        3
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_quad_grid(
    gfx: &mut crate::gfx::GfxState,
    grid_bx: f64,
    grid_bz: f64,
    cell: f64,
    rad: f64,
    sea_y: f64,
    t: f64,
    corners: &[f64],
) {
    let steps = ((rad * 2.0 / cell).floor() as usize) + 1;
    let npts = steps + 1;
    let lg = |i: usize, j: usize| -> f64 { corners.get(i * npts + j).copied().unwrap_or(0.0) };
    // Per grid point: (depth, screen x, screen y). Lets whole quads that are behind
    // the lens or wholly off-screen be skipped before any lighting/clipping work.
    let near = -gfx.camera.zdist + 0.05;
    let (sw, sh) = (gfx.width as f32, gfx.height as f32);
    let can_cull = gfx.camera.hyp_r <= 0.0 && gfx.camera.srz == 0.0;
    let mut pts: Vec<(f32, f32, f32)> = Vec::with_capacity(npts * npts);
    for gi in 0..npts {
        for gj in 0..npts {
            let x = (grid_bx - rad + gi as f64 * cell) as f32;
            let z = (grid_bz - rad + gj as f64 * cell) as f32;
            let y = (sea_y - lg(gi, gj)) as f32;
            let d = gfx.camera.depth(x, y, z);
            let (sx, sy, _) = if d > near { gfx.camera.project(x, y, z) } else { (0.0, 0.0, 0.0) };
            pts.push((d, sx, sy));
        }
    }
    let quad_hidden = |gi: usize, gj: usize| -> bool {
        let p = [
            pts[gi * npts + gj],
            pts[(gi + 1) * npts + gj],
            pts[gi * npts + gj + 1],
            pts[(gi + 1) * npts + gj + 1],
        ];
        if p.iter().all(|q| q.0 <= near) {
            return true;
        }
        if !can_cull || p.iter().any(|q| q.0 <= near) {
            return false;
        }
        const M: f32 = 48.0;
        p.iter().all(|q| q.1 < -M) || p.iter().all(|q| q.1 > sw + M)
            || p.iter().all(|q| q.2 < -M) || p.iter().all(|q| q.2 > sh + M)
    };
    let set_col = |gfx: &mut crate::gfx::GfxState, r: f64, g: f64, b: f64| {
        let c = |v: f64| -> u32 { v.clamp(0.0, 255.0) as u32 };
        gfx.color = (c(r) << 16) | (c(g) << 8) | c(b);
    };
    for gi in 0..steps {
        for gj in 0..steps {
            if quad_hidden(gi, gj) {
                continue;
            }
            let x0 = grid_bx - rad + gi as f64 * cell;
            let z0 = grid_bz - rad + gj as f64 * cell;
            let x1 = x0 + cell;
            let z1 = z0 + cell;
            let e00 = lg(gi, gj);
            let e10 = lg(gi + 1, gj);
            let e01 = lg(gi, gj + 1);
            let e11 = lg(gi + 1, gj + 1);
            let ym = (e00 + e10 + e01 + e11) * 0.25;
            let midx = (x0 + x1) * 0.5;
            let midz = (z0 + z1) * 0.5;
            let y00 = sea_y - e00;
            let y10 = sea_y - e10;
            let y01 = sea_y - e01;
            let y11 = sea_y - e11;

            let (mut cr, mut cg, mut cb) = if ym < 0.0 {
                (48.0, 108.0, 100.0)
            } else if ym < 0.08 {
                (224.0, 202.0, 130.0)
            } else if ym < 0.42 {
                (88.0, 178.0, 82.0)
            } else if ym < 0.86 {
                (118.0, 150.0, 96.0)
            } else {
                (174.0, 180.0, 176.0)
            };

            if ym > 0.10 {
                let bio = biome_theme(midx, midz, t);
                let lit = (0.82 + ym * 0.24).min(1.12);
                match bio {
                    0 => { cr = 82.0 * lit; cg = 166.0 * lit; cb = 76.0 * lit; },
                    1 => { cr = 86.0 * lit; cg = 166.0 * lit; cb = 122.0 * lit; },
                    2 => { cr = 226.0 * lit; cg = 196.0 * lit; cb = 116.0 * lit; },
                    _ => {
                        cr = 122.0 * lit; cg = 132.0 * lit; cb = 132.0 * lit;
                        if ym > 0.95 { cr = 224.0; cg = 228.0; cb = 228.0; }
                    },
                }
            }

            let wash = (0.94 + (midx * 0.055 + midz * 0.032).sin() * 0.07
                + (midx * 0.017 - midz * 0.071).sin() * 0.05)
                .clamp(0.84, 1.06);
            cr *= wash;
            cg *= wash;
            cb *= wash;

            let grain_amt = if ym < 0.0 { 6.0 } else if ym > 0.95 { 5.0 } else { 18.0 };
            let gif = gi as f64;
            let gjf = gj as f64;
            let gr1 = (hash01(gif * 7.13 + gjf * 3.71) - 0.5) * grain_amt;
            let gr2 = (hash01(gif * 2.91 + gjf * 8.47 + 91.0) - 0.5) * grain_amt;

            set_col(gfx, cr + gr1, cg + gr1, cb + gr1);
            gfx.submit_triangle(
                x0 as f32, y00 as f32, z0 as f32,
                x1 as f32, y10 as f32, z0 as f32,
                x1 as f32, y11 as f32, z1 as f32,
            );
            set_col(gfx, cr + gr2, cg + gr2, cb + gr2);
            gfx.submit_triangle(
                x0 as f32, y00 as f32, z0 as f32,
                x1 as f32, y11 as f32, z1 as f32,
                x0 as f32, y01 as f32, z1 as f32,
            );

            if ym < 0.06 {
                let wave = |x: f64, z: f64| -> f64 {
                    ((x + t * 1.6) * 0.5 + z * 0.3).sin() * 0.05
                        + (x * 0.22 - (z + t * 1.1) * 0.3).sin() * 0.03
                };
                let w00 = wave(x0, z0);
                let w10 = wave(x1, z0);
                let w01 = wave(x0, z1);
                let w11 = wave(x1, z1);
                let wm = (w00 + w10 + w01 + w11) * 0.25;
                let (sr, sg, sb) = if wm > 0.05 {
                    (210.0, 242.0, 250.0)
                } else if wm > 0.018 {
                    (122.0, 206.0, 234.0)
                } else {
                    (58.0, 168.0, 214.0)
                };
                set_col(gfx, sr, sg, sb);
                let sy = sea_y - 0.006;
                gfx.submit_triangle(
                    x0 as f32, (sy + w00) as f32, z0 as f32,
                    x1 as f32, (sy + w10) as f32, z0 as f32,
                    x1 as f32, (sy + w11) as f32, z1 as f32,
                );
                gfx.submit_triangle(
                    x0 as f32, (sy + w00) as f32, z0 as f32,
                    x1 as f32, (sy + w11) as f32, z1 as f32,
                    x0 as f32, (sy + w01) as f32, z1 as f32,
                );
            }
        }
    }
}
