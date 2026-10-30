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
thread_local! {
    /// px_snap_save / px_snap_load: (width, height, pixels)
    static SNAP: std::cell::RefCell<(usize, usize, Vec<u32>)> = std::cell::RefCell::new((0, 0, Vec::new()));
}

fn blend(dst: u32, r: u32, g: u32, b: u32, a: f32) -> u32 {
    let ia = (a.clamp(0.0, 1.0) * 256.0) as u32;
    blend_i(dst, r.min(255), g.min(255), b.min(255), ia)
}

/// integer blend, `ia` 0..256: the hot path of every fill
#[inline(always)]
fn blend_i(dst: u32, r: u32, g: u32, b: u32, ia: u32) -> u32 {
    let na = 256 - ia;
    let dr = (dst >> 16) & 255;
    let dg = (dst >> 8) & 255;
    let db = dst & 255;
    (((r * ia + dr * na) >> 8) << 16) | (((g * ia + dg * na) >> 8) << 8) | ((b * ia + db * na) >> 8)
}

/// fill a span of one row with a colour at a constant alpha
#[inline(always)]
fn fill_span(row: &mut [u32], r: u32, g: u32, b: u32, ia: u32) {
    if ia >= 256 {
        row.fill((r << 16) | (g << 8) | b);
    } else if ia > 0 {
        let (pr, pg, pb, na) = (r * ia, g * ia, b * ia, 256 - ia);
        for px in row.iter_mut() {
            let d = *px;
            *px = (((pr + ((d >> 16) & 255) * na) >> 8) << 16) | (((pg + ((d >> 8) & 255) * na) >> 8) << 8) | ((pb + (d & 255) * na) >> 8);
        }
    }
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
            // st_del(key) — remove a key from the store, so a long-lived program (the node, per connected peer)
            // does not grow the store without bound. Returns 1 if the key was present, 0 otherwise.
            "st_del" => {
                let had = self.gstore.remove(&key).is_some();
                ok(Value::Number(had as i32 as f64))
            }
            _ => None,
        }
    }

    fn px_builtin(&mut self, name: &str, args: &[Value]) -> Option<EvalResult> {
        let n = |i: usize| Self::num(args, i, 0.0);
        let unit = Some(Ok(Value::Unit));
        match name {
            // px_lines(ea, eb, px, py, pd, colour, alpha, esel, sel_colour): a whole mesh's edges in one call. Edges with
            // an end behind the near plane (pd <= 0.05) are skipped; edges flagged in esel get a doubled highlight line.
            "px_lines" => {
                let fl = |v: Option<&Value>| -> Vec<f64> {
                    match v {
                        Some(Value::List(l)) => l.iter().map(|x| if let Value::Number(n) = x { *n } else { 0.0 }).collect(),
                        _ => Vec::new(),
                    }
                };
                let (ea, eb, px, py, pd, esel) = (fl(args.first()), fl(args.get(1)), fl(args.get(2)), fl(args.get(3)), fl(args.get(4)), fl(args.get(7)));
                let (col, alpha, scol) = (n(5), n(6), n(8));
                let num = Value::Number;
                for i in 0..ea.len().min(eb.len()) {
                    let (a, b) = (ea[i] as usize, eb[i] as usize);
                    if a >= px.len() || b >= px.len() || pd[a] <= 0.05 || pd[b] <= 0.05 {
                        continue;
                    }
                    if i < esel.len() && esel[i] > 0.5 {
                        self.px_builtin("px_line", &[num(px[a]), num(py[a]), num(px[b]), num(py[b]), num(scol), num(1.0)]);
                        self.px_builtin("px_line", &[num(px[a] + 1.0), num(py[a]), num(px[b] + 1.0), num(py[b]), num(scol), num(0.6)]);
                    } else {
                        self.px_builtin("px_line", &[num(px[a]), num(py[a]), num(px[b]), num(py[b]), num(col), num(alpha)]);
                    }
                }
                unit
            }
            // px_dots(px, py, pd, vsel, active, colour, sel_colour, active_colour): vertex markers for a whole mesh
            "px_dots" => {
                let fl = |v: Option<&Value>| -> Vec<f64> {
                    match v {
                        Some(Value::List(l)) => l.iter().map(|x| if let Value::Number(n) = x { *n } else { 0.0 }).collect(),
                        _ => Vec::new(),
                    }
                };
                let (px, py, pd, vsel) = (fl(args.first()), fl(args.get(1)), fl(args.get(2)), fl(args.get(3)));
                let (act, col, scol, acol) = (n(4), n(5), n(6), n(7));
                let num = Value::Number;
                for i in 0..px.len().min(py.len()).min(pd.len()) {
                    if pd[i] <= 0.05 {
                        continue;
                    }
                    if i < vsel.len() && vsel[i] > 0.5 {
                        let c = if i as f64 == act { acol } else { scol };
                        self.px_builtin("px_rect", &[num(px[i] - 2.0), num(py[i] - 2.0), num(5.0), num(5.0), num(c)]);
                    } else {
                        self.px_builtin("px_rect", &[num(px[i] - 1.0), num(py[i] - 1.0), num(3.0), num(3.0), num(col)]);
                    }
                }
                unit
            }
            "px_text" => {
                // px_text(font, x, y, size, colour, text [, crisp]) — anti-aliased, honours px_clip; crisp > 0 sharpens the edge coverage
                // (soft grey fringes pushed to clear or solid) so small labels stay legible
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
                let crisp = Self::num(args, 6, 0.0) > 0.0;
                if inside && !crisp {
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
                    let (cr, cg, cb) = ((color >> 16) & 255, (color >> 8) & 255, color & 255);
                    for yy in iy0..iy1 {
                        for xx in ix0..ix1 {
                            let i = yy as usize * w + xx as usize;
                            let nv = scratch[(yy - by0) as usize * tw + (xx - bx0) as usize];
                            if !crisp {
                                gfx.buffer[i] = nv;
                                continue;
                            }
                            // coverage from how far the pixel moved towards the text colour, then a contrast curve on it
                            let o = gfx.buffer[i];
                            let ch = |v: u32, sh: u32| ((v >> sh) & 255) as i32;
                            let mut cov = 0.0f32;
                            for (sh, c) in [(16u32, cr), (8, cg), (0, cb)] {
                                let den = c as i32 - ch(o, sh);
                                if den.abs() >= 24 {
                                    cov = cov.max(((ch(nv, sh) - ch(o, sh)) as f32 / den as f32).clamp(0.0, 1.0));
                                }
                            }
                            let c2 = ((cov - 0.25) / 0.5).clamp(0.0, 1.0);
                            gfx.buffer[i] = blend_i(o, cr, cg, cb, (c2 * 256.0) as u32);
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
                let ia = if a >= 0.999 { 256 } else { (a.clamp(0.0, 1.0) * 256.0) as u32 };
                let (r, g, b) = (r.min(255), g.min(255), b.min(255));
                let mut fill = |x0: i32, y0: i32, x1: i32, y1: i32| {
                    let (x0, y0, x1, y1) = (x0.max(cx0), y0.max(cy0), x1.min(cx1), y1.min(cy1));
                    if x0 >= x1 || y0 >= y1 {
                        return;
                    }
                    for yy in y0..y1 {
                        let row = (yy * bw) as usize;
                        fill_span(&mut gfx.buffer[row + x0 as usize..row + x1 as usize], r, g, b, ia);
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
                let (r, g, b) = (r.min(255), g.min(255), b.min(255));
                let ia_full = (a.clamp(0.0, 1.0) * 256.0) as u32;
                // the distance field only near the edges (corners, borders); the interior of a row is one constant span
                let band = (rad.ceil() as i32) + 2;
                for yy in y0..y1 {
                    let py = (yy as f64 + 0.5) - ccy;
                    let edge_row = py.abs() > hy - rad - 1.5;
                    let (mut ix0, mut ix1) = (x1, x1);
                    if !edge_row {
                        ix0 = ((x.floor() as i32) + band).max(x0);
                        ix1 = (((x + w).ceil() as i32) - band).min(x1);
                        if ix1 <= ix0 {
                            ix0 = x1;
                            ix1 = x1;
                        }
                    }
                    let row = (yy * bw) as usize;
                    for xx in x0..x1 {
                        if xx == ix0 && ix1 > ix0 {
                            if !outline {
                                fill_span(&mut gfx.buffer[row + ix0 as usize..row + ix1 as usize], r, g, b, ia_full);
                            }
                        }
                        if xx >= ix0 && xx < ix1 {
                            continue;
                        }
                        let px = (xx as f64 + 0.5) - ccx;
                        let qx = px.abs() - (hx - rad);
                        let qy = py.abs() - (hy - rad);
                        let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - rad;
                        let cov = if outline { (0.5 - (d + 0.5).abs()).clamp(0.0, 1.0) } else { (0.5 - d).clamp(0.0, 1.0) } as f32;
                        if cov <= 0.0 {
                            continue;
                        }
                        let i = row + xx as usize;
                        gfx.buffer[i] = blend(gfx.buffer[i], r, g, b, a * cov);
                    }
                }
                let _ = bh;
                unit
            }
            "px_snap_save" => {
                // keep a copy of the whole framebuffer (the window behind a dialog)
                let g = self.gfx.borrow();
                SNAP.with(|sn| {
                    let mut sn = sn.borrow_mut();
                    sn.0 = g.width;
                    sn.1 = g.height;
                    sn.2.clear();
                    sn.2.extend_from_slice(&g.buffer);
                });
                unit
            }
            "px_snap_load" => {
                // put the kept copy back: 1 when it fitted the window, 0 otherwise (draw normally then)
                let mut g = self.gfx.borrow_mut();
                let ok = SNAP.with(|sn| {
                    let sn = sn.borrow();
                    if sn.0 == g.width && sn.1 == g.height && sn.2.len() == g.buffer.len() {
                        g.buffer.copy_from_slice(&sn.2);
                        true
                    } else {
                        false
                    }
                });
                return Some(Ok(Value::Number(ok as i32 as f64)));
            }
            "px_hatch" => {
                // (x, y, w, h, colour, angle degrees, spacing, a): parallel 1 px anti-aliased lines filling the box (the design
                // system's hatching), from each pixel's distance to the nearest line - no line drawing, no sqrt
                let (x, y, w, h) = (n(0), n(1), n(2), n(3));
                let (r, g, b) = self.tu(n(4));
                let (ang, sp, a) = (n(5), n(6).max(1.0), Self::num(args, 7, 1.0).clamp(0.0, 1.0));
                if !(w >= 1.0 && h >= 1.0) || a <= 0.0 {
                    return unit;
                }
                let rr = ang.to_radians();
                let (nx, ny) = (rr.sin(), rr.cos());
                let corners = [x * nx + y * ny, (x + w) * nx + y * ny, x * nx + (y + h) * ny, (x + w) * nx + (y + h) * ny];
                let lo = corners.iter().copied().fold(f64::MAX, f64::min);
                let off = lo + sp * 0.5;
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
                let x0 = (x.floor() as i32).max(cx0);
                let x1 = ((x + w).ceil() as i32).min(cx1);
                let y0 = (y.floor() as i32).max(cy0);
                let y1 = ((y + h).ceil() as i32).min(cy1);
                let (r, g, b) = (r.min(255), g.min(255), b.min(255));
                let inv = 1.0 / sp;
                for yy in y0..y1 {
                    let row = (yy * bw) as usize;
                    let pyn = (yy as f64 + 0.5) * ny;
                    for xx in x0..x1 {
                        let u = ((xx as f64 + 0.5) * nx + pyn - off) * inv;
                        let d = (u - u.round()).abs() * sp;
                        if d >= 1.0 {
                            continue;
                        }
                        let ia = (((1.0 - d) * a) * 256.0) as u32;
                        let i = row + xx as usize;
                        gfx.buffer[i] = blend_i(gfx.buffer[i], r, g, b, ia);
                    }
                }
                unit
            }
            "px_bars" => {
                // (x0, mid, step, scale, peaks list, colour [, a]): a waveform as one bar per peak, mirrored around `mid`
                let (x0, mid, step, scale) = (n(0), n(1), n(2), n(3));
                let (r, g, b) = self.tu(n(5));
                let ia = (Self::num(args, 6, 1.0).clamp(0.0, 1.0) * 256.0) as u32;
                let Some(Value::List(pk)) = args.get(4) else { return unit };
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
                let (r, g, b) = (r.min(255), g.min(255), b.min(255));
                for (i, v) in pk.iter().enumerate() {
                    let Value::Number(p) = v else { continue };
                    let hgt = p * scale;
                    if !(hgt > 0.0) {
                        continue;
                    }
                    let bx = x0 + i as f64 * step;
                    let (xa, xb) = ((bx.round() as i32).max(cx0), ((bx + step.max(1.0)).round() as i32).max(bx.round() as i32 + 1).min(cx1));
                    let (ya, yb) = (((mid - hgt).round() as i32).max(cy0), ((mid + hgt.max(0.5)).round() as i32).max((mid - hgt).round() as i32 + 1).min(cy1));
                    if xa >= xb || ya >= yb {
                        continue;
                    }
                    for yy in ya..yb {
                        let row = (yy * bw) as usize;
                        fill_span(&mut gfx.buffer[row + xa as usize..row + xb as usize], r, g, b, ia);
                    }
                }
                unit
            }
            "px_notes" => {
                // (notes [start, len, key, vel, ...] stride 7, count, x, y, w, h, beat width, lowest key, key span, loop beats (0 = none),
                //  visible x0, visible x1, colour a, colour b): the piano-roll thumbnail of a clip, 2 px per note, alpha from velocity
                let Some(Value::List(nl)) = args.first() else { return unit };
                let (cnt, x, y, w, h, bwd, lo, span, ll, vx0, vx1) = (n(1) as usize, n(2), n(3), n(4), n(5), n(6), n(7), n(8).max(1.0), n(9), n(10), n(11));
                let (ca, cb) = (self.tu(n(12)), self.tu(n(13)));
                let num = |i: usize| match nl.get(i) {
                    Some(Value::Number(v)) => *v,
                    _ => 0.0,
                };
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
                let cnt = cnt.min(nl.len() / 7);
                let (reps, rp0) = if ll > 0.5 { (((vx1 - x) / bwd / ll).ceil().max(0.0) as usize, ((vx0 - x) / bwd / ll).floor().max(0.0) as usize) } else { (1, 0) };
                for rp in rp0..reps.min(rp0 + 4096) {
                    for i in 0..cnt {
                        let ns = num(i * 7);
                        let nx = x + (ns + rp as f64 * ll) * bwd;
                        if nx >= vx1 || (ll > 0.5 && ns >= ll) {
                            continue;
                        }
                        let nw = (num(i * 7 + 1) * bwd).max(1.5).min(x + w - nx);
                        if nx + nw <= vx0 {
                            continue;
                        }
                        let ny = y + (1.0 - (num(i * 7 + 2) - lo) / span) * (h - 3.0);
                        let al = ((num(i * 7 + 3) * 200.0 + 30.0).clamp(40.0, 230.0) / 255.0 * 256.0) as u32;
                        let (r, g, b) = if i % 2 == 0 { ca } else { cb };
                        let (xa, xb) = ((nx.round() as i32).max(cx0), ((nx + nw).round() as i32).max(nx.round() as i32 + 1).min(cx1));
                        let (ya, yb) = ((ny.round() as i32).max(cy0), ((ny + 2.0).round() as i32).min(cy1));
                        if xa >= xb || ya >= yb {
                            continue;
                        }
                        for yy in ya..yb {
                            let row = (yy * bw) as usize;
                            fill_span(&mut gfx.buffer[row + xa as usize..row + xb as usize], r.min(255), g.min(255), b.min(255), al);
                        }
                    }
                }
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
                let fid = id as usize;
                if self.font_measure_fb(fid, &text, px) <= maxw {
                    return Some(Ok(Value::Str(text)));
                }
                let chars: Vec<char> = text.chars().collect();
                let mut end = chars.len();
                while end > 0 {
                    let cand: String = chars[..end].iter().collect::<String>() + "..";
                    if self.font_measure_fb(fid, &cand, px) <= maxw {
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
