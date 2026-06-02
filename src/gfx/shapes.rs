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
        let mut midpoint = |nm: &mut Mesh, a: u32, b: u32, mid: &mut std::collections::HashMap<(u32,u32),u32>| -> u32 {
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
        _ => return None,
    };
    m.transform(c);
    Some(m)
}

impl GfxState {
    /// Render a world-space mesh through the depth queue.
    /// mode: 0 filled, 1 wireframe, 2 both.
    pub fn emit_mesh(&mut self, m: &Mesh, mode: i32) {
        let near = -self.camera.zdist + 0.05;

        if mode == 0 || mode == 2 {
            for t in &m.tris {
                let a = m.verts[t[0] as usize];
                let b = m.verts[t[1] as usize];
                let c = m.verts[t[2] as usize];
                // world-space normal & centroid
                let ux=b[0]-a[0]; let uy=b[1]-a[1]; let uz=b[2]-a[2];
                let vx=c[0]-a[0]; let vy=c[1]-a[1]; let vz=c[2]-a[2];
                let normal = [uy*vz-uz*vy, uz*vx-ux*vz, ux*vy-uy*vx];
                let centroid = [(a[0]+b[0]+c[0])/3.0,(a[1]+b[1]+c[1])/3.0,(a[2]+b[2]+c[2])/3.0];
                let lit = crate::gfx::light::compute_lit_color(self.color, normal, centroid, &self.lights, self.ambient);
                let da=self.camera.depth(a[0],a[1],a[2]);
                let db=self.camera.depth(b[0],b[1],b[2]);
                let dc=self.camera.depth(c[0],c[1],c[2]);
                if da<=near || db<=near || dc<=near { continue; }
                let (sax,say,pa)=self.camera.project(a[0],a[1],a[2]);
                let (sbx,sby,pb)=self.camera.project(b[0],b[1],b[2]);
                let (scx,scy,pc)=self.camera.project(c[0],c[1],c[2]);
                let depth=(pa+pb+pc)/3.0;
                self.depth_queue.push_triangle(depth, lit, sax,say, sbx,sby, scx,scy);
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
