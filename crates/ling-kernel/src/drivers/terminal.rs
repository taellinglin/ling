//! A desktop terminal window: a colored, themed, zoomable shell. The grid
//! resolution is derived from the window size divided by the zoomed glyph
//! size, so Ctrl +/- literally changes how many characters fit -- "text
//! resolution by text-size zoom", as asked.
//!
//! The shell here is kernel-side and independent of `lsh` (which is a
//! separate `.ling` program compiled into the text kernel, not this
//! graphics one). It supports a useful built-in command set wired
//! straight to kernel services -- lingfs (`ls`/`cat`), the network
//! (`curl`, `dns`), the clock (`date`), theming (`theme`), clipboard
//! (`copy`/`paste`) -- rather than pretending to run arbitrary programs
//! (there's no process model here; disclosed). Output lines carry a color
//! class so errors read red, ok green, and command echoes accent.

use crate::arch::rtc;
use crate::drivers::{clipboard, font8x8, framebuffer, netstack, theme};
use crate::fs::lingfs;

const COLS: usize = 128; // max stored chars per line
const LINES: usize = 240; // scrollback ring
const INPUT_MAX: usize = 200;

#[derive(Clone, Copy, PartialEq)]
enum Cls {
    Normal,
    Accent,
    Ok,
    Err,
    Dim,
}

#[derive(Clone, Copy)]
struct Row {
    text: [u8; COLS],
    len: usize,
    cls: Cls,
}
const EMPTY_ROW: Row = Row { text: [0; COLS], len: 0, cls: Cls::Normal };

static mut ROWS: [Row; LINES] = [EMPTY_ROW; LINES];
static mut HEAD: usize = 0; // next write index (ring)
static mut COUNT: usize = 0; // rows written (<= LINES)
static mut INPUT: [u8; INPUT_MAX] = [0; INPUT_MAX];
static mut INPUT_LEN: usize = 0;
static mut ZOOM: u32 = 1;
static mut SCROLL: usize = 0; // rows scrolled up from the bottom
static mut STARTED: bool = false;

fn rows() -> &'static mut [Row; LINES] {
    unsafe { &mut *&raw mut ROWS }
}

fn push(cls: Cls, s: &[u8]) {
    unsafe {
        let r = &mut rows()[HEAD];
        let n = s.len().min(COLS);
        r.text[..n].copy_from_slice(&s[..n]);
        r.len = n;
        r.cls = cls;
        HEAD = (HEAD + 1) % LINES;
        if COUNT < LINES {
            COUNT += 1;
        }
        SCROLL = 0; // jump to bottom on new output
    }
}

/// Append possibly-multi-line bytes, splitting on '\n' and wrapping long
/// lines at COLS -- the shape terminal output actually arrives in.
fn push_wrapped(cls: Cls, bytes: &[u8]) {
    let mut line_start = 0;
    for i in 0..=bytes.len() {
        let at_nl = i < bytes.len() && bytes[i] == b'\n';
        let at_end = i == bytes.len();
        if at_nl || at_end || i - line_start >= COLS {
            push(cls, &bytes[line_start..i]);
            line_start = if at_nl { i + 1 } else { i };
            if at_end {
                break;
            }
        }
    }
}

fn banner() {
    push(Cls::Accent, b"LingOS terminal -- type 'help'");
    push(Cls::Dim, b"a graphics-mode shell (separate from the text-mode lsh)");
}

fn prompt_echo() {
    unsafe {
        let mut line = [0u8; INPUT_MAX + 8];
        line[0] = b'>';
        line[1] = b' ';
        let n = INPUT_LEN.min(INPUT_MAX);
        line[2..2 + n].copy_from_slice(&(&*&raw const INPUT)[..n]);
        push(Cls::Accent, &line[..2 + n]);
    }
}

fn input_str() -> &'static str {
    unsafe { core::str::from_utf8(&(&*&raw const INPUT)[..INPUT_LEN]).unwrap_or("") }
}

fn split_cmd(s: &str) -> (&str, &str) {
    match s.find(' ') {
        Some(i) => (&s[..i], s[i + 1..].trim_start()),
        None => (s, ""),
    }
}

fn run(line: &str) {
    let (cmd, arg) = split_cmd(line.trim());
    match cmd {
        "" => {},
        "help" => {
            push(Cls::Normal, b"commands:");
            push(Cls::Normal, b"  help  clear  echo <t>  ls [dir]  cat <file>");
            push(Cls::Normal, b"  date  theme dark|light  copy <t>  paste");
            push(Cls::Normal, b"  dns <host>   curl <url>   bring <url>");
        },
        "clear" => unsafe {
            COUNT = 0;
            HEAD = 0;
            SCROLL = 0;
        },
        "echo" => push(Cls::Normal, arg.as_bytes()),
        "date" => {
            let dt = rtc::read();
            let mut b = [0u8; 32];
            let mut n = 0;
            n += w4(&mut b[n..], dt.year as u32);
            b[n] = b'-'; n += 1;
            n += w2(&mut b[n..], dt.month as u32);
            b[n] = b'-'; n += 1;
            n += w2(&mut b[n..], dt.day as u32);
            b[n] = b' '; n += 1;
            n += w2(&mut b[n..], dt.hour as u32);
            b[n] = b':'; n += 1;
            n += w2(&mut b[n..], dt.minute as u32);
            b[n] = b':'; n += 1;
            n += w2(&mut b[n..], dt.second as u32);
            push(Cls::Normal, &b[..n]);
            push(Cls::Dim, b"(CMOS RTC, UTC)");
        },
        "theme" => {
            if arg == "light" {
                theme::set(1);
            } else if arg == "dark" {
                theme::set(0);
            } else {
                push(Cls::Err, b"usage: theme dark|light");
            }
        },
        "copy" => {
            clipboard::set(arg.as_bytes());
            push(Cls::Ok, b"copied to clipboard");
        },
        "paste" => push(Cls::Normal, clipboard::get()),
        "ls" => {
            let mut idx = 0;
            let mut nm = [0u8; 64];
            let mut any = false;
            while idx < 128 {
                match lingfs::list_entry(arg, idx, &mut nm) {
                    Some((len, is_dir)) => {
                        let mut line = [0u8; 70];
                        let mut n = 0;
                        line[..len].copy_from_slice(&nm[..len]);
                        n += len;
                        if is_dir {
                            line[n] = b'/';
                            n += 1;
                        }
                        push(if is_dir { Cls::Accent } else { Cls::Normal }, &line[..n]);
                        any = true;
                        idx += 1;
                    },
                    None => break,
                }
            }
            if !any {
                push(Cls::Dim, b"(empty)");
            }
        },
        "cat" => {
            if arg.is_empty() {
                push(Cls::Err, b"usage: cat <file>");
            } else {
                static mut CATBUF: [u8; 8192] = [0; 8192];
                let cb = unsafe { &mut *&raw mut CATBUF };
                match lingfs::read_file_all(arg, cb) {
                    Ok(Some(len)) => push_wrapped(Cls::Normal, &cb[..len]),
                    _ => push(Cls::Err, b"cat: no such file"),
                }
            }
        },
        "dns" => {
            if arg.is_empty() {
                push(Cls::Err, b"usage: dns <host>");
            } else {
                push(Cls::Dim, b"resolving...");
                match netstack::dns_resolve(arg) {
                    Some(ip) => {
                        let mut b = [0u8; 32];
                        let mut n = 0;
                        for (k, oct) in ip.iter().enumerate() {
                            if k > 0 {
                                b[n] = b'.';
                                n += 1;
                            }
                            n += w_any(&mut b[n..], *oct as u32);
                        }
                        push(Cls::Ok, &b[..n]);
                    },
                    None => push(Cls::Err, b"dns: could not resolve"),
                }
            }
        },
        "curl" => {
            if arg.is_empty() {
                push(Cls::Err, b"usage: curl <url>");
            } else {
                fetch_and_dump(arg, false);
            }
        },
        "bring" => {
            if arg.is_empty() {
                push(Cls::Err, b"usage: bring <url>");
            } else {
                fetch_and_dump(arg, true);
            }
        },
        _ => {
            let mut b = [0u8; 96];
            let p = b"unknown command: ";
            b[..p.len()].copy_from_slice(p);
            let n = (cmd.len()).min(b.len() - p.len());
            b[p.len()..p.len() + n].copy_from_slice(&cmd.as_bytes()[..n]);
            push(Cls::Err, &b[..p.len() + n]);
        },
    }
}

fn fetch_and_dump(url: &str, render: bool) {
    let Some((host, port, path, tls)) = netstack::parse_url(url) else {
        push(Cls::Err, b"bad URL");
        return;
    };
    if tls {
        push(Cls::Err, b"https not supported yet (http:// only)");
        return;
    }
    let Some(ip) = netstack::dns_resolve(host) else {
        push(Cls::Err, b"could not resolve host");
        return;
    };
    push(Cls::Dim, b"fetching...");
    static mut WEBBUF: [u8; 32 * 1024] = [0; 32 * 1024];
    let wb = unsafe { &mut *&raw mut WEBBUF };
    match netstack::http_get(ip, port, path, host, wb) {
        Some(len) => {
            if render {
                // Render via the bring engine into a small page, then dump
                // its text lines (headings/links flattened).
                use bring_browser::{layout, Page};
                static mut PG: Page = Page::new();
                let pg = unsafe { &mut *&raw mut PG };
                layout(&wb[..len], COLS - 2, pg);
                for l in pg.lines[..pg.line_count].iter() {
                    let cls = if l.link != u8::MAX { Cls::Accent } else { Cls::Normal };
                    push(cls, l.text());
                }
            } else {
                push_wrapped(Cls::Normal, &wb[..len]);
            }
        },
        None => push(Cls::Err, b"fetch failed (connect/non-200/timeout)"),
    }
}

fn w2(buf: &mut [u8], v: u32) -> usize {
    buf[0] = b'0' + ((v / 10) % 10) as u8;
    buf[1] = b'0' + (v % 10) as u8;
    2
}
fn w4(buf: &mut [u8], v: u32) -> usize {
    buf[0] = b'0' + ((v / 1000) % 10) as u8;
    buf[1] = b'0' + ((v / 100) % 10) as u8;
    buf[2] = b'0' + ((v / 10) % 10) as u8;
    buf[3] = b'0' + (v % 10) as u8;
    4
}
fn w_any(buf: &mut [u8], mut v: u32) -> usize {
    if v == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut t = [0u8; 10];
    let mut k = 0;
    while v > 0 {
        t[k] = b'0' + (v % 10) as u8;
        v /= 10;
        k += 1;
    }
    for i in 0..k {
        buf[i] = t[k - 1 - i];
    }
    k
}

pub fn key(k: u8) {
    use crate::drivers::keyboard as kb;
    unsafe {
        if !STARTED {
            STARTED = true;
            banner();
        }
        match k {
            kb::CTRL_ZOOM_IN => ZOOM = (ZOOM + 1).min(3),
            kb::CTRL_ZOOM_OUT => ZOOM = (ZOOM - 1).max(1),
            kb::CTRL_ZOOM_RESET => ZOOM = 1,
            kb::CTRL_C => {
                // Copy the last output line to the clipboard.
                if COUNT > 0 {
                    let last = (HEAD + LINES - 1) % LINES;
                    clipboard::set(&rows()[last].text[..rows()[last].len]);
                }
            },
            kb::CTRL_V => {
                let clip = clipboard::get();
                for &b in clip {
                    if b >= 0x20 && b < 0x7F && INPUT_LEN < INPUT_MAX {
                        INPUT[INPUT_LEN] = b;
                        INPUT_LEN += 1;
                    }
                }
            },
            kb::UP_ARROW => SCROLL = (SCROLL + 1).min(COUNT),
            kb::DOWN_ARROW => SCROLL = SCROLL.saturating_sub(1),
            b'\n' | b'\r' => {
                prompt_echo();
                let mut tmp = [0u8; INPUT_MAX];
                let n = INPUT_LEN;
                tmp[..n].copy_from_slice(&(&*&raw const INPUT)[..n]);
                INPUT_LEN = 0;
                run(core::str::from_utf8(&tmp[..n]).unwrap_or(""));
            },
            0x08 => {
                if INPUT_LEN > 0 {
                    INPUT_LEN -= 1;
                }
            },
            0x20..=0x7E => {
                if INPUT_LEN < INPUT_MAX {
                    INPUT[INPUT_LEN] = k;
                    INPUT_LEN += 1;
                }
            },
            _ => {},
        }
    }
}

fn cls_color(c: Cls) -> u32 {
    match c {
        Cls::Accent => theme::color(theme::SLOT_ACCENT),
        Cls::Ok => 0x6AC06A,
        Cls::Err => theme::color(theme::SLOT_ERROR),
        Cls::Dim => theme::color(theme::SLOT_DIM),
        Cls::Normal => theme::color(theme::SLOT_TEXT),
    }
}

/// Draw the terminal into a window content rect. Grid geometry follows the
/// zoom: bigger glyphs => fewer rows/cols => lower text resolution.
pub fn draw(x: u32, y: u32, w: u32, h: u32) {
    let panel = theme::color(theme::SLOT_PANEL);
    unsafe {
        if !STARTED {
            STARTED = true;
            banner();
        }
    }
    let z = unsafe { ZOOM };
    let gw = 8 * z;
    let gh = 8 * z + 2 * z;
    let cols = ((w / gw).max(1)) as usize;
    // Reserve the last grid row for the input prompt.
    let grid_rows = (h / gh).max(2) as usize;
    let out_rows = grid_rows - 1;

    let count = unsafe { COUNT };
    let head = unsafe { HEAD };
    let scroll = unsafe { SCROLL }.min(count.saturating_sub(1));
    // Bottom visible row is (count-1-scroll); show `out_rows` up from there.
    let last = count.saturating_sub(1).saturating_sub(scroll);
    let first = last.saturating_sub(out_rows - 1);
    let mut ry = y;
    for vis in first..=last {
        if count == 0 {
            break;
        }
        // ring index of logical row `vis` (0 = oldest kept)
        let ring = (head + LINES - count + vis) % LINES;
        let r = &rows()[ring];
        let color = cls_color(r.cls);
        let n = r.len.min(cols);
        for (ci, &ch) in r.text[..n].iter().enumerate() {
            font8x8::draw_char_scaled(x + ci as u32 * gw, ry, ch, color, panel, z);
        }
        ry += gh;
    }

    // Input prompt on the bottom row.
    let py = y + (grid_rows as u32 - 1) * gh;
    let accent = theme::color(theme::SLOT_ACCENT);
    font8x8::draw_char_scaled(x, py, b'>', accent, panel, z);
    let inp = unsafe { &(&*&raw const INPUT)[..INPUT_LEN] };
    let shown = inp.len().min(cols.saturating_sub(3));
    let start = inp.len() - shown;
    for (ci, &ch) in inp[start..].iter().enumerate() {
        font8x8::draw_char_scaled(x + (ci as u32 + 2) * gw, py, ch, theme::color(theme::SLOT_TEXT), panel, z);
    }
    // Caret.
    framebuffer::back_fill_rect(x + (shown as u32 + 2) * gw, py, gw / 4 + 1, gh, accent);
    let _ = input_str;
}
