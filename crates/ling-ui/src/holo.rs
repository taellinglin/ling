//! Holographic vector UI primitives — a stroke font, sci-fi frame geometry, and
//! immediate-mode hit-testing. Everything is returned as line segments
//! `[x0,y0,x1,y1]` in a local box so a renderer can scale, glow and depth-shift
//! them however it likes.

/// Point-in-rect test (immediate-mode hit testing).
pub fn hit_rect(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32) -> bool {
    px >= x && px <= x + w && py >= y && py <= y + h
}

// ── Stroke font ─────────────────────────────────────────────────────────────
// Glyphs are line segments in a unit em-box: x∈[0,1] right, y∈[0,1] down.

const L: f32 = 0.12;
const M: f32 = 0.5;
const R: f32 = 0.88;
const T: f32 = 0.0;
const U: f32 = 0.25;
const C: f32 = 0.5;
const D: f32 = 0.75;
const B: f32 = 1.0;

/// Stroke segments for a single character (uppercased for letters).
/// Unknown / non-ASCII glyphs render as a small box placeholder.
pub fn glyph(c: char) -> Vec<[f32; 4]> {
    let c = c.to_ascii_uppercase();
    let s: &[[f32; 4]] = match c {
        ' ' => &[],
        'A' => &[[L, B, M, T], [M, T, R, B], [0.26, C, 0.74, C]],
        'B' => &[
            [L, T, L, B],
            [L, T, R, U],
            [R, U, L, C],
            [L, C, R, D],
            [R, D, L, B],
        ],
        'C' => &[[R, T, L, T], [L, T, L, B], [L, B, R, B]],
        'D' => &[[L, T, L, B], [L, T, R, U], [R, U, R, D], [R, D, L, B]],
        'E' => &[[R, T, L, T], [L, T, L, B], [L, B, R, B], [L, C, 0.7, C]],
        'F' => &[[R, T, L, T], [L, T, L, B], [L, C, 0.7, C]],
        'G' => &[
            [R, T, L, T],
            [L, T, L, B],
            [L, B, R, B],
            [R, B, R, C],
            [R, C, M, C],
        ],
        'H' => &[[L, T, L, B], [R, T, R, B], [L, C, R, C]],
        'I' => &[[M, T, M, B], [L, T, R, T], [L, B, R, B]],
        'J' => &[[R, T, R, B], [R, B, L, B], [L, B, L, D]],
        'K' => &[[L, T, L, B], [R, T, L, C], [L, C, R, B]],
        'L' => &[[L, T, L, B], [L, B, R, B]],
        'M' => &[[L, B, L, T], [L, T, M, C], [M, C, R, T], [R, T, R, B]],
        'N' => &[[L, B, L, T], [L, T, R, B], [R, B, R, T]],
        'O' => &[[L, T, R, T], [R, T, R, B], [R, B, L, B], [L, B, L, T]],
        'P' => &[[L, B, L, T], [L, T, R, U], [R, U, L, C]],
        'Q' => &[
            [L, T, R, T],
            [R, T, R, B],
            [R, B, L, B],
            [L, B, L, T],
            [M, D, R, B],
        ],
        'R' => &[[L, B, L, T], [L, T, R, U], [R, U, L, C], [L, C, R, B]],
        'S' => &[
            [R, T, L, T],
            [L, T, L, C],
            [L, C, R, C],
            [R, C, R, B],
            [R, B, L, B],
        ],
        'T' => &[[L, T, R, T], [M, T, M, B]],
        'U' => &[[L, T, L, B], [L, B, R, B], [R, B, R, T]],
        'V' => &[[L, T, M, B], [M, B, R, T]],
        'W' => &[
            [L, T, 0.3, B],
            [0.3, B, M, C],
            [M, C, 0.7, B],
            [0.7, B, R, T],
        ],
        'X' => &[[L, T, R, B], [R, T, L, B]],
        'Y' => &[[L, T, M, C], [R, T, M, C], [M, C, M, B]],
        'Z' => &[[L, T, R, T], [R, T, L, B], [L, B, R, B]],
        '0' => &[
            [L, T, R, T],
            [R, T, R, B],
            [R, B, L, B],
            [L, B, L, T],
            [L, B, R, T],
        ],
        '1' => &[[M, T, M, B], [L, U, M, T], [L, B, R, B]],
        '2' => &[[L, U, M, T], [M, T, R, U], [R, U, L, B], [L, B, R, B]],
        '3' => &[
            [L, T, R, T],
            [R, T, R, C],
            [R, C, M, C],
            [R, C, R, B],
            [R, B, L, B],
        ],
        '4' => &[[R, B, R, T], [R, T, L, C], [L, C, R, C]],
        '5' => &[
            [R, T, L, T],
            [L, T, L, C],
            [L, C, R, C],
            [R, C, R, B],
            [R, B, L, B],
        ],
        '6' => &[
            [R, T, L, C],
            [L, C, L, B],
            [L, B, R, B],
            [R, B, R, C],
            [R, C, L, C],
        ],
        '7' => &[[L, T, R, T], [R, T, M, B]],
        '8' => &[
            [L, T, R, T],
            [R, T, R, B],
            [R, B, L, B],
            [L, B, L, T],
            [L, C, R, C],
        ],
        '9' => &[
            [R, C, L, C],
            [L, C, L, T],
            [L, T, R, T],
            [R, T, R, B],
            [R, B, L, B],
        ],
        '-' => &[[L, C, R, C]],
        '_' => &[[L, B, R, B]],
        '.' => &[[0.44, 0.92, 0.56, 0.92]],
        ',' => &[[0.5, 0.8, 0.4, 0.98]],
        '!' => &[[M, T, M, 0.66], [M, 0.86, M, B]],
        '?' => &[
            [L, U, M, T],
            [M, T, R, U],
            [R, U, M, C],
            [M, C, M, D],
            [M, 0.92, M, B],
        ],
        ':' => &[[0.44, 0.34, 0.56, 0.34], [0.44, 0.66, 0.56, 0.66]],
        '/' => &[[L, B, R, T]],
        '+' => &[[L, C, R, C], [M, U, M, D]],
        '*' => &[
            [L, C, R, C],
            [M, U, M, D],
            [0.24, 0.3, 0.76, 0.7],
            [0.76, 0.3, 0.24, 0.7],
        ],
        '=' => &[[L, 0.38, R, 0.38], [L, 0.62, R, 0.62]],
        '>' => &[[L, U, R, C], [R, C, L, D]],
        '<' => &[[R, U, L, C], [L, C, R, D]],
        _ => &[[L, U, R, U], [R, U, R, D], [R, D, L, D], [L, D, L, U]], // placeholder box
    };
    s.to_vec()
}

/// Lay out a string as line segments. Each glyph occupies `gw`×`gh`, advancing by
/// `gw + spacing`. Returns segments in absolute coords starting at `(x, y)`.
pub fn text_lines(text: &str, x: f32, y: f32, gw: f32, gh: f32, spacing: f32) -> Vec<[f32; 4]> {
    let mut out = Vec::new();
    let mut cx = x;
    for ch in text.chars() {
        for seg in glyph(ch) {
            out.push([
                cx + seg[0] * gw,
                y + seg[1] * gh,
                cx + seg[2] * gw,
                y + seg[3] * gh,
            ]);
        }
        cx += gw + spacing;
    }
    out
}

/// Width a string would occupy with the given glyph width + spacing.
pub fn text_width(text: &str, gw: f32, spacing: f32) -> f32 {
    let n = text.chars().count() as f32;
    if n == 0.0 {
        0.0
    } else {
        n * gw + (n - 1.0) * spacing
    }
}

// ── Holographic frame geometry ──────────────────────────────────────────────

/// Sci-fi corner brackets for a rect — eight short segments at the four corners.
pub fn corner_brackets(x: f32, y: f32, w: f32, h: f32, len: f32) -> Vec<[f32; 4]> {
    let l = len.min(w * 0.5).min(h * 0.5);
    let (x1, y1) = (x + w, y + h);
    vec![
        [x, y, x + l, y],
        [x, y, x, y + l], // top-left
        [x1, y, x1 - l, y],
        [x1, y, x1, y + l], // top-right
        [x, y1, x + l, y1],
        [x, y1, x, y1 - l], // bottom-left
        [x1, y1, x1 - l, y1],
        [x1, y1, x1, y1 - l], // bottom-right
    ]
}

/// A clipped/beveled rectangle outline (corners cut → holographic "panel" look).
pub fn beveled_rect(x: f32, y: f32, w: f32, h: f32, bevel: f32) -> Vec<[f32; 4]> {
    let b = bevel.min(w * 0.5).min(h * 0.5);
    let (x1, y1) = (x + w, y + h);
    vec![
        [x + b, y, x1 - b, y],    // top
        [x1 - b, y, x1, y + b],   // top-right cut
        [x1, y + b, x1, y1 - b],  // right
        [x1, y1 - b, x1 - b, y1], // bottom-right cut
        [x1 - b, y1, x + b, y1],  // bottom
        [x + b, y1, x, y1 - b],   // bottom-left cut
        [x, y1 - b, x, y + b],    // left
        [x, y + b, x + b, y],     // top-left cut
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_testing() {
        assert!(hit_rect(15.0, 15.0, 10.0, 10.0, 20.0, 20.0));
        assert!(!hit_rect(5.0, 5.0, 10.0, 10.0, 20.0, 20.0));
    }

    #[test]
    fn font_covers_alphanumerics_and_lays_out() {
        // Every A–Z/0–9 has at least one stroke.
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".chars() {
            assert!(!glyph(c).is_empty(), "glyph {c} empty");
        }
        assert!(glyph(' ').is_empty());
        // Unknown → placeholder box (4 segs).
        assert_eq!(glyph('灵').len(), 4);
        let segs = text_lines("HI 7", 0.0, 0.0, 10.0, 16.0, 2.0);
        assert!(!segs.is_empty());
        assert!((text_width("ABCD", 10.0, 2.0) - (4.0 * 10.0 + 3.0 * 2.0)).abs() < 1e-3);
    }

    #[test]
    fn frame_geometry_segment_counts() {
        assert_eq!(corner_brackets(0.0, 0.0, 100.0, 50.0, 10.0).len(), 8);
        assert_eq!(beveled_rect(0.0, 0.0, 100.0, 50.0, 8.0).len(), 8);
    }
}
