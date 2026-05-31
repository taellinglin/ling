//! ling-ui — retained-mode UI widgets and flex layout for Ling.

pub mod widget;
pub mod layout;
pub mod event;
pub mod renderer;

pub use widget::{View, Widget, Label, Button};
pub use layout::{FlexDirection, LayoutNode, Style, simple_layout};
pub use event::Event;
pub use renderer::SoftwareRenderer;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
