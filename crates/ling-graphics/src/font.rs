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

// ── Direct framebuffer glyph rendering ────────────────────────────────────────
//
// `GlyphFont` rasterizes TrueType/OpenType glyphs with fontdue and alpha-blends
// them straight into a packed `0x00RRGGBB` software framebuffer (the format used
// by the native Ling window). This powers the per-language UI fonts: load a
// TTF once, then draw text in any color with crisp anti-aliased coverage.

/// A loaded font that blits glyphs directly to a `u32` framebuffer.
pub struct GlyphFont {
    font: Font,
    /// Cache of rasterized bitmaps keyed by (char, px.to_bits()) → (metrics, coverage).
    cache: HashMap<(u32, u32), (fontdue::Metrics, Vec<u8>)>,
}

impl GlyphFont {
    /// Load from raw TTF/OTF bytes.
    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        let font = Font::from_bytes(data, FontSettings::default()).map_err(|e| e.to_string())?;
        Ok(Self { font, cache: HashMap::new() })
    }

    /// Load from a file path.
    pub fn from_path(path: &str) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        Self::from_bytes(&bytes)
    }

    fn glyph(&mut self, c: char, px: f32) -> &(fontdue::Metrics, Vec<u8>) {
        let key = (c as u32, px.to_bits());
        self.cache.entry(key).or_insert_with(|| self.font.rasterize(c, px))
    }

    /// Total advance width of `text` at size `px`, in pixels.
    pub fn measure(&mut self, text: &str, px: f32) -> f32 {
        text.chars().map(|c| self.font.metrics(c, px).advance_width).sum()
    }

    /// Draw `text` into a packed `0x00RRGGBB` framebuffer of `w`×`h` pixels.
    ///
    /// `(x, y)` is the top-left of the text box (matching the vector `ui_text`
    /// convention): the baseline is placed `px` below `y` via the font's ascent.
    /// Each glyph's coverage alpha-blends `color` over the existing pixels.
    pub fn draw_text(
        &mut self,
        buf: &mut [u32], w: usize, h: usize,
        x: f32, y: f32, px: f32, color: u32, text: &str,
    ) {
        if w == 0 || h == 0 || px <= 0.0 { return; }
        let lm = self.font.horizontal_line_metrics(px);
        let ascent = lm.map(|m| m.ascent).unwrap_or(px * 0.8);
        let baseline = y + ascent;

        let cr = ((color >> 16) & 0xFF) as f32;
        let cg = ((color >> 8) & 0xFF) as f32;
        let cb = (color & 0xFF) as f32;

        let mut pen_x = x;
        for c in text.chars() {
            let (metrics, bitmap) = self.glyph(c, px).clone();
            if metrics.width > 0 && metrics.height > 0 {
                // Top-left of this glyph's bitmap in screen space.
                let gx0 = (pen_x + metrics.xmin as f32).round() as i32;
                let gy0 = (baseline - metrics.ymin as f32 - metrics.height as f32).round() as i32;
                for gy in 0..metrics.height {
                    let py = gy0 + gy as i32;
                    if py < 0 || py as usize >= h { continue; }
                    let row = py as usize * w;
                    for gx in 0..metrics.width {
                        let px_ = gx0 + gx as i32;
                        if px_ < 0 || px_ as usize >= w { continue; }
                        let a = bitmap[gy * metrics.width + gx] as f32 / 255.0;
                        if a <= 0.0 { continue; }
                        let idx = row + px_ as usize;
                        let dst = buf[idx];
                        let dr = ((dst >> 16) & 0xFF) as f32;
                        let dg = ((dst >> 8) & 0xFF) as f32;
                        let db = (dst & 0xFF) as f32;
                        let nr = (cr * a + dr * (1.0 - a)).min(255.0) as u32;
                        let ng = (cg * a + dg * (1.0 - a)).min(255.0) as u32;
                        let nb = (cb * a + db * (1.0 - a)).min(255.0) as u32;
                        buf[idx] = (nr << 16) | (ng << 8) | nb;
                    }
                }
            }
            pen_x += metrics.advance_width;
        }
    }
}
