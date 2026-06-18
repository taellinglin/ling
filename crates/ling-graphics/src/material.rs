use crate::color::Color;

/// CPU-side RGBA image data.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TextureData {
    pub width: usize,
    pub height: usize,
    /// RGBA bytes, row-major, top-left origin.
    pub data: Vec<u8>,
}

impl TextureData {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, data: vec![0u8; width * height * 4] }
    }

    pub fn from_color(width: usize, height: usize, color: Color) -> Self {
        let bytes = color.to_rgba_bytes();
        let data = bytes
            .iter()
            .cloned()
            .cycle()
            .take(width * height * 4)
            .collect();
        Self { width, height, data }
    }

    pub fn set_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = (y * self.width + x) * 4;
        let b = color.to_rgba_bytes();
        self.data[idx..idx + 4].copy_from_slice(&b);
    }

    pub fn get_pixel(&self, x: usize, y: usize) -> Color {
        if x >= self.width || y >= self.height {
            return Color::TRANSPARENT;
        }
        let idx = (y * self.width + x) * 4;
        Color::new(
            self.data[idx] as f32 / 255.0,
            self.data[idx + 1] as f32 / 255.0,
            self.data[idx + 2] as f32 / 255.0,
            self.data[idx + 3] as f32 / 255.0,
        )
    }

    /// Bilinear sample with UV coordinates in [0, 1].
    pub fn sample(&self, u: f32, v: f32) -> Color {
        let u = u.fract().abs();
        let v = v.fract().abs();
        let fx = u * (self.width as f32 - 1.0);
        let fy = v * (self.height as f32 - 1.0);
        let x0 = fx as usize;
        let y0 = fy as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let c00 = self.get_pixel(x0, y0);
        let c10 = self.get_pixel(x1, y0);
        let c01 = self.get_pixel(x0, y1);
        let c11 = self.get_pixel(x1, y1);
        let top = c00.lerp(c10, tx);
        let bottom = c01.lerp(c11, tx);
        top.lerp(bottom, ty)
    }
}

// ── Alpha mode ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AlphaMode {
    Opaque,
    /// Alpha tested: fragments with alpha < cutoff are discarded.
    Mask {
        cutoff: f32,
    },
    /// Alpha blended (order-dependent).
    Blend,
    /// Premultiplied alpha blending.
    Premultiplied,
}

// ── Material ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Material {
    pub albedo_color: Color,
    pub albedo_texture: Option<TextureData>,
    pub normal_texture: Option<TextureData>,
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: Color,
    pub alpha_mode: AlphaMode,
    /// Colorization: multiply sampled color by this tint.
    pub tint: Color,
    pub double_sided: bool,
}

impl Material {
    pub fn new(color: Color) -> Self {
        Self {
            albedo_color: color,
            albedo_texture: None,
            normal_texture: None,
            metallic: 0.0,
            roughness: 0.8,
            emissive: Color::BLACK,
            alpha_mode: AlphaMode::Opaque,
            tint: Color::WHITE,
            double_sided: false,
        }
    }

    pub fn with_texture(mut self, tex: TextureData) -> Self {
        self.albedo_texture = Some(tex);
        self
    }

    pub fn with_metallic_roughness(mut self, m: f32, r: f32) -> Self {
        self.metallic = m;
        self.roughness = r;
        self
    }

    pub fn with_alpha(mut self, mode: AlphaMode) -> Self {
        self.alpha_mode = mode;
        self
    }

    pub fn with_emissive(mut self, c: Color) -> Self {
        self.emissive = c;
        self
    }

    /// Sample the effective albedo at texture coordinates (u, v).
    pub fn sample_albedo(&self, u: f32, v: f32) -> Color {
        let base = match &self.albedo_texture {
            Some(tex) => tex.sample(u, v),
            None => self.albedo_color,
        };
        base * self.tint
    }
}

impl Default for Material {
    fn default() -> Self {
        Self::new(Color::WHITE)
    }
}
