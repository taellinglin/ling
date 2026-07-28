//! Docking panels, splitters, and window layout partitioning.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone)]
pub struct SplitPane {
    pub direction: SplitDirection,
    pub ratio: f32,
    pub min_size_a: f32,
    pub min_size_b: f32,
    pub splitter_thickness: f32,
    pub is_dragging: bool,
}

impl SplitPane {
    pub fn new(direction: SplitDirection, ratio: f32) -> Self {
        Self {
            direction,
            ratio: ratio.clamp(0.05, 0.95),
            min_size_a: 40.0,
            min_size_b: 40.0,
            splitter_thickness: 6.0,
            is_dragging: false,
        }
    }

    pub fn compute_layout(&self, x: f32, y: f32, w: f32, h: f32) -> ([f32; 4], [f32; 4], [f32; 4]) {
        let t = self.splitter_thickness;
        match self.direction {
            SplitDirection::Horizontal => {
                let available = (w - t).max(self.min_size_a + self.min_size_b);
                let mut w_a = (available * self.ratio).max(self.min_size_a);
                let mut w_b = (available - w_a).max(self.min_size_b);

                if w_a + w_b > available {
                    w_a = available - self.min_size_b;
                    w_b = self.min_size_b;
                }

                let rect_a = [x, y, w_a, h];
                let rect_s = [x + w_a, y, t, h];
                let rect_b = [x + w_a + t, y, w_b, h];
                (rect_a, rect_s, rect_b)
            }
            SplitDirection::Vertical => {
                let available = (h - t).max(self.min_size_a + self.min_size_b);
                let mut h_a = (available * self.ratio).max(self.min_size_a);
                let mut h_b = (available - h_a).max(self.min_size_b);

                if h_a + h_b > available {
                    h_a = available - self.min_size_b;
                    h_b = self.min_size_b;
                }

                let rect_a = [x, y, w, h_a];
                let rect_s = [x, y + h_a, w, t];
                let rect_b = [x, y + h_a + t, w, h_b];
                (rect_a, rect_s, rect_b)
            }
        }
    }

    pub fn update_drag(&mut self, cursor_x: f32, cursor_y: f32, x: f32, y: f32, w: f32, h: f32) {
        if !self.is_dragging {
            return;
        }

        let t = self.splitter_thickness;
        match self.direction {
            SplitDirection::Horizontal => {
                let rel = (cursor_x - x - t * 0.5) / (w - t).max(1.0);
                self.ratio = rel.clamp(0.05, 0.95);
            }
            SplitDirection::Vertical => {
                let rel = (cursor_y - y - t * 0.5) / (h - t).max(1.0);
                self.ratio = rel.clamp(0.05, 0.95);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_pane_bounds() {
        let pane = SplitPane::new(SplitDirection::Horizontal, 0.5);
        let (rect_a, rect_s, rect_b) = pane.compute_layout(0.0, 0.0, 500.0, 300.0);

        assert_eq!(rect_a[3], 300.0);
        assert_eq!(rect_s[2], 6.0);
        assert_eq!(rect_a[2] + rect_s[2] + rect_b[2], 500.0);
    }
}
