//! Bridge between Ling values and the `ling-daw` engine (`daw_*`, `fs_*`, `dlg_*`
//! builtins) plus two small toolkits used by LingStationX:
//!   * `st_*`  — a global key/value store so top-level functions can share state
//!   * `px_*`  — fast alpha-blended, clip-aware pixel primitives
use super::{EvalErr, EvalResult, Interpreter, Value};
use ling_daw::{Daw, Val};
use std::rc::Rc;

fn to_val(v: &Value) -> Val {
    match v {
        Value::Number(n) => Val::Num(*n),
        Value::Bool(b) => Val::Num(if *b { 1.0 } else { 0.0 }),
        Value::Str(s) => Val::Str(s.clone()),
        Value::List(l) => Val::List(l.iter().map(to_val).collect()),
        other => Val::Str(other.to_string()),
    }
}

fn from_val(v: Val) -> Value {
    match v {
        Val::Num(n) => Value::Number(n),
        Val::Str(s) => Value::Str(s),
        Val::List(l) => Value::List(Rc::new(l.into_iter().map(from_val).collect())),
        Val::Unit => Value::Unit,
    }
}

#[inline]
fn blend(dst: u32, r: u32, g: u32, b: u32, a: f32) -> u32 {
    let mix = |s: u32, d: u32| ((s as f32 * a + d as f32 * (1.0 - a)) as u32).min(255);
    (mix(r, (dst >> 16) & 255) << 16) | (mix(g, (dst >> 8) & 255) << 8) | mix(b, dst & 255)
}

fn unpack(c: f64) -> (u32, u32, u32) {
    let c = c.max(0.0) as u32;
    ((c >> 16) & 255, (c >> 8) & 255, c & 255)
}

/// LingStation's "Black" colour scheme: remap the chrome greys to near-black.
fn black_theme(rgb: (u32, u32, u32)) -> (u32, u32, u32) {
    match rgb {
        (27, 27, 27) => (0, 0, 0),
        (22, 23, 26) => (4, 4, 5),
        (60, 60, 60) => (26, 26, 26),
        (74, 74, 74) => (38, 38, 38),
        (45, 45, 45) => (16, 16, 16),
        (30, 30, 34) => (10, 10, 12),
        (70, 70, 78) => (48, 48, 54),
        (205, 208, 216) => (245, 245, 245),
        (140, 144, 152) => (168, 172, 180),
        other => other,
    }
}

impl Interpreter {
    /// Unpack a packed colour, applying the active colour scheme.
    fn tu(&self, c: f64) -> (u32, u32, u32) {
        let rgb = unpack(c);
        if self.px_black {
            black_theme(rgb)
        } else {
            rgb
        }
    }

    /// `None` when `name` is not one of the LingStationX builtins.
    pub(super) fn daw_builtin(&mut self, name: &str, args: &[Value]) -> Option<EvalResult> {
        if name.starts_with("st_") {
            return self.st_builtin(name, args);
        }
        if name.starts_with("px_") {
            return self.px_builtin(name, args);
        }
        if !(name.starts_with("daw_") || name.starts_with("fs_") || name.starts_with("dlg_")) {
            return None;
        }
        let vals: Vec<Val> = args.iter().map(to_val).collect();
        let daw = self.daw.get_or_insert_with(Daw::new);
        let r = daw.call(name, &vals)?;
        Some(r.map(from_val).map_err(EvalErr::Runtime))
    }

    fn num(args: &[Value], i: usize, d: f64) -> f64 {
        match args.get(i) {
            Some(Value::Number(n)) => *n,
            Some(Value::Bool(b)) => *b as i32 as f64,
            Some(Value::Str(s)) => s.parse().unwrap_or(d),
            _ => d,
        }
    }

    fn st_builtin(&mut self, name: &str, args: &[Value]) -> Option<EvalResult> {
        let key = args.first().map(|v| v.to_string()).unwrap_or_default();
        let ok = |v: Value| Some(Ok(v));
        match name {
            "st_set" => {
                let v = args.get(1).cloned().unwrap_or(Value::Unit);
                self.gstore.insert(key, v);
                ok(Value::Unit)
            }
            "st_get" => ok(self.gstore.get(&key).cloned().or_else(|| args.get(1).cloned()).unwrap_or(Value::Number(0.0))),
            "st_has" => ok(Value::Number(self.gstore.contains_key(&key) as i32 as f64)),
            "st_add" => {
                let cur = match self.gstore.get(&key) {
                    Some(Value::Number(n)) => *n,
                    _ => 0.0,
                };
                let nv = cur + Self::num(args, 1, 0.0);
                self.gstore.insert(key, Value::Number(nv));
                ok(Value::Number(nv))
            }
            "st_llen" => ok(Value::Number(match self.gstore.get(&key) {
                Some(Value::List(l)) => l.len() as f64,
                _ => 0.0,
            })),
            "st_lget" => {
                let i = Self::num(args, 1, 0.0) as usize;
                ok(match self.gstore.get(&key) {
                    Some(Value::List(l)) => l.get(i).cloned().unwrap_or(Value::Number(0.0)),
                    _ => Value::Number(0.0),
                })
            }
            "st_lset" => {
                let i = Self::num(args, 1, 0.0).max(0.0) as usize;
                if i >= (1 << 22) {
                    return ok(Value::Unit);
                }
                let v = args.get(2).cloned().unwrap_or(Value::Unit);
                let e = self.gstore.entry(key).or_insert_with(|| Value::List(Rc::new(vec![])));
                if let Value::List(l) = e {
                    let l = Rc::make_mut(l);
                    while l.len() <= i {
                        l.push(Value::Number(0.0));
                    }
                    l[i] = v;
                }
                ok(Value::Unit)
            }
            "st_lpush" => {
                let v = args.get(1).cloned().unwrap_or(Value::Unit);
                let e = self.gstore.entry(key).or_insert_with(|| Value::List(Rc::new(vec![])));
                if let Value::List(l) = e {
                    Rc::make_mut(l).push(v);
                }
                ok(Value::Unit)
            }
            "st_ldel" => {
                let i = Self::num(args, 1, 0.0) as usize;
                if let Some(Value::List(l)) = self.gstore.get_mut(&key) {
                    let l = Rc::make_mut(l);
                    if i < l.len() {
                        l.remove(i);
                    }
                }
                ok(Value::Unit)
            }
            "st_lclear" => {
                self.gstore.insert(key, Value::List(Rc::new(vec![])));
                ok(Value::Unit)
            }
            _ => None,
        }
    }

    fn px_builtin(&mut self, name: &str, args: &[Value]) -> Option<EvalResult> {
        let n = |i: usize| Self::num(args, i, 0.0);
        let unit = Some(Ok(Value::Unit));
        match name {
            "px_text" => {
                // px_text(font, x, y, size, colour, text) — anti-aliased, honours px_clip
                let id = n(0) as i64;
                let (x, y, px) = (n(1) as f32, n(2) as f32, n(3) as f32);
                let (r, g, b) = self.tu(n(4));
                let color = (r << 16) | (g << 8) | b;
                let text = args.get(5).map(|v| v.to_string()).unwrap_or_default();
                if id < 0 || id as usize >= self.fonts.len() || px <= 0.0 || text.is_empty() {
                    return unit;
                }
                let clip = self.px_clip;
                let (sw, sh) = {
                    let g = self.gfx.borrow();
                    (g.width as i32, g.height as i32)
                };
                let (cx0, cy0, cx1, cy1) = match clip {
                    Some(c) => (c[0].max(0), c[1].max(0), c[2].min(sw), c[3].min(sh)),
                    None => (0, 0, sw, sh),
                };
                // cheap cull: text line box vs clip
                if y > cy1 as f32 || y + px * 1.3 < cy0 as f32 || x > cx1 as f32 {
                    return unit;
                }
                let glyphs = self.font_layout_2d_glyphs(id as usize, x, y, px, &text);
                let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for gl in &glyphs {
                    for c in gl {
                        for p in c {
                            minx = minx.min(p[0]);
                            maxx = maxx.max(p[0]);
                            miny = miny.min(p[1]);
                            maxy = maxy.max(p[1]);
                        }
                    }
                }
                if minx > maxx {
                    return unit;
                }
                let (bx0, by0) = ((minx.floor() as i32 - 2).max(0), (miny.floor() as i32 - 2).max(0));
                let (bx1, by1) = ((maxx.ceil() as i32 + 2).min(sw), (maxy.ceil() as i32 + 2).min(sh));
                if bx0 >= bx1 || by0 >= by1 || bx1 <= cx0 || bx0 >= cx1 || by1 <= cy0 || by0 >= cy1 {
                    return unit;
                }
                let mut gfx = self.gfx.borrow_mut();
                let (w, h) = (gfx.width, gfx.height);
                let inside = bx0 >= cx0 && bx1 <= cx1 && by0 >= cy0 && by1 <= cy1;
                if inside {
                    for contours in &glyphs {
                        crate::gfx::raster::fill_contours_aa(&mut gfx.buffer, w, h, color, false, contours);
                    }
                } else {
                    // rasterise on a scratch copy of the bbox, then copy back only what is inside the clip
                    let (tw, th) = ((bx1 - bx0) as usize, (by1 - by0) as usize);
                    let mut scratch = vec![0u32; tw * th];
                    for row in 0..th {
                        let src = (by0 as usize + row) * w + bx0 as usize;
                        scratch[row * tw..row * tw + tw].copy_from_slice(&gfx.buffer[src..src + tw]);
                    }
                    for contours in &glyphs {
                        let shifted: Vec<Vec<[f32; 2]>> = contours.iter().map(|c| c.iter().map(|p| [p[0] - bx0 as f32, p[1] - by0 as f32]).collect()).collect();
                        crate::gfx::raster::fill_contours_aa(&mut scratch, tw, th, color, false, &shifted);
                    }
                    let (ix0, iy0, ix1, iy1) = (bx0.max(cx0), by0.max(cy0), bx1.min(cx1), by1.min(cy1));
                    for yy in iy0..iy1 {
                        for xx in ix0..ix1 {
                            gfx.buffer[yy as usize * w + xx as usize] = scratch[(yy - by0) as usize * tw + (xx - bx0) as usize];
                        }
                    }
                }
                unit
            }
            "px_theme" => {
                self.px_black = n(0) > 0.5;
                unit
            }
            "px_flat" => {
                // 2-D UI mode: switch off the 3-D cel-shading tone ramp / post effects
                let mut g = self.gfx.borrow_mut();
                g.toon.ramp.stops.clear();
                g.toon.outline_px = 0.0;
                g.toon.bloom_strength = 0.0;
                g.toon.ao_strength = 0.0;
                g.toon.fxaa = false;
                unit
            }
            "px_clip" => {
                let (x, y, w, h) = (n(0), n(1), n(2), n(3));
                self.px_clip = Some([x.floor() as i32, y.floor() as i32, (x + w).ceil() as i32, (y + h).ceil() as i32]);
                unit
            }
            "px_color" => {
                // px_color(c) — set the pen (text / draw_poly) from a packed colour
                let (r, g, b) = self.tu(n(0));
                self.gfx.borrow_mut().color = (r << 16) | (g << 8) | b;
                unit
            }
            "px_unclip" => {
                self.px_clip = None;
                unit
            }
            "px_rect" | "px_outline" | "px_hline" | "px_vline" => {
                let (x, y, mut w, mut h) = (n(0), n(1), n(2), n(3));
                let (r, g, b, a);
                match name {
                    "px_hline" => {
                        // (x, y, len, colour [, a])
                        h = 1.0;
                        (r, g, b) = self.tu(n(3));
                        a = Self::num(args, 4, 1.0) as f32;
                    }
                    "px_vline" => {
                        // (x, y, len, colour [, a])
                        h = w;
                        w = 1.0;
                        (r, g, b) = self.tu(n(3));
                        a = Self::num(args, 4, 1.0) as f32;
                    }
                    _ => {
                        (r, g, b) = self.tu(n(4));
                        a = Self::num(args, 5, 1.0) as f32;
                    }
                }
                let clip = self.px_clip;
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width as i32, gfx.height as i32);
                let (mut cx0, mut cy0, mut cx1, mut cy1) = (0, 0, bw, bh);
                if let Some(c) = clip {
                    cx0 = cx0.max(c[0]);
                    cy0 = cy0.max(c[1]);
                    cx1 = cx1.min(c[2]);
                    cy1 = cy1.min(c[3]);
                }
                let mut fill = |x0: i32, y0: i32, x1: i32, y1: i32| {
                    let (x0, y0, x1, y1) = (x0.max(cx0), y0.max(cy0), x1.min(cx1), y1.min(cy1));
                    if x0 >= x1 || y0 >= y1 {
                        return;
                    }
                    let col = (r.min(255) << 16) | (g.min(255) << 8) | b.min(255);
                    for yy in y0..y1 {
                        let row = (yy * bw) as usize;
                        for xx in x0..x1 {
                            let i = row + xx as usize;
                            gfx.buffer[i] = if a >= 0.999 { col } else { blend(gfx.buffer[i], r.min(255), g.min(255), b.min(255), a) };
                        }
                    }
                };
                let (x0, y0) = (x.round() as i32, y.round() as i32);
                let (x1, y1) = ((x + w).round() as i32, (y + h).round() as i32);
                let _ = bh;
                if name == "px_outline" {
                    fill(x0, y0, x1, y0 + 1);
                    fill(x0, y1 - 1, x1, y1);
                    fill(x0, y0 + 1, x0 + 1, y1 - 1);
                    fill(x1 - 1, y0 + 1, x1, y1 - 1);
                } else {
                    fill(x0, y0, x1.max(x0 + 1), y1.max(y0 + 1));
                }
                unit
            }
            "px_rrect" | "px_rrect_outline" => {
                // (x, y, w, h, radius, colour [, a]) — anti-aliased rounded rectangle
                let (x, y, w, h, rad) = (n(0), n(1), n(2), n(3), n(4));
                let (r, g, b) = self.tu(n(5));
                let a = Self::num(args, 6, 1.0) as f32;
                let outline = name == "px_rrect_outline";
                let clip = self.px_clip;
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width as i32, gfx.height as i32);
                let (mut cx0, mut cy0, mut cx1, mut cy1) = (0, 0, bw, bh);
                if let Some(c) = clip {
                    cx0 = cx0.max(c[0]);
                    cy0 = cy0.max(c[1]);
                    cx1 = cx1.min(c[2]);
                    cy1 = cy1.min(c[3]);
                }
                let (hx, hy) = (w * 0.5, h * 0.5);
                let rad = rad.min(hx).min(hy).max(0.0);
                let (ccx, ccy) = (x + hx, y + hy);
                let x0 = (x.floor() as i32).max(cx0);
                let x1 = ((x + w).ceil() as i32).min(cx1);
                let y0 = (y.floor() as i32).max(cy0);
                let y1 = ((y + h).ceil() as i32).min(cy1);
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        let px = (xx as f64 + 0.5) - ccx;
                        let py = (yy as f64 + 0.5) - ccy;
                        let qx = px.abs() - (hx - rad);
                        let qy = py.abs() - (hy - rad);
                        let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - rad;
                        let cov = if outline { (0.5 - (d + 0.5).abs()).clamp(0.0, 1.0) } else { (0.5 - d).clamp(0.0, 1.0) } as f32;
                        if cov <= 0.0 {
                            continue;
                        }
                        let i = (yy * bw + xx) as usize;
                        gfx.buffer[i] = blend(gfx.buffer[i], r, g, b, a * cov);
                    }
                }
                let _ = bh;
                unit
            }
            "px_disc" | "px_ring" => {
                // (cx, cy, radius, colour [, a])
                let (cx, cy, rad) = (n(0), n(1), n(2));
                let (r, g, b) = self.tu(n(3));
                let a = Self::num(args, 4, 1.0) as f32;
                let ring = name == "px_ring";
                let clip = self.px_clip;
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width as i32, gfx.height as i32);
                let (mut cx0, mut cy0, mut cx1, mut cy1) = (0, 0, bw, bh);
                if let Some(c) = clip {
                    cx0 = cx0.max(c[0]);
                    cy0 = cy0.max(c[1]);
                    cx1 = cx1.min(c[2]);
                    cy1 = cy1.min(c[3]);
                }
                let x0 = ((cx - rad - 1.0).floor() as i32).max(cx0);
                let x1 = ((cx + rad + 1.0).ceil() as i32).min(cx1);
                let y0 = ((cy - rad - 1.0).floor() as i32).max(cy0);
                let y1 = ((cy + rad + 1.0).ceil() as i32).min(cy1);
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        let d = (((xx as f64 + 0.5) - cx).powi(2) + ((yy as f64 + 0.5) - cy).powi(2)).sqrt();
                        let cov = if ring { (1.0 - (d - rad).abs()).clamp(0.0, 1.0) } else { (rad - d + 0.5).clamp(0.0, 1.0) } as f32;
                        if cov <= 0.0 {
                            continue;
                        }
                        let i = (yy * bw + xx) as usize;
                        gfx.buffer[i] = blend(gfx.buffer[i], r.min(255), g.min(255), b.min(255), a * cov);
                    }
                }
                unit
            }
            "px_line" => {
                // (x0, y0, x1, y1, colour [, a]) — 1px anti-aliased line
                let (mut x0, mut y0, mut x1, mut y1) = (n(0), n(1), n(2), n(3));
                let (r, g, b) = self.tu(n(4));
                let a = Self::num(args, 5, 1.0) as f32;
                let clip = self.px_clip;
                let mut gfx = self.gfx.borrow_mut();
                let (bw, bh) = (gfx.width as i32, gfx.height as i32);
                let (mut cx0, mut cy0, mut cx1, mut cy1) = (0, 0, bw, bh);
                if let Some(c) = clip {
                    cx0 = cx0.max(c[0]);
                    cy0 = cy0.max(c[1]);
                    cx1 = cx1.min(c[2]);
                    cy1 = cy1.min(c[3]);
                }
                if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
                    return unit;
                }
                let steep = (y1 - y0).abs() > (x1 - x0).abs();
                if steep {
                    std::mem::swap(&mut x0, &mut y0);
                    std::mem::swap(&mut x1, &mut y1);
                }
                if x0 > x1 {
                    std::mem::swap(&mut x0, &mut x1);
                    std::mem::swap(&mut y0, &mut y1);
                }
                let dx = x1 - x0;
                let grad = if dx.abs() < 1e-9 { 1.0 } else { (y1 - y0) / dx };
                let mut y = y0;
                let mut x = x0.round();
                y += grad * (x - x0);
                let mut xe = x1.round();
                // walk only the part that can touch the clip window (major axis after the swap)
                let (lo, hi) = if steep { (cy0 as f64 - 2.0, cy1 as f64 + 2.0) } else { (cx0 as f64 - 2.0, cx1 as f64 + 2.0) };
                if x < lo {
                    y += grad * (lo - x);
                    x = lo;
                }
                if xe > hi {
                    xe = hi;
                }
                let mut plot = |px: i32, py: i32, cov: f64| {
                    if px < cx0 || py < cy0 || px >= cx1 || py >= cy1 {
                        return;
                    }
                    let i = (py * bw + px) as usize;
                    gfx.buffer[i] = blend(gfx.buffer[i], r.min(255), g.min(255), b.min(255), a * cov as f32);
                };
                while x <= xe {
                    let yi = y.floor();
                    let f = y - yi;
                    if steep {
                        plot(yi as i32, x as i32, 1.0 - f);
                        plot(yi as i32 + 1, x as i32, f);
                    } else {
                        plot(x as i32, yi as i32, 1.0 - f);
                        plot(x as i32, yi as i32 + 1, f);
                    }
                    y += grad;
                    x += 1.0;
                }
                let _ = bh;
                unit
            }
            "px_fit" => {
                // (font, size, max_width, text) -> text truncated with ".." to fit
                let id = n(0) as i64;
                let px = n(1) as f32;
                let maxw = n(2) as f32;
                let text = args.get(3).map(|v| v.to_string()).unwrap_or_default();
                if id < 0 || id as usize >= self.fonts.len() {
                    return Some(Ok(Value::Str(text)));
                }
                let f = &mut self.fonts[id as usize];
                if f.measure(&text, px) <= maxw {
                    return Some(Ok(Value::Str(text)));
                }
                let chars: Vec<char> = text.chars().collect();
                let mut end = chars.len();
                while end > 0 {
                    let cand: String = chars[..end].iter().collect::<String>() + "..";
                    if f.measure(&cand, px) <= maxw {
                        return Some(Ok(Value::Str(cand)));
                    }
                    end -= 1;
                }
                Some(Ok(Value::Str(String::new())))
            }
            _ => None,
        }
    }
}
