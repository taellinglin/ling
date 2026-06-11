//! UI widget types.

/// Core trait for all UI elements.
pub trait Widget {
    fn id(&self) -> u64;
    fn render(&self, ctx: &mut RenderCtx);
    fn handle(&mut self, event: &crate::event::Event) -> bool;
}

/// Immediate-mode render context passed to widgets.
pub struct RenderCtx {
    pub width:  f32,
    pub height: f32,
}

/// Root view container — holds a tree of boxed widgets.
pub struct View {
    pub children: Vec<Box<dyn Widget>>,
    #[allow(dead_code)] // reserved for stable per-widget ids (not yet read)
    next_id: u64,
}

impl View {
    pub fn new() -> Self { Self { children: vec![], next_id: 1 } }
    pub fn add(&mut self, w: impl Widget + 'static) { self.children.push(Box::new(w)); }
}

impl Default for View { fn default() -> Self { Self::new() } }

/// A simple text label.
pub struct Label { id: u64, pub text: String }

impl Label {
    pub fn new(id: u64, text: impl Into<String>) -> Self {
        Self { id, text: text.into() }
    }
}

impl Widget for Label {
    fn id(&self) -> u64 { self.id }
    fn render(&self, _ctx: &mut RenderCtx) {}
    fn handle(&mut self, _e: &crate::event::Event) -> bool { false }
}

/// A clickable button.
pub struct Button { id: u64, pub label: String, pub pressed: bool }

impl Button {
    pub fn new(id: u64, label: impl Into<String>) -> Self {
        Self { id, label: label.into(), pressed: false }
    }
}

impl Widget for Button {
    fn id(&self) -> u64 { self.id }
    fn render(&self, _ctx: &mut RenderCtx) {}
    fn handle(&mut self, e: &crate::event::Event) -> bool {
        if let crate::event::Event::Click { widget_id } = e {
            if *widget_id == self.id { self.pressed = true; return true; }
        }
        false
    }
}
