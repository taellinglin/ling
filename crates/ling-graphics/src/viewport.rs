use crate::color::Color;

/// A logical render area (pixel dimensions + clear color).
///
/// This is renderer-agnostic: it can back a software framebuffer,
/// and later map to a real GPU/window viewport.
#[derive(Debug, Clone)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub clear_color: Color,
}

impl Viewport {
    pub fn new(width: u32, height: u32, clear_color: Color) -> Self {
        Self { width, height, clear_color }
    }
}

