//! UI input events.

use crate::shortcut::KeyCombo as ShortcutKeyCombo;

#[derive(Debug, Clone, PartialEq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone)]
pub enum Event {
    Click { widget_id: u64 },
    RightClick { x: f32, y: f32 },
    MouseDown { button: MouseButton, x: f32, y: f32 },
    MouseUp { button: MouseButton, x: f32, y: f32 },
    MouseDrag { delta_x: f32, delta_y: f32, x: f32, y: f32 },
    KeyDown { key: String },
    KeyUp { key: String },
    KeyCombo { combo: ShortcutKeyCombo },
    MouseMove { x: f32, y: f32 },
    Resize { width: f32, height: f32 },
    Scroll { delta_x: f32, delta_y: f32 },
    TextInput { text: String },
    CommandTriggered { command_id: String },
}
