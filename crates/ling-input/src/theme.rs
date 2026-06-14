//! Themer for on-screen controls — colors, shapes, opacity, presets.
//!
//! This crate doesn't draw; it produces *data* a renderer (ling-graphics /
//! ling-ui) consumes. A [`ControlTheme`] is fully serializable so players can
//! save/share skins. The default [`ControlTheme::ling`] uses the project
//! palette: navy / teal / rose-red / grey / vine-green.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// 8-bit straight-alpha RGBA color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    #[must_use]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgba(r, g, b, 255)
    }
    /// From a packed `0xRRGGBB` literal.
    #[must_use]
    pub const fn hex(rgb: u32) -> Self {
        Self::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    }
    #[must_use]
    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }
    /// Multiply alpha by `k` (`0..=1`), for fade-out of idle controls.
    #[must_use]
    pub fn faded(self, k: f32) -> Self {
        Self { a: (f32::from(self.a) * k.clamp(0.0, 1.0)) as u8, ..self }
    }
    /// Linear-ish blend toward `other` by `t` (`0..=1`).
    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t) as u8;
        Self {
            r: mix(self.r, other.r),
            g: mix(self.g, other.g),
            b: mix(self.b, other.b),
            a: mix(self.a, other.a),
        }
    }
    /// As normalized `[r, g, b, a]` floats for a renderer.
    #[must_use]
    pub fn to_f32(self) -> [f32; 4] {
        [
            f32::from(self.r) / 255.0,
            f32::from(self.g) / 255.0,
            f32::from(self.b) / 255.0,
            f32::from(self.a) / 255.0,
        ]
    }
}

/// The Ling default palette (matches diagnostics + editor theme).
pub mod palette {
    use super::Color;
    /// Navy blue — secondary / notes (`#3B6EA5`).
    pub const NAVY: Color = Color::hex(0x3B_6E_A5);
    /// Dark navy — backgrounds (`#14233D`).
    pub const NAVY_BG: Color = Color::hex(0x14_23_3D);
    /// Teal — structure / frames (`#2A9D8F`).
    pub const TEAL: Color = Color::hex(0x2A_9D_8F);
    /// Rose red — accents / active (`#E84A6F`).
    pub const ROSE: Color = Color::hex(0xE8_4A_6F);
    /// Grey — neutral / disabled (`#8D99AE`).
    pub const GREY: Color = Color::hex(0x8D_99_AE);
    /// Vine green — success / confirm (`#7FB069`).
    pub const VINE: Color = Color::hex(0x7F_B0_69);
}

/// Outline / body shape of a control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum ControlShape {
    Circle,
    RoundedRect,
    Pill,
    Hexagon,
}

impl Default for ControlShape {
    fn default() -> Self {
        Self::Circle
    }
}

/// A complete skin for on-screen controls. Renderer-agnostic data.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ControlTheme {
    pub name: String,
    /// Control body fill.
    pub base: Color,
    /// Stick knob / pressed-button fill.
    pub knob: Color,
    pub outline: Color,
    pub label: Color,
    /// Highlight tint while pressed/active.
    pub pressed: Color,
    /// Accent for rings, ticks, focus.
    pub accent: Color,
    /// Global opacity multiplier while touched (`0..=1`).
    pub opacity: f32,
    /// Opacity while idle/untouched — controls fade away on mobile.
    pub idle_opacity: f32,
    /// Seconds to fade between idle and active.
    pub fade_secs: f32,
    /// Corner radius (for `RoundedRect`).
    pub corner: f32,
    pub outline_width: f32,
    pub shape: ControlShape,
}

impl Default for ControlTheme {
    fn default() -> Self {
        Self::ling()
    }
}

impl ControlTheme {
    /// The default Ling skin (navy/teal/rose palette, semi-transparent).
    #[must_use]
    pub fn ling() -> Self {
        Self {
            name: "Ling".into(),
            base: palette::NAVY_BG.with_alpha(150),
            knob: palette::TEAL.with_alpha(220),
            outline: palette::NAVY.with_alpha(220),
            label: palette::GREY,
            pressed: palette::ROSE.with_alpha(230),
            accent: palette::VINE,
            opacity: 0.95,
            idle_opacity: 0.45,
            fade_secs: 0.20,
            corner: 18.0,
            outline_width: 2.5,
            shape: ControlShape::Circle,
        }
    }

    /// Minimal, mostly-transparent — only the knob is visible.
    #[must_use]
    pub fn minimal() -> Self {
        Self {
            name: "Minimal".into(),
            base: palette::GREY.with_alpha(40),
            knob: Color::rgb(255, 255, 255).with_alpha(180),
            outline: palette::GREY.with_alpha(80),
            opacity: 0.8,
            idle_opacity: 0.15,
            ..Self::ling()
        }
    }

    /// High-contrast accessibility skin (opaque, thick outlines).
    #[must_use]
    pub fn high_contrast() -> Self {
        Self {
            name: "High Contrast".into(),
            base: Color::rgb(0, 0, 0),
            knob: Color::rgb(255, 255, 0),
            outline: Color::rgb(255, 255, 255),
            label: Color::rgb(255, 255, 255),
            pressed: Color::rgb(255, 80, 80),
            accent: Color::rgb(0, 255, 255),
            opacity: 1.0,
            idle_opacity: 1.0,
            fade_secs: 0.0,
            outline_width: 5.0,
            ..Self::ling()
        }
    }

    /// Neon — rose knob on dark, vine accent. Pure vibes.
    #[must_use]
    pub fn neon() -> Self {
        Self {
            name: "Neon".into(),
            base: Color::hex(0x0A_0A_14).with_alpha(160),
            knob: palette::ROSE.with_alpha(235),
            outline: palette::TEAL,
            accent: palette::VINE,
            pressed: palette::VINE.with_alpha(235),
            shape: ControlShape::Hexagon,
            ..Self::ling()
        }
    }

    /// The opacity to draw at given how recently the control was touched
    /// (`active_blend` `0` idle .. `1` active), eased over `fade_secs`.
    #[must_use]
    pub fn current_opacity(&self, active_blend: f32) -> f32 {
        let t = active_blend.clamp(0.0, 1.0);
        (self.idle_opacity + (self.opacity - self.idle_opacity) * t).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_unpacks() {
        assert_eq!(Color::hex(0x3B_6E_A5), Color::rgb(0x3B, 0x6E, 0xA5));
    }

    #[test]
    fn opacity_interpolates() {
        let t = ControlTheme::ling();
        assert!((t.current_opacity(0.0) - t.idle_opacity).abs() < 1e-6);
        assert!((t.current_opacity(1.0) - t.opacity).abs() < 1e-6);
    }
}
