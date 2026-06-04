//! ling-ui — UI for Ling: retained-mode widgets + flex layout, plus the 2030
//! holographic toolkit ([`anim`] easings/springs/tweens and [`holo`] vector
//! stroke font + sci-fi frame geometry + immediate-mode hit-testing).

pub mod widget;
pub mod layout;
pub mod event;
pub mod renderer;
pub mod anim;
pub mod holo;

pub use widget::{View, Widget, Label, Button};
pub use layout::{FlexDirection, LayoutNode, Style, simple_layout};
pub use event::Event;
pub use renderer::SoftwareRenderer;
pub use anim::{Easing, Spring, Tween, ease};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
