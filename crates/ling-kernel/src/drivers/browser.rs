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

use crate::drivers::{font8x8, framebuffer, netstack, theme};
use bring_browser::{layout, LineKind, Page, MAX_URL};

static mut PAGE: Page = Page::new();
static mut BODY: [u8; 120 * 1024] = [0; 120 * 1024];
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

pub fn go(url: &str, cols: usize) -> bool {
    let Some((host, port, path, tls)) = netstack::parse_url(url) else {
        unsafe { STATUS = "bad URL (want http://host[:port]/path)" };
        return false;
    };
    loading_toast(url);
    let body = unsafe { &mut *&raw mut BODY };
    let mut body_off = 0usize;
    let len = if tls {
        // HTTPS via the in-kernel TLS 1.3 client. Note: certificate validation
        // isn't done yet -- encrypted but not authenticated. https_get returns
        // the full response, so strip the HTTP headers before layout.
        let hport = if port == 0 { 443 } else { port };
        let mut noop = |_: &[u8]| {};
        match crate::tls::https_get(host, hport, path, body, &mut noop) {
            Ok(n) if n > 0 => {
                body_off = crate::tls::http_body_offset(&body[..n]);
                n
            },
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
        match netstack::http_get(ip, port, path, host, body) {
            Some(n) => n,
            None => {
                unsafe { STATUS = "fetch failed (connect refused, non-200, or timeout)" };
                return false;
            },
        }
    };
    layout(&body[body_off..len], cols, page());
    unsafe {
        SCROLL = 0;
        STATUS = if page().truncated {
            "loaded (truncated: page bigger than the line buffer)"
        } else {
            "loaded"
        };
    }
    set_url(url);
    true
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
pub fn draw_page(x: u32, y: u32, _w: u32, h: u32) {
    let panel = theme::color(theme::SLOT_PANEL);
    let dim = theme::color(theme::SLOT_DIM);
    let row_h = 14u32;
    let rows = (h.saturating_sub(20) / row_h) as usize;
    unsafe { VISIBLE_ROWS = rows };

    font8x8::draw_str(x, y, status().as_bytes(), dim, panel);

    let p = page();
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
            font8x8::draw_str(cx, ry, &nb[..n], theme::color(theme::SLOT_ERROR), panel);
            cx += n as u32 * 8 + 4;
        }
        let color = line_color(l.kind, l.link, l.bold);
        font8x8::draw_str(cx, ry, l.text(), color, panel);
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
