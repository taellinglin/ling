//! ling-ui — UI for Ling: retained-mode widgets, flex layout, application controls
//! (context menus, menu bars, hotkeys, split panes, property grids, command palette),
//! plus the 2030 holographic vector toolkit.

pub mod anim;
pub mod dock;
pub mod event;
pub mod holo;
pub mod layout;
pub mod menu;
pub mod palette;
pub mod property;
pub mod renderer;
pub mod shortcut;
pub mod tooltip;
pub mod widget;
pub mod widgets;

pub use anim::{ease, Easing, Spring, Tween};
pub use dock::{SplitDirection, SplitPane};
pub use event::{Event, MouseButton};
pub use layout::{simple_layout, FlexDirection, LayoutNode, Style};
pub use menu::{MenuBar, MenuCategory, MenuItem, MenuItemKind, MenuPopup};
pub use palette::{CommandItem, CommandPalette};
pub use property::{Dropdown, PropertySection, Toggle, ValueDrag};
pub use renderer::SoftwareRenderer;
pub use shortcut::{KeyCombo, Modifiers, ShortcutBinding, ShortcutRegistry};
pub use tooltip::Tooltip;
pub use widget::{Button, Label, View, Widget};
pub use widgets::{
    command_palette_draw, menu_bar_draw, menu_popup_draw, splitter_draw, toggle_draw, tooltip_draw,
    value_drag_draw, Draw, Rgba,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
