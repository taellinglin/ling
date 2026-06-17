//! Software renderer stub for ling-ui.

use crate::widget::{RenderCtx, Widget};

pub struct SoftwareRenderer {
    pub width: u32,
    pub height: u32,
    pub buffer: Vec<u32>,
}

impl SoftwareRenderer {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            buffer: vec![0xFF_18_18_18; (width * height) as usize],
        }
    }

    pub fn clear(&mut self, color: u32) {
        self.buffer.fill(color);
    }

    pub fn render_widget(&mut self, widget: &dyn Widget) {
        let mut ctx = RenderCtx { width: self.width as f32, height: self.height as f32 };
        widget.render(&mut ctx);
    }

    pub fn pixels(&self) -> &[u32] {
        &self.buffer
    }
}
