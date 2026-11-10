//! Project files (.lanime = zip of doc.json, 16-bit PNG layers and the 2.5D parts) and image export.

use crate::px::*;
use crate::scene::{Img, Scene};
use crate::{Doc, Layer, LayerMeta, Studio};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct DocFile {
    version: u32,
    app: String,
    w: usize,
    h: usize,
    dpi: f32,
    name: String,
    paper: [f32; 3],
    paper_on: bool,
    meta: String,
    active: usize,
    layers: Vec<LayerMeta>,
    scene: Scene,
}
impl Default for DocFile {
    fn default() -> Self {
        DocFile { version: 1, app: "LingAnime".into(), w: 1920, h: 1080, dpi: 144.0, name: String::new(), paper: [1.0; 3], paper_on: true, meta: String::new(), active: 0, layers: vec![], scene: Scene::default() }
    }
}

fn png16(w: usize, h: usize, px: Vec<u16>) -> Result<Vec<u8>, String> {
    let img = image::ImageBuffer::<image::Rgba<u16>, Vec<u16>>::from_raw(w as u32, h as u32, px).ok_or("bad image size")?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba16(img).write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

pub fn save(st: &Studio, path: &str) -> Result<(), String> {
    let d = &st.doc;
    let file = DocFile {
        version: 1,
        app: "LingAnime".into(),
        w: d.w,
        h: d.h,
        dpi: d.dpi,
        name: d.name.clone(),
        paper: d.paper,
        paper_on: d.paper_on,
        meta: d.meta.clone(),
        active: d.active,
        layers: d.layers.iter().map(|l| l.m.clone()).collect(),
        scene: d.scene.clone(),
    };
    let tmp = format!("{path}.saving");
    {
        let f = std::fs::File::create(&tmp).map_err(|e| format!("cannot write {path}: {e}"))?;
        let mut z = zip::ZipWriter::new(f);
        let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(true);
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
        z.start_file("doc.json", opt).map_err(|e| e.to_string())?;
        z.write_all(serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?.as_bytes()).map_err(|e| e.to_string())?;
        for (i, l) in d.layers.iter().enumerate() {
            if l.m.kind == 1 || l.t.is_empty() {
                continue;
            }
            let png = png16(l.t.w, l.t.h, l.t.to_rgba16())?;
            z.start_file(format!("layers/{i}.png"), stored).map_err(|e| e.to_string())?;
            z.write_all(&png).map_err(|e| e.to_string())?;
        }
        for (i, p) in d.scene.parts.iter().enumerate() {
            let img = &p.img;
            if img.w == 0 {
                continue;
            }
            let mut out = std::io::Cursor::new(Vec::new());
            image::write_buffer_with_format(&mut out, &img.to_straight(), img.w as u32, img.h as u32, image::ExtendedColorType::Rgba8, image::ImageFormat::Png).map_err(|e| e.to_string())?;
            z.start_file(format!("parts/{i}.png"), stored).map_err(|e| e.to_string())?;
            z.write_all(&out.into_inner()).map_err(|e| e.to_string())?;
        }
        // a flattened preview so file browsers and other apps can show the picture
        let flat = st.flatten16(true);
        let k = (512.0 / d.w.max(d.h) as f32).min(1.0);
        let (tw, th) = (((d.w as f32 * k) as u32).max(1), ((d.h as f32 * k) as u32).max(1));
        let img8: Vec<u8> = flat.iter().map(|v| (v >> 8) as u8).collect();
        if let Some(full) = image::RgbaImage::from_raw(d.w as u32, d.h as u32, img8) {
            let thumb = image::imageops::thumbnail(&full, tw, th);
            let mut out = std::io::Cursor::new(Vec::new());
            if thumb.write_to(&mut out, image::ImageFormat::Png).is_ok() {
                z.start_file("thumbnail.png", stored).map_err(|e| e.to_string())?;
                z.write_all(&out.into_inner()).map_err(|e| e.to_string())?;
            }
        }
        z.finish().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace {path}: {e}"))?;
    Ok(())
}

pub fn open(path: &str) -> Result<Doc, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("{path} is not a LingAnime project: {e}"))?;
    let mut txt = String::new();
    z.by_name("doc.json").map_err(|_| "doc.json missing".to_string())?.read_to_string(&mut txt).map_err(|e| e.to_string())?;
    let file: DocFile = serde_json::from_str(&txt).map_err(|e| format!("doc.json: {e}"))?;
    let (w, h) = (file.w, file.h);
    let mut layers = Vec::new();
    for (i, m) in file.layers.into_iter().enumerate() {
        let mut l = Layer { m, t: Tiles::new(w, h) };
        if l.m.kind == 1 {
            l.reraster();
        } else if let Ok(mut e) = z.by_name(&format!("layers/{i}.png")) {
            let mut buf = Vec::new();
            e.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            let img = image::load_from_memory(&buf).map_err(|e| e.to_string())?.to_rgba16();
            if img.width() as usize == w && img.height() as usize == h {
                l.t.put_rgba16(&img.into_raw());
            }
        }
        layers.push(l);
    }
    if layers.is_empty() {
        layers.push(Layer { m: LayerMeta::default(), t: Tiles::new(w, h) });
    }
    let mut scene = file.scene;
    for i in 0..scene.parts.len() {
        if let Ok(mut e) = z.by_name(&format!("parts/{i}.png")) {
            let mut buf = Vec::new();
            e.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            let img = image::load_from_memory(&buf).map_err(|e| e.to_string())?.to_rgba8();
            let (iw, ih) = (img.width() as usize, img.height() as usize);
            scene.parts[i].img = Arc::new(Img::from_straight(iw, ih, &img.into_raw()));
            scene.parts[i].rebuild_occ();
        }
    }
    let active = file.active.min(layers.len() - 1);
    Ok(Doc { w, h, dpi: file.dpi, name: file.name, paper: file.paper, paper_on: file.paper_on, meta: file.meta, layers, active, sel: None, scene })
}

fn save_rgba16(path: &str, w: usize, h: usize, px: Vec<u16>, scale: f32, paper: Option<[f32; 3]>) -> Result<(), String> {
    let lower = path.to_ascii_lowercase();
    let jpg = lower.ends_with(".jpg") || lower.ends_with(".jpeg");
    let mut img = image::ImageBuffer::<image::Rgba<u16>, Vec<u16>>::from_raw(w as u32, h as u32, px).ok_or("bad image size")?;
    if (scale - 1.0).abs() > 1e-3 {
        let nw = ((w as f32 * scale).round() as u32).max(1);
        let nh = ((h as f32 * scale).round() as u32).max(1);
        img = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Lanczos3);
    }
    if jpg {
        let bg = paper.unwrap_or([1.0, 1.0, 1.0]);
        let rgb = image::RgbImage::from_fn(img.width(), img.height(), |x, y| {
            let p = img.get_pixel(x, y).0;
            let a = p[3] as f32 / 65535.0;
            let c = |i: usize| ((p[i] as f32 / 65535.0 * a + bg[i] * (1.0 - a)) * 255.0 + 0.5) as u8;
            image::Rgb([c(0), c(1), c(2)])
        });
        return rgb.save(path).map_err(|e| e.to_string());
    }
    let img8 = image::DynamicImage::ImageRgba16(img).to_rgba8();
    img8.save(path).map_err(|e| e.to_string())
}

pub fn export_flat(st: &Studio, path: &str, scale: f32, paper: bool) -> Result<(), String> {
    let px = st.flatten16(paper);
    save_rgba16(path, st.doc.w, st.doc.h, px, scale, if paper && st.doc.paper_on { Some(st.doc.paper) } else { None })
}

pub fn export_layer(st: &Studio, i: usize, path: &str) -> Result<(), String> {
    let l = st.doc.layers.get(i).ok_or("no such layer")?;
    save_rgba16(path, l.t.w, l.t.h, l.t.to_rgba16(), 1.0, None)
}

pub fn export_layers(st: &Studio, dir: &str) -> Result<usize, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut n = 0;
    for (i, l) in st.doc.layers.iter().enumerate() {
        if l.t.is_empty() {
            continue;
        }
        let safe: String = l.m.name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        let p = std::path::Path::new(dir).join(format!("{:02}_{}.png", i, safe));
        save_rgba16(&p.to_string_lossy(), l.t.w, l.t.h, l.t.to_rgba16(), 1.0, None)?;
        n += 1;
    }
    Ok(n)
}
