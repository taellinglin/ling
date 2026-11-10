//! glTF 2.0 binary (.glb) models with skins + skeletal animation, no_std: the
//! kernel side of the host's `gltf_*` builtins (`ling-physics/src/gltf.rs` +
//! `ling/src/runtime/mod.rs`). Reads the GLB container, its JSON (serde_json,
//! alloc) and the accessors it uses -- float / u8 / u16 / u32 components --
//! and decodes embedded PNG base-colour textures. Skinning is the host's:
//! node TRS overridden by the clip's channels at `time`, world transforms down
//! the hierarchy, joint = world * inverse-bind, 4-weight linear blend; models
//! with no skin can be `autorig`ged (the host's 20-bone Biped envelope rig).
//! Drawing lives in g3d (`gltf_anim_draw_tex` / `gltf_pose_draw_tex`).

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use glam::{EulerRot, Mat4, Quat, Vec3, Vec4};
use serde_json::Value as J;

#[derive(Clone)]
pub struct Vert {
    pub pos: Vec3,
    pub uv: [f32; 2],
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

pub struct GMesh {
    pub name: String,
    pub verts: Vec<Vert>,
    pub indices: Vec<u32>,
    /// Index into `Model::textures`.
    pub base_tex: Option<usize>,
}

/// A decoded base-colour texture, `0xAARRGGBB`, row-major.
pub struct GImage {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u32>,
}

struct Node {
    trs: (Vec3, Quat, Vec3),
    children: Vec<usize>,
    name: String,
}

struct Joint {
    node: usize,
    inverse_bind: Mat4,
}

#[derive(Clone, Copy, PartialEq)]
enum Target {
    T,
    R,
    S,
}

struct Channel {
    node: usize,
    target: Target,
    times: Vec<f32>,
    values: Vec<Vec4>,
    step: bool,
}

impl Channel {
    fn sample(&self, t: f32) -> Vec4 {
        if self.times.is_empty() {
            return Vec4::ZERO;
        }
        let last = *self.times.last().unwrap_or(&1.0);
        let t = if last > 0.0 { t - libm::floorf(t / last) * last } else { 0.0 };
        let idx = self.times.partition_point(|&s| s <= t).saturating_sub(1).min(self.times.len() - 1);
        let next = (idx + 1).min(self.times.len() - 1);
        if idx == next || self.step {
            return self.values[idx];
        }
        let (lo, hi) = (self.times[idx], self.times[next]);
        let f = ((t - lo) / (hi - lo)).clamp(0.0, 1.0);
        self.values[idx].lerp(self.values[next], f)
    }
}

pub struct Anim {
    pub name: String,
    channels: Vec<Channel>,
    pub duration: f32,
}

struct Bone {
    head: Vec3,
    tail: Vec3,
    parent: i32,
}

pub struct Model {
    pub meshes: Vec<GMesh>,
    nodes: Vec<Node>,
    skin: Vec<Joint>,
    pub anims: Vec<Anim>,
    bones: Vec<Bone>,
    pub textures: Vec<GImage>,
    pub hidden: Vec<bool>,
    /// per-mesh multiplicative tint (0xRRGGBB, white = none), set by `gltf_mesh_tint`
    pub tints: Vec<u32>,
}

// ── GLB parsing ──────────────────────────────────────────────────────────────

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn ju(v: &J, k: &str) -> Option<usize> {
    v.get(k)?.as_u64().map(|x| x as usize)
}

fn jf(v: &J) -> f32 {
    v.as_f64().unwrap_or(0.0) as f32
}

struct Doc<'a> {
    j: J,
    bin: &'a [u8],
}

impl<'a> Doc<'a> {
    fn arr(&self, k: &str) -> &[J] {
        self.j.get(k).and_then(|v| v.as_array()).map(|v| &v[..]).unwrap_or(&[])
    }

    /// The bytes of buffer view `bv` (the GLB's own BIN chunk only).
    fn view(&self, bv: usize) -> Option<(&'a [u8], usize)> {
        let v = self.arr("bufferViews").get(bv)?;
        let off = ju(v, "byteOffset").unwrap_or(0);
        let len = ju(v, "byteLength")?;
        let stride = ju(v, "byteStride").unwrap_or(0);
        Some((self.bin.get(off..off + len)?, stride))
    }

    /// Accessor `ai` as rows of f32 (normalized ints scale to 0..1, plain ints stay integral).
    fn read(&self, ai: usize) -> Option<(Vec<f32>, usize)> {
        let a = self.arr("accessors").get(ai)?;
        let count = ju(a, "count")?;
        let comps = match a.get("type")?.as_str()? {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            "MAT4" => 16,
            _ => return None,
        };
        let ct = ju(a, "componentType")?;
        let norm = a.get("normalized").and_then(|v| v.as_bool()).unwrap_or(false);
        let csize = match ct {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => return None,
        };
        let mut out = vec![0.0f32; count * comps];
        let Some(bv) = ju(a, "bufferView") else { return Some((out, comps)) };
        let (bytes, stride) = self.view(bv)?;
        let base = ju(a, "byteOffset").unwrap_or(0);
        let stride = if stride == 0 { comps * csize } else { stride };
        for i in 0..count {
            for c in 0..comps {
                let o = base + i * stride + c * csize;
                if o + csize > bytes.len() {
                    return None;
                }
                let v = match ct {
                    5126 => f32::from_bits(le32(bytes, o)),
                    5125 => le32(bytes, o) as f32,
                    5123 => {
                        let x = u16::from_le_bytes([bytes[o], bytes[o + 1]]) as f32;
                        if norm { x / 65535.0 } else { x }
                    },
                    5122 => {
                        let x = i16::from_le_bytes([bytes[o], bytes[o + 1]]) as f32;
                        if norm { (x / 32767.0).max(-1.0) } else { x }
                    },
                    5121 => {
                        let x = bytes[o] as f32;
                        if norm { x / 255.0 } else { x }
                    },
                    _ => {
                        let x = bytes[o] as i8 as f32;
                        if norm { (x / 127.0).max(-1.0) } else { x }
                    },
                };
                out[i * comps + c] = v;
            }
        }
        Some((out, comps))
    }

    /// Indices as u32 straight from the view (exact for any size).
    fn read_indices(&self, ai: usize) -> Option<Vec<u32>> {
        let a = self.arr("accessors").get(ai)?;
        let count = ju(a, "count")?;
        let ct = ju(a, "componentType")?;
        let (bytes, stride) = self.view(ju(a, "bufferView")?)?;
        let base = ju(a, "byteOffset").unwrap_or(0);
        let csize = match ct {
            5121 => 1,
            5123 => 2,
            _ => 4,
        };
        let stride = if stride == 0 { csize } else { stride };
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let o = base + i * stride;
            if o + csize > bytes.len() {
                return None;
            }
            out.push(match csize {
                1 => bytes[o] as u32,
                2 => u16::from_le_bytes([bytes[o], bytes[o + 1]]) as u32,
                _ => le32(bytes, o),
            });
        }
        Some(out)
    }

    fn image(&self, ii: usize) -> Option<GImage> {
        let im = self.arr("images").get(ii)?;
        let (bytes, _) = self.view(ju(im, "bufferView")?)?;
        let img = crate::drivers::image::decode_png_rgba(bytes)?;
        Some(GImage { w: img.w, h: img.h, px: img.px })
    }
}

fn mat4_of(v: &[f32]) -> Mat4 {
    let mut a = [0.0f32; 16];
    a.copy_from_slice(&v[..16]);
    Mat4::from_cols_array(&a)
}

/// Parse a .glb. `None` for anything that isn't a binary glTF 2.0 with its
/// data in the GLB's own BIN chunk.
pub fn load(data: &[u8]) -> Option<Model> {
    if data.len() < 20 || &data[0..4] != b"glTF" {
        return None;
    }
    let jlen = le32(data, 12) as usize;
    if &data[16..20] != b"JSON" || 20 + jlen > data.len() {
        return None;
    }
    let j: J = serde_json::from_slice(&data[20..20 + jlen]).ok()?;
    let mut o = 20 + jlen;
    let mut bin: &[u8] = &[];
    while o + 8 <= data.len() {
        let len = le32(data, o) as usize;
        let kind = &data[o + 4..o + 8];
        let body = data.get(o + 8..o + 8 + len)?;
        if kind == b"BIN\0" {
            bin = body;
        }
        o += 8 + len;
    }
    let doc = Doc { j, bin };

    // textures: material -> base-colour image, decoded once per image
    let mut textures = Vec::new();
    let mut img_tex: Vec<(usize, usize)> = Vec::new(); // (image index, textures index)
    let mut mat_tex: Vec<Option<usize>> = Vec::new();
    for m in doc.arr("materials") {
        let ti = m.get("pbrMetallicRoughness").and_then(|p| p.get("baseColorTexture")).and_then(|t| ju(t, "index"));
        let src = ti.and_then(|t| doc.arr("textures").get(t)).and_then(|t| ju(t, "source"));
        let slot = match src {
            Some(s) => match img_tex.iter().find(|(i, _)| *i == s) {
                Some(&(_, k)) => Some(k),
                None => doc.image(s).map(|img| {
                    textures.push(img);
                    img_tex.push((s, textures.len() - 1));
                    textures.len() - 1
                }),
            },
            None => None,
        };
        mat_tex.push(slot);
    }

    // meshes: one GMesh per primitive
    let mut meshes = Vec::new();
    for m in doc.arr("meshes") {
        let name = String::from(m.get("name").and_then(|v| v.as_str()).unwrap_or(""));
        for p in m.get("primitives").and_then(|v| v.as_array()).map(|v| &v[..]).unwrap_or(&[]) {
            let Some(attrs) = p.get("attributes") else { continue };
            let Some((pos, _)) = ju(attrs, "POSITION").and_then(|a| doc.read(a)) else { continue };
            let nv = pos.len() / 3;
            let uv = ju(attrs, "TEXCOORD_0").and_then(|a| doc.read(a)).map(|r| r.0);
            let jn = ju(attrs, "JOINTS_0").and_then(|a| doc.read(a)).map(|r| r.0);
            let wt = ju(attrs, "WEIGHTS_0").and_then(|a| doc.read(a)).map(|r| r.0);
            let mut verts = Vec::with_capacity(nv);
            for i in 0..nv {
                let g = |v: &Option<Vec<f32>>, k: usize, d: f32| v.as_ref().and_then(|x| x.get(k).copied()).unwrap_or(d);
                verts.push(Vert {
                    pos: Vec3::new(pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]),
                    uv: [g(&uv, i * 2, 0.0), g(&uv, i * 2 + 1, 0.0)],
                    joints: [g(&jn, i * 4, 0.0) as u16, g(&jn, i * 4 + 1, 0.0) as u16, g(&jn, i * 4 + 2, 0.0) as u16, g(&jn, i * 4 + 3, 0.0) as u16],
                    weights: [g(&wt, i * 4, 1.0), g(&wt, i * 4 + 1, 0.0), g(&wt, i * 4 + 2, 0.0), g(&wt, i * 4 + 3, 0.0)],
                });
            }
            let indices = match ju(p, "indices") {
                Some(a) => doc.read_indices(a).unwrap_or_default(),
                None => (0..nv as u32).collect(),
            };
            let base_tex = ju(p, "material").and_then(|mi| mat_tex.get(mi).copied().flatten());
            meshes.push(GMesh { name: name.clone(), verts, indices, base_tex });
        }
    }

    // nodes
    let mut nodes = Vec::new();
    for n in doc.arr("nodes") {
        let trs = if let Some(m) = n.get("matrix").and_then(|v| v.as_array()) {
            let v: Vec<f32> = m.iter().map(jf).collect();
            if v.len() >= 16 {
                let (s, r, t) = mat4_of(&v).to_scale_rotation_translation();
                (t, r, s)
            } else {
                (Vec3::ZERO, Quat::IDENTITY, Vec3::ONE)
            }
        } else {
            let a3 = |k: &str, d: Vec3| n.get(k).and_then(|v| v.as_array()).filter(|v| v.len() >= 3).map(|v| Vec3::new(jf(&v[0]), jf(&v[1]), jf(&v[2]))).unwrap_or(d);
            let r = n.get("rotation").and_then(|v| v.as_array()).filter(|v| v.len() >= 4).map(|v| Quat::from_xyzw(jf(&v[0]), jf(&v[1]), jf(&v[2]), jf(&v[3])).normalize()).unwrap_or(Quat::IDENTITY);
            (a3("translation", Vec3::ZERO), r, a3("scale", Vec3::ONE))
        };
        let children = n.get("children").and_then(|v| v.as_array()).map(|v| v.iter().filter_map(|c| c.as_u64().map(|x| x as usize)).collect()).unwrap_or_default();
        let name = String::from(n.get("name").and_then(|v| v.as_str()).unwrap_or(""));
        nodes.push(Node { trs, children, name });
    }

    // the first skin
    let mut skin = Vec::new();
    if let Some(s) = doc.arr("skins").first() {
        let ibm = ju(s, "inverseBindMatrices").and_then(|a| doc.read(a)).map(|r| r.0).unwrap_or_default();
        if let Some(js) = s.get("joints").and_then(|v| v.as_array()) {
            for (i, jn) in js.iter().enumerate() {
                let m = if ibm.len() >= (i + 1) * 16 { mat4_of(&ibm[i * 16..]) } else { Mat4::IDENTITY };
                skin.push(Joint { node: jn.as_u64().unwrap_or(0) as usize, inverse_bind: m });
            }
        }
    }

    // animations
    let mut anims = Vec::new();
    for a in doc.arr("animations") {
        let samplers = a.get("samplers").and_then(|v| v.as_array()).map(|v| &v[..]).unwrap_or(&[]);
        let mut channels = Vec::new();
        let mut duration = 0.0f32;
        for ch in a.get("channels").and_then(|v| v.as_array()).map(|v| &v[..]).unwrap_or(&[]) {
            let Some(t) = ch.get("target") else { continue };
            let target = match t.get("path").and_then(|v| v.as_str()) {
                Some("translation") => Target::T,
                Some("rotation") => Target::R,
                Some("scale") => Target::S,
                _ => continue,
            };
            let Some(node) = ju(t, "node") else { continue };
            let Some(sm) = ju(ch, "sampler").and_then(|i| samplers.get(i)) else { continue };
            let Some((times, _)) = ju(sm, "input").and_then(|i| doc.read(i)) else { continue };
            let Some((vals, comps)) = ju(sm, "output").and_then(|i| doc.read(i)) else { continue };
            let interp = sm.get("interpolation").and_then(|v| v.as_str()).unwrap_or("LINEAR");
            // cubic spline keeps (in-tangent, value, out-tangent) per key: take the value
            let (per, off) = if interp == "CUBICSPLINE" { (comps * 3, comps) } else { (comps, 0) };
            let mut values = Vec::with_capacity(times.len());
            for k in 0..times.len() {
                let b = k * per + off;
                let g = |c: usize| vals.get(b + c).copied().unwrap_or(0.0);
                values.push(if comps == 4 { Vec4::new(g(0), g(1), g(2), g(3)) } else { Vec4::new(g(0), g(1), g(2), 0.0) });
            }
            if let Some(&last) = times.last() {
                duration = duration.max(last);
            }
            channels.push(Channel { node, target, times, values, step: interp == "STEP" });
        }
        anims.push(Anim { name: String::from(a.get("name").and_then(|v| v.as_str()).unwrap_or("")), channels, duration });
    }

    let n = meshes.len();
    Some(Model { meshes, nodes, skin, anims, bones: Vec::new(), textures, hidden: vec![false; n], tints: vec![0xFF_FFFF; n] })
}

// ── skinning ─────────────────────────────────────────────────────────────────

/// The `*root*` turn of a pose-layer list about its `*pivot*` (identity without a `*root*`).
fn root_mat(pose: &[(String, [f32; 3])]) -> Mat4 {
    let mut pivot = Vec3::ZERO;
    let mut m = Mat4::IDENTITY;
    for (name, e) in pose {
        if name == "*pivot*" {
            pivot = Vec3::new(e[0], e[1], e[2]);
        }
    }
    for (name, e) in pose {
        if name == "*root*" {
            m = Mat4::from_translation(pivot) * Mat4::from_quat(Quat::from_euler(EulerRot::XYZ, e[0], e[1], e[2])) * Mat4::from_translation(-pivot);
        }
    }
    m
}

fn point_seg_dist(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let l2 = ab.length_squared();
    let t = if l2 > 1e-9 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + ab * t)).length()
}

impl Model {
    /// Per-mesh skinned positions for clip `anim` at `time` (hidden meshes empty).
    /// Per-mesh skinned positions for clip `anim` at `time` (hidden meshes empty).
    pub fn skin_animated(&self, anim: usize, time: f32) -> Vec<Vec<Vec3>> {
        if self.nodes.is_empty() || self.skin.is_empty() {
            return self.bind_positions();
        }
        let n = self.nodes.len();
        let world = self.world_mats(anim, time);
        let joint: Vec<Mat4> = self.skin.iter().map(|j| if j.node < n { world[j.node] * j.inverse_bind } else { Mat4::IDENTITY }).collect();
        self.skin_with(&joint)
    }

    /// Skin THIS model with the pose of `src`'s clip `anim` at `time`, joints
    /// matched to `src` nodes by name. Equipment baked on the same skeleton but
    /// without animations follows the body exactly; joints `src` lacks keep
    /// their own bind pose.
    pub fn skin_animated_from(&self, src: &Model, anim: usize, time: f32) -> Vec<Vec<Vec3>> {
        if self.nodes.is_empty() || self.skin.is_empty() {
            return self.bind_positions();
        }
        let n = self.nodes.len();
        let sw = src.world_mats(anim, time);
        let own = self.world_mats(usize::MAX, 0.0);
        let joint: Vec<Mat4> = self
            .skin
            .iter()
            .map(|j| {
                if j.node >= n {
                    return Mat4::IDENTITY;
                }
                let nm = &self.nodes[j.node].name;
                let w = if nm.is_empty() { None } else { src.nodes.iter().position(|x| x.name == *nm).map(|k| sw[k]) };
                w.unwrap_or(own[j.node]) * j.inverse_bind
            })
            .collect();
        self.skin_with(&joint)
    }

    /// `skin_animated` / `skin_animated_from` plus procedural pose layers (the host's
    /// `gltf_anim_draw_tex` arg 11): each `(bone, euler XYZ)` post-multiplies that node's local
    /// rotation; `*root*` turns the whole skeleton in model space about the `*pivot*` point.
    pub fn skin_animated_posed(&self, src: Option<&Model>, anim: usize, time: f32, pose: &[(String, [f32; 3])]) -> Vec<Vec<Vec3>> {
        if self.nodes.is_empty() || self.skin.is_empty() {
            // an unskinned model still takes the whole-model turn
            let rm = root_mat(pose);
            return self.bind_positions().into_iter().map(|m| m.into_iter().map(|p| rm.transform_point3(p)).collect()).collect();
        }
        let n = self.nodes.len();
        let joint: Vec<Mat4> = match src {
            None => {
                let world = self.world_mats_posed(anim, time, pose);
                self.skin.iter().map(|j| if j.node < n { world[j.node] * j.inverse_bind } else { Mat4::IDENTITY }).collect()
            },
            Some(sm) => {
                let sw = sm.world_mats_posed(anim, time, pose);
                let own = self.world_mats(usize::MAX, 0.0);
                self.skin
                    .iter()
                    .map(|j| {
                        if j.node >= n {
                            return Mat4::IDENTITY;
                        }
                        let nm = &self.nodes[j.node].name;
                        let w = if nm.is_empty() { None } else { sm.nodes.iter().position(|x| x.name == *nm).map(|k| sw[k]) };
                        w.unwrap_or(own[j.node]) * j.inverse_bind
                    })
                    .collect()
            },
        };
        self.skin_with(&joint)
    }

    fn bind_positions(&self) -> Vec<Vec<Vec3>> {
        self.meshes.iter().enumerate().map(|(mi, m)| if self.hidden[mi] { Vec::new() } else { m.verts.iter().map(|v| v.pos).collect() }).collect()
    }

    /// World matrix of every node posed by clip `anim` at `time` (base TRS
    /// where the clip has no channel; an out-of-range clip = bind pose).
    fn world_mats(&self, anim: usize, time: f32) -> Vec<Mat4> {
        self.world_mats_posed(anim, time, &[])
    }

    fn world_mats_posed(&self, anim: usize, time: f32, pose: &[(String, [f32; 3])]) -> Vec<Mat4> {
        let n = self.nodes.len();
        let mut t: Vec<Vec3> = self.nodes.iter().map(|x| x.trs.0).collect();
        let mut r: Vec<Quat> = self.nodes.iter().map(|x| x.trs.1).collect();
        let mut s: Vec<Vec3> = self.nodes.iter().map(|x| x.trs.2).collect();
        if let Some(a) = self.anims.get(anim) {
            for ch in &a.channels {
                if ch.node >= n {
                    continue;
                }
                let v = ch.sample(time);
                match ch.target {
                    Target::T => t[ch.node] = Vec3::new(v.x, v.y, v.z),
                    Target::R => r[ch.node] = Quat::from_xyzw(v.x, v.y, v.z, v.w).normalize(),
                    Target::S => s[ch.node] = Vec3::new(v.x, v.y, v.z),
                }
            }
        }
        for (name, e) in pose {
            if name != "*root*" && name != "*pivot*" {
                if let Some(i) = self.nodes.iter().position(|nd| nd.name == *name) {
                    r[i] = (r[i] * Quat::from_euler(EulerRot::XYZ, e[0], e[1], e[2])).normalize();
                }
            }
        }
        let root_m = root_mat(pose);
        let local: Vec<Mat4> = (0..n).map(|i| Mat4::from_scale_rotation_translation(s[i], r[i], t[i])).collect();
        let mut parent = vec![usize::MAX; n];
        for (pi, node) in self.nodes.iter().enumerate() {
            for &c in &node.children {
                if c < n {
                    parent[c] = pi;
                }
            }
        }
        let mut world = vec![Mat4::IDENTITY; n];
        let mut done = vec![false; n];
        for _ in 0..n {
            let mut progressed = false;
            for i in 0..n {
                if done[i] {
                    continue;
                }
                let p = parent[i];
                if p == usize::MAX {
                    world[i] = root_m * local[i];
                    done[i] = true;
                    progressed = true;
                } else if done[p] {
                    world[i] = world[p] * local[i];
                    done[i] = true;
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }
        world
    }

    fn skin_with(&self, joint: &[Mat4]) -> Vec<Vec<Vec3>> {
        self.meshes
            .iter()
            .enumerate()
            .map(|(mi, m)| {
                if self.hidden[mi] {
                    return Vec::new();
                }
                m.verts
                    .iter()
                    .map(|v| {
                        let p = v.pos.extend(1.0);
                        let mut acc = Vec4::ZERO;
                        let mut ws = 0.0f32;
                        for k in 0..4 {
                            let w = v.weights[k];
                            let ji = v.joints[k] as usize;
                            if w > 0.0 && ji < joint.len() {
                                acc += joint[ji] * p * w;
                                ws += w;
                            }
                        }
                        if ws > 1e-4 { (acc / ws).truncate() } else { v.pos }
                    })
                    .collect()
            })
            .collect()
    }

    /// The host's procedural 20-bone humanoid ("Bip01 ...") with 4-bone
    /// envelope weights, for a model that shipped without a skeleton.
    pub fn autorig(&mut self) -> usize {
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for m in &self.meshes {
            for v in &m.verts {
                lo = lo.min(v.pos);
                hi = hi.max(v.pos);
            }
        }
        if !lo.is_finite() || !hi.is_finite() {
            return 0;
        }
        let h = (hi.y - lo.y).max(1e-3);
        let (cx, cz) = ((lo.x + hi.x) * 0.5, (lo.z + hi.z) * 0.5);
        let aw = (hi.x - lo.x).max(1e-3) * 0.5;
        let ad = (hi.z - lo.z).max(1e-3) * 0.5;
        let y = |f: f32| lo.y + f * h;
        let b = |hx: f32, hy: f32, hz: f32, tx: f32, ty: f32, tz: f32, p: i32| Bone { head: Vec3::new(hx, hy, hz), tail: Vec3::new(tx, ty, tz), parent: p };
        self.bones = vec![
            b(cx, y(0.50), cz, cx, y(0.50), cz, -1),
            b(cx, y(0.50), cz, cx, y(0.58), cz, 0),
            b(cx, y(0.58), cz, cx, y(0.68), cz, 1),
            b(cx, y(0.68), cz, cx, y(0.78), cz, 2),
            b(cx, y(0.78), cz, cx, y(0.86), cz, 3),
            b(cx, y(0.86), cz, cx, y(1.00), cz, 4),
            b(cx, y(0.80), cz, cx + aw * 0.25, y(0.79), cz, 3),
            b(cx + aw * 0.25, y(0.79), cz, cx + aw * 0.58, y(0.76), cz, 6),
            b(cx + aw * 0.58, y(0.76), cz, cx + aw * 0.88, y(0.68), cz, 7),
            b(cx + aw * 0.88, y(0.68), cz, cx + aw * 1.02, y(0.62), cz, 8),
            b(cx, y(0.80), cz, cx - aw * 0.25, y(0.79), cz, 3),
            b(cx - aw * 0.25, y(0.79), cz, cx - aw * 0.58, y(0.76), cz, 10),
            b(cx - aw * 0.58, y(0.76), cz, cx - aw * 0.88, y(0.68), cz, 11),
            b(cx - aw * 0.88, y(0.68), cz, cx - aw * 1.02, y(0.62), cz, 12),
            b(cx + aw * 0.18, y(0.50), cz, cx + aw * 0.18, y(0.27), cz, 1),
            b(cx + aw * 0.18, y(0.27), cz, cx + aw * 0.18, y(0.06), cz, 14),
            b(cx + aw * 0.18, y(0.06), cz, cx + aw * 0.18, y(0.0), cz + ad * 0.55, 15),
            b(cx - aw * 0.18, y(0.50), cz, cx - aw * 0.18, y(0.27), cz, 1),
            b(cx - aw * 0.18, y(0.27), cz, cx - aw * 0.18, y(0.06), cz, 17),
            b(cx - aw * 0.18, y(0.06), cz, cx - aw * 0.18, y(0.0), cz + ad * 0.55, 18),
        ];
        for m in &mut self.meshes {
            for v in &mut m.verts {
                let mut best: [(f32, usize); 4] = [(f32::INFINITY, 0); 4];
                for (i, bn) in self.bones.iter().enumerate().skip(1) {
                    let d = point_seg_dist(v.pos, bn.head, bn.tail);
                    if d < best[0].0 {
                        best = [(d, i), best[0], best[1], best[2]];
                    } else if d < best[1].0 {
                        best = [best[0], (d, i), best[1], best[2]];
                    } else if d < best[2].0 {
                        best = [best[0], best[1], (d, i), best[2]];
                    } else if d < best[3].0 {
                        best[3] = (d, i);
                    }
                }
                let w: [f32; 4] = core::array::from_fn(|k| 1.0 / (best[k].0 * best[k].0 + 1e-4));
                let s = w[0] + w[1] + w[2] + w[3];
                v.joints = core::array::from_fn(|k| best[k].1 as u16);
                v.weights = core::array::from_fn(|k| w[k] / s);
            }
        }
        self.bones.len()
    }

    /// Skin by a flat XYZ-euler pose (3 per auto-rig bone); rest positions when not rigged.
    pub fn skin_local(&self, euler: &[f32]) -> Vec<Vec<Vec3>> {
        if self.bones.is_empty() {
            return self.meshes.iter().enumerate().map(|(mi, m)| if self.hidden[mi] { Vec::new() } else { m.verts.iter().map(|v| v.pos).collect() }).collect();
        }
        let nb = self.bones.len();
        let mut world = vec![Mat4::IDENTITY; nb];
        for i in 0..nb {
            let bn = &self.bones[i];
            let e = |k: usize| euler.get(i * 3 + k).copied().unwrap_or(0.0);
            let r = Quat::from_euler(EulerRot::XYZ, e(0), e(1), e(2));
            let ph = if bn.parent < 0 { Vec3::ZERO } else { self.bones[bn.parent as usize].head };
            let local = Mat4::from_translation(bn.head - ph) * Mat4::from_quat(r);
            world[i] = if bn.parent < 0 { local } else { world[bn.parent as usize] * local };
        }
        for (i, w) in world.iter_mut().enumerate() {
            *w *= Mat4::from_translation(-self.bones[i].head);
        }
        self.meshes
            .iter()
            .enumerate()
            .map(|(mi, m)| {
                if self.hidden[mi] {
                    return Vec::new();
                }
                m.verts
                    .iter()
                    .map(|v| {
                        let p = v.pos.extend(1.0);
                        let mut sp = Vec3::ZERO;
                        for k in 0..4 {
                            let w = v.weights[k];
                            if w != 0.0 {
                                let j = (v.joints[k] as usize).min(nb - 1);
                                sp += (world[j] * p).truncate() * w;
                            }
                        }
                        sp
                    })
                    .collect()
            })
            .collect()
    }
}

// ── the model table ──────────────────────────────────────────────────────────

static mut MODELS: Vec<Model> = Vec::new();

pub fn models() -> &'static mut Vec<Model> {
    unsafe { &mut *(&raw mut MODELS) }
}

pub fn get(h: f64) -> Option<&'static mut Model> {
    if h < 0.0 {
        return None;
    }
    models().get_mut(h as usize)
}
