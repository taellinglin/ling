//! Body mesh import: glTF/GLB (rest pose, node transforms baked), Ling `.lmesh` and OBJ.
//! Winding and handedness do not matter downstream because bodies are voxelised.

use crate::mesh::Tris;
use crate::gltf;
use glam::Vec3;

pub struct Part {
    pub name: String,
    pub tris: Tris,
    pub enabled: bool,
}

pub struct Model {
    pub path: String,
    pub parts: Vec<Part>,
    /// Skeleton joints found in the file as `(name, world position)`.
    pub joints: Vec<(String, [f32; 3])>,
}

const ACCESSORY_WORDS: &[&str] = &[
    "cube", "rose", "flower", "circle", "star", "wing", "ribbon", "plane", "ground", "shadow", "arena",
    "light", "camera",
];

fn is_accessory(name: &str) -> bool {
    let n = name.to_lowercase();
    ACCESSORY_WORDS.iter().any(|w| n.contains(w))
}

impl Model {
    pub fn load(path: &str) -> Result<Model, String> {
        let lower = path.to_lowercase();
        let mut m = if lower.ends_with(".glb") || lower.ends_with(".gltf") {
            load_gltf(path)?
        } else if lower.ends_with(".lmesh") {
            load_lmesh(path)?
        } else if lower.ends_with(".obj") {
            load_obj(path)?
        } else {
            return Err(format!("unsupported mesh format: {path}"));
        };
        if m.parts.iter().all(|p| p.tris.is_empty()) {
            return Err(format!("{path}: no triangles"));
        }
        m.path = path.to_string();
        Ok(m)
    }

    /// Union of the enabled parts.
    pub fn tris(&self) -> Tris {
        let mut t = Tris::default();
        for p in self.parts.iter().filter(|p| p.enabled) {
            t.extend(&p.tris);
        }
        t
    }

    pub fn tri_count(&self) -> usize {
        self.parts.iter().filter(|p| p.enabled).map(|p| p.tris.len()).sum()
    }
}

/// Scales so the height is `height` (when given), centres x/z and rests the feet on y = 0.
/// Joint positions follow the same transform.
pub fn normalize(tris: &mut Tris, joints: &mut [(String, [f32; 3])], height: Option<f32>) -> f32 {
    let Some((lo, hi)) = tris.bounds() else { return 1.0 };
    let h = (hi[1] - lo[1]).max(1e-6);
    let scale = height.map(|t| t / h).unwrap_or(1.0);
    let off = [-(lo[0] + hi[0]) * 0.5 * scale, -lo[1] * scale, -(lo[2] + hi[2]) * 0.5 * scale];
    tris.transform(scale, off);
    for (_, p) in joints.iter_mut() {
        for k in 0..3 {
            p[k] = p[k] * scale + off[k];
        }
    }
    scale
}

fn load_gltf(path: &str) -> Result<Model, String> {
    let g = gltf::load(path)?;
    let world = g.world_matrices();
    let mut parts = Vec::new();
    let mut joints = Vec::new();
    for (i, node) in g.nodes.iter().enumerate() {
        let w = world[i];
        if node.name.starts_with("J_Bip") || node.name.to_lowercase().contains("bip01") {
            let p = w.transform_point3(Vec3::ZERO);
            joints.push((node.name.clone(), [p.x, p.y, p.z]));
        }
        let Some(mesh) = node.mesh.and_then(|mi| g.meshes.get(mi)) else { continue };
        let mut tris = Tris::default();
        for c in mesh.tris.as_chunks::<3>().0 {
            let p = w.transform_point3(Vec3::new(c[0], c[1], c[2]));
            tris.v.extend_from_slice(&[p.x, p.y, p.z]);
        }
        let name = if node.name.is_empty() { mesh.name.clone() } else { node.name.clone() };
        let enabled = !is_accessory(&name) && !is_accessory(&mesh.name);
        parts.push(Part { name, tris, enabled });
    }
    Ok(Model { path: String::new(), parts, joints })
}

/// `LMSH` v1: header (magic, version, height f32, triangle count u32), then per
/// triangle nine f32 and three colour bytes. The engine stores Y pointing down.
fn load_lmesh(path: &str) -> Result<Model, String> {
    let b = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if b.len() < 16 || &b[0..4] != b"LMSH" {
        return Err(format!("{path}: not an LMSH file"));
    }
    let ntri = u32::from_le_bytes([b[12], b[13], b[14], b[15]]) as usize;
    if b.len() < 16 + ntri * 39 {
        return Err(format!("{path}: truncated"));
    }
    let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let mut tris = Tris::default();
    for t in 0..ntri {
        let o = 16 + t * 39;
        let p = |k: usize| [f(o + k * 12), -f(o + k * 12 + 4), f(o + k * 12 + 8)];
        tris.push(p(0), p(1), p(2));
    }
    Ok(Model { path: String::new(), parts: vec![Part { name: "body".into(), tris, enabled: true }], joints: Vec::new() })
}

fn load_obj(path: &str) -> Result<Model, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let mut verts: Vec<[f32; 3]> = Vec::new();
    let mut tris = Tris::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let c: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
                if c.len() >= 3 {
                    verts.push([c[0], c[1], c[2]]);
                }
            },
            Some("f") => {
                let ids: Vec<usize> = it
                    .filter_map(|s| s.split('/').next()?.parse::<i64>().ok())
                    .map(|i| if i < 0 { (verts.len() as i64 + i) as usize } else { (i - 1) as usize })
                    .collect();
                for k in 1..ids.len().saturating_sub(1) {
                    if let (Some(a), Some(b), Some(c)) = (verts.get(ids[0]), verts.get(ids[k]), verts.get(ids[k + 1])) {
                        tris.push(*a, *b, *c);
                    }
                }
            },
            _ => {},
        }
    }
    Ok(Model { path: String::new(), parts: vec![Part { name: "obj".into(), tris, enabled: true }], joints: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obj_import_triangulates_quads() {
        let dir = std::env::temp_dir().join("ling_body_obj_test.obj");
        std::fs::write(&dir, "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n").unwrap();
        let m = Model::load(dir.to_str().unwrap()).unwrap();
        assert_eq!(m.tri_count(), 2);
        let _ = std::fs::remove_file(dir);
    }

    #[test]
    fn normalize_rests_feet_and_scales() {
        let mut t = Tris::default();
        t.push([2.0, 1.0, 5.0], [3.0, 1.0, 5.0], [2.5, 3.0, 5.5]);
        let mut j: Vec<(String, [f32; 3])> = Vec::new();
        normalize(&mut t, &mut j, Some(1.0));
        let (lo, hi) = t.bounds().unwrap();
        assert!(lo[1].abs() < 1e-5 && (hi[1] - 1.0).abs() < 1e-5);
        assert!(((lo[0] + hi[0]) * 0.5).abs() < 1e-5);
    }
}
