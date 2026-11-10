//! Project files and image export in the no_std build: the same .lanime layout as the desktop's `io` module
//! (a zip of doc.json, 16-bit PNG layers, the 2.5D parts' PNGs and a thumbnail), through the in-crate zip / PNG
//! codecs and the host's files.

#[allow(unused_imports)]
use crate::prelude::*;
use crate::os::{self, png, zip};
use crate::px::*;
use crate::scene::{Img, Scene};
use crate::{Doc, Layer, LayerMeta, Studio};
use alloc::sync::Arc;
use serde::{Deserialize, Serialize};

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
    let mut z = zip::Writer::new();
    z.add("doc.json", serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?.as_bytes(), true);
    for (i, l) in d.layers.iter().enumerate() {
        if l.m.kind == 1 || l.t.is_empty() {
            continue;
        }
        let png = png::encode_rgba16(l.t.w, l.t.h, &l.t.to_rgba16());
        z.add(&format!("layers/{i}.png"), &png, false);
    }
    for (i, p) in d.scene.parts.iter().enumerate() {
        let img = &p.img;
        if img.w == 0 {
            continue;
        }
        z.add(&format!("parts/{i}.png"), &png::encode_rgba8(img.w, img.h, &img.to_straight()), false);
    }
    // a flattened preview so file browsers and other apps can show the picture
    let flat = st.flatten16(true);
    let k = (512.0 / d.w.max(d.h) as f32).min(1.0);
    let (tw, th) = (((d.w as f32 * k) as u32).max(1), ((d.h as f32 * k) as u32).max(1));
    let img8: Vec<u8> = flat.iter().map(|v| (v >> 8) as u8).collect();
    drop(flat);
    if let Some(full) = os::image::RgbaImage::from_raw(d.w as u32, d.h as u32, img8) {
        let thumb = os::image::imageops::thumbnail(&full, tw, th);
        z.add("thumbnail.png", &png::encode_rgba8(thumb.width() as usize, thumb.height() as usize, thumb.as_raw()), false);
    }
    os::write(path, &z.finish())
}

pub fn open(path: &str) -> Result<Doc, String> {
    let data = os::read(path).map_err(|_| format!("cannot open {path}"))?;
    let z = zip::Reader::new(&data).map_err(|e| format!("{path} is not a LingAnime project: {e}"))?;
    let txt = z.read("doc.json").ok_or_else(|| "doc.json missing".to_string())??;
    let file: DocFile = serde_json::from_slice(&txt).map_err(|e| format!("doc.json: {e}"))?;
    let (w, h) = (file.w, file.h);
    let mut layers = Vec::new();
    for (i, m) in file.layers.into_iter().enumerate() {
        let mut l = Layer { m, t: Tiles::new(w, h) };
        if l.m.kind == 1 {
            l.reraster();
        } else if let Some(buf) = z.read(&format!("layers/{i}.png")) {
            let img = os::image::load_from_memory(&buf?).map_err(|e| e.to_string())?;
            let (iw, ih, px) = img.to_rgba16_raw();
            if iw as usize == w && ih as usize == h {
                l.t.put_rgba16(&px);
            }
        }
        layers.push(l);
    }
    if layers.is_empty() {
        layers.push(Layer { m: LayerMeta::default(), t: Tiles::new(w, h) });
    }
    let mut scene = file.scene;
    for i in 0..scene.parts.len() {
        if let Some(buf) = z.read(&format!("parts/{i}.png")) {
            let img = os::image::load_from_memory(&buf?).map_err(|e| e.to_string())?.to_rgba8();
            let (iw, ih) = (img.width() as usize, img.height() as usize);
            scene.parts[i].img = Arc::new(Img::from_straight(iw, ih, &img.into_raw()));
            scene.parts[i].rebuild_occ();
        }
    }
    let active = file.active.min(layers.len() - 1);
    Ok(Doc { w, h, dpi: file.dpi, name: file.name, paper: file.paper, paper_on: file.paper_on, meta: file.meta, layers, active, sel: None, scene })
}

fn save_rgba16(path: &str, w: usize, h: usize, px: Vec<u16>, scale: f32, _paper: Option<[f32; 3]>) -> Result<(), String> {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        return Err("JPEG files cannot be written on LingOS - export as .png".into());
    }
    if (scale - 1.0).abs() > 1e-3 {
        let nw = ((w as f32 * scale).round() as u32).max(1);
        let nh = ((h as f32 * scale).round() as u32).max(1);
        let img8: Vec<u8> = px.iter().map(|&v| ((v as u32 * 255 + 32767) / 65535) as u8).collect();
        drop(px);
        let img = os::image::RgbaImage::from_raw(w as u32, h as u32, img8).ok_or("bad image size")?;
        let img = os::image::imageops::resize(&img, nw, nh, os::image::imageops::FilterType::Lanczos3);
        return os::write(path, &png::encode_rgba8(nw as usize, nh as usize, img.as_raw()));
    }
    let img8: Vec<u8> = px.iter().map(|&v| ((v as u32 * 255 + 32767) / 65535) as u8).collect();
    drop(px);
    os::write(path, &png::encode_rgba8(w, h, &img8))
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
    os::mkdir_all(dir);
    let mut n = 0;
    for (i, l) in st.doc.layers.iter().enumerate() {
        if l.t.is_empty() {
            continue;
        }
        let safe: String = l.m.name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        let p = os::join(dir, &format!("{:02}_{}.png", i, safe));
        save_rgba16(&p, l.t.w, l.t.h, l.t.to_rgba16(), 1.0, None)?;
        n += 1;
    }
    Ok(n)
}
