//! The slice of the `image` crate's API the engine uses, for the no_std build: RGBA8 buffers, a resize, and
//! files through the host (PNG by the in-crate codec, anything else through the host's decoder).

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// an error with a message
#[derive(Debug)]
pub struct ImageError(pub String);
impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
fn err<T>(s: impl Into<String>) -> Result<T, ImageError> {
    Err(ImageError(s.into()))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba<T>(pub [T; 4]);
impl<T> core::ops::Index<usize> for Rgba<T> {
    type Output = T;
    fn index(&self, i: usize) -> &T {
        &self.0[i]
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ExtendedColorType {
    Rgba8,
}

/// straight RGBA8, row-major
#[derive(Clone)]
pub struct RgbaImage {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

impl RgbaImage {
    pub fn new(w: u32, h: u32) -> RgbaImage {
        RgbaImage { w, h, px: vec![0; w as usize * h as usize * 4] }
    }
    pub fn from_raw(w: u32, h: u32, px: Vec<u8>) -> Option<RgbaImage> {
        if px.len() < w as usize * h as usize * 4 {
            return None;
        }
        Some(RgbaImage { w, h, px })
    }
    pub fn width(&self) -> u32 {
        self.w
    }
    pub fn height(&self) -> u32 {
        self.h
    }
    pub fn dimensions(&self) -> (u32, u32) {
        (self.w, self.h)
    }
    pub fn get_pixel(&self, x: u32, y: u32) -> Rgba<u8> {
        let o = (y as usize * self.w as usize + x as usize) * 4;
        Rgba([self.px[o], self.px[o + 1], self.px[o + 2], self.px[o + 3]])
    }
    pub fn put_pixel(&mut self, x: u32, y: u32, p: Rgba<u8>) {
        if x >= self.w || y >= self.h {
            return;
        }
        let o = (y as usize * self.w as usize + x as usize) * 4;
        self.px[o..o + 4].copy_from_slice(&p.0);
    }
    pub fn pixels(&self) -> impl Iterator<Item = Rgba<u8>> + '_ {
        self.px.chunks_exact(4).map(|c| Rgba([c[0], c[1], c[2], c[3]]))
    }
    pub fn as_raw(&self) -> &Vec<u8> {
        &self.px
    }
    pub fn into_raw(self) -> Vec<u8> {
        self.px
    }
    pub fn save<P: AsRef<str>>(&self, path: P) -> Result<(), ImageError> {
        save_buffer(path, &self.px, self.w, self.h, ExtendedColorType::Rgba8)
    }
}

/// a decoded picture
pub enum DynamicImage {
    Rgba16(u32, u32, Vec<u16>),
    Rgba8(RgbaImage),
}
impl DynamicImage {
    pub fn width(&self) -> u32 {
        match self {
            DynamicImage::Rgba16(w, _, _) => *w,
            DynamicImage::Rgba8(i) => i.w,
        }
    }
    pub fn height(&self) -> u32 {
        match self {
            DynamicImage::Rgba16(_, h, _) => *h,
            DynamicImage::Rgba8(i) => i.h,
        }
    }
    pub fn to_rgba8(&self) -> RgbaImage {
        match self {
            DynamicImage::Rgba8(i) => i.clone(),
            DynamicImage::Rgba16(w, h, px) => RgbaImage { w: *w, h: *h, px: px.iter().map(|&v| ((v as u32 * 255 + 32767) / 65535) as u8).collect() },
        }
    }
    /// straight RGBA16 (w, h, pixels)
    pub fn to_rgba16_raw(&self) -> (u32, u32, Vec<u16>) {
        match self {
            DynamicImage::Rgba16(w, h, px) => (*w, *h, px.clone()),
            DynamicImage::Rgba8(i) => (i.w, i.h, i.px.iter().map(|&v| v as u16 * 257).collect()),
        }
    }
}

pub fn load_from_memory(data: &[u8]) -> Result<DynamicImage, ImageError> {
    if super::png::is_png(data) {
        match super::png::decode(data) {
            Ok(p) => return Ok(DynamicImage::Rgba16(p.w as u32, p.h as u32, p.px)),
            Err(e) => {
                if let Some((w, h, px)) = super::decode_other(data) {
                    return Ok(DynamicImage::Rgba8(RgbaImage { w: w as u32, h: h as u32, px }));
                }
                return err(e);
            }
        }
    }
    if let Some((w, h, px)) = super::decode_other(data) {
        if px.len() >= w * h * 4 {
            return Ok(DynamicImage::Rgba8(RgbaImage { w: w as u32, h: h as u32, px }));
        }
    }
    if data.len() > 2 && data[0] == 0xFF && data[1] == 0xD8 {
        return err("JPEG pictures cannot be opened on LingOS yet - use PNG");
    }
    err("unsupported picture format (LingOS reads PNG, BMP and SVG)")
}

pub fn open<P: AsRef<str>>(path: P) -> Result<DynamicImage, ImageError> {
    let data = super::read(path.as_ref()).map_err(ImageError)?;
    load_from_memory(&data)
}

pub fn image_dimensions<P: AsRef<str>>(path: P) -> Result<(u32, u32), ImageError> {
    let data = super::read(path.as_ref()).map_err(ImageError)?;
    if let Some((w, h)) = super::png::dimensions(&data) {
        return Ok((w as u32, h as u32));
    }
    let i = load_from_memory(&data)?;
    Ok((i.width(), i.height()))
}

fn is_jpeg(path: &str) -> bool {
    let l = path.to_ascii_lowercase();
    l.ends_with(".jpg") || l.ends_with(".jpeg")
}

pub fn save_buffer<P: AsRef<str>>(path: P, px: &[u8], w: u32, h: u32, _c: ExtendedColorType) -> Result<(), ImageError> {
    let path = path.as_ref();
    if is_jpeg(path) {
        return err("JPEG files cannot be written on LingOS - save as .png");
    }
    if px.len() < w as usize * h as usize * 4 {
        return err("bad image size");
    }
    let png = super::png::encode_rgba8(w as usize, h as usize, px);
    super::write(path, &png).map_err(ImageError)
}

pub mod imageops {
    use super::RgbaImage;
    use alloc::vec::Vec;

    #[derive(Clone, Copy, Debug)]
    pub enum FilterType {
        Nearest,
        Triangle,
        CatmullRom,
        Gaussian,
        Lanczos3,
    }

    /// box average when shrinking (alpha-weighted colour), bilinear when growing
    pub fn resize(img: &RgbaImage, nw: u32, nh: u32, _f: FilterType) -> RgbaImage {
        let (w, h) = (img.w as usize, img.h as usize);
        let (nw, nh) = (nw.max(1) as usize, nh.max(1) as usize);
        let mut out = Vec::with_capacity(nw * nh * 4);
        if w == 0 || h == 0 {
            return RgbaImage::new(nw as u32, nh as u32);
        }
        let src = &img.px;
        let sx = w as f32 / nw as f32;
        let sy = h as f32 / nh as f32;
        for y in 0..nh {
            for x in 0..nw {
                if sx > 1.0 || sy > 1.0 {
                    let x0 = (x as f32 * sx) as usize;
                    let x1 = (((x + 1) as f32 * sx) as usize).clamp(x0 + 1, w);
                    let y0 = (y as f32 * sy) as usize;
                    let y1 = (((y + 1) as f32 * sy) as usize).clamp(y0 + 1, h);
                    let mut acc = [0f32; 4];
                    for yy in y0..y1 {
                        for xx in x0..x1 {
                            let o = (yy * w + xx) * 4;
                            let a = src[o + 3] as f32;
                            acc[0] += src[o] as f32 * a;
                            acc[1] += src[o + 1] as f32 * a;
                            acc[2] += src[o + 2] as f32 * a;
                            acc[3] += a;
                        }
                    }
                    let n = ((x1 - x0) * (y1 - y0)) as f32;
                    let a = acc[3];
                    let c = |v: f32| if a > 0.0 { (v / a + 0.5).min(255.0) as u8 } else { 0 };
                    out.extend_from_slice(&[c(acc[0]), c(acc[1]), c(acc[2]), (a / n + 0.5).min(255.0) as u8]);
                } else {
                    let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
                    let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
                    let (xi, yi) = ((fx as usize).min(w - 1), (fy as usize).min(h - 1));
                    let (xj, yj) = ((xi + 1).min(w - 1), (yi + 1).min(h - 1));
                    let (tx, ty) = (fx - xi as f32, fy - yi as f32);
                    for k in 0..4 {
                        let g = |xx: usize, yy: usize| src[(yy * w + xx) * 4 + k] as f32;
                        let top = g(xi, yi) + (g(xj, yi) - g(xi, yi)) * tx;
                        let bot = g(xi, yj) + (g(xj, yj) - g(xi, yj)) * tx;
                        out.push((top + (bot - top) * ty + 0.5).clamp(0.0, 255.0) as u8);
                    }
                }
            }
        }
        RgbaImage { w: nw as u32, h: nh as u32, px: out }
    }

    pub fn thumbnail(img: &RgbaImage, nw: u32, nh: u32) -> RgbaImage {
        resize(img, nw, nh, FilterType::Triangle)
    }
}
