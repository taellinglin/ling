// src/gfx/shapes.rs — parametric 3-D primitive mesh library ("Inkscape for 3-D").
//
// Each generator returns a `Mesh` in LOCAL space (roughly spanning [-1,1],
// centred at the origin). `build()` applies a per-axis scale, an Euler
// rotation (X→Y→Z, radians) and a translation, producing a world-space mesh
// ready for `GfxState::emit_mesh`.
//
// Rendering reuses the engine's existing pipeline: filled triangles are
// cel-lit + projected + queued exactly like `draw_triangle_3d`; wireframe
// edges are projected + queued like `draw_line_3d`.
//
// Draw modes (the `mode` arg of every shape builtin):
//   0 = filled      1 = wireframe      2 = both (wire on top of fill)

use std::collections::HashSet;
use std::f32::consts::PI;
use super::GfxState;

/// A triangle mesh plus an explicit edge list for clean wireframes.
#[derive(Default, Clone)]
pub struct Mesh {
    pub verts: Vec<[f32; 3]>,
    pub tris:  Vec<[u32; 3]>,
    pub edges: Vec<[u32; 2]>,
    /// Smooth (area-weighted averaged) per-vertex normals, world space.
    /// Populated by `build()` after transform; empty until then.
    pub normals: Vec<[f32; 3]>,
}

impl Mesh {
    fn v(&mut self, x: f32, y: f32, z: f32) -> u32 {
        let i = self.verts.len() as u32;
        self.verts.push([x, y, z]);
        i
    }
    fn tri(&mut self, a: u32, b: u32, c: u32) { self.tris.push([a, b, c]); }
    fn edge(&mut self, a: u32, b: u32)        { self.edges.push([a, b]); }

    /// Add a convex polygon (fan-triangulated) and its perimeter edges.
    fn face(&mut self, idx: &[u32]) {
        for k in 1..idx.len() - 1 {
            self.tris.push([idx[0], idx[k], idx[k + 1]]);
        }
        for k in 0..idx.len() {
            self.edges.push([idx[k], idx[(k + 1) % idx.len()]]);
        }
    }

    /// Derive a deduplicated edge list from the triangles (for curved meshes).
    fn edges_from_tris(&mut self) {
        let mut seen: HashSet<(u32, u32)> = HashSet::new();
        for t in &self.tris {
            for &(a, b) in &[(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let k = if a < b { (a, b) } else { (b, a) };
                if seen.insert(k) { self.edges.push([k.0, k.1]); }
            }
        }
    }

    /// Compute area-weighted smooth per-vertex normals from the current
    /// (already transformed) verts + tris — gives continuous shading with no
    /// faceted edges.
    fn compute_smooth_normals(&mut self) {
        let mut n = vec![[0.0f32; 3]; self.verts.len()];
        for t in &self.tris {
            let a = self.verts[t[0] as usize];
            let b = self.verts[t[1] as usize];
            let c = self.verts[t[2] as usize];
            let u = [b[0]-a[0], b[1]-a[1], b[2]-a[2]];
            let v = [c[0]-a[0], c[1]-a[1], c[2]-a[2]];
            let f = [u[1]*v[2]-u[2]*v[1], u[2]*v[0]-u[0]*v[2], u[0]*v[1]-u[1]*v[0]];
            for &i in t { let i = i as usize; n[i][0]+=f[0]; n[i][1]+=f[1]; n[i][2]+=f[2]; }
        }
        for p in &mut n {
            let l = (p[0]*p[0]+p[1]*p[1]+p[2]*p[2]).sqrt();
            if l > 1e-8 { p[0]/=l; p[1]/=l; p[2]/=l; }
        }
        self.normals = n;
    }

    /// scale → rotate(Euler XYZ) → translate, in place.
    fn transform(&mut self, c: [f32; 9]) {
        let (cx, cy, cz) = (c[0], c[1], c[2]);
        let (sx, sy, sz) = (c[3], c[4], c[5]);
        let (rx, ry, rz) = (c[6], c[7], c[8]);
        let (srx, crx) = rx.sin_cos();
        let (sry, cry) = ry.sin_cos();
        let (srz, crz) = rz.sin_cos();
        for p in &mut self.verts {
            let mut x = p[0] * sx;
            let mut y = p[1] * sy;
            let mut z = p[2] * sz;
            // rotate X
            let (ny, nz) = (y * crx - z * srx, y * srx + z * crx); y = ny; z = nz;
            // rotate Y
            let (nx, nz2) = (x * cry + z * sry, -x * sry + z * cry); x = nx; z = nz2;
            // rotate Z
            let (nx2, ny2) = (x * crz - y * srz, x * srz + y * crz); x = nx2; y = ny2;
            *p = [x + cx, y + cy, z + cz];
        }
    }
}

// ── small helpers ───────────────────────────────────────────────────────────
#[inline] fn iarg(v: f32, default: i32) -> i32 { if v > 0.5 { v.round() as i32 } else { default } }
#[inline] fn farg(v: f32, default: f32) -> f32 { if v > 1e-6 { v } else { default } }

// ── Platonic / dice solids ───────────────────────────────────────────────────

fn cube() -> Mesh {
    let mut m = Mesh::default();
    let s = 1.0;
    let p = [
        m.v(-s,-s,-s), m.v(s,-s,-s), m.v(s,s,-s), m.v(-s,s,-s), // back  0..3
        m.v(-s,-s, s), m.v(s,-s, s), m.v(s,s, s), m.v(-s,s, s), // front 4..7
    ];
    m.face(&[p[0],p[1],p[2],p[3]]); // -Z
    m.face(&[p[5],p[4],p[7],p[6]]); // +Z
    m.face(&[p[4],p[0],p[3],p[7]]); // -X
    m.face(&[p[1],p[5],p[6],p[2]]); // +X
    m.face(&[p[4],p[5],p[1],p[0]]); // -Y
    m.face(&[p[3],p[2],p[6],p[7]]); // +Y
    m
}

fn tetrahedron() -> Mesh {
    let mut m = Mesh::default();
    let a = 1.0;
    let p = [
        m.v( a, a, a), m.v( a,-a,-a), m.v(-a, a,-a), m.v(-a,-a, a),
    ];
    m.face(&[p[0],p[1],p[2]]);
    m.face(&[p[0],p[3],p[1]]);
    m.face(&[p[0],p[2],p[3]]);
    m.face(&[p[1],p[3],p[2]]);
    m
}

fn octahedron() -> Mesh {
    let mut m = Mesh::default();
    let p = [
        m.v( 1.0,0.0,0.0), m.v(-1.0,0.0,0.0),
        m.v(0.0, 1.0,0.0), m.v(0.0,-1.0,0.0),
        m.v(0.0,0.0, 1.0), m.v(0.0,0.0,-1.0),
    ];
    m.face(&[p[0],p[2],p[4]]); m.face(&[p[2],p[1],p[4]]);
    m.face(&[p[1],p[3],p[4]]); m.face(&[p[3],p[0],p[4]]);
    m.face(&[p[2],p[0],p[5]]); m.face(&[p[1],p[2],p[5]]);
    m.face(&[p[3],p[1],p[5]]); m.face(&[p[0],p[3],p[5]]);
    m
}

fn icosahedron_raw() -> Mesh {
    let mut m = Mesh::default();
    let t = (1.0 + 5.0_f32.sqrt()) / 2.0;
    let s = 1.0 / (1.0 + t*t).sqrt(); // normalise to unit radius
    let vs = [
        [-1., t, 0.],[1., t, 0.],[-1.,-t, 0.],[1.,-t, 0.],
        [0.,-1., t],[0., 1., t],[0.,-1.,-t],[0., 1.,-t],
        [ t, 0.,-1.],[ t, 0., 1.],[-t, 0.,-1.],[-t, 0., 1.],
    ];
    for v in vs { m.v(v[0]*s, v[1]*s, v[2]*s); }
    let f = [
        [0,11,5],[0,5,1],[0,1,7],[0,7,10],[0,10,11],
        [1,5,9],[5,11,4],[11,10,2],[10,7,6],[7,1,8],
        [3,9,4],[3,4,2],[3,2,6],[3,6,8],[3,8,9],
        [4,9,5],[2,4,11],[6,2,10],[8,6,7],[9,8,1],
    ];
    for t in f { m.tri(t[0],t[1],t[2]); }
    m
}

fn icosahedron() -> Mesh { let mut m = icosahedron_raw(); m.edges_from_tris(); m }

fn icosphere(subdiv: i32) -> Mesh {
    let mut m = icosahedron_raw();
    let n = subdiv.clamp(0, 4);
    for _ in 0..n {
        let mut nm = Mesh::default();
        let mut mid: std::collections::HashMap<(u32,u32),u32> = std::collections::HashMap::new();
        for v in &m.verts { nm.verts.push(*v); }
        let midpoint = |nm: &mut Mesh, a: u32, b: u32, mid: &mut std::collections::HashMap<(u32,u32),u32>| -> u32 {
            let key = if a < b { (a,b) } else { (b,a) };
            if let Some(&i) = mid.get(&key) { return i; }
            let pa = nm.verts[a as usize]; let pb = nm.verts[b as usize];
            let mut mp = [(pa[0]+pb[0])/2.0,(pa[1]+pb[1])/2.0,(pa[2]+pb[2])/2.0];
            let l = (mp[0]*mp[0]+mp[1]*mp[1]+mp[2]*mp[2]).sqrt();
            mp = [mp[0]/l, mp[1]/l, mp[2]/l];
            let i = nm.verts.len() as u32; nm.verts.push(mp); mid.insert(key, i); i
        };
        for t in &m.tris {
            let a = midpoint(&mut nm, t[0], t[1], &mut mid);
            let b = midpoint(&mut nm, t[1], t[2], &mut mid);
            let c = midpoint(&mut nm, t[2], t[0], &mut mid);
            nm.tri(t[0],a,c); nm.tri(t[1],b,a); nm.tri(t[2],c,b); nm.tri(a,b,c);
        }
        m = nm;
    }
    m.edges_from_tris();
    m
}

fn dodecahedron() -> Mesh {
    let mut m = Mesh::default();
    let phi = (1.0 + 5.0_f32.sqrt()) / 2.0;
    let b = 1.0 / phi;
    let c = phi;
    let r = (3.0_f32).sqrt(); // normalise so |(1,1,1)| family → unit-ish
    let s = 1.0 / r;
    let vs = [
        [ 1., 1., 1.],[ 1., 1.,-1.],[ 1.,-1., 1.],[ 1.,-1.,-1.],
        [-1., 1., 1.],[-1., 1.,-1.],[-1.,-1., 1.],[-1.,-1.,-1.],
        [0., b, c],[0., b,-c],[0.,-b, c],[0.,-b,-c],
        [ b, c, 0.],[ b,-c, 0.],[-b, c, 0.],[-b,-c, 0.],
        [ c, 0., b],[ c, 0.,-b],[-c, 0., b],[-c, 0.,-b],
    ];
    for v in vs { m.v(v[0]*s, v[1]*s, v[2]*s); }
    let faces: [[u32;5];12] = [
        [0,8,10,2,16],[0,16,17,1,12],[0,12,14,4,8],
        [1,9,5,14,12],[1,17,3,11,9],[2,10,6,15,13],
        [2,13,3,17,16],[3,13,15,7,11],[4,14,5,19,18],
        [4,18,6,10,8],[5,9,11,7,19],[6,18,19,7,15],
    ];
    for f in faces { m.face(&f); }
    m
}

// ── round / swept solids ──────────────────────────────────────────────────────

fn uv_sphere(seg: i32, rings: i32) -> Mesh {
    let mut m = Mesh::default();
    let seg = seg.clamp(3, 128);
    let rings = rings.clamp(2, 128);
    for r in 0..=rings {
        let v = r as f32 / rings as f32;
        let theta = v * PI;            // 0..pi
        let (st, ct) = theta.sin_cos();
        for s in 0..=seg {
            let u = s as f32 / seg as f32;
            let phi = u * 2.0 * PI;
            let (sp, cp) = phi.sin_cos();
            m.v(st * cp, ct, st * sp);
        }
    }
    let stride = seg + 1;
    for r in 0..rings {
        for s in 0..seg {
            let a = (r * stride + s) as u32;
            let b = (r * stride + s + 1) as u32;
            let cc = ((r + 1) * stride + s) as u32;
            let d = ((r + 1) * stride + s + 1) as u32;
            m.tri(a, cc, b); m.tri(b, cc, d);
        }
    }
    m.edges_from_tris();
    m
}

fn dome(seg: i32, rings: i32) -> Mesh {
    // upper hemisphere (y in [0..1]) with a closing base ring
    let mut m = Mesh::default();
    let seg = seg.clamp(3, 128);
    let rings = rings.clamp(1, 128);
    for r in 0..=rings {
        let v = r as f32 / rings as f32;
        let theta = v * (PI / 2.0);    // 0..pi/2
        let (st, ct) = theta.sin_cos();
        for s in 0..=seg {
            let phi = s as f32 / seg as f32 * 2.0 * PI;
            let (sp, cp) = phi.sin_cos();
            m.v(st * cp, ct, st * sp);
        }
    }
    let stride = seg + 1;
    for r in 0..rings {
        for s in 0..seg {
            let a = (r*stride+s) as u32; let b=(r*stride+s+1) as u32;
            let cc=((r+1)*stride+s) as u32; let d=((r+1)*stride+s+1) as u32;
            m.tri(a, cc, b); m.tri(b, cc, d);
        }
    }
    // base cap
    let centre = m.v(0.0, 0.0, 0.0);
    for s in 0..seg {
        let a = ((rings)*stride+s) as u32; let b=((rings)*stride+s+1) as u32;
        m.tri(centre, b, a);
    }
    m.edges_from_tris();
    m
}

fn cylinder(seg: i32) -> Mesh {
    let mut m = Mesh::default();
    let seg = seg.clamp(3, 256);
    // rings at y=-1 (bottom) and y=+1 (top)
    for s in 0..seg {
        let phi = s as f32 / seg as f32 * 2.0 * PI;
        let (sp, cp) = phi.sin_cos();
        m.v(cp, -1.0, sp);
        m.v(cp,  1.0, sp);
    }
    for s in 0..seg {
        let b0 = (2*s) as u32; let t0 = (2*s+1) as u32;
        let b1 = (2*((s+1)%seg)) as u32; let t1 = (2*((s+1)%seg)+1) as u32;
        m.tri(b0, t0, b1); m.tri(b1, t0, t1);
        m.edge(b0, b1); m.edge(t0, t1); m.edge(b0, t0);
    }
    let cb = m.v(0.0,-1.0,0.0); let ct = m.v(0.0,1.0,0.0);
    for s in 0..seg {
        let b0=(2*s) as u32; let b1=(2*((s+1)%seg)) as u32;
        let t0=(2*s+1) as u32; let t1=(2*((s+1)%seg)+1) as u32;
        m.tri(cb, b1, b0); m.tri(ct, t0, t1);
    }
    m
}

fn cone(seg: i32) -> Mesh {
    let mut m = Mesh::default();
    let seg = seg.clamp(3, 256);
    let apex = m.v(0.0, 1.0, 0.0);
    let base0 = m.verts.len() as u32;
    for s in 0..seg {
        let phi = s as f32 / seg as f32 * 2.0 * PI;
        let (sp, cp) = phi.sin_cos();
        m.v(cp, -1.0, sp);
    }
    let centre = m.v(0.0, -1.0, 0.0);
    for s in 0..seg {
        let a = base0 + s as u32; let b = base0 + ((s+1)%seg) as u32;
        m.tri(apex, a, b);   // side
        m.tri(centre, b, a); // base
        m.edge(a, b); m.edge(apex, a);
    }
    m
}

fn capsule(seg: i32, rings: i32) -> Mesh {
    // cylinder body (y -1..1) capped by two hemispheres of radius 1
    let mut m = Mesh::default();
    let seg = seg.clamp(3, 128);
    let rings = rings.clamp(1, 64);
    let stride = seg + 1;
    // top hemisphere: theta 0..pi/2 mapped onto y = 1 + cos*? keep radius 1 sphere centred at y=+1
    let mut ring_start = Vec::new();
    let total_rows = 2 * rings; // top hemi rows + bottom hemi rows
    for row in 0..=total_rows {
        ring_start.push(m.verts.len() as u32);
        let (cy_off, theta) = if row <= rings {
            // top hemisphere: row 0 = pole (theta 0)
            let v = row as f32 / rings as f32;
            (1.0, v * PI / 2.0)
        } else {
            // bottom hemisphere
            let v = (row - rings) as f32 / rings as f32;
            (-1.0, PI / 2.0 + v * PI / 2.0)
        };
        let (st, ct) = theta.sin_cos();
        for s in 0..=seg {
            let phi = s as f32 / seg as f32 * 2.0 * PI;
            let (sp, cp) = phi.sin_cos();
            m.v(st * cp, cy_off + ct, st * sp);
        }
    }
    for row in 0..total_rows as usize {
        for s in 0..seg {
            let a = ring_start[row] + s as u32;
            let b = ring_start[row] + s as u32 + 1;
            let c = ring_start[row + 1] + s as u32;
            let d = ring_start[row + 1] + s as u32 + 1;
            m.tri(a, c, b); m.tri(b, c, d);
        }
    }
    let _ = stride;
    m.edges_from_tris();
    m
}

fn torus(seg: i32, sides: i32, tube: f32) -> Mesh {
    let mut m = Mesh::default();
    let seg = seg.clamp(3, 256);    // around the ring
    let sides = sides.clamp(3, 128); // around the tube
    let tube = tube.clamp(0.02, 0.9);
    for i in 0..seg {
        let u = i as f32 / seg as f32 * 2.0 * PI;
        let (su, cu) = u.sin_cos();
        for j in 0..sides {
            let v = j as f32 / sides as f32 * 2.0 * PI;
            let (sv, cv) = v.sin_cos();
            let r = 1.0 - tube + tube * cv;
            m.v(r * cu, tube * sv, r * su);
        }
    }
    for i in 0..seg {
        for j in 0..sides {
            let a = (i*sides + j) as u32;
            let b = (i*sides + (j+1)%sides) as u32;
            let c = (((i+1)%seg)*sides + j) as u32;
            let d = (((i+1)%seg)*sides + (j+1)%sides) as u32;
            m.tri(a, c, b); m.tri(b, c, d);
        }
    }
    m.edges_from_tris();
    m
}

// ── prisms / pyramids ─────────────────────────────────────────────────────────

fn pyramid(sides: i32) -> Mesh {
    let mut m = Mesh::default();
    let sides = sides.clamp(3, 128);
    let apex = m.v(0.0, 1.0, 0.0);
    let base0 = m.verts.len() as u32;
    let mut ring = Vec::new();
    for s in 0..sides {
        let phi = s as f32 / sides as f32 * 2.0 * PI;
        let (sp, cp) = phi.sin_cos();
        ring.push(m.v(cp, -1.0, sp));
    }
    for s in 0..sides as usize {
        let a = ring[s]; let b = ring[(s+1)%sides as usize];
        m.tri(apex, a, b);
        m.edge(a, b); m.edge(apex, a);
    }
    // base face (reversed for outward normal)
    let mut rev: Vec<u32> = ring.clone(); rev.reverse();
    for k in 1..rev.len()-1 { m.tri(rev[0], rev[k], rev[k+1]); }
    let _ = base0;
    m
}

fn prism(sides: i32) -> Mesh {
    let mut m = Mesh::default();
    let sides = sides.clamp(3, 128);
    let mut bot = Vec::new(); let mut top = Vec::new();
    for s in 0..sides {
        let phi = s as f32 / sides as f32 * 2.0 * PI;
        let (sp, cp) = phi.sin_cos();
        bot.push(m.v(cp, -1.0, sp));
        top.push(m.v(cp,  1.0, sp));
    }
    let n = sides as usize;
    for s in 0..n {
        let b0=bot[s]; let b1=bot[(s+1)%n]; let t0=top[s]; let t1=top[(s+1)%n];
        m.tri(b0, t0, b1); m.tri(b1, t0, t1);
        m.edge(b0,b1); m.edge(t0,t1); m.edge(b0,t0);
    }
    for k in 1..n-1 { m.tri(top[0], top[k], top[k+1]); }
    let mut rb: Vec<u32> = bot.clone(); rb.reverse();
    for k in 1..rb.len()-1 { m.tri(rb[0], rb[k], rb[k+1]); }
    m
}

fn frustum(sides: i32, top_ratio: f32) -> Mesh {
    let mut m = Mesh::default();
    let sides = sides.clamp(3, 256);
    let tr = top_ratio.clamp(0.0, 1.0);
    let mut bot = Vec::new(); let mut top = Vec::new();
    for s in 0..sides {
        let phi = s as f32 / sides as f32 * 2.0 * PI;
        let (sp, cp) = phi.sin_cos();
        bot.push(m.v(cp, -1.0, sp));
        top.push(m.v(cp*tr, 1.0, sp*tr));
    }
    let n = sides as usize;
    for s in 0..n {
        let b0=bot[s]; let b1=bot[(s+1)%n]; let t0=top[s]; let t1=top[(s+1)%n];
        m.tri(b0, t0, b1); m.tri(b1, t0, t1);
        m.edge(b0,b1); m.edge(t0,t1); m.edge(b0,t0);
    }
    if tr > 0.001 { for k in 1..n-1 { m.tri(top[0], top[k], top[k+1]); } }
    let mut rb: Vec<u32> = bot.clone(); rb.reverse();
    for k in 1..rb.len()-1 { m.tri(rb[0], rb[k], rb[k+1]); }
    m
}

// ── mechanical / architectural ────────────────────────────────────────────────

fn gear(teeth: i32, tooth: f32) -> Mesh {
    // flat gear in the XZ plane, extruded ±1 in Y; `tooth` = radial tooth depth.
    let mut m = Mesh::default();
    let teeth = teeth.clamp(3, 96);
    let tooth = tooth.clamp(0.02, 0.6);
    let pts = teeth * 4;            // 4 control points per tooth
    let mut bot = Vec::new(); let mut top = Vec::new();
    for i in 0..pts {
        let phi = i as f32 / pts as f32 * 2.0 * PI;
        // square-ish tooth profile: outer for first half of each tooth, inner for second
        let phase = (i % 4) as f32;
        let r = if phase < 2.0 { 1.0 } else { 1.0 - tooth };
        let (sp, cp) = phi.sin_cos();
        bot.push(m.v(cp*r, -1.0, sp*r));
        top.push(m.v(cp*r,  1.0, sp*r));
    }
    let n = pts as usize;
    for s in 0..n {
        let b0=bot[s]; let b1=bot[(s+1)%n]; let t0=top[s]; let t1=top[(s+1)%n];
        m.tri(b0, t0, b1); m.tri(b1, t0, t1);   // rim
        m.edge(b0,b1); m.edge(t0,t1); m.edge(b0,t0);
    }
    let cb = m.v(0.0,-1.0,0.0); let ct = m.v(0.0,1.0,0.0);
    for s in 0..n {
        let b0=bot[s]; let b1=bot[(s+1)%n]; let t0=top[s]; let t1=top[(s+1)%n];
        m.tri(cb, b1, b0); m.tri(ct, t0, t1);   // caps
    }
    m
}

fn gyro(rings: i32) -> Mesh {
    // nested gimbal: `rings` tori on alternating axes at shrinking radius.
    let mut m = Mesh::default();
    let rings = rings.clamp(1, 6);
    for k in 0..rings {
        let scale = 1.0 - k as f32 * (0.8 / rings as f32);
        let mut ring = torus(40, 8, 0.06 / scale.max(0.2));
        // rotate each ring onto a different axis
        let rot = match k % 3 {
            0 => [0.0, 0.0, 0.0],
            1 => [PI/2.0, 0.0, 0.0],
            _ => [0.0, 0.0, PI/2.0],
        };
        ring.transform([0.0,0.0,0.0, scale,scale,scale, rot[0],rot[1],rot[2]]);
        let base = m.verts.len() as u32;
        for v in &ring.verts { m.verts.push(*v); }
        for t in &ring.tris { m.tri(t[0]+base, t[1]+base, t[2]+base); }
        for e in &ring.edges { m.edge(e[0]+base, e[1]+base); }
    }
    m
}

// ── exotic / compound shapes ──────────────────────────────────────────────────

fn append_mesh(dst: &mut Mesh, src: &Mesh) {
    let base = dst.verts.len() as u32;
    for v in &src.verts { dst.verts.push(*v); }
    for t in &src.tris  { dst.tri(t[0]+base, t[1]+base, t[2]+base); }
    for e in &src.edges { dst.edge(e[0]+base, e[1]+base); }
}

fn box_between(x0:f32,x1:f32,y0:f32,y1:f32,z0:f32,z1:f32) -> Mesh {
    let mut m = Mesh::default();
    let p=[
        m.v(x0,y0,z0),m.v(x1,y0,z0),m.v(x1,y1,z0),m.v(x0,y1,z0),
        m.v(x0,y0,z1),m.v(x1,y0,z1),m.v(x1,y1,z1),m.v(x0,y1,z1),
    ];
    m.face(&[p[0],p[1],p[2],p[3]]);
    m.face(&[p[5],p[4],p[7],p[6]]);
    m.face(&[p[4],p[0],p[3],p[7]]);
    m.face(&[p[1],p[5],p[6],p[2]]);
    m.face(&[p[4],p[5],p[1],p[0]]);
    m.face(&[p[3],p[2],p[6],p[7]]);
    m
}

/// Tube swept along a helix around the Y axis (height −1..1).
fn helix(turns: i32, tube: f32, sides: i32) -> Mesh {
    let mut m = Mesh::default();
    let turns = turns.clamp(1, 24);
    let sides = sides.clamp(3, 32);
    let tube  = tube.clamp(0.02, 0.5);
    let seg_per = 24;
    let total = turns * seg_per;
    for i in 0..=total {
        let ang = (i as f32 / seg_per as f32) * 2.0 * PI;
        let y = -1.0 + 2.0 * (i as f32 / total as f32);
        let cen = [ang.cos(), y, ang.sin()];
        let radial = [ang.cos(), 0.0, ang.sin()];
        let up = [0.0, 1.0, 0.0];
        for j in 0..sides {
            let v = j as f32 / sides as f32 * 2.0 * PI;
            let (sv, cv) = v.sin_cos();
            m.v(cen[0] + tube*(cv*radial[0] + sv*up[0]),
                cen[1] + tube*(cv*radial[1] + sv*up[1]),
                cen[2] + tube*(cv*radial[2] + sv*up[2]));
        }
    }
    let s = sides;
    for i in 0..total {
        for j in 0..sides {
            let a=(i*s+j) as u32; let b=(i*s+(j+1)%s) as u32;
            let c=((i+1)*s+j) as u32; let d=((i+1)*s+(j+1)%s) as u32;
            m.tri(a,c,b); m.tri(b,c,d);
        }
    }
    m.edges_from_tris();
    m
}

/// Semicircular archway — circular tube swept over a 180° arc in the XY plane.
fn arch(segs: i32, tube: f32) -> Mesh {
    let mut m = Mesh::default();
    let segs = segs.clamp(6, 128);
    let sides = 10i32;
    let tube = tube.clamp(0.05, 0.4);
    for i in 0..=segs {
        let a = PI * (i as f32 / segs as f32);     // 0..π
        let cen = [a.cos(), a.sin(), 0.0];
        let radial = [a.cos(), a.sin(), 0.0];
        let binorm = [0.0, 0.0, 1.0];
        for j in 0..sides {
            let v = j as f32 / sides as f32 * 2.0 * PI;
            let (sv, cv) = v.sin_cos();
            m.v(cen[0] + tube*(cv*radial[0] + sv*binorm[0]),
                cen[1] + tube*(cv*radial[1] + sv*binorm[1]),
                cen[2] + tube*(cv*radial[2] + sv*binorm[2]));
        }
    }
    for i in 0..segs {
        for j in 0..sides {
            let a=(i*sides+j) as u32; let b=(i*sides+(j+1)%sides) as u32;
            let c=((i+1)*sides+j) as u32; let d=((i+1)*sides+(j+1)%sides) as u32;
            m.tri(a,c,b); m.tri(b,c,d);
        }
    }
    m.edges_from_tris();
    m
}

/// Staircase of `steps` cuboid steps rising along +Y and +Z.
fn stairs(steps: i32) -> Mesh {
    let mut m = Mesh::default();
    let steps = steps.clamp(2, 40);
    let sh = 2.0 / steps as f32;
    let sd = 2.0 / steps as f32;
    for i in 0..steps {
        let y0 = -1.0 + i as f32 * sh; let y1 = y0 + sh;
        let z0 = -1.0 + i as f32 * sd; let zf = z0 + sd;
        let blk = box_between(-1.0, 1.0, y0, y1, z0, zf);
        append_mesh(&mut m, &blk);
    }
    m
}

/// Star-shaped prism: an N-point star cross-section extruded along Y.
fn star_prism(points: i32, inner: f32) -> Mesh {
    let mut m = Mesh::default();
    let points = points.clamp(3, 32);
    let inner = inner.clamp(0.1, 0.95);
    let n = (points * 2) as usize;
    let mut bot = Vec::new(); let mut top = Vec::new();
    for k in 0..n {
        let ang = k as f32 / n as f32 * 2.0 * PI;
        let r = if k % 2 == 0 { 1.0 } else { inner };
        let (s, c) = ang.sin_cos();
        bot.push(m.v(c*r, -1.0, s*r));
        top.push(m.v(c*r,  1.0, s*r));
    }
    for k in 0..n {
        let b0=bot[k]; let b1=bot[(k+1)%n]; let t0=top[k]; let t1=top[(k+1)%n];
        m.tri(b0,t0,b1); m.tri(b1,t0,t1);
        m.edge(b0,b1); m.edge(t0,t1); m.edge(b0,t0);
    }
    for k in 1..n-1 { m.tri(top[0], top[k], top[k+1]); }
    let mut rb = bot.clone(); rb.reverse();
    for k in 1..rb.len()-1 { m.tri(rb[0], rb[k], rb[k+1]); }
    m
}

/// A row of `count` capsule "beads" along X — a chain / caterpillar.
fn capsule_chain(count: i32) -> Mesh {
    let mut m = Mesh::default();
    let count = count.clamp(1, 12);
    let step = 2.0 / count as f32;
    for i in 0..count {
        let mut c = capsule(12, 4);
        let cx = -1.0 + (i as f32 + 0.5) * step;
        c.transform([cx, 0.0, 0.0,  step*0.5, step*0.5, step*0.5,  0.0, 0.0, PI/2.0]);
        append_mesh(&mut m, &c);
    }
    m
}

/// Möbius strip — a half-twisted band looped once.
fn mobius(segs: i32, width: f32) -> Mesh {
    let mut m = Mesh::default();
    let segs = segs.clamp(8, 240);
    let w = width.clamp(0.05, 0.6);
    for i in 0..=segs {
        let u = i as f32 / segs as f32 * 2.0 * PI;
        for &vv in &[-1.0f32, 1.0] {
            let v = vv * w;
            let x = (1.0 + v/2.0 * (u/2.0).cos()) * u.cos();
            let y = v/2.0 * (u/2.0).sin();
            let z = (1.0 + v/2.0 * (u/2.0).cos()) * u.sin();
            m.v(x, y, z);
        }
    }
    for i in 0..segs {
        let a=(2*i) as u32; let b=(2*i+1) as u32; let c=(2*(i+1)) as u32; let d=(2*(i+1)+1) as u32;
        m.tri(a,c,b); m.tri(b,c,d);
    }
    m.edges_from_tris();
    m
}

/// Resolve a builtin call name (in any supported language) to a canonical
/// shape kind. Returns `None` if the name is not a 3-D primitive.
pub fn canon(name: &str) -> Option<&'static str> {
    Some(match name {
        // cube / box
        "cube" | "box" | "立方体" | "方块" | "箱" | "정육면체" | "상자"
            | "ลูกบาศก์" | "กล่อง" => "cube",
        // sphere
        "sphere" | "球体" | "球" | "구" | "ทรงกลม" => "sphere",
        // icosphere
        "icosphere" | "二十面球" | "アイコ球" | "아이코구체" | "ทรงกลมเหลี่ยม" => "icosphere",
        // dome (hemisphere)
        "dome" | "穹顶" | "ドーム" | "돔" | "โดม" => "dome",
        // cylinder
        "cylinder" | "圆柱" | "円柱" | "원기둥" | "ทรงกระบอก" => "cylinder",
        // cone
        "cone" | "圆锥" | "円錐" | "원뿔" | "กรวย" => "cone",
        // capsule
        "capsule" | "胶囊" | "カプセル" | "캡슐" | "แคปซูล" => "capsule",
        // torus / ring
        "torus" | "ring" | "圆环" | "トーラス" | "토러스" | "ทอรัส" => "torus",
        // pyramid
        "pyramid" | "金字塔" | "ピラミッド" | "피라미드" | "พีระมิด" => "pyramid",
        // prism
        "prism" | "棱柱" | "角柱" | "각기둥" | "ปริซึม" => "prism",
        // frustum
        "frustum" | "棱台" | "錐台" | "원뿔대" | "กรวยตัด" => "frustum",
        // tetrahedron / d4
        "tetrahedron" | "d4" | "四面体" | "정사면체" | "ทรงสี่หน้า" => "tetrahedron",
        // octahedron / d8
        "octahedron" | "d8" | "八面体" | "정팔면체" | "ทรงแปดหน้า" => "octahedron",
        // dodecahedron / d12
        "dodecahedron" | "d12" | "十二面体" | "정십이면체" | "ทรงสิบสองหน้า" => "dodecahedron",
        // icosahedron / d20
        "icosahedron" | "d20" | "二十面体" | "정이십면체" | "ทรงยี่สิบหน้า" => "icosahedron",
        // gear / cog
        "gear" | "cog" | "齿轮" | "歯車" | "톱니바퀴" | "เฟือง" => "gear",
        // gyro
        "gyro" | "陀螺" | "ジャイロ" | "자이로" | "ไจโร" => "gyro",
        // helix
        "helix" | "螺旋线" | "らせん" | "나선" | "เกลียว" => "helix",
        // spring
        "spring" | "弹簧" | "ばね" | "스프링" | "สปริง" => "spring",
        // arch
        "arch" | "拱门" | "アーチ" | "아치" | "ซุ้มโค้ง" => "arch",
        // stairs
        "stairs" | "楼梯" | "階段" | "계단" | "บันได" => "stairs",
        // star prism
        "star_prism" | "star" | "星柱" | "星型柱" | "별기둥" | "แท่งดาว" => "star_prism",
        // capsule chain
        "capsule_chain" | "chain" | "胶囊链" | "カプセル鎖" | "캡슐체인" | "โซ่แคปซูล" => "capsule_chain",
        // mobius
        "mobius" | "莫比乌斯" | "メビウス" | "뫼비우스" | "เมอบีอุส" => "mobius",
        _ => return None,
    })
}

/// Build a transformed, world-space mesh for `kind`.
/// `c` = [cx,cy,cz, sx,sy,sz, rx,ry,rz]; `e0..e2` = shape-specific extras.
pub fn build(kind: &str, c: [f32; 9], e0: f32, e1: f32, e2: f32) -> Option<Mesh> {
    let mut m = match kind {
        "cube" | "box"        => cube(),
        "sphere"              => uv_sphere(iarg(e0,16), iarg(e1,12)),
        "icosphere"           => icosphere(iarg(e0,1)),
        "dome"                => dome(iarg(e0,24), iarg(e1,8)),
        "cylinder"            => cylinder(iarg(e0,24)),
        "cone"                => cone(iarg(e0,24)),
        "capsule"             => capsule(iarg(e0,16), iarg(e1,6)),
        "torus" | "ring"      => torus(iarg(e0,32), iarg(e1,12), farg(e2,0.35)),
        "pyramid"             => pyramid(iarg(e0,4)),
        "prism"               => prism(iarg(e0,6)),
        "frustum"             => frustum(iarg(e0,24), farg(e1,0.5)),
        "tetrahedron" | "d4"  => { let mut t = tetrahedron(); t.edges = vec![]; t.edges_from_tris(); t }
        "octahedron"  | "d8"  => { let mut t = octahedron();  t.edges = vec![]; t.edges_from_tris(); t }
        "dodecahedron"| "d12" => dodecahedron(),
        "icosahedron" | "d20" => icosahedron(),
        "gear" | "cog"        => gear(iarg(e0,12), farg(e1,0.25)),
        "gyro"                => gyro(iarg(e0,3)),
        "helix"               => helix(iarg(e0,3), farg(e1,0.15), iarg(e2,8)),
        "spring"              => helix(iarg(e0,6), farg(e1,0.12), iarg(e2,8)),
        "arch"                => arch(iarg(e0,24), farg(e1,0.18)),
        "stairs"              => stairs(iarg(e0,5)),
        "star_prism"          => star_prism(iarg(e0,5), farg(e1,0.5)),
        "capsule_chain"       => capsule_chain(iarg(e0,3)),
        "mobius"              => mobius(iarg(e0,60), farg(e1,0.3)),
        _ => return None,
    };
    m.transform(c);
    m.compute_smooth_normals();
    Some(m)
}

impl GfxState {
    /// Render a world-space mesh through the depth queue.
    /// mode: 0 filled, 1 wireframe, 2 both.
    pub fn emit_mesh(&mut self, m: &Mesh, mode: i32) {
        let near = -self.camera.zdist + 0.05;

        let want_fill = mode == 0 || mode == 2;
        if want_fill {
            let have_normals = m.normals.len() == m.verts.len() && self.shade_mode != 0;
            if have_normals {
                // ── smooth cel / holographic path ─────────────────────────────
                // Per-vertex coloured lighting (smooth normals) → Gouraud
                // interpolation → per-pixel posterise. No faceted edges.
                let base = ling_graphics::shading::unpack(self.color);
                let eye = [self.camera.tx, self.camera.ty, self.camera.tz];
                let lights: Vec<ling_graphics::shading::LightS> = self.lights.iter().map(|l| {
                    ling_graphics::shading::LightS { pos:[l.x,l.y,l.z], color:[l.r,l.g,l.b], intensity:l.intensity, radius:l.radius }
                }).collect();
                let mut sp = self.shade;
                sp.ambient = self.ambient;              // scene ambient drives fill
                if self.shade_mode == 1 { sp.holo = false; sp.rim *= 0.4; }
                let bands = sp.bands;
                for t in &m.tris {
                    let ia=t[0] as usize; let ib=t[1] as usize; let ic=t[2] as usize;
                    let a=m.verts[ia]; let b=m.verts[ib]; let c=m.verts[ic];
                    let da=self.camera.depth(a[0],a[1],a[2]);
                    let db=self.camera.depth(b[0],b[1],b[2]);
                    let dc=self.camera.depth(c[0],c[1],c[2]);
                    if da<=near && db<=near && dc<=near { continue; } // all behind → drop
                    // Lit colours per vertex (kept unpacked so clipping can lerp them).
                    let la = ling_graphics::shading::lit_vertex(base, m.normals[ia], a, eye, &lights, &sp);
                    let lb = ling_graphics::shading::lit_vertex(base, m.normals[ib], b, eye, &lights, &sp);
                    let lc = ling_graphics::shading::lit_vertex(base, m.normals[ic], c, eye, &lights, &sp);
                    // Near-plane clip (keeps large straddling tiles instead of dropping them).
                    let poly = near_clip_poly(&[(a,la,da),(b,lb,db),(c,lc,dc)], near);
                    if poly.len() < 3 { continue; }
                    let proj: Vec<(f32,f32,f32,u32)> = poly.iter().map(|(p,col)| {
                        let (sx,sy,pz)=self.camera.project(p[0],p[1],p[2]);
                        (sx,sy,pz, ling_graphics::shading::pack(*col))
                    }).collect();
                    let depth = proj.iter().map(|v| v.2).sum::<f32>() / proj.len() as f32;
                    let mut k=1;
                    while k+1 < proj.len() {
                        self.depth_queue.push_triangle_g(depth,
                            proj[0].0,proj[0].1,proj[0].3, proj[k].0,proj[k].1,proj[k].3, proj[k+1].0,proj[k+1].1,proj[k+1].3, bands);
                        k+=1;
                    }
                }
            } else {
                // ── flat per-face path (shade_mode 0) ─────────────────────────
                for t in &m.tris {
                    let a = m.verts[t[0] as usize];
                    let b = m.verts[t[1] as usize];
                    let c = m.verts[t[2] as usize];
                    let ux=b[0]-a[0]; let uy=b[1]-a[1]; let uz=b[2]-a[2];
                    let vx=c[0]-a[0]; let vy=c[1]-a[1]; let vz=c[2]-a[2];
                    let normal = [uy*vz-uz*vy, uz*vx-ux*vz, ux*vy-uy*vx];
                    let centroid = [(a[0]+b[0]+c[0])/3.0,(a[1]+b[1]+c[1])/3.0,(a[2]+b[2]+c[2])/3.0];
                    let lit = crate::gfx::light::compute_lit_color(self.color, normal, centroid, &self.lights, self.ambient);
                    let da=self.camera.depth(a[0],a[1],a[2]);
                    let db=self.camera.depth(b[0],b[1],b[2]);
                    let dc=self.camera.depth(c[0],c[1],c[2]);
                    if da<=near && db<=near && dc<=near { continue; } // all behind → drop
                    // Near-plane clip (flat colour, so vertex colour is irrelevant here).
                    let poly = near_clip_poly(&[(a,[0.0;3],da),(b,[0.0;3],db),(c,[0.0;3],dc)], near);
                    if poly.len() < 3 { continue; }
                    let proj: Vec<(f32,f32,f32)> = poly.iter()
                        .map(|(p,_)| self.camera.project(p[0],p[1],p[2])).collect();
                    let depth = proj.iter().map(|v| v.2).sum::<f32>() / proj.len() as f32;
                    let mut k=1;
                    while k+1 < proj.len() {
                        self.depth_queue.push_triangle(depth, lit,
                            proj[0].0,proj[0].1, proj[k].0,proj[k].1, proj[k+1].0,proj[k+1].1);
                        k+=1;
                    }
                }
            }
        }

        if mode == 1 || mode == 2 {
            let color = self.color;
            // small bias so wireframe paints on top of fills in "both" mode
            let bias = if mode == 2 { 0.03 } else { 0.0 };
            for e in &m.edges {
                let mut a = m.verts[e[0] as usize];
                let mut b = m.verts[e[1] as usize];
                let da=self.camera.depth(a[0],a[1],a[2]);
                let db=self.camera.depth(b[0],b[1],b[2]);
                if da<=near && db<=near { continue; }
                if da<=near {
                    let t=(near-da)/(db-da);
                    a=[a[0]+t*(b[0]-a[0]), a[1]+t*(b[1]-a[1]), a[2]+t*(b[2]-a[2])];
                } else if db<=near {
                    let t=(near-da)/(db-da);
                    b=[a[0]+t*(b[0]-a[0]), a[1]+t*(b[1]-a[1]), a[2]+t*(b[2]-a[2])];
                }
                let (sax,say,pa)=self.camera.project(a[0],a[1],a[2]);
                let (sbx,sby,pb)=self.camera.project(b[0],b[1],b[2]);
                let depth=(pa+pb)/2.0 - bias;
                self.depth_queue.push_line(depth, color, sax,say, sbx,sby);
            }
        }
    }
}

/// Near-plane clip of a convex polygon (Sutherland–Hodgman). Each input vertex is
/// `(world_pos, colour_rgb, camera_depth)`; a vertex is kept when `depth > near`.
/// Vertices created on crossing edges interpolate both position and colour, so a
/// large floor/wall tile straddling the near plane is trimmed to its in-front
/// portion rather than dropped wholesale (which made tiles pop out when close).
fn near_clip_poly(vin: &[([f32; 3], [f32; 3], f32)], near: f32) -> Vec<([f32; 3], [f32; 3])> {
    let n = vin.len();
    let mut out: Vec<([f32; 3], [f32; 3])> = Vec::with_capacity(n + 1);
    for i in 0..n {
        let a = &vin[i];
        let b = &vin[(i + 1) % n];
        let ain = a.2 > near;
        let bin = b.2 > near;
        if ain { out.push((a.0, a.1)); }
        if ain != bin {
            let t = (near - a.2) / (b.2 - a.2);
            let lerp3 = |p: [f32; 3], q: [f32; 3]| {
                [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t, p[2] + (q[2] - p[2]) * t]
            };
            out.push((lerp3(a.0, b.0), lerp3(a.1, b.1)));
        }
    }
    out
}
