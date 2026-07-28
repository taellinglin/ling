//! Context menus, popup menus, submenus, and top application menu bar.

#[derive(Debug, Clone, PartialEq)]
pub enum MenuItemKind {
    Action {
        id: String,
        shortcut: Option<String>,
        checked: Option<bool>,
    },
    Submenu {
        items: Vec<MenuItem>,
    },
    Separator,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    pub label: String,
    pub kind: MenuItemKind,
    pub enabled: bool,
}

impl MenuItem {
    pub fn action(
        label: impl Into<String>,
        id: impl Into<String>,
        shortcut: Option<impl Into<String>>,
    ) -> Self {
        Self {
            label: label.into(),
            kind: MenuItemKind::Action {
                id: id.into(),
                shortcut: shortcut.map(|s| s.into()),
                checked: None,
            },
            enabled: true,
        }
    }

    pub fn checkable(
        label: impl Into<String>,
        id: impl Into<String>,
        checked: bool,
        shortcut: Option<impl Into<String>>,
    ) -> Self {
        Self {
            label: label.into(),
            kind: MenuItemKind::Action {
                id: id.into(),
                shortcut: shortcut.map(|s| s.into()),
                checked: Some(checked),
            },
            enabled: true,
        }
    }

    pub fn submenu(label: impl Into<String>, items: Vec<MenuItem>) -> Self {
        Self {
            label: label.into(),
            kind: MenuItemKind::Submenu { items },
            enabled: true,
        }
    }

    pub fn separator() -> Self {
        Self {
            label: String::new(),
            kind: MenuItemKind::Separator,
            enabled: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MenuPopup {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub items: Vec<MenuItem>,
    pub hovered_index: Option<usize>,
    pub active_submenu: Option<Box<MenuPopup>>,
}

impl MenuPopup {
    pub fn new(x: f32, y: f32, items: Vec<MenuItem>, screen_w: f32, screen_h: f32) -> Self {
        let item_height = 24.0;
        let padding = 8.0;
        let min_w = 160.0;
        let total_h = items.len() as f32 * item_height + padding * 2.0;

        let width = items
            .iter()
            .map(|it| (it.label.len() * 8 + 60) as f32)
            .fold(min_w, f32::max);

        let final_x = if x + width > screen_w {
            (x - width).max(0.0)
        } else {
            x
        };

        let final_y = if y + total_h > screen_h {
            (y - total_h).max(0.0)
        } else {
            y
        };

        Self {
            x: final_x,
            y: final_y,
            width,
            height: total_h,
            items,
            hovered_index: None,
            active_submenu: None,
        }
    }

    pub fn set_hover(&mut self, cursor_x: f32, cursor_y: f32, screen_w: f32, screen_h: f32) {
        if cursor_x >= self.x
            && cursor_x <= self.x + self.width
            && cursor_y >= self.y + 8.0
            && cursor_y <= self.y + self.height - 8.0
        {
            let rel_y = cursor_y - (self.y + 8.0);
            let idx = (rel_y / 24.0).floor() as usize;
            if idx < self.items.len()
                && self.items[idx].enabled
                && self.hovered_index != Some(idx)
            {
                self.hovered_index = Some(idx);
                if let MenuItemKind::Submenu { ref items } = self.items[idx].kind {
                    let sub_x = self.x + self.width;
                    let sub_y = self.y + 8.0 + idx as f32 * 24.0;
                    self.active_submenu = Some(Box::new(MenuPopup::new(
                        sub_x,
                        sub_y,
                        items.clone(),
                        screen_w,
                        screen_h,
                    )));
                } else {
                    self.active_submenu = None;
                }
            }
        } else if let Some(ref mut sub) = self.active_submenu {
            sub.set_hover(cursor_x, cursor_y, screen_w, screen_h);
        } else {
            self.hovered_index = None;
        }
    }
}

#[derive(Debug, Clone)]
pub struct MenuCategory {
    pub title: String,
    pub items: Vec<MenuItem>,
}

#[derive(Debug, Clone)]
pub struct MenuBar {
    pub categories: Vec<MenuCategory>,
    pub active_category: Option<usize>,
    pub popup: Option<MenuPopup>,
}

impl MenuBar {
    pub fn new(categories: Vec<MenuCategory>) -> Self {
        Self {
            categories,
            active_category: None,
            popup: None,
        }
    }

    pub fn open_category(&mut self, index: usize, bar_x: f32, bar_y: f32, screen_w: f32, screen_h: f32) {
        if index < self.categories.len() {
            self.active_category = Some(index);
            let offset_x = bar_x + index as f32 * 64.0 + 8.0;
            self.popup = Some(MenuPopup::new(
                offset_x,
                bar_y + 28.0,
                self.categories[index].items.clone(),
                screen_w,
                screen_h,
            ));
        }
    }

    pub fn close(&mut self) {
        self.active_category = None;
        self.popup = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_menu_popup_edge_flip() {
        let items = vec![
            MenuItem::action("Copy", "edit.copy", Some("Ctrl+C")),
            MenuItem::action("Paste", "edit.paste", Some("Ctrl+V")),
        ];
        let popup = MenuPopup::new(780.0, 580.0, items, 800.0, 600.0);
        assert!(popup.x < 780.0);
        assert!(popup.y < 580.0);
    }
}
