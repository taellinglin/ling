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
static mut STATUS: &'static str = "press u for a URL, 1-9 follow links, arrows scroll";

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
    if tls {
        unsafe {
            STATUS = "https needs a TLS stack LingOS doesn't have yet -- try http://";
        }
        return false;
    }
    let Some(ip) = netstack::dns_resolve(host) else {
        unsafe { STATUS = "DNS: no address for that host" };
        return false;
    };
    let body = unsafe { &mut *&raw mut BODY };
    let Some(len) = netstack::http_get(ip, port, path, host, body) else {
        unsafe { STATUS = "fetch failed (connect refused, non-200, or timeout)" };
        return false;
    };
    layout(&body[..len], cols, page());
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

/// Follow link number `n` (1-based, as displayed). Root-relative and
/// absolute http URLs work; https and protocol-relative are refused with
/// a status message.
pub fn follow(n: usize, cols: usize) -> bool {
    let p = page();
    if n == 0 || n > p.link_count {
        return false;
    }
    let href = p.links[n - 1];
    let href = core::str::from_utf8(href.href()).unwrap_or("");
    let mut url_buf = [0u8; MAX_URL * 2];
    let target: &str = if href.starts_with("http://") || href.starts_with("https://") {
        href
    } else if href.starts_with('/') {
        // Same host: splice scheme+host[:port] from the current URL.
        let cur = current_url();
        let after_scheme = cur.strip_prefix("http://").unwrap_or(cur);
        let host_end = after_scheme.find('/').unwrap_or(after_scheme.len());
        let prefix_len = 7 + host_end;
        let total = prefix_len + href.len();
        if total > url_buf.len() {
            return false;
        }
        url_buf[..prefix_len].copy_from_slice(&cur.as_bytes()[..prefix_len]);
        url_buf[prefix_len..total].copy_from_slice(href.as_bytes());
        core::str::from_utf8(&url_buf[..total]).unwrap_or("")
    } else {
        unsafe { STATUS = "relative links beyond '/' aren't resolved yet" };
        return false;
    };
    go(target, cols)
}

pub fn scroll(delta: i32, visible_rows: usize) {
    unsafe {
        let max = page().line_count.saturating_sub(visible_rows / 2);
        let s = SCROLL as i64 + delta as i64;
        SCROLL = s.clamp(0, max as i64) as usize;
    }
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

/// Render the page into a window's content rect (real pixels). The WM
/// calls this for KIND_WEB windows after drawing the chrome.
pub fn draw(x: u32, y: u32, w: u32, h: u32) {
    let panel = theme::color(theme::SLOT_PANEL);
    let dim = theme::color(theme::SLOT_DIM);
    let row_h = 14u32;
    let rows = (h.saturating_sub(40) / row_h) as usize;

    // URL + status header.
    font8x8::draw_str(x, y, current_url().as_bytes(), theme::color(theme::SLOT_ACCENT), panel);
    font8x8::draw_str(x, y + 16, status().as_bytes(), dim, panel);

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
        let ry = y + 36 + r as u32 * row_h;
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
        font8x8::draw_str(x, y + 40, b"bring: browser in ling", theme::color(theme::SLOT_TEXT), panel);
        font8x8::draw_str(x, y + 58, b"press u, then type a URL (http:// only for now)", dim, panel);
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
