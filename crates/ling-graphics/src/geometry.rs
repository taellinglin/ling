use glam::{Vec2, Vec3};
use std::collections::HashMap;
use crate::color::Color;
use crate::math::Aabb;

// ── Vertex ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub position: Vec3,
    pub normal: Vec3,
    pub uv: Vec2,
    pub color: Color,
    pub tangent: Vec3,
}

impl Vertex {
    pub fn new(position: Vec3, normal: Vec3, uv: Vec2) -> Self {
        Self { position, normal, uv, color: Color::WHITE, tangent: Vec3::X }
    }

    pub fn with_color(mut self, c: Color) -> Self { self.color = c; self }
}

// ── Mesh ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub aabb: Aabb,
}

impl Mesh {
    pub fn new(vertices: Vec<Vertex>, indices: Vec<u32>) -> Self {
        let positions: Vec<Vec3> = vertices.iter().map(|v| v.position).collect();
        let aabb = if positions.is_empty() {
            Aabb::new(Vec3::ZERO, Vec3::ZERO)
        } else {
            Aabb::from_points(&positions)
        };
        Self { vertices, indices, aabb }
    }

    pub fn triangle_count(&self) -> usize { self.indices.len() / 3 }

    /// Recompute smooth normals from triangle face normals.
    pub fn compute_normals(&mut self) {
        let mut normals = vec![Vec3::ZERO; self.vertices.len()];
        for tri in self.indices.chunks(3) {
            let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            let p0 = self.vertices[i0].position;
            let p1 = self.vertices[i1].position;
            let p2 = self.vertices[i2].position;
            let n = (p1 - p0).cross(p2 - p0);
            normals[i0] += n;
            normals[i1] += n;
            normals[i2] += n;
        }
        for (v, n) in self.vertices.iter_mut().zip(normals) {
            v.normal = n.normalize_or_zero();
        }
    }

    /// Compute tangents for normal mapping (requires UVs).
    pub fn compute_tangents(&mut self) {
        let mut tangents = vec![Vec3::ZERO; self.vertices.len()];
        for tri in self.indices.chunks(3) {
            let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            let p0 = self.vertices[i0].position;
            let p1 = self.vertices[i1].position;
            let p2 = self.vertices[i2].position;
            let uv0 = self.vertices[i0].uv;
            let uv1 = self.vertices[i1].uv;
            let uv2 = self.vertices[i2].uv;
            let e1 = p1 - p0;
            let e2 = p2 - p0;
            let du1 = uv1.x - uv0.x;
            let dv1 = uv1.y - uv0.y;
            let du2 = uv2.x - uv0.x;
            let dv2 = uv2.y - uv0.y;
            let denom = du1 * dv2 - du2 * dv1;
            if denom.abs() < 1e-8 { continue; }
            let inv = 1.0 / denom;
            let t = (e1 * dv2 - e2 * dv1) * inv;
            tangents[i0] += t;
            tangents[i1] += t;
            tangents[i2] += t;
        }
        for (v, t) in self.vertices.iter_mut().zip(tangents) {
            v.tangent = t.normalize_or_zero();
        }
    }
}

// ── MeshBuilder ───────────────────────────────────────────────────────────────

pub struct MeshBuilder {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
}

impl MeshBuilder {
    pub fn new() -> Self { Self { vertices: Vec::new(), indices: Vec::new() } }

    pub fn add_vertex(&mut self, v: Vertex) -> u32 {
        let idx = self.vertices.len() as u32;
        self.vertices.push(v);
        idx
    }

    pub fn add_triangle(&mut self, i0: u32, i1: u32, i2: u32) {
        self.indices.extend_from_slice(&[i0, i1, i2]);
    }

    pub fn build(self) -> Mesh { Mesh::new(self.vertices, self.indices) }
}

impl Default for MeshBuilder { fn default() -> Self { Self::new() } }

// ── Procedural primitives ─────────────────────────────────────────────────────

/// Axis-aligned unit cube (−0.5 to +0.5 on each axis), with correct per-face normals.
pub fn cube(half: f32) -> Mesh {
    let h = half;
    let faces: &[(Vec3, Vec3, Vec3)] = &[
        // normal, tangent_u, tangent_v
        ( Vec3::Z,  Vec3::X,  Vec3::Y),  // +Z
        (-Vec3::Z, -Vec3::X,  Vec3::Y),  // -Z
        ( Vec3::Y,  Vec3::X, -Vec3::Z),  // +Y
        (-Vec3::Y,  Vec3::X,  Vec3::Z),  // -Y
        ( Vec3::X, -Vec3::Z,  Vec3::Y),  // +X
        (-Vec3::X,  Vec3::Z,  Vec3::Y),  // -X
    ];
    let mut verts = Vec::new();
    let mut idx = Vec::new();
    for &(n, tu, tv) in faces {
        let base = verts.len() as u32;
        let center = n * h;
        let corners = [
            center - tu * h - tv * h,
            center + tu * h - tv * h,
            center + tu * h + tv * h,
            center - tu * h + tv * h,
        ];
        let uvs = [Vec2::new(0.0,0.0), Vec2::new(1.0,0.0), Vec2::new(1.0,1.0), Vec2::new(0.0,1.0)];
        for (p, uv) in corners.iter().zip(uvs.iter()) {
            verts.push(Vertex { position: *p, normal: n, uv: *uv, color: Color::WHITE, tangent: tu });
        }
        idx.extend_from_slice(&[base, base+1, base+2, base, base+2, base+3]);
    }
    Mesh::new(verts, idx)
}

/// UV sphere with given radius, rings, and sectors.
pub fn sphere(radius: f32, rings: u32, sectors: u32) -> Mesh {
    let rings = rings.max(2);
    let sectors = sectors.max(3);
    let mut verts = Vec::new();
    let mut idx = Vec::new();

    for r in 0..=rings {
        let phi = std::f32::consts::PI * r as f32 / rings as f32;
        let (sin_phi, cos_phi) = phi.sin_cos();
        for s in 0..=sectors {
            let theta = 2.0 * std::f32::consts::PI * s as f32 / sectors as f32;
            let (sin_t, cos_t) = theta.sin_cos();
            let x = sin_phi * cos_t;
            let y = cos_phi;
            let z = sin_phi * sin_t;
            let p = Vec3::new(x, y, z);
            let u = s as f32 / sectors as f32;
            let v = r as f32 / rings as f32;
            verts.push(Vertex { position: p * radius, normal: p, uv: Vec2::new(u, v), color: Color::WHITE, tangent: Vec3::new(-sin_t, 0.0, cos_t) });
        }
    }

    let w = sectors + 1;
    for r in 0..rings {
        for s in 0..sectors {
            let i0 = r * w + s;
            let i1 = i0 + 1;
            let i2 = (r + 1) * w + s;
            let i3 = i2 + 1;
            idx.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
        }
    }
    Mesh::new(verts, idx)
}

/// Icosphere with given radius and subdivision level (0 = raw icosahedron, 20 triangles).
pub fn icosphere(radius: f32, subdivisions: u32) -> Mesh {
    let t = (1.0 + 5.0_f32.sqrt()) / 2.0;
    let base_verts: &[[f32; 3]] = &[
        [-1.0,  t,  0.0], [ 1.0,  t,  0.0], [-1.0, -t,  0.0], [ 1.0, -t,  0.0],
        [ 0.0, -1.0,  t], [ 0.0,  1.0,  t], [ 0.0, -1.0, -t], [ 0.0,  1.0, -t],
        [ t,  0.0, -1.0], [ t,  0.0,  1.0], [-t,  0.0, -1.0], [-t,  0.0,  1.0],
    ];
    let base_faces: &[[u32; 3]] = &[
        [0,11,5],[0,5,1],[0,1,7],[0,7,10],[0,10,11],
        [1,5,9],[5,11,4],[11,10,2],[10,7,6],[7,1,8],
        [3,9,4],[3,4,2],[3,2,6],[3,6,8],[3,8,9],
        [4,9,5],[2,4,11],[6,2,10],[8,6,7],[9,8,1],
    ];

    let mut points: Vec<Vec3> = base_verts.iter()
        .map(|v| Vec3::new(v[0], v[1], v[2]).normalize())
        .collect();
    let mut faces: Vec<[u32; 3]> = base_faces.to_vec();

    let mut midpoint_cache: HashMap<u64, u32> = HashMap::new();

    let mut get_midpoint = |a: u32, b: u32, pts: &mut Vec<Vec3>| -> u32 {
        let key = if a < b { ((a as u64) << 32) | b as u64 } else { ((b as u64) << 32) | a as u64 };
        if let Some(&idx) = midpoint_cache.get(&key) { return idx; }
        let mid = (pts[a as usize] + pts[b as usize]).normalize();
        let idx = pts.len() as u32;
        pts.push(mid);
        midpoint_cache.insert(key, idx);
        idx
    };

    for _ in 0..subdivisions {
        let mut new_faces = Vec::with_capacity(faces.len() * 4);
        for tri in &faces {
            let m0 = get_midpoint(tri[0], tri[1], &mut points);
            let m1 = get_midpoint(tri[1], tri[2], &mut points);
            let m2 = get_midpoint(tri[2], tri[0], &mut points);
            new_faces.push([tri[0], m0, m2]);
            new_faces.push([tri[1], m1, m0]);
            new_faces.push([tri[2], m2, m1]);
            new_faces.push([m0, m1, m2]);
        }
        faces = new_faces;
    }

    let verts: Vec<Vertex> = points.iter().map(|&p| {
        let u = p.z.atan2(p.x) / (2.0 * std::f32::consts::PI) + 0.5;
        let v = p.y.asin() / std::f32::consts::PI + 0.5;
        Vertex { position: p * radius, normal: p, uv: Vec2::new(u, v), color: Color::WHITE, tangent: Vec3::new(-p.z, 0.0, p.x).normalize_or_zero() }
    }).collect();

    let indices: Vec<u32> = faces.iter().flat_map(|t| t.iter().cloned()).collect();
    Mesh::new(verts, indices)
}

/// Cone: apex at +Y, base at −Y, with given radius, height, and radial segments.
pub fn cone(radius: f32, height: f32, segments: u32) -> Mesh {
    let segments = segments.max(3);
    let mut verts = Vec::new();
    let mut idx   = Vec::new();
    let apex = Vec3::new(0.0, height * 0.5, 0.0);
    let apex_idx = verts.len() as u32;
    verts.push(Vertex { position: apex, normal: Vec3::Y, uv: Vec2::new(0.5, 0.0), color: Color::WHITE, tangent: Vec3::X });
    let base_center_idx = verts.len() as u32;
    verts.push(Vertex { position: Vec3::new(0.0, -height * 0.5, 0.0), normal: -Vec3::Y, uv: Vec2::new(0.5, 0.5), color: Color::WHITE, tangent: Vec3::X });

    let first_base = verts.len() as u32;
    for i in 0..=segments {
        let angle = 2.0 * std::f32::consts::PI * i as f32 / segments as f32;
        let (s, c) = angle.sin_cos();
        let pos = Vec3::new(c * radius, -height * 0.5, s * radius);
        let side_n = Vec3::new(c * height, radius, s * height).normalize();
        let uv = Vec2::new(i as f32 / segments as f32, 1.0);
        verts.push(Vertex { position: pos, normal: side_n, uv, color: Color::WHITE, tangent: Vec3::new(-s, 0.0, c) });
    }
    let first_base_cap = verts.len() as u32;
    for i in 0..=segments {
        let angle = 2.0 * std::f32::consts::PI * i as f32 / segments as f32;
        let (s, c) = angle.sin_cos();
        let pos = Vec3::new(c * radius, -height * 0.5, s * radius);
        let uv = Vec2::new(c * 0.5 + 0.5, s * 0.5 + 0.5);
        verts.push(Vertex { position: pos, normal: -Vec3::Y, uv, color: Color::WHITE, tangent: Vec3::X });
    }
    // Sides
    for i in 0..segments {
        idx.extend_from_slice(&[apex_idx, first_base + i, first_base + i + 1]);
    }
    // Base cap
    for i in 0..segments {
        idx.extend_from_slice(&[base_center_idx, first_base_cap + i + 1, first_base_cap + i]);
    }
    Mesh::new(verts, idx)
}

/// Square pyramid: apex at +Y, base at −Y.
pub fn pyramid(base_half: f32, height: f32) -> Mesh {
    let h2 = height * 0.5;
    let b  = base_half;
    let apex = Vec3::new(0.0, h2, 0.0);

    let base_pts = [
        Vec3::new(-b, -h2, -b),
        Vec3::new( b, -h2, -b),
        Vec3::new( b, -h2,  b),
        Vec3::new(-b, -h2,  b),
    ];

    let mut verts = Vec::new();
    let mut idx   = Vec::new();

    // Four triangular faces
    for i in 0..4 {
        let a = base_pts[i];
        let c = base_pts[(i + 1) % 4];
        let n = (c - a).cross(apex - a).normalize();
        let base = verts.len() as u32;
        verts.push(Vertex { position: apex, normal: n, uv: Vec2::new(0.5, 0.0), color: Color::WHITE, tangent: Vec3::X });
        verts.push(Vertex { position: a, normal: n, uv: Vec2::new(0.0, 1.0), color: Color::WHITE, tangent: Vec3::X });
        verts.push(Vertex { position: c, normal: n, uv: Vec2::new(1.0, 1.0), color: Color::WHITE, tangent: Vec3::X });
        idx.extend_from_slice(&[base, base + 1, base + 2]);
    }

    // Base quad
    let bn = -Vec3::Y;
    let base_start = verts.len() as u32;
    for (i, &p) in base_pts.iter().enumerate() {
        let u = if i == 1 || i == 2 { 1.0 } else { 0.0 };
        let v = if i == 2 || i == 3 { 1.0 } else { 0.0 };
        verts.push(Vertex { position: p, normal: bn, uv: Vec2::new(u, v), color: Color::WHITE, tangent: Vec3::X });
    }
    idx.extend_from_slice(&[base_start, base_start+2, base_start+1, base_start, base_start+3, base_start+2]);

    Mesh::new(verts, idx)
}

/// Cylinder with top and bottom caps.
pub fn cylinder(radius: f32, height: f32, segments: u32) -> Mesh {
    let segments = segments.max(3);
    let h2 = height * 0.5;
    let mut verts = Vec::new();
    let mut idx   = Vec::new();

    // Side
    let side_start = 0u32;
    for i in 0..=segments {
        let angle = 2.0 * std::f32::consts::PI * i as f32 / segments as f32;
        let (s, c) = angle.sin_cos();
        let n = Vec3::new(c, 0.0, s);
        let u = i as f32 / segments as f32;
        verts.push(Vertex { position: Vec3::new(c * radius,  h2, s * radius), normal: n, uv: Vec2::new(u, 0.0), color: Color::WHITE, tangent: Vec3::new(-s, 0.0, c) });
        verts.push(Vertex { position: Vec3::new(c * radius, -h2, s * radius), normal: n, uv: Vec2::new(u, 1.0), color: Color::WHITE, tangent: Vec3::new(-s, 0.0, c) });
    }
    for i in 0..segments {
        let b = side_start + i * 2;
        idx.extend_from_slice(&[b, b+2, b+1, b+1, b+2, b+3]);
    }

    // Caps
    for (cap_y, cap_n, flip) in [( h2, Vec3::Y, false), (-h2, -Vec3::Y, true)] {
        let center = verts.len() as u32;
        verts.push(Vertex { position: Vec3::new(0.0, cap_y, 0.0), normal: cap_n, uv: Vec2::new(0.5, 0.5), color: Color::WHITE, tangent: Vec3::X });
        let first = verts.len() as u32;
        for i in 0..=segments {
            let angle = 2.0 * std::f32::consts::PI * i as f32 / segments as f32;
            let (s, c) = angle.sin_cos();
            verts.push(Vertex { position: Vec3::new(c * radius, cap_y, s * radius), normal: cap_n, uv: Vec2::new(c * 0.5 + 0.5, s * 0.5 + 0.5), color: Color::WHITE, tangent: Vec3::X });
        }
        for i in 0..segments {
            if flip {
                idx.extend_from_slice(&[center, first + i, first + i + 1]);
            } else {
                idx.extend_from_slice(&[center, first + i + 1, first + i]);
            }
        }
    }

    Mesh::new(verts, idx)
}

/// Torus centered at origin, with major radius R and tube radius r.
pub fn torus(major_radius: f32, tube_radius: f32, major_segs: u32, tube_segs: u32) -> Mesh {
    let major_segs = major_segs.max(3);
    let tube_segs  = tube_segs.max(3);
    let mut verts = Vec::new();
    let mut idx   = Vec::new();

    for i in 0..=major_segs {
        let u = 2.0 * std::f32::consts::PI * i as f32 / major_segs as f32;
        let (su, cu) = u.sin_cos();
        let center = Vec3::new(cu * major_radius, 0.0, su * major_radius);
        let radial  = Vec3::new(cu, 0.0, su);
        for j in 0..=tube_segs {
            let v = 2.0 * std::f32::consts::PI * j as f32 / tube_segs as f32;
            let (sv, cv) = v.sin_cos();
            let pos = center + (radial * cv + Vec3::Y * sv) * tube_radius;
            let n   = (radial * cv + Vec3::Y * sv).normalize();
            verts.push(Vertex {
                position: pos,
                normal: n,
                uv: Vec2::new(i as f32 / major_segs as f32, j as f32 / tube_segs as f32),
                color: Color::WHITE,
                tangent: Vec3::new(-su, 0.0, cu),
            });
        }
    }

    let w = tube_segs + 1;
    for i in 0..major_segs {
        for j in 0..tube_segs {
            let i0 = i * w + j;
            let i1 = i0 + 1;
            let i2 = (i + 1) * w + j;
            let i3 = i2 + 1;
            idx.extend_from_slice(&[i0, i1, i2, i1, i3, i2]);
        }
    }
    Mesh::new(verts, idx)
}

/// Flat plane in the XZ plane, subdivided into a grid.
pub fn plane(half_size: f32, subdivisions: u32) -> Mesh {
    let n = subdivisions.max(1) + 1;
    let mut verts = Vec::new();
    let mut idx   = Vec::new();
    let step = half_size * 2.0 / subdivisions.max(1) as f32;
    for row in 0..n {
        for col in 0..n {
            let x = -half_size + col as f32 * step;
            let z = -half_size + row as f32 * step;
            let u = col as f32 / (n - 1) as f32;
            let v = row as f32 / (n - 1) as f32;
            verts.push(Vertex { position: Vec3::new(x, 0.0, z), normal: Vec3::Y, uv: Vec2::new(u, v), color: Color::WHITE, tangent: Vec3::X });
        }
    }
    for row in 0..n - 1 {
        for col in 0..n - 1 {
            let i0 = row * n + col;
            let i1 = i0 + 1;
            let i2 = (row + 1) * n + col;
            let i3 = i2 + 1;
            idx.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
        }
    }
    Mesh::new(verts, idx)
}
