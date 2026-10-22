//! Mesh writers: Wavefront OBJ, binary STL and binary glTF (GLB) with an optional
//! skeleton and envelope skin weights.

use crate::humanoid::Humanoid;
use crate::mesh::Mesh;
use serde_json::json;
use std::fmt::Write as _;

pub fn obj_text(m: &Mesh) -> String {
    let mut s = String::with_capacity(m.pos.len() * 24);
    s.push_str("# ling-body\no body\n");
    for p in m.pos.as_chunks::<3>().0 {
        let _ = writeln!(s, "v {:.6} {:.6} {:.6}", p[0], p[1], p[2]);
    }
    let has_uv = m.uv.len() / 2 == m.vert_count();
    let has_n = m.nrm.len() == m.pos.len();
    if has_uv {
        for t in m.uv.as_chunks::<2>().0 {
            let _ = writeln!(s, "vt {:.6} {:.6}", t[0], t[1]);
        }
    }
    if has_n {
        for n in m.nrm.as_chunks::<3>().0 {
            let _ = writeln!(s, "vn {:.5} {:.5} {:.5}", n[0], n[1], n[2]);
        }
    }
    for t in m.idx.as_chunks::<3>().0 {
        let r = |i: u32| {
            let i = i + 1;
            match (has_uv, has_n) {
                (true, true) => format!("{i}/{i}/{i}"),
                (true, false) => format!("{i}/{i}"),
                (false, true) => format!("{i}//{i}"),
                (false, false) => format!("{i}"),
            }
        };
        let _ = writeln!(s, "f {} {} {}", r(t[0]), r(t[1]), r(t[2]));
    }
    s
}

pub fn stl_bytes(m: &Mesh) -> Vec<u8> {
    let mut out = vec![0u8; 80];
    out[..9].copy_from_slice(b"ling-body");
    out.extend_from_slice(&(m.tri_count() as u32).to_le_bytes());
    for t in m.idx.as_chunks::<3>().0 {
        let p = |i: u32| {
            let i = i as usize * 3;
            [m.pos[i], m.pos[i + 1], m.pos[i + 2]]
        };
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let mut n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-20);
        n.iter_mut().for_each(|x| *x /= l);
        for f in n.iter().chain(a.iter()).chain(b.iter()).chain(c.iter()) {
            out.extend_from_slice(&f.to_le_bytes());
        }
        out.extend_from_slice(&[0, 0]);
    }
    out
}

fn dist_to_segment(p: [f32; 3], a: [f32; 3], b: [f32; 3]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
    let l2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    let t = if l2 > 1e-12 { ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let d = [ap[0] - ab[0] * t, ap[1] - ab[1] * t, ap[2] - ab[2] * t];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Four strongest joint influences per vertex. Every non-root joint owns the bone
/// from its parent to itself and that bone is driven by the parent.
pub fn skin_weights(m: &Mesh, rig: &Humanoid) -> (Vec<[u8; 4]>, Vec<[f32; 4]>) {
    let bones: Vec<(usize, [f32; 3], [f32; 3])> = rig
        .joints
        .iter()
        .filter_map(|j| j.parent.map(|p| (p, rig.joints[p].pos, j.pos)))
        .collect();
    let mut joints = Vec::with_capacity(m.vert_count());
    let mut weights = Vec::with_capacity(m.vert_count());
    for p in m.pos.as_chunks::<3>().0 {
        let p = [p[0], p[1], p[2]];
        let mut d: Vec<(f32, usize)> =
            bones.iter().map(|(drv, a, b)| (dist_to_segment(p, *a, *b), *drv)).collect();
        d.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut ji = [0u8; 4];
        let mut w = [0.0f32; 4];
        let mut used = 0;
        for (dist, drv) in d {
            if used == 4 {
                break;
            }
            if ji[..used].contains(&(drv as u8)) {
                continue;
            }
            ji[used] = drv as u8;
            w[used] = 1.0 / (dist * dist * dist * dist + 1e-6);
            used += 1;
        }
        let sum: f32 = w.iter().sum();
        w.iter_mut().for_each(|x| *x /= sum);
        joints.push(ji);
        weights.push(w);
    }
    (joints, weights)
}

fn pad4(v: &mut Vec<u8>, fill: u8) {
    while !v.len().is_multiple_of(4) {
        v.push(fill);
    }
}

pub fn glb_bytes(m: &Mesh, rig: Option<&Humanoid>) -> Vec<u8> {
    let mut bin: Vec<u8> = Vec::new();
    let mut views: Vec<serde_json::Value> = Vec::new();
    let mut accessors: Vec<serde_json::Value> = Vec::new();
    let mut add = |bin: &mut Vec<u8>, bytes: &[u8], target: Option<u32>, comp: u32, count: usize, ty: &str,
                   bounds: Option<(Vec<f32>, Vec<f32>)>, normalized: bool| {
        pad4(bin, 0);
        let off = bin.len();
        bin.extend_from_slice(bytes);
        let mut view = json!({ "buffer": 0, "byteOffset": off, "byteLength": bytes.len() });
        if let Some(t) = target {
            view["target"] = json!(t);
        }
        views.push(view);
        let mut acc = json!({ "bufferView": views.len() - 1, "componentType": comp, "count": count, "type": ty });
        if let Some((lo, hi)) = bounds {
            acc["min"] = json!(lo);
            acc["max"] = json!(hi);
        }
        if normalized {
            acc["normalized"] = json!(true);
        }
        accessors.push(acc);
        accessors.len() - 1
    };

    let f32_bytes = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let (lo, hi) = m.bounds().unwrap_or(([0.0; 3], [0.0; 3]));
    let mut attrs = serde_json::Map::new();
    let a = add(&mut bin, &f32_bytes(&m.pos), Some(34962), 5126, m.vert_count(), "VEC3", Some((lo.to_vec(), hi.to_vec())), false);
    attrs.insert("POSITION".into(), json!(a));
    if m.nrm.len() == m.pos.len() {
        let a = add(&mut bin, &f32_bytes(&m.nrm), Some(34962), 5126, m.vert_count(), "VEC3", None, false);
        attrs.insert("NORMAL".into(), json!(a));
    }
    if m.uv.len() / 2 == m.vert_count() {
        let a = add(&mut bin, &f32_bytes(&m.uv), Some(34962), 5126, m.vert_count(), "VEC2", None, false);
        attrs.insert("TEXCOORD_0".into(), json!(a));
    }
    if let Some(rig) = rig {
        let (ji, wi) = skin_weights(m, rig);
        let jb: Vec<u8> = ji.iter().flatten().copied().collect();
        let wb: Vec<u8> = wi.iter().flatten().flat_map(|x| x.to_le_bytes()).collect();
        let a = add(&mut bin, &jb, Some(34962), 5121, m.vert_count(), "VEC4", None, false);
        attrs.insert("JOINTS_0".into(), json!(a));
        let a = add(&mut bin, &wb, Some(34962), 5126, m.vert_count(), "VEC4", None, false);
        attrs.insert("WEIGHTS_0".into(), json!(a));
    }
    let ib: Vec<u8> = m.idx.iter().flat_map(|i| i.to_le_bytes()).collect();
    let ia = add(&mut bin, &ib, Some(34963), 5125, m.idx.len(), "SCALAR", None, false);

    let mut nodes = vec![json!({ "name": "body", "mesh": 0 })];
    let mut skins = Vec::new();
    let mut scene_nodes = vec![0usize];
    if let Some(rig) = rig {
        let base = nodes.len();
        for (i, j) in rig.joints.iter().enumerate() {
            let local = match j.parent {
                Some(p) => [0, 1, 2].map(|k| j.pos[k] - rig.joints[p].pos[k]),
                None => j.pos,
            };
            let children: Vec<usize> =
                rig.joints.iter().enumerate().filter(|(_, c)| c.parent == Some(i)).map(|(c, _)| base + c).collect();
            let mut n = json!({ "name": j.name, "translation": local });
            if !children.is_empty() {
                n["children"] = json!(children);
            }
            nodes.push(n);
        }
        let mut ibm: Vec<f32> = Vec::new();
        for j in &rig.joints {
            ibm.extend_from_slice(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
            ibm.extend_from_slice(&[-j.pos[0], -j.pos[1], -j.pos[2], 1.0]);
        }
        let ia_ibm = add(&mut bin, &f32_bytes(&ibm), None, 5126, rig.joints.len(), "MAT4", None, false);
        skins.push(json!({
            "name": "t_pose", "inverseBindMatrices": ia_ibm, "skeleton": base,
            "joints": (0..rig.joints.len()).map(|i| base + i).collect::<Vec<_>>(),
        }));
        nodes[0]["skin"] = json!(0);
        scene_nodes.push(base);
    }

    let mut doc = json!({
        "asset": { "version": "2.0", "generator": "ling-body" },
        "scene": 0,
        "scenes": [{ "nodes": scene_nodes }],
        "nodes": nodes,
        "meshes": [{ "name": "body", "primitives": [{ "attributes": attrs, "indices": ia, "material": 0 }] }],
        "materials": [{ "name": "skin", "pbrMetallicRoughness": { "baseColorFactor": [0.93, 0.76, 0.66, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.65 } }],
        "buffers": [{ "byteLength": bin.len() }],
        "bufferViews": views,
        "accessors": accessors,
    });
    if !skins.is_empty() {
        doc["skins"] = json!(skins);
    }
    let mut js = doc.to_string().into_bytes();
    pad4(&mut js, b' ');
    pad4(&mut bin, 0);
    let total = 12 + 8 + js.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(js.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&js);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

pub fn write(path: &str, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = std::path::Path::new(path).parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{path}: {e}"))?;
        }
    }
    std::fs::write(path, bytes).map_err(|e| format!("{path}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tetra() -> Mesh {
        let mut m = Mesh {
            pos: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            nrm: vec![],
            uv: vec![],
            idx: vec![0, 2, 1, 0, 1, 3, 1, 2, 3, 0, 3, 2],
        };
        m.compute_normals();
        m.compute_uvs();
        m
    }

    #[test]
    fn obj_and_stl_sizes() {
        let m = tetra();
        assert_eq!(obj_text(&m).lines().filter(|l| l.starts_with("f ")).count(), 4);
        assert_eq!(stl_bytes(&m).len(), 84 + 4 * 50);
    }

    #[test]
    fn glb_reloads_through_the_gltf_loader() {
        let rig = Humanoid::new(1.0, 1.0, 1.0, 1.0);
        let (lo, hi) = rig.bounds();
        let grid = crate::Grid::fit(lo, hi, 0.04, 3);
        let phi = rig.fill(&grid);
        let mesh = crate::mesher::build(&phi, &grid, 0.0, 1, 0.0);
        let bytes = glb_bytes(&mesh, Some(&rig));
        let path = std::env::temp_dir().join("ling_body_test.glb");
        std::fs::write(&path, &bytes).unwrap();
        let g = crate::gltf::load(path.to_str().unwrap()).unwrap();
        assert_eq!(g.meshes[0].tris.len(), mesh.idx.len() * 3);
        assert_eq!(g.nodes.len(), 1 + rig.joints.len());
        let model = crate::model::Model::load(path.to_str().unwrap()).unwrap();
        assert_eq!(model.tri_count(), mesh.tri_count());
        let (lo2, hi2) = model.tris().bounds().unwrap();
        let (lo1, hi1) = mesh.bounds().unwrap();
        for k in 0..3 {
            assert!((lo1[k] - lo2[k]).abs() < 1e-4 && (hi1[k] - hi2[k]).abs() < 1e-4);
        }
        let _ = std::fs::remove_file(path);
    }
}
