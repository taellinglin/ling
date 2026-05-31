//! Flex layout engine (wraps Taffy when the `layout` feature is enabled).

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlexDirection { Row, Column, RowReverse, ColumnReverse }

#[derive(Debug, Clone, Copy)]
pub struct LayoutNode {
    pub x: f32, pub y: f32,
    pub width: f32, pub height: f32,
}

#[derive(Debug, Clone)]
pub struct Style {
    pub direction: FlexDirection,
    pub gap:       f32,
    pub padding:   f32,
}

impl Default for Style {
    fn default() -> Self {
        Self { direction: FlexDirection::Column, gap: 4.0, padding: 8.0 }
    }
}

/// Lay out `count` children of equal size inside `parent`.
pub fn simple_layout(parent: LayoutNode, count: usize, style: &Style) -> Vec<LayoutNode> {
    if count == 0 { return vec![]; }
    let available = match style.direction {
        FlexDirection::Row | FlexDirection::RowReverse =>
            parent.width  - style.padding * 2.0 - style.gap * (count - 1) as f32,
        _ =>
            parent.height - style.padding * 2.0 - style.gap * (count - 1) as f32,
    };
    let child_size = (available / count as f32).max(0.0);
    (0..count).map(|i| {
        let offset = style.padding + i as f32 * (child_size + style.gap);
        match style.direction {
            FlexDirection::Row => LayoutNode {
                x: parent.x + offset, y: parent.y + style.padding,
                width: child_size, height: parent.height - style.padding * 2.0,
            },
            _ => LayoutNode {
                x: parent.x + style.padding, y: parent.y + offset,
                width: parent.width - style.padding * 2.0, height: child_size,
            },
        }
    }).collect()
}
