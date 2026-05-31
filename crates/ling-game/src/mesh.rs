//! Procedural mesh generation for ling-game.
//!
//! All functions return a `ProceduralMesh` with interleaved vertex data
//! and a flat index list for use with any triangle-based renderer.

use std::f32::consts::{PI, TAU};

#[derive(Clone, Debug, Default)]
pub struct ProceduralMesh {
    /// Flat: [x,y,z, nx,ny,nz, u,v]  per vertex
    pub vertices: Vec<[f32; 8]>,
    pub indices:  Vec<u32>,
}

impl ProceduralMesh {
    pub fn new() -> Self { Self::default() }

    fn push_vert(&mut self, pos: [f32; 3], nor: [f32; 3], uv: [f32; 2]) -> u32 {
        let i = self.vertices.len() as u32;
        self.vertices.push([pos[0], pos[1], pos[2], nor[0], nor[1], nor[2], uv[0], uv[1]]);
        i
    }

    fn push_tri(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend_from_slice(&[a, b, c]);
    }

    fn push_quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.push_tri(a, b, c);
        self.push_tri(a, c, d);
    }

    /// Positions extracted from vertices.
    pub fn positions(&self) -> Vec<[f32; 3]> {
        self.vertices.iter().map(|v| [v[0], v[1], v[2]]).collect()
    }

    /// Normals extracted.
    pub fn normals(&self) -> Vec<[f32; 3]> {
        self.vertices.iter().map(|v| [v[3], v[4], v[5]]).collect()
    }
}

/// UV sphere.
pub fn sphere(radius: f32, rings: u32, sectors: u32) -> ProceduralMesh {
    let mut m = ProceduralMesh::new();
    for r in 0..=rings {
        let phi = PI * r as f32 / rings as f32;
        for s in 0..=sectors {
            let theta = TAU * s as f32 / sectors as f32;
            let x = phi.sin() * theta.cos();
            let y = phi.cos();
            let z = phi.sin() * theta.sin();
            let u = s as f32 / sectors as f32;
            let v = r as f32 / rings as f32;
            m.push_vert([x * radius, y * radius, z * radius], [x, y, z], [u, v]);
        }
    }
    for r in 0..rings {
        for s in 0..sectors {
            let row   = sectors + 1;
            let a = r * row + s;
            let b = a + 1;
            let c = a + row;
            let d = c + 1;
            m.push_quad(a, b, d, c);
        }
    }
    m
}

/// Torus.
pub fn torus(major_r: f32, minor_r: f32, major_segs: u32, minor_segs: u32) -> ProceduralMesh {
    let mut m = ProceduralMesh::new();
    for i in 0..=major_segs {
        let phi = TAU * i as f32 / major_segs as f32;
        let cx = phi.cos() * major_r;
        let cz = phi.sin() * major_r;
        for j in 0..=minor_segs {
            let theta = TAU * j as f32 / minor_segs as f32;
            let rx = theta.cos() * phi.cos();
            let ry = theta.sin();
            let rz = theta.cos() * phi.sin();
            let px = cx + rx * minor_r;
            let py = ry * minor_r;
            let pz = cz + rz * minor_r;
            let u = i as f32 / major_segs as f32;
            let v = j as f32 / minor_segs as f32;
            m.push_vert([px, py, pz], [rx, ry, rz], [u, v]);
        }
    }
    let row = minor_segs + 1;
    for i in 0..major_segs {
        for j in 0..minor_segs {
            let a = i * row + j;
            let b = a + 1;
            let c = (i + 1) * row + j;
            let d = c + 1;
            m.push_quad(a, b, d, c);
        }
    }
    m
}

/// Cylinder (open top and bottom).
pub fn cylinder(radius: f32, height: f32, segments: u32) -> ProceduralMesh {
    let mut m = ProceduralMesh::new();
    for ring in 0..=1u32 {
        let y = ring as f32 * height - height * 0.5;
        for i in 0..=segments {
            let theta = TAU * i as f32 / segments as f32;
            let nx = theta.cos(); let nz = theta.sin();
            let u = i as f32 / segments as f32;
            let v = ring as f32;
            m.push_vert([nx * radius, y, nz * radius], [nx, 0.0, nz], [u, v]);
        }
    }
    let row = segments + 1;
    for i in 0..segments {
        let a = i; let b = a + 1; let c = a + row; let d = c + 1;
        m.push_quad(a, c, d, b);
    }
    m
}

/// Flat grid (ground plane).
pub fn grid(width: f32, depth: f32, cols: u32, rows: u32) -> ProceduralMesh {
    let mut m = ProceduralMesh::new();
    for row in 0..=rows {
        for col in 0..=cols {
            let x = col as f32 / cols as f32 * width - width * 0.5;
            let z = row as f32 / rows as f32 * depth - depth * 0.5;
            let u = col as f32 / cols as f32;
            let v = row as f32 / rows as f32;
            m.push_vert([x, 0.0, z], [0.0, 1.0, 0.0], [u, v]);
        }
    }
    for row in 0..rows {
        for col in 0..cols {
            let stride = cols + 1;
            let a = row * stride + col;
            let b = a + 1;
            let c = a + stride;
            let d = c + 1;
            m.push_quad(a, b, d, c);
        }
    }
    m
}

/// Box / cube with independent face normals.
pub fn cube(size: f32) -> ProceduralMesh {
    let h = size * 0.5;
    let mut m = ProceduralMesh::new();
    // 6 faces: +X -X +Y -Y +Z -Z
    let faces: [([f32; 3], [f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([h,h,-h], [h,-h,-h], [h,-h,h], [h,h,h]),   // +X
        ([-h,h,h], [-h,-h,h], [-h,-h,-h], [-h,h,-h]), // -X
        ([-h,h,-h], [-h,h,h], [h,h,h], [h,h,-h]),    // +Y
        ([-h,-h,h], [-h,-h,-h], [h,-h,-h], [h,-h,h]),// -Y
        ([-h,h,h], [-h,-h,h], [h,-h,h], [h,h,h]),    // +Z
        ([h,h,-h], [h,-h,-h], [-h,-h,-h], [-h,h,-h]),// -Z
    ];
    let norms = [[1.0,0.0,0.0],[-1.0,0.0,0.0],[0.0,1.0,0.0],[0.0,-1.0,0.0],[0.0,0.0,1.0],[0.0,0.0,-1.0]];
    let uvs: [[f32;2]; 4] = [[0.0,0.0],[0.0,1.0],[1.0,1.0],[1.0,0.0]];
    for (face, n) in faces.iter().zip(norms.iter()) {
        let verts = [face.0, face.1, face.2, face.3];
        let base = m.vertices.len() as u32;
        for (v, uv) in verts.iter().zip(uvs.iter()) { m.push_vert(*v, *n, *uv); }
        m.push_quad(base, base+1, base+2, base+3);
    }
    m
}

/// Möbius strip (single-sided surface).
pub fn mobius(radius: f32, width: f32, segments: u32, twists: u32) -> ProceduralMesh {
    let mut m = ProceduralMesh::new();
    let half_w = width * 0.5;
    for i in 0..=segments {
        let t = i as f32 / segments as f32;
        let phi   = TAU * t;
        let twist = PI * twists as f32 * t;
        for j in 0..=1 {
            let s  = j as f32 * 2.0 - 1.0; // -1 or +1
            let dx = twist.cos() * s * half_w;
            let dy = twist.sin() * s * half_w;
            let px = (radius + dx) * phi.cos();
            let py = dy;
            let pz = (radius + dx) * phi.sin();
            let nx = phi.cos(); let ny = 0.0; let nz = phi.sin();
            m.push_vert([px, py, pz], [nx, ny, nz], [t, j as f32]);
        }
    }
    for i in 0..segments {
        let a = i * 2; let b = a + 1; let c = a + 2; let d = a + 3;
        m.push_quad(a, b, d, c);
    }
    m
}

/// Icosphere (subdivision-based sphere).
pub fn icosphere(radius: f32, subdivisions: u32) -> ProceduralMesh {
    // Start with icosahedron vertices.
    let phi = (1.0 + 5.0f32.sqrt()) / 2.0;
    let base_verts: Vec<[f32; 3]> = {
        let mut v = vec![
            [-1.0, phi, 0.0], [1.0, phi, 0.0], [-1.0,-phi, 0.0], [1.0,-phi, 0.0],
            [0.0,-1.0, phi], [0.0, 1.0, phi], [0.0,-1.0,-phi], [0.0, 1.0,-phi],
            [phi, 0.0,-1.0], [phi, 0.0, 1.0], [-phi, 0.0,-1.0], [-phi, 0.0, 1.0],
        ];
        for p in &mut v {
            let len = (p[0]*p[0] + p[1]*p[1] + p[2]*p[2]).sqrt();
            p[0] /= len; p[1] /= len; p[2] /= len;
        }
        v
    };
    let base_faces: Vec<[usize; 3]> = vec![
        [0,11,5],[0,5,1],[0,1,7],[0,7,10],[0,10,11],
        [1,5,9],[5,11,4],[11,10,2],[10,7,6],[7,1,8],
        [3,9,4],[3,4,2],[3,2,6],[3,6,8],[3,8,9],
        [4,9,5],[2,4,11],[6,2,10],[8,6,7],[9,8,1],
    ];

    let mut verts = base_verts;
    let mut faces = base_faces;

    for _ in 0..subdivisions {
        let mut new_faces = Vec::new();
        let mut mid_cache: std::collections::HashMap<(usize, usize), usize> = std::collections::HashMap::new();
        let mut mid_point = |a: usize, b: usize, v: &mut Vec<[f32; 3]>| -> usize {
            let key = if a < b { (a, b) } else { (b, a) };
            if let Some(&idx) = mid_cache.get(&key) { return idx; }
            let va = v[a]; let vb = v[b];
            let mut mp = [(va[0]+vb[0])*0.5, (va[1]+vb[1])*0.5, (va[2]+vb[2])*0.5];
            let len = (mp[0]*mp[0]+mp[1]*mp[1]+mp[2]*mp[2]).sqrt();
            mp[0]/=len; mp[1]/=len; mp[2]/=len;
            let idx = v.len();
            v.push(mp);
            mid_cache.insert(key, idx);
            idx
        };
        for face in &faces {
            let [a,b,c] = *face;
            let ab = mid_point(a, b, &mut verts);
            let bc = mid_point(b, c, &mut verts);
            let ca = mid_point(c, a, &mut verts);
            new_faces.extend_from_slice(&[[a,ab,ca],[b,bc,ab],[c,ca,bc],[ab,bc,ca]]);
        }
        faces = new_faces;
    }

    let mut m = ProceduralMesh::new();
    for face in &faces {
        for &vi in face {
            let n = verts[vi];
            m.push_vert([n[0]*radius, n[1]*radius, n[2]*radius], n,
                        [(n[0].atan2(n[2]) / TAU + 0.5), (n[1].asin() / PI + 0.5)]);
        }
    }
    // Flat index list (already in order).
    m.indices = (0..m.vertices.len() as u32).collect();
    m
}

/// Hyperbolic surface — parametric patch in the Poincaré half-plane model.
/// Good for making the "inside of a hyperbolic sphere" tile pattern visible.
pub fn hyperbolic_patch(u_segs: u32, v_segs: u32, curvature: f32) -> ProceduralMesh {
    let mut m = ProceduralMesh::new();
    let k = curvature.abs().sqrt();
    for vi in 0..=v_segs {
        let v = vi as f32 / v_segs as f32 * PI;
        for ui in 0..=u_segs {
            let u = ui as f32 / u_segs as f32 * TAU;
            // Pseudosphere (tractroid) — an embedding of part of the hyperbolic plane.
            let x = (1.0 / k) * u.cos() / v.cosh();
            let y = (1.0 / k) * (v - v.tanh());
            let z = (1.0 / k) * u.sin() / v.cosh();
            let nx = u.cos() * v.cosh();
            let ny = -v.sinh() / v.cosh().powi(2);
            let nz = u.sin() * v.cosh();
            let len = (nx*nx + ny*ny + nz*nz).sqrt().max(1e-6);
            m.push_vert([x, y, z], [nx/len, ny/len, nz/len], [ui as f32 / u_segs as f32, vi as f32 / v_segs as f32]);
        }
    }
    let row = u_segs + 1;
    for vi in 0..v_segs {
        for ui in 0..u_segs {
            let a = vi * row + ui; let b = a + 1; let c = a + row; let d = c + 1;
            m.push_quad(a, b, d, c);
        }
    }
    m
}

/// Pentagon (for hyperbolic room floors).
pub fn pentagon(side_len: f32, flipped: bool) -> ProceduralMesh {
    let mut m = ProceduralMesh::new();
    let r = side_len / (2.0 * (PI / 5.0).sin());
    let verts: Vec<[f32; 3]> = (0..5).map(|i| {
        let a = TAU * i as f32 / 5.0 - PI * 0.5;
        [r * a.cos(), 0.0, r * a.sin()]
    }).collect();
    let n = if flipped { [0.0, -1.0, 0.0] } else { [0.0, 1.0, 0.0] };
    // Fan triangulation from centre.
    let centre = m.push_vert([0.0, 0.0, 0.0], n, [0.5, 0.5]);
    let ids: Vec<u32> = verts.iter().enumerate().map(|(i, &v)| {
        let a = i as f32 / 5.0;
        m.push_vert(v, n, [a.cos() * 0.5 + 0.5, a.sin() * 0.5 + 0.5])
    }).collect();
    for i in 0..5 {
        let a = ids[i]; let b = ids[(i + 1) % 5];
        if flipped { m.push_tri(centre, b, a); } else { m.push_tri(centre, a, b); }
    }
    m
}

/// Merge two meshes together.
pub fn merge(a: &ProceduralMesh, b: &ProceduralMesh) -> ProceduralMesh {
    let offset = a.vertices.len() as u32;
    let mut m = a.clone();
    m.vertices.extend_from_slice(&b.vertices);
    m.indices.extend(b.indices.iter().map(|&i| i + offset));
    m
}
