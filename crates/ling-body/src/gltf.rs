//! Minimal glTF 2.0 / GLB reader: geometry and node transforms only. Materials,
//! textures and animation are ignored, so files that require texture extensions
//! still load. Draco-compressed meshes are rejected with a clear error.

use glam::{Mat4, Quat, Vec3};
use serde_json::Value;

pub struct GltfMesh {
    pub name: String,
    /// Triangle corner positions, nine floats per triangle, in mesh space.
    pub tris: Vec<f32>,
}

pub struct GltfNode {
    pub name: String,
    pub local: Mat4,
    pub mesh: Option<usize>,
    pub children: Vec<usize>,
}

pub struct Gltf {
    pub meshes: Vec<GltfMesh>,
    pub nodes: Vec<GltfNode>,
    pub roots: Vec<usize>,
}

fn base64(s: &str) -> Result<Vec<u8>, String> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        let v = val(c).ok_or("invalid base64 in glTF buffer")?;
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

fn load_buffers(doc: &Value, glb_bin: Option<Vec<u8>>, dir: &std::path::Path) -> Result<Vec<Vec<u8>>, String> {
    let mut out = Vec::new();
    let mut glb_bin = glb_bin;
    for b in doc["buffers"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        match b["uri"].as_str() {
            None => out.push(glb_bin.take().ok_or("buffer without data")?),
            Some(uri) if uri.starts_with("data:") => {
                let payload = uri.split_once(',').ok_or("bad data URI")?.1;
                out.push(base64(payload)?);
            },
            Some(uri) => {
                let p = dir.join(uri.replace("%20", " "));
                out.push(std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?);
            },
        }
    }
    Ok(out)
}

struct Reader<'a> {
    doc: &'a Value,
    buffers: &'a [Vec<u8>],
}

impl Reader<'_> {
    fn view(&self, accessor: &Value) -> Result<(&[u8], usize, Option<usize>), String> {
        let view = accessor["bufferView"].as_u64().ok_or("accessor without bufferView")?;
        let v = &self.doc["bufferViews"][view as usize];
        let buf = self.buffers.get(v["buffer"].as_u64().unwrap_or(0) as usize).ok_or("buffer index")?;
        let base = v["byteOffset"].as_u64().unwrap_or(0) as usize + accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
        Ok((buf, base, v["byteStride"].as_u64().map(|s| s as usize)))
    }

    /// Components of an accessor as `f32`, normalising integer types when flagged.
    fn floats(&self, idx: usize) -> Result<Vec<f32>, String> {
        let a = &self.doc["accessors"][idx];
        let count = a["count"].as_u64().ok_or("accessor count")? as usize;
        let width = match a["type"].as_str().unwrap_or("SCALAR") {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            _ => return Err("unsupported accessor type".into()),
        };
        let comp = a["componentType"].as_u64().ok_or("componentType")?;
        let size = match comp {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => return Err("unsupported component type".into()),
        };
        let norm = a["normalized"].as_bool().unwrap_or(false);
        if a["bufferView"].is_null() {
            return Ok(vec![0.0; count * width]);
        }
        let (buf, base, stride) = self.view(a)?;
        let stride = stride.unwrap_or(size * width);
        let mut out = Vec::with_capacity(count * width);
        for i in 0..count {
            for c in 0..width {
                let o = base + i * stride + c * size;
                let b = buf.get(o..o + size).ok_or("accessor out of range")?;
                out.push(match comp {
                    5126 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                    5125 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32,
                    5123 => {
                        let v = u16::from_le_bytes([b[0], b[1]]) as f32;
                        if norm { v / 65535.0 } else { v }
                    },
                    5122 => {
                        let v = i16::from_le_bytes([b[0], b[1]]) as f32;
                        if norm { (v / 32767.0).max(-1.0) } else { v }
                    },
                    5121 => {
                        let v = b[0] as f32;
                        if norm { v / 255.0 } else { v }
                    },
                    _ => {
                        let v = b[0] as i8 as f32;
                        if norm { (v / 127.0).max(-1.0) } else { v }
                    },
                });
            }
        }
        Ok(out)
    }

    fn indices(&self, idx: usize) -> Result<Vec<u32>, String> {
        let a = &self.doc["accessors"][idx];
        let count = a["count"].as_u64().ok_or("accessor count")? as usize;
        let size = match a["componentType"].as_u64().ok_or("componentType")? {
            5121 => 1,
            5123 => 2,
            5125 => 4,
            _ => return Err("unsupported index type".into()),
        };
        let (buf, base, _) = self.view(a)?;
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let o = base + i * size;
            let b = buf.get(o..o + size).ok_or("index accessor out of range")?;
            out.push(match size {
                1 => b[0] as u32,
                2 => u16::from_le_bytes([b[0], b[1]]) as u32,
                _ => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            });
        }
        Ok(out)
    }
}

fn node_matrix(n: &Value) -> Mat4 {
    if let Some(m) = n["matrix"].as_array() {
        let v: Vec<f32> = m.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect();
        if let Ok(a) = <[f32; 16]>::try_from(v) {
            return Mat4::from_cols_array(&a);
        }
    }
    let vec = |k: &str, d: &[f32]| -> Vec<f32> {
        match n[k].as_array() {
            Some(a) if a.len() == d.len() => a.iter().zip(d).map(|(x, d)| x.as_f64().map_or(*d, |f| f as f32)).collect(),
            _ => d.to_vec(),
        }
    };
    let t = vec("translation", &[0.0; 3]);
    let s = vec("scale", &[1.0; 3]);
    let r = vec("rotation", &[0.0, 0.0, 0.0, 1.0]);
    let q = Quat::from_xyzw(r[0], r[1], r[2], r[3]).normalize();
    Mat4::from_scale_rotation_translation(Vec3::new(s[0], s[1], s[2]), q, Vec3::new(t[0], t[1], t[2]))
}

fn split_glb(bytes: &[u8]) -> Result<(&[u8], Option<Vec<u8>>), String> {
    let mut pos = 12;
    let (mut json, mut bin) = (None, None);
    while pos + 8 <= bytes.len() {
        let len = u32::from_le_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]) as usize;
        let kind = &bytes[pos + 4..pos + 8];
        let data = bytes.get(pos + 8..pos + 8 + len).ok_or("truncated GLB chunk")?;
        if kind == b"JSON" {
            json = Some(data);
        } else if kind == b"BIN\0" {
            bin = Some(data.to_vec());
        }
        pos += 8 + len;
    }
    Ok((json.ok_or("GLB without JSON chunk")?, bin))
}

fn read_primitives(r: &Reader, mesh: &Value, path: &str) -> Result<Vec<f32>, String> {
    let mut tris: Vec<f32> = Vec::new();
    for prim in mesh["primitives"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        if prim["extensions"]["KHR_draco_mesh_compression"].is_object() {
            return Err(format!("{path}: Draco-compressed meshes are not supported"));
        }
        let Some(pa) = prim["attributes"]["POSITION"].as_u64() else { continue };
        let pos = r.floats(pa as usize)?;
        let idx = match prim["indices"].as_u64() {
            Some(i) => r.indices(i as usize)?,
            None => (0..(pos.len() / 3) as u32).collect(),
        };
        let mut tri = |a: u32, b: u32, c: u32| -> Result<(), String> {
            for i in [a, b, c] {
                let p = pos.get(i as usize * 3..i as usize * 3 + 3).ok_or("index out of range")?;
                tris.extend_from_slice(p);
            }
            Ok(())
        };
        match prim["mode"].as_u64().unwrap_or(4) {
            4 => idx.as_chunks::<3>().0.iter().try_for_each(|t| tri(t[0], t[1], t[2]))?,
            5 => (2..idx.len()).try_for_each(|i| {
                if i % 2 == 0 { tri(idx[i - 2], idx[i - 1], idx[i]) } else { tri(idx[i - 1], idx[i - 2], idx[i]) }
            })?,
            6 => (2..idx.len()).try_for_each(|i| tri(idx[0], idx[i - 1], idx[i]))?,
            _ => {},
        }
    }
    Ok(tris)
}

pub fn load(path: &str) -> Result<Gltf, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let (json_bytes, bin) = if bytes.len() > 20 && &bytes[0..4] == b"glTF" { split_glb(&bytes)? } else { (&bytes[..], None) };
    let doc: Value = serde_json::from_slice(json_bytes).map_err(|e| format!("{path}: {e}"))?;
    let dir = std::path::Path::new(path).parent().unwrap_or(std::path::Path::new("."));
    let buffers = load_buffers(&doc, bin, dir)?;
    let r = Reader { doc: &doc, buffers: &buffers };

    let mut meshes = Vec::new();
    for m in doc["meshes"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        meshes.push(GltfMesh { name: m["name"].as_str().unwrap_or("").to_string(), tris: read_primitives(&r, m, path)? });
    }

    let ids = |v: &Value| -> Vec<usize> {
        v.as_array().map(|a| a.iter().filter_map(|x| x.as_u64().map(|i| i as usize)).collect()).unwrap_or_default()
    };
    let nodes: Vec<GltfNode> = doc["nodes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .map(|n| GltfNode {
            name: n["name"].as_str().unwrap_or("").to_string(),
            local: node_matrix(n),
            mesh: n["mesh"].as_u64().map(|m| m as usize),
            children: ids(&n["children"]),
        })
        .collect();
    let mut roots = ids(&doc["scenes"][doc["scene"].as_u64().unwrap_or(0) as usize]["nodes"]);
    if roots.is_empty() {
        let mut has_parent = vec![false; nodes.len()];
        for c in nodes.iter().flat_map(|n| &n.children) {
            if let Some(h) = has_parent.get_mut(*c) {
                *h = true;
            }
        }
        roots = (0..nodes.len()).filter(|&i| !has_parent[i]).collect();
    }
    Ok(Gltf { meshes, nodes, roots })
}

impl Gltf {
    /// World matrix of every node reachable from the scene roots.
    pub fn world_matrices(&self) -> Vec<Mat4> {
        let mut world = vec![Mat4::IDENTITY; self.nodes.len()];
        let mut seen = vec![false; self.nodes.len()];
        let mut stack: Vec<usize> = Vec::new();
        for &r in &self.roots {
            if r < self.nodes.len() && !seen[r] {
                world[r] = self.nodes[r].local;
                seen[r] = true;
                stack.push(r);
            }
        }
        while let Some(i) = stack.pop() {
            for &c in &self.nodes[i].children {
                if c < self.nodes.len() && !seen[c] {
                    world[c] = world[i] * self.nodes[c].local;
                    seen[c] = true;
                    stack.push(c);
                }
            }
        }
        world
    }
}
