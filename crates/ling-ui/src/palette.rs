//! Command palette fuzzy search launcher modal.

#[derive(Debug, Clone)]
pub struct CommandItem {
    pub id: String,
    pub label: String,
    pub category: String,
    pub shortcut_str: Option<String>,
}

impl CommandItem {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        category: impl Into<String>,
        shortcut_str: Option<impl Into<String>>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            category: category.into(),
            shortcut_str: shortcut_str.map(|s| s.into()),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CommandPalette {
    pub is_open: bool,
    pub query: String,
    pub items: Vec<CommandItem>,
    pub selected_index: usize,
}

impl CommandPalette {
    pub fn new(items: Vec<CommandItem>) -> Self {
        Self {
            is_open: false,
            query: String::new(),
            items,
            selected_index: 0,
        }
    }

    pub fn open(&mut self) {
        self.is_open = true;
        self.query.clear();
        self.selected_index = 0;
    }

    pub fn close(&mut self) {
        self.is_open = false;
        self.query.clear();
    }

    pub fn filtered_items(&self) -> Vec<&CommandItem> {
        if self.query.trim().is_empty() {
            return self.items.iter().collect();
        }

        let q = self.query.to_lowercase();
        self.items
            .iter()
            .filter(|item| {
                item.label.to_lowercase().contains(&q)
                    || item.category.to_lowercase().contains(&q)
            })
            .collect()
    }

    pub fn select_next(&mut self) {
        let count = self.filtered_items().len();
        if count > 0 {
            self.selected_index = (self.selected_index + 1) % count;
        }
    }

    pub fn select_prev(&mut self) {
        let count = self.filtered_items().len();
        if count > 0 {
            self.selected_index = if self.selected_index == 0 {
                count - 1
            } else {
                self.selected_index - 1
            };
        }
    }

    pub fn confirm_selected(&mut self) -> Option<String> {
        let filtered = self.filtered_items();
        if let Some(item) = filtered.get(self.selected_index) {
            let id = item.id.clone();
            self.close();
            Some(id)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_palette_filtering() {
        let items = vec![
            CommandItem::new("file.open", "Open File", "File", Some("Ctrl+O")),
            CommandItem::new("edit.undo", "Undo", "Edit", Some("Ctrl+Z")),
        ];
        let mut palette = CommandPalette::new(items);
        palette.open();
        palette.query = "open".to_string();

        let filtered = palette.filtered_items();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "file.open");
    }
}
