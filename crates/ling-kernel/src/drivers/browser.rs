//! The browser embedder: fetch a page over `netstack`, lay it out with
//! the `bring-browser` engine (its own package -- ../../../bring-browser),
//! and either render into the desktop's Browser window (scroll +
//! follow-links-by-number) or dump lynx-style to the text console for
//! `bring --browse`. All state kernel-side per the usual `.ling`
//! no-rebinding constraint.
//!
//! Honest limits, stated: plain HTTP only (https URLs are refused with a
//! message, never silently downgraded -- no TLS stack yet); the engine
//! renders the documented HTML subset (see bring-browser's README and
//! roadmap); pages over ~120KiB body are truncated with a marker.

use crate::drivers::{font8x8, framebuffer, image, netstack, theme};
use alloc::vec::Vec;
use bring_browser::{layout_styled, LineKind, Page, MAX_IMAGES, MAX_URL, NO_COLOR};

static mut PAGE: Page = Page::new();
static mut BODY: [u8; 120 * 1024] = [0; 120 * 1024];
/// Concatenated text of the page's external stylesheets, fed to the layout
/// engine (it caps what it parses, so this need not be huge).
static mut CSS_SHEETS: [u8; 24 * 1024] = [0; 24 * 1024];
/// Scratch for one stylesheet or image download before it's used. Sized to
/// hold a full image response (the registry's avatars are ~84KiB) so images
/// don't truncate and fail to decode.
static mut SHEET_TMP: [u8; 160 * 1024] = [0; 160 * 1024];

// Decoded inline-image cache, parallel to the current page's `images` table.
// Each entry is a thumbnail scaled to fit the reserved inline box; heap-backed
// so a page with no images costs nothing, and cleared on each navigation.
const IMG_EMPTY: u8 = 0;
const IMG_READY: u8 = 1;
const IMG_FAILED: u8 = 2;
/// Inline image box: scaled to fit within these bounds (px). Height matches the
/// ~7 rows the engine reserves per image (IMG_RESERVE_ROWS + the image line).
const IMG_BOX_W: u32 = 560;
const IMG_BOX_H: u32 = 96;
static mut IMG_STATE: [u8; MAX_IMAGES] = [IMG_EMPTY; MAX_IMAGES];
static mut IMG_PX: [Option<Vec<u32>>; MAX_IMAGES] = [const { None }; MAX_IMAGES];
static mut IMG_W: [u32; MAX_IMAGES] = [0; MAX_IMAGES];
static mut IMG_H: [u32; MAX_IMAGES] = [0; MAX_IMAGES];

// A keep-alive session for the sub-resource phase (stylesheets + images):
// same-host, same-scheme resources reuse one TLS handshake instead of paying
// one each. Opened after the main page loads, closed when the page is done.
static mut SESS_ACTIVE: bool = false;
static mut SESS_HOST: [u8; 128] = [0; 128];
static mut SESS_HOST_LEN: usize = 0;

fn subresource_open(host: &str) {
    let hb = host.as_bytes();
    if hb.len() >= 128 {
        return;
    }
    let mut noop = |_: &[u8]| {};
    if crate::tls::https_open(host, 443, &mut noop) {
        unsafe {
            let sh = &mut *&raw mut SESS_HOST;
            sh[..hb.len()].copy_from_slice(hb);
            SESS_HOST_LEN = hb.len();
            SESS_ACTIVE = true;
        }
    }
}

fn subresource_close() {
    unsafe {
        if SESS_ACTIVE {
            crate::tls::https_close();
            SESS_ACTIVE = false;
        }
    }
}

fn sess_host_matches(host: &str) -> bool {
    unsafe { SESS_ACTIVE && host.as_bytes() == &(&*&raw const SESS_HOST)[..SESS_HOST_LEN] }
}
static mut CUR_URL: [u8; MAX_URL] = [0; MAX_URL];
static mut CUR_URL_LEN: usize = 0;
static mut SCROLL: usize = 0;
/// Visible content rows, captured by `draw_page` so the scroll helpers can
/// page and clamp against the real viewport height.
static mut VISIBLE_ROWS: usize = 20;
static mut STATUS: &'static str = "arrows/PgUp/PgDn/Home/End scroll, 1-9 follow links";

fn page() -> &'static mut Page {
    unsafe { &mut *&raw mut PAGE }
}

pub fn status() -> &'static str {
    unsafe { STATUS }
}

pub fn current_url() -> &'static str {
    unsafe { core::str::from_utf8(&(&*&raw const CUR_URL)[..CUR_URL_LEN]).unwrap_or("") }
}

fn set_url(url: &str) {
    unsafe {
        let n = url.len().min(MAX_URL);
        let buf = &mut *&raw mut CUR_URL;
        buf[..n].copy_from_slice(&url.as_bytes()[..n]);
        CUR_URL_LEN = n;
    }
}

/// Fetch `url` and lay it out at `cols` columns. Returns false (with the
/// reason in `status()`) on any failure -- the previous page stays.
/// Paint a centered "loading <url>" toast and present it immediately.
/// The desktop loop is single-threaded, so a fetch blocks all redraws
/// while it runs -- without this the screen would sit stale (looking
/// frozen) until the fetch returns. This at least tells the user what's
/// happening; the bounded netstack budget keeps the block short.
fn loading_toast(url: &str) {
    let w = framebuffer::width();
    let bw = 520u32.min(w.saturating_sub(40));
    let bx = (w.saturating_sub(bw)) / 2;
    let by = 60u32;
    framebuffer::back_blend_rounded_rect(bx + 4, by + 5, bw, 40, 10, theme::color(theme::SLOT_SHADOW), 90);
    framebuffer::back_fill_rounded_rect(bx, by, bw, 40, 10, theme::color(theme::SLOT_PANEL_BORDER));
    framebuffer::back_fill_rounded_rect(bx + 1, by + 1, bw - 2, 38, 9, theme::color(theme::SLOT_PANEL));
    font8x8::draw_str(bx + 14, by + 8, b"loading", theme::color(theme::SLOT_ACCENT), theme::color(theme::SLOT_PANEL));
    let u = url.as_bytes();
    let n = u.len().min(((bw - 90) / 8) as usize);
    font8x8::draw_str(bx + 84, by + 8, &u[..n], theme::color(theme::SLOT_TEXT), theme::color(theme::SLOT_PANEL));
    font8x8::draw_str(bx + 14, by + 22, b"(the desktop waits here until the fetch returns)", theme::color(theme::SLOT_DIM), theme::color(theme::SLOT_PANEL));
    framebuffer::present();
}

/// Max HTTP redirects to follow before giving up (loop guard).
const MAX_REDIRECTS: usize = 6;

/// Case-insensitive header lookup in a raw HTTP response (header block only).
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

pub fn go(url: &str, cols: usize) -> bool {
    let body = unsafe { &mut *&raw mut BODY };
    // Current URL, rewritten in place as we follow redirects. Almost every
    // real site 301/302s the bare/http URL to its canonical https one, so
    // without redirect following bring only loaded rare 200-direct pages.
    let mut cur = [0u8; 512];
    let s0 = url.trim();
    let mut cur_len = s0.len().min(cur.len());
    cur[..cur_len].copy_from_slice(&s0.as_bytes()[..cur_len]);

    let mut len = 0usize;
    let mut got = false;
    for _ in 0..MAX_REDIRECTS {
        let curs = core::str::from_utf8(&cur[..cur_len]).unwrap_or("");
        let Some((host, port, path, tls)) = netstack::parse_url(curs) else {
            unsafe { STATUS = "bad URL (want http://host[:port]/path)" };
            return false;
        };
        loading_toast(curs);
        // Fetch the FULL raw response (headers + body) so we can read status
        // + Location. https_get already returns the full response.
        let n = if tls {
            let hport = if port == 0 { 443 } else { port };
            let mut noop = |_: &[u8]| {};
            match crate::tls::https_get(host, hport, path, body, &mut noop) {
                Ok(n) if n > 0 => n,
                Ok(_) => {
                    unsafe { STATUS = "https: empty response" };
                    return false;
                },
                Err(e) => {
                    unsafe { STATUS = e };
                    return false;
                },
            }
        } else {
            let Some(ip) = netstack::dns_resolve(host) else {
                unsafe { STATUS = "DNS: no address for that host" };
                return false;
            };
            match netstack::http_get_raw(ip, port, path, host, body) {
                Some(n) => n,
                None => {
                    unsafe { STATUS = "fetch failed (connect refused or timeout)" };
                    return false;
                },
            }
        };
        if n < 12 {
            unsafe { STATUS = "malformed response" };
            return false;
        }
        let code = (body[9] - b'0') as u32 * 100
            + (body[10] - b'0') as u32 * 10
            + (body[11] - b'0') as u32;
        if (300..400).contains(&code) {
            // Resolve Location into a scratch buffer (still borrowing `curs`),
            // then rewrite `cur` after those borrows end.
            let mut nb = [0u8; 512];
            let nlen = find_header(&body[..n], b"Location")
                .and_then(|loc| core::str::from_utf8(loc).ok())
                .and_then(|loc| resolve_url(curs, loc.trim(), &mut nb));
            match nlen {
                Some(nn) => {
                    let nn = nn.min(cur.len());
                    cur[..nn].copy_from_slice(&nb[..nn]);
                    cur_len = nn;
                    continue;
                },
                None => {
                    unsafe { STATUS = "redirect with no usable Location" };
                    return false;
                },
            }
        }
        if code != 200 {
            unsafe { STATUS = "fetch failed (non-200 status)" };
            return false;
        }
        len = n;
        got = true;
        break;
    }
    if !got {
        unsafe { STATUS = "too many redirects" };
        return false;
    }

    // Render the final 200 response.
    let body_off = crate::tls::http_body_offset(&body[..len]);
    let final_url = core::str::from_utf8(&cur[..cur_len]).unwrap_or("");
    let (host, _port, _path, tls) = netstack::parse_url(final_url).unwrap_or(("", 0, "", false));
    // Keep-alive session to the page host for sub-resources (CSS + images).
    if tls {
        subresource_open(host);
    }
    let css_len = fetch_stylesheets(&body[body_off..len], final_url);
    let css = unsafe { &(&*&raw const CSS_SHEETS)[..css_len] };
    layout_styled(&body[body_off..len], css, cols, page());
    fetch_page_images(final_url);
    subresource_close();
    unsafe {
        SCROLL = 0;
        STATUS = if page().truncated {
            "loaded (truncated: page bigger than the line buffer)"
        } else {
            "loaded"
        };
    }
    set_url(final_url);
    true
}

/// Fetch a URL's response BODY (headers stripped) into `out`, returning the
/// length. Used for sub-resources (stylesheets); one-shot per resource.
fn fetch_body(url: &str, out: &mut [u8]) -> Option<usize> {
    let (host, port, path, tls) = netstack::parse_url(url)?;
    let strip = |out: &mut [u8], n: usize| -> usize {
        let off = crate::tls::http_body_offset(&out[..n]);
        if off > 0 {
            out.copy_within(off..n, 0);
            n - off
        } else {
            n
        }
    };
    if tls {
        // Reuse the open sub-resource session for same-host fetches (one
        // handshake for all of a page's stylesheets + images).
        if sess_host_matches(host) {
            if let Some(n) = crate::tls::https_next(host, path, out) {
                if n > 0 {
                    return Some(strip(out, n));
                }
            }
            return None;
        }
        let mut noop = |_: &[u8]| {};
        match crate::tls::https_get(host, port, path, out, &mut noop) {
            Ok(n) if n > 0 => Some(strip(out, n)),
            _ => None,
        }
    } else {
        let ip = netstack::dns_resolve(host)?;
        netstack::http_get(ip, port, path, host, out)
    }
}

/// Extract the value of attribute `attr` from a tag's bytes (quoted or bare).
fn tag_attr<'t>(tag: &'t [u8], attr: &[u8]) -> Option<&'t [u8]> {
    let mut i = 0;
    while i + attr.len() + 1 < tag.len() {
        let at_word_start = i == 0 || tag[i - 1] == b' ' || tag[i - 1] == b'\t' || tag[i - 1] == b'\n';
        if at_word_start && tag[i..].len() > attr.len() {
            let mut m = true;
            for j in 0..attr.len() {
                if tag[i + j].to_ascii_lowercase() != attr[j] {
                    m = false;
                    break;
                }
            }
            if m && tag[i + attr.len()] == b'=' {
                let v = &tag[i + attr.len() + 1..];
                return Some(match v.first() {
                    Some(&q) if q == b'"' || q == b'\'' => {
                        let end = v[1..].iter().position(|&c| c == q).map(|p| p + 1).unwrap_or(v.len());
                        &v[1..end]
                    },
                    _ => {
                        let end = v.iter().position(|&c| c == b' ' || c == b'\t' || c == b'>').unwrap_or(v.len());
                        &v[..end]
                    },
                });
            }
        }
        i += 1;
    }
    None
}

fn contains_ci(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() {
        return needle.is_empty();
    }
    (0..=hay.len() - needle.len()).any(|i| {
        (0..needle.len()).all(|j| hay[i + j].to_ascii_lowercase() == needle[j])
    })
}

/// Scan `html` for `<link rel="stylesheet" href="...">`, fetch each sheet
/// (resolved against `base`), and concatenate their bodies into CSS_SHEETS.
/// Bounded to a few sheets and the buffer size. Returns bytes written.
fn fetch_stylesheets(html: &[u8], base: &str) -> usize {
    let sheets = unsafe { &mut *&raw mut CSS_SHEETS };
    let tmp = unsafe { &mut *&raw mut SHEET_TMP };
    let mut total = 0usize;
    let mut count = 0usize;
    let mut i = 0usize;
    while i + 5 < html.len() && count < 6 && total < sheets.len() {
        // Find the next "<link".
        if !(html[i] == b'<'
            && html[i + 1].to_ascii_lowercase() == b'l'
            && html[i + 2].to_ascii_lowercase() == b'i'
            && html[i + 3].to_ascii_lowercase() == b'n'
            && html[i + 4].to_ascii_lowercase() == b'k')
        {
            i += 1;
            continue;
        }
        let Some(rel_end) = html[i..].iter().position(|&c| c == b'>') else { break };
        let tag = &html[i..i + rel_end];
        i += rel_end + 1;
        // Only stylesheet links.
        let is_sheet = tag_attr(tag, b"rel").map(|r| contains_ci(r, b"stylesheet")).unwrap_or(false);
        let Some(href) = tag_attr(tag, b"href") else { continue };
        if !is_sheet {
            continue;
        }
        let Ok(hstr) = core::str::from_utf8(href) else { continue };
        let mut url_buf = [0u8; MAX_URL * 2];
        let Some(ulen) = resolve_url(base, hstr, &mut url_buf) else { continue };
        let Ok(sheet_url) = core::str::from_utf8(&url_buf[..ulen]) else { continue };
        count += 1;
        if let Some(blen) = fetch_body(sheet_url, tmp) {
            let take = blen.min(sheets.len() - total);
            sheets[total..total + take].copy_from_slice(&tmp[..take]);
            total += take;
            // A newline between sheets so rules never run together.
            if total < sheets.len() {
                sheets[total] = b'\n';
                total += 1;
            }
        }
    }
    total
}

/// Navigate from the URL/search bar: if `input` looks like a URL, go to
/// it (prepending http:// when it has no scheme); otherwise treat it as a
/// search and route through DuckDuckGo's HTML endpoint over HTTPS. DDG's
/// `lite` view is server-rendered plain HTML (no JavaScript), so it lays out
/// cleanly in the text-flow engine and its result links follow normally.
pub fn navigate(input: &str, cols: usize) -> bool {
    let s = input.trim();
    if s.is_empty() {
        return false;
    }
    let has_scheme = s.starts_with("http://") || s.starts_with("https://");
    let looks_url = has_scheme || (!s.contains(' ') && s.contains('.'));
    if looks_url {
        return go(s, cols);
    }
    // Search: https://lite.duckduckgo.com/lite/?q=<url-encoded input>
    let mut url = [0u8; 400];
    let prefix = b"https://lite.duckduckgo.com/lite/?q=";
    let mut n = prefix.len();
    url[..n].copy_from_slice(prefix);
    for &b in s.as_bytes() {
        if n + 3 >= url.len() {
            break;
        }
        match b {
            b' ' => {
                url[n] = b'+';
                n += 1;
            },
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                url[n] = b;
                n += 1;
            },
            _ => {
                // percent-encode everything else
                let hex = b"0123456789ABCDEF";
                url[n] = b'%';
                url[n + 1] = hex[(b >> 4) as usize];
                url[n + 2] = hex[(b & 0xF) as usize];
                n += 3;
            },
        }
    }
    let target = core::str::from_utf8(&url[..n]).unwrap_or("");
    go(target, cols)
}

/// Copy `s` into `out` at `w`, advancing `w`; false if it wouldn't fit.
fn push_bytes(out: &mut [u8], w: &mut usize, s: &[u8]) -> bool {
    if *w + s.len() > out.len() {
        return false;
    }
    out[*w..*w + s.len()].copy_from_slice(s);
    *w += s.len();
    true
}

/// Normalize a path's `.`/`..` segments, writing the result (leading '/',
/// trailing '/' preserved) to `out` at `w`. `path` is the path only (no query).
fn normalize_path(path: &[u8], out: &mut [u8], w: &mut usize) -> bool {
    let mut segs: [(usize, usize); 64] = [(0, 0); 64];
    let mut n = 0usize;
    let plen = path.len();
    let mut i = 0usize;
    while i < plen {
        while i < plen && path[i] == b'/' {
            i += 1;
        }
        let s = i;
        while i < plen && path[i] != b'/' {
            i += 1;
        }
        if i > s {
            let seg = &path[s..i];
            if seg == b"." {
                // current dir: drop
            } else if seg == b".." {
                if n > 0 {
                    n -= 1;
                }
            } else if n < segs.len() {
                segs[n] = (s, i - s);
                n += 1;
            }
        }
    }
    if n == 0 {
        return push_bytes(out, w, b"/");
    }
    for &(s, l) in &segs[..n] {
        if !push_bytes(out, w, b"/") || !push_bytes(out, w, &path[s..s + l]) {
            return false;
        }
    }
    if plen > 0 && path[plen - 1] == b'/' {
        return push_bytes(out, w, b"/");
    }
    true
}

/// Resolve `href` against absolute `base` into `out`, returning the length.
/// Handles absolute, protocol-relative (`//host/p`), root-relative (`/p`),
/// query-only (`?q`), fragment-only (`#f`), and dot-relative (`x`, `./x`,
/// `../x`) references -- enough for real intra-site navigation.
fn resolve_url(base: &str, href: &str, out: &mut [u8]) -> Option<usize> {
    let mut w = 0usize;
    // Already absolute.
    if href.starts_with("http://") || href.starts_with("https://") {
        return push_bytes(out, &mut w, href.as_bytes()).then_some(w);
    }
    // Split base into scheme, host, and path (path begins with '/').
    let (scheme, rest) = if let Some(r) = base.strip_prefix("https://") {
        ("https://", r)
    } else if let Some(r) = base.strip_prefix("http://") {
        ("http://", r)
    } else {
        ("http://", base)
    };
    // Protocol-relative: keep base's scheme, take href's host+path.
    if let Some(rel) = href.strip_prefix("//") {
        let colon = &scheme[..scheme.len() - 2]; // "https:" / "http:"
        if !push_bytes(out, &mut w, colon.as_bytes())
            || !push_bytes(out, &mut w, b"//")
            || !push_bytes(out, &mut w, rel.as_bytes())
        {
            return None;
        }
        return Some(w);
    }
    let host_end = rest.find('/').unwrap_or(rest.len());
    let host = &rest[..host_end];
    let raw_path = if host_end < rest.len() { &rest[host_end..] } else { "/" };
    // Base path without its own query/fragment.
    let base_path = {
        let b = raw_path.as_bytes();
        let mut e = b.len();
        for (i, &c) in b.iter().enumerate() {
            if c == b'?' || c == b'#' {
                e = i;
                break;
            }
        }
        &raw_path[..e]
    };
    // Split href into a path part and a query/fragment tail.
    let (href_path, tail) = {
        let b = href.as_bytes();
        let mut cut = b.len();
        for (i, &c) in b.iter().enumerate() {
            if c == b'?' || c == b'#' {
                cut = i;
                break;
            }
        }
        (&href[..cut], &href[cut..])
    };

    // Everything below is scheme://host + normalized path + tail.
    if !push_bytes(out, &mut w, scheme.as_bytes()) || !push_bytes(out, &mut w, host.as_bytes()) {
        return None;
    }

    if href.starts_with('#') || href.starts_with('?') {
        // Same document: reuse the base path, apply href's tail (the fragment
        // is dropped -- our engine has no in-page anchors, so it just reloads).
        if !push_bytes(out, &mut w, base_path.as_bytes()) {
            return None;
        }
        if href.starts_with('?') && !push_bytes(out, &mut w, tail.as_bytes()) {
            return None;
        }
        return Some(w);
    }

    if href_path.starts_with('/') {
        // Root-relative.
        if !normalize_path(href_path.as_bytes(), out, &mut w) {
            return None;
        }
    } else {
        // Dot-relative: resolve against the base path's directory.
        let bp = base_path.as_bytes();
        let dir_end = bp.iter().rposition(|&c| c == b'/').map(|i| i + 1).unwrap_or(0);
        let mut combined = [0u8; 512];
        let mut cn = 0usize;
        if !push_bytes(&mut combined, &mut cn, &bp[..dir_end])
            || !push_bytes(&mut combined, &mut cn, href_path.as_bytes())
        {
            return None;
        }
        if !normalize_path(&combined[..cn], out, &mut w) {
            return None;
        }
    }
    // Preserve a query string from the href (fragment dropped).
    if tail.starts_with('?') && !push_bytes(out, &mut w, tail.as_bytes()) {
        return None;
    }
    Some(w)
}

/// Follow link number `n` (1-based, as displayed), resolving its href against
/// the current page URL. Absolute, root-relative, and dot-relative links all
/// navigate; https works too now.
pub fn follow(n: usize, cols: usize) -> bool {
    let p = page();
    if n == 0 || n > p.link_count {
        return false;
    }
    let mut href_buf = [0u8; MAX_URL];
    let hb = p.links[n - 1].href();
    let hn = hb.len().min(href_buf.len());
    href_buf[..hn].copy_from_slice(&hb[..hn]);
    let Ok(href) = core::str::from_utf8(&href_buf[..hn]) else { return false };

    let mut cur_buf = [0u8; MAX_URL];
    let cb = current_url().as_bytes();
    let cn = cb.len().min(cur_buf.len());
    cur_buf[..cn].copy_from_slice(&cb[..cn]);
    let Ok(base) = core::str::from_utf8(&cur_buf[..cn]) else { return false };

    let mut url_buf = [0u8; MAX_URL * 2];
    let Some(len) = resolve_url(base, href, &mut url_buf) else {
        unsafe { STATUS = "link URL too long to resolve" };
        return false;
    };
    let Ok(target) = core::str::from_utf8(&url_buf[..len]) else { return false };
    go(target, cols)
}

/// Nearest-neighbor scale an image to fit within (max_w, max_h) preserving
/// aspect ratio; returns (w, h, pixels).
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
/// images as a scaled thumbnail. Bounded to the first several images so a
/// gallery page can't stall the load indefinitely. JPEG has no decoder, so
/// those fall back to their alt text.
fn fetch_page_images(base: &str) {
    // Drop any images cached from the previous page.
    unsafe {
        let px = &mut *&raw mut IMG_PX;
        for i in 0..MAX_IMAGES {
            IMG_STATE[i] = IMG_EMPTY;
            px[i] = None;
        }
    }
    let count = page().image_count.min(MAX_IMAGES);
    let tmp = unsafe { &mut *&raw mut SHEET_TMP };
    for i in 0..count.min(10) {
        // Copy the src out of the page before any fetch (which reuses buffers).
        let mut sb = [0u8; MAX_URL];
        let src = page().images[i].src();
        let sn = src.len().min(sb.len());
        sb[..sn].copy_from_slice(&src[..sn]);
        // Dedup: if an earlier image has the identical src and already loaded,
        // reuse its decoded thumbnail instead of fetching + rasterizing again
        // (pages reference the same logo/emblem repeatedly).
        let mut duped = false;
        for j in 0..i {
            if page().images[j].src() == &sb[..sn] && unsafe { IMG_STATE[j] } == IMG_READY {
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
        let Ok(srcs) = core::str::from_utf8(&sb[..sn]) else {
            unsafe { IMG_STATE[i] = IMG_FAILED };
            continue;
        };
        let mut url_buf = [0u8; MAX_URL * 2];
        let Some(ulen) = resolve_url(base, srcs, &mut url_buf) else {
            unsafe { IMG_STATE[i] = IMG_FAILED };
            continue;
        };
        let Ok(iu) = core::str::from_utf8(&url_buf[..ulen]) else {
            unsafe { IMG_STATE[i] = IMG_FAILED };
            continue;
        };
        let Some(blen) = fetch_body(iu, tmp) else {
            unsafe { IMG_STATE[i] = IMG_FAILED };
            continue;
        };
        let decoded = if image::looks_svg(&tmp[..blen]) {
            // Rasterize SVG at a small canvas -- it's only a thumbnail, and a
            // full-resolution complex SVG can take seconds to scanline-fill.
            image::decode_svg_capped(&tmp[..blen], 160)
        } else {
            image::decode_png(&tmp[..blen])
        };
        match decoded {
            Some(img) if img.w > 0 && img.h > 0 => {
                let (sw, sh, px) = scale_to_box(&img, IMG_BOX_W, IMG_BOX_H);
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

/// Blit cached image `ii` at (x, ry), clamped to `max_w` wide and not past
/// `bottom` (the content rect's lower edge).
fn blit_image(ii: usize, x: u32, ry: u32, max_w: u32, bottom: u32) {
    if ii >= MAX_IMAGES || unsafe { IMG_STATE[ii] } != IMG_READY {
        return;
    }
    let (iw, ih) = unsafe { (IMG_W[ii], IMG_H[ii]) };
    let Some(px) = (unsafe { &(&*&raw const IMG_PX)[ii] }) else { return };
    let cols = iw.min(max_w);
    for oy in 0..ih {
        let py = ry + oy;
        if py >= bottom {
            break;
        }
        for ox in 0..cols {
            framebuffer::back_set_pixel(x + ox, py, px[(oy * iw + ox) as usize]);
        }
    }
}

/// Handle a click in the page content at `rel_y` pixels below the draw_page
/// origin: if it lands on a link line, follow that link. Returns true if a
/// link was followed. `rel_y` uses the same geometry draw_page lays out with
/// (an 18px status band, then 14px rows).
pub fn click_page(rel_y: i64, cols: usize) -> bool {
    if rel_y < 18 {
        return false;
    }
    let row = ((rel_y - 18) / 14) as usize;
    let idx = unsafe { SCROLL } + row;
    let p = page();
    if idx >= p.line_count {
        return false;
    }
    let link = p.lines[idx].link;
    if link != u8::MAX {
        return follow(link as usize + 1, cols);
    }
    false
}

/// Largest valid scroll offset: keep at least a couple of lines on screen.
fn scroll_max() -> usize {
    let vis = unsafe { VISIBLE_ROWS }.max(1);
    page().line_count.saturating_sub(vis.saturating_sub(2))
}

/// Scroll by `delta` lines (negative = up).
pub fn scroll(delta: i32) {
    unsafe {
        let s = (SCROLL as i64 + delta as i64).clamp(0, scroll_max() as i64);
        SCROLL = s as usize;
    }
}

/// Scroll by one viewport (dir -1 = up, +1 = down), keeping a couple of lines
/// of overlap for continuity.
pub fn scroll_page(dir: i32) {
    let step = (unsafe { VISIBLE_ROWS }.max(3) - 2) as i32;
    scroll(dir * step);
}

/// Jump to the top of the page.
pub fn scroll_home() {
    unsafe { SCROLL = 0 };
}

/// Jump to the bottom of the page.
pub fn scroll_end() {
    unsafe { SCROLL = scroll_max() };
}

fn line_color(kind: LineKind, link: u8, bold: bool) -> u32 {
    if link != u8::MAX {
        return theme::color(theme::SLOT_ACCENT);
    }
    match kind {
        LineKind::Heading1 | LineKind::Heading2 | LineKind::Heading3 => {
            theme::color(theme::SLOT_ACCENT)
        },
        LineKind::Pre => theme::color(theme::SLOT_DIM),
        _ => {
            if bold {
                theme::color(theme::SLOT_TEXT)
            } else {
                theme::color(theme::SLOT_TEXT)
            }
        },
    }
}

/// Render just the page body into a content rect (real pixels), starting
/// at `y`. The WM draws the URL/search bar above this itself (it owns the
/// editable input). `y` is already below that bar.
pub fn draw_page(x: u32, y: u32, w: u32, h: u32) {
    let dim = theme::color(theme::SLOT_DIM);
    let row_h = 14u32;
    let rows = (h.saturating_sub(20) / row_h) as usize;
    unsafe { VISIBLE_ROWS = rows };

    let p = page();
    // CSS page background (from a body/html rule), else the theme panel.
    let panel = if p.bg != NO_COLOR { p.bg } else { theme::color(theme::SLOT_PANEL) };
    if p.bg != NO_COLOR {
        framebuffer::back_fill_rect(x, y, w, h, panel);
    }

    font8x8::draw_str(x, y, status().as_bytes(), dim, panel);

    let scroll = unsafe { SCROLL };
    let mut link_counter = 0usize;
    // Count links appearing before the viewport so numbers stay stable.
    for li in p.lines[..scroll.min(p.line_count)].iter() {
        if li.link != u8::MAX {
            link_counter = link_counter.max(li.link as usize + 1);
        }
    }
    for r in 0..rows {
        let idx = scroll + r;
        if idx >= p.line_count {
            break;
        }
        let l = &p.lines[idx];
        let ry = y + 18 + r as u32 * row_h;
        let mut cx = x;
        // Per-line CSS background (element background), else the page bg.
        let lbg = if l.bg != NO_COLOR { l.bg } else { panel };
        if l.bg != NO_COLOR {
            framebuffer::back_fill_rect(x, ry.saturating_sub(1), w, row_h, lbg);
        }
        if l.kind == LineKind::Image {
            // Blit the decoded picture over its reserved rows, or show the alt
            // text if it isn't available (still loading elsewhere, or a format
            // with no decoder such as JPEG).
            if l.img != u8::MAX && unsafe { IMG_STATE[l.img as usize] } == IMG_READY {
                blit_image(l.img as usize, x, ry, w, y + h);
            } else {
                font8x8::draw_str(x, ry, l.text(), dim, lbg);
            }
            continue;
        }
        if l.kind == LineKind::ListItem {
            framebuffer::back_fill_circle(x + 3, ry + 4, 2, dim);
            cx += 12;
        }
        if l.link != u8::MAX && (l.link as usize) + 1 > link_counter {
            // First line of this link in the viewport: prefix its number.
            link_counter = l.link as usize + 1;
            let mut nb = [0u8; 5];
            let mut n = 0;
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
            font8x8::draw_str(cx, ry, &nb[..n], theme::color(theme::SLOT_ERROR), lbg);
            cx += n as u32 * 8 + 4;
        }
        // CSS color wins over the kind/link default when set. If a background
        // is set but no text color, pick black/white for legibility (a CSS
        // background must never leave text unreadable against it).
        let color = if l.color != NO_COLOR {
            l.color
        } else if l.bg != NO_COLOR {
            let (r, g, b) = ((l.bg >> 16) & 0xff, (l.bg >> 8) & 0xff, l.bg & 0xff);
            if (r * 30 + g * 59 + b * 11) / 100 > 140 {
                0x101014
            } else {
                0xf0f0f0
            }
        } else {
            line_color(l.kind, l.link, l.bold)
        };
        // Honor text-align (center/right) for plain lines with no link/list
        // prefix, where a shifted start column is unambiguous.
        let text_px = l.len as u32 * 8;
        if l.align != 0 && l.link == u8::MAX && l.kind != LineKind::ListItem && text_px < w {
            cx = match l.align {
                1 => x + (w - text_px) / 2, // center
                2 => x + (w - text_px),     // right
                _ => cx,
            };
        }
        font8x8::draw_str(cx, ry, l.text(), color, lbg);
        if matches!(l.kind, LineKind::Heading1) {
            // Underline h1 -- the one embellishment font8x8 can afford.
            framebuffer::back_fill_rect(cx, ry + 10, (l.len as u32) * 8, 1, color);
        }
    }
    if p.line_count == 0 {
        font8x8::draw_str(x, y + 22, b"bring: browser in ling", theme::color(theme::SLOT_TEXT), panel);
        font8x8::draw_str(x, y + 40, b"type a URL or search above and press Enter (http:// for now)", dim, panel);
    }
}

/// lynx-style console dump for `bring --browse` in the text shell.
pub fn dump_to_console() {
    let p = page();
    crate::console_write(b"--- ");
    crate::console_write(&p.title[..p.title_len]);
    crate::console_write(b" ---\n");
    for l in p.lines[..p.line_count].iter() {
        match l.kind {
            LineKind::Heading1 | LineKind::Heading2 | LineKind::Heading3 => {
                crate::console_write(b"# ");
            },
            LineKind::ListItem => crate::console_write(b" * "),
            _ => {},
        }
        crate::console_write(l.text());
        crate::console_write(b"\n");
    }
    if p.link_count > 0 {
        crate::console_write(b"--- links ---\n");
        for (i, l) in p.links[..p.link_count].iter().enumerate() {
            let mut nb = [b' '; 4];
            let d = i + 1;
            if d >= 10 {
                nb[0] = b'0' + (d / 10) as u8;
            }
            nb[1] = b'0' + (d % 10) as u8;
            nb[2] = b':';
            crate::console_write(&nb[..4]);
            crate::console_write(l.href());
            crate::console_write(b"\n");
        }
    }
}
