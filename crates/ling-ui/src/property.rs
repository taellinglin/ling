//! Property inspector controls: numeric drag fields, toggles, dropdowns, and sections.

#[derive(Debug, Clone)]
pub struct ValueDrag {
    pub label: String,
    pub value: f32,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub is_dragging: bool,
    pub drag_start_x: f32,
    pub drag_start_value: f32,
}

impl ValueDrag {
    pub fn new(label: impl Into<String>, value: f32, min: f32, max: f32, step: f32) -> Self {
        Self {
            label: label.into(),
            value: value.clamp(min, max),
            min,
            max,
            step,
            is_dragging: false,
            drag_start_x: 0.0,
            drag_start_value: value,
        }
    }

    pub fn start_drag(&mut self, start_x: f32) {
        self.is_dragging = true;
        self.drag_start_x = start_x;
        self.drag_start_value = self.value;
    }

    pub fn update_drag(&mut self, current_x: f32, shift_pressed: bool, ctrl_pressed: bool) {
        if !self.is_dragging {
            return;
        }

        let delta_x = current_x - self.drag_start_x;
        let mut scale = (self.max - self.min) / 400.0;
        if shift_pressed {
            scale *= 0.1;
        }

        let mut raw = self.drag_start_value + delta_x * scale;
        if ctrl_pressed && self.step > 0.0 {
            raw = (raw / self.step).round() * self.step;
        }

        self.value = raw.clamp(self.min, self.max);
    }

    pub fn end_drag(&mut self) {
        self.is_dragging = false;
    }
}

#[derive(Debug, Clone)]
pub struct Toggle {
    pub label: String,
    pub value: bool,
}

impl Toggle {
    pub fn new(label: impl Into<String>, value: bool) -> Self {
        Self { label: label.into(), value }
    }

    pub fn toggle(&mut self) {
        self.value = !self.value;
    }
}

#[derive(Debug, Clone)]
pub struct Dropdown {
    pub label: String,
    pub options: Vec<String>,
    pub selected_index: usize,
    pub is_open: bool,
}

impl Dropdown {
    pub fn new(label: impl Into<String>, options: Vec<String>, selected_index: usize) -> Self {
        Self { label: label.into(), options, selected_index, is_open: false }
    }

    pub fn select(&mut self, index: usize) {
        if index < self.options.len() {
            self.selected_index = index;
            self.is_open = false;
        }
    }

    pub fn selected_text(&self) -> &str {
        self.options
            .get(self.selected_index)
            .map(|s| s.as_str())
            .unwrap_or("")
    }
}

#[derive(Debug, Clone)]
pub struct PropertySection {
    pub title: String,
    pub collapsed: bool,
}

impl PropertySection {
    pub fn new(title: impl Into<String>) -> Self {
        Self { title: title.into(), collapsed: false }
    }

    pub fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_value_drag_scrub() {
        let mut drag = ValueDrag::new("Scale", 1.0, 0.0, 10.0, 0.1);
        drag.start_drag(100.0);
        drag.update_drag(140.0, false, false);
        assert!(drag.value > 1.0);
        drag.end_drag();
        assert!(!drag.is_dragging);
    }
}
