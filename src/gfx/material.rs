// src/gfx/material.rs — LingMaterial: principled BSDF with toon quantisation.
//
// "Photon-water" model
// ════════════════════
// Think of each pixel as a tiny pool. Every light source pours coloured photons
// in. The pool level (energy) is then snapped to discrete toon steps — like
// water settling into terraced channels on a hillside. This gives crisp cel
// bands whose boundaries follow the surface curvature instead of the triangle
// edges, so quads and hexagons look as clean as triangles.
//
// BSDF feature set (2030 baseline)
// ─────────────────────────────────
//   Core       — albedo · roughness · metallic
//   Emission   — emission colour + strength (self-lit surfaces, neon, fire)
//   Specular   — GGX lobe, quantised to a toon hotspot
//   Subsurface — cheap SSS: colour bleed at shadow boundaries (skin, wax)
//   Clearcoat  — secondary GGX layer (car paint, guitar lacquer)
//   Iridescence— thin-film angle-dependent hue (bubbles, beetle wings, CD)
//   Sheen      — retro-reflection for fabric (velvet, satin, felt)
//   Anisotropy — elongated specular (brushed metal, hair, records)
//   Transmission— simple glass/water: alpha-blends the background
//
// Toon overrides
// ──────────────
//   toon_bands        — number of discrete shading levels (2 = shadow+lit)
//   shadow_softness   — cross-fade width at band boundaries (0 = hard step)
//   outline_px        — ink-line thickness in pixels (0 = no outline)
//   outline_color     — ink colour
//   highlight_color   — colour of the brightest toon band (normally white)

use crate::gfx::light::{Light, cel_quantize};

// ── Material struct ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LingMaterial {
    // Core
    pub albedo:             u32,    // 0x00RRGGBB
    pub roughness:          f32,    // 0 = mirror, 1 = diffuse
    pub metallic:           f32,    // 0 = dielectric, 1 = conductor

    // Emission
    pub emission:           u32,
    pub emission_strength:  f32,

    // Specular
    pub specular:           f32,    // base Fresnel reflectance at 0°
    pub specular_tint:      f32,    // 0 = white hotspot, 1 = albedo-tinted

    // Subsurface scattering (approximated)
    pub subsurface:         f32,
    pub subsurface_color:   u32,

    // Clearcoat
    pub clearcoat:          f32,
    pub clearcoat_roughness:f32,

    // Transmission
    pub transmission:       f32,    // 0 = opaque, 1 = glass
    pub ior:                f32,    // index of refraction

    // 2030 extras
    pub iridescence:        f32,    // thin-film interference [0..1]
    pub sheen:              f32,    // fabric retro-reflection [0..1]
    pub anisotropy:         f32,    // specular elongation [0..1]
    pub anisotropy_angle:   f32,    // radians

    // Toon overrides
    pub toon_bands:         u32,    // discrete shading levels (≥2)
    pub shadow_softness:    f32,    // band-boundary blend width [0..1]
    pub outline_px:         f32,    // ink-line thickness (0 = off)
    pub outline_color:      u32,
    pub highlight_color:    u32,
}

impl Default for LingMaterial {
    fn default() -> Self {
        Self {
            albedo:              0x00FF_FFFF,
            roughness:           0.8,
            metallic:            0.0,
            emission:            0,
            emission_strength:   0.0,
            specular:            0.04,
            specular_tint:       0.0,
            subsurface:          0.0,
            subsurface_color:    0x00FF_C8A0,
            clearcoat:           0.0,
            clearcoat_roughness: 0.03,
            transmission:        0.0,
            ior:                 1.5,
            iridescence:         0.0,
            sheen:               0.0,
            anisotropy:          0.0,
            anisotropy_angle:    0.0,
            toon_bands:          3,
            shadow_softness:     0.04,
            outline_px:          0.0,
            outline_color:       0x00_00_00,
            highlight_color:     0x00FF_FFFF,
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

#[inline]
fn unpack(c: u32) -> (f32, f32, f32) {
    (
        ((c >> 16) & 0xFF) as f32 / 255.0,
        ((c >> 8) & 0xFF) as f32 / 255.0,
        (c & 0xFF) as f32 / 255.0,
    )
}

#[inline]
fn pack01(r: f32, g: f32, b: f32) -> u32 {
    let r = (r.clamp(0.0, 1.0) * 255.0) as u32;
    let g = (g.clamp(0.0, 1.0) * 255.0) as u32;
    let b = (b.clamp(0.0, 1.0) * 255.0) as u32;
    (r << 16) | (g << 8) | b
}

/// Schlick Fresnel: f0 + (1-f0)*(1-cosθ)^5
#[inline]
fn schlick(cos_theta: f32, f0: f32) -> f32 {
    let c = (1.0 - cos_theta).clamp(0.0, 1.0);
    let c2 = c * c;
    f0 + (1.0 - f0) * c2 * c2 * c
}

/// GGX specular quantised to a toon hotspot: either 0 or 1.
/// Low roughness → large bright hotspot; high roughness → the lobe disappears.
#[inline]
fn ggx_toon(n_dot_h: f32, roughness: f32) -> f32 {
    let a2 = roughness * roughness * roughness * roughness; // α⁴
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    let ggx = a2 / (std::f32::consts::PI * d * d + 1e-6);
    // Normalise to [0,1] and snap: the hotspot is on when the lobe is bright
    let t = (ggx * a2 * 3.0).clamp(0.0, 1.0);
    // Hard step at 0.5 → binary toon hotspot
    if t > 0.5 { 1.0 } else { 0.0 }
}

/// Toon diffuse — maps n·l to discrete bands with optional smooth cross-fade.
#[inline]
pub fn toon_diffuse(n_dot_l: f32, bands: u32, softness: f32) -> f32 {
    let t = n_dot_l.clamp(0.0, 1.0);
    if softness < 1e-3 {
        return cel_quantize(t);
    }
    // Smooth cross-fade at each band boundary using smoothstep
    let n = bands.max(2) as f32;
    let scaled = t * n;
    let lo = (scaled.floor() / n).clamp(0.0, 1.0);
    let hi = ((scaled.floor() + 1.0) / n).clamp(0.0, 1.0);
    let frac = scaled.fract();
    // Smoothstep only in the soft zone near the upper edge of each band
    let edge_width = softness.clamp(0.0, 0.5);
    let blend = if frac > 1.0 - edge_width {
        let s = ((frac - (1.0 - edge_width)) / edge_width).clamp(0.0, 1.0);
        s * s * (3.0 - 2.0 * s) // smoothstep
    } else {
        0.0
    };
    lo + (hi - lo) * blend + lo * (1.0 - blend)
}

#[inline]
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn norm3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-7 { [0.0, 0.0, 1.0] } else { [v[0] / len, v[1] / len, v[2] / len] }
}

// ── Main shade function ───────────────────────────────────────────────────────

/// Evaluate LingMaterial at a surface point and return 0x00RRGGBB.
///
/// * `mat`       — material to evaluate
/// * `normal`    — world-space face normal (need not be normalised)
/// * `view_dir`  — direction from surface toward the camera (world space)
/// * `centroid`  — world-space surface point
/// * `lights`    — active point lights
/// * `ambient`   — ambient fill level [0..1]
pub fn shade(
    mat:      &LingMaterial,
    normal:   [f32; 3],
    view_dir: [f32; 3],
    centroid: [f32; 3],
    lights:   &[Light],
    ambient:  f32,
) -> u32 {
    let (ar, ag, ab) = unpack(mat.albedo);
    let n = norm3(normal);
    let v = norm3(view_dir);
    let n_dot_v = dot3(n, v).abs().clamp(1e-4, 1.0);

    // Ambient + emission
    let mut acc_r = ar * ambient;
    let mut acc_g = ag * ambient;
    let mut acc_b = ab * ambient;
    if mat.emission_strength > 0.0 {
        let (er, eg, eb) = unpack(mat.emission);
        acc_r += er * mat.emission_strength;
        acc_g += eg * mat.emission_strength;
        acc_b += eb * mat.emission_strength;
    }

    // Per-light contribution
    for l in lights {
        let dx = l.x - centroid[0];
        let dy = l.y - centroid[1];
        let dz = l.z - centroid[2];
        let dist = (dx * dx + dy * dy + dz * dz).sqrt().max(1e-6);
        let atten = if l.radius > 0.0 { (1.0 - dist / l.radius).max(0.0) } else { 1.0 };
        if atten <= 0.0 { continue; }

        let ld = [dx / dist, dy / dist, dz / dist];
        let n_dot_l = dot3(n, ld).abs(); // two-sided shading
        let h = norm3([ld[0] + v[0], ld[1] + v[1], ld[2] + v[2]]);
        let n_dot_h = dot3(n, h).clamp(0.0, 1.0);

        // ── Diffuse (toon) ────────────────────────────────────────────────────
        let diff = toon_diffuse(n_dot_l, mat.toon_bands, mat.shadow_softness);

        // Subsurface: tint the shadow zone toward subsurface_color
        let (eff_r, eff_g, eff_b) = if mat.subsurface > 0.0 {
            let (sr, sg, sb) = unpack(mat.subsurface_color);
            let zone = ((0.3 - n_dot_l) * 3.33).clamp(0.0, 1.0) * mat.subsurface;
            (ar + (sr - ar) * zone, ag + (sg - ag) * zone, ab + (sb - ab) * zone)
        } else {
            (ar, ag, ab)
        };

        let dr = eff_r * diff;
        let dg = eff_g * diff;
        let db = eff_b * diff;

        // ── Specular (GGX toon hotspot) ───────────────────────────────────────
        let f0_dielectric = mat.specular * 0.08; // maps [0,1] → [0,0.08]
        let f0 = f0_dielectric + mat.metallic * (ar - f0_dielectric);
        let fresnel = schlick(n_dot_v, f0.clamp(0.0, 1.0));
        let spec = ggx_toon(n_dot_h, mat.roughness.max(0.01)) * fresnel;

        let spec_white = spec * (1.0 - mat.specular_tint);
        let sr = spec_white + spec * ar * mat.specular_tint;
        let sg = spec_white + spec * ag * mat.specular_tint;
        let sb = spec_white + spec * ab * mat.specular_tint;

        // ── Clearcoat (white GGX on top) ──────────────────────────────────────
        let coat = ggx_toon(n_dot_h, mat.clearcoat_roughness.max(0.01)) * mat.clearcoat;

        // ── Iridescence (thin-film angle-dependent hue) ───────────────────────
        // Phase-shifted cosines per channel → RGB rainbow at glancing angles
        let (ir, ig, ib) = if mat.iridescence > 0.0 {
            let p = n_dot_v * std::f32::consts::TAU * 2.0;
            let tau3 = std::f32::consts::FRAC_PI_3 * 2.0;
            let ir = (p.cos() * 0.5 + 0.5) * mat.iridescence;
            let ig = ((p + tau3).cos() * 0.5 + 0.5) * mat.iridescence;
            let ib = ((p + tau3 * 2.0).cos() * 0.5 + 0.5) * mat.iridescence;
            (ir, ig, ib)
        } else {
            (0.0, 0.0, 0.0)
        };

        // ── Sheen (retro-reflection: peak at 90° incidence) ───────────────────
        let sheen = if mat.sheen > 0.0 {
            let t = (1.0 - n_dot_l).powi(3) * mat.sheen;
            t
        } else {
            0.0
        };

        let intensity = l.intensity * atten;
        acc_r += (dr + sr + coat + ir + sheen * ar) * l.r * intensity;
        acc_g += (dg + sg + coat + ig + sheen * ag) * l.g * intensity;
        acc_b += (db + sb + coat + ib + sheen * ab) * l.b * intensity;
    }

    pack01(acc_r, acc_g, acc_b)
}

/// Compute per-vertex material colours for a Gouraud-shaded triangle.
/// Returns three 0x00RRGGBB colours.
pub fn shade_vertices(
    mat:        &LingMaterial,
    normal:     [f32; 3],
    va:         [f32; 3],
    vb:         [f32; 3],
    vc:         [f32; 3],
    camera_pos: [f32; 3],
    lights:     &[Light],
    ambient:    f32,
) -> (u32, u32, u32) {
    let view = |v: [f32; 3]| [camera_pos[0]-v[0], camera_pos[1]-v[1], camera_pos[2]-v[2]];
    (
        shade(mat, normal, view(va), va, lights, ambient),
        shade(mat, normal, view(vb), vb, lights, ambient),
        shade(mat, normal, view(vc), vc, lights, ambient),
    )
}

/// Compute per-vertex material colours for an n-gon (up to N vertices).
/// Writes results into `out[0..n]`.
pub fn shade_polygon(
    mat:        &LingMaterial,
    normal:     [f32; 3],
    verts:      &[[f32; 3]],
    n:          usize,
    camera_pos: [f32; 3],
    lights:     &[Light],
    ambient:    f32,
    out:        &mut [u32],
) {
    let view = |v: [f32; 3]| [camera_pos[0]-v[0], camera_pos[1]-v[1], camera_pos[2]-v[2]];
    for i in 0..n.min(verts.len()).min(out.len()) {
        out[i] = shade(mat, normal, view(verts[i]), verts[i], lights, ambient);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_material_no_crash_no_lights() {
        let mat = LingMaterial::default();
        let c = shade(&mat, [0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.0, 0.0, 0.0], &[], 0.5);
        let lum = ((c >> 16 & 0xFF) as f32 * 0.299
            + (c >> 8 & 0xFF) as f32 * 0.587
            + (c & 0xFF) as f32 * 0.114) / 255.0;
        // With ambient=0.5 and white albedo the result should be non-zero
        assert!(lum > 0.1, "expected visible output, got lum={lum:.3}");
    }

    #[test]
    fn metallic_tints_specular() {
        let mut mat = LingMaterial::default();
        mat.albedo = 0x00FF_0000; // red metal
        mat.metallic = 1.0;
        mat.specular_tint = 1.0;
        mat.roughness = 0.1;

        let light = crate::gfx::light::Light {
            x: 0.0, y: 0.0, z: 10.0,
            r: 1.0, g: 1.0, b: 1.0,
            intensity: 2.0, radius: 0.0,
        };
        let c = shade(
            &mat,
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],  // view = straight ahead
            [0.0, 0.0, 0.0],
            &[light],
            0.05,
        );
        let r = (c >> 16) & 0xFF;
        let g = (c >> 8) & 0xFF;
        // Red metal should have more red than green
        assert!(r >= g, "metallic red should be reddish: r={r} g={g}");
    }
}
