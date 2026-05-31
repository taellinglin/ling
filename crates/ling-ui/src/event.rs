//! UI input events.

#[derive(Debug, Clone)]
pub enum Event {
    Click      { widget_id: u64 },
    KeyDown    { key: String },
    KeyUp      { key: String },
    MouseMove  { x: f32, y: f32 },
    Resize     { width: f32, height: f32 },
    Scroll     { delta_x: f32, delta_y: f32 },
    TextInput  { text: String },
}
