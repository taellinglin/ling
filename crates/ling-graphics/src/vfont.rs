//! Vector fonts — extract real glyph outlines from a TTF/OTF, cache each glyph
//! once as a compact `<font>/<codepoint>.ling` vector-path file, and hand back
//! flattened polylines for crisp, resolution-independent rendering in 2D/3D/4D.
//!
//! Unlike the bitmap [`crate::font::GlyphFont`], nothing here is rasterized to a
//! fixed pixel grid: curves are preserved *as curves* on disk and flattened
//! adaptively at render time, so the same glyph stays sharp at any size or when
//! projected through the 3D/4D camera.
//!
//! ## On-disk format (`cache/fonts/<Font>/<codepoint>.ling`)
//! A tiny SVG-path-like dialect — fast to parse, diff-able, curve-preserving:
//! ```text
//! # ling glyph — font=Orbitron cp=65 char=A adv=0.6123
//! M 0.1040 0.0000
//! L 0.4510 0.7030
//! Q 0.5000 0.7600 0.5490 0.7030     ; quadratic: cx cy  x y
//! C 0.10 0.20 0.30 0.40 0.50 0.00   ; cubic:     c1 c1  c2 c2  x y
//! Z
//! ```
//! Coordinates are normalized to the em (units / `units_per_em`), y-up, baseline
//! at 0. `adv` is the normalized horizontal advance.

use std::collections::HashMap;
use std::path::PathBuf;
use ttf_parser::OutlineBuilder;

// ── Glyph geometry (normalized em space, y-up, baseline 0) ───────────────────

#[derive(Clone, Debug)]
enum Seg {
    Line([f32; 2]),
    Quad([f32; 2], [f32; 2]),            // control, end
    Cubic([f32; 2], [f32; 2], [f32; 2]), // control1, control2, end
}

#[derive(Clone, Debug, Default)]
struct Contour {
    start: [f32; 2],
    segs: Vec<Seg>,
}

#[derive(Clone, Debug, Default)]
struct Glyph {
    contours: Vec<Contour>,
    advance: f32,
}

/// Flattened polylines for one glyph plus its advance — all in normalized em
/// space (x→right, **y→up**, baseline at 0). Callers map this into 2D screen
/// space or onto a 3D plane.
#[derive(Clone)]
pub struct GlyphOutline {
    pub polylines: Vec<Vec<[f32; 2]>>,
    pub advance: f32,
}

// ── Outline extraction from ttf-parser ───────────────────────────────────────

#[derive(Default)]
struct Collector {
    contours: Vec<Contour>,
    cur: Option<Contour>,
    cp: [f32; 2],
}

impl Collector {
    fn finish_cur(&mut self) {
        if let Some(c) = self.cur.take() {
            if !c.segs.is_empty() {
                self.contours.push(c);
            }
        }
    }
}

impl OutlineBuilder for Collector {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish_cur();
        self.cp = [x, y];
        self.cur = Some(Contour { start: [x, y], segs: Vec::new() });
    }
    fn line_to(&mut self, x: f32, y: f32) {
        if let Some(c) = &mut self.cur {
            c.segs.push(Seg::Line([x, y]));
        }
        self.cp = [x, y];
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        if let Some(c) = &mut self.cur {
            c.segs.push(Seg::Quad([x1, y1], [x, y]));
        }
        self.cp = [x, y];
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        if let Some(c) = &mut self.cur {
            c.segs.push(Seg::Cubic([x1, y1], [x2, y2], [x, y]));
        }
        self.cp = [x, y];
    }
    fn close(&mut self) {
        self.finish_cur();
    }
}

// ── Curve compression: merge near-collinear consecutive line segments ────────

fn unit(d: [f32; 2]) -> Option<[f32; 2]> {
    let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if l < 1e-9 {
        None
    } else {
        Some([d[0] / l, d[1] / l])
    }
}

/// Drop redundant interior points on straight runs (Line→Line where the two
/// edges point the same way). Curves are left untouched.
fn compress_contour(start: [f32; 2], segs: &[Seg]) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::with_capacity(segs.len());
    let mut cp = start; // current on-curve point
    let mut line_a: Option<[f32; 2]> = None; // start of the last Line in `out`
    for seg in segs {
        match seg {
            Seg::Line(p) => {
                if let (Some(a), Some(Seg::Line(_))) = (line_a, out.last()) {
                    let d1 = unit([cp[0] - a[0], cp[1] - a[1]]);
                    let d2 = unit([p[0] - cp[0], p[1] - cp[1]]);
                    if let (Some(d1), Some(d2)) = (d1, d2) {
                        let cross = (d1[0] * d2[1] - d1[1] * d2[0]).abs();
                        let dot = d1[0] * d2[0] + d1[1] * d2[1];
                        if cross < 2.0e-3 && dot > 0.0 {
                            *out.last_mut().unwrap() = Seg::Line(*p); // extend a→p
                            cp = *p;
                            continue;
                        }
                    }
                }
                out.push(Seg::Line(*p));
                line_a = Some(cp);
                cp = *p;
            },
            Seg::Quad(c, p) => {
                out.push(Seg::Quad(*c, *p));
                cp = *p;
                line_a = None;
            },
            Seg::Cubic(a, b, p) => {
                out.push(Seg::Cubic(*a, *b, *p));
                cp = *p;
                line_a = None;
            },
        }
    }
    out
}

// ── Adaptive flattening (de Casteljau, screen-pixel tolerance in em units) ───

fn flat_quad(p0: [f32; 2], c: [f32; 2], p1: [f32; 2], tol: f32, out: &mut Vec<[f32; 2]>) {
    // distance from control point to the chord
    let dx = p1[0] - p0[0];
    let dy = p1[1] - p0[1];
    let d = ((c[0] - p0[0]) * dy - (c[1] - p0[1]) * dx).abs();
    let chord2 = dx * dx + dy * dy;
    if d * d <= tol * tol * chord2 || chord2 < 1e-12 {
        out.push(p1);
        return;
    }
    let m = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
    let p01 = m(p0, c);
    let p12 = m(c, p1);
    let mid = m(p01, p12);
    flat_quad(p0, p01, mid, tol, out);
    flat_quad(mid, p12, p1, tol, out);
}

fn flat_cubic(
    p0: [f32; 2],
    c1: [f32; 2],
    c2: [f32; 2],
    p1: [f32; 2],
    tol: f32,
    out: &mut Vec<[f32; 2]>,
) {
    let dx = p1[0] - p0[0];
    let dy = p1[1] - p0[1];
    let d1 = ((c1[0] - p0[0]) * dy - (c1[1] - p0[1]) * dx).abs();
    let d2 = ((c2[0] - p0[0]) * dy - (c2[1] - p0[1]) * dx).abs();
    let chord2 = dx * dx + dy * dy;
    if (d1 + d2) * (d1 + d2) <= tol * tol * chord2 || chord2 < 1e-12 {
        out.push(p1);
        return;
    }
    let m = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
    let p01 = m(p0, c1);
    let p12 = m(c1, c2);
    let p23 = m(c2, p1);
    let p012 = m(p01, p12);
    let p123 = m(p12, p23);
    let mid = m(p012, p123);
    flat_cubic(p0, p01, p012, mid, tol, out);
    flat_cubic(mid, p123, p23, p1, tol, out);
}

// ── The font ─────────────────────────────────────────────────────────────────

pub struct VectorFont {
    bytes: Vec<u8>,
    name: String,
    upm: f32,
    ascent: f32,  // normalized
    descent: f32, // normalized (negative)
    /// Desired weight on the variable-font `wght` axis (e.g. 600 for a bold,
    /// solid UI look). `None` → use the font's default instance.
    weight: Option<f32>,
    cache_dir: PathBuf,
    glyphs: HashMap<char, Glyph>,
    /// Tessellated-outline cache keyed by (char, tolerance bucket). Flattening the
    /// béziers is the per-call cost; caching it makes repeated per-frame draws of
    /// the same glyphs (UI text, glyph rings, …) effectively free.
    outline_cache: HashMap<(char, u32), GlyphOutline>,
}

impl VectorFont {
    /// Load a font from a TTF/OTF file using its default weight.
    pub fn from_path(path: &str) -> Result<Self, String> {
        Self::from_path_weight(path, None)
    }

    /// Load a font, optionally pinning the variable-font weight axis (`wght`).
    /// The glyph cache lives at `cache/fonts/<file-stem>[@<weight>]/`.
    pub fn from_path_weight(path: &str, weight: Option<f32>) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        let name = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "font".into());
        Self::from_bytes(bytes, &name, weight)
    }

    pub fn from_bytes(bytes: Vec<u8>, name: &str, weight: Option<f32>) -> Result<Self, String> {
        let face = ttf_parser::Face::parse(&bytes, 0).map_err(|e| format!("{e:?}"))?;
        let upm = face.units_per_em() as f32;
        let ascent = face.ascender() as f32 / upm;
        let descent = face.descender() as f32 / upm;
        let dir = match weight {
            Some(w) => format!("{name}@{}", w as i32),
            None => name.to_string(),
        };
        let cache_dir = PathBuf::from("cache").join("fonts").join(dir);
        Ok(Self {
            bytes,
            name: name.to_string(),
            upm,
            ascent,
            descent,
            weight,
            cache_dir,
            glyphs: HashMap::new(),
            outline_cache: HashMap::new(),
        })
    }

    pub fn ascent(&self) -> f32 {
        self.ascent
    }
    pub fn descent(&self) -> f32 {
        self.descent
    }

    /// Ensure the glyph for `ch` is in memory: hot-cache → on-disk `.ling` →
    /// extract-from-TTF (then write the `.ling`).
    fn ensure(&mut self, ch: char) {
        if self.glyphs.contains_key(&ch) {
            return;
        }
        let file = self.cache_dir.join(format!("{}.ling", ch as u32));
        if let Ok(text) = std::fs::read_to_string(&file) {
            if let Some(g) = parse_glyph_ling(&text) {
                self.glyphs.insert(ch, g);
                return;
            }
        }
        let g = self.extract(ch);
        let _ = std::fs::create_dir_all(&self.cache_dir);
        let _ = std::fs::write(&file, serialize_glyph_ling(&self.name, ch, &g));
        self.glyphs.insert(ch, g);
    }

    /// Pull the outline straight from the TTF, normalize to em, compress.
    fn extract(&self, ch: char) -> Glyph {
        let mut face = match ttf_parser::Face::parse(&self.bytes, 0) {
            Ok(f) => f,
            Err(_) => return Glyph { contours: vec![], advance: 0.5 },
        };
        // Pin the weight axis on variable fonts for a bolder, solid look.
        if let Some(w) = self.weight {
            let _ = face.set_variation(ttf_parser::Tag::from_bytes(b"wght"), w);
        }
        let gid = match face.glyph_index(ch) {
            Some(g) => g,
            None => return Glyph { contours: vec![], advance: 0.5 },
        };
        let advance = face
            .glyph_hor_advance(gid)
            .map(|a| a as f32 / self.upm)
            .unwrap_or(0.5);

        let mut col = Collector::default();
        face.outline_glyph(gid, &mut col);
        col.finish_cur();

        let upm = self.upm;
        let n = |p: [f32; 2]| [p[0] / upm, p[1] / upm];
        let contours = col
            .contours
            .into_iter()
            .map(|c| {
                let start = n(c.start);
                let segs: Vec<Seg> = c
                    .segs
                    .iter()
                    .map(|s| match s {
                        Seg::Line(p) => Seg::Line(n(*p)),
                        Seg::Quad(a, p) => Seg::Quad(n(*a), n(*p)),
                        Seg::Cubic(a, b, p) => Seg::Cubic(n(*a), n(*b), n(*p)),
                    })
                    .collect();
                let segs = compress_contour(start, &segs);
                Contour { start, segs }
            })
            .collect();

        Glyph { contours, advance }
    }

    /// Normalized advance width of `ch`.
    pub fn advance(&mut self, ch: char) -> f32 {
        self.ensure(ch);
        self.glyphs[&ch].advance
    }

    /// Pixel width of `text` at size `px`.
    pub fn measure(&mut self, text: &str, px: f32) -> f32 {
        text.chars().map(|c| self.advance(c)).sum::<f32>() * px
    }

    /// Flattened outline of `ch`, with curves subdivided so the deviation stays
    /// under `tol_em` (express your pixel tolerance as `tol_px / px`).
    pub fn glyph_outline(&mut self, ch: char, tol_em: f32) -> GlyphOutline {
        let tol = tol_em.max(1e-5);
        // Cache flattened outlines: béziers are only subdivided once per (char,
        // tolerance), so drawing the same glyphs every frame stops re-tessellating.
        let key = (ch, (tol * 100_000.0) as u32);
        if let Some(o) = self.outline_cache.get(&key) {
            return o.clone();
        }
        self.ensure(ch);
        let g = &self.glyphs[&ch];
        let mut polylines = Vec::with_capacity(g.contours.len());
        for c in &g.contours {
            let mut pl = Vec::new();
            let mut cur = c.start;
            pl.push(cur);
            for s in &c.segs {
                match s {
                    Seg::Line(p) => {
                        pl.push(*p);
                        cur = *p;
                    },
                    Seg::Quad(ctrl, p) => {
                        flat_quad(cur, *ctrl, *p, tol, &mut pl);
                        cur = *p;
                    },
                    Seg::Cubic(a, b, p) => {
                        flat_cubic(cur, *a, *b, *p, tol, &mut pl);
                        cur = *p;
                    },
                }
            }
            // close the contour back to its start
            if pl.len() > 1 {
                pl.push(c.start);
            }
            polylines.push(pl);
        }
        let out = GlyphOutline { polylines, advance: g.advance };
        self.outline_cache.insert(key, out.clone());
        out
    }
}

// ── (De)serialization ────────────────────────────────────────────────────────

fn serialize_glyph_ling(font: &str, ch: char, g: &Glyph) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "# ling glyph — font={font} cp={} char={} adv={:.4}\n",
        ch as u32, ch, g.advance
    ));
    for c in &g.contours {
        s.push_str(&format!("M {:.4} {:.4}\n", c.start[0], c.start[1]));
        for seg in &c.segs {
            match seg {
                Seg::Line(p) => s.push_str(&format!("L {:.4} {:.4}\n", p[0], p[1])),
                Seg::Quad(a, p) => s.push_str(&format!(
                    "Q {:.4} {:.4} {:.4} {:.4}\n",
                    a[0], a[1], p[0], p[1]
                )),
                Seg::Cubic(a, b, p) => s.push_str(&format!(
                    "C {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}\n",
                    a[0], a[1], b[0], b[1], p[0], p[1]
                )),
            }
        }
        s.push_str("Z\n");
    }
    s
}

fn parse_glyph_ling(text: &str) -> Option<Glyph> {
    let mut advance = 0.5f32;
    let mut contours: Vec<Contour> = Vec::new();
    let mut cur: Option<Contour> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('#') {
            if let Some(i) = rest.find("adv=") {
                if let Ok(v) = rest[i + 4..]
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .parse::<f32>()
                {
                    advance = v;
                }
            }
            continue;
        }
        let mut it = line.split_whitespace();
        let op = it.next()?;
        let nums: Vec<f32> = it.filter_map(|t| t.parse::<f32>().ok()).collect();
        match op {
            "M" => {
                if let Some(c) = cur.take() {
                    contours.push(c);
                }
                cur = Some(Contour { start: [*nums.first()?, *nums.get(1)?], segs: Vec::new() });
            },
            "L" => {
                if let Some(c) = &mut cur {
                    c.segs.push(Seg::Line([*nums.first()?, *nums.get(1)?]));
                }
            },
            "Q" => {
                if let Some(c) = &mut cur {
                    c.segs.push(Seg::Quad(
                        [*nums.first()?, *nums.get(1)?],
                        [*nums.get(2)?, *nums.get(3)?],
                    ));
                }
            },
            "C" => {
                if let Some(c) = &mut cur {
                    c.segs.push(Seg::Cubic(
                        [*nums.first()?, *nums.get(1)?],
                        [*nums.get(2)?, *nums.get(3)?],
                        [*nums.get(4)?, *nums.get(5)?],
                    ));
                }
            },
            "Z" => {
                if let Some(c) = cur.take() {
                    contours.push(c);
                }
            },
            _ => {},
        }
    }
    if let Some(c) = cur.take() {
        contours.push(c);
    }
    Some(Glyph { contours, advance })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collinear_lines_merge() {
        let segs = vec![
            Seg::Line([0.5, 0.0]),
            Seg::Line([1.0, 0.0]),
            Seg::Line([1.0, 1.0]),
        ];
        let out = compress_contour([0.0, 0.0], &segs);
        // the two collinear horizontal lines collapse into one
        assert_eq!(out.len(), 2);
        match out[0] {
            Seg::Line(p) => assert_eq!(p, [1.0, 0.0]),
            _ => panic!(),
        }
    }

    #[test]
    fn glyph_ling_roundtrips() {
        let g = Glyph {
            advance: 0.6,
            contours: vec![Contour {
                start: [0.1, 0.0],
                segs: vec![
                    Seg::Line([0.4, 0.7]),
                    Seg::Quad([0.5, 0.8], [0.6, 0.7]),
                    Seg::Line([0.9, 0.0]),
                ],
            }],
        };
        let text = serialize_glyph_ling("Test", 'A', &g);
        let back = parse_glyph_ling(&text).unwrap();
        assert!((back.advance - 0.6).abs() < 1e-3);
        assert_eq!(back.contours.len(), 1);
        assert_eq!(back.contours[0].segs.len(), 3);
        // curve preserved as a curve, not flattened
        assert!(matches!(back.contours[0].segs[1], Seg::Quad(..)));
    }
}
