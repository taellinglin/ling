//! The horizon embedder: fetch a page over `netstack`, lay it out with the
//! `horizon-browser` engine (its own package -- ../../../horizon-browser),
//! and render its pixel display list into the desktop's Horizon window.
//! The network/fetch plumbing is the same spirit as `drivers/browser.rs`
//! (`bring`'s embedder) -- only what happens to the bytes once they're a
//! bare `[u8]` body changes: a real DOM + CSS box-model layout instead of
//! wrapped text lines.
//!
//! Redirects: `fetch_raw` follows up to `MAX_REDIRECTS` HTTP 3xx `Location`
//! responses (relative or absolute) before treating a response as final --
//! both the main page and each inline image go through it, so a moved page
//! or a CDN-redirected image just resolves instead of failing outright.
//!
//! Images: `<img src>` is fetched (relative to the page's *final*,
//! post-redirect URL via `resolve_url`), decoded (PNG or a small SVG
//! subset -- no JPEG decoder exists yet, so those fall back to a
//! placeholder), scaled to fit the box the layout already reserved for it,
//! and cached for the page's lifetime -- the same approach
//! `drivers/browser.rs` uses for `bring`.
//!
//! Honest limits, stated (see horizon-browser's own README for the engine
//! side): no external stylesheet fetch yet (only inline `style=""` and
//! `<style>` blocks). Plain HTTP or HTTPS, same as `bring`.

use crate::drivers::{font8x8, framebuffer, image, netstack, theme};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use horizon_browser::{layout, DisplayItem, Page};

const CHAR_W: u32 = 8;
const LINE_H: u32 = 14;
/// Redirect hops a single fetch will follow before giving up -- generous
/// enough for a real CDN chain, bounded so a redirect loop can't hang the
/// desktop.
const MAX_REDIRECTS: u32 = 5;

static mut PAGE: Page = Page::new();
static mut BODY: [u8; 120 * 1024] = [0; 120 * 1024];
static mut CUR_URL: String = String::new();
static mut SCROLL: u32 = 0;
static mut VISIBLE_H: u32 = 300;
static mut STATUS: &'static str = "arrows/PgUp/PgDn/Home/End scroll, 1-9 follow links";

// -- Inline image cache -------------------------------------------------
// Decoded thumbnails, parallel to the current page's `images` table, each
// scaled to fit the box `layout_img` already reserved for it. Heap-backed
// so a page with no images costs nothing, and cleared on each navigation.
const MAX_IMAGES: usize = 24;
const IMG_EMPTY: u8 = 0;
const IMG_READY: u8 = 1;
const IMG_FAILED: u8 = 2;
static mut IMG_STATE: [u8; MAX_IMAGES] = [IMG_EMPTY; MAX_IMAGES];
static mut IMG_PX: [Option<Vec<u32>>; MAX_IMAGES] = [const { None }; MAX_IMAGES];
static mut IMG_W: [u32; MAX_IMAGES] = [0; MAX_IMAGES];
static mut IMG_H: [u32; MAX_IMAGES] = [0; MAX_IMAGES];
/// Scratch for one image download before it's decoded. Sized to hold a full
/// response (matches `drivers/browser.rs`'s `SHEET_TMP` reasoning: the
/// registry's own avatars are ~84KiB).
static mut IMG_TMP: [u8; 160 * 1024] = [0; 160 * 1024];

fn page() -> &'static mut Page {
    unsafe { &mut *&raw mut PAGE }
}

pub fn status() -> &'static str {
    unsafe { STATUS }
}

pub fn current_url() -> &'static str {
    unsafe { (&*&raw const CUR_URL).as_str() }
}

/// Fetch `url`'s raw response (status line + headers + body) into `out`,
/// transparently following up to `MAX_REDIRECTS` 3xx `Location` redirects.
/// Returns the response length and the URL actually fetched (which may
/// differ from `url` after a redirect) -- callers need that final URL to
/// correctly resolve relative sub-resources and to record as the current
/// page location. A bare `host/path` with no scheme is treated as `http://`.
fn fetch_raw(url: &str, out: &mut [u8]) -> Result<(usize, String), &'static str> {
    let mut current = if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("http://{url}")
    };
    for _ in 0..MAX_REDIRECTS {
        let Some((host, port, path, tls)) = netstack::parse_url(&current) else {
            return Err("bad URL (want http://host[:port]/path)");
        };
        let n = if tls {
            let hport = if port == 0 { 443 } else { port };
            let mut noop = |_: &[u8]| {};
            match crate::tls::https_get(host, hport, path, out, &mut noop) {
                Ok(n) if n > 0 => n,
                Ok(_) => return Err("https: empty response"),
                Err(e) => return Err(e),
            }
        } else {
            let Some(ip) = netstack::dns_resolve(host) else {
                return Err("DNS: no address for that host");
            };
            match netstack::http_get_raw(ip, port, path, host, out) {
                Some(n) => n,
                None => return Err("fetch failed (connect refused or timeout)"),
            }
        };
        if n < 12 {
            return Err("malformed response");
        }
        let code = (out[9] - b'0') as u32 * 100 + (out[10] - b'0') as u32 * 10 + (out[11] - b'0') as u32;
        if (300..400).contains(&code) {
            let next = find_header(&out[..n], b"Location")
                .and_then(|loc| core::str::from_utf8(loc).ok())
                .and_then(|loc| resolve_url(&current, loc.trim()));
            match next {
                Some(target) => {
                    current = target;
                    continue;
                },
                None => return Err("redirect with no usable Location header"),
            }
        }
        if code != 200 {
            return Err("fetch failed (non-200 status)");
        }
        return Ok((n, current));
    }
    Err("too many redirects")
}

/// Case-insensitive header lookup in a raw HTTP response, scanning only the
/// header block (up to the first blank line). Returns the trimmed value.
fn find_header<'a>(resp: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let end = crate::tls::http_body_offset(resp);
    let headers = if end > 0 { &resp[..end] } else { resp };
    for line in headers.split(|&b| b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if let Some(colon) = line.iter().position(|&b| b == b':') {
            let (k, v) = (&line[..colon], &line[colon + 1..]);
            if k.eq_ignore_ascii_case(name) {
                return Some(v.strip_prefix(b" ").unwrap_or(v));
            }
        }
    }
    None
}

/// Resolve `href` against absolute `base` (`scheme://host[:port]/path...`).
/// Handles absolute, protocol-relative (`//host/p`), root-relative (`/p`),
/// query/fragment-only, and dot-relative (`x`, `./x`, `../x`) references --
/// the same grammar `drivers/browser.rs`'s `resolve_url` implements for
/// `bring`, ported here to `String` since horizon's embedder already leans
/// on `alloc` rather than fixed byte buffers.
fn resolve_url(base: &str, href: &str) -> Option<String> {
    if href.starts_with("http://") || href.starts_with("https://") {
        return Some(href.to_string());
    }
    let (scheme, rest) = if let Some(r) = base.strip_prefix("https://") {
        ("https://", r)
    } else if let Some(r) = base.strip_prefix("http://") {
        ("http://", r)
    } else {
        return None;
    };
    if let Some(rel) = href.strip_prefix("//") {
        let colon = &scheme[..scheme.len() - 2]; // "https:" / "http:"
        return Some(format!("{colon}//{rel}"));
    }
    let host_end = rest.find('/').unwrap_or(rest.len());
    let host = &rest[..host_end];
    let raw_path = if host_end < rest.len() { &rest[host_end..] } else { "/" };
    let base_path = &raw_path[..raw_path.find(['?', '#']).unwrap_or(raw_path.len())];
    let cut = href.find(['?', '#']).unwrap_or(href.len());
    let (href_path, tail) = (&href[..cut], &href[cut..]);

    let mut out = format!("{scheme}{host}");
    if href.starts_with('#') || href.starts_with('?') {
        out.push_str(base_path);
        if href.starts_with('?') {
            out.push_str(tail);
        }
        return Some(out);
    }
    if href_path.starts_with('/') {
        out.push_str(&normalize_path(href_path));
    } else {
        let dir_end = base_path.rfind('/').map(|i| i + 1).unwrap_or(0);
        let combined = format!("{}{}", &base_path[..dir_end], href_path);
        out.push_str(&normalize_path(&combined));
    }
    if tail.starts_with('?') {
        out.push_str(tail);
    }
    Some(out)
}

/// Collapse `.`/`..` segments in `path`, returning a normalized path that
/// always starts with `/` (and keeps a trailing `/` if `path` had one).
fn normalize_path(path: &str) -> String {
    let mut segs: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {},
            ".." => {
                segs.pop();
            },
            s => segs.push(s),
        }
    }
    let mut out = String::new();
    for s in &segs {
        out.push('/');
        out.push_str(s);
    }
    if out.is_empty() {
        out.push('/');
    } else if path.ends_with('/') {
        out.push('/');
    }
    out
}

/// Fetch `url` and lay it out at `viewport_w` pixels. Returns false (with
/// the reason in `status()`) on any failure -- the previous page stays.
pub fn go(url: &str, viewport_w: u32) -> bool {
    let body = unsafe { &mut *&raw mut BODY };
    let (len, final_url) = match fetch_raw(url, body) {
        Ok(r) => r,
        Err(e) => {
            unsafe { STATUS = e };
            return false;
        },
    };
    let body_off = crate::tls::http_body_offset(&body[..len]);
    layout(&body[body_off..len], viewport_w, CHAR_W, LINE_H, page());
    fetch_page_images(&final_url);
    unsafe {
        SCROLL = 0;
        STATUS = "loaded";
        CUR_URL = final_url;
    }
    true
}

/// Navigate from the URL/search bar: if `input` looks like a URL, go to it
/// (prepending nothing -- a bare host still needs a scheme, same as
/// `bring`); otherwise search DuckDuckGo's HTML `lite` endpoint.
pub fn navigate(input: &str, viewport_w: u32) -> bool {
    let s = input.trim();
    if s.is_empty() {
        return false;
    }
    let has_scheme = s.starts_with("http://") || s.starts_with("https://");
    let looks_url = has_scheme || (!s.contains(' ') && s.contains('.'));
    if looks_url {
        return go(s, viewport_w);
    }
    let mut url = String::from("https://lite.duckduckgo.com/lite/?q=");
    for b in s.bytes() {
        match b {
            b' ' => url.push('+'),
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => url.push(b as char),
            _ => {
                let hex = b"0123456789ABCDEF";
                url.push('%');
                url.push(hex[(b >> 4) as usize] as char);
                url.push(hex[(b & 0xF) as usize] as char);
            },
        }
    }
    go(&url, viewport_w)
}

/// Follow link number `n` (1-based, as displayed), resolving its href
/// (absolute, protocol-relative, root-relative, or dot-relative) against
/// the current page's URL.
pub fn follow(n: usize, viewport_w: u32) -> bool {
    let href = {
        let p = page();
        if n == 0 || n > p.links.len() {
            return false;
        }
        p.links[n - 1].href.clone()
    };
    match resolve_url(current_url(), &href) {
        Some(target) => go(&target, viewport_w),
        None => {
            unsafe { STATUS = "could not resolve link against the current page URL" };
            false
        },
    }
}

/// The box (w, h) the layout reserved for image `idx`, or a sane default if
/// somehow not found (e.g. `idx` came from a page that's since navigated
/// away).
fn image_box(idx: usize) -> (u32, u32) {
    for item in &page().items {
        if let DisplayItem::Image { w, h, image, .. } = item {
            if *image == idx {
                return (*w, *h);
            }
        }
    }
    (CHAR_W * 20, LINE_H * 6)
}

/// Nearest-neighbor scale an image to fit within (max_w, max_h) preserving
/// aspect ratio; returns (w, h, pixels). Same algorithm `drivers/browser.rs`
/// uses for `bring`'s inline thumbnails.
fn scale_to_box(img: &image::Image, max_w: u32, max_h: u32) -> (u32, u32, Vec<u32>) {
    let iw = img.w.max(1);
    let ih = img.h.max(1);
    let mut sw = max_w;
    let mut sh = (ih * max_w) / iw;
    if sh > max_h {
        sh = max_h;
        sw = (iw * max_h) / ih;
    }
    sw = sw.clamp(1, max_w);
    sh = sh.clamp(1, max_h);
    let mut out = Vec::with_capacity((sw * sh) as usize);
    for oy in 0..sh {
        let syy = (oy * ih / sh).min(ih - 1);
        for ox in 0..sw {
            let sxx = (ox * iw / sw).min(iw - 1);
            out.push(img.px[(syy * iw + sxx) as usize]);
        }
    }
    (sw, sh, out)
}

/// Fetch, decode (PNG or subset-SVG), and cache each of the page's inline
/// images, scaled to fit the box the layout already reserved for it.
/// Bounded to `MAX_IMAGES`; redirects are followed the same as the page
/// fetch. JPEG has no decoder, so those fall back to the `[img]` placeholder.
fn fetch_page_images(base: &str) {
    unsafe {
        let px = &mut *&raw mut IMG_PX;
        for i in 0..MAX_IMAGES {
            IMG_STATE[i] = IMG_EMPTY;
            px[i] = None;
        }
    }
    let count = page().images.len().min(MAX_IMAGES);
    let tmp = unsafe { &mut *&raw mut IMG_TMP };
    for i in 0..count {
        let src = page().images[i].src.clone();
        // Dedup: an earlier image with the identical src that already
        // loaded gets its decoded thumbnail reused instead of re-fetched.
        let mut duped = false;
        for j in 0..i {
            if page().images[j].src == src && unsafe { IMG_STATE[j] } == IMG_READY {
                unsafe {
                    IMG_W[i] = IMG_W[j];
                    IMG_H[i] = IMG_H[j];
                    let px = &mut *&raw mut IMG_PX;
                    px[i] = px[j].clone();
                    IMG_STATE[i] = IMG_READY;
                }
                duped = true;
                break;
            }
        }
        if duped {
            continue;
        }
        let Some(iu) = resolve_url(base, &src) else {
            unsafe { IMG_STATE[i] = IMG_FAILED };
            continue;
        };
        let Ok((n, _)) = fetch_raw(&iu, tmp) else {
            unsafe { IMG_STATE[i] = IMG_FAILED };
            continue;
        };
        let body_off = crate::tls::http_body_offset(&tmp[..n]);
        let bytes = &tmp[body_off..n];
        let decoded = if image::looks_svg(bytes) {
            image::decode_svg_capped(bytes, 256)
        } else {
            image::decode_png(bytes)
        };
        match decoded {
            Some(img) if img.w > 0 && img.h > 0 => {
                let (box_w, box_h) = image_box(i);
                let (sw, sh, px) = scale_to_box(&img, box_w.max(1), box_h.max(1));
                unsafe {
                    IMG_W[i] = sw;
                    IMG_H[i] = sh;
                    (&mut *&raw mut IMG_PX)[i] = Some(px);
                    IMG_STATE[i] = IMG_READY;
                }
            },
            _ => unsafe { IMG_STATE[i] = IMG_FAILED },
        }
    }
}

fn scroll_max() -> u32 {
    let vis = unsafe { VISIBLE_H }.max(1);
    page().height.saturating_sub(vis.saturating_sub(40))
}

/// Scroll by `delta` pixels (negative = up).
pub fn scroll(delta: i32) {
    unsafe {
        let s = (SCROLL as i64 + (delta * LINE_H as i32) as i64).clamp(0, scroll_max() as i64);
        SCROLL = s as u32;
    }
}

pub fn scroll_page(dir: i32) {
    let step = (unsafe { VISIBLE_H }.max(60) - 40) as i32;
    let s = (unsafe { SCROLL } as i64 + (dir * step) as i64).clamp(0, scroll_max() as i64);
    unsafe { SCROLL = s as u32 };
}

pub fn scroll_home() {
    unsafe { SCROLL = 0 };
}

pub fn scroll_end() {
    unsafe { SCROLL = scroll_max() };
}

/// Handle a click in the page content at `rel_y` pixels below `draw_page`'s
/// origin: if it lands on a link's text, follow it. Returns true if a link
/// was followed.
pub fn click_page(rel_x: i64, rel_y: i64, viewport_w: u32) -> bool {
    if rel_y < 18 {
        return false;
    }
    let py = rel_y - 18 + unsafe { SCROLL } as i64;
    let p = page();
    for item in &p.items {
        if let DisplayItem::Text { x, y, text, link: Some(li), .. } = item {
            let iy = *y as i64;
            if py >= iy && py < iy + LINE_H as i64 {
                let ix = *x as i64;
                let tw = text.chars().count() as i64 * CHAR_W as i64;
                if rel_x >= ix && rel_x < ix + tw {
                    return follow(*li as usize + 1, viewport_w);
                }
            }
        }
    }
    false
}

/// Render just the page body into a content rect (real pixels), starting
/// at `y`. The WM draws the URL/search bar above this itself.
pub fn draw_page(x: u32, y: u32, w: u32, h: u32) {
    unsafe { VISIBLE_H = h };
    let dim = theme::color(theme::SLOT_DIM);
    let text_color = theme::color(theme::SLOT_TEXT);
    let accent = theme::color(theme::SLOT_ACCENT);

    let p = page();
    let panel = p.background.unwrap_or(theme::color(theme::SLOT_PANEL));
    framebuffer::back_fill_rect(x, y, w, h, panel);
    font8x8::draw_str(x, y, status().as_bytes(), dim, panel);

    if p.items.is_empty() {
        font8x8::draw_str(x, y + 22, b"horizon: a real box-model browser for lingos", text_color, panel);
        font8x8::draw_str(x, y + 40, b"type a URL or search above and press Enter (http:// for now)", dim, panel);
        return;
    }

    let content_y = y + 18;
    let scroll = unsafe { SCROLL } as i64;
    let bottom = (y + h) as i64;

    // Pre-scan items above the viewport so link numbers stay stable while
    // scrolling (matches `bring`'s browser.rs -- see its draw_page).
    let mut link_counter = 0usize;
    for item in &p.items {
        if let DisplayItem::Text { y: iy, link: Some(li), .. } = item {
            if (*iy as i64) < scroll {
                link_counter = link_counter.max(*li as usize + 1);
            }
        }
    }

    for item in &p.items {
        let iy = match item {
            DisplayItem::Rect { y, .. } | DisplayItem::Text { y, .. } | DisplayItem::Image { y, .. } | DisplayItem::Canvas { y, .. } => *y,
        };
        let ih = match item {
            DisplayItem::Rect { h, .. } | DisplayItem::Image { h, .. } | DisplayItem::Canvas { h, .. } => *h,
            DisplayItem::Text { .. } => LINE_H,
        };
        let ry = content_y as i64 + iy as i64 - scroll;
        if ry + ih as i64 <= content_y as i64 || ry >= bottom {
            continue;
        }
        let ry = ry.max(0) as u32;
        match item {
            DisplayItem::Rect { x: ix, w: iw, h: rh, color, .. } => {
                framebuffer::back_fill_rect(x + ix, ry, *iw, *rh, *color);
            },
            DisplayItem::Text { x: ix, text, color, bold: _, heading, link, .. } => {
                let mut cx = x + ix;
                if let Some(li) = link {
                    let li = *li as usize;
                    if li + 1 > link_counter {
                        link_counter = li + 1;
                        let mut nb = [0u8; 5];
                        let mut n = 0usize;
                        nb[n] = b'[';
                        n += 1;
                        let d = link_counter;
                        if d >= 10 {
                            nb[n] = b'0' + (d / 10) as u8;
                            n += 1;
                        }
                        nb[n] = b'0' + (d % 10) as u8;
                        nb[n + 1] = b']';
                        n += 2;
                        font8x8::draw_str(cx, ry, &nb[..n], theme::color(theme::SLOT_ERROR), panel);
                        cx += n as u32 * 8 + 4;
                    }
                }
                let col = color.unwrap_or(if link.is_some() || *heading { accent } else { text_color });
                font8x8::draw_str(cx, ry, text.as_bytes(), col, panel);
            },
            DisplayItem::Canvas { x: ix, y: _, w: iw, h: ih2, canvas } => {
                if let Some(surf) = p.canvases.get(*canvas) {
                    let cw = surf.w.min(*iw);
                    let ch = surf.h.min(*ih2);
                    for py in 0..ch {
                        let dy = ry + py;
                        if dy >= bottom as u32 {
                            break;
                        }
                        for px in 0..cw {
                            framebuffer::back_set_pixel(x + ix + px, dy, surf.pixels[(py * surf.w + px) as usize]);
                        }
                    }
                }
            },
            DisplayItem::Image { x: ix, w: iw, h: ih2, image, .. } => {
                let ii = *image;
                let ready = ii < MAX_IMAGES && unsafe { IMG_STATE[ii] } == IMG_READY;
                let blitted = ready
                    && (unsafe { &(&*&raw const IMG_PX)[ii] }).as_ref().map(|px| {
                        let (iw2, ih3) = unsafe { (IMG_W[ii], IMG_H[ii]) };
                        for oy in 0..ih3 {
                            let py = ry + oy;
                            if py >= bottom as u32 {
                                break;
                            }
                            for ox in 0..iw2 {
                                framebuffer::back_set_pixel(x + ix + ox, py, px[(oy * iw2 + ox) as usize]);
                            }
                        }
                    })
                    .is_some();
                if !blitted {
                    // Still loading, failed, or a format with no decoder
                    // (JPEG) -- a visible placeholder beats silently
                    // dropping the box.
                    framebuffer::back_fill_rect(x + ix, ry, *iw, *ih2, theme::color(theme::SLOT_PANEL_BORDER));
                    font8x8::draw_str(x + ix + 4, ry + 4, b"[img]", dim, theme::color(theme::SLOT_PANEL_BORDER));
                }
            },
        }
    }
}
