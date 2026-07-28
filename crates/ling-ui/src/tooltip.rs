//! Tooltip hover state and card placement.

#[derive(Debug, Clone)]
pub struct Tooltip {
    pub text: String,
    pub target_rect: [f32; 4],
    pub hover_timer: f32,
    pub delay: f32,
}

impl Tooltip {
    pub fn new(text: impl Into<String>, target_rect: [f32; 4]) -> Self {
        Self {
            text: text.into(),
            target_rect,
            hover_timer: 0.0,
            delay: 0.4,
        }
    }

    pub fn update(&mut self, is_hovered: bool, delta_time: f32) -> bool {
        if is_hovered {
            self.hover_timer += delta_time;
            self.hover_timer >= self.delay
        } else {
            self.hover_timer = 0.0;
            false
        }
    }

    pub fn compute_position(&self, card_w: f32, card_h: f32, screen_w: f32, screen_h: f32) -> [f32; 2] {
        let [tx, ty, tw, _th] = self.target_rect;
        let mut x = tx + tw * 0.5 - card_w * 0.5;
        let mut y = ty - card_h - 6.0;

        if y < 0.0 {
            y = ty + _th + 6.0;
        }

        x = x.clamp(4.0, (screen_w - card_w - 4.0).max(4.0));
        y = y.clamp(4.0, (screen_h - card_h - 4.0).max(4.0));

        [x, y]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tooltip_timer() {
        let mut tt = Tooltip::new("Help text", [10.0, 10.0, 50.0, 20.0]);
        assert!(!tt.update(true, 0.2));
        assert!(tt.update(true, 0.3));
    }
}
