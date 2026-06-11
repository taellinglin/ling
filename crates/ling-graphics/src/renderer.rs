use glam::{Vec3, Vec4, Mat4};
use crate::camera::Camera3D;
use crate::color::Color;
use crate::geometry::{Mesh, Vertex};
use crate::material::{Material, AlphaMode};
use crate::scene::Transform;
use crate::font::FontAtlas;

// ── Frame buffer ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct FrameBuffer {
    pub width: u32,
    pub height: u32,
    /// RGBA bytes, row-major, top-left origin.
    pub pixels: Vec<u8>,
    pub depth: Vec<f32>,
}

impl FrameBuffer {
    pub fn new(width: u32, height: u32) -> Self {
        let n = (width * height) as usize;
        Self { width, height, pixels: vec![0u8; n * 4], depth: vec![f32::INFINITY; n] }
    }

    pub fn clear(&mut self, color: Color) {
        let b = color.to_rgba_bytes();
        for chunk in self.pixels.chunks_exact_mut(4) { chunk.copy_from_slice(&b); }
        self.depth.fill(f32::INFINITY);
    }

    pub fn set_pixel(&mut self, x: u32, y: u32, color: Color, depth: f32) {
        if x >= self.width || y >= self.height { return; }
        let idx = (y * self.width + x) as usize;
        if depth >= self.depth[idx] { return; }
        self.depth[idx] = depth;
        let base = idx * 4;
        let b = color.to_rgba_bytes();
        self.pixels[base..base + 4].copy_from_slice(&b);
    }

    pub fn blend_pixel(&mut self, x: u32, y: u32, color: Color, depth: f32) {
        if x >= self.width || y >= self.height { return; }
        let idx = (y * self.width + x) as usize;
        if depth >= self.depth[idx] { return; }
        let base = idx * 4;
        let dst = Color::new(
            self.pixels[base]     as f32 / 255.0,
            self.pixels[base + 1] as f32 / 255.0,
            self.pixels[base + 2] as f32 / 255.0,
            self.pixels[base + 3] as f32 / 255.0,
        );
        let blended = crate::color::BlendMode::Normal.blend(color, dst);
        let b = blended.to_rgba_bytes();
        self.pixels[base..base + 4].copy_from_slice(&b);
        if color.a >= 1.0 { self.depth[idx] = depth; }
    }
}

// ── Renderer trait ────────────────────────────────────────────────────────────

pub trait Renderer {
    fn begin_frame(&mut self, width: u32, height: u32, clear_color: Color);
    fn draw_mesh(&mut self, mesh: &Mesh, transform: &Transform, material: &Material, camera: &Camera3D);
    fn draw_text(&mut self, text: &str, world_pos: Vec3, font: &mut FontAtlas, px: f32, color: Color, camera: &Camera3D);
    fn end_frame(&mut self) -> &FrameBuffer;
}

// ── Software rasterizer ───────────────────────────────────────────────────────

#[allow(dead_code)] // `color` carried for future flat-shaded path (not yet read)
struct ScreenVert {
    x: f32, y: f32,
    z: f32,       // NDC depth in [−1, 1]
    inv_w: f32,   // 1/clip.w for perspective-correct interpolation
    u: f32, v: f32,
    color: Color,
}

pub struct SoftwareRenderer {
    fb: FrameBuffer,
    light_dir: Vec3,
    ambient: f32,
}

impl SoftwareRenderer {
    pub fn new() -> Self {
        Self {
            fb: FrameBuffer::new(1, 1),
            light_dir: Vec3::new(0.5, -1.0, -0.5).normalize(),
            ambient: 0.2,
        }
    }

    pub fn set_light(&mut self, dir: Vec3) { self.light_dir = dir.normalize(); }

    fn project(&self, mvp: Mat4, v: &Vertex) -> Option<ScreenVert> {
        let clip = mvp * Vec4::new(v.position.x, v.position.y, v.position.z, 1.0);
        if clip.w.abs() < 1e-6 { return None; }
        let inv_w = 1.0 / clip.w;
        let ndc = clip.truncate() * inv_w;
        if ndc.z < -1.0 || ndc.z > 1.0 { return None; }
        let x = (ndc.x + 1.0) * 0.5 * self.fb.width  as f32;
        let y = (1.0 - (ndc.y + 1.0) * 0.5) * self.fb.height as f32;
        Some(ScreenVert { x, y, z: ndc.z, inv_w, u: v.uv.x * inv_w, v: v.uv.y * inv_w, color: v.color })
    }

    fn rasterize(&mut self, sv: [&ScreenVert; 3], material: &Material, model_mat: Mat4, normals: [Vec3; 3]) {
        let w = self.fb.width as i32;
        let h = self.fb.height as i32;

        let min_x = sv.iter().map(|v| v.x as i32).min().unwrap().max(0);
        let max_x = sv.iter().map(|v| v.x as i32).max().unwrap().min(w - 1);
        let min_y = sv.iter().map(|v| v.y as i32).min().unwrap().max(0);
        let max_y = sv.iter().map(|v| v.y as i32).max().unwrap().min(h - 1);
        if min_x > max_x || min_y > max_y { return; }

        let edge = |a: &ScreenVert, b: &ScreenVert, px: f32, py: f32| -> f32 {
            (b.x - a.x) * (py - a.y) - (b.y - a.y) * (px - a.x)
        };

        let area = edge(sv[0], sv[1], sv[2].x, sv[2].y);
        if area.abs() < 1.0 { return; }

        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let fx = px as f32 + 0.5;
                let fy = py as f32 + 0.5;
                let w0 = edge(sv[1], sv[2], fx, fy);
                let w1 = edge(sv[2], sv[0], fx, fy);
                let w2 = edge(sv[0], sv[1], fx, fy);

                if (area > 0.0 && w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0)
                    || (area < 0.0 && w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0)
                {
                    let b0 = w0 / area;
                    let b1 = w1 / area;
                    let b2 = w2 / area;

                    let depth = b0 * sv[0].z + b1 * sv[1].z + b2 * sv[2].z;

                    // Perspective-correct UV
                    let inv_w = b0 * sv[0].inv_w + b1 * sv[1].inv_w + b2 * sv[2].inv_w;
                    let u = (b0 * sv[0].u + b1 * sv[1].u + b2 * sv[2].u) / inv_w;
                    let v = (b0 * sv[0].v + b1 * sv[1].v + b2 * sv[2].v) / inv_w;

                    // Interpolate and transform world normal
                    let n_local = normals[0] * b0 + normals[1] * b1 + normals[2] * b2;
                    let n_world = (model_mat * Vec4::new(n_local.x, n_local.y, n_local.z, 0.0))
                        .truncate().normalize_or_zero();

                    // Diffuse lighting
                    let ndotl = (-self.light_dir).dot(n_world).max(0.0);
                    let light = (self.ambient + (1.0 - self.ambient) * ndotl).min(1.0);

                    let albedo = material.sample_albedo(u, v);
                    let lit = Color::new(albedo.r * light, albedo.g * light, albedo.b * light, albedo.a);
                    let lit = (lit + material.emissive).clamp();

                    match material.alpha_mode {
                        AlphaMode::Mask { cutoff } => {
                            if lit.a < cutoff { continue; }
                            self.fb.set_pixel(px as u32, py as u32, lit, depth);
                        }
                        AlphaMode::Blend | AlphaMode::Premultiplied => {
                            self.fb.blend_pixel(px as u32, py as u32, lit, depth);
                        }
                        AlphaMode::Opaque => {
                            self.fb.set_pixel(px as u32, py as u32, lit, depth);
                        }
                    }
                }
            }
        }
    }
}

impl Default for SoftwareRenderer { fn default() -> Self { Self::new() } }

impl Renderer for SoftwareRenderer {
    fn begin_frame(&mut self, width: u32, height: u32, clear_color: Color) {
        if self.fb.width != width || self.fb.height != height {
            self.fb = FrameBuffer::new(width, height);
        }
        self.fb.clear(clear_color);
    }

    fn draw_mesh(&mut self, mesh: &Mesh, transform: &Transform, material: &Material, camera: &Camera3D) {
        let model = transform.matrix();
        let mvp   = camera.view_proj() * model;

        let sv: Vec<Option<ScreenVert>> = mesh.vertices.iter().map(|v| self.project(mvp, v)).collect();

        for tri in mesh.indices.chunks(3) {
            let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            if let (Some(a), Some(b), Some(c)) = (&sv[i0], &sv[i1], &sv[i2]) {
                let normals = [
                    mesh.vertices[i0].normal,
                    mesh.vertices[i1].normal,
                    mesh.vertices[i2].normal,
                ];
                self.rasterize([a, b, c], material, model, normals);
            }
        }
    }

    fn draw_text(&mut self, text: &str, world_pos: Vec3, font: &mut FontAtlas, px: f32, color: Color, camera: &Camera3D) {
        let text_mesh = crate::font::generate_text_mesh(font, text, px, color);
        let mut mat = Material::new(color);
        mat.albedo_texture = Some(font.texture.clone());
        let t = Transform::from_translation(world_pos);
        self.draw_mesh(&text_mesh, &t, &mat, camera);
    }

    fn end_frame(&mut self) -> &FrameBuffer { &self.fb }
}
