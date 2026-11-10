// Included into g3d.rs: the host gfx builtins bigger games (FLY for FUN) draw
// their world with, on the kernel's rasterizer. Semantics follow
// ling/src/gfx/mod.rs (`submit_triangle`), shapes.rs (`draw_color_mesh`),
// runtime/texmesh.rs (`txm_*`), runtime/terrain_bg.rs and the gltf_* arms of
// runtime/mod.rs.

use crate::drivers::gltf;
use crate::lingrt::mk_str;

/// `gltf_load` paths already reported as failing.
static mut GLTF_REPORTED: Vec<String> = Vec::new();

impl State {
    /// The window-space view (centre, focal) -- `project_3d` / `mouse_ray`
    /// answer in window pixels whatever the render scale.
    fn win_view(&mut self) -> (f32, f32, f32) {
        let (w, h) = self.win_dims().unwrap_or((1280, 720));
        // inside a display-size UI layer: the frame's camera, scaled to the layer's pixels
        if let Some((rw, rh, k)) = crate::lingapp::ui_layer_scene() {
            let (cx, cy) = self.cam.center.unwrap_or((rw as f32 * 0.5, rh as f32 * 0.5));
            let focal = if self.cam.focal > 0.0 { self.cam.focal } else { rh as f32 };
            return (cx * k, cy * k, focal * k);
        }
        let (cx, cy) = self.cam.center.unwrap_or((w as f32 * 0.5, h as f32 * 0.5));
        let focal = if self.cam.focal > 0.0 { self.cam.focal } else { h as f32 };
        (cx, cy, focal)
    }

    fn lens_ok(&self, d: [f32; 3]) -> bool {
        if self.lens_guard <= 0.0 {
            return true;
        }
        let g = -self.cam.zdist + 0.05 + self.lens_guard;
        d[0] >= g && d[1] >= g && d[2] >= g
    }

    /// `draw_triangle_3d` in the pen colour: host vertex lighting (ambient +
    /// |n.l| point lights), the pen's alpha and blend mode, fog.
    fn tri3d(&mut self, wp: [V3; 3], color: u32) {
        let d = [self.cam.depth(wp[0]), self.cam.depth(wp[1]), self.cam.depth(wp[2])];
        if !self.lens_ok(d) {
            return;
        }
        let base = col3(color);
        let nrm = norm(cross(sub(wp[1], wp[0]), sub(wp[2], wp[0])));
        let lit = |p: V3| -> V3 {
            let a = self.ambient;
            let mut acc = [base[0] * a, base[1] * a, base[2] * a];
            for l in &self.lights {
                let dv = sub(l.p, p);
                let dist = sqrt(dot(dv, dv)).max(1e-6);
                let att = if l.r > 0.0 { (1.0 - dist / l.r).max(0.0) } else { 1.0 };
                if att <= 0.0 {
                    continue;
                }
                let raw = (dot(nrm, [dv[0] / dist, dv[1] / dist, dv[2] / dist])).abs().min(1.0);
                let k = raw * l.i * att;
                acc[0] += base[0] * k * l.c[0];
                acc[1] += base[1] * k * l.c[1];
                acc[2] += base[2] * k * l.c[2];
            }
            [acc[0].min(1.0), acc[1].min(1.0), acc[2].min(1.0)]
        };
        let cols = [lit(wp[0]), lit(wp[1]), lit(wp[2])];
        let alpha = crate::lingapp::pen_alpha();
        let blend = self.blend;
        let p = Params { tex: TexId::None, alpha, blend, zwrite: alpha > 0.97 && blend == 0, ztest: self.depth_test, lit: false, amb: 0.0, fog: self.fog_end > 0.0, hole: [0.0; 4], sprite: false };
        let cull = self.pen_cull();
        self.push_tri(wp, [[0.0; 2]; 3], cols, p, cull, 0.0);
    }

    /// `draw_line_3d`: near-clipped, projected, a pen-coloured line (no depth).
    fn line3d(&mut self, a: V3, b: V3) {
        let near = -self.cam.zdist + 0.05;
        let (mut a, mut b) = (a, b);
        let (da, db) = (self.cam.depth(a), self.cam.depth(b));
        if da <= near && db <= near {
            return;
        }
        if da <= near {
            a = lerp3(a, b, (near - da) / (db - da));
        } else if db <= near {
            b = lerp3(b, a, (near - db) / (da - db));
        }
        let (cx, cy, focal) = self.view();
        let (x0, y0, z0) = self.cam.project(a, cx, cy, focal);
        let (x1, y1, _) = self.cam.project(b, cx, cy, focal);
        let c = self.fog_apply(crate::lingapp::gfx_color(), z0);
        self.sync();
        if let Some((ptr, bw, bh)) = self.target() {
            let buf = unsafe { core::slice::from_raw_parts_mut(ptr, bw * bh) };
            line2d(buf, bw, bh, x0, y0, x1, y1, c);
            self.drew_3d = true;
        }
    }

    /// `mesh_draw(id, x, y, z, scale, yaw, ...)`: the .lmesh's own colours, unlit.
    fn cmesh_draw(&mut self, id: usize, o: V3, sc: f32, yaw: f32) {
        if id >= self.cmeshes.len() {
            return;
        }
        let m = core::mem::replace(&mut self.cmeshes[id], CMesh { pos: Vec::new(), col: Vec::new() });
        let (cs, sn) = (cos(yaw), sin(yaw));
        // (the host's draw_color_mesh queues the baked colours as they are: no fog)
        let p = Params { tex: TexId::None, alpha: 1.0, blend: 0, zwrite: true, ztest: self.depth_test, lit: false, amb: 0.0, fog: false, hole: [0.0; 4], sprite: false };
        for (ti, c) in m.col.iter().enumerate() {
            let w = |k: usize| {
                let q = m.pos[ti * 3 + k];
                [o[0] + (q[0] * cs + q[2] * sn) * sc, o[1] + q[1] * sc, o[2] + (q[2] * cs - q[0] * sn) * sc]
            };
            let col = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0];
            let cull = self.pen_cull();
            self.push_tri([w(0), w(1), w(2)], [[0.0; 2]; 3], [col; 3], p, cull, 0.0);
        }
        self.cmeshes[id] = m;
    }

    /// A glTF mesh's base-colour texture in `texs`, built once per (model, image): meshes that share an
    /// image share the texture (every mesh used to get its own copy -- 155 MB across the NPC models).
    fn gltf_tex(&mut self, h: usize, mi: usize, model: &gltf::Model) -> Option<u32> {
        let ti = model.meshes.get(mi)?.base_tex?;
        if let Some(&t) = self.gltex.get(&(h, ti)) {
            return Some(t);
        }
        let img = model.textures.get(ti)?;
        let size = pow2(img.w.max(img.h).clamp(8, 512));
        self.texs.push(Tex::new(size, resample(&img.px, img.w, img.h, size)));
        let id = (self.texs.len() - 1) as u32;
        self.gltex.insert((h, ti), id);
        Some(id)
    }

    /// Draw skinned glTF positions the host's way: glTF Y-up flipped to the
    /// engine's Y-down (x, -y, -z), scaled, turned by `yaw`, placed at `o`.
    #[allow(clippy::too_many_arguments)]
    fn gltf_draw(&mut self, h: usize, skinned: &[Vec<glam::Vec3>], o: V3, scale: f32, yaw: f32, lit_amb: f32, cull: bool, tint: V3) {
        let Some(model) = gltf::get(h as f64) else { return };
        let (sy, cyy) = (sin(yaw), cos(yaw));
        let fog = self.fog_end > 0.0;
        for (mi, mesh) in model.meshes.iter().enumerate() {
            let Some(sk) = skinned.get(mi) else { continue };
            if sk.is_empty() {
                continue;
            }
            let tex = self.gltf_tex(h, mi, model);
            let world: Vec<V3> = sk
                .iter()
                .map(|v| {
                    let (gx, gy, gz) = (v.x * scale, -v.y * scale, -v.z * scale);
                    [o[0] + gx * cyy + gz * sy, o[1] + gy, o[2] - gx * sy + gz * cyy]
                })
                .collect();
            let p = Params {
                tex: match tex {
                    Some(t) => TexId::Id(t),
                    None => TexId::None,
                },
                alpha: 1.0,
                blend: 0,
                zwrite: true,
                ztest: self.depth_test,
                lit: lit_amb >= 0.0,
                amb: lit_amb.max(0.0),
                fog,
                hole: [0.0; 4],
                sprite: false,
            };
            let base = if tex.is_some() { [1.0; 3] } else { col3(0xC8B49B) };
            let mt = col3(model.tints.get(mi).copied().unwrap_or(0xFF_FFFF));
            let col = [base[0] * mt[0] * tint[0], base[1] * mt[1] * tint[1], base[2] * mt[2] * tint[2]];
            let nv = world.len();
            for t in mesh.indices.chunks_exact(3) {
                let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
                if a >= nv || b >= nv || c >= nv {
                    continue;
                }
                let wp = [world[a], world[b], world[c]];
                let d = [self.cam.depth(wp[0]), self.cam.depth(wp[1]), self.cam.depth(wp[2])];
                // these small triangles are dropped, not clipped, at the lens
                let lensn = -self.cam.zdist + 0.02 + self.lens_guard.max(0.1);
                if d[0] <= lensn || d[1] <= lensn || d[2] <= lensn {
                    continue;
                }
                let uv = [mesh.verts[a].uv, mesh.verts[b].uv, mesh.verts[c].uv];
                // host winding: a back face has non-negative screen area
                let pc = if cull { 1 } else { self.pen_cull() };
                self.push_tri(wp, uv, [col; 3], p, pc, 0.0);
            }
        }
    }

    /// `terrain_draw_grid`: the exact port of draw_ground's quad loop.
    #[allow(clippy::too_many_arguments)]
    fn terrain_draw(&mut self, gbx: f64, gbz: f64, cell: f64, rad: f64, sea_y: f64, t: f64, corners: &[f64]) {
        let steps = (libm::floor(rad * 2.0 / cell) as usize) + 1;
        let npts = steps + 1;
        let lg = |i: usize, j: usize| corners.get(i * npts + j).copied().unwrap_or(0.0);
        let col = |r: f64, g: f64, b: f64| -> u32 {
            let c = |v: f64| v.clamp(0.0, 255.0) as u32;
            (c(r) << 16) | (c(g) << 8) | c(b)
        };
        let v = |x: f64, y: f64, z: f64| [x as f32, y as f32, z as f32];
        for gi in 0..steps {
            for gj in 0..steps {
                let x0 = gbx - rad + gi as f64 * cell;
                let z0 = gbz - rad + gj as f64 * cell;
                let (x1, z1) = (x0 + cell, z0 + cell);
                let (e00, e10, e01, e11) = (lg(gi, gj), lg(gi + 1, gj), lg(gi, gj + 1), lg(gi + 1, gj + 1));
                let ym = (e00 + e10 + e01 + e11) * 0.25;
                let (midx, midz) = ((x0 + x1) * 0.5, (z0 + z1) * 0.5);
                let (y00, y10, y01, y11) = (sea_y - e00, sea_y - e10, sea_y - e01, sea_y - e11);
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
                    let lit = (0.82 + ym * 0.24).min(1.12);
                    match biome_theme(midx, midz, t) {
                        0 => (cr, cg, cb) = (82.0 * lit, 166.0 * lit, 76.0 * lit),
                        1 => (cr, cg, cb) = (86.0 * lit, 166.0 * lit, 122.0 * lit),
                        2 => (cr, cg, cb) = (226.0 * lit, 196.0 * lit, 116.0 * lit),
                        _ => {
                            (cr, cg, cb) = (122.0 * lit, 132.0 * lit, 132.0 * lit);
                            if ym > 0.95 {
                                (cr, cg, cb) = (224.0, 228.0, 228.0);
                            }
                        },
                    }
                }
                let wash = (0.94 + libm::sin(midx * 0.055 + midz * 0.032) * 0.07 + libm::sin(midx * 0.017 - midz * 0.071) * 0.05).clamp(0.84, 1.06);
                cr *= wash;
                cg *= wash;
                cb *= wash;
                let grain = if ym < 0.0 {
                    6.0
                } else if ym > 0.95 {
                    5.0
                } else {
                    18.0
                };
                let (gif, gjf) = (gi as f64, gj as f64);
                let gr1 = (hash01(gif * 7.13 + gjf * 3.71) - 0.5) * grain;
                let gr2 = (hash01(gif * 2.91 + gjf * 8.47 + 91.0) - 0.5) * grain;
                self.tri3d([v(x0, y00, z0), v(x1, y10, z0), v(x1, y11, z1)], col(cr + gr1, cg + gr1, cb + gr1));
                self.tri3d([v(x0, y00, z0), v(x1, y11, z1), v(x0, y01, z1)], col(cr + gr2, cg + gr2, cb + gr2));
                if ym < 0.06 {
                    let wave = |x: f64, z: f64| libm::sin((x + t * 1.6) * 0.5 + z * 0.3) * 0.05 + libm::sin(x * 0.22 - (z + t * 1.1) * 0.3) * 0.03;
                    let (w00, w10, w01, w11) = (wave(x0, z0), wave(x1, z0), wave(x0, z1), wave(x1, z1));
                    let wm = (w00 + w10 + w01 + w11) * 0.25;
                    let c = if wm > 0.05 {
                        col(210.0, 242.0, 250.0)
                    } else if wm > 0.018 {
                        col(122.0, 206.0, 234.0)
                    } else {
                        col(58.0, 168.0, 214.0)
                    };
                    let sy = sea_y - 0.006;
                    self.tri3d([v(x0, sy + w00, z0), v(x1, sy + w10, z0), v(x1, sy + w11, z1)], c);
                    self.tri3d([v(x0, sy + w00, z0), v(x1, sy + w11, z1), v(x0, sy + w01, z1)], c);
                }
            }
        }
    }
}

// ── the host's terrain maths (runtime/terrain_bg.rs, bit-for-bit) ────────────

fn wrap_period(v: f64, p: f64) -> f64 {
    v - libm::floor(v / p) * p
}

fn torus_delta(a: f64, b: f64, p: f64) -> f64 {
    let d = libm::fabs(a - b);
    if d > p * 0.5 { p - d } else { d }
}

#[allow(clippy::too_many_arguments)]
fn island_lobe(u: f64, v: f64, cx: f64, cz: f64, sx: f64, sz: f64, amp: f64, p: f64) -> f64 {
    let dx = torus_delta(u, cx, p) / sx;
    let dz = torus_delta(v, cz, p) / sz;
    let mut g = 1.0 / (1.0 + (dx * dx + dz * dz) * 1.25);
    g *= g;
    g *= g;
    amp * g
}

#[allow(clippy::too_many_arguments)]
fn continent_lobe(u: f64, v: f64, cx: f64, cz: f64, sx: f64, sz: f64, amp: f64, p: f64) -> f64 {
    let dx = torus_delta(u, cx, p) / sx;
    let dz = torus_delta(v, cz, p) / sz;
    amp * (1.0 / (1.0 + (dx * dx + dz * dz) * 0.7))
}

fn terrain_elev(x: f64, z: f64) -> f64 {
    use libm::sin;
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
    let nf = sin(u * 0.021 + v * 0.017) * 0.16 + sin(u * 0.041 - v * 0.033) * 0.09 + sin(u * 0.087 + v * 0.061) * 0.045;
    let mut hills = sin(u * 0.055 + v * 0.041) * 0.34;
    hills += sin(u * 0.031 - v * 0.049) * 0.26;
    hills += sin(u * 0.108 + v * 0.089) * 0.14;
    hills += sin(u * 0.190 - v * 0.160) * 0.07;
    let valley = (sin(u * 0.0072 + v * 0.0053) * 0.85 + sin(u * 0.0140 - v * 0.0098) * 0.42).max(-0.55);
    let landf = (h - 0.5).clamp(0.0, 1.0);
    let bmod = sin(u * 0.09 + v * 0.07) * 0.40 + sin(u * 0.045 - v * 0.058) * 0.25;
    h - 0.66 + nf + valley * 0.5 + (bmod + hills) * (landf * 0.7 + 0.3)
}

fn compute_grid(bx: f64, bz: f64, cell: f64, rad: f64) -> TGrid {
    let steps = libm::floor(rad * 2.0 / cell) + 1.0;
    let npts = steps + 1.0;
    let n = npts as usize;
    let mut corners = Vec::with_capacity(n * n);
    for ci in 0..n {
        let ccx = bx - rad + ci as f64 * cell;
        for cj in 0..n {
            corners.push(terrain_elev(ccx, bz - rad + cj as f64 * cell));
        }
    }
    let get = |i: f64| corners.get(if i < 0.0 { 0 } else { i as usize }).copied().unwrap_or(0.0);
    let (mut nx, mut ny, mut nz) = (Vec::with_capacity(n * n), Vec::with_capacity(n * n), Vec::with_capacity(n * n));
    for ni in 0..n {
        let ni = ni as f64;
        let im1 = (ni - 1.0).max(0.0);
        let ip1 = (ni + 1.0).min(steps);
        for nj in 0..n {
            let nj = nj as f64;
            let jm1 = (nj - 1.0).max(0.0);
            let jp1 = (nj + 1.0).min(steps);
            let (el, er) = (get(im1 * npts + nj), get(ip1 * npts + nj));
            let (ed, eu) = (get(ni * npts + jm1), get(ni * npts + jp1));
            let dx = if (ip1 - im1) * cell < 0.001 { cell } else { (ip1 - im1) * cell };
            let dz = if (jp1 - jm1) * cell < 0.001 { cell } else { (jp1 - jm1) * cell };
            let (gx, gz) = ((er - el) / dx, (eu - ed) / dz);
            let l = libm::sqrt(gx * gx + 1.0 + gz * gz);
            nx.push(gx / l);
            ny.push(1.0 / l);
            nz.push(gz / l);
        }
    }
    TGrid { bx, bz, corners, nx, ny, nz }
}

fn hash01(i: f64) -> f64 {
    let v = libm::sin(i * 12.9898 + 7.13) * 43758.5453;
    v - libm::floor(v)
}

fn biome_theme(x: f64, z: f64, t: f64) -> i32 {
    let p = 1440.0;
    let u = wrap_period(x + p * 0.5, p);
    let v = wrap_period(z + p * 0.5, p);
    let a = libm::sin(u * 0.0095 + v * 0.0061 + t * 0.012);
    let b = libm::sin(u * 0.0078 - v * 0.0110 + t * 0.009);
    if a > 0.0 {
        if b > 0.0 { 0 } else { 2 }
    } else if b > 0.0 {
        1
    } else {
        3
    }
}

fn nlist(v: &[f64]) -> u64 {
    list_from(v.iter().map(|&x| num(x)).collect())
}

/// Parse an `.lmesh` ("LMSH", u32 version, f32 height, u32 ntri, ntri * (3 * xyz f32 + rgb u8)).
fn load_lmesh(path: &str) -> Option<CMesh> {
    let b = crate::vfs::read(path)?;
    if b.len() < 16 || &b[0..4] != b"LMSH" {
        return None;
    }
    let rd = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let ntri = u32::from_le_bytes([b[12], b[13], b[14], b[15]]) as usize;
    if b.len() < 16 + ntri * 39 {
        return None;
    }
    let mut pos = Vec::with_capacity(ntri * 3);
    let mut col = Vec::with_capacity(ntri);
    let mut o = 16;
    for _ in 0..ntri {
        for _ in 0..3 {
            pos.push([rd(o), rd(o + 4), rd(o + 8)]);
            o += 12;
        }
        col.push([b[o], b[o + 1], b[o + 2]]);
        o += 3;
    }
    Some(CMesh { pos, col })
}

/// A flat pose-layer list `[name, rx, ry, rz, ...]` (the host's gltf_anim_draw_tex arg 11).
fn pose_layers(args: &[u64], i: usize) -> Vec<(String, [f32; 3])> {
    let mut out = Vec::new();
    if let Some(&v) = args.get(i) {
        if is_list(v) {
            let items = list_items(v);
            let mut k = 0;
            while k + 3 < items.len() {
                let nm = crate::lingrt::str_of(items[k]);
                out.push((String::from(nm), [to_f64(items[k + 1]) as f32, to_f64(items[k + 2]) as f32, to_f64(items[k + 3]) as f32]));
                k += 4;
            }
        }
    }
    out
}

fn euler_list(args: &[u64], i: usize) -> Vec<f32> {
    match args.get(i) {
        Some(&v) if is_list(v) => list_items(v).iter().map(|&x| to_f64(x) as f32).collect(),
        _ => Vec::new(),
    }
}

/// The host-gfx builtins above; `None` for any other name.
fn dispatch_host(s: &mut State, name: &[u8], args: &[u64]) -> Option<u64> {
    Some(match name {
        b"draw_triangle_3d" | b"triangle3d" => {
            let v = |i: usize| [f(args, i, 0.0), f(args, i + 1, 0.0), f(args, i + 2, 0.0)];
            s.tri3d([v(0), v(3), v(6)], crate::lingapp::gfx_color());
            UNIT
        },
        b"draw_line_3d" | b"line3d" => {
            s.line3d([f(args, 0, 0.0), f(args, 1, 0.0), f(args, 2, 0.0)], [f(args, 3, 0.0), f(args, 4, 0.0), f(args, 5, 0.0)]);
            UNIT
        },
        // hyper_warp(x, y, z) -> [x, y, z]: where the hyperbolic lens draws a world point (the host's)
        b"hyper_warp" => {
            let w = s.cam.warp([f(args, 0, 0.0), f(args, 1, 0.0), f(args, 2, 0.0)]);
            list_from(vec![num(w[0] as f64), num(w[1] as f64), num(w[2] as f64)])
        },
        b"project_3d" => {
            let p = [f(args, 0, 0.0), f(args, 1, 0.0), f(args, 2, 0.0)];
            let d = s.cam.depth(p);
            if d <= -s.cam.zdist + 0.05 {
                return Some(list_from(vec![num(-99999.0), num(-99999.0), num(d as f64)]));
            }
            let (cx, cy, focal) = s.win_view();
            let (x, y, z) = s.cam.project(p, cx, cy, focal);
            list_from(vec![num(x as f64), num(y as f64), num(z as f64)])
        },
        b"set_projection" => {
            s.cam.center = Some((f(args, 0, 960.0), f(args, 1, 540.0)));
            s.cam.focal = f(args, 2, 1080.0);
            s.cam.zdist = f(args, 3, 5.0);
            UNIT
        },
        b"set_camera_hyper" => {
            s.cam.hyp = [f(args, 0, 0.0).max(0.0), f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0), f(args, 4, 0.0)];
            let (cx, _, focal) = s.win_view();
            s.cam.tan_h = (cx / focal.max(1.0)).max(0.05);
            UNIT
        },
        b"mouse_ray" => {
            let (mx, my) = crate::lingapp::app_mouse_xy();
            let (cx, cy, focal) = s.win_view();
            let c = &s.cam;
            let dcx = (mx as f32 - cx) / focal;
            let dcy = (my as f32 - cy) / focal;
            let (ax, ay, az) = (dcx, c.crx * dcy + c.srx, -c.srx * dcy + c.crx);
            let d = norm([c.cry * ax + c.sry * az, ay, -c.sry * ax + c.cry * az]);
            list_from(vec![num(c.t[0] as f64), num(c.t[1] as f64), num(c.t[2] as f64), num(d[0] as f64), num(d[1] as f64), num(d[2] as f64)])
        },
        b"tm_lens_guard" => {
            s.lens_guard = f(args, 0, 0.0).max(0.0);
            UNIT
        },
        b"depth_blur" => UNIT,
        // flush_3d_clip(x, y, w, h): the 3-D drawn since the last flush lands only inside that window-pixel
        // rect (a portrait well); the rest of the window keeps what was drawn on it. Without a rect it is a flush.
        b"flush_3d_clip" => {
            if args.len() >= 4 {
                s.clip_seen = true;
                let (x, y) = (libm::floorf(f(args, 0, 0.0)), libm::floorf(f(args, 1, 0.0)));
                let (w, h) = (libm::ceilf(f(args, 2, 0.0)), libm::ceilf(f(args, 3, 0.0)));
                s.flush_in(Some([x as i32, y as i32, w as i32, h as i32]));
            } else {
                s.flush();
            }
            UNIT
        },
        // ── .lmesh colour meshes ──
        b"mesh_load" => {
            let path = crate::lingapp::path_arg(args, 0);
            match load_lmesh(&path) {
                Some(m) => {
                    s.cmeshes.push(m);
                    num((s.cmeshes.len() - 1) as f64)
                },
                None => {
                    log(&alloc::format!("g3d: mesh_load {path}: not an .lmesh\n"));
                    num(-1.0)
                },
            }
        },
        // mesh_free(id): the .lmesh's memory goes back; its handle stays valid (draws nothing) and a
        // later mesh_load gets a new handle
        b"mesh_free" => {
            let id = n(args, 0, -1.0);
            if id >= 0.0 && (id as usize) < s.cmeshes.len() {
                s.cmeshes[id as usize] = CMesh { pos: Vec::new(), col: Vec::new() };
            }
            UNIT
        },
        b"mesh_draw" => {
            let id = n(args, 0, -1.0);
            if id >= 0.0 {
                s.cmesh_draw(id as usize, [f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)], f(args, 4, 1.0), f(args, 5, 0.0));
            }
            UNIT
        },
        // ── txm_*: the LTM1 meshes + textures of tm_*, the host's texmesh arguments ──
        b"txm_load" => return dispatch(b"tm_load", args),
        b"txm_tex" => {
            let path = crate::lingapp::path_arg(args, 0);
            match load_tex(&path, 256) {
                Some(t) => {
                    s.texs.push(t);
                    num((s.texs.len() - 1) as f64)
                },
                None => num(-1.0),
            }
        },
        b"txm_groups" => return dispatch(b"tm_groups", args),
        b"txm_draw" => {
            // (mesh, group, tex, ox, oy, oz, scale, yaw, tint, alpha, shade, cull, lit_amb)
            // -> tm_draw's (..., tint, alpha, uo, vo, us, vs, shade, blend, cull, lit_amb)
            if n(args, 2, -1.0) < 0.0 {
                return Some(num(0.0));
            }
            let a = |i: usize, d: f64| num(n(args, i, d));
            let v = vec![
                a(0, -1.0),
                a(1, -1.0),
                a(2, -1.0),
                a(3, 0.0),
                a(4, 0.0),
                a(5, 0.0),
                a(6, 1.0),
                a(7, 0.0),
                a(8, 16777215.0),
                a(9, 1.0),
                num(0.0),
                num(0.0),
                num(1.0),
                num(1.0),
                a(10, 0.5),
                num(s.blend as f64),
                a(11, 0.0),
                a(12, -1.0),
            ];
            num(s.tm_draw(&v) as f64)
        },
        b"txm_quad" | b"txm_tri" => {
            let nv = if name == b"txm_quad" { 4 } else { 3 };
            let Some(tex) = tex_arg(s, n(args, 0, -1.0), false).filter(|_| n(args, 0, -1.0) >= 0.0) else { return Some(num(0.0)) };
            let mut q = [[0.0f32; 3]; 4];
            let mut uv = [[0.0f32; 2]; 4];
            for k in 0..nv {
                let b = 1 + k * 5;
                q[k] = [f(args, b, 0.0), f(args, b + 1, 0.0), f(args, b + 2, 0.0)];
                uv[k] = [f(args, b + 3, 0.0), f(args, b + 4, 0.0)];
            }
            let tint = col3(rgb(n(args, 1 + nv * 5, 16777215.0)));
            let alpha = f(args, 2 + nv * 5, 1.0).clamp(0.0, 1.0);
            let blend = s.blend;
            let p = Params { tex, alpha, blend, zwrite: alpha > 0.97 && blend == 0, ztest: s.depth_test, lit: false, amb: 0.0, fog: s.fog_end > 0.0, hole: [0.0; 4], sprite: false };
            let mut c = s.push_tri([q[0], q[1], q[2]], [uv[0], uv[1], uv[2]], [tint; 3], p, 0, 0.0);
            if nv == 4 {
                c += s.push_tri([q[0], q[2], q[3]], [uv[0], uv[2], uv[3]], [tint; 3], p, 0, 0.0);
            }
            num(c as f64)
        },
        // ── glTF models ──
        b"gltf_load" => {
            let path = crate::lingapp::path_arg(args, 0);
            // a card read can fail once and succeed again: a file that exists gets one more try
            let bytes = crate::vfs::read(&path).or_else(|| if crate::vfs::exists(&path) { crate::vfs::read(&path) } else { None });
            let why = match &bytes {
                None if crate::vfs::exists(&path) => "the file is there but could not be read (SD read error or out of memory)",
                None => "no such file (not copied to the card -- run live/lingforfun-assets.py)",
                Some(_) => "not readable as .glb",
            };
            match bytes.as_deref().and_then(gltf::load) {
                Some(m) => {
                    gltf::models().push(m);
                    num((gltf::models().len() - 1) as f64)
                },
                None => {
                    // reported once per file: games retry a missing model every half second
                    let seen = unsafe { &mut *(&raw mut GLTF_REPORTED) };
                    if !seen.iter().any(|p| *p == path) {
                        seen.push(path.clone());
                        log(&alloc::format!("g3d: gltf_load {path}: {why} ({} MiB RAM free, {} models loaded)\n", crate::mm::frame::free_frame_count() * 4 / 1024, gltf::models().len()));
                    }
                    num(-1.0)
                },
            }
        },
        b"gltf_autorig" => num(gltf::get(n(args, 0, -1.0)).map(|m| m.autorig()).unwrap_or(0) as f64),
        b"gltf_anim_count" => num(gltf::get(n(args, 0, -1.0)).map(|m| m.anims.len()).unwrap_or(0) as f64),
        b"gltf_anim_name" => mk_str(&gltf::get(n(args, 0, -1.0)).and_then(|m| m.anims.get(n(args, 1, 0.0) as usize)).map(|a| a.name.clone()).unwrap_or_default()),
        b"gltf_anim_dur" => num(gltf::get(n(args, 0, -1.0)).and_then(|m| m.anims.get(n(args, 1, 0.0).max(0.0) as usize)).map(|a| a.duration).unwrap_or(0.0) as f64),
        b"gltf_tris" => num(gltf::get(n(args, 0, -1.0)).map(|m| m.meshes.iter().map(|x| x.indices.len()).sum::<usize>() / 3).unwrap_or(0) as f64),
        b"gltf_mesh_count" => num(gltf::get(n(args, 0, -1.0)).map(|m| m.meshes.len()).unwrap_or(0) as f64),
        b"gltf_mesh_name" => mk_str(&gltf::get(n(args, 0, -1.0)).and_then(|m| m.meshes.get(n(args, 1, -1.0).max(0.0) as usize)).map(|x| x.name.clone()).unwrap_or_default()),
        b"gltf_mesh_show" => {
            if let Some(m) = gltf::get(n(args, 0, -1.0)) {
                let i = n(args, 1, -1.0);
                if i >= 0.0 && (i as usize) < m.hidden.len() {
                    m.hidden[i as usize] = n(args, 2, 1.0) == 0.0;
                }
            }
            UNIT
        },
        // gltf_mesh_show_list(handle, list): mesh i shown when list[i] is non-zero (meshes past the
        // list's end left as they are); how many were set
        b"gltf_mesh_show_list" => {
            let mut set = 0usize;
            if let (Some(m), Some(&l)) = (gltf::get(n(args, 0, -1.0)), args.get(1)) {
                let items = crate::lingrt::list_items(l);
                for (i, &v) in items.iter().take(m.hidden.len()).enumerate() {
                    m.hidden[i] = !crate::lingrt::truthy(v);
                    set += 1;
                }
            }
            num(set as f64)
        },
        b"gltf_mesh_tint" => {
            if let Some(m) = gltf::get(n(args, 0, -1.0)) {
                let i = n(args, 1, -1.0);
                let c = n(args, 2, 16777215.0);
                if i >= 0.0 && (i as usize) < m.tints.len() {
                    m.tints[i as usize] = if c >= 0.0 { rgb(c) } else { 0xFF_FFFF };
                }
            }
            UNIT
        },
        b"gltf_bounds" => {
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            if let Some(m) = gltf::get(n(args, 0, -1.0)) {
                for mesh in m.skin_animated(n(args, 1, 0.0).max(0.0) as usize, f(args, 2, 0.0)) {
                    for p in mesh {
                        for (k, c) in [p.x, p.y, p.z].into_iter().enumerate() {
                            lo[k] = lo[k].min(c);
                            hi[k] = hi[k].max(c);
                        }
                    }
                }
            }
            if !lo[0].is_finite() {
                lo = [0.0; 3];
                hi = [0.0; 3];
            }
            list_from(lo.iter().chain(hi.iter()).map(|&v| num(v as f64)).collect())
        },
        b"gltf_anim_draw_tex" | b"gltf_anim_draw" => {
            let h = n(args, 0, -1.0);
            if let Some(m) = gltf::get(h) {
                // optional arg 10: animation SOURCE model (the body) — joints
                // matched by name, so clip-less equipment moves with the body
                let src = n(args, 10, -1.0);
                let (ai, at) = (n(args, 6, 0.0).max(0.0) as usize, f(args, 7, 0.0));
                // optional arg 11: procedural pose layers [bone, rx, ry, rz, ...] on top of the clip
                // ("*root*" / "*pivot*" turn the whole model: a ride's tilt, a swim stroke)
                let pose = pose_layers(args, 11);
                let srcm = gltf::models().get(if src >= 0.0 && src != h { src as usize } else { usize::MAX });
                let sk = if !pose.is_empty() {
                    m.skin_animated_posed(srcm.map(|x| &*x), ai, at, &pose)
                } else {
                    match srcm {
                        Some(sm) => m.skin_animated_from(sm, ai, at),
                        None => m.skin_animated(ai, at),
                    }
                };
                // optional arg 12: a tint (0xRRGGBB) the model's colours are multiplied by (a coloured hallway)
                let tint = if args.len() > 12 && n(args, 12, -1.0) >= 0.0 { col3(rgb(n(args, 12, 16777215.0))) } else { [1.0, 1.0, 1.0] };
                s.gltf_draw(h as usize, &sk, [f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)], f(args, 4, 1.0), f(args, 5, 0.0), f(args, 8, -1.0), n(args, 9, 0.0) != 0.0, tint);
            }
            UNIT
        },
        b"gltf_pose_draw_tex" | b"gltf_pose_draw" => {
            let h = n(args, 0, -1.0);
            if let Some(m) = gltf::get(h) {
                let sk = m.skin_local(&euler_list(args, 6));
                s.gltf_draw(h as usize, &sk, [f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)], f(args, 4, 1.0), f(args, 5, 0.0), f(args, 7, -1.0), n(args, 8, 0.0) != 0.0, [1.0; 3]);
            }
            UNIT
        },
        b"gltf_draw" => {
            let h = n(args, 0, -1.0);
            if let Some(m) = gltf::get(h) {
                let sk = m.skin_local(&[]);
                s.gltf_draw(h as usize, &sk, [f(args, 1, 0.0), f(args, 2, 0.0), f(args, 3, 0.0)], f(args, 4, 1.0), f(args, 5, 0.0), -1.0, false, [1.0; 3]);
            }
            UNIT
        },
        // ── terrain (computed at once; the host runs it on a thread) ──
        b"terrain_grid_start" => {
            let g = compute_grid(n(args, 0, 0.0), n(args, 1, 0.0), n(args, 2, 1.35), n(args, 3, 36.0));
            s.tgrid_seq += 1;
            let id = alloc::format!("tg{}", s.tgrid_seq);
            if s.tgrids.len() >= 4 {
                s.tgrids.remove(0);
            }
            s.tgrids.push((id.clone(), g));
            mk_str(&id)
        },
        b"terrain_grid_poll" => {
            let id = crate::lingapp::path_arg(args, 0);
            match s.tgrids.iter().find(|(k, _)| *k == id) {
                Some((_, g)) => list_from(vec![num(g.bx), num(g.bz), nlist(&g.corners), nlist(&g.nx), nlist(&g.ny), nlist(&g.nz)]),
                None => list_from(Vec::new()),
            }
        },
        b"terrain_draw_grid" => {
            let corners: Vec<f64> = match args.get(6) {
                Some(&v) if is_list(v) => list_items(v).iter().map(|&x| to_f64(x)).collect(),
                _ => Vec::new(),
            };
            s.terrain_draw(n(args, 0, 0.0), n(args, 1, 0.0), n(args, 2, 1.35), n(args, 3, 36.0), n(args, 4, 0.08), n(args, 5, 0.0), &corners);
            UNIT
        },
        b"terrain_elev" => num(terrain_elev(n(args, 0, 0.0), n(args, 1, 0.0))),
        _ => return None,
    })
}
