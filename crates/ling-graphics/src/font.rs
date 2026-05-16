use std::collections::HashMap;
use glam::{Vec2, Vec3};
use fontdue::{Font, FontSettings};
use crate::color::Color;
use crate::geometry::{Vertex, Mesh};
use crate::material::TextureData;

// ── Glyph metrics after rasterization ────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct GlyphInfo {
    /// UV rect in the atlas texture.
    pub uv_min: Vec2,
    pub uv_max: Vec2,
    /// Pixel size of the glyph bitmap.
    pub size: Vec2,
    /// Left/bottom bearing in pixels.
    pub bearing: Vec2,
    /// How far to advance the cursor after this glyph.
    pub advance: f32,
}

impl GlyphInfo {
    fn empty(advance: f32) -> Self {
        Self { uv_min: Vec2::ZERO, uv_max: Vec2::ZERO, size: Vec2::ZERO, bearing: Vec2::ZERO, advance }
    }
}

// ── Font atlas ────────────────────────────────────────────────────────────────

pub struct FontAtlas {
    font: Font,
    pub texture: TextureData,
    glyphs: HashMap<(u32, u32), GlyphInfo>, // (char as u32, px.to_bits())
    cursor_x: usize,
    cursor_y: usize,
    row_height: usize,
}

impl FontAtlas {
    /// Load a TrueType/OpenType font from raw bytes. `atlas_size` must be a power of two.
    pub fn from_bytes(font_data: &[u8], atlas_size: usize) -> Result<Self, String> {
        let font = Font::from_bytes(font_data, FontSettings::default())
            .map_err(|e| e.to_string())?;
        let texture = TextureData::new(atlas_size, atlas_size);
        Ok(Self {
            font,
            texture,
            glyphs: HashMap::new(),
            cursor_x: 1,
            cursor_y: 1,
            row_height: 0,
        })
    }

    /// Retrieve (and rasterize if missing) glyph info for a char at a given pixel size.
    pub fn get_or_rasterize(&mut self, c: char, px: f32) -> GlyphInfo {
        let key = (c as u32, px.to_bits());
        if let Some(g) = self.glyphs.get(&key) { return g.clone(); }
        self.rasterize_glyph(c, px);
        self.glyphs.get(&key).cloned().unwrap_or_else(|| GlyphInfo::empty(px * 0.5))
    }

    fn rasterize_glyph(&mut self, c: char, px: f32) {
        let (metrics, bitmap) = self.font.rasterize(c, px);

        if metrics.width == 0 || metrics.height == 0 {
            let key = (c as u32, px.to_bits());
            self.glyphs.insert(key, GlyphInfo::empty(metrics.advance_width));
            return;
        }

        let aw = self.texture.width;
        let ah = self.texture.height;

        if self.cursor_x + metrics.width + 1 > aw {
            self.cursor_x = 1;
            self.cursor_y += self.row_height + 1;
            self.row_height = 0;
        }

        if self.cursor_y + metrics.height + 1 > ah {
            // Atlas full — insert a dummy entry so we don't retry endlessly
            let key = (c as u32, px.to_bits());
            self.glyphs.insert(key, GlyphInfo::empty(metrics.advance_width));
            return;
        }

        for gy in 0..metrics.height {
            for gx in 0..metrics.width {
                let alpha = bitmap[gy * metrics.width + gx];
                let dx = self.cursor_x + gx;
                let dy = self.cursor_y + gy;
                let idx = (dy * aw + dx) * 4;
                self.texture.data[idx]     = 255;
                self.texture.data[idx + 1] = 255;
                self.texture.data[idx + 2] = 255;
                self.texture.data[idx + 3] = alpha;
            }
        }

        let uv_min = Vec2::new(
            self.cursor_x as f32 / aw as f32,
            self.cursor_y as f32 / ah as f32,
        );
        let uv_max = Vec2::new(
            (self.cursor_x + metrics.width) as f32 / aw as f32,
            (self.cursor_y + metrics.height) as f32 / ah as f32,
        );

        self.row_height = self.row_height.max(metrics.height);
        self.cursor_x += metrics.width + 1;

        let key = (c as u32, px.to_bits());
        self.glyphs.insert(key, GlyphInfo {
            uv_min,
            uv_max,
            size: Vec2::new(metrics.width as f32, metrics.height as f32),
            bearing: Vec2::new(metrics.xmin as f32, metrics.ymin as f32),
            advance: metrics.advance_width,
        });
    }
}

// ── Text mesh generation ──────────────────────────────────────────────────────

/// Generate a flat Mesh (quads) for `text` in the XY plane, using the given font atlas.
/// The mesh origin is at the left baseline. Scale with a Transform to place in 3D/4D space.
pub fn generate_text_mesh(atlas: &mut FontAtlas, text: &str, px: f32, color: Color) -> Mesh {
    let mut vertices = Vec::new();
    let mut indices  = Vec::new();
    let mut cursor_x = 0.0f32;

    for ch in text.chars() {
        let info = atlas.get_or_rasterize(ch, px);
        if info.size.x > 0.0 && info.size.y > 0.0 {
            let x0 = cursor_x + info.bearing.x;
            let y0 = info.bearing.y;
            let x1 = x0 + info.size.x;
            let y1 = y0 + info.size.y;

            let base = vertices.len() as u32;
            vertices.push(Vertex { position: Vec3::new(x0, y0, 0.0), normal: Vec3::Z, uv: info.uv_min,                                        color, tangent: Vec3::X });
            vertices.push(Vertex { position: Vec3::new(x1, y0, 0.0), normal: Vec3::Z, uv: Vec2::new(info.uv_max.x, info.uv_min.y), color, tangent: Vec3::X });
            vertices.push(Vertex { position: Vec3::new(x1, y1, 0.0), normal: Vec3::Z, uv: info.uv_max,                                        color, tangent: Vec3::X });
            vertices.push(Vertex { position: Vec3::new(x0, y1, 0.0), normal: Vec3::Z, uv: Vec2::new(info.uv_min.x, info.uv_max.y), color, tangent: Vec3::X });

            indices.extend_from_slice(&[base, base+1, base+2, base, base+2, base+3]);
        }
        cursor_x += info.advance;
    }

    Mesh::new(vertices, indices)
}

/// Measure the pixel-width of a string without rasterizing.
pub fn measure_text(atlas: &mut FontAtlas, text: &str, px: f32) -> f32 {
    text.chars().map(|c| atlas.get_or_rasterize(c, px).advance).sum()
}
