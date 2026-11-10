//! Cross-platform textured mesh (LTM1) support — `txm_*` builtins.
//!
//! A small, self-contained alternative to the native-only `tm_*` family in
//! game3d.rs: it loads the same LTM1 format and uses the same textured-triangle
//! rasterizer (depth queue `push_triangle_tex` -> `fill_triangle_tex_shaded`),
//! but compiles on wasm32 too (files come from `crate::assets` on the web, the
//! filesystem natively), so the game can draw textured, per-vertex-UV models in
//! the browser and on every native target.
//!
//! LTM1 (little-endian): "LTM1" u32 nGroups; per group: u8 nameLen, name,
//! u32 nVerts, u32 nTris, nVerts*[x,y,z,u,v,nx,ny,nz] f32, nTris*[i0,i1,i2] u32.

use std::sync::Arc;

use crate::gfx::depth::{TexData, TexShade, TexVert};

pub struct TxGroup {
    pub name: String,
    pub pos: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    pub nrm: Vec<[f32; 3]>,
    pub idx: Vec<[u32; 3]>,
}

pub struct TxMesh {
    pub groups: Vec<TxGroup>,
}

fn rd_u32(b: &[u8], o: &mut usize) -> Option<u32> {
    if *o + 4 > b.len() {
        return None;
    }
    let v = u32::from_le_bytes([b[*o], b[*o + 1], b[*o + 2], b[*o + 3]]);
    *o += 4;
    Some(v)
}

fn rd_f32(b: &[u8], o: &mut usize) -> Option<f32> {
    if *o + 4 > b.len() {
        return None;
    }
    let v = f32::from_le_bytes([b[*o], b[*o + 1], b[*o + 2], b[*o + 3]]);
    *o += 4;
    Some(v)
}

pub fn parse_ltm(b: &[u8]) -> Result<TxMesh, String> {
    if b.len() < 8 || &b[0..4] != b"LTM1" {
        return Err("not an LTM1 mesh".into());
    }
    let mut o = 4usize;
    let ng = rd_u32(b, &mut o).ok_or("truncated")?;
    let mut groups = Vec::new();
    for _ in 0..ng {
        if o >= b.len() {
            return Err("truncated".into());
        }
        let nl = b[o] as usize;
        o += 1;
        let name = String::from_utf8_lossy(&b[o..(o + nl).min(b.len())]).to_string();
        o += nl;
        let nv = rd_u32(b, &mut o).ok_or("truncated")? as usize;
        let nt = rd_u32(b, &mut o).ok_or("truncated")? as usize;
        let mut g = TxGroup { name, pos: Vec::with_capacity(nv), uv: Vec::with_capacity(nv), nrm: Vec::with_capacity(nv), idx: Vec::with_capacity(nt) };
        for _ in 0..nv {
            let x = rd_f32(b, &mut o).ok_or("truncated")?;
            let y = rd_f32(b, &mut o).ok_or("truncated")?;
            let z = rd_f32(b, &mut o).ok_or("truncated")?;
            let u = rd_f32(b, &mut o).ok_or("truncated")?;
            let v = rd_f32(b, &mut o).ok_or("truncated")?;
            let nx = rd_f32(b, &mut o).ok_or("truncated")?;
            let ny = rd_f32(b, &mut o).ok_or("truncated")?;
            let nz = rd_f32(b, &mut o).ok_or("truncated")?;
            g.pos.push([x, y, z]);
            g.uv.push([u, v]);
            g.nrm.push([nx, ny, nz]);
        }
        for _ in 0..nt {
            let a = rd_u32(b, &mut o).ok_or("truncated")?;
            let c = rd_u32(b, &mut o).ok_or("truncated")?;
            let d = rd_u32(b, &mut o).ok_or("truncated")?;
            if (a as usize) < nv && (c as usize) < nv && (d as usize) < nv {
                g.idx.push([a, c, d]);
            }
        }
        groups.push(g);
    }
    Ok(TxMesh { groups })
}

/// Decode a PNG/JPEG byte buffer to a power-of-two `TexData`.
pub fn tex_from_bytes(bytes: &[u8]) -> Result<TexData, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?.to_rgba8();
    let (w, h) = img.dimensions();
    let sw = pow2(w.clamp(4, 1024));
    let sh = pow2(h.clamp(4, 1024));
    let src = img.as_raw();
    let mut px = Vec::with_capacity((sw * sh) as usize);
    for y in 0..sh {
        let sy = (y * h / sh).min(h - 1);
        for x in 0..sw {
            let sx = (x * w / sw).min(w - 1);
            let i = ((sy * w + sx) * 4) as usize;
            let (r, g, b, a) = (src[i] as u32, src[i + 1] as u32, src[i + 2] as u32, src[i + 3] as u32);
            px.push((a << 24) | (r << 16) | (g << 8) | b);
        }
    }
    Ok(TexData::new(sw, sh, px))
}

fn pow2(v: u32) -> u32 {
    let mut p = 1;
    while p < v {
        p <<= 1;
    }
    p
}

// ── small vec helpers ───────────────────────────────────────────────────────
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: [f32; 3]) -> [f32; 3] {
    let n = dot(a, a).sqrt();
    if n > 1e-6 {
        [a[0] / n, a[1] / n, a[2] / n]
    } else {
        [0.0, 1.0, 0.0]
    }
}
fn lerp_rgb(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let f = |sh: u32| {
        let ca = ((a >> sh) & 255) as f32;
        let cb = ((b >> sh) & 255) as f32;
        (ca + (cb - ca) * t).round() as u32
    };
    (f(16) << 16) | (f(8) << 8) | f(0)
}

/// Rotate a vector by a column-major 3x3 (built by the caller from yaw).
fn mul3(m: &[f32; 9], p: [f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[3] * p[1] + m[6] * p[2],
        m[1] * p[0] + m[4] * p[1] + m[7] * p[2],
        m[2] * p[0] + m[5] * p[1] + m[8] * p[2],
    ]
}

/// One triangle: near-plane clip, perspective-correct TexVert build, push to the
/// depth queue. Mirrors game3d's `g3d_push_poly` so output matches `tm_draw`.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_poly(
    gfx: &mut crate::gfx::GfxState,
    wp: [[f32; 3]; 3],
    uv: [[f32; 2]; 3],
    tex: &Arc<TexData>,
    tint: u32,
    cull: u8,
    shade: Option<&Arc<TexShade>>,
) -> usize {
    push_poly_g(gfx, wp, uv, tex, tint, cull, shade, true)
}

/// `push_poly` with the lens guard optional: glTF models (block hallways, rooms,
/// characters) are near-CLIPPED instead, so a wall tile the camera is right next
/// to stays drawn (its in-front part) rather than vanishing.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_poly_g(
    gfx: &mut crate::gfx::GfxState,
    wp: [[f32; 3]; 3],
    uv: [[f32; 2]; 3],
    tex: &Arc<TexData>,
    tint: u32,
    cull: u8,
    shade: Option<&Arc<TexShade>>,
    guard: bool,
) -> usize {
    let near = -gfx.camera.zdist + 0.05;
    // warp each corner once (the hyperbolic warp is the costly part of a
    // projection): depth, near-clip and projection use the warped points, the
    // per-pixel lighting keeps the true world positions
    let hp = [
        gfx.camera.hyper(wp[0][0], wp[0][1], wp[0][2]),
        gfx.camera.hyper(wp[1][0], wp[1][1], wp[1][2]),
        gfx.camera.hyper(wp[2][0], wp[2][1], wp[2][2]),
    ];
    let hw = [[hp[0].0, hp[0].1, hp[0].2], [hp[1].0, hp[1].1, hp[1].2], [hp[2].0, hp[2].1, hp[2].2]];
    let d = [
        gfx.camera.depth_unwarped(hw[0][0], hw[0][1], hw[0][2]),
        gfx.camera.depth_unwarped(hw[1][0], hw[1][1], hw[1][2]),
        gfx.camera.depth_unwarped(hw[2][0], hw[2][1], hw[2][2]),
    ];
    if d[0] <= near && d[1] <= near && d[2] <= near {
        return 0;
    }
    // Hyperbolic lens: the warp bends vertices, so a long triangle (16-unit terrain and water
    // cells, a building's walls and floors) stays flat between its corners -- the ground drifted
    // under the hero and props sank into it or floated. Split it at its edge midpoints until the
    // bend reads as smooth (game3d::hyper_split has the error bound); every prop, model and the
    // ground then bend the same way.
    if let Some(kids) = super::game3d::hyper_split(&gfx.camera, &wp, &uv) {
        let mut n = 0;
        for (cw, cu) in kids {
            n += push_poly_g(gfx, cw, cu, tex, tint, cull, shade, guard);
        }
        return n;
    }
    // lens guard (tm_lens_guard): a prop triangle with a corner close to the eye is
    // dropped — the camera swinging through a tree or wall otherwise smears huge
    // near-clipped slivers across the view. Large triangles (terrain quads, 16 u)
    // are kept so the ground under a low camera never opens a hole.
    if guard && gfx.lens_guard > 0.0 {
        // (was × 6 — a 1.5-unit bubble at the default 0.25 that blanked whole
        // wall/house tiles the camera merely passed close to; × 2 only drops
        // geometry practically touching the lens)
        let g = near + gfx.lens_guard * 2.0;
        if d[0] < g || d[1] < g || d[2] < g {
            let e = |i: usize, j: usize| {
                let (dx, dy, dz) = (wp[i][0] - wp[j][0], wp[i][1] - wp[j][1], wp[i][2] - wp[j][2]);
                dx * dx + dy * dy + dz * dz
            };
            if e(0, 1).max(e(1, 2)).max(e(2, 0)) < 36.0 {
                return 0;
            }
        }
    }
    // (world position, warped position, uv) per clipped corner
    let mut cp: [([f32; 3], [f32; 3], [f32; 2]); 4] = [([0.0; 3], [0.0; 3], [0.0; 2]); 4];
    let mut cn = 0usize;
    let lerp3 = |a: [f32; 3], b: [f32; 3], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
    for i in 0..3 {
        let j = (i + 1) % 3;
        let ain = d[i] > near;
        let bin = d[j] > near;
        if ain && cn < 4 {
            cp[cn] = (wp[i], hw[i], uv[i]);
            cn += 1;
        }
        if ain != bin && cn < 4 {
            let t = (near - d[i]) / (d[j] - d[i]);
            cp[cn] = (
                lerp3(wp[i], wp[j], t),
                lerp3(hw[i], hw[j], t),
                [uv[i][0] + (uv[j][0] - uv[i][0]) * t, uv[i][1] + (uv[j][1] - uv[i][1]) * t],
            );
            cn += 1;
        }
    }
    if cn < 3 {
        return 0;
    }
    let zd = gfx.camera.zdist;
    let mut tv = [TexVert::default(); 4];
    for k in 0..cn {
        let (p, h, t) = cp[k];
        let (sx, sy, z) = gfx.camera.project_unwarped(h[0], h[1], h[2]);
        let iw = 1.0 / (z + zd).max(1e-4);
        tv[k] = TexVert { x: sx, y: sy, z, iw, uw: t[0] * iw, vw: t[1] * iw, wx: p[0] * iw, wy: p[1] * iw, wz: p[2] * iw };
    }
    if cull != 0 {
        let area = (tv[1].x - tv[0].x) * (tv[2].y - tv[0].y) - (tv[1].y - tv[0].y) * (tv[2].x - tv[0].x);
        if (cull == 1 && area > 0.0) || (cull == 2 && area < 0.0) {
            return 0;
        }
    }
    let sh = shade.cloned();
    gfx.depth_queue.push_triangle_tex_shaded([tv[0], tv[1], tv[2]], tex.clone(), tint, [0.0; 4], true, sh.clone());
    let mut n = 1;
    if cn == 4 {
        gfx.depth_queue.push_triangle_tex_shaded([tv[0], tv[2], tv[3]], tex.clone(), tint, [0.0; 4], true, sh);
        n = 2;
    }
    n
}

impl super::Interpreter {
    pub(super) fn texmesh_builtin(&mut self, name: &str, args: &[Value]) -> Option<EvalResult> {
        match name {
            // txm_load(path) -> mesh handle, or -1 (streams via assets on the web)
            "txm_load" | "텍스처메시로드" | "加载纹理网格" | "テクスチャメッシュ読込"
            | "โหลดเมชเท็กซ์เจอร์" | "بارگذاری_مش_بافت" | "تحميل_شبكة_منقوشة"
            | "טען_רשת_ממורקמת" | "ٹیکسچر_میش_لوڈ" | "charger_maillage_texturé"
            | "texturmesh_laden" | "загрузить_текстурную_сетку" => {
                let path = self.arg_str(args, 0, "");
                let bytes = match read_asset(self, &path) {
                    Some(b) => b,
                    None => return Some(Ok(Value::Number(-1.0))),
                };
                match parse_ltm(&bytes) {
                    Ok(m) => {
                        self.tex_meshes.push(m);
                        Some(Ok(Value::Number((self.tex_meshes.len() - 1) as f64)))
                    },
                    Err(_) => Some(Ok(Value::Number(-1.0))),
                }
            },
            // txm_tex(path) -> texture handle, or -1
            "txm_tex" | "텍스처로드2" | "加载纹理2" | "テクスチャ読込2"
            | "โหลดเท็กซ์เจอร์2" | "بارگذاری_بافت" | "تحميل_نقش"
            | "טען_מרקם" | "ٹیکسچر_لوڈ2" | "charger_texture2"
            | "textur_laden2" | "загрузить_текстуру2" => {
                let path = self.arg_str(args, 0, "");
                let bytes = match read_asset(self, &path) {
                    Some(b) => b,
                    None => return Some(Ok(Value::Number(-1.0))),
                };
                match tex_from_bytes(&bytes) {
                    Ok(t) => {
                        self.tex_images.push(Arc::new(t));
                        Some(Ok(Value::Number((self.tex_images.len() - 1) as f64)))
                    },
                    Err(_) => Some(Ok(Value::Number(-1.0))),
                }
            },
            // ── ground collision from the DRAWN terrain mesh ──
            // txm_ground_add(mesh, ox, oy, oz): add a loaded mesh's triangles, placed the way
            //   txm_draw(mesh, -1, tex, ox, oy, oz, 1, 0, ...) draws them, to the collision set
            //   (once per mesh + offset) -> triangles added
            // txm_ground_y(x, z): the drawn surface's height there (the highest one, y-down),
            //   or -99999 where no added mesh covers it
            // txm_ground_clear(): empty the set
            "txm_ground_add" => {
                let mi = self.arg_num(args, 0, -1.0).ok()? as i64;
                let o = [
                    self.arg_num(args, 1, 0.0).ok()? as f32,
                    self.arg_num(args, 2, 0.0).ok()? as f32,
                    self.arg_num(args, 3, 0.0).ok()? as f32,
                ];
                let n = match (mi >= 0).then(|| self.tex_meshes.get(mi as usize)).flatten() {
                    Some(m) => ground::add(mi as usize, m, o),
                    None => 0,
                };
                Some(Ok(Value::Number(n as f64)))
            },
            "txm_ground_y" => {
                let x = self.arg_num(args, 0, 0.0).ok()? as f32;
                let z = self.arg_num(args, 1, 0.0).ok()? as f32;
                Some(Ok(Value::Number(ground::height(x, z).map_or(-99999.0, |y| y as f64))))
            },
            "txm_ground_clear" => {
                ground::clear();
                Some(Ok(Value::Unit))
            },
            "txm_groups" | "텍스처메시그룹수" | "纹理网格组数" => {
                let mi = self.arg_num(args, 0, -1.0).ok()? as i64;
                let n = if mi >= 0 {
                    self.tex_meshes.get(mi as usize).map(|m| m.groups.len()).unwrap_or(0)
                } else {
                    0
                };
                Some(Ok(Value::Number(n as f64)))
            },
            // txm_draw(mesh, group(-1=all), tex, ox, oy, oz, scale, yaw, tint, alpha, shade, cull, lit_amb)
            // txm_lod(px1, px2) — distance-based detail for txm_draw: a mesh group whose
            //   projected radius is under px1 pixels draws a simplified copy (vertex
            //   clustering, built once), under px2 a coarser one. 0 = off (the default
            //   unless LING_TXM_LOD="px1,px2" is set). Returns the active px1.
            "txm_lod" => {
                let a = self.arg_num(args, 0, 0.0).ok()? as f32;
                let b = self.arg_num(args, 1, 0.0).ok()? as f32;
                set_lod(a, b);
                return Some(Ok(Value::Number(lod_thresholds().0 as f64)));
            },
            "txm_draw" | "텍스처메시그리기" | "绘制纹理网格" | "テクスチャメッシュ描画"
            | "วาดเมชเท็กซ์เจอร์" | "رسم_مش_بافت" | "ارسم_شبكة_منقوشة"
            | "צייר_רשת_ממורקמת" | "ٹیکسچر_میش_ڈرا" | "dessiner_maillage_texturé"
            | "texturmesh_zeichnen" | "нарисовать_текстурную_сетку" => {
                let mi = self.arg_num(args, 0, -1.0).ok()? as i64;
                let gi = self.arg_num(args, 1, -1.0).ok()? as i64;
                let ti = self.arg_num(args, 2, -1.0).ok()? as i64;
                let o = [
                    self.arg_num(args, 3, 0.0).ok()? as f32,
                    self.arg_num(args, 4, 0.0).ok()? as f32,
                    self.arg_num(args, 5, 0.0).ok()? as f32,
                ];
                let sc = self.arg_num(args, 6, 1.0).ok()? as f32;
                let yaw = self.arg_num(args, 7, 0.0).ok()? as f32;
                let tint = (self.arg_num(args, 8, 16777215.0).ok()? as u32) & 0xFFFFFF;
                let alpha = (self.arg_num(args, 9, 1.0).ok()? as f32).clamp(0.0, 1.0);
                let shade_amt = (self.arg_num(args, 10, 0.5).ok()? as f32).clamp(0.0, 1.0);
                let cull = self.arg_num(args, 11, 0.0).ok()? as u8;
                let lit_amb = self.arg_num(args, 12, -1.0).ok()? as f32;
                // PBR-ish (metallic/roughness): a cheap per-triangle Blinn-Phong
                // specular highlight from the key light + view direction. metallic
                // tints the highlight toward the albedo (metals) vs white (dielectric);
                // roughness widens/dims it. 0 metallic = matte, the old look.
                let metallic = (self.arg_num(args, 13, 0.0).ok()? as f32).clamp(0.0, 1.0);
                let roughness = (self.arg_num(args, 14, 0.6).ok()? as f32).clamp(0.04, 1.0);
                if mi < 0 || mi as usize >= self.tex_meshes.len() {
                    return Some(Ok(Value::Number(0.0)));
                }
                let tex = match self.tex_images.get(ti as usize) {
                    Some(t) => t.clone(),
                    None => return Some(Ok(Value::Number(0.0))),
                };
                // column-major yaw rotation about Y
                let (cy, sy) = (yaw.cos(), yaw.sin());
                let m = [cy, 0.0, -sy, 0.0, 1.0, 0.0, sy, 0.0, cy];
                let light = norm([0.35, -0.8, -0.45]);
                let spec_pow = (2.0f32).powf((1.0 - roughness) * 9.0 + 1.0); // ~2..1024
                let spec_k = metallic * 0.04f32.max(1.0 - roughness * 0.7);
                let mut gfx = self.gfx.borrow_mut();
                let eye = [gfx.camera.tx, gfx.camera.ty, gfx.camera.tz];
                let (pm, pa) = (gfx.blend, gfx.alpha);
                gfx.depth_queue.set_state(pm, alpha);
                let shade: Option<Arc<TexShade>> = if lit_amb >= 0.0 {
                    Some(Arc::new(TexShade {
                        amb: lit_amb.max(0.0),
                        lights: gfx.lights.iter().map(|l| [l.x, l.y, l.z, l.r, l.g, l.b, l.intensity, if l.radius > 0.0 { l.radius } else { 1.0e6 }]).collect(),
                        fog_start: gfx.fog_start,
                        fog_end: gfx.fog_end,
                        fog_color: gfx.fog_color,
                        lit: true,
                        ripples: Vec::new(),
                    }))
                } else {
                    None
                };
                let mesh = &self.tex_meshes[mi as usize];
                let mut count = 0usize;
                for (gidx, g) in mesh.groups.iter().enumerate() {
                    if gi >= 0 && gidx != gi as usize {
                        continue;
                    }
                    // whole-group view cull (bounding sphere, cached per mesh/group): a
                    // prop or terrain chunk entirely behind the lens or off-screen would
                    // only have every triangle transformed, shaded and then dropped
                    let Some(screen_r) = group_view(&gfx, mi as usize, gidx, g, &m, sc, o) else {
                        continue;
                    };
                    // set_cull(3) (or a per-call cull of 3): cull this group only if
                    // it is a closed surface, on the side its winding says is the back
                    let cull = if cull == 3 || (cull == 0 && gfx.cull == 3) {
                        crate::gfx::poly::auto_cull_mode(group_orientation(mi as usize, gidx, g), sc)
                    } else {
                        cull
                    };
                    // distance-based detail: small on screen → a simplified copy of the group
                    // (txm_lod / LING_TXM_LOD); the cull side comes from the full mesh above
                    let lod = lod_group(mi as usize, gidx, g, screen_r);
                    let g: &TxGroup = match &lod {
                        Some(l) => l,
                        None => g,
                    };
                    for t in &g.idx {
                        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
                        let wp = [
                            add3(mul3s(&m, g.pos[a], sc), o),
                            add3(mul3s(&m, g.pos[b], sc), o),
                            add3(mul3s(&m, g.pos[c], sc), o),
                        ];
                        let uv = [g.uv[a], g.uv[b], g.uv[c]];
                        // flat per-triangle diffuse shade + optional PBR-ish spec, baked into the tint (cheap)
                        let tt = if shade_amt > 0.0 || spec_k > 0.0 {
                            let fnrm = norm(cross(sub(wp[1], wp[0]), sub(wp[2], wp[0])));
                            let vn = mul3(&m, g.nrm[a]);
                            let n = if dot(vn, vn) > 0.25 { norm(vn) } else { fnrm };
                            let l = dot(n, light).abs();
                            let k = 1.0 - shade_amt * (1.0 - (0.4 + 0.6 * l));
                            let mut base = lerp_rgb(0, tint, k);
                            if spec_k > 0.0 {
                                let cen = [(wp[0][0] + wp[1][0] + wp[2][0]) / 3.0, (wp[0][1] + wp[1][1] + wp[2][1]) / 3.0, (wp[0][2] + wp[1][2] + wp[2][2]) / 3.0];
                                let v = norm(sub(eye, cen));
                                // key light points along -light; the direction to it is +light (negated)
                                let h = norm([v[0] - light[0], v[1] - light[1], v[2] - light[2]]);
                                let nh = dot(n, h).max(0.0);
                                let s = nh.powf(spec_pow) * spec_k;
                                if s > 0.001 {
                                    // metals tint the highlight toward the albedo; dielectrics stay white
                                    let hi = lerp_rgb(0xFFFFFF, tint, metallic);
                                    base = add_rgb(base, scale_rgb(hi, s));
                                }
                            }
                            base
                        } else {
                            tint
                        };
                        count += push_poly(&mut gfx, wp, uv, &tex, tt, cull, shade.as_ref());
                    }
                }
                gfx.depth_queue.set_state(pm, pa);
                Some(Ok(Value::Number(count as f64)))
            },
            // txm_quad(tex, x0,y0,z0,u0,v0, x1,y1,z1,u1,v1, x2,y2,z2,u2,v2, x3,y3,z3,u3,v3, tint, alpha)
            //   one textured world-space quad (0-1-2 + 0-2-3), unlit, double-sided,
            //   using the CURRENT blend mode (블렌드모드(1) = additive glow). For
            //   effects: billboards, animated rings/cones, decals — geometry the
            //   script builds per frame, which txm_draw's baked meshes can't do.
            // txm_tri(tex, x0,y0,z0,u0,v0, x1,y1,z1,u1,v1, x2,y2,z2,u2,v2, tint, alpha) — one triangle.
            "txm_quad" | "txm_tri" => {
                let ti = self.arg_num(args, 0, -1.0).ok()? as i64;
                let nv = if name == "txm_quad" { 4 } else { 3 };
                let mut p = [[0.0f32; 3]; 4];
                let mut uv = [[0.0f32; 2]; 4];
                for k in 0..nv {
                    let b = 1 + k * 5;
                    p[k] = [
                        self.arg_num(args, b, 0.0).ok()? as f32,
                        self.arg_num(args, b + 1, 0.0).ok()? as f32,
                        self.arg_num(args, b + 2, 0.0).ok()? as f32,
                    ];
                    uv[k] = [self.arg_num(args, b + 3, 0.0).ok()? as f32, self.arg_num(args, b + 4, 0.0).ok()? as f32];
                }
                let tint = (self.arg_num(args, 1 + nv * 5, 16777215.0).ok()? as u32) & 0xFFFFFF;
                let alpha = (self.arg_num(args, 2 + nv * 5, 1.0).ok()? as f32).clamp(0.0, 1.0);
                let tex = match if ti >= 0 { self.tex_images.get(ti as usize) } else { None } {
                    Some(t) => t.clone(),
                    None => return Some(Ok(Value::Number(0.0))),
                };
                let mut gfx = self.gfx.borrow_mut();
                let (pm, pa) = (gfx.blend, gfx.alpha);
                gfx.depth_queue.set_state(pm, alpha);
                let mut n = push_poly(&mut gfx, [p[0], p[1], p[2]], [uv[0], uv[1], uv[2]], &tex, tint, 0, None);
                if nv == 4 {
                    n += push_poly(&mut gfx, [p[0], p[2], p[3]], [uv[0], uv[2], uv[3]], &tex, tint, 0, None);
                }
                gfx.depth_queue.set_state(pm, pa);
                Some(Ok(Value::Number(n as f64)))
            },
            _ => None,
        }
    }
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale_rgb(c: u32, s: f32) -> u32 {
    let f = |sh: u32| (((c >> sh) & 255) as f32 * s).round().clamp(0.0, 255.0) as u32;
    (f(16) << 16) | (f(8) << 8) | f(0)
}
fn add_rgb(a: u32, b: u32) -> u32 {
    let f = |sh: u32| (((a >> sh) & 255) + ((b >> sh) & 255)).min(255);
    (f(16) << 16) | (f(8) << 8) | f(0)
}
fn mul3s(m: &[f32; 9], p: [f32; 3], s: f32) -> [f32; 3] {
    let r = mul3(m, p);
    [r[0] * s, r[1] * s, r[2] * s]
}

/// Read a model/texture file: filesystem natively (also relative to the script
/// dir), the streamed-asset registry on the web (requesting it if absent).
fn read_asset(interp: &super::Interpreter, path: &str) -> Option<Vec<u8>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Ok(b) = std::fs::read(path) {
            return Some(b);
        }
        if let Some(dir) = &interp.source_dir {
            if let Ok(b) = std::fs::read(dir.join(path)) {
                return Some(b);
            }
        }
        None
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = interp;
        match crate::assets::get(path) {
            Some(b) => Some(b),
            None => {
                crate::assets::request(path);
                None
            },
        }
    }
}

use super::EvalResult;
use super::Value;


thread_local! {
    /// (mesh, group) → (vertex count when measured, local centre, local radius)
    static TXM_SPHERE: std::cell::RefCell<std::collections::HashMap<(usize, usize), (usize, [f32; 3], f32)>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// true when the group's bounding sphere is certainly invisible: wholly behind
/// the lens, or wholly off-screen (with the hyperbolic view on, only beyond the
/// 1.8× half-FOV radius that camera.hyper() leaves unwarped).
/// Closed-surface orientation of a textured mesh group (for `set_cull(3)`),
/// computed once per (mesh, group) and cached with the group's size as a
/// staleness check.
fn group_orientation(mi: usize, gidx: usize, g: &TxGroup) -> i8 {
    thread_local! {
        static TXM_ORIENT: std::cell::RefCell<std::collections::HashMap<(usize, usize), (usize, i8)>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    TXM_ORIENT.with(|c| {
        if let Some(&(n, o)) = c.borrow().get(&(mi, gidx)) {
            if n == g.idx.len() {
                return o;
            }
        }
        let o = crate::gfx::poly::closed_mesh_orientation(g.idx.iter().filter_map(|t| {
            let (a, b, cc) = (t[0] as usize, t[1] as usize, t[2] as usize);
            Some([*g.pos.get(a)?, *g.pos.get(b)?, *g.pos.get(cc)?])
        }));
        c.borrow_mut().insert((mi, gidx), (g.idx.len(), o));
        o
    })
}

// ── distance-based detail (LOD) for textured meshes ────────────────────────────
// Thresholds in projected-radius pixels: below LOD_PX1 a group draws its level-1
// simplification, below LOD_PX2 level 2. 0 = off. Set by txm_lod(px1, px2) or the
// LING_TXM_LOD="px1,px2" environment variable (read once).
static LOD_PX1: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);
static LOD_PX2: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn lod_thresholds() -> (f32, f32) {
    use std::sync::atomic::Ordering::Relaxed;
    if LOD_PX1.load(Relaxed) == u32::MAX {
        let (a, b) = std::env::var("LING_TXM_LOD")
            .ok()
            .and_then(|v| {
                let mut it = v.split(',').map(|x| x.trim().parse::<f32>().ok());
                Some((it.next()??, it.next().flatten().unwrap_or(0.0)))
            })
            .unwrap_or((0.0, 0.0));
        set_lod(a, b);
    }
    (f32::from_bits(LOD_PX1.load(Relaxed)), f32::from_bits(LOD_PX2.load(Relaxed)))
}

pub(crate) fn set_lod(px1: f32, px2: f32) {
    use std::sync::atomic::Ordering::Relaxed;
    let px1 = if px1.is_finite() { px1.max(0.0) } else { 0.0 };
    let px2 = if px2.is_finite() { px2.clamp(0.0, px1) } else { 0.0 };
    LOD_PX1.store(px1.to_bits(), Relaxed);
    LOD_PX2.store(px2.to_bits(), Relaxed);
}

thread_local! {
    /// (mesh, group, level) → (source vertex count, simplified group or None = not worth it)
    static TXM_LOD: std::cell::RefCell<std::collections::HashMap<(usize, usize, u8), (usize, Option<std::rc::Rc<TxGroup>>)>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The simplified copy of group `g` to draw at projected radius `screen_r`, if the
/// LOD is on and the group is small enough on screen (None = draw the full group).
fn lod_group(mi: usize, gidx: usize, g: &TxGroup, screen_r: f32) -> Option<std::rc::Rc<TxGroup>> {
    let (px1, px2) = lod_thresholds();
    if px1 <= 0.0 || !(screen_r < px1) {
        return None;
    }
    let level: u8 = if screen_r < px2 { 2 } else { 1 };
    TXM_LOD.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((n, lod)) = c.get(&(mi, gidx, level)) {
            if *n == g.pos.len() {
                return lod.clone();
            }
        }
        let cells = if level == 2 { 6.0 } else { 12.0 };
        let simp = cluster_lod(g, cells);
        // only worth it when it removes a real share of the triangles
        let lod = if simp.idx.len() * 100 <= g.idx.len() * 85 && !simp.idx.is_empty() { Some(std::rc::Rc::new(simp)) } else { None };
        c.insert((mi, gidx, level), (g.pos.len(), lod.clone()));
        lod
    })
}

/// Vertex-clustering simplification: snap every vertex to a grid of `cells` cells
/// across the group's largest extent, merge each cell's vertices into one (first
/// vertex's uv / normal, averaged position) and drop triangles that collapse or
/// repeat. Winding is kept, so back-face culling still works on the result.
fn cluster_lod(g: &TxGroup, cells: f32) -> TxGroup {
    use std::collections::{HashMap, HashSet};
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in &g.pos {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let ext = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]);
    let mut out = TxGroup { name: g.name.clone(), pos: Vec::new(), uv: Vec::new(), nrm: Vec::new(), idx: Vec::new() };
    if !(ext > 0.0) || g.pos.is_empty() {
        out.pos = g.pos.clone();
        out.uv = g.uv.clone();
        out.nrm = g.nrm.clone();
        out.idx = g.idx.clone();
        return out;
    }
    let cs = ext / cells;
    let mut cell_of: HashMap<(i32, i32, i32), u32> = HashMap::new();
    let mut sums: Vec<([f32; 3], f32)> = Vec::new();
    let mut remap: Vec<u32> = Vec::with_capacity(g.pos.len());
    for (vi, p) in g.pos.iter().enumerate() {
        let key = (((p[0] - lo[0]) / cs) as i32, ((p[1] - lo[1]) / cs) as i32, ((p[2] - lo[2]) / cs) as i32);
        let ni = *cell_of.entry(key).or_insert_with(|| {
            out.pos.push(*p);
            out.uv.push(g.uv.get(vi).copied().unwrap_or([0.0; 2]));
            out.nrm.push(g.nrm.get(vi).copied().unwrap_or([0.0; 3]));
            sums.push(([0.0; 3], 0.0));
            (out.pos.len() - 1) as u32
        });
        let s = &mut sums[ni as usize];
        for k in 0..3 {
            s.0[k] += p[k];
        }
        s.1 += 1.0;
        remap.push(ni);
    }
    for (i, (sum, n)) in sums.iter().enumerate() {
        out.pos[i] = [sum[0] / n, sum[1] / n, sum[2] / n];
    }
    let mut seen: HashSet<[u32; 3]> = HashSet::new();
    for t in &g.idx {
        let (a, bb, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
        if a >= remap.len() || bb >= remap.len() || c >= remap.len() {
            continue;
        }
        let (a, bb, c) = (remap[a], remap[bb], remap[c]);
        if a == bb || bb == c || a == c {
            continue;
        }
        let mut k = [a, bb, c];
        k.sort_unstable();
        if seen.insert(k) {
            out.idx.push([a, bb, c]);
        }
    }
    out
}

/// View test of a whole group (bounding sphere, cached per mesh/group): `None` when it
/// is behind the lens or off-screen, else its projected radius in pixels (infinite
/// when the sphere reaches the eye plane) — the screen size the LOD picks from.
fn group_view(gfx: &crate::gfx::GfxState, mi: usize, gidx: usize, g: &TxGroup, m: &[f32; 9], sc: f32, o: [f32; 3]) -> Option<f32> {
    let (c, r) = TXM_SPHERE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(&(n, c, r)) = cache.get(&(mi, gidx)) {
            if n == g.pos.len() {
                return (c, r);
            }
        }
        if g.pos.is_empty() {
            return ([0.0; 3], 0.0);
        }
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in &g.pos {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
        let mut r2 = 0.0f32;
        for p in &g.pos {
            let (dx, dy, dz) = (p[0] - c[0], p[1] - c[1], p[2] - c[2]);
            r2 = r2.max(dx * dx + dy * dy + dz * dz);
        }
        let r = r2.sqrt();
        cache.insert((mi, gidx), (g.pos.len(), c, r));
        (c, r)
    });
    if g.pos.is_empty() {
        return None;
    }
    let wc = add3(mul3s(m, c, sc), o);
    let rw = r * sc.abs() + 0.5;
    let cam = &gfx.camera;
    let (sx, sy, rz) = cam.project(wc[0], wc[1], wc[2]);
    let near = -cam.zdist + 0.05;
    if rz + rw < near {
        return None;
    }
    let dd = rz + cam.zdist;
    if dd <= rw + 0.5 {
        return Some(f32::INFINITY); // the sphere reaches the eye plane: perspective size unbounded
    }
    let size = cam.focal * rw / (dd - rw);
    let pr = size * 1.15 + 16.0;
    if cam.hyp_r <= 0.0 {
        let (w, h) = (gfx.width as f32, gfx.height as f32);
        let off = sx + pr < 0.0 || sx - pr > w || sy + pr < 0.0 || sy - pr > h;
        if off { None } else { Some(size) }
    } else {
        let rad = ((sx - cam.cx) * (sx - cam.cx) + (sy - cam.cy) * (sy - cam.cy)).sqrt();
        if rad - pr > 1.85 * cam.cx { None } else { Some(size) }
    }
}

#[cfg(test)]
mod lod_tests {
    use super::*;

    #[test]
    fn cluster_lod_reduces_and_keeps_winding() {
        // a finely tessellated, consistently wound 32x32 grid (2048 triangles)
        let n = 32u32;
        let mut g = TxGroup { name: "grid".into(), pos: vec![], uv: vec![], nrm: vec![], idx: vec![] };
        for j in 0..=n {
            for i in 0..=n {
                g.pos.push([i as f32 / n as f32, ((i * 7 + j * 3) % 5) as f32 * 0.002, j as f32 / n as f32]);
                g.uv.push([i as f32 / n as f32, j as f32 / n as f32]);
                g.nrm.push([0.0, 1.0, 0.0]);
            }
        }
        let v = |i: u32, j: u32| j * (n + 1) + i;
        for j in 0..n {
            for i in 0..n {
                g.idx.push([v(i, j), v(i + 1, j), v(i + 1, j + 1)]);
                g.idx.push([v(i, j), v(i + 1, j + 1), v(i, j + 1)]);
            }
        }
        let normal_y = |p: &[[f32; 3]], t: &[u32; 3]| {
            let (a, b, c) = (p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]);
            let (u, w) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
            u[2] * w[0] - u[0] * w[2]
        };
        let src_sign = normal_y(&g.pos, &g.idx[0]).signum();
        for cells in [12.0, 6.0] {
            let l = cluster_lod(&g, cells);
            assert!(l.idx.len() * 4 < g.idx.len(), "cells {cells}: {} of {} tris", l.idx.len(), g.idx.len());
            assert!(!l.idx.is_empty());
            for t in &l.idx {
                assert!(t.iter().all(|&k| (k as usize) < l.pos.len()));
                assert_eq!(normal_y(&l.pos, t).signum(), src_sign, "winding flipped");
            }
        }
    }
}

/// Ground collision built from the drawn terrain meshes: their triangles in a uniform
/// XZ grid, so "how high is the ground here" is the very surface the rasteriser draws
/// (not a separate height map that can disagree with it).
mod ground {
    use super::TxMesh;
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    const CELL: f32 = 8.0;

    #[derive(Default)]
    struct Ground {
        tris: Vec<[f32; 9]>,
        grid: HashMap<(i32, i32), Vec<u32>>,
        added: HashSet<(usize, i32, i32, i32)>,
    }

    thread_local! {
        static G: RefCell<Ground> = RefCell::new(Ground::default());
    }

    fn cell(v: f32) -> i32 {
        (v / CELL).floor() as i32
    }

    pub fn clear() {
        G.with(|g| *g.borrow_mut() = Ground::default());
    }

    pub fn add(mi: usize, m: &TxMesh, o: [f32; 3]) -> usize {
        let key = (mi, (o[0] * 16.0) as i32, (o[1] * 16.0) as i32, (o[2] * 16.0) as i32);
        G.with(|g| {
            let mut g = g.borrow_mut();
            if !g.added.insert(key) {
                return 0;
            }
            let mut n = 0;
            for grp in &m.groups {
                for t in &grp.idx {
                    let p = |k: usize| {
                        let v = grp.pos.get(t[k] as usize).copied().unwrap_or([0.0; 3]);
                        [v[0] + o[0], v[1] + o[1], v[2] + o[2]]
                    };
                    let (a, b, c) = (p(0), p(1), p(2));
                    // skip walls: a near-vertical triangle has (almost) no XZ footprint
                    let area = (b[0] - a[0]) * (c[2] - a[2]) - (c[0] - a[0]) * (b[2] - a[2]);
                    if area.abs() < 1e-6 {
                        continue;
                    }
                    let id = g.tris.len() as u32;
                    g.tris.push([a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2]]);
                    let (x0, x1) = (cell(a[0].min(b[0]).min(c[0])), cell(a[0].max(b[0]).max(c[0])));
                    let (z0, z1) = (cell(a[2].min(b[2]).min(c[2])), cell(a[2].max(b[2]).max(c[2])));
                    for cx in x0..=x1 {
                        for cz in z0..=z1 {
                            g.grid.entry((cx, cz)).or_default().push(id);
                        }
                    }
                    n += 1;
                }
            }
            n
        })
    }

    pub fn height(x: f32, z: f32) -> Option<f32> {
        G.with(|g| {
            let g = g.borrow();
            let ids = g.grid.get(&(cell(x), cell(z)))?;
            let mut best: Option<f32> = None;
            for &id in ids {
                let t = &g.tris[id as usize];
                let (ax, az, bx, bz, cx, cz) = (t[0], t[2], t[3], t[5], t[6], t[8]);
                let d = (bz - cz) * (ax - cx) + (cx - bx) * (az - cz);
                if d.abs() < 1e-9 {
                    continue;
                }
                let w0 = ((bz - cz) * (x - cx) + (cx - bx) * (z - cz)) / d;
                let w1 = ((cz - az) * (x - cx) + (ax - cx) * (z - cz)) / d;
                let w2 = 1.0 - w0 - w1;
                let e = -1e-4;
                if w0 >= e && w1 >= e && w2 >= e {
                    let y = w0 * t[1] + w1 * t[4] + w2 * t[7];
                    // y-down: the highest surface has the smallest y
                    best = Some(best.map_or(y, |b: f32| b.min(y)));
                }
            }
            best
        })
    }

    #[cfg(test)]
    mod tests {
        use super::super::{TxGroup, TxMesh};

        #[test]
        fn height_follows_the_triangles() {
            super::clear();
            let m = TxMesh { groups: vec![TxGroup { name: String::new(), pos: vec![[0.0, 1.0, 0.0], [10.0, 3.0, 0.0], [0.0, 5.0, 10.0]], uv: vec![], nrm: vec![], idx: vec![[0, 1, 2]] }] };
            assert_eq!(super::add(0, &m, [100.0, 0.0, 0.0]), 1);
            assert_eq!(super::add(0, &m, [100.0, 0.0, 0.0]), 0, "same mesh + offset once");
            let y = super::height(102.0, 2.0).unwrap();
            assert!((y - (1.0 + 0.2 * 2.0 + 0.2 * 4.0)).abs() < 1e-4, "{y}");
            assert!(super::height(50.0, 2.0).is_none());
        }
    }
}
