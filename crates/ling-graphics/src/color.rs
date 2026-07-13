/// Linear RGBA color, all components in [0, 1].
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const BLACK: Self = Self { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
    pub const BLUE: Self = Self { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };
    pub const CYAN: Self = Self { r: 0.0, g: 1.0, b: 1.0, a: 1.0 };
    pub const GREEN: Self = Self { r: 0.0, g: 1.0, b: 0.0, a: 1.0 };
    pub const MAGENTA: Self = Self { r: 1.0, g: 0.0, b: 1.0, a: 1.0 };
    pub const RED: Self = Self { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
    pub const TRANSPARENT: Self = Self { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
    pub const WHITE: Self = Self { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
    pub const YELLOW: Self = Self { r: 1.0, g: 1.0, b: 0.0, a: 1.0 };

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub fn from_srgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgb(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b))
    }

    pub fn from_srgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self::new(
            srgb_to_linear(r),
            srgb_to_linear(g),
            srgb_to_linear(b),
            a as f32 / 255.0,
        )
    }

    pub fn from_hex(hex: u32) -> Self {
        let r = ((hex >> 16) & 0xFF) as u8;
        let g = ((hex >> 8) & 0xFF) as u8;
        let b = (hex & 0xFF) as u8;
        Self::from_srgb(r, g, b)
    }

    pub fn to_rgba_bytes(self) -> [u8; 4] {
        [
            (self.r.clamp(0.0, 1.0) * 255.0) as u8,
            (self.g.clamp(0.0, 1.0) * 255.0) as u8,
            (self.b.clamp(0.0, 1.0) * 255.0) as u8,
            (self.a.clamp(0.0, 1.0) * 255.0) as u8,
        ]
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self::new(
            self.r + (other.r - self.r) * t,
            self.g + (other.g - self.g) * t,
            self.b + (other.b - self.b) * t,
            self.a + (other.a - self.a) * t,
        )
    }

    pub fn premultiply_alpha(self) -> Self {
        Self::new(self.r * self.a, self.g * self.a, self.b * self.a, self.a)
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// Clamp all components to [0, 1].
    pub fn clamp(self) -> Self {
        Self::new(
            self.r.clamp(0.0, 1.0),
            self.g.clamp(0.0, 1.0),
            self.b.clamp(0.0, 1.0),
            self.a.clamp(0.0, 1.0),
        )
    }

    pub fn luminance(self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }
}

impl std::ops::Add for Color {
    type Output = Self;

    fn add(self, r: Self) -> Self {
        Self::new(self.r + r.r, self.g + r.g, self.b + r.b, self.a + r.a)
    }
}
impl std::ops::Mul for Color {
    type Output = Self;

    fn mul(self, r: Self) -> Self {
        Self::new(self.r * r.r, self.g * r.g, self.b * r.b, self.a * r.a)
    }
}
impl std::ops::Mul<f32> for Color {
    type Output = Self;

    fn mul(self, s: f32) -> Self {
        Self::new(self.r * s, self.g * s, self.b * s, self.a * s)
    }
}

fn srgb_to_linear(c: u8) -> f32 {
    let f = c as f32 / 255.0;
    if f <= 0.04045 {
        f / 12.92
    } else {
        ((f + 0.055) / 1.055).powf(2.4)
    }
}

// ── Blend modes ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BlendMode {
    /// Porter-Duff "over" compositing.
    Normal,
    Additive,
    Multiply,
    Screen,
    Overlay,
    Subtract,
}

impl BlendMode {
    pub fn blend(self, src: Color, dst: Color) -> Color {
        let sa = src.a;
        let da = dst.a;
        match self {
            BlendMode::Normal => Color::new(
                src.r * sa + dst.r * (1.0 - sa),
                src.g * sa + dst.g * (1.0 - sa),
                src.b * sa + dst.b * (1.0 - sa),
                sa + da * (1.0 - sa),
            ),
            BlendMode::Additive => Color::new(
                (src.r * sa + dst.r).min(1.0),
                (src.g * sa + dst.g).min(1.0),
                (src.b * sa + dst.b).min(1.0),
                (sa + da).min(1.0),
            ),
            BlendMode::Multiply => Color::new(
                src.r * dst.r,
                src.g * dst.g,
                src.b * dst.b,
                (sa * da).min(1.0),
            ),
            BlendMode::Screen => Color::new(
                1.0 - (1.0 - src.r) * (1.0 - dst.r),
                1.0 - (1.0 - src.g) * (1.0 - dst.g),
                1.0 - (1.0 - src.b) * (1.0 - dst.b),
                (sa + da - sa * da).min(1.0),
            ),
            BlendMode::Overlay => {
                let ov = |s: f32, d: f32| {
                    if d < 0.5 {
                        2.0 * s * d
                    } else {
                        1.0 - 2.0 * (1.0 - s) * (1.0 - d)
                    }
                };
                Color::new(
                    ov(src.r, dst.r),
                    ov(src.g, dst.g),
                    ov(src.b, dst.b),
                    (sa + da - sa * da).min(1.0),
                )
            },
            BlendMode::Subtract => Color::new(
                (dst.r - src.r * sa).max(0.0),
                (dst.g - src.g * sa).max(0.0),
                (dst.b - src.b * sa).max(0.0),
                da,
            ),
        }
    }
}

// ── Color gradient ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ColorGradient {
    stops: Vec<(f32, Color)>,
}

impl ColorGradient {
    pub fn new(stops: Vec<(f32, Color)>) -> Self {
        let mut s = stops;
        s.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        Self { stops: s }
    }

    pub fn sample(&self, t: f32) -> Color {
        if self.stops.is_empty() {
            return Color::TRANSPARENT;
        }
        if self.stops.len() == 1 {
            return self.stops[0].1;
        }
        if t <= self.stops[0].0 {
            return self.stops[0].1;
        }
        let last = self.stops.last().unwrap();
        if t >= last.0 {
            return last.1;
        }
        for i in 0..self.stops.len() - 1 {
            let (t0, c0) = self.stops[i];
            let (t1, c1) = self.stops[i + 1];
            if t >= t0 && t <= t1 {
                let local = (t - t0) / (t1 - t0);
                return c0.lerp(c1, local);
            }
        }
        self.stops.last().unwrap().1
    }
}
