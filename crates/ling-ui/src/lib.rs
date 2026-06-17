//! ling-ui — UI for Ling: retained-mode widgets + flex layout, plus the 2030
//! holographic toolkit ([`anim`] easings/springs/tweens and [`holo`] vector
//! stroke font + sci-fi frame geometry + immediate-mode hit-testing).

pub mod anim;
pub mod event;
pub mod holo;
pub mod layout;
pub mod renderer;
pub mod widget;
pub mod widgets;

pub use anim::{ease, Easing, Spring, Tween};
pub use event::Event;
pub use layout::{simple_layout, FlexDirection, LayoutNode, Style};
pub use renderer::SoftwareRenderer;
pub use widget::{Button, Label, View, Widget};
pub use widgets::{Draw, Rgba};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
