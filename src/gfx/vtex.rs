// src/gfx/vtex.rs — vector texture primitives
//
// Every function draws a geometric pattern on a 3-D plane defined by:
//   centre (cx,cy,cz)  +  U tangent (ux,uy,uz)  +  V tangent (vx,vy,vz)
//
// Lines are pushed to the DepthQueue with DEPTH_BIAS subtracted so they sort
// in front of (i.e., are drawn after) solid surfaces at the same depth.

use crate::gfx::{depth::DepthQueue, camera::Camera3D};
use std::f32::consts::TAU;

/// How much to shift vtex lines toward the camera vs. same-depth surfaces.
const DEPTH_BIAS: f32 = -0.06;

// ── helpers ──────────────────────────────────────────────────────────────────

#[inline]
fn p2w(
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    u:f32, v:f32,
) -> (f32,f32,f32) {
    (cx + ux*u + vx*v,
     cy + uy*u + vy*v,
     cz + uz*u + vz*v)
}

/// Push a 3-D line segment with near-plane clipping.
/// One endpoint behind the camera is lerped to the near plane;
/// both behind = segment discarded entirely.
#[inline]
fn push_seg(
    q: &mut DepthQueue, cam: &Camera3D, color: u32,
    mut ax:f32, mut ay:f32, mut az:f32,
    mut bx:f32, mut by:f32, mut bz:f32,
) {
    let near = -cam.zdist + 0.05;
    let da = cam.depth(ax, ay, az);
    let db = cam.depth(bx, by, bz);

    // Cull if both are behind the near plane
    if da <= near && db <= near { return; }

    // Clip A to near plane
    if da <= near {
        let t = (near - da) / (db - da);
        ax += t * (bx - ax);
        ay += t * (by - ay);
        az += t * (bz - az);
    }
    // Clip B to near plane
    else if db <= near {
        let t = (near - da) / (db - da);
        bx = ax + t * (bx - ax);
        by = ay + t * (by - ay);
        bz = az + t * (bz - az);
    }

    let (sax, say, da2) = cam.project(ax, ay, az);
    let (sbx, sby, db2) = cam.project(bx, by, bz);
    q.push_line((da2 + db2) * 0.5 + DEPTH_BIAS, color, sax, say, sbx, sby);
}

/// 3-channel sine-wave colour cycle — 120° apart.
#[inline]
pub fn cycle(phase: f32) -> u32 {
    let r = (phase.sin()              * 127.0 + 128.0) as u32;
    let g = ((phase+2.094).sin()      * 127.0 + 128.0) as u32;
    let b = ((phase+4.189).sin()      * 127.0 + 128.0) as u32;
    (r<<16)|(g<<8)|b
}

// ── draw_grid ─────────────────────────────────────────────────────────────────
/// `cols`×`rows` rectilinear grid. `cw`/`ch` = cell width/height in world units.
/// Colour cycles per line proportional to position + time.
pub fn draw_grid(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    cols:usize, rows:usize, cw:f32, ch:f32,
    fr:f32, hue:f32,
) {
    let hw = cols as f32 * cw * 0.5;
    let hh = rows as f32 * ch * 0.5;
    for i in 0..=cols {
        let u = i as f32 * cw - hw;
        let color = cycle(fr*0.03 + hue + u*0.15);
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u,-hh);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u, hh);
        push_seg(q,cam,color, ax,ay,az, bx,by,bz);
    }
    for j in 0..=rows {
        let v = j as f32 * ch - hh;
        let color = cycle(fr*0.03 + hue + v*0.15);
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz,-hw,v);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, hw,v);
        push_seg(q,cam,color, ax,ay,az, bx,by,bz);
    }
}

// ── draw_rings ────────────────────────────────────────────────────────────────
/// Concentric N-sided polygon rings.  Each ring rotates by an additional
/// `twist` radians relative to the previous — creates a vortex/galaxy feel.
pub fn draw_rings(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    n_rings:usize, n_sides:usize, max_r:f32, twist:f32,
    fr:f32, hue:f32,
) {
    let step = max_r / n_rings as f32;
    for ring in 0..n_rings {
        let r      = (ring+1) as f32 * step;
        let rot    = fr*0.012 + ring as f32 * twist;
        let color  = cycle(fr*0.025 + hue + ring as f32 * 0.52);
        for side in 0..n_sides {
            let a0 = rot + side     as f32 * TAU / n_sides as f32;
            let a1 = rot + (side+1) as f32 * TAU / n_sides as f32;
            let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a0.cos()*r, a0.sin()*r);
            let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a1.cos()*r, a1.sin()*r);
            push_seg(q,cam,color, ax,ay,az, bx,by,bz);
        }
    }
}

// ── draw_star ─────────────────────────────────────────────────────────────────
/// Star polygon that alternates between outer and inner radius.
/// `rot_speed` is radians per frame (the star spins).
pub fn draw_star(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    n_points:usize, r_outer:f32, r_inner:f32, rot_speed:f32,
    fr:f32, hue:f32,
) {
    let total = n_points * 2;
    let rot   = fr * rot_speed;
    let color = cycle(fr*0.025 + hue);
    let mut pu = 0_f32; let mut pv = 0_f32; let mut first = true;
    for i in 0..=total {
        let a = rot + i as f32 * TAU / total as f32;
        let r = if i % 2 == 0 { r_outer } else { r_inner };
        let u = a.cos() * r;
        let v = a.sin() * r;
        if !first {
            let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, pu,pv);
            let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u,v);
            push_seg(q,cam,color, ax,ay,az, bx,by,bz);
        }
        pu=u; pv=v; first=false;
    }
}

// ── draw_spiral ───────────────────────────────────────────────────────────────
/// Archimedean spiral: `n_turns` revolutions from centre to `max_r`.
pub fn draw_spiral(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    n_turns:f32, max_r:f32, n_steps:usize,
    fr:f32, hue:f32,
) {
    let base_rot = fr * 0.008;
    let mut prev: Option<(f32,f32,f32)> = None;
    for step in 0..=n_steps {
        let t = step as f32 / n_steps as f32;
        let a = base_rot + t * n_turns * TAU;
        let r = t * max_r;
        let (wx,wy,wz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a.cos()*r, a.sin()*r);
        let color = cycle(fr*0.02 + hue + t*5.0);
        if let Some((px,py,pz)) = prev {
            push_seg(q,cam,color, px,py,pz, wx,wy,wz);
        }
        prev = Some((wx,wy,wz));
    }
}

// ── draw_flower ───────────────────────────────────────────────────────────────
/// Flower of Life: 7 circles (centre + 6 surrounding at radius distance).
/// Each circle is drawn as an `n_sides`-gon.  The whole flower slowly rotates.
pub fn draw_flower(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    radius:f32, n_sides:usize,
    fr:f32, hue:f32,
) {
    // Centre circle
    draw_ngon(q,cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, 0.0,0.0, radius, n_sides, fr, hue);
    // 6 petals
    for i in 0..6 {
        let a  = i as f32 * TAU / 6.0;
        let ou = a.cos() * radius;
        let ov = a.sin() * radius;
        draw_ngon(q,cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, ou,ov, radius, n_sides, fr, hue + i as f32*0.45);
    }
}

fn draw_ngon(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    ou:f32, ov:f32, r:f32, n_sides:usize,
    fr:f32, hue:f32,
) {
    let color = cycle(fr*0.022 + hue);
    let rot   = fr * 0.009;
    for i in 0..n_sides {
        let a0 = rot + i     as f32 * TAU / n_sides as f32;
        let a1 = rot + (i+1) as f32 * TAU / n_sides as f32;
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, ou+a0.cos()*r, ov+a0.sin()*r);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, ou+a1.cos()*r, ov+a1.sin()*r);
        push_seg(q,cam,color, ax,ay,az, bx,by,bz);
    }
}

// ── draw_halftone ─────────────────────────────────────────────────────────────
/// Dithered surface fill using a grid of short cross-hatch marks.
///
/// Divides the plane into `cols`×`rows` cells; inside each cell draws a
/// pair of small diagonal strokes whose length is proportional to `density`
/// (0 = invisible, 1 = full cell).  Use `density` values < 0.5 for a sparse
/// pointillist look, 0.7–1.0 for heavy cross-hatching.
///
/// `phase` is the colour cycle offset; `fr` drives slow colour animation.
pub fn draw_halftone(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    cols:usize, rows:usize, cell_w:f32, cell_h:f32,
    density:f32,
    fr:f32, hue:f32,
) {
    let hw = cols as f32 * cell_w * 0.5;
    let hh = rows as f32 * cell_h * 0.5;
    let half = density * 0.5;

    for row in 0..rows {
        let v_c = (row as f32 + 0.5) * cell_h - hh; // cell centre V
        for col in 0..cols {
            let u_c = (col as f32 + 0.5) * cell_w - hw; // cell centre U

            let phase = fr * 0.022 + hue
                + (col as f32 * 0.31 + row as f32 * 0.19); // dither offset
            let color = cycle(phase);

            // Half-arm in U and V directions from the cell centre
            let hu = cell_w * half;
            let hv = cell_h * half;

            // Diagonal 1: (-hu,-hv) → (+hu,+hv)
            let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u_c-hu, v_c-hv);
            let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u_c+hu, v_c+hv);
            push_seg(q, cam, color, ax,ay,az, bx,by,bz);

            // Diagonal 2: (+hu,-hv) → (-hu,+hv)  — cross-hatch
            let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u_c+hu, v_c-hv);
            let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u_c-hu, v_c+hv);
            push_seg(q, cam, color, ax,ay,az, bx,by,bz);
        }
    }
}

// ── draw_tessellated ──────────────────────────────────────────────────────────
/// 4-D-inspired tessellated fill.
///
/// Fills a planar region with a grid of small triangles whose vertex positions
/// are displaced by a sine-wave field — giving an organic, animated "3-D
/// dithering" appearance reminiscent of 4-D projection artefacts.
///
/// `cell` = grid spacing, `amplitude` = vertex displacement (0.1–0.4 of cell),
/// `freq` = number of sine waves across the region (3–8 looks good).
pub fn draw_tessellated(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    cols:usize, rows:usize, cell:f32,
    amplitude:f32, freq:f32,
    fr:f32, hue:f32,
) {
    let hw = cols as f32 * cell * 0.5;
    let hh = rows as f32 * cell * 0.5;
    let time = fr * 0.016;

    // Displaced vertex at grid intersection (ci, ri)
    let vert = |ci: usize, ri: usize| -> (f32, f32) {
        let u0 = ci as f32 * cell - hw;
        let v0 = ri as f32 * cell - hh;
        let d = amplitude * cell
            * (freq * (u0 * 0.5 + time)).sin()
            * (freq * (v0 * 0.5 + time * 0.7)).cos();
        (u0 + d, v0 - d)
    };

    for ri in 0..rows {
        for ci in 0..cols {
            // Four corners of this cell (possibly displaced)
            let (u00,v00) = vert(ci,   ri  );
            let (u10,v10) = vert(ci+1, ri  );
            let (u01,v01) = vert(ci,   ri+1);
            let (u11,v11) = vert(ci+1, ri+1);

            let phase = time + hue + (ci as f32 * 0.28 + ri as f32 * 0.37);
            let c0 = cycle(phase);
            let c1 = cycle(phase + 1.047);  // 60° offset

            // Lower-left triangle: 00 → 10 → 01
            let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u00,v00);
            let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u10,v10);
            let (ex,ey,ez) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u01,v01);
            push_seg(q, cam, c0, ax,ay,az, bx,by,bz);
            push_seg(q, cam, c0, bx,by,bz, ex,ey,ez);
            push_seg(q, cam, c0, ex,ey,ez, ax,ay,az);

            // Upper-right triangle: 10 → 11 → 01
            let (fx,fy,fz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u11,v11);
            push_seg(q, cam, c1, bx,by,bz, fx,fy,fz);
            push_seg(q, cam, c1, fx,fy,fz, ex,ey,ez);
            // shared edge bx→ex already drawn above — skip to save lines
        }
    }
}

// ── draw_lotus ────────────────────────────────────────────────────────────────
/// Thai lotus (dok bua): n_petals pointed kite-shaped petals arranged radially.
/// Each petal is a kite from two base-ring points to an outer tip.
/// An inner hub ring is drawn in the centre.
pub fn draw_lotus(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    r_inner:f32, r_outer:f32, n_petals:usize,
    fr:f32, hue:f32,
) {
    let rot    = fr * 0.006;
    let spread = TAU / (n_petals as f32 * 2.0); // half the arc per petal

    for i in 0..n_petals {
        let a_mid   = rot + i as f32 * TAU / n_petals as f32;
        let a_left  = a_mid - spread * 0.55;
        let a_right = a_mid + spread * 0.55;
        let color   = cycle(fr * 0.020 + hue + i as f32 * (TAU / n_petals as f32));

        // Three petal points: left base, outer tip, right base
        let (alx,aly,alz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz,
            a_left.cos()*r_inner,  a_left.sin()*r_inner);
        let (arx,ary,arz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz,
            a_right.cos()*r_inner, a_right.sin()*r_inner);
        let (tipx,tipy,tipz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz,
            a_mid.cos()*r_outer, a_mid.sin()*r_outer);

        push_seg(q, cam, color, alx,aly,alz, tipx,tipy,tipz);
        push_seg(q, cam, color, tipx,tipy,tipz, arx,ary,arz);
        push_seg(q, cam, color, arx,ary,arz, alx,aly,alz);
    }

    // Inner hub circle (24-gon)
    let hub_color = cycle(fr * 0.020 + hue + 2.1);
    for i in 0..24_usize {
        let a0 = rot + i     as f32 * TAU / 24.0;
        let a1 = rot + (i+1) as f32 * TAU / 24.0;
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a0.cos()*r_inner, a0.sin()*r_inner);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a1.cos()*r_inner, a1.sin()*r_inner);
        push_seg(q, cam, hub_color, ax,ay,az, bx,by,bz);
    }
}

// ── draw_chakra ───────────────────────────────────────────────────────────────
/// Thai Dhamma chakra (Wheel of the Law).
/// Outer rim (36-gon) + inner hub (12-gon) + n_spokes spokes + rim tick marks.
pub fn draw_chakra(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    r:f32, n_spokes:usize,
    fr:f32, hue:f32,
) {
    let rot   = fr * 0.009;
    let r_hub = r * 0.18;

    // Outer rim (36-gon)
    let c_rim = cycle(fr * 0.018 + hue);
    for i in 0..36_usize {
        let a0 = rot + i as f32 * TAU / 36.0;
        let a1 = rot + (i+1) as f32 * TAU / 36.0;
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a0.cos()*r, a0.sin()*r);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a1.cos()*r, a1.sin()*r);
        push_seg(q, cam, c_rim, ax,ay,az, bx,by,bz);
    }

    // Inner hub circle (12-gon)
    let c_hub = cycle(fr * 0.018 + hue + 1.8);
    for i in 0..12_usize {
        let a0 = rot + i as f32 * TAU / 12.0;
        let a1 = rot + (i+1) as f32 * TAU / 12.0;
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a0.cos()*r_hub, a0.sin()*r_hub);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a1.cos()*r_hub, a1.sin()*r_hub);
        push_seg(q, cam, c_hub, ax,ay,az, bx,by,bz);
    }

    // Spokes + inter-spoke tick marks on the rim
    for i in 0..n_spokes {
        let a_spoke = rot + i as f32 * TAU / n_spokes as f32;
        let c_spoke = cycle(fr * 0.018 + hue + i as f32 * 0.72);

        // Spoke: hub → rim
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz,
            a_spoke.cos()*r_hub, a_spoke.sin()*r_hub);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz,
            a_spoke.cos()*r, a_spoke.sin()*r);
        push_seg(q, cam, c_spoke, ax,ay,az, bx,by,bz);

        // Tick between this spoke and the next
        let a_tick = rot + (i as f32 + 0.5) * TAU / n_spokes as f32;
        let r_tick0 = r * 0.87;
        let (tx0,ty0,tz0) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz,
            a_tick.cos()*r_tick0, a_tick.sin()*r_tick0);
        let (tx1,ty1,tz1) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz,
            a_tick.cos()*r, a_tick.sin()*r);
        push_seg(q, cam, c_rim, tx0,ty0,tz0, tx1,ty1,tz1);
    }
}

// ── draw_yantra ───────────────────────────────────────────────────────────────
/// Thai yantra: nested pairs of interlocked up/down equilateral triangles
/// (like a Sri Yantra simplified), enclosed in a two-ring bhupura (earth square)
/// with traditional T-shaped gates on all four sides.
pub fn draw_yantra(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    n_layers:usize, max_r:f32,
    fr:f32, hue:f32,
) {
    let rot = fr * 0.003; // very slow rotation — yantra is nearly static

    // Nested interlocked triangle pairs, scaling inward
    for layer in 0..n_layers {
        let scale  = max_r * (1.0 - layer as f32 * 0.20);
        let c_up   = cycle(fr * 0.015 + hue + layer as f32 * 0.65);
        let c_down = cycle(fr * 0.015 + hue + layer as f32 * 0.65 + 3.14);

        // Up triangle (apex at top of pattern)
        draw_equi_tri(q, cam, cx,cy,cz, ux,uy,uz, vx,vy,vz,
            rot + 3.14159 * 0.5,   // apex pointing toward +V
            scale, c_up);

        // Down triangle (rotated 60° = inverted)
        draw_equi_tri(q, cam, cx,cy,cz, ux,uy,uz, vx,vy,vz,
            rot + 3.14159 * 0.5 + TAU / 6.0,
            scale * 0.92, c_down);
    }

    // Bhupura: outer + inner square with 4 T-gates
    let sq  = max_r * 1.40;
    let sq2 = max_r * 1.18;
    let gate = max_r * 0.28;
    let c_sq = cycle(fr * 0.015 + hue + 5.5);
    draw_bhupura(q, cam, cx,cy,cz, ux,uy,uz, vx,vy,vz, sq, sq2, gate, c_sq);
}

// ── draw_yantra helpers ───────────────────────────────────────────────────────

/// Draw a single equilateral triangle centred at the plane's origin,
/// with one apex at angle `apex_rot` from the +U axis.
fn draw_equi_tri(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    apex_rot:f32, r:f32, color:u32,
) {
    for i in 0..3 {
        let a0 = apex_rot + i     as f32 * TAU / 3.0;
        let a1 = apex_rot + (i+1) as f32 * TAU / 3.0;
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a0.cos()*r, a0.sin()*r);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a1.cos()*r, a1.sin()*r);
        push_seg(q, cam, color, ax,ay,az, bx,by,bz);
    }
}

/// Outer + inner axis-aligned squares with T-gates on all 4 sides.
///
/// Gate shape (top side as example):
///   ─────  gap  ─────   ← outer square top
///        │     │         ← gate posts (from outer to inner)
///   ──────────────────   ← inner square top (intact)
fn draw_bhupura(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    sq:f32, sq2:f32, gate:f32, color:u32,
) {
    // Outer square (4 full sides)
    let oc = [(-sq,-sq),(sq,-sq),(sq,sq),(-sq,sq)];
    for i in 0..4 {
        let (u0,v0) = oc[i]; let (u1,v1) = oc[(i+1)%4];
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u0,v0);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u1,v1);
        push_seg(q, cam, color, ax,ay,az, bx,by,bz);
    }

    // Inner square (4 full sides)
    let ic = [(-sq2,-sq2),(sq2,-sq2),(sq2,sq2),(-sq2,sq2)];
    for i in 0..4 {
        let (u0,v0) = ic[i]; let (u1,v1) = ic[(i+1)%4];
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u0,v0);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u1,v1);
        push_seg(q, cam, color, ax,ay,az, bx,by,bz);
    }

    // 8 gate posts: 2 per side, connecting inner to outer square.
    // The "gate" opening is the gap between the two posts on each outer side.
    //   Bottom: posts at U=±gate, V from -sq2 to -sq
    //   Top:    posts at U=±gate, V from  sq2 to  sq
    //   Left:   posts at V=±gate, U from -sq  to -sq2
    //   Right:  posts at V=±gate, U from  sq2 to  sq
    let posts: &[(f32,f32,f32,f32)] = &[
        (-gate, -sq2,  -gate, -sq ),   // bottom-left post
        ( gate, -sq2,   gate, -sq ),   // bottom-right post
        (-gate,  sq2,  -gate,  sq ),   // top-left post
        ( gate,  sq2,   gate,  sq ),   // top-right post
        (-sq,  -gate,  -sq2, -gate),   // left-bottom post
        (-sq,   gate,  -sq2,  gate),   // left-top post
        ( sq2, -gate,   sq,  -gate),   // right-bottom post
        ( sq2,  gate,   sq,   gate),   // right-top post
    ];
    for &(u0,v0,u1,v1) in posts {
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u0,v0);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u1,v1);
        push_seg(q, cam, color, ax,ay,az, bx,by,bz);
    }
}

// ── draw_spiked_cog ───────────────────────────────────────────────────────────
/// A mechanical gear with sharp triangular spike teeth.
/// Draws: outer spike crown, body ring, inner hub ring, spokes.
///
/// `n_teeth`  — number of spikes around the crown
/// `r_body`   — radius of the flat gear body ring
/// `r_spike`  — radius of spike tips (> r_body)
/// `r_hub`    — inner hub radius
/// `n_spokes` — spokes from hub to body (0 = none)
pub fn draw_spiked_cog(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    n_teeth:usize, r_body:f32, r_spike:f32, r_hub:f32, n_spokes:usize,
    fr:f32, hue:f32,
) {
    let rot   = fr * 0.007;
    let arc   = TAU / n_teeth as f32;
    let half  = arc * 0.38; // base half-width of each tooth

    let c_body  = cycle(fr * 0.018 + hue);
    let c_spike = cycle(fr * 0.018 + hue + 1.05);
    let c_hub   = cycle(fr * 0.018 + hue + 2.10);

    // Spike teeth: triangles from body ring to spike tip
    for i in 0..n_teeth {
        let a_mid = rot + i as f32 * arc;
        let a_l   = a_mid - half;
        let a_r   = a_mid + half;

        // Base-left → tip
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a_l.cos()*r_body, a_l.sin()*r_body);
        let (tx,ty,tz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a_mid.cos()*r_spike, a_mid.sin()*r_spike);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a_r.cos()*r_body, a_r.sin()*r_body);
        push_seg(q, cam, c_spike, ax,ay,az, tx,ty,tz);
        push_seg(q, cam, c_spike, tx,ty,tz, bx,by,bz);
        // Base between teeth (body ring arc segment)
        push_seg(q, cam, c_body, bx,by,bz, ax,ay,az);
    }

    // Inner body ring (smooth polygon)
    let sides_body = (n_teeth * 3).max(24);
    for i in 0..sides_body {
        let a0 = rot + i     as f32 * TAU / sides_body as f32;
        let a1 = rot + (i+1) as f32 * TAU / sides_body as f32;
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a0.cos()*r_body*0.72, a0.sin()*r_body*0.72);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a1.cos()*r_body*0.72, a1.sin()*r_body*0.72);
        push_seg(q, cam, c_body, ax,ay,az, bx,by,bz);
    }

    // Hub ring
    let sides_hub = 16usize;
    for i in 0..sides_hub {
        let a0 = rot + i     as f32 * TAU / sides_hub as f32;
        let a1 = rot + (i+1) as f32 * TAU / sides_hub as f32;
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a0.cos()*r_hub, a0.sin()*r_hub);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a1.cos()*r_hub, a1.sin()*r_hub);
        push_seg(q, cam, c_hub, ax,ay,az, bx,by,bz);
    }

    // Spokes
    for i in 0..n_spokes {
        let a = rot + i as f32 * TAU / n_spokes as f32;
        let c = cycle(fr * 0.018 + hue + i as f32 * (TAU / n_spokes as f32));
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a.cos()*r_hub, a.sin()*r_hub);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, a.cos()*r_body*0.70, a.sin()*r_body*0.70);
        push_seg(q, cam, c, ax,ay,az, bx,by,bz);
    }
}

// ── draw_torii ────────────────────────────────────────────────────────────────
/// A Japanese torii gate silhouette drawn on a plane (U=right, V=up).
/// `width` = total gate width, `height` = pillar height.
/// Draws two pillars, a straight nuki crossbar, and a curved kasagi (top beam).
pub fn draw_torii(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    width:f32, height:f32,
    fr:f32, hue:f32,
) {
    let hw  = width  * 0.5;
    let hh  = height * 0.5;
    let rot = fr * 0.0003; // very slow drift
    let overhang = width * 0.14; // kasagi extends beyond pillars
    let nuki_v   = hh * 0.55;   // nuki crossbar height (55% up)

    let c0 = cycle(fr * 0.012 + hue);
    let c1 = cycle(fr * 0.012 + hue + 1.2);
    let c2 = cycle(fr * 0.012 + hue + 2.4);

    // Left pillar
    {
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot - hw, -hh);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot - hw,  hh);
        push_seg(q, cam, c0, ax,ay,az, bx,by,bz);
    }
    // Right pillar
    {
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot + hw, -hh);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot + hw,  hh);
        push_seg(q, cam, c0, ax,ay,az, bx,by,bz);
    }

    // Nuki — straight crossbar at 55% height
    {
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot - hw, nuki_v);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot + hw, nuki_v);
        push_seg(q, cam, c1, ax,ay,az, bx,by,bz);
    }

    // Kasagi — curved top beam (arc approximated as polyline)
    let n_seg = 20usize;
    let kasagi_v = hh + height * 0.06; // slightly above pillar tops
    let sag      = height * 0.04;      // upward curve at ends
    let mut prev: Option<(f32,f32,f32)> = None;
    for i in 0..=n_seg {
        let t  = i as f32 / n_seg as f32;
        let u  = rot + (t - 0.5) * (width + overhang * 2.0);
        // Parabolic uplift at both ends
        let up = sag * (1.0 - 4.0 * (t - 0.5) * (t - 0.5));
        let v  = kasagi_v + up;
        let (wx,wy,wz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, u, v);
        let color = cycle(fr * 0.012 + hue + 2.4 + t * 0.8);
        if let Some((px,py,pz)) = prev {
            push_seg(q, cam, color, px,py,pz, wx,wy,wz);
        }
        prev = Some((wx,wy,wz));
    }

    // Kasagi bottom edge (straight)
    {
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot - hw - overhang, kasagi_v - height * 0.05);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot + hw + overhang, kasagi_v - height * 0.05);
        push_seg(q, cam, c2, ax,ay,az, bx,by,bz);
    }

    // Shimagi (rectangular beam between kasagi and nuki)
    {
        let sv = hh + height * 0.01;
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot - hw, sv);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot + hw, sv);
        push_seg(q, cam, c1, ax,ay,az, bx,by,bz);
    }
}

// ── draw_pagoda ───────────────────────────────────────────────────────────────
/// Multi-tiered pagoda silhouette drawn on a plane (U=right, V=up).
/// Each tier is narrower than the one below. Eaves overhang at each tier top.
///
/// `n_tiers`  — number of storeys (3–7)
/// `base_w`   — half-width of the bottom tier
/// `tier_h`   — height of each tier
/// `taper`    — width reduction factor per tier (0.65–0.80)
/// `eave_out` — eave overhang as fraction of tier half-width (0.2–0.4)
pub fn draw_pagoda(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    n_tiers:usize, base_w:f32, tier_h:f32, taper:f32, eave_out:f32,
    fr:f32, hue:f32,
) {
    let mut w    = base_w;
    let mut v0   = -(n_tiers as f32 * tier_h * 0.5); // bottom of pagoda
    let rot      = fr * 0.0002;

    for tier in 0..n_tiers {
        let tf   = tier as f32;
        let v1   = v0 + tier_h;                   // top of this tier
        let eave = w * eave_out;
        let c_wall = cycle(fr * 0.010 + hue + tf * 0.62);
        let c_eave = cycle(fr * 0.010 + hue + tf * 0.62 + 1.8);

        // Left wall
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot - w, v0);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot - w, v1);
        push_seg(q, cam, c_wall, ax,ay,az, bx,by,bz);
        // Right wall
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot + w, v0);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot + w, v1);
        push_seg(q, cam, c_wall, ax,ay,az, bx,by,bz);
        // Floor of this tier
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot - w, v0);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot + w, v0);
        push_seg(q, cam, c_wall, ax,ay,az, bx,by,bz);

        // Eave: slightly wider than tier, with upturned corners
        let e_y   = v1 + tier_h * 0.08;  // eave sits above tier top
        let e_tip = e_y + tier_h * 0.06; // upturned corner lift
        let ew    = w + eave;

        // Left eave: left tip → centre
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot - ew, e_tip);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot,      e_y);
        push_seg(q, cam, c_eave, ax,ay,az, bx,by,bz);
        // Right eave: centre → right tip
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot,      e_y);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot + ew, e_tip);
        push_seg(q, cam, c_eave, ax,ay,az, bx,by,bz);
        // Eave soffit (underside, horizontal)
        let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot - ew, e_y - tier_h * 0.03);
        let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot + ew, e_y - tier_h * 0.03);
        push_seg(q, cam, c_eave, ax,ay,az, bx,by,bz);

        v0  = v1;
        w  *= taper;
    }

    // Finial spire above the top tier
    let spire_h = tier_h * 1.4;
    let c_spire = cycle(fr * 0.010 + hue + 4.5);
    let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot - w, v0);
    let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot + w, v0);
    let (sx,sy,sz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz, rot,      v0 + spire_h);
    push_seg(q, cam, c_spire, ax,ay,az, sx,sy,sz);
    push_seg(q, cam, c_spire, sx,sy,sz, bx,by,bz);
    push_seg(q, cam, c_spire, ax,ay,az, bx,by,bz);
}

// ── Thai letter glyph table ───────────────────────────────────────────────────
// 8 simplified Thai-inspired stroke shapes.
// Coordinates are in [-0.45, 0.45] cell-space (u=right, v=up).
// Each entry is a slice of (u0,v0,u1,v1) stroke segments.

static GLYPHS: &[&[(f32,f32,f32,f32)]] = &[
    // 0 — ก-like: bracket + top loop
    &[(-0.38, 0.12,  0.38, 0.12),   // top horizontal bar
      ( 0.38, 0.12,  0.38,-0.30),   // right descender
      (-0.08,-0.30,  0.38,-0.30),   // bottom bar (short)
      (-0.38, 0.12, -0.38, 0.44),   // left riser
      (-0.38, 0.44,  0.06, 0.44)],  // loop cap

    // 1 — ข-like: D-shape
    &[(-0.30, 0.30,  0.18, 0.30),
      ( 0.18, 0.30,  0.38, 0.00),
      ( 0.38, 0.00,  0.18,-0.30),
      ( 0.18,-0.30, -0.30,-0.30),
      (-0.30,-0.30, -0.30, 0.30),
      (-0.38, 0.44,  0.02, 0.44)],  // cap

    // 2 — ค-like: diamond + descender
    &[(-0.32, 0.00,  0.00, 0.32),
      ( 0.00, 0.32,  0.32, 0.00),
      ( 0.32, 0.00,  0.00,-0.32),
      ( 0.00,-0.32, -0.32, 0.00),
      ( 0.00,-0.32,  0.00,-0.44)],

    // 3 — ง-like: S-curve
    &[(-0.10, 0.44,  0.34, 0.22),
      ( 0.34, 0.22,  0.00, 0.00),
      ( 0.00, 0.00, -0.34,-0.22),
      (-0.34,-0.22,  0.10,-0.44)],

    // 4 — จ-like: open C + tail
    &[( 0.34, 0.32, -0.18, 0.32),
      (-0.18, 0.32, -0.36, 0.00),
      (-0.36, 0.00, -0.18,-0.30),
      (-0.18,-0.30,  0.18,-0.30),
      ( 0.18,-0.30,  0.18,-0.44)],

    // 5 — ต-like: box + tail
    &[(-0.32, 0.30,  0.32, 0.30),
      ( 0.32, 0.30,  0.32, 0.00),
      (-0.32, 0.00,  0.32, 0.00),
      (-0.32, 0.30, -0.32, 0.00),
      ( 0.00, 0.00,  0.00,-0.44),
      ( 0.00,-0.44,  0.28,-0.44)],

    // 6 — ล-like: small box + descender
    &[(-0.26, 0.32,  0.26, 0.32),
      ( 0.26, 0.32,  0.26, 0.02),
      ( 0.26, 0.02, -0.26, 0.02),
      (-0.26, 0.02, -0.26, 0.32),
      ( 0.00, 0.02,  0.00,-0.44)],

    // 7 — ว-like: arc
    &[(-0.36, 0.32,  0.36, 0.32),
      ( 0.36, 0.32,  0.44, 0.00),
      ( 0.44, 0.00,  0.36,-0.30),
      ( 0.36,-0.30,  0.00,-0.44)],
];

// ── draw_letter_rain ──────────────────────────────────────────────────────────
/// Thai-letter Matrix rain on a plane.
///
/// `n_cols` columns scroll at varying speeds.  Within each column `n_visible`
/// letters are visible, bright at the head and fading to black at the tail.
/// Odd columns are offset by half a row height (honeycomb grid — gives the
/// "hexagonal cylinder" texture the eye expects from a prism surface).
///
/// `speed` is the base scroll rate in world units per frame.
pub fn draw_letter_rain(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    n_cols: usize, n_visible: usize,
    col_w: f32, row_h: f32,
    speed: f32,
    fr: f32, hue: f32,
) {
    let half_w   = (n_cols as f32 * col_w) * 0.5;
    let half_h   = (n_visible as f32 * row_h) * 0.5;
    let total_h  = n_visible as f32 * row_h;
    let lw       = col_w  * 0.36;  // letter width scale
    let lh       = row_h  * 0.38;  // letter height scale

    for ci in 0..n_cols {
        let u = ci as f32 * col_w - half_w + col_w * 0.5;

        // Each column has its own speed and phase (based on column index)
        let speed_k = speed * (1.0 + 0.45 * (ci as f32 * 1.618_f32).sin());
        let phase_k = ci as f32 * 2.73;

        // Hex-grid offset: odd columns shift down by half a cell
        let hex_offset = if ci % 2 == 1 { row_h * 0.5 } else { 0.0 };

        // Scroll position (increases with time, wraps)
        let scroll = (fr * speed_k + phase_k * 11.3 + hex_offset)
            .rem_euclid(total_h);

        for ri in 0..n_visible {
            // V of this letter after scrolling (top = +half_h, scrolls downward)
            let v_from_top = (ri as f32 * row_h + scroll).rem_euclid(total_h);
            let v = half_h - v_from_top;
            if v < -half_h - row_h { continue; }

            // Brightness: head of stream (top) = 1.0, tail = 0.0
            let brightness = 1.0 - v_from_top / total_h;
            if brightness < 0.07 { continue; }

            // Letter selection: slow flicker per column + time
            let time_slot = (fr * 0.028 + phase_k) as usize;
            let glyph_idx = ci.wrapping_mul(7)
                .wrapping_add(ri.wrapping_mul(13))
                .wrapping_add(time_slot) % GLYPHS.len();

            let color = cycle_dark(
                fr * 0.018 + hue + ci as f32 * 0.33 + brightness * 1.8,
                brightness,
            );

            for &(du0, dv0, du1, dv1) in GLYPHS[glyph_idx] {
                let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz,
                    u + du0*lw, v + dv0*lh);
                let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz,
                    u + du1*lw, v + dv1*lh);
                push_seg(q, cam, color, ax,ay,az, bx,by,bz);
            }
        }
    }
}

/// Brightness-scaled colour cycle.  `brightness` ∈ [0..1].
#[inline]
fn cycle_dark(phase: f32, brightness: f32) -> u32 {
    let b = brightness.clamp(0.0, 1.0);
    let r = ((phase.sin()              * 127.0 + 128.0) * b) as u32;
    let g = (((phase+2.094).sin()      * 127.0 + 128.0) * b) as u32;
    let b_ch = (((phase+4.189).sin()   * 127.0 + 128.0) * b) as u32;
    (r.min(255)<<16)|(g.min(255)<<8)|b_ch.min(255)
}

// ── draw_hyperbolic_uv ────────────────────────────────────────────────────────
/// Poincaré-disc-inspired UV grid for the ceiling.
///
/// Draws `n_circles` concentric circles spaced with a hyperbolic tanh ramp
/// (bunched near the outer edge) and `n_rays` radial lines that carry a subtle
/// wobble — together they look like the UV parameter-lines of a hyperbolic
/// surface projected onto the plane.
pub fn draw_hyperbolic_uv(
    q: &mut DepthQueue, cam: &Camera3D,
    cx:f32,cy:f32,cz:f32,
    ux:f32,uy:f32,uz:f32,
    vx:f32,vy:f32,vz:f32,
    max_r: f32, n_circles: usize, n_rays: usize,
    fr: f32, hue: f32,
) {
    let rot  = fr * 0.004;
    let c_ramp = 2.8_f32;   // controls how aggressively circles bunch at boundary

    // Hyperbolic-spaced concentric circles
    for k in 1..=n_circles {
        let t = k as f32 / (n_circles + 1) as f32;
        let r = max_r * (c_ramp * t).tanh() / c_ramp.tanh();
        let color = cycle(fr * 0.014 + hue + k as f32 * 0.35);
        let n_seg = 48usize;
        for i in 0..n_seg {
            let a0 = rot + i as f32 * TAU / n_seg as f32;
            let a1 = rot + (i+1) as f32 * TAU / n_seg as f32;
            let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz,
                a0.cos()*r, a0.sin()*r);
            let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz,
                a1.cos()*r, a1.sin()*r);
            push_seg(q, cam, color, ax,ay,az, bx,by,bz);
        }
    }

    // Radial rays with hyperbolic radial spacing + slight angular wobble
    let wobble = 0.07_f32;
    for k in 0..n_rays {
        let base_angle = rot * 0.5 + k as f32 * TAU / n_rays as f32;
        let color = cycle(fr * 0.014 + hue + 1.8 + k as f32 * TAU / n_rays as f32);
        let n_seg = 18usize;
        for i in 0..n_seg {
            let t0 = i as f32 / n_seg as f32;
            let t1 = (i+1) as f32 / n_seg as f32;
            // tanh radial remap: inner points compressed, outer stretched
            let r0 = max_r * (c_ramp * t0).tanh() / c_ramp.tanh();
            let r1 = max_r * (c_ramp * t1).tanh() / c_ramp.tanh();
            // Subtle angular wobble — rays curve slightly like geodesics
            let a0 = base_angle + wobble * (t0 * TAU).sin();
            let a1 = base_angle + wobble * (t1 * TAU).sin();
            let (ax,ay,az) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz,
                a0.cos()*r0, a0.sin()*r0);
            let (bx,by,bz) = p2w(cx,cy,cz, ux,uy,uz, vx,vy,vz,
                a1.cos()*r1, a1.sin()*r1);
            push_seg(q, cam, color, ax,ay,az, bx,by,bz);
        }
    }
}
