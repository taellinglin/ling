// src/runtime/glb.rs — native glTF 2.0 reader (.glb binary and .gltf with embedded / external buffers) for LingModeler.
// Produces one object per mesh primitive: welded-free positions already in world space, triangles (CCW = outward, like glTF),
// one colour per triangle sampled from the material's base-colour texture (or vertex colours / the colour factor) and UVs.
// Skins and animations are not read here (positions are the bind pose).
use serde_json::Value as J;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct Obj {
    pub name: String,
    pub vx: Vec<f32>,
    pub vy: Vec<f32>,
    pub vz: Vec<f32>,
    pub fi: Vec<u32>,
    pub fcol: Vec<u32>,
    pub uv: Vec<f32>,
}

static LAST_ERR: Mutex<String> = Mutex::new(String::new());
pub fn set_error(e: String) {
    if let Ok(mut g) = LAST_ERR.lock() {
        *g = e;
    }
}
pub fn last_error() -> String {
    LAST_ERR.lock().map(|g| g.clone()).unwrap_or_default()
}

type M4 = [f64; 16]; // column-major, as in glTF

fn ident() -> M4 {
    let mut m = [0.0; 16];
    m[0] = 1.0;
    m[5] = 1.0;
    m[10] = 1.0;
    m[15] = 1.0;
    m
}
fn mul(a: &M4, b: &M4) -> M4 {
    let mut r = [0.0; 16];
    for c in 0..4 {
        for row in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k * 4 + row] * b[c * 4 + k];
            }
            r[c * 4 + row] = s;
        }
    }
    r
}
fn node_matrix(n: &J) -> M4 {
    if let Some(a) = n["matrix"].as_array() {
        if a.len() == 16 {
            let mut m = [0.0; 16];
            for (i, v) in a.iter().enumerate() {
                m[i] = v.as_f64().unwrap_or(0.0);
            }
            return m;
        }
    }
    let arr = |k: &str, d: &[f64]| -> Vec<f64> {
        match n[k].as_array() {
            Some(a) if a.len() == d.len() => a.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect(),
            _ => d.to_vec(),
        }
    };
    let t = arr("translation", &[0.0, 0.0, 0.0]);
    let r = arr("rotation", &[0.0, 0.0, 0.0, 1.0]);
    let s = arr("scale", &[1.0, 1.0, 1.0]);
    let (x, y, z, w) = (r[0], r[1], r[2], r[3]);
    let rot = [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w)],
        [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w)],
        [2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y)],
    ]; // rot[c] = column c
    let mut m = ident();
    for c in 0..3 {
        for row in 0..3 {
            m[c * 4 + row] = rot[c][row] * s[c];
        }
    }
    m[12] = t[0];
    m[13] = t[1];
    m[14] = t[2];
    m
}

fn decode_b64(s: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s.trim()).unwrap_or_default()
}

fn load_uri(uri: &str, dir: &str) -> Vec<u8> {
    if let Some(p) = uri.find("base64,") {
        return decode_b64(&uri[p + 7..]);
    }
    let rel = uri.replace("%20", " ");
    std::fs::read(format!("{}/{}", dir, rel)).unwrap_or_default()
}

fn accessor(root: &J, bufs: &[Vec<u8>], ai: usize) -> Result<(Vec<f64>, usize), String> {
    let acc = &root["accessors"][ai];
    if acc.is_null() {
        return Err(format!("accessor {} missing", ai));
    }
    let count = acc["count"].as_u64().unwrap_or(0) as usize;
    let ctype = acc["componentType"].as_u64().unwrap_or(5126);
    let nc = match acc["type"].as_str().unwrap_or("SCALAR") {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" | "MAT2" => 4,
        "MAT3" => 9,
        "MAT4" => 16,
        _ => 1,
    };
    let norm = acc["normalized"].as_bool().unwrap_or(false);
    let mut out = vec![0.0f64; count * nc];
    if let Some(vi) = acc["bufferView"].as_u64() {
        let view = &root["bufferViews"][vi as usize];
        let bi = view["buffer"].as_u64().unwrap_or(0) as usize;
        let buf = bufs.get(bi).ok_or_else(|| "buffer missing".to_string())?;
        let off = view["byteOffset"].as_u64().unwrap_or(0) as usize + acc["byteOffset"].as_u64().unwrap_or(0) as usize;
        let cs = match ctype {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            _ => 4,
        };
        let stride = view["byteStride"].as_u64().map(|s| s as usize).filter(|&s| s > 0).unwrap_or(cs * nc);
        for i in 0..count {
            for c in 0..nc {
                let p = off + i * stride + c * cs;
                if p + cs > buf.len() {
                    continue;
                }
                let b = &buf[p..p + cs];
                let mut v = match ctype {
                    5126 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
                    5125 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
                    5123 => u16::from_le_bytes([b[0], b[1]]) as f64,
                    5122 => i16::from_le_bytes([b[0], b[1]]) as f64,
                    5121 => b[0] as f64,
                    _ => (b[0] as i8) as f64,
                };
                if norm {
                    v = match ctype {
                        5121 => v / 255.0,
                        5123 => v / 65535.0,
                        5120 => (v / 127.0).max(-1.0),
                        5122 => (v / 32767.0).max(-1.0),
                        _ => v,
                    };
                }
                out[i * nc + c] = v;
            }
        }
    }
    Ok((out, nc))
}

struct Img {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

fn load_image(root: &J, bufs: &[Vec<u8>], dir: &str, tex: usize, cache: &mut HashMap<usize, Option<Img>>) {
    if cache.contains_key(&tex) {
        return;
    }
    let mut img = None;
    if let Some(src) = root["textures"][tex]["source"].as_u64() {
        let im = &root["images"][src as usize];
        let bytes: Vec<u8> = if let Some(vi) = im["bufferView"].as_u64() {
            let v = &root["bufferViews"][vi as usize];
            let bi = v["buffer"].as_u64().unwrap_or(0) as usize;
            let off = v["byteOffset"].as_u64().unwrap_or(0) as usize;
            let len = v["byteLength"].as_u64().unwrap_or(0) as usize;
            bufs.get(bi).and_then(|b| b.get(off..off + len)).map(|s| s.to_vec()).unwrap_or_default()
        } else if let Some(u) = im["uri"].as_str() {
            load_uri(u, dir)
        } else {
            Vec::new()
        };
        if let Ok(d) = image::load_from_memory(&bytes) {
            let r = d.to_rgba8();
            img = Some(Img { w: r.width() as usize, h: r.height() as usize, px: r.into_raw() });
        }
    }
    cache.insert(tex, img);
}

fn pack(r: f64, g: f64, b: f64) -> u32 {
    let q = |v: f64| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    (q(r) << 16) | (q(g) << 8) | q(b)
}

pub fn read(path: &str) -> Result<Vec<Obj>, String> {
    let raw = std::fs::read(path).map_err(|e| format!("cannot read {}: {}", path, e))?;
    let dir = std::path::Path::new(path).parent().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|| ".".into());
    let mut bin_chunk: Vec<u8> = Vec::new();
    let json_text: String;
    if raw.len() >= 20 && &raw[0..4] == b"glTF" {
        let mut o = 12usize;
        let mut js = String::new();
        while o + 8 <= raw.len() {
            let cl = u32::from_le_bytes([raw[o], raw[o + 1], raw[o + 2], raw[o + 3]]) as usize;
            let ct = u32::from_le_bytes([raw[o + 4], raw[o + 5], raw[o + 6], raw[o + 7]]);
            let end = (o + 8 + cl).min(raw.len());
            if ct == 0x4E4F534A {
                js = String::from_utf8_lossy(&raw[o + 8..end]).to_string();
            } else if ct == 0x004E4942 && bin_chunk.is_empty() {
                bin_chunk = raw[o + 8..end].to_vec();
            }
            o += 8 + cl;
        }
        json_text = js;
    } else {
        json_text = String::from_utf8_lossy(&raw).to_string();
    }
    let root: J = serde_json::from_str(&json_text).map_err(|e| format!("not a glTF file: {}", e))?;
    // buffers
    let mut bufs: Vec<Vec<u8>> = Vec::new();
    for b in root["buffers"].as_array().cloned().unwrap_or_default() {
        match b["uri"].as_str() {
            Some(u) => bufs.push(load_uri(u, &dir)),
            None => bufs.push(std::mem::take(&mut bin_chunk)),
        }
    }
    // node hierarchy -> world matrices
    let nodes = root["nodes"].as_array().cloned().unwrap_or_default();
    let nn = nodes.len();
    let mut parent = vec![usize::MAX; nn];
    for (i, n) in nodes.iter().enumerate() {
        for c in n["children"].as_array().cloned().unwrap_or_default() {
            if let Some(ci) = c.as_u64() {
                if (ci as usize) < nn {
                    parent[ci as usize] = i;
                }
            }
        }
    }
    let mut world: Vec<Option<M4>> = vec![None; nn];
    fn world_of(i: usize, nodes: &[J], parent: &[usize], world: &mut Vec<Option<M4>>, depth: usize) -> M4 {
        if let Some(m) = world[i] {
            return m;
        }
        let local = node_matrix(&nodes[i]);
        let m = if parent[i] != usize::MAX && depth < 256 { mul(&world_of(parent[i], nodes, parent, world, depth + 1), &local) } else { local };
        world[i] = Some(m);
        m
    }
    // materials
    struct Mat {
        factor: [f64; 4],
        tex: Option<(usize, usize)>,
        opaque: bool,
        name: String,
    }
    let mut mats: Vec<Mat> = Vec::new();
    for m in root["materials"].as_array().cloned().unwrap_or_default() {
        let pbr = &m["pbrMetallicRoughness"];
        let mut f = [1.0, 1.0, 1.0, 1.0];
        if let Some(a) = pbr["baseColorFactor"].as_array() {
            for (i, v) in a.iter().take(4).enumerate() {
                f[i] = v.as_f64().unwrap_or(1.0);
            }
        }
        let tex = pbr["baseColorTexture"]["index"].as_u64().map(|t| (t as usize, pbr["baseColorTexture"]["texCoord"].as_u64().unwrap_or(0) as usize));
        let name = m["name"].as_str().unwrap_or("material").replace(" (Instance)", "");
        mats.push(Mat { factor: f, tex, opaque: m["alphaMode"].as_str().unwrap_or("OPAQUE") == "OPAQUE", name });
    }
    let mut imgs: HashMap<usize, Option<Img>> = HashMap::new();
    let meshes = root["meshes"].as_array().cloned().unwrap_or_default();
    let mut out: Vec<Obj> = Vec::new();
    for i in 0..nn {
        let node = &nodes[i];
        let mi = match node["mesh"].as_u64() {
            Some(m) => m as usize,
            None => continue,
        };
        let mesh = match meshes.get(mi) {
            Some(m) => m,
            None => continue,
        };
        // a skinned mesh's own node transform is ignored by the spec: its vertices are already in world space
        let wm = if node["skin"].is_null() { world_of(i, &nodes, &parent, &mut world, 0) } else { ident() };
        let det = wm[0] * (wm[5] * wm[10] - wm[6] * wm[9]) - wm[4] * (wm[1] * wm[10] - wm[2] * wm[9]) + wm[8] * (wm[1] * wm[6] - wm[2] * wm[5]);
        let prims = mesh["primitives"].as_array().cloned().unwrap_or_default();
        for prim in &prims {
            if prim["mode"].as_u64().unwrap_or(4) != 4 {
                continue;
            }
            let pos_acc = match prim["attributes"]["POSITION"].as_u64() {
                Some(a) => a as usize,
                None => continue,
            };
            let (pos, _) = accessor(&root, &bufs, pos_acc)?;
            let nv = pos.len() / 3;
            let idx: Vec<usize> = match prim["indices"].as_u64() {
                Some(a) => accessor(&root, &bufs, a as usize)?.0.iter().map(|&v| v as usize).collect(),
                None => (0..nv).collect(),
            };
            let mat = prim["material"].as_u64().and_then(|m| mats.get(m as usize));
            let factor = mat.map(|m| m.factor).unwrap_or([1.0, 1.0, 1.0, 1.0]);
            let opaque = mat.map(|m| m.opaque).unwrap_or(true);
            let texcoord = mat.and_then(|m| m.tex).map(|t| t.1).unwrap_or(0);
            let uv_acc = prim["attributes"][format!("TEXCOORD_{}", texcoord)].as_u64().or_else(|| prim["attributes"]["TEXCOORD_0"].as_u64());
            let uvs = match uv_acc {
                Some(a) => accessor(&root, &bufs, a as usize).map(|r| r.0).unwrap_or_default(),
                None => Vec::new(),
            };
            let (cols, cn) = match prim["attributes"]["COLOR_0"].as_u64() {
                Some(a) => accessor(&root, &bufs, a as usize).unwrap_or((Vec::new(), 4)),
                None => (Vec::new(), 4),
            };
            let has_tex = mat.and_then(|m| m.tex);
            if let Some((t, _)) = has_tex {
                load_image(&root, &bufs, &dir, t, &mut imgs);
            }
            let img = has_tex.and_then(|(t, _)| imgs.get(&t)).and_then(|o| o.as_ref());
            // world-space positions
            let mut wp: Vec<[f32; 3]> = Vec::with_capacity(nv);
            for v in 0..nv {
                let (x, y, z) = (pos[v * 3], pos[v * 3 + 1], pos[v * 3 + 2]);
                wp.push([
                    (wm[0] * x + wm[4] * y + wm[8] * z + wm[12]) as f32,
                    (wm[1] * x + wm[5] * y + wm[9] * z + wm[13]) as f32,
                    (wm[2] * x + wm[6] * y + wm[10] * z + wm[14]) as f32,
                ]);
            }
            let mut remap: HashMap<usize, u32> = HashMap::new();
            let mut ob = Obj { name: String::new(), vx: Vec::new(), vy: Vec::new(), vz: Vec::new(), fi: Vec::new(), fcol: Vec::new(), uv: Vec::new() };
            for t in 0..idx.len() / 3 {
                let (a, b, c) = (idx[t * 3], idx[t * 3 + 1], idx[t * 3 + 2]);
                if a >= nv || b >= nv || c >= nv {
                    continue;
                }
                // colour at the triangle centre
                let mut rgba = factor;
                if let (Some(im), true) = (img, uvs.len() >= nv * 2) {
                    let u = (uvs[a * 2] + uvs[b * 2] + uvs[c * 2]) / 3.0;
                    let v = (uvs[a * 2 + 1] + uvs[b * 2 + 1] + uvs[c * 2 + 1]) / 3.0;
                    let x = (((u - u.floor()) * (im.w as f64 - 1.0)).round() as usize).min(im.w - 1);
                    let y = (((v - v.floor()) * (im.h as f64 - 1.0)).round() as usize).min(im.h - 1);
                    let p = (y * im.w + x) * 4;
                    for k in 0..4 {
                        rgba[k] *= im.px[p + k] as f64 / 255.0;
                    }
                } else if cols.len() >= nv * cn && cn >= 3 {
                    for k in 0..3 {
                        rgba[k] *= (cols[a * cn + k] + cols[b * cn + k] + cols[c * cn + k]) / 3.0;
                    }
                }
                if !opaque && rgba[3] < 0.35 {
                    continue;
                }
                let mut tri = [a, b, c];
                if det < 0.0 {
                    tri.swap(1, 2); // a mirrored node transform flips the winding
                }
                for &v in &tri {
                    let nid = remap.len() as u32;
                    let id = *remap.entry(v).or_insert_with(|| {
                        ob.vx.push(wp[v][0]);
                        ob.vy.push(wp[v][1]);
                        ob.vz.push(wp[v][2]);
                        nid
                    });
                    ob.fi.push(id);
                    if uvs.len() >= nv * 2 {
                        ob.uv.push(uvs[v * 2] as f32);
                        ob.uv.push(1.0 - uvs[v * 2 + 1] as f32);
                    } else {
                        ob.uv.push(0.0);
                        ob.uv.push(0.0);
                    }
                }
                ob.fcol.push(pack(rgba[0], rgba[1], rgba[2]));
            }
            if ob.fcol.is_empty() {
                continue;
            }
            let nname = node["name"].as_str().or_else(|| mesh["name"].as_str()).unwrap_or("Mesh");
            ob.name = if prims.len() > 1 { mat.map(|m| m.name.clone()).unwrap_or_else(|| nname.to_string()) } else { nname.to_string() };
            out.push(ob);
        }
    }
    if out.is_empty() {
        return Err("the file has no triangle meshes".into());
    }
    Ok(out)
}
