//! Procedural texture generation for ling-game.
//!
//! All generators return RGBA pixel data as `Vec<u8>` (4 bytes per pixel, row-major).

// ── Noise helpers ──────────────────────────────────────────────────────────────

fn hash2(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32)
        .wrapping_add((y as u32).wrapping_mul(2654435769))
        .wrapping_add(seed.wrapping_mul(1234567891));
    h ^= h >> 16;
    h = h.wrapping_mul(0x45d9f3b);
    h ^= h >> 16;
    h as f32 / u32::MAX as f32
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn vnoise(x: f32, y: f32, seed: u32) -> f32 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let xf = smooth(x - xi as f32);
    let yf = smooth(y - yi as f32);
    let a = hash2(xi, yi, seed);
    let b = hash2(xi + 1, yi, seed);
    let c = hash2(xi, yi + 1, seed);
    let d = hash2(xi + 1, yi + 1, seed);
    let ab = a + (b - a) * xf;
    let cd = c + (d - c) * xf;
    ab + (cd - ab) * yf
}

fn fbm(x: f32, y: f32, octaves: u32, seed: u32) -> f32 {
    let mut v = 0.0f32;
    let mut a = 0.5f32;
    let mut f = 1.0f32;
    for i in 0..octaves {
        v += vnoise(x * f, y * f, seed.wrapping_add(i * 7919)) * a;
        a *= 0.5;
        f *= 2.0;
    }
    v
}

// ── IQ cosine palette ──────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Palette {
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    d: [f32; 3],
}

impl Palette {
    pub fn new(a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) -> Self {
        Self { a, b, c, d }
    }

    pub fn rainbow() -> Self {
        Self::new(
            [0.5, 0.5, 0.5],
            [0.5, 0.5, 0.5],
            [1.0, 1.0, 1.0],
            [0.0, 0.333, 0.667],
        )
    }

    pub fn fire() -> Self {
        Self::new(
            [0.8, 0.4, 0.1],
            [0.7, 0.3, 0.1],
            [1.0, 0.5, 0.3],
            [0.0, 0.5, 0.8],
        )
    }

    pub fn ocean() -> Self {
        Self::new(
            [0.1, 0.4, 0.7],
            [0.3, 0.3, 0.4],
            [0.8, 1.0, 0.5],
            [0.3, 0.0, 0.6],
        )
    }

    pub fn psychedelic() -> Self {
        Self::new(
            [0.5, 0.5, 0.5],
            [0.8, 0.8, 0.8],
            [1.0, 1.3, 0.7],
            [0.0, 0.15, 0.3],
        )
    }

    pub fn neon() -> Self {
        Self::new(
            [0.5, 0.5, 0.5],
            [0.5, 0.5, 0.5],
            [2.0, 1.0, 0.0],
            [0.5, 0.2, 0.25],
        )
    }

    pub fn forest() -> Self {
        Self::new(
            [0.3, 0.5, 0.2],
            [0.2, 0.3, 0.1],
            [1.0, 0.5, 0.8],
            [0.1, 0.3, 0.6],
        )
    }

    pub fn eval(&self, t: f32) -> [f32; 3] {
        [0, 1, 2].map(|i| {
            (self.a[i] + self.b[i] * (std::f32::consts::TAU * (self.c[i] * t + self.d[i])).cos())
                .clamp(0.0, 1.0)
        })
    }

    pub fn rgba(&self, t: f32) -> [u8; 4] {
        let [r, g, b] = self.eval(t);
        [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, 255]
    }

    pub fn rgba_alpha(&self, t: f32, a: f32) -> [u8; 4] {
        let [r, g, b] = self.eval(t);
        [
            (r * 255.0) as u8,
            (g * 255.0) as u8,
            (b * 255.0) as u8,
            (a * 255.0) as u8,
        ]
    }
}

// ── Pixel helpers ─────────────────────────────────────────────────────────────

pub struct TexBuf {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
impl TexBuf {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            data: vec![0u8; (width * height * 4) as usize],
            width,
            height,
        }
    }

    pub fn set(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        let i = ((y * self.width + x) * 4) as usize;
        if i + 3 < self.data.len() {
            self.data[i..i + 4].copy_from_slice(&rgba);
        }
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.data
    }
}

// ── Generators ────────────────────────────────────────────────────────────────

/// Checkerboard pattern.
pub fn checkerboard(
    width: u32,
    height: u32,
    tiles: u32,
    color_a: [u8; 4],
    color_b: [u8; 4],
) -> Vec<u8> {
    let mut tex = TexBuf::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let tx = x * tiles / width;
            let ty = y * tiles / height;
            tex.set(x, y, if (tx + ty).is_multiple_of(2) { color_a } else { color_b });
        }
    }
    tex.into_vec()
}

/// Fractal Brownian Motion noise texture.
pub fn noise(
    width: u32,
    height: u32,
    scale: f32,
    octaves: u32,
    seed: u64,
    palette: &Palette,
) -> Vec<u8> {
    let mut tex = TexBuf::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let v = fbm(
                x as f32 * scale / width as f32,
                y as f32 * scale / height as f32,
                octaves,
                seed as u32,
            );
            tex.set(x, y, palette.rgba(v));
        }
    }
    tex.into_vec()
}

/// Smooth linear gradient between two colours.
pub fn gradient(width: u32, height: u32, from: [u8; 4], to: [u8; 4], angle_deg: f32) -> Vec<u8> {
    let mut tex = TexBuf::new(width, height);
    let a = angle_deg.to_radians();
    let ca = a.cos();
    let sa = a.sin();
    for y in 0..height {
        for x in 0..width {
            let nx = x as f32 / width as f32 - 0.5;
            let ny = y as f32 / height as f32 - 0.5;
            let t = ((nx * ca + ny * sa) + 0.707) / 1.414;
            let t = t.clamp(0.0, 1.0);
            let px = [
                (from[0] as f32 + (to[0] as f32 - from[0] as f32) * t) as u8,
                (from[1] as f32 + (to[1] as f32 - from[1] as f32) * t) as u8,
                (from[2] as f32 + (to[2] as f32 - from[2] as f32) * t) as u8,
                (from[3] as f32 + (to[3] as f32 - from[3] as f32) * t) as u8,
            ];
            tex.set(x, y, px);
        }
    }
    tex.into_vec()
}

/// Mandelbrot / Julia set fractal texture.
pub fn mandelbrot(
    width: u32,
    height: u32,
    palette: &Palette,
    zoom: f64,
    cx: f64,
    cy: f64,
    max_iter: u32,
) -> Vec<u8> {
    let mut tex = TexBuf::new(width, height);
    for py in 0..height {
        for px in 0..width {
            let zx = (px as f64 / width as f64 - 0.5) / zoom + cx;
            let zy = (py as f64 / height as f64 - 0.5) / zoom + cy;
            let mut x = 0.0f64;
            let mut y = 0.0f64;
            let mut i = 0u32;
            while i < max_iter && x * x + y * y < 4.0 {
                let tmp = x * x - y * y + zx;
                y = 2.0 * x * y + zy;
                x = tmp;
                i += 1;
            }
            let t = if i == max_iter {
                0.0
            } else {
                (i as f32 - (x as f32 * x as f32 + y as f32 * y as f32).ln().ln() / 2.0f32.ln())
                    / max_iter as f32
            };
            tex.set(px, py, palette.rgba(t.clamp(0.0, 1.0)));
        }
    }
    tex.into_vec()
}

/// Julia set with complex parameter c.
pub fn julia(
    width: u32,
    height: u32,
    palette: &Palette,
    c_re: f64,
    c_im: f64,
    max_iter: u32,
) -> Vec<u8> {
    let mut tex = TexBuf::new(width, height);
    for py in 0..height {
        for px in 0..width {
            let mut zx = (px as f64 / width as f64 - 0.5) * 3.5;
            let mut zy = (py as f64 / height as f64 - 0.5) * 3.5;
            let mut i = 0u32;
            while i < max_iter && zx * zx + zy * zy < 4.0 {
                let tmp = zx * zx - zy * zy + c_re;
                zy = 2.0 * zx * zy + c_im;
                zx = tmp;
                i += 1;
            }
            let t = i as f32 / max_iter as f32;
            tex.set(px, py, palette.rgba(t));
        }
    }
    tex.into_vec()
}

/// Voronoi / cellular noise texture.
pub fn voronoi(width: u32, height: u32, cell_count: u32, seed: u64, palette: &Palette) -> Vec<u8> {
    // Generate cell centres.
    let cells: Vec<[f32; 2]> = (0..cell_count)
        .map(|i| {
            let h = hash2(i as i32, 0, seed as u32);
            let h2 = hash2(i as i32, 1, seed as u32 + 999);
            [h, h2]
        })
        .collect();

    let mut tex = TexBuf::new(width, height);
    for py in 0..height {
        for px in 0..width {
            let fx = px as f32 / width as f32;
            let fy = py as f32 / height as f32;
            let (min_d, nearest) =
                cells
                    .iter()
                    .enumerate()
                    .fold((f32::MAX, 0usize), |(d, idx), (i, &[cx, cy])| {
                        let dd = (fx - cx).powi(2) + (fy - cy).powi(2);
                        if dd < d {
                            (dd, i)
                        } else {
                            (d, idx)
                        }
                    });
            let t = (nearest as f32 / cell_count as f32 + min_d * 4.0) % 1.0;
            tex.set(px, py, palette.rgba(t));
        }
    }
    tex.into_vec()
}

/// Animated hypnotic spiral / op-art texture.
pub fn hypnotic_spiral(
    width: u32,
    height: u32,
    freq: f32,
    bands: u32,
    time: f32,
    palette: &Palette,
) -> Vec<u8> {
    let mut tex = TexBuf::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let nx = x as f32 / width as f32 - 0.5;
            let ny = y as f32 / height as f32 - 0.5;
            let r = (nx * nx + ny * ny).sqrt();
            let theta = ny.atan2(nx);
            let t = ((r * freq - theta / std::f32::consts::TAU + time * 0.5) * bands as f32 % 1.0)
                .abs();
            tex.set(x, y, palette.rgba(t));
        }
    }
    tex.into_vec()
}

/// Frequency band texture — maps audio bands to color stripes (for wall/floor VJ visuals).
pub fn freq_map(
    bands: &[f32],
    palette: &Palette,
    width: u32,
    height: u32,
    time: f32,
    speed: f32,
) -> Vec<u8> {
    let mut tex = TexBuf::new(width, height);
    let n = bands.len().max(1);
    for y in 0..height {
        for x in 0..width {
            let band_idx = (x as usize * n / width as usize).min(n - 1);
            let mag = bands[band_idx].clamp(0.0, 1.0);
            let fill_y = (mag * height as f32) as u32;
            if y >= height - fill_y {
                let t = (x as f32 / width as f32 + time * speed) % 1.0;
                let [r, g, b] = palette.eval(t);
                let brightness = mag * (1.0 - (y as f32 / height as f32) * 0.5);
                tex.set(
                    x,
                    y,
                    [
                        (r * brightness * 255.0) as u8,
                        (g * brightness * 255.0) as u8,
                        (b * brightness * 255.0) as u8,
                        255,
                    ],
                );
            }
        }
    }
    tex.into_vec()
}

/// Color-cycling: offset the palette phase each frame.
pub fn color_cycle(data: &mut [u8], phase: f32, speed: f32) {
    for px in data.chunks_mut(4) {
        let h = px[0] as f32 / 255.0;
        let new_h = (h + speed * phase) % 1.0;
        // Simple hue rotation in RGB — approximate.
        let d = [px[0] as f32, px[1] as f32, px[2] as f32];
        let r = d[0] * (1.0 - speed) + d[1] * speed * new_h;
        px[0] = r.clamp(0.0, 255.0) as u8;
    }
}

/// Halftone dot pattern.
pub fn halftone(width: u32, height: u32, dot_size: f32, palette: &Palette, time: f32) -> Vec<u8> {
    let mut tex = TexBuf::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let fx = x as f32 / width as f32;
            let fy = y as f32 / height as f32;
            let gx = (fx / dot_size).floor();
            let gy = (fy / dot_size).floor();
            let lx = (fx / dot_size - gx - 0.5) * 2.0;
            let ly = (fy / dot_size - gy - 0.5) * 2.0;
            let r = (lx * lx + ly * ly).sqrt();
            let t = (gx / (1.0 / dot_size) + time * 0.1) % 1.0;
            let [pr, pg, pb] = palette.eval(t);
            let a = if r < 0.7 {
                ((0.7 - r) / 0.7).clamp(0.0, 1.0)
            } else {
                0.0
            };
            tex.set(
                x,
                y,
                [
                    (pr * 255.0) as u8,
                    (pg * 255.0) as u8,
                    (pb * 255.0) as u8,
                    (a * 255.0) as u8,
                ],
            );
        }
    }
    tex.into_vec()
}

/// Convolution rings / ripple pattern (good for water surfaces).
pub fn ripple(
    width: u32,
    height: u32,
    freq: f32,
    cx: f32,
    cy: f32,
    time: f32,
    palette: &Palette,
) -> Vec<u8> {
    let mut tex = TexBuf::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 / width as f32 - cx;
            let dy = y as f32 / height as f32 - cy;
            let r = (dx * dx + dy * dy).sqrt();
            let t = ((r * freq - time) % 1.0).abs();
            tex.set(x, y, palette.rgba(t));
        }
    }
    tex.into_vec()
}
