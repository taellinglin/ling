//! Image decoding for the kernel. PNG (8-bit truecolor RGB/RGBA and grayscale,
//! non-interlaced) via miniz_oxide inflate + in-tree chunk parse and scanline
//! unfiltering; plus a small subset-SVG rasterizer (basic shapes). Output is a
//! row-major buffer of 0x00RRGGBB pixels the framebuffer can blit.

use crate::drivers::framebuffer;
use alloc::vec;
use alloc::vec::Vec;

pub struct Image {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u32>, // row-major 0x00RRGGBB
}

fn be32(b: &[u8], i: usize) -> u32 {
    ((b[i] as u32) << 24) | ((b[i + 1] as u32) << 16) | ((b[i + 2] as u32) << 8) | b[i + 3] as u32
}

fn paeth(a: i32, b: i32, c: i32) -> i32 {
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Decode a PNG (8-bit, color type 0/2/4/6, non-interlaced).
pub fn decode_png(data: &[u8]) -> Option<Image> {
    if data.len() < 8 || data[..8] != [137, 80, 78, 71, 13, 10, 26, 10] {
        return None;
    }
    let mut pos = 8;
    let (mut w, mut h, mut depth, mut color) = (0u32, 0u32, 0u8, 0u8);
    let mut idat: Vec<u8> = Vec::new();
    while pos + 8 <= data.len() {
        let len = be32(data, pos) as usize;
        let ctype = &data[pos + 4..pos + 8];
        let cstart = pos + 8;
        if cstart + len + 4 > data.len() {
            break;
        }
        let cdata = &data[cstart..cstart + len];
        match ctype {
            b"IHDR" => {
                w = be32(cdata, 0);
                h = be32(cdata, 4);
                depth = cdata[8];
                color = cdata[9];
                if cdata[12] != 0 {
                    return None; // interlaced not supported
                }
            },
            b"IDAT" => idat.extend_from_slice(cdata),
            b"IEND" => break,
            _ => {},
        }
        pos = cstart + len + 4;
    }
    if w == 0 || h == 0 || depth != 8 {
        return None;
    }
    let bpp: usize = match color {
        0 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => return None,
    };
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&idat).ok()?;
    let stride = w as usize * bpp;
    if raw.len() < (stride + 1) * h as usize {
        return None;
    }
    let mut out: Vec<u32> = Vec::with_capacity((w * h) as usize);
    let mut prev = vec![0u8; stride];
    let mut cur = vec![0u8; stride];
    let mut ri = 0usize;
    for _y in 0..h as usize {
        let filter = raw[ri];
        ri += 1;
        for x in 0..stride {
            let rb = raw[ri + x] as i32;
            let a = if x >= bpp { cur[x - bpp] as i32 } else { 0 };
            let b = prev[x] as i32;
            let c = if x >= bpp { prev[x - bpp] as i32 } else { 0 };
            let v = match filter {
                1 => rb + a,
                2 => rb + b,
                3 => rb + (a + b) / 2,
                4 => rb + paeth(a, b, c),
                _ => rb,
            } & 0xff;
            cur[x] = v as u8;
        }
        ri += stride;
        for x in 0..w as usize {
            let o = x * bpp;
            let (r, g, b) = match color {
                0 | 4 => (cur[o], cur[o], cur[o]),
                _ => (cur[o], cur[o + 1], cur[o + 2]),
            };
            out.push(((r as u32) << 16) | ((g as u32) << 8) | b as u32);
        }
        core::mem::swap(&mut prev, &mut cur);
    }
    Some(Image { w, h, px: out })
}

// ── Subset SVG rasterizer ───────────────────────────────────────────────────
// Honest scope: solid fills/strokes of rect/circle/ellipse/line/polygon/
// polyline/path (M/L/H/V/C/Q/Z, curves sampled to segments); hex/rgb()/named
// colors; canvas from viewBox or width/height. NO gradients, filters, text,
// transforms, or CSS -- so a complex illustration (e.g. ling_country.svg)
// won't render faithfully, but icons/logos/diagrams do.

pub fn looks_svg(b: &[u8]) -> bool {
    b.windows(4).take(512).any(|w| w == b"<svg")
}

fn svg_num(s: &str) -> f64 {
    // Parse a leading float (optional sign, digits, '.', exponent).
    let b = s.as_bytes();
    let mut i = 0;
    let mut neg = false;
    if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
        neg = b[i] == b'-';
        i += 1;
    }
    let mut intp = 0f64;
    while i < b.len() && b[i].is_ascii_digit() {
        intp = intp * 10.0 + (b[i] - b'0') as f64;
        i += 1;
    }
    let mut frac = 0f64;
    let mut scale = 1f64;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            frac = frac * 10.0 + (b[i] - b'0') as f64;
            scale *= 10.0;
            i += 1;
        }
    }
    let v = intp + frac / scale;
    if neg {
        -v
    } else {
        v
    }
}

/// Value of attribute `name` within tag text `tag` (name="value").
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let bytes = tag.as_bytes();
    let key = name.as_bytes();
    let mut i = 0;
    while i + key.len() + 1 < bytes.len() {
        if &bytes[i..i + key.len()] == key {
            let mut j = i + key.len();
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'=' {
                j += 1;
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'"' || bytes[j] == b'\'') {
                    j += 1;
                }
                let start = j;
                while j < bytes.len() && bytes[j] != b'"' && bytes[j] != b'\'' {
                    j += 1;
                }
                return core::str::from_utf8(&bytes[start..j]).ok();
            }
        }
        i += 1;
    }
    None
}

fn hex1(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as u32),
        b'a'..=b'f' => Some((c - b'a' + 10) as u32),
        b'A'..=b'F' => Some((c - b'A' + 10) as u32),
        _ => None,
    }
}

/// Parse an SVG color; None for "none"/unparseable (shape skipped).
fn parse_color(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.is_empty() || s == "none" {
        return None;
    }
    if let Some(h) = s.strip_prefix('#') {
        let hb = h.as_bytes();
        if hb.len() >= 6 {
            let mut v = 0u32;
            for k in 0..6 {
                v = (v << 4) | hex1(hb[k])?;
            }
            return Some(v);
        }
        if hb.len() >= 3 {
            let (r, g, b) = (hex1(hb[0])?, hex1(hb[1])?, hex1(hb[2])?);
            return Some((r * 17) << 16 | (g * 17) << 8 | (b * 17));
        }
        return None;
    }
    if let Some(inner) = s.strip_prefix("rgb(") {
        let inner = inner.trim_end_matches(')');
        let mut it = inner.split(|c| c == ',' || c == ' ').filter(|x| !x.is_empty());
        let r = svg_num(it.next()?) as u32 & 0xff;
        let g = svg_num(it.next()?) as u32 & 0xff;
        let b = svg_num(it.next()?) as u32 & 0xff;
        return Some((r << 16) | (g << 8) | b);
    }
    Some(match s {
        "black" => 0x000000,
        "white" => 0xffffff,
        "red" => 0xff0000,
        "green" => 0x008000,
        "lime" => 0x00ff00,
        "blue" => 0x0000ff,
        "yellow" => 0xffff00,
        "cyan" | "aqua" => 0x00ffff,
        "magenta" | "fuchsia" => 0xff00ff,
        "gray" | "grey" => 0x808080,
        "silver" => 0xc0c0c0,
        "orange" => 0xffa500,
        "purple" => 0x800080,
        "navy" => 0x000080,
        "teal" => 0x008080,
        "maroon" => 0x800000,
        "olive" => 0x808000,
        _ => return None,
    })
}

/// Scanline even-odd fill of a polygon into the pixel buffer.
fn fill_poly(px: &mut [u32], w: u32, h: u32, verts: &[(f64, f64)], color: u32) {
    if verts.len() < 3 {
        return;
    }
    let mut ys = h as i32;
    let mut ye = 0i32;
    for &(_, y) in verts {
        ys = ys.min(y as i32);
        ye = ye.max(y as i32 + 1);
    }
    ys = ys.max(0);
    ye = ye.min(h as i32);
    let mut xs = [0f64; 64];
    let mut y = ys;
    while y < ye {
        let yc = y as f64 + 0.5;
        let mut n = 0;
        let mut j = verts.len() - 1;
        for i in 0..verts.len() {
            let (xi, yi) = verts[i];
            let (xj, yj) = verts[j];
            if (yi <= yc && yj > yc) || (yj <= yc && yi > yc) {
                let t = (yc - yi) / (yj - yi);
                if n < xs.len() {
                    xs[n] = xi + t * (xj - xi);
                    n += 1;
                }
            }
            j = i;
        }
        // sort intersections
        for a in 0..n {
            for b in a + 1..n {
                if xs[b] < xs[a] {
                    xs.swap(a, b);
                }
            }
        }
        let mut k = 0;
        while k + 1 < n {
            let x0 = xs[k].max(0.0) as u32;
            let x1 = (xs[k + 1] as i32).min(w as i32) as u32;
            let mut x = x0;
            while x < x1 {
                px[(y as u32 * w + x) as usize] = color;
                x += 1;
            }
            k += 2;
        }
        y += 1;
    }
}

/// Parse a path `d` string into filled polygons (subpaths), sampling curves.
// ── Affine transforms (SVG transform= + viewBox mapping) ────────────────────
// A 2x3 affine matrix [a,b,c,d,e,f]: x' = a*x + c*y + e, y' = b*x + d*y + f.
type Mat = [f64; 6];
const MAT_ID: Mat = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// Compose `m` after `n` (apply n first, then m): result = m * n.
fn mat_mul(m: Mat, n: Mat) -> Mat {
    [
        m[0] * n[0] + m[2] * n[1],
        m[1] * n[0] + m[3] * n[1],
        m[0] * n[2] + m[2] * n[3],
        m[1] * n[2] + m[3] * n[3],
        m[0] * n[4] + m[2] * n[5] + m[4],
        m[1] * n[4] + m[3] * n[5] + m[5],
    ]
}

fn mat_apply(m: &Mat, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// Parse an SVG `transform` attribute (translate/scale/rotate/matrix, possibly
/// several, applied left to right) into a single matrix.
fn parse_transform(s: &str) -> Mat {
    let mut m = MAT_ID;
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        // function name
        while i < b.len() && !(b[i] as char).is_ascii_alphabetic() {
            i += 1;
        }
        let ns = i;
        while i < b.len() && (b[i] as char).is_ascii_alphabetic() {
            i += 1;
        }
        if i >= b.len() || ns == i {
            break;
        }
        let name = &s[ns..i];
        // args in ( ... )
        while i < b.len() && b[i] != b'(' {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        i += 1;
        let as_ = i;
        while i < b.len() && b[i] != b')' {
            i += 1;
        }
        let args = &s[as_..i.min(b.len())];
        if i < b.len() {
            i += 1;
        }
        let mut nums = [0f64; 6];
        let mut n = 0;
        for tok in args.split(|c| c == ' ' || c == ',' || c == '\t' || c == '\n').filter(|x| !x.is_empty()) {
            if n < 6 {
                nums[n] = svg_num(tok);
                n += 1;
            }
        }
        let t = match name {
            "translate" => [1.0, 0.0, 0.0, 1.0, nums[0], if n > 1 { nums[1] } else { 0.0 }],
            "scale" => [nums[0], 0.0, 0.0, if n > 1 { nums[1] } else { nums[0] }, 0.0, 0.0],
            "rotate" => {
                let a = nums[0] * 0.017453292519943295;
                let (c, s) = (cos(a), sin(a));
                if n >= 3 {
                    // rotate(a, cx, cy) = translate(cx,cy) rot translate(-cx,-cy)
                    let (cx, cy) = (nums[1], nums[2]);
                    let rot = [c, s, -s, c, 0.0, 0.0];
                    let t1 = [1.0, 0.0, 0.0, 1.0, cx, cy];
                    let t2 = [1.0, 0.0, 0.0, 1.0, -cx, -cy];
                    mat_mul(mat_mul(t1, rot), t2)
                } else {
                    [c, s, -s, c, 0.0, 0.0]
                }
            },
            "matrix" => [nums[0], nums[1], nums[2], nums[3], nums[4], nums[5]],
            _ => MAT_ID,
        };
        m = mat_mul(m, t);
    }
    m
}

/// Find the element with `id="wanted"` and return its full text span (open tag
/// through matching close, or the self-closing tag). For `<use>` resolution.
fn find_element_by_id<'a>(text: &'a str, wanted: &str) -> Option<&'a str> {
    let b = text.as_bytes();
    // Look for id="wanted" or id='wanted'.
    let mut search = 0usize;
    let idpos = loop {
        let rest = text.get(search..)?;
        let p = rest.find("id=")? + search;
        let after = &b[p + 3..];
        let q = *after.first()?;
        let val = if q == b'"' || q == b'\'' {
            let end = after[1..].iter().position(|&c| c == q)? + 1;
            core::str::from_utf8(&after[1..end]).ok()?
        } else {
            ""
        };
        if val == wanted {
            break p;
        }
        search = p + 3;
    };
    // Back up to the enclosing '<'.
    let mut lt = idpos;
    while lt > 0 && b[lt] != b'<' {
        lt -= 1;
    }
    if b[lt] != b'<' {
        return None;
    }
    // Tag name.
    let name_start = lt + 1;
    let mut ne = name_start;
    while ne < b.len() && !(b[ne] == b' ' || b[ne] == b'\t' || b[ne] == b'\n' || b[ne] == b'>' || b[ne] == b'/') {
        ne += 1;
    }
    let name = &text[name_start..ne];
    // End of the open tag.
    let gt = text[lt..].find('>')? + lt;
    if gt > 0 && b[gt - 1] == b'/' {
        return Some(&text[lt..=gt]); // self-closing
    }
    // Find matching close, honoring nesting of same-name tags.
    let mut depth = 1i32;
    let mut j = gt + 1;
    let mut close_buf = [0u8; 32];
    let cl = name.len().min(30);
    close_buf[0] = b'<';
    close_buf[1] = b'/';
    close_buf[2..2 + cl].copy_from_slice(&name.as_bytes()[..cl]);
    let open0 = name.as_bytes()[0];
    while j < b.len() && depth > 0 {
        if b[j] == b'<' {
            if b.get(j + 1) == Some(&b'/') && text[j..].as_bytes().get(2..2 + cl) == Some(&close_buf[2..2 + cl]) {
                depth -= 1;
                if depth == 0 {
                    let end = text[j..].find('>')? + j;
                    return Some(&text[lt..=end]);
                }
            } else if b.get(j + 1) == Some(&open0) && text[j + 1..].starts_with(name) {
                depth += 1;
            }
        }
        j += 1;
    }
    None
}

fn path_polys(d: &str, out: &mut Vec<Vec<(f64, f64)>>) {
    let b = d.as_bytes();
    let mut i = 0;
    let (mut cx, mut cy) = (0f64, 0f64);
    let (mut sx, mut sy) = (0f64, 0f64);
    let mut cur: Vec<(f64, f64)> = Vec::new();
    let mut nums = [0f64; 8];
    let read_nums = |b: &[u8], i: &mut usize, nums: &mut [f64], count: usize| -> bool {
        let mut got = 0;
        while got < count {
            while *i < b.len() && (b[*i] == b' ' || b[*i] == b',' || b[*i] == b'\n' || b[*i] == b'\t' || b[*i] == b'\r') {
                *i += 1;
            }
            let start = *i;
            if *i < b.len() && (b[*i] == b'-' || b[*i] == b'+') {
                *i += 1;
            }
            while *i < b.len() && (b[*i].is_ascii_digit() || b[*i] == b'.') {
                *i += 1;
            }
            if *i == start {
                return false;
            }
            nums[got] = svg_num(core::str::from_utf8(&b[start..*i]).unwrap_or("0"));
            got += 1;
        }
        true
    };
    while i < b.len() {
        let c = b[i];
        if c == b' ' || c == b',' || c == b'\n' || c == b'\t' || c == b'\r' {
            i += 1;
            continue;
        }
        let rel = c.is_ascii_lowercase();
        let cmd = c.to_ascii_uppercase();
        i += 1;
        match cmd {
            b'M' => {
                if !read_nums(b, &mut i, &mut nums, 2) {
                    break;
                }
                if cur.len() >= 3 {
                    out.push(core::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
                cx = if rel { cx + nums[0] } else { nums[0] };
                cy = if rel { cy + nums[1] } else { nums[1] };
                sx = cx;
                sy = cy;
                cur.push((cx, cy));
            },
            b'L' => {
                if !read_nums(b, &mut i, &mut nums, 2) {
                    break;
                }
                cx = if rel { cx + nums[0] } else { nums[0] };
                cy = if rel { cy + nums[1] } else { nums[1] };
                cur.push((cx, cy));
            },
            b'H' => {
                if !read_nums(b, &mut i, &mut nums, 1) {
                    break;
                }
                cx = if rel { cx + nums[0] } else { nums[0] };
                cur.push((cx, cy));
            },
            b'V' => {
                if !read_nums(b, &mut i, &mut nums, 1) {
                    break;
                }
                cy = if rel { cy + nums[0] } else { nums[0] };
                cur.push((cx, cy));
            },
            b'C' => {
                if !read_nums(b, &mut i, &mut nums, 6) {
                    break;
                }
                let (x1, y1) = (if rel { cx + nums[0] } else { nums[0] }, if rel { cy + nums[1] } else { nums[1] });
                let (x2, y2) = (if rel { cx + nums[2] } else { nums[2] }, if rel { cy + nums[3] } else { nums[3] });
                let (ex, ey) = (if rel { cx + nums[4] } else { nums[4] }, if rel { cy + nums[5] } else { nums[5] });
                let mut t = 1;
                while t <= 8 {
                    let u = t as f64 / 8.0;
                    let mv = 1.0 - u;
                    let x = mv * mv * mv * cx + 3.0 * mv * mv * u * x1 + 3.0 * mv * u * u * x2 + u * u * u * ex;
                    let y = mv * mv * mv * cy + 3.0 * mv * mv * u * y1 + 3.0 * mv * u * u * y2 + u * u * u * ey;
                    cur.push((x, y));
                    t += 1;
                }
                cx = ex;
                cy = ey;
            },
            b'Q' => {
                if !read_nums(b, &mut i, &mut nums, 4) {
                    break;
                }
                let (x1, y1) = (if rel { cx + nums[0] } else { nums[0] }, if rel { cy + nums[1] } else { nums[1] });
                let (ex, ey) = (if rel { cx + nums[2] } else { nums[2] }, if rel { cy + nums[3] } else { nums[3] });
                let mut t = 1;
                while t <= 8 {
                    let u = t as f64 / 8.0;
                    let mv = 1.0 - u;
                    let x = mv * mv * cx + 2.0 * mv * u * x1 + u * u * ex;
                    let y = mv * mv * cy + 2.0 * mv * u * y1 + u * u * ey;
                    cur.push((x, y));
                    t += 1;
                }
                cx = ex;
                cy = ey;
            },
            b'Z' => {
                cx = sx;
                cy = sy;
                if cur.len() >= 3 {
                    out.push(core::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
            },
            _ => {
                // Unknown command (S/T/A, etc.) -- bail out of this path.
                break;
            },
        }
    }
    if cur.len() >= 3 {
        out.push(cur);
    }
}

/// Decode a subset of SVG to a raster Image (white ground).
pub fn decode_svg(data: &[u8]) -> Option<Image> {
    decode_svg_capped(data, 1024)
}

/// Decode a subset SVG, rasterizing at a canvas capped to `cap` px on its
/// longest side. Inline browser thumbnails pass a small cap so a complex
/// coat-of-arms doesn't rasterize at full resolution (scanline fill is
/// O(canvas x paths) -- a 1024px canvas can take seconds).
pub fn decode_svg_capped(data: &[u8], cap: u32) -> Option<Image> {
    let text = core::str::from_utf8(data).ok()?;
    let svg_pos = text.find("<svg")?;
    let svg_tag_end = text[svg_pos..].find('>')? + svg_pos;
    let svg_tag = &text[svg_pos..svg_tag_end];
    // The user-space rect: viewBox if present, else width/height at origin.
    let (mut vx, mut vy, mut vw, mut vh) = (0f64, 0f64, 0f64, 0f64);
    if let Some(vb) = attr(svg_tag, "viewBox") {
        let mut it = vb.split(|c| c == ' ' || c == ',').filter(|x| !x.is_empty());
        if let (Some(a), Some(b), Some(c), Some(d)) = (it.next(), it.next(), it.next(), it.next()) {
            vx = svg_num(a);
            vy = svg_num(b);
            vw = svg_num(c);
            vh = svg_num(d);
        }
    }
    if vw <= 0.0 || vh <= 0.0 {
        if let (Some(ws), Some(hs)) = (attr(svg_tag, "width"), attr(svg_tag, "height")) {
            vw = svg_num(ws);
            vh = svg_num(hs);
            vx = 0.0;
            vy = 0.0;
        }
    }
    if vw <= 0.0 || vh <= 0.0 {
        return None;
    }
    // Output size follows the viewBox aspect, capped on the long side.
    let (mut w, mut h) = (vw as u32, vh as u32);
    if w == 0 || h == 0 {
        return None;
    }
    if w > cap || h > cap {
        let s = if w >= h { cap as f64 / w as f64 } else { cap as f64 / h as f64 };
        w = ((w as f64 * s) as u32).max(1);
        h = ((h as f64 * s) as u32).max(1);
    }
    // Anti-aliasing: rasterize at SSx and box-downsample to (w,h). The whole
    // document renders through one affine transform (viewBox -> canvas), so
    // shapes, groups, and <use>d symbols land in the right place at the right
    // size -- and coordinates are baked into canvas space once, up front,
    // rather than re-scaled per shape.
    const SS: u32 = 2;
    let (cw, ch) = (w * SS, h * SS);
    let mut buf = vec![0xffffffu32; (cw * ch) as usize];
    let sx = cw as f64 / vw;
    let sy = ch as f64 / vh;
    let root: Mat = [sx, 0.0, 0.0, sy, -vx * sx, -vy * sy];

    // Wall-clock budget: a complex real-world SVG can accumulate enough
    // rasterization work to stall the UI. Bail out with a partial render
    // rather than hang. x86_64 only (the SVG path isn't exercised on rpi).
    #[cfg(target_arch = "x86_64")]
    let deadline = crate::arch::timer::now_us() + 1_500_000;
    #[cfg(not(target_arch = "x86_64"))]
    let deadline = 0u64;

    let content = &text[svg_tag_end + 1..];
    svg_render(content, text, root, &mut buf, cw, ch, 0, deadline);

    // Box-downsample SSxSS -> (w,h) for anti-aliased edges.
    let mut px = vec![0u32; (w * h) as usize];
    let n = SS * SS;
    for oy in 0..h {
        for ox in 0..w {
            let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
            for dy in 0..SS {
                for dx in 0..SS {
                    let c = buf[(((oy * SS + dy) * cw) + (ox * SS + dx)) as usize];
                    r += (c >> 16) & 0xff;
                    g += (c >> 8) & 0xff;
                    b += c & 0xff;
                }
            }
            px[(oy * w + ox) as usize] = ((r / n) << 16) | ((g / n) << 8) | (b / n);
        }
    }
    Some(Image { w, h, px })
}

/// Resolve a shape's `fill` attribute to a color, following `url(#grad)`
/// gradient references (approximated as the average of their stops).
fn shape_fill(full: &str, fill_attr: Option<&str>) -> Option<u32> {
    let f = fill_attr?.trim();
    if let Some(rest) = f.strip_prefix("url(") {
        let id = rest.trim_start_matches(|c| c == '#' || c == ' ');
        let id = id.split(|c| c == ')' || c == ' ').next()?.trim_start_matches('#');
        return gradient_color(full, id);
    }
    parse_color(f)
}

/// Average the stop colors of the gradient with `id` (a linear/radial gradient
/// rendered as a representative flat color -- we don't do real gradients).
fn gradient_color(full: &str, id: &str) -> Option<u32> {
    let el = find_element_by_id(full, id)?;
    let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
    let mut rest = el;
    while let Some(p) = rest.find("stop-color") {
        let after = &rest[p + "stop-color".len()..];
        // Value follows a ':' (style) or '="' (attribute).
        let mut i = 0;
        let ab = after.as_bytes();
        while i < ab.len() && (ab[i] == b':' || ab[i] == b'=' || ab[i] == b'"' || ab[i] == b'\'' || ab[i] == b' ') {
            i += 1;
        }
        let vs = i;
        while i < ab.len() && !(ab[i] == b'"' || ab[i] == b'\'' || ab[i] == b';' || ab[i] == b' ' || ab[i] == b'>') {
            i += 1;
        }
        if let Some(c) = parse_color(&after[vs..i]) {
            r += (c >> 16) & 0xff;
            g += (c >> 8) & 0xff;
            b += c & 0xff;
            n += 1;
        }
        rest = &after[i..];
    }
    if n > 0 {
        Some(((r / n) << 16) | ((g / n) << 8) | (b / n))
    } else {
        None
    }
}

/// Recursively rasterize an SVG fragment `text` (part of the whole `full`
/// document, used for id lookups) under transform `base`, into `buf` (w x h).
/// Handles nested `<g transform>` via a matrix stack, skips `<defs>`/`<symbol>`
/// content, expands `<use>` by rendering the referenced element, and bakes
/// every coordinate through the current transform before filling.
fn svg_render(text: &str, full: &str, base: Mat, buf: &mut [u32], w: u32, h: u32, depth: u32, deadline: u64) {
    if depth > 6 {
        return;
    }
    let _ = deadline;
    let mut gstack = [MAT_ID; 24];
    gstack[0] = base;
    let mut gtop = 0usize;
    let mut skip = 0i32;
    let mut rest = text;
    let mut guard = 0u32;
    loop {
        guard += 1;
        if guard > 12000 {
            break;
        }
        #[cfg(target_arch = "x86_64")]
        if crate::arch::timer::now_us() > deadline {
            break;
        }
        let Some(lt) = rest.find('<') else { break };
        let after = &rest[lt + 1..];
        let Some(gt) = after.find('>') else { break };
        let tag = &after[..gt];
        let self_close = tag.ends_with('/');
        rest = &after[gt + 1..];
        if tag.starts_with("!--") || tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        let closing = tag.starts_with('/');
        let name_part = if closing { &tag[1..] } else { tag };
        let name_end = name_part
            .find(|c: char| c == ' ' || c == '\t' || c == '\n' || c == '/' || c == '>')
            .unwrap_or(name_part.len());
        let name = &name_part[..name_end];
        if closing {
            match name {
                "g" => {
                    if gtop > 0 {
                        gtop -= 1;
                    }
                },
                "defs" | "symbol" | "clipPath" | "mask" => {
                    if skip > 0 {
                        skip -= 1;
                    }
                },
                _ => {},
            }
            continue;
        }
        match name {
            "defs" | "symbol" | "clipPath" | "mask" => {
                if !self_close {
                    skip += 1;
                }
                continue;
            },
            "g" => {
                let t = attr(tag, "transform").map(parse_transform).unwrap_or(MAT_ID);
                let m = mat_mul(gstack[gtop], t);
                if !self_close && gtop + 1 < gstack.len() {
                    gstack[gtop + 1] = m;
                    gtop += 1;
                }
                continue;
            },
            _ => {},
        }
        if skip > 0 {
            continue;
        }
        let own = attr(tag, "transform").map(parse_transform).unwrap_or(MAT_ID);
        let ctm = mat_mul(gstack[gtop], own);
        let fill = shape_fill(full, attr(tag, "fill"));
        let stroke = attr(tag, "stroke").and_then(parse_color);
        match name {
            "use" => {
                let href = attr(tag, "href").or_else(|| attr(tag, "xlink:href"));
                if let Some(href) = href {
                    let id = href.trim_start_matches('#');
                    let ux = attr(tag, "x").map(svg_num).unwrap_or(0.0);
                    let uy = attr(tag, "y").map(svg_num).unwrap_or(0.0);
                    let ctm2 = mat_mul(ctm, [1.0, 0.0, 0.0, 1.0, ux, uy]);
                    if let Some(el) = find_element_by_id(full, id) {
                        // If the target is a container the walker would skip
                        // (symbol/svg) or descend (g), render its inner content
                        // directly under the use's transform.
                        let inner = container_inner(el).unwrap_or(el);
                        svg_render(inner, full, ctm2, buf, w, h, depth + 1, deadline);
                    }
                }
            },
            "rect" => {
                if let Some(col) = fill {
                    let x = attr(tag, "x").map(svg_num).unwrap_or(0.0);
                    let y = attr(tag, "y").map(svg_num).unwrap_or(0.0);
                    let rw = attr(tag, "width").map(svg_num).unwrap_or(0.0);
                    let rh = attr(tag, "height").map(svg_num).unwrap_or(0.0);
                    let mut v = [(x, y), (x + rw, y), (x + rw, y + rh), (x, y + rh)];
                    for p in v.iter_mut() {
                        *p = mat_apply(&ctm, p.0, p.1);
                    }
                    fill_poly(buf, w, h, &v, col);
                }
            },
            "circle" | "ellipse" => {
                if let Some(col) = fill {
                    let cx = attr(tag, "cx").map(svg_num).unwrap_or(0.0);
                    let cy = attr(tag, "cy").map(svg_num).unwrap_or(0.0);
                    let (rx, ry) = if name == "circle" {
                        let r = attr(tag, "r").map(svg_num).unwrap_or(0.0);
                        (r, r)
                    } else {
                        (attr(tag, "rx").map(svg_num).unwrap_or(0.0), attr(tag, "ry").map(svg_num).unwrap_or(0.0))
                    };
                    let mut verts: Vec<(f64, f64)> = Vec::with_capacity(48);
                    let mut a = 0;
                    while a < 48 {
                        let t = a as f64 / 48.0 * 6.2831853;
                        verts.push(mat_apply(&ctm, cx + rx * cos(t), cy + ry * sin(t)));
                        a += 1;
                    }
                    fill_poly(buf, w, h, &verts, col);
                }
            },
            "polygon" | "polyline" => {
                if let Some(pts) = attr(tag, "points") {
                    let mut it = pts.split(|c| c == ' ' || c == ',' || c == '\n').filter(|x| !x.is_empty());
                    let mut verts: Vec<(f64, f64)> = Vec::new();
                    while let (Some(a), Some(b)) = (it.next(), it.next()) {
                        verts.push(mat_apply(&ctm, svg_num(a), svg_num(b)));
                    }
                    if let Some(col) = fill {
                        fill_poly(buf, w, h, &verts, col);
                    } else if let Some(col) = stroke {
                        stroke_path(buf, w, h, &verts, col);
                    }
                }
            },
            "line" => {
                if let Some(col) = stroke {
                    let verts = [
                        mat_apply(&ctm, attr(tag, "x1").map(svg_num).unwrap_or(0.0), attr(tag, "y1").map(svg_num).unwrap_or(0.0)),
                        mat_apply(&ctm, attr(tag, "x2").map(svg_num).unwrap_or(0.0), attr(tag, "y2").map(svg_num).unwrap_or(0.0)),
                    ];
                    stroke_path(buf, w, h, &verts, col);
                }
            },
            "path" => {
                if let Some(d) = attr(tag, "d") {
                    let mut polys: Vec<Vec<(f64, f64)>> = Vec::new();
                    path_polys(d, &mut polys);
                    for p in polys.iter_mut() {
                        for pt in p.iter_mut() {
                            *pt = mat_apply(&ctm, pt.0, pt.1);
                        }
                        if let Some(col) = fill {
                            fill_poly(buf, w, h, p, col);
                        } else if let Some(col) = stroke {
                            stroke_path(buf, w, h, p, col);
                        }
                    }
                }
            },
            _ => {},
        }
    }
}

/// If `el` is a container (`<g>`, `<symbol>`, `<svg>`), return its inner
/// content (between the open tag's `>` and its closing tag); else None.
fn container_inner(el: &str) -> Option<&str> {
    let nb = el.as_bytes();
    if nb.first() != Some(&b'<') {
        return None;
    }
    let name_end = el[1..].find(|c: char| c == ' ' || c == '\t' || c == '\n' || c == '>' || c == '/')? + 1;
    let name = &el[1..name_end];
    if name != "g" && name != "symbol" && name != "svg" {
        return None;
    }
    let open_end = el.find('>')?;
    if el.as_bytes().get(open_end.wrapping_sub(1)) == Some(&b'/') {
        return Some(""); // self-closing container: nothing inside
    }
    let close = el.rfind("</")?;
    if close > open_end + 1 {
        Some(&el[open_end + 1..close])
    } else {
        Some("")
    }
}

fn stroke_path(px: &mut [u32], w: u32, h: u32, verts: &[(f64, f64)], color: u32) {
    for seg in verts.windows(2) {
        draw_line(px, w, h, seg[0], seg[1], color);
    }
}

fn draw_line(px: &mut [u32], w: u32, h: u32, a: (f64, f64), b: (f64, f64), color: u32) {
    let dx = (b.0 - a.0).abs();
    let dy = (b.1 - a.1).abs();
    // Cap steps: a stroke coordinate far outside the (already small) canvas
    // would otherwise spin this loop millions of times. Anything past the
    // canvas is clipped anyway, so a generous cap costs nothing.
    let steps = dx.max(dy).max(1.0).min(8192.0) as i32;
    let mut t = 0;
    while t <= steps {
        let u = t as f64 / steps as f64;
        let x = (a.0 + (b.0 - a.0) * u) as i32;
        let y = (a.1 + (b.1 - a.1) * u) as i32;
        if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
            px[(y as u32 * w + x as u32) as usize] = color;
        }
        t += 1;
    }
}

// Small trig for the ellipse tessellation (range-reduced Taylor, like tls/ling).
fn sin(x: f64) -> f64 {
    let mut t = x % 6.2831853;
    if t > 3.14159265 {
        t -= 6.2831853;
    } else if t < -3.14159265 {
        t += 6.2831853;
    }
    let t2 = t * t;
    t * (1.0 - t2 * (1.0 / 6.0 - t2 * (1.0 / 120.0 - t2 * (1.0 / 5040.0 - t2 / 362880.0))))
}
fn cos(x: f64) -> f64 {
    sin(x + 1.5707963)
}

/// Blit an image to the framebuffer, scaled to fit and centered, on a dark
/// ground (nearest-neighbor). Writes the visible buffer directly.
pub fn show_fullscreen(img: &Image) {
    let fw = framebuffer::width();
    let fh = framebuffer::height();
    if fw == 0 || fh == 0 || img.w == 0 || img.h == 0 {
        return;
    }
    framebuffer::fill_rect(0, 0, fw, fh, 0x0a0a18);
    // Fit within (fw, fh) preserving aspect ratio.
    let mut tw = fw;
    let mut th = img.h * fw / img.w;
    if th > fh {
        th = fh;
        tw = img.w * fh / img.h;
    }
    let ox = (fw - tw) / 2;
    let oy = (fh - th) / 2;
    let mut dy = 0;
    while dy < th {
        let sy = dy * img.h / th;
        let row = (sy * img.w) as usize;
        let mut dx = 0;
        while dx < tw {
            let sx = dx * img.w / tw;
            framebuffer::set_pixel(ox + dx, oy + dy, img.px[row + sx as usize]);
            dx += 1;
        }
        dy += 1;
    }
}
