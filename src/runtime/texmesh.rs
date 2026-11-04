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

/// txm_draw groups at least this big are transformed + shaded on rayon.
#[cfg(not(target_arch = "wasm32"))]
const PAR_TRIS: usize = 1024;

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
    Ok(TexData { w: sw, h: sh, px })
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
    let lg = if guard { gfx.lens_guard } else { 0.0 };
    match prep_poly(&gfx.camera, lg, wp, uv, cull) {
        Some((tv, cn)) => push_prepped(gfx, &tv, cn, tex, tint, shade),
        None => 0,
    }
}

/// Queue a triangle `prep_poly` produced (a fan of 1 or 2 when near-clipped to a quad).
pub(super) fn push_prepped(
    gfx: &mut crate::gfx::GfxState,
    tv: &[TexVert; 4],
    cn: usize,
    tex: &Arc<TexData>,
    tint: u32,
    shade: Option<&Arc<TexShade>>,
) -> usize {
    let sh = shade.cloned();
    if cn == 4 {
        gfx.depth_queue.push_triangle_tex_shaded([tv[0], tv[1], tv[2]], tex.clone(), tint, [0.0; 4], true, sh.clone());
        gfx.depth_queue.push_triangle_tex_shaded([tv[0], tv[2], tv[3]], tex.clone(), tint, [0.0; 4], true, sh);
        2
    } else {
        gfx.depth_queue.push_triangle_tex_shaded([tv[0], tv[1], tv[2]], tex.clone(), tint, [0.0; 4], true, sh);
        1
    }
}

/// The pure half of `push_poly_g` (no GfxState writes, so it runs on worker
/// threads): near-plane clip, lens guard (`lens_guard` 0 = off), projection and
/// back-face cull. Returns the projected polygon and its vertex count (3 or 4).
pub(super) fn prep_poly(
    cam: &crate::gfx::Camera3D,
    lens_guard: f32,
    wp: [[f32; 3]; 3],
    uv: [[f32; 2]; 3],
    cull: u8,
) -> Option<([TexVert; 4], usize)> {
    let near = -cam.zdist + 0.05;
    let d = [
        cam.depth(wp[0][0], wp[0][1], wp[0][2]),
        cam.depth(wp[1][0], wp[1][1], wp[1][2]),
        cam.depth(wp[2][0], wp[2][1], wp[2][2]),
    ];
    if d[0] <= near && d[1] <= near && d[2] <= near {
        return None;
    }
    // lens guard (tm_lens_guard): a prop triangle with a corner close to the eye is
    // dropped — the camera swinging through a tree or wall otherwise smears huge
    // near-clipped slivers across the view. Large triangles (terrain quads, 16 u)
    // are kept so the ground under a low camera never opens a hole.
    if lens_guard > 0.0 {
        // (was × 6 — a 1.5-unit bubble at the default 0.25 that blanked whole
        // wall/house tiles the camera merely passed close to; × 2 only drops
        // geometry practically touching the lens)
        let g = near + lens_guard * 2.0;
        if d[0] < g || d[1] < g || d[2] < g {
            let e = |i: usize, j: usize| {
                let (dx, dy, dz) = (wp[i][0] - wp[j][0], wp[i][1] - wp[j][1], wp[i][2] - wp[j][2]);
                dx * dx + dy * dy + dz * dz
            };
            if e(0, 1).max(e(1, 2)).max(e(2, 0)) < 36.0 {
                return None;
            }
        }
    }
    let mut cp: [([f32; 3], [f32; 2]); 4] = [([0.0; 3], [0.0; 2]); 4];
    let mut cn = 0usize;
    for i in 0..3 {
        let j = (i + 1) % 3;
        let ain = d[i] > near;
        let bin = d[j] > near;
        if ain && cn < 4 {
            cp[cn] = (wp[i], uv[i]);
            cn += 1;
        }
        if ain != bin && cn < 4 {
            let t = (near - d[i]) / (d[j] - d[i]);
            cp[cn] = (
                [wp[i][0] + (wp[j][0] - wp[i][0]) * t, wp[i][1] + (wp[j][1] - wp[i][1]) * t, wp[i][2] + (wp[j][2] - wp[i][2]) * t],
                [uv[i][0] + (uv[j][0] - uv[i][0]) * t, uv[i][1] + (uv[j][1] - uv[i][1]) * t],
            );
            cn += 1;
        }
    }
    if cn < 3 {
        return None;
    }
    let zd = cam.zdist;
    let mut tv = [TexVert::default(); 4];
    for k in 0..cn {
        let (p, t) = cp[k];
        let (sx, sy, z) = cam.project(p[0], p[1], p[2]);
        let iw = 1.0 / (z + zd).max(1e-4);
        tv[k] = TexVert { x: sx, y: sy, z, iw, uw: t[0] * iw, vw: t[1] * iw, wx: p[0] * iw, wy: p[1] * iw, wz: p[2] * iw };
    }
    if cull != 0 {
        let area = (tv[1].x - tv[0].x) * (tv[2].y - tv[0].y) - (tv[1].y - tv[0].y) * (tv[2].x - tv[0].x);
        if (cull == 1 && area > 0.0) || (cull == 2 && area < 0.0) {
            return None;
        }
    }
    Some((tv, cn))
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
                let cam = gfx.camera.clone();
                let lg = gfx.lens_guard;
                for (gidx, g) in mesh.groups.iter().enumerate() {
                    if gi >= 0 && gidx != gi as usize {
                        continue;
                    }
                    // whole-group view cull (bounding sphere, cached per mesh/group): a
                    // prop or terrain chunk entirely behind the lens or off-screen would
                    // only have every triangle transformed, shaded and then dropped
                    if group_culled(&gfx, mi as usize, gidx, g, &m, sc, o) {
                        continue;
                    }
                    let tri = |t: &[u32; 3]| -> Option<([TexVert; 4], usize, u32)> {
                        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
                        let wp = [
                            add3(mul3s(&m, g.pos[a], sc), o),
                            add3(mul3s(&m, g.pos[b], sc), o),
                            add3(mul3s(&m, g.pos[c], sc), o),
                        ];
                        let uv = [g.uv[a], g.uv[b], g.uv[c]];
                        let (tv, cn) = prep_poly(&cam, lg, wp, uv, cull)?;
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
                        Some((tv, cn, tt))
                    };
                    // big groups (terrain chunks, buildings): transform + shade on all
                    // cores, then queue in mesh order so the output is unchanged
                    #[cfg(not(target_arch = "wasm32"))]
                    if g.idx.len() >= PAR_TRIS {
                        use rayon::prelude::*;
                        let out: Vec<Option<([TexVert; 4], usize, u32)>> = g.idx.par_iter().with_min_len(256).map(tri).collect();
                        for (tv, cn, tt) in out.into_iter().flatten() {
                            count += push_prepped(&mut gfx, &tv, cn, &tex, tt, shade.as_ref());
                        }
                        continue;
                    }
                    for t in &g.idx {
                        if let Some((tv, cn, tt)) = tri(t) {
                            count += push_prepped(&mut gfx, &tv, cn, &tex, tt, shade.as_ref());
                        }
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
fn group_culled(gfx: &crate::gfx::GfxState, mi: usize, gidx: usize, g: &TxGroup, m: &[f32; 9], sc: f32, o: [f32; 3]) -> bool {
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
        return true;
    }
    let wc = add3(mul3s(m, c, sc), o);
    let rw = r * sc.abs() + 0.5;
    let cam = &gfx.camera;
    let (sx, sy, rz) = cam.project(wc[0], wc[1], wc[2]);
    let near = -cam.zdist + 0.05;
    if rz + rw < near {
        return true;
    }
    let dd = rz + cam.zdist;
    if dd <= rw + 0.5 {
        return false; // the sphere reaches the eye plane: perspective size unbounded
    }
    let pr = cam.focal * rw / (dd - rw) * 1.15 + 16.0;
    if cam.hyp_r <= 0.0 {
        let (w, h) = (gfx.width as f32, gfx.height as f32);
        sx + pr < 0.0 || sx - pr > w || sy + pr < 0.0 || sy - pr > h
    } else {
        let rad = ((sx - cam.cx) * (sx - cam.cx) + (sy - cam.cy) * (sy - cam.cy)).sqrt();
        rad - pr > 1.85 * cam.cx
    }
}
