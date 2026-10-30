//! Images for a browser-shaped program: fetched and decoded off the interpreter thread.
//!
//! `image_fetch(url)` starts a job (http / https with the `web` feature, `data:` URIs, local
//! paths) and returns its id at once; `image_job(id)` reports -2 while it runs, -1 when it
//! failed, else the decoded image's handle in the runtime's image table (the one `image_draw`
//! reads). PNG, JPEG, GIF (first frame), WebP, BMP, ICO and SVG are understood. Pictures are
//! kept at most `MAX_SIDE` pixels on a side.

use std::sync::Mutex;

use base64::Engine;
use image::RgbaImage;

const MAX_SIDE: u32 = 2048;
const MAX_BYTES: usize = 24 * 1024 * 1024;
/// at most this many fetches in flight; the rest wait their turn
const MAX_WORKERS: usize = 6;

enum Job {
    Waiting(String),
    Running,
    Done(RgbaImage),
    Failed,
    Taken(usize),
}

static JOBS: Mutex<Vec<Job>> = Mutex::new(Vec::new());
static RUNNING: Mutex<usize> = Mutex::new(0);

/// Start fetching `url`; returns the job id.
pub fn fetch(url: &str) -> usize {
    let mut jobs = JOBS.lock().unwrap();
    jobs.push(Job::Waiting(url.to_string()));
    let id = jobs.len() - 1;
    drop(jobs);
    pump();
    id
}

/// Start waiting jobs while fewer than MAX_WORKERS run.
fn pump() {
    loop {
        let mut running = RUNNING.lock().unwrap();
        if *running >= MAX_WORKERS {
            return;
        }
        let mut jobs = JOBS.lock().unwrap();
        let Some(id) = jobs.iter().position(|j| matches!(j, Job::Waiting(_))) else {
            return;
        };
        let Job::Waiting(url) = std::mem::replace(&mut jobs[id], Job::Running) else {
            return;
        };
        *running += 1;
        drop(jobs);
        drop(running);
        std::thread::spawn(move || {
            let out = load(&url);
            if let Err(e) = &out {
                if std::env::var_os("LING_IMG_DEBUG").is_some() {
                    eprintln!("image_fetch {url}: {e}");
                }
            }
            JOBS.lock().unwrap()[id] = match out {
                Ok(img) => Job::Done(img),
                Err(_) => Job::Failed,
            };
            *RUNNING.lock().unwrap() -= 1;
            pump();
        });
    }
}

/// -2 running, -1 failed, else the image handle (the decoded picture moves into `images`).
pub fn poll(id: usize, images: &mut Vec<RgbaImage>) -> f64 {
    let mut jobs = JOBS.lock().unwrap();
    let Some(job) = jobs.get_mut(id) else {
        return -1.0;
    };
    match job {
        Job::Waiting(_) | Job::Running => -2.0,
        Job::Failed => -1.0,
        Job::Taken(h) => *h as f64,
        Job::Done(_) => {
            let Job::Done(img) = std::mem::replace(job, Job::Failed) else {
                return -1.0;
            };
            images.push(img);
            let h = images.len() - 1;
            *job = Job::Taken(h);
            h as f64
        },
    }
}

/// Jobs not started yet are dropped (a new page was opened).
pub fn cancel_waiting() {
    for j in JOBS.lock().unwrap().iter_mut() {
        if matches!(j, Job::Waiting(_)) {
            *j = Job::Failed;
        }
    }
}

fn load(url: &str) -> Result<RgbaImage, String> {
    let (bytes, ctype) = if let Some(rest) = url.strip_prefix("data:") {
        data_uri(rest)?
    } else if url.starts_with("http://") || url.starts_with("https://") {
        #[cfg(feature = "web")]
        {
            super::web::get_bytes_blocking(url, MAX_BYTES)?
        }
        #[cfg(not(feature = "web"))]
        {
            return Err("built without the web feature".into());
        }
    } else {
        let path = url.strip_prefix("file:").unwrap_or(url).trim_start_matches("///");
        (std::fs::read(path).map_err(|e| e.to_string())?, String::new())
    };
    decode(&bytes, &ctype)
}

/// `data:[<type>][;base64],<data>`
fn data_uri(rest: &str) -> Result<(Vec<u8>, String), String> {
    let comma = rest.find(',').ok_or("bad data: URI")?;
    let (meta, data) = (&rest[..comma], &rest[comma + 1..]);
    let ctype = meta.split(';').next().unwrap_or("").to_ascii_lowercase();
    let bytes = if meta.ends_with(";base64") {
        let clean: String = data.chars().filter(|c| !c.is_whitespace()).collect();
        base64::engine::general_purpose::STANDARD
            .decode(clean.trim_end_matches('='))
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(clean.trim_end_matches('=')))
            .map_err(|e| e.to_string())?
    } else {
        percent_decode(data).into_bytes()
    };
    Ok((bytes, ctype))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn looks_svg(bytes: &[u8], ctype: &str) -> bool {
    if ctype.contains("svg") {
        return true;
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    head.contains("<svg") || (head.trim_start().starts_with("<?xml") && !head.contains("<html"))
}

pub fn decode(bytes: &[u8], ctype: &str) -> Result<RgbaImage, String> {
    if looks_svg(bytes, ctype) {
        return svg(bytes, 0, 0);
    }
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?.to_rgba8();
    Ok(fit(img))
}

fn fit(img: RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w <= MAX_SIDE && h <= MAX_SIDE {
        return img;
    }
    let k = MAX_SIDE as f64 / w.max(h) as f64;
    let (nw, nh) = (((w as f64 * k) as u32).max(1), ((h as f64 * k) as u32).max(1));
    image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
}

/// Rasterise SVG markup at (w, h) pixels; 0 = the picture's own size (or its viewBox's).
pub fn svg(bytes: &[u8], w: u32, h: u32) -> Result<RgbaImage, String> {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(bytes, &opt).map_err(|e| e.to_string())?;
    let ts = tree.size();
    let (sw, sh) = (ts.width().max(1.0), ts.height().max(1.0));
    let (mut tw, mut th) = (w, h);
    if tw == 0 && th == 0 {
        tw = sw.ceil() as u32;
        th = sh.ceil() as u32;
    } else if tw == 0 {
        tw = (th as f32 * sw / sh).ceil() as u32;
    } else if th == 0 {
        th = (tw as f32 * sh / sw).ceil() as u32;
    }
    let (tw, th) = (tw.clamp(1, MAX_SIDE), th.clamp(1, MAX_SIDE));
    let mut pm = resvg::tiny_skia::Pixmap::new(tw, th).ok_or("pixmap")?;
    let tf = resvg::tiny_skia::Transform::from_scale(tw as f32 / sw, th as f32 / sh);
    resvg::render(&tree, tf, &mut pm.as_mut());
    let mut img = RgbaImage::new(tw, th);
    for (p, c) in img.pixels_mut().zip(pm.pixels()) {
        // tiny-skia pixels are premultiplied
        let a = c.alpha() as u32;
        let un = |v: u8| if a == 0 { 0 } else { ((v as u32 * 255 + a / 2) / a).min(255) as u8 };
        *p = image::Rgba([un(c.red()), un(c.green()), un(c.blue()), a as u8]);
    }
    Ok(img)
}

/// A smoothly resampled copy of `img` at (w, h): drawing a large photo small with nearest
/// sampling shimmers, so a page asks for a copy at the size it shows the picture.
pub fn resized(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let (w, h) = (w.clamp(1, MAX_SIDE), h.clamp(1, MAX_SIDE));
    let filter = if w < img.width() / 2 { image::imageops::FilterType::Triangle } else { image::imageops::FilterType::CatmullRom };
    image::imageops::resize(img, w, h, filter)
}

// ── downloads: file_fetch(url, path) / file_job(id) ──

enum FileJob {
    Running,
    Done(f64),
    Failed,
}

static FILE_JOBS: Mutex<Vec<FileJob>> = Mutex::new(Vec::new());

/// Start saving `url` (http / https, a data: URI, or a local path) to `path`; returns the job id.
pub fn file_fetch(url: &str, path: &str) -> usize {
    let mut jobs = FILE_JOBS.lock().unwrap();
    jobs.push(FileJob::Running);
    let id = jobs.len() - 1;
    drop(jobs);
    let (url, path) = (url.to_string(), path.to_string());
    std::thread::spawn(move || {
        let bytes: Result<Vec<u8>, String> = if let Some(rest) = url.strip_prefix("data:") {
            data_uri(rest).map(|(b, _)| b)
        } else if url.starts_with("http://") || url.starts_with("https://") {
            #[cfg(feature = "web")]
            {
                super::web::get_bytes_blocking(&url, 2 * 1024 * 1024 * 1024).map(|(b, _)| b)
            }
            #[cfg(not(feature = "web"))]
            {
                Err("built without the web feature".to_string())
            }
        } else {
            let p = url.strip_prefix("file:").unwrap_or(&url).trim_start_matches("///");
            std::fs::read(p).map_err(|e| e.to_string())
        };
        let out = bytes.and_then(|b| std::fs::write(&path, &b).map(|_| b.len() as f64).map_err(|e| e.to_string()));
        FILE_JOBS.lock().unwrap()[id] = match out {
            Ok(n) => FileJob::Done(n),
            Err(_) => FileJob::Failed,
        };
    });
    id
}

/// -2 running, -1 failed, else the number of bytes written
pub fn file_job(id: usize) -> f64 {
    match FILE_JOBS.lock().unwrap().get(id) {
        Some(FileJob::Running) => -2.0,
        Some(FileJob::Done(n)) => *n,
        _ => -1.0,
    }
}
