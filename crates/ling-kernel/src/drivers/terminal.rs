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
use crate::drivers::{clipboard, font8x8, framebuffer, lingfu, llm, media, mixer, netstack, theme};
use crate::fs::{lingfs, users};
use alloc::vec::Vec;

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

// ── Terminal theme: the LingStationX look, customizable ─────────────────────
// This window's own text / background / accent colors and font size,
// independent of the global theme: `term <preset>`, `term text|back|accent
// <color>`, `term font <1|2|3>` -- persisted to lingfs `/termtheme` so the
// choice survives a reboot. The "dusk" preset (the default) follows the
// global theme slots, exactly as before.
const TERM_PRESETS: [(&str, u32, u32, u32); 8] = [
    ("dusk", 0, 0, 0), // follows the global theme (text / panel / accent)
    ("ink", 0xC8D0E0, 0x0B0B10, 0x3BAAEB),
    ("paper", 0x20242C, 0xF2EFE6, 0x1F6FD6),
    ("rose", 0xF7D9DD, 0x160A0E, 0xED4A37),
    ("ember", 0xFFD9C0, 0x160D06, 0xF7941E),
    ("gold", 0xF7ECC0, 0x141006, 0xD1B600),
    ("moss", 0xCDEBD3, 0x06120A, 0x45C870),
    ("iris", 0xE2D9F7, 0x0E0A16, 0x9A53D6),
];
/// ROYGBIV, the design system's key palette (theme.ling's C_RED..C_VIOLET).
const ROYGBIV: [u32; 7] = [0xED4A37, 0xF7941E, 0xF7D846, 0x45C870, 0x3BAAEB, 0x4A64D8, 0x9A53D6];
/// Resolved (text, back, accent) when customized; None = follow the global theme.
static mut THEME_CUSTOM: Option<(u32, u32, u32)> = None;
static mut THEME_LOADED: bool = false;

// The prompt line's cached pieces (reading lingfs every frame would be
// wasteful): refreshed when the terminal starts and after every command.
static mut HOSTNAME: [u8; 32] = [0; 32];
static mut HOSTNAME_LEN: usize = 0;
static mut MIAO_SOUL: [u8; 24] = [0; 24];
static mut MIAO_SOUL_LEN: usize = 0;
static mut MIAO_DIRTY: bool = false; // offerings sit on the altar

// Command history (up/down recall), like a real shell.
const HIST_N: usize = 32;
static mut HIST: [[u8; INPUT_MAX]; HIST_N] = [[0; INPUT_MAX]; HIST_N];
static mut HIST_LEN: [usize; HIST_N] = [0; HIST_N];
static mut HIST_HEAD: usize = 0; // next write slot (ring)
static mut HIST_COUNT: usize = 0; // entries stored (<= HIST_N)
static mut HIST_NAV: usize = 0; // steps back currently recalled (0 = fresh line)

// Current working directory: a single lingfs dir name (empty = root). lingfs
// is one level deep, so this is at most one component.
static mut CWD: [u8; 64] = [0; 64];
static mut CWD_LEN: usize = 0;

// sudo/su password entry: 0 = normal input, 1 = collecting a sudo password,
// 2 = collecting an su password. PENDING holds the sudo command line (mode 1)
// or the target username (mode 2).
static mut PW_MODE: u8 = 0;
static mut PW_INPUT: [u8; INPUT_MAX] = [0; INPUT_MAX];
static mut PW_LEN: usize = 0;
static mut PENDING: [u8; INPUT_MAX] = [0; INPUT_MAX];
static mut PENDING_LEN: usize = 0;

fn rows() -> &'static mut [Row; LINES] {
    unsafe { &mut *&raw mut ROWS }
}

/// Integer blend: `a` toward `b` by num/den per channel.
fn mix(a: u32, b: u32, num: u32, den: u32) -> u32 {
    let ch = |sa: u32, sb: u32| {
        let (ca, cb) = ((a >> sa) & 0xFF, (b >> sa) & 0xFF);
        let _ = sb;
        (ca * (den - num) + cb * num) / den
    };
    (ch(16, 16) << 16) | (ch(8, 8) << 8) | ch(0, 0)
}

/// Is this background light? (for ok/err contrast on e.g. the paper preset)
fn is_light(back: u32) -> bool {
    let (r, g, b) = ((back >> 16) & 0xFF, (back >> 8) & 0xFF, back & 0xFF);
    2 * r + 3 * g + b > 3 * 255
}

/// The terminal's resolved (text, back, accent).
fn theme_colors() -> (u32, u32, u32) {
    match unsafe { THEME_CUSTOM } {
        Some(t) => t,
        None => (theme::color(theme::SLOT_TEXT), theme::color(theme::SLOT_PANEL), theme::color(theme::SLOT_ACCENT)),
    }
}

/// "RRGGBB" / "#RRGGBB" / a ROYGBIV name / "white" / "black".
fn parse_color(s: &str) -> Option<u32> {
    let names = ["red", "orange", "yellow", "green", "blue", "indigo", "violet"];
    if let Some(i) = names.iter().position(|n| *n == s) {
        return Some(ROYGBIV[i]);
    }
    match s {
        "white" => return Some(0xF5F5F5),
        "black" => return Some(0x0B0B10),
        "grey" | "gray" => return Some(0x8C9098),
        _ => {},
    }
    let h = s.trim_start_matches('#');
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(h, 16).ok()
}

/// Persist the theme + font to lingfs `/termtheme` (absolute: see services.rs
/// for why a bare name would scatter under the CWD).
fn save_theme() {
    let mut text = alloc::string::String::new();
    if let Some((t, b, a)) = unsafe { THEME_CUSTOM } {
        text.push_str(&alloc::format!("text {t:06X}\nback {b:06X}\naccent {a:06X}\n"));
    }
    text.push_str(&alloc::format!("font {}\n", unsafe { ZOOM }));
    let _ = lingfs::write_file_any("/termtheme", text.as_bytes());
}

fn load_theme() {
    unsafe {
        if THEME_LOADED {
            return;
        }
        THEME_LOADED = true;
    }
    let mut buf = [0u8; 256];
    let Ok(Some(n)) = lingfs::read_file_all("/termtheme", &mut buf) else { return };
    let (mut t, mut b, mut a) = (None, None, None);
    for line in core::str::from_utf8(&buf[..n]).unwrap_or("").lines() {
        let mut f = line.split_whitespace();
        match (f.next(), f.next()) {
            (Some("text"), Some(v)) => t = parse_color(v),
            (Some("back"), Some(v)) => b = parse_color(v),
            (Some("accent"), Some(v)) => a = parse_color(v),
            (Some("font"), Some(v)) => unsafe { ZOOM = v.parse::<u32>().unwrap_or(1).clamp(1, 3) },
            _ => {},
        }
    }
    if let (Some(t), Some(b), Some(a)) = (t, b, a) {
        unsafe { THEME_CUSTOM = Some((t, b, a)) };
    }
}

/// Refresh the prompt's cached pieces: hostname, and -- when the CWD holds a
/// .miao shrine -- the living soul and whether offerings sit on the altar
/// (the way zsh's git prompt shows the branch and a dirty mark).
fn update_prompt_info() {
    let mut buf = [0u8; 256];
    unsafe {
        HOSTNAME_LEN = 0;
        if let Ok(Some(n)) = lingfs::read_file_all("/hostname", &mut buf) {
            let s = core::str::from_utf8(&buf[..n]).unwrap_or("").trim();
            let k = s.len().min(HOSTNAME.len());
            HOSTNAME[..k].copy_from_slice(&s.as_bytes()[..k]);
            HOSTNAME_LEN = k;
        }
        MIAO_SOUL_LEN = 0;
        MIAO_DIRTY = false;
    }
    // Read the shrine through vfs (card then lingfs): with a mounted data
    // volume, home folders live there, so a raw lingfs read would miss it.
    let cwd = cwd_str();
    let soul_path = if cwd.is_empty() { alloc::string::String::from(".miao/SOUL") } else { alloc::format!("{cwd}/.miao/SOUL") };
    if let Some(bytes) = crate::vfs::read(&soul_path) {
        // "soul: main" -- the word after the first space
        let s = core::str::from_utf8(&bytes).unwrap_or("");
        let soul = s.split_whitespace().nth(1).unwrap_or("main").trim();
        unsafe {
            let k = soul.len().min(MIAO_SOUL.len());
            MIAO_SOUL[..k].copy_from_slice(&soul.as_bytes()[..k]);
            MIAO_SOUL_LEN = k;
        }
        let altar = if cwd.is_empty() { alloc::string::String::from(".miao/altar") } else { alloc::format!("{cwd}/.miao/altar") };
        if crate::vfs::read(&altar).is_some_and(|a| !a.is_empty()) {
            unsafe { MIAO_DIRTY = true };
        }
    }
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
        TOTAL += 1;
        SCROLL = 0; // jump to bottom on new output
    }
}

// ── Mouse text selection (drag over the output; Ctrl+C / right-click copy) ──
// Positions are (absolute row number, column): absolute = counted since the
// terminal started, so new output scrolling the ring does not move a
// selection; a row that fell out of the scrollback just drops out of it.
static mut TOTAL: u64 = 0; // rows ever pushed
static mut SEL_A: Option<(u64, usize)> = None; // anchor (where the drag began)
static mut SEL_B: Option<(u64, usize)> = None; // the other end
static mut SEL_DRAG: bool = false;
/// The last draw's grid: (x, y, glyph w, row h, cols, out rows, first abs row shown).
static mut GRID: (i64, i64, i64, i64, usize, usize, u64) = (0, 0, 8, 10, 1, 1, 0);

/// The ring slot of absolute row `abs`, if still in the scrollback.
fn abs_row(abs: u64) -> Option<&'static Row> {
    unsafe {
        let oldest = TOTAL - COUNT as u64;
        if abs < oldest || abs >= TOTAL {
            return None;
        }
        let back = (TOTAL - abs) as usize; // 1 = newest
        Some(&rows()[(HEAD + LINES - back) % LINES])
    }
}

/// Mouse position -> (absolute row, column) on the output grid, clamped.
fn cell_at(mx: i64, my: i64) -> Option<(u64, usize)> {
    let (gx, gy, gw, gh, cols, out_rows, first) = unsafe { GRID };
    if unsafe { COUNT } == 0 {
        return None;
    }
    let r = ((my - gy).max(0) / gh.max(1)).min(out_rows as i64 - 1).max(0) as u64;
    let c = ((mx - gx).max(0) / gw.max(1)).min(cols as i64) as usize;
    let abs = (first + r).min(unsafe { TOTAL } - 1);
    Some((abs, c))
}

/// The selection in order: ((row, col) start, (row, col) end exclusive col).
fn sel_range() -> Option<((u64, usize), (u64, usize))> {
    let (a, b) = unsafe { (SEL_A?, SEL_B?) };
    if a == b {
        return None;
    }
    Some(if a <= b { (a, b) } else { (b, a) })
}

/// Columns [from, to) of absolute row `abs` that the selection covers.
fn sel_cols(abs: u64, len: usize) -> Option<(usize, usize)> {
    let ((r0, c0), (r1, c1)) = sel_range()?;
    if abs < r0 || abs > r1 {
        return None;
    }
    let from = if abs == r0 { c0 } else { 0 };
    let to = if abs == r1 { c1 } else { usize::MAX };
    // whole-row spans past the text still mark the row (an empty line copies as "")
    let to = to.min(len.max(from));
    Some((from.min(len), to))
}

/// The selected text, rows joined with '\n' (trailing spaces trimmed).
fn sel_text() -> Option<Vec<u8>> {
    let ((r0, _), (r1, _)) = sel_range()?;
    let mut out = Vec::new();
    for abs in r0..=r1 {
        let Some(row) = abs_row(abs) else { continue };
        if let Some((f, t)) = sel_cols(abs, row.len) {
            let mut s = &row.text[f..t.max(f)];
            while let [rest @ .., b' '] = s {
                s = rest;
            }
            out.extend_from_slice(s);
        }
        if abs != r1 {
            out.push(b'\n');
        }
    }
    Some(out)
}

fn sel_clear() {
    unsafe {
        SEL_A = None;
        SEL_B = None;
        SEL_DRAG = false;
    }
}

/// Copy the selection to the clipboard (and clear it); false when none.
fn copy_selection() -> bool {
    match sel_text() {
        Some(t) => {
            clipboard::set(&t);
            sel_clear();
            true
        },
        None => false,
    }
}

/// Paste the clipboard into the command line (one line: newlines become spaces).
fn paste() {
    let clip = clipboard::get();
    unsafe {
        if PW_MODE != 0 {
            return;
        }
        for &b in clip {
            let b = if b == b'\n' || b == b'\r' || b == b'\t' { b' ' } else { b };
            if b >= 0x20 && INPUT_LEN < INPUT_MAX {
                INPUT[INPUT_LEN] = b;
                INPUT_LEN += 1;
            }
        }
    }
}

// ── Tab completion: commands, their sub-commands, files and folders ─────────
const COMMANDS: &[&str] = &[
    "2fa", "ask", "bench3d", "cat", "cd", "clear", "copy", "cores", "cp", "cpu", "cryptotest", "curl", "date",
    "dns", "echo", "gpu", "help", "https", "imgview", "install-disk", "ling", "lingfu", "llm", "ls", "miao", "mkdir", "mv", "nic",
    "nvme", "paste", "prof", "pwd", "rename-user", "report", "rm", "services", "ssh", "ssh-serve", "su", "sudo", "term", "theme",
    "update-boot", "whoami", "wifi-intel",
];
const SUBCOMMANDS: &[(&str, &[&str])] = &[
    ("lingfu", &["install", "search", "list", "update", "upgrade", "remove", "sync", "info"]),
    ("term", &["dusk", "ink", "paper", "rose", "ember", "gold", "moss", "iris", "text", "back", "accent", "font"]),
    ("ssh-serve", &["on", "off"]),
    ("llm", &["status", "load", "stop"]),
    ("wifi-intel", &["up", "regs"]),
    ("ling", &["run", "eval", "demo"]),
    ("2fa", &["on", "off", "status"]),
    ("imgview", &["demo", "demosvg"]),
];

/// Entries of a terminal-relative directory: (name, is_dir). `dir` is the
/// typed folder part ("" = the working directory; a leading '/' = lingfs root).
fn complete_entries(dir: &str) -> Vec<(alloc::string::String, bool)> {
    let path = if let Some(abs) = dir.strip_prefix('/') {
        crate::vfs::norm(abs)
    } else if dir == "media/" || dir.starts_with("media/") {
        // like `ls`: media/... is always the mounted volumes, whatever the cwd
        crate::vfs::norm(dir)
    } else {
        crate::vfs::norm(&alloc::format!("{}/{dir}", cwd_str()))
    };
    let mut out = Vec::new();
    if crate::fs::media::is_media_path(&path) {
        for (name, is_dir, _) in crate::vfs::list(&path) {
            out.push((name, is_dir));
        }
        return out;
    }
    if path.is_empty() && !crate::fs::media::names().is_empty() {
        out.push((alloc::string::String::from("media"), true));
    }
    let mut nm = [0u8; 64];
    for idx in 0..512 {
        match lingfs::list_entry(&path, idx, &mut nm) {
            Some((len, is_dir)) => {
                out.push((alloc::string::String::from(core::str::from_utf8(&nm[..len]).unwrap_or("")), is_dir));
            },
            None => break,
        }
    }
    out
}

fn common_prefix<'a>(items: &[&'a str]) -> &'a str {
    let Some(first) = items.first() else { return "" };
    let mut n = first.len();
    for s in &items[1..] {
        n = n.min(first.bytes().zip(s.bytes()).take_while(|(a, b)| a == b).count());
    }
    &first[..n]
}

/// Tab: complete the word before the cursor. One match completes it (a
/// space after commands / files, a '/' after folders); several complete as far
/// as they agree and, when that adds nothing, are listed above the prompt.
fn tab_complete() {
    let line = unsafe { alloc::string::String::from_utf8_lossy(&(&*&raw const INPUT)[..INPUT_LEN]).into_owned() };
    let word_start = line.rfind(' ').map(|i| i + 1).unwrap_or(0);
    let word = &line[word_start..];
    let before: Vec<&str> = line[..word_start].split_whitespace().collect();
    // (candidate full words, is_dir flags); `base` = the part of `word` kept in front
    let mut cands: Vec<(alloc::string::String, bool)> = Vec::new();
    let mut base = "";
    if before.is_empty() {
        for c in COMMANDS {
            if c.starts_with(word) {
                cands.push((alloc::string::String::from(*c), false));
            }
        }
    } else {
        if before.len() == 1 {
            if let Some((_, subs)) = SUBCOMMANDS.iter().find(|(c, _)| *c == before[0]) {
                for s in subs.iter() {
                    if s.starts_with(word) {
                        cands.push((alloc::string::String::from(*s), false));
                    }
                }
            }
        }
        let (dir, prefix) = match word.rfind('/') {
            Some(i) => (&word[..i + 1], &word[i + 1..]),
            None => ("", word),
        };
        base = dir;
        let dirs_only = before[0] == "cd";
        for (name, is_dir) in complete_entries(dir) {
            if name.starts_with(prefix) && (!dirs_only || is_dir) && (prefix.starts_with('.') || !name.starts_with('.')) {
                cands.push((name, is_dir));
            }
        }
    }
    cands.sort();
    cands.dedup();
    if cands.is_empty() {
        return;
    }
    let typed = &word[base.len().min(word.len())..];
    let (insert, suffix) = if cands.len() == 1 {
        let (c, d) = &cands[0];
        (c.clone(), if *d { "/" } else { " " })
    } else {
        let names: Vec<&str> = cands.iter().map(|(c, _)| c.as_str()).collect();
        let p = common_prefix(&names);
        if p.len() <= typed.len() {
            // nothing more to add: show the choices (like a shell's second Tab)
            let mut row = alloc::string::String::new();
            for (c, d) in &cands {
                let item = alloc::format!("{c}{}  ", if *d { "/" } else { "" });
                if row.len() + item.len() > COLS - 1 {
                    push(Cls::Dim, row.trim_end().as_bytes());
                    row.clear();
                }
                row.push_str(&item);
            }
            if !row.is_empty() {
                push(Cls::Dim, row.trim_end().as_bytes());
            }
            return;
        }
        (alloc::string::String::from(p), "")
    };
    let mut new_line = alloc::string::String::from(&line[..word_start]);
    new_line.push_str(base);
    new_line.push_str(&insert);
    new_line.push_str(suffix);
    unsafe {
        let n = new_line.len().min(INPUT_MAX);
        (&mut *&raw mut INPUT)[..n].copy_from_slice(&new_line.as_bytes()[..n]);
        INPUT_LEN = n;
        HIST_NAV = 0;
    }
}

/// Left button pressed inside the terminal: start a selection.
pub fn mouse_down(mx: i64, my: i64) {
    let c = cell_at(mx, my);
    unsafe {
        SEL_A = c;
        SEL_B = c;
        SEL_DRAG = c.is_some();
    }
}

/// Left button held: extend the selection (the WM calls this every frame).
pub fn mouse_drag(mx: i64, my: i64) {
    unsafe {
        if !SEL_DRAG {
            return;
        }
        // dragging past the top / bottom edge scrolls the scrollback
        let (_, gy, _, gh, _, out_rows, _) = GRID;
        if my < gy && SCROLL + 1 < COUNT {
            SCROLL += 1;
        } else if my > gy + gh * out_rows as i64 && SCROLL > 0 {
            SCROLL -= 1;
        }
        SEL_B = cell_at(mx, my);
    }
}

/// Left button released.
pub fn mouse_up() {
    unsafe { SEL_DRAG = false };
}

pub fn selecting() -> bool {
    unsafe { SEL_DRAG }
}

/// Right-click: copy the selection if there is one, else paste (Windows Terminal style).
pub fn right_click() {
    if !copy_selection() {
        paste();
    }
}

/// Mouse wheel over the terminal: scroll the scrollback (positive = up).
pub fn wheel(lines: i32) {
    unsafe {
        if lines > 0 {
            SCROLL = (SCROLL + lines as usize).min(COUNT.saturating_sub(1));
        } else {
            SCROLL = SCROLL.saturating_sub((-lines) as usize);
        }
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
    load_theme();
    update_prompt_info();
    push(Cls::Accent, b"LingOS terminal -- type 'help'");
    push(Cls::Dim, b"a graphics-mode shell (separate from the text-mode lsh); 'term' themes it");
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

/// Run `line` in this terminal exactly as if the user typed and submitted it:
/// echo the prompt, record it in history, dispatch. The WM uses this to launch
/// an installed package (`ling run pkg-<name>/main.ling`) into a Terminal
/// window from the start menu, reusing all the terminal's rendering + the
/// in-kernel Ling interpreter rather than duplicating an app runner.
pub fn run_command(line: &str) {
    unsafe {
        if !STARTED {
            STARTED = true;
            banner();
        }
        let mut echo = [0u8; INPUT_MAX + 8];
        echo[0] = b'>';
        echo[1] = b' ';
        let n = line.len().min(INPUT_MAX);
        echo[2..2 + n].copy_from_slice(&line.as_bytes()[..n]);
        push(Cls::Accent, &echo[..2 + n]);
        history_push(&line.as_bytes()[..n]);
        run(&line[..n]);
    }
}

fn cwd_str() -> &'static str {
    unsafe { core::str::from_utf8(&(&*&raw const CWD)[..CWD_LEN]).unwrap_or("") }
}

/// True if `name` is a directory at lingfs root (for `cd`).
fn dir_exists(name: &str) -> bool {
    let mut idx = 0;
    let mut nm = [0u8; 64];
    while idx < 256 {
        match lingfs::list_entry("", idx, &mut nm) {
            Some((len, is_dir)) => {
                if is_dir && &nm[..len] == name.as_bytes() {
                    return true;
                }
                idx += 1;
            },
            None => break,
        }
    }
    false
}

/// Resolve a possibly-relative path against the CWD into `out`, returning the
/// slice. An absolute-looking path (with a '/') or an empty CWD passes
/// through unchanged; otherwise "cwd/name".
fn resolve_path<'a>(arg: &str, out: &'a mut [u8; 128]) -> &'a str {
    let cwd = cwd_str();
    if cwd.is_empty() || arg.contains('/') {
        let n = arg.len().min(out.len());
        out[..n].copy_from_slice(&arg.as_bytes()[..n]);
        return core::str::from_utf8(&out[..n]).unwrap_or("");
    }
    let mut n = 0;
    let cb = cwd.as_bytes();
    let cl = cb.len().min(out.len());
    out[..cl].copy_from_slice(&cb[..cl]);
    n += cl;
    if n < out.len() {
        out[n] = b'/';
        n += 1;
    }
    let ab = arg.as_bytes();
    let al = ab.len().min(out.len() - n);
    out[n..n + al].copy_from_slice(&ab[..al]);
    n += al;
    core::str::from_utf8(&out[..n]).unwrap_or("")
}

/// Record a command in history (skips empties and consecutive duplicates).
fn history_push(cmd: &[u8]) {
    unsafe {
        HIST_NAV = 0;
        if cmd.is_empty() {
            return;
        }
        if HIST_COUNT > 0 {
            let last = (HIST_HEAD + HIST_N - 1) % HIST_N;
            if HIST_LEN[last] == cmd.len() && HIST[last][..cmd.len()] == *cmd {
                return;
            }
        }
        let n = cmd.len().min(INPUT_MAX);
        HIST[HIST_HEAD][..n].copy_from_slice(&cmd[..n]);
        HIST_LEN[HIST_HEAD] = n;
        HIST_HEAD = (HIST_HEAD + 1) % HIST_N;
        if HIST_COUNT < HIST_N {
            HIST_COUNT += 1;
        }
    }
}

/// Recall a history entry into the input line. `delta` +1 = further back,
/// -1 = toward the fresh line.
fn history_recall(delta: i32) {
    unsafe {
        if HIST_COUNT == 0 {
            return;
        }
        let new_nav = (HIST_NAV as i32 + delta).clamp(0, HIST_COUNT as i32) as usize;
        HIST_NAV = new_nav;
        if new_nav == 0 {
            INPUT_LEN = 0;
            return;
        }
        let idx = (HIST_HEAD + HIST_N - new_nav) % HIST_N;
        let len = HIST_LEN[idx];
        INPUT[..len].copy_from_slice(&HIST[idx][..len]);
        INPUT_LEN = len;
    }
}

fn split_cmd(s: &str) -> (&str, &str) {
    match s.find(' ') {
        Some(i) => (&s[..i], s[i + 1..].trim_start()),
        None => (s, ""),
    }
}

/// The disk installer's lines, wrapped to the window's width.
#[cfg(target_arch = "x86_64")]
fn push_install(lines: alloc::vec::Vec<(u8, alloc::string::String)>) {
    use crate::drivers::installer as ins;
    let w = unsafe { GRID.4 }.clamp(20, COLS - 1);
    for (k, s) in lines {
        let cls = match k {
            ins::DIM => Cls::Dim,
            ins::ERR => Cls::Err,
            ins::OK => Cls::Ok,
            ins::ACCENT => Cls::Accent,
            _ => Cls::Normal,
        };
        if s.is_empty() {
            push(cls, b"");
            continue;
        }
        let b = s.as_bytes();
        let mut at = 0;
        while at < b.len() {
            let mut end = (at + w).min(b.len());
            // break at a space when the line is cut, and never inside a UTF-8 character
            if end < b.len() {
                if let Some(sp) = b[at..end].iter().rposition(|&c| c == b' ') {
                    if sp > w / 2 {
                        end = at + sp + 1;
                    }
                }
                while end > at + 1 && (b[end] & 0xC0) == 0x80 {
                    end -= 1;
                }
            }
            push(cls, &b[at..end]);
            at = end;
        }
    }
}

/// show whatever the model task has produced since the last look
fn pump_llm(cols: usize) {
    if !llm::has_output() {
        return;
    }
    llm::drain_lines(cols.clamp(20, COLS - 1), &mut |kind, s| {
        let cls = match kind {
            0 => Cls::Normal,
            1 => Cls::Dim,
            _ => Cls::Err,
        };
        push(cls, s.as_bytes());
    });
}

fn run(line: &str) {
    pump_llm(COLS - 1);
    let (cmd, arg) = split_cmd(line.trim());
    match cmd {
        "" => {},
        "ls" if crate::fs::media::is_media_path(arg) => {
            let items = crate::vfs::list(arg);
            if items.is_empty() {
                push(Cls::Dim, b"(empty)");
            }
            for (name, is_dir, size) in items {
                let mut line = alloc::string::String::from(name.as_str());
                if is_dir {
                    line.push('/');
                } else {
                    line.push_str(&alloc::format!("  {size}"));
                }
                push(Cls::Normal, line.as_bytes());
            }
        },
        "cat" if crate::fs::media::is_media_path(arg) => match crate::vfs::read(arg) {
            Some(bytes) => push_wrapped(Cls::Normal, &bytes[..bytes.len().min(8192)]),
            None => push(Cls::Err, b"cat: no such file"),
        },
        "mkdir" if crate::fs::media::is_media_path(arg) => {
            if crate::vfs::mkdir(arg) {
                push(Cls::Ok, b"created");
            } else {
                push(Cls::Err, b"mkdir: failed");
            }
        },
        "help" => {
            push(Cls::Normal, b"commands:");
            push(Cls::Normal, b"  help  clear  echo <t>  ls [dir]  cat <file>");
            push(Cls::Normal, b"  date  theme dark|light  copy <t>  paste");
            push(Cls::Normal, b"  nic   dns <host>   curl <url>   bring <url>");
            push(Cls::Normal, b"  play <file.wav>   stop");
            push(Cls::Normal, b"  lingfu sync [--all]|upgrade|search <q>|install <name>|--all");
            push(Cls::Normal, b"  ling run <file.ling>|eval <src>   run demo|donut|<app.elf>");
            push(Cls::Normal, b"  cd <dir>   sudo <cmd>   su [user]   ling-life");
            push(Cls::Normal, b"  ssh [user@]host   ssh-serve on|off   services [enable|disable ssh]");
            push(Cls::Normal, b"  term [preset|text|back|accent|font ...]   the terminal's own look");
            push(Cls::Normal, b"  miao <rite...>   the temple of versions (lingfu install ling-miao)");
            push(Cls::Normal, b"  cryptotest");
            push(Cls::Normal, b"  https <url>   imgview demo|demosvg|<url>");
            push(Cls::Normal, b"  ask <question>   llm [status|load [path]|stop]   (Qwen3, on this machine)");
            push(Cls::Normal, b"  install-disk [n]   install LingOS from the USB stick onto a disk");
            push(Cls::Dim, b"  up/down: command history");
        },
        "clear" => unsafe {
            COUNT = 0;
            HEAD = 0;
            SCROLL = 0;
            SEL_A = None;
            SEL_B = None;
        },
        "echo" => push(Cls::Normal, arg.as_bytes()),
        "ask" => {
            if arg.is_empty() {
                push(Cls::Err, b"usage: ask <question>");
            } else if !llm::start() {
                push(Cls::Err, b"ask: no free task slot for the model");
            } else if !llm::ask(arg) {
                push(Cls::Dim, b"llm is still busy with the last question (llm stop)");
            } else if llm::phase() == llm::Phase::Idle || llm::phase() == llm::Phase::Failed {
                push(Cls::Dim, b"asking: the model loads first (from the model disk or a .gguf on a FAT volume)...");
            } else {
                push(Cls::Dim, b"asking...");
            }
        },
        "llm" => match arg {
            "" | "status" => {
                let s = llm::status();
                push(Cls::Normal, s.as_bytes());
            },
            // `llm load [path]`: the default model, `disk` (x86's raw model disk) or a .gguf (media/disk1/models/x.gguf, ...)
            a if a == "load" || a.starts_with("load ") => {
                if llm::request_load(a[4..].trim()) {
                    push(Cls::Dim, b"loading the model (llm status shows how far)...");
                }
            },
            "stop" => {
                llm::cancel();
                push(Cls::Dim, b"stopping");
            },
            _ => push(Cls::Err, b"usage: llm [status|load [path.gguf|disk]|stop]"),
        },
        "play" => {
            if arg.is_empty() {
                push(Cls::Err, b"usage: play <file.wav>");
            } else if media::open(arg) {
                mixer::pcm_play();
                push(Cls::Ok, b"playing (media)");
            } else {
                push(Cls::Err, media::status().as_bytes());
            }
        },
        "stop" => {
            mixer::pcm_stop();
            push(Cls::Dim, b"stopped");
        },
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
            let dir = if arg.is_empty() { cwd_str() } else { arg };
            let mut idx = 0;
            let mut nm = [0u8; 64];
            let mut any = false;
            while idx < 128 {
                match lingfs::list_entry(dir, idx, &mut nm) {
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
                let mut pbuf = [0u8; 128];
                let path = resolve_path(arg, &mut pbuf);
                match lingfs::read_file_all(path, cb) {
                    Ok(Some(len)) => push_wrapped(Cls::Normal, &cb[..len]),
                    _ => push(Cls::Err, b"cat: no such file"),
                }
            }
        },
        "cp" | "mv" => {
            let mut it = arg.split_whitespace();
            let (Some(a), Some(b)) = (it.next(), it.next()) else {
                push(Cls::Err, if cmd == "cp" { b"usage: cp <src> <dst>" } else { b"usage: mv <src> <dst>" });
                return;
            };
            let (mut pa, mut pb) = ([0u8; 128], [0u8; 128]);
            let src = alloc::string::String::from(resolve_path(a, &mut pa));
            let dst = crate::vfs::target_for(&src, resolve_path(b, &mut pb));
            let ok = if cmd == "cp" { crate::vfs::copy(&src, &dst) } else { crate::vfs::rename(&src, &dst) };
            if ok {
                push(Cls::Ok, dst.as_bytes());
            } else {
                push(Cls::Err, b"failed (source missing, destination not writable, or folder not empty)");
            }
        },
        "rm" => {
            if arg.is_empty() {
                push(Cls::Err, b"usage: rm <path>");
            } else {
                let mut pb = [0u8; 128];
                let p = resolve_path(arg, &mut pb);
                if crate::vfs::delete(p) {
                    push(Cls::Ok, b"deleted");
                } else {
                    push(Cls::Err, b"rm: not deleted (missing, a non-empty folder, or read-only storage)");
                }
            }
        },
        "nic" => {
            // which NIC (e1000 / igc), MAC, link speed, IP
            let up = crate::drivers::netdev::ensure_init();
            let line = crate::drivers::netdev::status_line();
            push_wrapped(if up { Cls::Ok } else { Cls::Err }, line.replace("  ", "\n").as_bytes());
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
        "lingfu" => run_lingfu(arg),
        "ling" => run_ling(arg),
        "run" => run_native(arg),
        "ssh" => run_ssh(arg),
        "ssh-serve" => run_ssh_serve(arg),
        "term" => run_term(arg),
        "miao" => run_miao(arg),
        "services" => run_services(arg),
        "2fa" => run_2fa(arg),
        "https" => run_https(arg),
        "imgview" => run_imgview(arg),
        "cd" => {
            // Nested paths, `~` (home) and `..` all work now, not just a
            // single root-level dir -- so you can cd into your project folder
            // and run `miao` there (the prompt's shrine segment follows cwd).
            let target: alloc::string::String = if arg.is_empty() || arg == "~" {
                crate::vfs::home()
            } else if arg == "/" {
                alloc::string::String::new()
            } else if let Some(rest) = arg.strip_prefix('/') {
                crate::vfs::norm(rest)
            } else if arg == ".." || arg.starts_with("../") || arg.contains("..") {
                crate::vfs::norm(&alloc::format!("{}/{arg}", cwd_str()))
            } else {
                let cwd = cwd_str();
                if cwd.is_empty() {
                    crate::vfs::norm(arg)
                } else {
                    crate::vfs::norm(&alloc::format!("{cwd}/{arg}"))
                }
            };
            if target.is_empty() {
                unsafe { CWD_LEN = 0 };
            } else if target.len() <= 64 && (crate::vfs::is_dir(&target) || dir_exists(&target)) {
                let b = target.as_bytes();
                let n = b.len().min(64);
                unsafe {
                    CWD[..n].copy_from_slice(&b[..n]);
                    CWD_LEN = n;
                }
            } else if target.len() > 64 {
                push(Cls::Err, b"cd: path too long for this shell");
            } else {
                push(Cls::Err, b"cd: no such directory");
            }
        },
        "pwd" => {
            let c = cwd_str();
            if c.is_empty() {
                push(Cls::Normal, b"/");
            } else {
                let mut b = [0u8; 66];
                b[0] = b'/';
                let n = c.len().min(64);
                b[1..1 + n].copy_from_slice(&c.as_bytes()[..n]);
                push(Cls::Normal, &b[..1 + n]);
            }
        },
        "whoami" => push(Cls::Normal, lingfs::current_user().as_bytes()),
        "sudo" => {
            if arg.is_empty() {
                push(Cls::Err, b"usage: sudo <command>");
            } else {
                let b = arg.as_bytes();
                let n = b.len().min(INPUT_MAX);
                unsafe {
                    PENDING[..n].copy_from_slice(&b[..n]);
                    PENDING_LEN = n;
                    PW_MODE = 1;
                    PW_LEN = 0;
                }
                let mut m = [0u8; 64];
                let p = b"[sudo] password for ";
                m[..p.len()].copy_from_slice(p);
                let u = lingfs::current_user();
                let un = u.len().min(m.len() - p.len() - 1);
                m[p.len()..p.len() + un].copy_from_slice(&u.as_bytes()[..un]);
                m[p.len() + un] = b':';
                push(Cls::Dim, &m[..p.len() + un + 1]);
            }
        },
        "rename-user" => {
            let new = arg.trim();
            if new.is_empty() || new.contains(' ') {
                push(Cls::Err, b"usage: rename-user <new name>   (renames the signed-in account; asks for its password)");
            } else {
                let b = new.as_bytes();
                let n = b.len().min(INPUT_MAX);
                unsafe {
                    PENDING[..n].copy_from_slice(&b[..n]);
                    PENDING_LEN = n;
                    PW_MODE = 3;
                    PW_LEN = 0;
                }
                let line = alloc::format!("password for {} (to rename it to {new}):", lingfs::current_user());
                push(Cls::Dim, line.as_bytes());
            }
        },
        "su" => {
            let target = if arg.is_empty() { "root" } else { arg };
            let b = target.as_bytes();
            let n = b.len().min(INPUT_MAX);
            unsafe {
                PENDING[..n].copy_from_slice(&b[..n]);
                PENDING_LEN = n;
                PW_MODE = 2;
                PW_LEN = 0;
            }
            push(Cls::Dim, b"password:");
        },
        "ling-life" => {
            // life::run() is the fullscreen text-mode idle screensaver; it
            // takes over the console and can't render inside this framebuffer
            // window, so don't launch it from here (it would freeze the WM).
            push(Cls::Normal, b"ling-life: Conway's Life screensaver.");
            push(Cls::Dim, b"It runs fullscreen on idle; a windowed Life app is on the roadmap.");
        },
        "cryptotest" => {
            // Prove the TLS 1.3 crypto suite computes correct answers in-kernel
            // (known-answer test vectors), not just that it compiled.
            let t = crate::crypto::selftest();
            let mark = |ok: bool| if ok { b"[ok]  ".as_slice() } else { b"[FAIL]".as_slice() };
            let mut row = |ok: bool, name: &[u8]| {
                let mut b = [0u8; 48];
                let m = mark(ok);
                b[..m.len()].copy_from_slice(m);
                b[m.len()] = b' ';
                let n = name.len().min(b.len() - m.len() - 1);
                b[m.len() + 1..m.len() + 1 + n].copy_from_slice(&name[..n]);
                push(if ok { Cls::Ok } else { Cls::Err }, &b[..m.len() + 1 + n]);
            };
            push(Cls::Accent, b"TLS crypto self-test (known-answer vectors):");
            row(t.rdrand, b"RDRAND entropy");
            row(t.sha256, b"SHA-256          (FIPS 180-4)");
            row(t.sha512, b"SHA-512          (FIPS 180-4)");
            row(t.hmac, b"HMAC-SHA256      (RFC 4231)");
            row(t.hkdf, b"HKDF-SHA256      (RFC 5869)");
            row(t.chachapoly, b"ChaCha20-Poly1305 (RFC 8439)");
            row(t.x25519, b"X25519 ECDHE     (RFC 7748)");
            row(t.ed25519, b"Ed25519 sign+vfy (RFC 8032)");
            if t.all_ok() {
                push(Cls::Ok, b"all primitives pass -- ready for TLS + SSH");
            } else {
                push(Cls::Err, b"some primitives failed");
            }
        },
        "alloctest" => {
            // Exercise the new kernel global allocator: a growing Vec (which
            // reallocs), a String, then drop (which frees). Prints a known
            // checksum -- sum of i*i for i in 0..1000 == 332833500 -- so a
            // correct allocator is verifiable at a glance.
            let mut v: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
            let mut i: u32 = 0;
            while i < 1000 {
                v.push(i * i);
                i += 1;
            }
            let mut sum: u32 = 0;
            for x in v.iter() {
                sum = sum.wrapping_add(*x);
            }
            let mut s = alloc::string::String::new();
            s.push_str("alloc ok: sum=");
            let mut nb = [0u8; 16];
            let n = w_any(&mut nb, sum);
            if let Ok(ds) = core::str::from_utf8(&nb[..n]) {
                s.push_str(ds);
            }
            s.push_str(" len=");
            let mut lb = [0u8; 16];
            let ln = w_any(&mut lb, v.len() as u32);
            if let Ok(ls) = core::str::from_utf8(&lb[..ln]) {
                s.push_str(ls);
            }
            push(Cls::Ok, s.as_bytes());
            // v and s drop here, exercising free().
        },
        // install LingOS onto a disk (drivers::installer): list, then ERASE
        // RELEASE GATE (main session, 2026-11-09): until the installer passes its
        // QEMU tests it only runs inside a VM; real hardware gets a notice
        // instead (the user's PC has Windows + Debian disks). Remove the gate
        // once it is verified.
        #[cfg(target_arch = "x86_64")]
        "install-disk" => push_install(crate::drivers::installer::command(arg)),
        #[cfg(target_arch = "x86_64")]
        "update-boot" => push_install(crate::drivers::installer::update_boot()),
        // the hardware / diagnostic commands the console and SSH share
        "cpu" | "gpu" | "nvme" | "wifi-intel" | "cores" | "prof" | "bench3d" | "report" => {
            let mut cwd = alloc::string::String::new();
            let out = crate::drivers::sshcmds::run(cmd, arg, "live", &mut cwd).unwrap_or_default();
            for l in out.lines() {
                let cls = if l.contains("FAIL") || l.contains("failed") { Cls::Err } else { Cls::Normal };
                push(cls, l.as_bytes());
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
    // the prompt line's hostname / miao-shrine pieces may have changed
    update_prompt_info();
}

/// `term` -- this terminal's look: presets, per-part colors, font size.
fn run_term(arg: &str) {
    let (sub, rest) = split_cmd(arg);
    let show = || {
        let (t, b, a) = theme_colors();
        push(Cls::Accent, b"term -- this terminal's look (persisted)");
        push(Cls::Normal, alloc::format!("  text {t:06X}   back {b:06X}   accent {a:06X}   font {}", unsafe { ZOOM }).as_bytes());
        push(Cls::Normal, b"  term <preset>              dusk ink paper rose ember gold moss iris");
        push(Cls::Normal, b"  term text|back|accent <c>  RRGGBB or red..violet, white, black, grey");
        push(Cls::Normal, b"  term font <1|2|3>          the pixel font, 1x..3x");
        push(Cls::Dim, b"  dusk follows the global theme (theme dark|light)");
    };
    match sub {
        "" | "help" | "show" | "list" => show(),
        "font" => {
            let z = rest.parse::<u32>().unwrap_or(0);
            if (1..=3).contains(&z) {
                unsafe { ZOOM = z };
                save_theme();
                push(Cls::Ok, b"font size set");
            } else {
                push(Cls::Err, b"usage: term font <1|2|3>  (Ctrl +/- zooms too)");
            }
        },
        "text" | "back" | "accent" => {
            let Some(c) = parse_color(rest) else {
                push(Cls::Err, b"term: that color is neither RRGGBB nor a name (red..violet)");
                return;
            };
            let (mut t, mut b, mut a) = theme_colors();
            match sub {
                "text" => t = c,
                "back" => b = c,
                _ => a = c,
            }
            unsafe { THEME_CUSTOM = Some((t, b, a)) };
            save_theme();
            push(Cls::Ok, b"color set");
        },
        name => match TERM_PRESETS.iter().find(|(n, _, _, _)| *n == name) {
            Some((n, t, b, a)) => {
                unsafe { THEME_CUSTOM = if *n == "dusk" { None } else { Some((*t, *b, *a)) } };
                save_theme();
                push(Cls::Ok, b"preset applied");
            },
            None => {
                push(Cls::Err, b"term: no such preset");
                show();
            },
        },
    }
}

/// `ssh-serve on|off|status` -- the SSH + SFTP server, persisted so the
/// choice holds at every boot (same switch as Settings > Startup).
fn run_ssh_serve(arg: &str) {
    match arg.trim() {
        "on" | "start" | "enable" => {
            crate::startup::set("ssh", true);
            let ip = netstack::self_ip();
            push(Cls::Ok, b"ssh-serve: ON -- SSH + SFTP server, port 22 (persisted: starts at boot)");
            push(Cls::Normal, alloc::format!("  reach it at {}.{}.{}.{} (static IP or DHCP, whichever the system uses)", ip[0], ip[1], ip[2], ip[3]).as_bytes());
        },
        "off" | "stop" | "disable" => {
            crate::startup::set("ssh", false);
            push(Cls::Ok, b"ssh-serve: OFF -- the listener is closed and it stays off at boot");
        },
        "" | "status" => {
            if crate::drivers::sshd::enabled() {
                let ip = netstack::self_ip();
                push(Cls::Ok, alloc::format!("ssh-serve: running on {}.{}.{}.{}:22 (SSH + SFTP)", ip[0], ip[1], ip[2], ip[3]).as_bytes());
            } else {
                push(Cls::Dim, b"ssh-serve: off");
            }
            if crate::startup::ssh_at_boot() {
                push(Cls::Normal, b"  starts at boot (ssh-serve off to stop)");
            } else {
                push(Cls::Normal, b"  not started at boot (ssh-serve on to start)");
            }
        },
        _ => push(Cls::Err, b"usage: ssh-serve [on|off|status]"),
    }
}

/// `miao <args>` -- the temple of versions: routes to the installed
/// `ling-miao` lingfu package (its LingOS edition). `--cwd /<dir>` hands it
/// the shell's working directory (lingfs-root absolute, see vfs::resolve) so
/// the rites run where the user stands, not inside the package folder.
fn run_miao(arg: &str) {
    if crate::lingc::locate("pkg-ling-miao").is_none() {
        push(Cls::Err, b"miao: the temple is not installed on this machine");
        push(Cls::Normal, b"  lingfu install ling-miao   (then: miao kai | wen | gong | ji | pu ...)");
        return;
    }
    let mut cmd = alloc::format!("run pkg-ling-miao --cwd /{}", cwd_str());
    if !arg.trim().is_empty() {
        cmd.push(' ');
        cmd.push_str(arg.trim());
    }
    run_ling(&cmd);
}

/// The desktop-terminal front end for the kernel `lingfu` package client.
/// Handles the *package-management* verbs that work without a compiler
/// (sync/search/install/update); the project/toolchain verbs (new, build,
/// run, test, ...) need the `ling` compiler, which doesn't run inside LingOS
/// yet -- so we say so plainly rather than pretend. lingfu writes through a
/// capture sink here so its output lands in this window, not the text console.
fn run_lingfu(arg: &str) {
    let (sub, rest) = split_cmd(arg);
    match sub {
        "" | "help" => {
            push(Cls::Accent, b"lingfu -- LingOS package client");
            push(Cls::Normal, b"  lingfu sync              refresh the catalog + upgrade installed");
            push(Cls::Normal, b"  lingfu sync --all        ... and install every package that fits");
            push(Cls::Normal, b"  lingfu search <query>    find packages");
            push(Cls::Normal, b"  lingfu install <name>    download + install a package");
            push(Cls::Normal, b"  lingfu install --all     install every catalog package that fits");
            push(Cls::Normal, b"  lingfu upgrade           update installed packages to the latest");
            push(Cls::Normal, b"  lingfu verify <name>     check a download's checksum, write nothing");
            push(Cls::Dim, b"  new/build/run/test need the ling compiler (host-only for now)");
        },
        "new" | "init" | "build" | "run" | "test" | "check" | "clean" | "doc" | "fmt"
        | "bench" | "tree" | "publish" | "add" | "manifest" | "wizard" => {
            push(Cls::Err, b"lingfu: that verb needs the ling compiler/toolchain,");
            push(Cls::Err, b"which doesn't run inside LingOS yet (no userspace/loader).");
            push(Cls::Dim, b"Package verbs that DO work here: sync, search, install, upgrade.");
        },
        "sync" if rest == "--all" => {
            push(Cls::Dim, b"syncing + installing every package that fits (this takes a while)...");
            lingfu::begin_capture();
            lingfu::sync_and_upgrade();
            lingfu::install_all();
            dump_capture();
        },
        "sync" => {
            push(Cls::Dim, b"contacting repo...");
            lingfu::begin_capture();
            lingfu::sync_and_upgrade();
            dump_capture();
        },
        "update" | "upgrade" => {
            push(Cls::Dim, b"checking installed packages...");
            lingfu::begin_capture();
            lingfu::upgrade_all();
            dump_capture();
        },
        "verify" if !rest.is_empty() => {
            push(Cls::Dim, b"downloading + checking (nothing is written)...");
            lingfu::begin_capture();
            lingfu::verify(rest);
            dump_capture();
        },
        "install" if rest == "--all" => {
            push(Cls::Dim, b"installing every package that fits (this takes a while)...");
            lingfu::begin_capture();
            lingfu::install_all();
            dump_capture();
        },
        "search" | "list" => {
            lingfu::begin_capture();
            if !lingfu::synced() {
                lingfu::sync();
            }
            lingfu::list(rest);
            dump_capture();
        },
        "install" => {
            if rest.is_empty() {
                push(Cls::Err, b"usage: lingfu install <name>");
            } else {
                push(Cls::Dim, b"installing...");
                lingfu::begin_capture();
                lingfu::install(rest);
                dump_capture();
            }
        },
        other => {
            let mut b = [0u8; 64];
            let p = b"lingfu: unknown subcommand: ";
            b[..p.len()].copy_from_slice(p);
            let n = other.len().min(b.len() - p.len());
            b[p.len()..p.len() + n].copy_from_slice(&other.as_bytes()[..n]);
            push(Cls::Err, &b[..p.len() + n]);
        },
    }
}

/// `ling run <file.ling> [args]` / `ling eval <source>` -- run a Ling program
/// with the in-kernel compiler + interpreter (crate::lingc: the real lexer,
/// parser and MIR lowering, interpreted over the same runtime bundled apps
/// use). Output and any error are printed into this window.
fn run_ling(arg: &str) {
    let (sub, rest) = split_cmd(arg);
    match sub {
        "" | "help" | "--help" | "-h" => {
            push(Cls::Accent, b"ling -- run Ling programs (in-kernel compiler + interpreter)");
            push(Cls::Normal, b"  ling run <file.ling> [args]   run a program (a folder runs its main.ling)");
            push(Cls::Normal, b"  ling eval <source>            run a one-line program");
            push(Cls::Normal, b"  ling demo                     a built-in test program");
            push(Cls::Dim, b"  the whole language; builtins: core, strings, lists, st_*, files, px_* graphics, daw_*");
        },
        "eval" => {
            if rest.is_empty() {
                push(Cls::Err, b"usage: ling eval <source>");
            } else {
                run_ling_source(rest);
            }
        },
        "demo" => run_ling_source(crate::ling::DEMO),
        "run" => {
            let mut words = rest.split(' ').filter(|w| !w.is_empty());
            let Some(file) = words.next() else {
                push(Cls::Err, b"usage: ling run <file.ling> [args]");
                return;
            };
            let args: alloc::vec::Vec<&str> = words.collect();
            let mut pbuf = [0u8; 128];
            let path = resolve_path(file, &mut pbuf);
            // Relative to the current folder first, then as typed from the
            // root (`ling run pkg-<name>` works from anywhere).
            let (out, err) = crate::ling::run_file(&[path, file], &args);
            show_ling_result(out, err);
        },
        _ => push(Cls::Err, b"ling: unknown subcommand (try: ling run <file> | ling eval <src>)"),
    }
}

/// `run demo` / `run <path.elf>` -- load a native AOT-compiled Ling app into
/// a ring-3 process and run it to completion, showing its output here. This
/// is the real native-app path (Cranelift AOT -> ling-user -> ELF loader ->
/// ring 3 -> syscalls), distinct from `ling run` (the in-kernel interpreter).
fn run_native(arg: &str) {
    // The donut is a graphics app: it maps the framebuffer and draws straight
    // to the screen (no text output to capture), then returns when it exits.
    if arg == "donut" {
        push(Cls::Dim, b"spinning up the donut (press any key to exit)...");
        let result = crate::run_app_bytes(crate::DONUT_APP_ELF);
        report_exit(result);
        return;
    }
    push(Cls::Dim, b"launching native app (ring 3)...");
    crate::console_capture_begin();
    let result = if arg.is_empty() || arg == "demo" {
        crate::run_app_bytes(crate::HELLO_APP_ELF)
    } else {
        let mut pbuf = [0u8; 128];
        let path = resolve_path(arg, &mut pbuf);
        crate::run_app_file(path)
    };
    let cap = crate::console_capture_end();
    for line in cap.split(|&b| b == b'\n') {
        if !line.is_empty() {
            push_wrapped(Cls::Normal, line);
        }
    }
    report_exit(result);
}

fn report_exit(result: Result<i32, &'static str>) {
    match result {
        Ok(code) => {
            let mut b = [0u8; 32];
            let p = b"[exited, code ";
            b[..p.len()].copy_from_slice(p);
            let mut n = p.len();
            n += w_any(&mut b[n..], code as u32);
            b[n] = b']';
            n += 1;
            push(Cls::Ok, &b[..n]);
        },
        Err(e) => {
            let mut b = [0u8; 96];
            let p = b"run: ";
            b[..p.len()].copy_from_slice(p);
            let k = e.len().min(b.len() - p.len());
            b[p.len()..p.len() + k].copy_from_slice(&e.as_bytes()[..k]);
            push(Cls::Err, &b[..p.len() + k]);
        },
    }
}

/// `ssh [user@]host[:port]` -- connect over TCP and do the SSH version-banner
/// exchange (RFC 4253's first step), showing the server's banner. The full
/// SSH transport (KEX/auth/shell) is the large protocol work that builds on
/// crypto.rs; this proves the client reaches and speaks to a real sshd.
fn run_ssh(arg: &str) {
    if arg.is_empty() {
        push(Cls::Err, b"usage: ssh [user@]host[:port]");
        return;
    }
    let hostport = match arg.rfind('@') {
        Some(i) => &arg[i + 1..],
        None => arg,
    };
    let (host, port) = match hostport.rfind(':') {
        Some(i) => (&hostport[..i], hostport[i + 1..].parse::<u16>().unwrap_or(22)),
        None => (hostport, 22u16),
    };
    push(Cls::Dim, b"resolving...");
    let Some(ip) = netstack::dns_resolve(host) else {
        push(Cls::Err, b"ssh: could not resolve host");
        return;
    };
    push(Cls::Dim, b"connecting...");
    if !netstack::tcp_connect(ip, port) {
        push(Cls::Err, b"ssh: connection refused / no route");
        return;
    }
    netstack::tcp_write(b"SSH-2.0-LingOS_0.1\r\n");
    let mut buf = [0u8; 512];
    let n = netstack::tcp_read_some(&mut buf, 3_000_000);
    if n == 0 {
        push(Cls::Err, b"ssh: connected, but no banner received");
        return;
    }
    let mut end = 0;
    while end < n && buf[end] != b'\r' && buf[end] != b'\n' {
        end += 1;
    }
    push(Cls::Ok, &buf[..end]);
    push(Cls::Dim, b"ssh: TCP + SSH version banner exchanged with the server.");
    push(Cls::Dim, b"     KEX/auth/shell (the SSH transport) build on the crypto suite -- WIP.");
}

const PNG_SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

/// `imgview [demo|<url>]` -- fetch (http/https) and decode an image, then show
/// it fullscreen (press a key to return). PNG (8-bit) and subset SVG.
fn run_imgview(arg: &str) {
    static mut IMGBUF: [u8; 512 * 1024] = [0; 512 * 1024];
    let ib = unsafe { &mut *&raw mut IMGBUF };
    let (len, off): (usize, usize) = if arg.is_empty() || arg == "demo" {
        static TEST_PNG: &[u8] = include_bytes!("../../testbins/test.png");
        let n = TEST_PNG.len().min(ib.len());
        ib[..n].copy_from_slice(&TEST_PNG[..n]);
        (n, 0)
    } else if arg == "demosvg" {
        static TEST_SVG: &[u8] = include_bytes!("../../testbins/test.svg");
        let n = TEST_SVG.len().min(ib.len());
        ib[..n].copy_from_slice(&TEST_SVG[..n]);
        (n, 0)
    } else {
        let Some((host, port, path, tls)) = netstack::parse_url(arg) else {
            push(Cls::Err, b"imgview: bad URL");
            return;
        };
        push(Cls::Dim, b"imgview: fetching...");
        if tls {
            let hport = if port == 0 { 443 } else { port };
            let mut noop = |_: &[u8]| {};
            match crate::tls::https_get(host, hport, path, ib, &mut noop) {
                Ok(n) if n > 0 => (n, crate::tls::http_body_offset(&ib[..n])),
                _ => {
                    push(Cls::Err, b"imgview: https fetch failed");
                    return;
                },
            }
        } else {
            let Some(ip) = netstack::dns_resolve(host) else {
                push(Cls::Err, b"imgview: DNS failed");
                return;
            };
            match netstack::http_get(ip, port, path, host, ib) {
                Some(n) => (n, 0),
                None => {
                    push(Cls::Err, b"imgview: fetch failed");
                    return;
                },
            }
        }
    };
    let bytes = &ib[off..len];
    let img = if bytes.len() >= 8 && bytes[..8] == PNG_SIG {
        crate::drivers::image::decode_png(bytes)
    } else if crate::drivers::image::looks_svg(bytes) {
        crate::drivers::image::decode_svg(bytes)
    } else {
        None
    };
    match img {
        Some(im) => {
            push(Cls::Ok, b"imgview: decoded -- showing (press a key)");
            crate::drivers::image::show_fullscreen(&im);
            crate::arch::timer::poll_until(20_000_000, || crate::drivers::keyboard::poll_char() != 0);
        },
        None => push(Cls::Err, b"imgview: could not decode (PNG 8-bit or subset SVG)"),
    }
}

/// `https <host[:port][/path]>` -- a real TLS 1.3 handshake (X25519 +
/// ChaCha20-Poly1305) and an encrypted GET, showing the decrypted response.
/// Certificate validation is NOT performed yet (disclosed) -- this proves the
/// handshake + record crypto, not authenticity.
fn run_https(arg: &str) {
    if arg.is_empty() {
        push(Cls::Err, b"usage: https <host[:port][/path]>");
        return;
    }
    let a = arg.strip_prefix("https://").unwrap_or(arg);
    let (hostpart, path) = match a.find('/') {
        Some(i) => (&a[..i], &a[i..]),
        None => (a, "/"),
    };
    let (host, port) = match hostpart.rfind(':') {
        Some(i) => (&hostpart[..i], hostpart[i + 1..].parse::<u16>().unwrap_or(443)),
        None => (hostpart, 443u16),
    };
    push(Cls::Dim, b"tls: cert validation is NOT done yet -- channel is encrypted, not authenticated");
    static mut HTTPSBUF: [u8; 32 * 1024] = [0; 32 * 1024];
    let buf = unsafe { &mut *&raw mut HTTPSBUF };
    let mut logline = |m: &[u8]| {
        let m = if m.last() == Some(&b'\n') { &m[..m.len() - 1] } else { m };
        push(Cls::Dim, m);
    };
    match crate::tls::https_get(host, port, path, buf, &mut logline) {
        Ok(0) => push(Cls::Err, b"https: handshake ok but no data decrypted"),
        Ok(len) => {
            push(Cls::Ok, b"https: response (decrypted):");
            push_wrapped(Cls::Normal, &buf[..len.min(2048)]);
        },
        Err(e) => {
            let mut b = [0u8; 96];
            let p = b"https: ";
            b[..p.len()].copy_from_slice(p);
            let k = e.len().min(b.len() - p.len());
            b[p.len()..p.len() + k].copy_from_slice(&e.as_bytes()[..k]);
            push(Cls::Err, &b[..p.len() + k]);
        },
    }
}

/// `2fa [status | setup | confirm <code> | off <code> | cancel]` for the
/// signed-in account: two-factor sign-in with an authenticator app
/// (crate::fs::twofa). `setup` prints the key to type into the app.
fn run_2fa(arg: &str) {
    use crate::fs::twofa;
    let user = lingfs::current_user();
    let (cmd, rest) = match arg.trim().split_once(' ') {
        Some((c, r)) => (c, r.trim()),
        None => (arg.trim(), ""),
    };
    match cmd {
        "" | "status" => {
            if twofa::enabled(user) {
                push(Cls::Ok, alloc::format!("2fa: on for {user} ({} recovery codes left)", twofa::recovery_left(user)).as_bytes());
            } else {
                push(Cls::Normal, alloc::format!("2fa: off for {user} -- '2fa setup' turns it on").as_bytes());
            }
        },
        "setup" => match twofa::begin(user) {
            Some((secret, uri)) => {
                push(Cls::Normal, b"In your authenticator app choose 'enter a setup key' and type:");
                let mut spaced = alloc::string::String::new();
                for (i, ch) in secret.chars().enumerate() {
                    if i > 0 && i % 4 == 0 {
                        spaced.push(' ');
                    }
                    spaced.push(ch);
                }
                push(Cls::Ok, alloc::format!("  account: LingOS {user}   key: {spaced}   (time-based, 6 digits)").as_bytes());
                push(Cls::Dim, alloc::format!("  ({uri})").as_bytes());
                push(Cls::Normal, b"Then: 2fa confirm <the 6-digit code the app shows>");
                if !crate::clock::is_set() {
                    push(Cls::Err, b"The clock is not set from the network: connect first, codes are time-based.");
                }
            },
            None => push(Cls::Err, b"2fa: already on (2fa off <code> first), or storage is not writable"),
        },
        "confirm" => match twofa::confirm(user, rest) {
            Some(codes) => {
                push(Cls::Ok, b"2fa: on. Signing in now asks for a code after the password.");
                push(Cls::Normal, b"Recovery codes (each works once -- write them down, they are not shown again):");
                for c in codes {
                    push(Cls::Ok, alloc::format!("  {c}").as_bytes());
                }
                push(Cls::Dim, b"SSH / su: type the password with the 6-digit code right after it.");
            },
            None => push(Cls::Err, b"2fa: that code did not match (or no setup pending) -- check the clock"),
        },
        "off" => {
            if twofa::disable(user, rest) {
                push(Cls::Ok, b"2fa: off");
            } else {
                push(Cls::Err, b"2fa: that code did not match");
            }
        },
        "cancel" => {
            twofa::cancel(user);
            push(Cls::Normal, b"2fa: setup cancelled");
        },
        _ => push(Cls::Normal, b"usage: 2fa [status | setup | confirm <code> | off <code> | cancel]"),
    }
}

/// `services [list|enable ssh|disable ssh]` -- configure which services start
/// at boot (persisted to lingfs /services, honored by services::boot_configure).
fn run_services(arg: &str) {
    let (sub, rest) = split_cmd(arg);
    match sub {
        "" | "list" | "status" => {
            if crate::services::ssh_enabled() {
                push(Cls::Ok, b"  ssh    enabled  (starts at boot)");
            } else {
                push(Cls::Dim, b"  ssh    disabled");
            }
            push(Cls::Dim, b"  services enable ssh | services disable ssh");
        },
        "enable" => {
            if rest == "ssh" {
                crate::services::set_ssh(true);
                push(Cls::Ok, b"ssh enabled -- starts at next boot");
            } else {
                push(Cls::Err, b"services: unknown service (only 'ssh')");
            }
        },
        "disable" => {
            if rest == "ssh" {
                crate::services::set_ssh(false);
                push(Cls::Ok, b"ssh disabled");
            } else {
                push(Cls::Err, b"services: unknown service (only 'ssh')");
            }
        },
        _ => push(Cls::Err, b"usage: services [list|enable ssh|disable ssh]"),
    }
}

fn run_ling_source(src: &str) {
    let (out, err) = crate::ling::run_source(src);
    show_ling_result(out, err);
}

fn show_ling_result(out: alloc::string::String, err: Option<alloc::string::String>) {
    if !out.is_empty() {
        push_wrapped(Cls::Normal, out.as_bytes());
    }
    if let Some(e) = err {
        let mut b = [0u8; 128];
        let p = b"ling error: ";
        b[..p.len()].copy_from_slice(p);
        let n = e.len().min(b.len() - p.len());
        b[p.len()..p.len() + n].copy_from_slice(&e.as_bytes()[..n]);
        push(Cls::Err, &b[..p.len() + n]);
    }
}

/// Push everything lingfu captured this call into the terminal, one line per
/// output line (blank lines dropped).
fn dump_capture() {
    let out = lingfu::end_capture();
    for line in out.split(|&b| b == b'\n') {
        if !line.is_empty() {
            push_wrapped(Cls::Normal, line);
        }
    }
}

fn fetch_and_dump(url: &str, render: bool) {
    let Some((host, port, path, tls)) = netstack::parse_url(url) else {
        push(Cls::Err, b"bad URL");
        return;
    };
    push(Cls::Dim, b"fetching...");
    static mut WEBBUF: [u8; 32 * 1024] = [0; 32 * 1024];
    let wb = unsafe { &mut *&raw mut WEBBUF };
    let (len, off) = if tls {
        // HTTPS via the in-kernel TLS 1.3 client (cert validation not done yet).
        let hport = if port == 0 { 443 } else { port };
        let mut noop = |_: &[u8]| {};
        match crate::tls::https_get(host, hport, path, wb, &mut noop) {
            Ok(n) if n > 0 => (n, crate::tls::http_body_offset(&wb[..n])),
            _ => {
                push(Cls::Err, b"https fetch failed (handshake/timeout)");
                return;
            },
        }
    } else {
        let Some(ip) = netstack::dns_resolve(host) else {
            push(Cls::Err, b"could not resolve host");
            return;
        };
        match netstack::http_get(ip, port, path, host, wb) {
            Some(n) => (n, 0usize),
            None => {
                push(Cls::Err, b"fetch failed (connect/non-200/timeout)");
                return;
            },
        }
    };
    if render {
        use bring_browser::{layout, Page};
        static mut PG: Page = Page::new();
        let pg = unsafe { &mut *&raw mut PG };
        layout(&wb[off..len], COLS - 2, pg);
        for l in pg.lines[..pg.line_count].iter() {
            let cls = if l.link != u8::MAX { Cls::Accent } else { Cls::Normal };
            push(cls, l.text());
        }
    } else {
        push_wrapped(Cls::Normal, &wb[off..len]);
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

/// While collecting a sudo/su password: masked entry, Enter submits, Esc
/// cancels, Backspace edits. Returns true if the key was consumed (i.e. we're
/// in password mode). On success, sudo re-runs the pending command and su
/// switches the current user via `lingfs::set_current_user`.
fn handle_pw_key(k: u8) -> bool {
    unsafe {
        if PW_MODE == 0 {
            return false;
        }
        match k {
            b'\n' | b'\r' => {
                let mode = PW_MODE;
                let plen = PENDING_LEN;
                let mut pend = [0u8; INPUT_MAX];
                pend[..plen].copy_from_slice(&(&*&raw const PENDING)[..plen]);
                let pw = core::str::from_utf8(&(&*&raw const PW_INPUT)[..PW_LEN]).unwrap_or("");
                if mode == 1 {
                    let user = lingfs::current_user();
                    if users::verify_login(user, pw) {
                        push(Cls::Dim, b"[sudo] ok");
                        PW_MODE = 0;
                        PW_LEN = 0;
                        PENDING_LEN = 0;
                        run(core::str::from_utf8(&pend[..plen]).unwrap_or(""));
                        return true;
                    }
                    push(Cls::Err, b"sudo: authentication failure");
                } else if mode == 2 {
                    let target = core::str::from_utf8(&pend[..plen]).unwrap_or("root");
                    if users::verify_login(target, pw) {
                        let mut gb = [0u8; 32];
                        let glen = users::group_of(target, &mut gb).unwrap_or(0);
                        let grp = core::str::from_utf8(&gb[..glen]).unwrap_or("users");
                        lingfs::set_current_user(target, grp);
                        let mut m = [0u8; 48];
                        let p = b"now logged in as ";
                        m[..p.len()].copy_from_slice(p);
                        let tn = target.len().min(m.len() - p.len());
                        m[p.len()..p.len() + tn].copy_from_slice(&target.as_bytes()[..tn]);
                        push(Cls::Ok, &m[..p.len() + tn]);
                    } else {
                        push(Cls::Err, b"su: authentication failure");
                    }
                } else if mode == 3 {
                    let new = core::str::from_utf8(&pend[..plen]).unwrap_or("");
                    let old = alloc::string::String::from(lingfs::current_user());
                    match users::rename(&old, new, pw) {
                        Ok(m) => push(Cls::Ok, m.as_bytes()),
                        Err(e) => push(Cls::Err, alloc::format!("rename-user: {e}").as_bytes()),
                    }
                }
                PW_MODE = 0;
                PW_LEN = 0;
                PENDING_LEN = 0;
            },
            0x08 => {
                if PW_LEN > 0 {
                    PW_LEN -= 1;
                }
            },
            0x1B => {
                PW_MODE = 0;
                PW_LEN = 0;
                PENDING_LEN = 0;
                push(Cls::Dim, b"(cancelled)");
            },
            0x20..=0x7E => {
                if PW_LEN < INPUT_MAX {
                    PW_INPUT[PW_LEN] = k;
                    PW_LEN += 1;
                }
            },
            _ => {},
        }
        true
    }
}

pub fn key(k: u8) {
    use crate::drivers::keyboard as kb;
    unsafe {
        if !STARTED {
            STARTED = true;
            banner();
        }
        // While a sudo/su password is being collected, all keys feed the
        // masked entry, not the command line.
        if handle_pw_key(k) {
            return;
        }
        match k {
            kb::CTRL_ZOOM_IN => {
                ZOOM = (ZOOM + 1).min(3);
                save_theme();
            },
            kb::CTRL_ZOOM_OUT => {
                ZOOM = ZOOM.saturating_sub(1).max(1);
                save_theme();
            },
            kb::CTRL_ZOOM_RESET => {
                ZOOM = 1;
                save_theme();
            },
            kb::CTRL_C => {
                // Copy the mouse selection; with none, the last output line.
                if !copy_selection() && COUNT > 0 {
                    let last = (HEAD + LINES - 1) % LINES;
                    clipboard::set(&rows()[last].text[..rows()[last].len]);
                }
            },
            kb::CTRL_A => {
                // select all the scrollback (then Ctrl+C / right-click copies it)
                if COUNT > 0 {
                    SEL_A = Some((TOTAL - COUNT as u64, 0));
                    SEL_B = Some((TOTAL - 1, COLS));
                }
            },
            kb::CTRL_V => paste(),
            0x1B => sel_clear(),
            0x09 => tab_complete(),
            kb::PGUP_KEY => wheel((GRID.5 as i32 - 1).max(1)),
            kb::PGDN_KEY => wheel(-((GRID.5 as i32 - 1).max(1))),
            // Up/Down walk command history (like a real shell), not the
            // scrollback -- new output already snaps the view to the bottom.
            kb::UP_ARROW => history_recall(1),
            kb::DOWN_ARROW => history_recall(-1),
            b'\n' | b'\r' => {
                prompt_echo();
                let mut tmp = [0u8; INPUT_MAX];
                let n = INPUT_LEN;
                tmp[..n].copy_from_slice(&(&*&raw const INPUT)[..n]);
                INPUT_LEN = 0;
                // the disk installer's "type ERASE" question takes this line
                #[cfg(target_arch = "x86_64")]
                if crate::drivers::installer::awaiting() {
                    push_install(crate::drivers::installer::answer(core::str::from_utf8(&tmp[..n]).unwrap_or("")));
                    return;
                }
                history_push(&tmp[..n]);
                run(core::str::from_utf8(&tmp[..n]).unwrap_or(""));
            },
            0x08 => {
                INPUT_LEN = crate::drivers::ime::utf8_pop(&(&*&raw const INPUT)[..], INPUT_LEN);
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

/// UTF-8 typed into the command line (from the input method; the key path only carries ASCII).
pub fn insert_text(b: &[u8]) {
    unsafe {
        if PW_MODE != 0 || INPUT_LEN + b.len() > INPUT_MAX {
            return;
        }
        (&mut *&raw mut INPUT)[INPUT_LEN..INPUT_LEN + b.len()].copy_from_slice(b);
        INPUT_LEN += b.len();
    }
}

fn cls_color(c: Cls) -> u32 {
    let (text, back, accent) = theme_colors();
    match c {
        Cls::Accent => accent,
        Cls::Ok => {
            if is_light(back) {
                0x1F8A45
            } else {
                0x6AC06A
            }
        },
        Cls::Err => {
            if is_light(back) {
                0xC03A28
            } else {
                theme::color(theme::SLOT_ERROR)
            }
        },
        Cls::Dim => mix(text, back, 2, 5),
        Cls::Normal => text,
    }
}

/// Draw the terminal into a window content rect. Grid geometry follows the
/// zoom: bigger glyphs => fewer rows/cols => lower text resolution. The look
/// is the LingStationX design system's: the terminal's own background color,
/// a ROYGBIV key rule along the top, corner brackets, and a zsh-style prompt
/// line (user@host, cwd, and the miao shrine's soul + dirty mark).
pub fn draw(x: u32, y: u32, w: u32, h: u32) {
    unsafe {
        if !STARTED {
            STARTED = true;
            banner();
        }
    }
    let (txt, back, accent) = theme_colors();
    let dim = mix(txt, back, 2, 5);
    // The content area in the terminal's own background (the window body
    // around it keeps the global panel color -- a framed screen).
    framebuffer::back_fill_rect(x, y, w, h, back);
    // ROYGBIV rule: the design system's seven keys, across the top edge.
    let seg_w = w / 7;
    for (i, c) in ROYGBIV.iter().enumerate() {
        let sx = x + i as u32 * seg_w;
        let sw = if i == 6 { w - 6 * seg_w } else { seg_w };
        framebuffer::back_fill_rect(sx, y, sw, 2, *c);
    }
    // Corner brackets (dimmed accent), under the text.
    let bc = mix(accent, back, 2, 5);
    let arm = 10u32.min(w / 4);
    for (cx, cy, dx, dy) in [(x, y + 4, 1i32, 1i32), (x + w - 1, y + 4, -1, 1), (x, y + h - 1, 1, -1), (x + w - 1, y + h - 1, -1, -1)] {
        let hx = if dx > 0 { cx } else { cx - (arm - 1) };
        framebuffer::back_fill_rect(hx, cy, arm, 1, bc);
        let vy = if dy > 0 { cy } else { cy - (arm - 1) };
        framebuffer::back_fill_rect(cx, vy, 1, arm, bc);
    }

    let z = unsafe { ZOOM };
    let gw = 8 * z;
    let gh = 8 * z + 2 * z;
    let cols = ((w / gw).max(1)) as usize;
    pump_llm(cols.saturating_sub(1));
    // a running disk install moves on a slice per frame and reports here
    #[cfg(target_arch = "x86_64")]
    if crate::drivers::installer::running() {
        push_install(crate::drivers::installer::pump());
    }
    // Text starts under the rule; the last two grid rows are the prompt
    // info line and the input line.
    let top = y + 5;
    let grid_rows = (((h.saturating_sub(5)) / gh).max(3)) as usize;
    let out_rows = grid_rows - 2;

    let count = unsafe { COUNT };
    let head = unsafe { HEAD };
    let scroll = unsafe { SCROLL }.min(count.saturating_sub(1));
    // Bottom visible row is (count-1-scroll); show `out_rows` up from there.
    let last = count.saturating_sub(1).saturating_sub(scroll);
    let first = last.saturating_sub(out_rows.saturating_sub(1));
    let abs0 = unsafe { TOTAL } - count as u64; // absolute number of logical row 0
    unsafe { GRID = (x as i64, top as i64, gw as i64, gh as i64, cols, out_rows, abs0 + first as u64) };
    let sel_back = mix(accent, back, 2, 5);
    let mut ry = top;
    for vis in first..=last {
        if count == 0 {
            break;
        }
        // ring index of logical row `vis` (0 = oldest kept)
        let ring = (head + LINES - count + vis) % LINES;
        let r = &rows()[ring];
        let color = cls_color(r.cls);
        let n = r.len.min(cols);
        let sel = sel_cols(abs0 + vis as u64, n);
        if let Some((f, t)) = sel {
            // the selection band: the selected cells, plus a sliver on an empty / short line
            let wcells = (t.min(cols).saturating_sub(f)).max(if t > f || n == 0 { 1 } else { 0 });
            if wcells > 0 {
                let wpx = if t > f { wcells as u32 * gw } else { gw / 2 };
                framebuffer::back_fill_rect(x + f as u32 * gw, ry, wpx, gh, sel_back);
            }
        }
        for (ci, &ch) in r.text[..n].iter().enumerate() {
            let in_sel = matches!(sel, Some((f, t)) if ci >= f && ci < t);
            let bg = if in_sel { sel_back } else { back };
            font8x8::draw_char_scaled(x + ci as u32 * gw, ry, ch, color, bg, z);
        }
        // output in any script (ls of Unicode names, programs that print UTF-8)
        if sel.is_none() {
            font8x8::overlay_utf8(x, ry, &r.text[..n], gw, 8 * z, color, back, cols);
        }
        ry += gh;
    }

    // Prompt info line (zsh-style): user@host  /cwd  (miao:soul +)
    let iy = top + (grid_rows as u32 - 2) * gh;
    {
        let max_x = x + w;
        let mut px = x;
        let mut seg = |s: &[u8], color: u32| {
            for &ch in s {
                if px + gw > max_x {
                    return;
                }
                font8x8::draw_char_scaled(px, iy, ch, color, back, z);
                px += gw;
            }
        };
        let ok_col = if is_light(back) { 0x1F8A45 } else { 0x6AC06A };
        let red = if is_light(back) { 0xC03A28 } else { ROYGBIV[0] };
        let user = lingfs::current_user();
        seg(user.as_bytes(), ok_col);
        seg(b"@", dim);
        let hn = unsafe { &(&*&raw const HOSTNAME)[..HOSTNAME_LEN] };
        seg(if hn.is_empty() { b"lingos" } else { hn }, accent);
        seg(b"  /", red);
        seg(cwd_str().as_bytes(), red);
        let soul = unsafe { &(&*&raw const MIAO_SOUL)[..MIAO_SOUL_LEN] };
        if !soul.is_empty() {
            seg(b"  (", dim);
            seg(b"miao:", ROYGBIV[6]);
            seg(soul, ROYGBIV[4]);
            if unsafe { MIAO_DIRTY } {
                seg(b" +", ROYGBIV[2]);
            }
            seg(b")", dim);
        }
    }

    // Input prompt on the bottom row. In sudo/su password mode, show a
    // "password:" label and mask the entry with asterisks.
    let py = top + (grid_rows as u32 - 1) * gh;
    let pw_mode = unsafe { PW_MODE };
    if pw_mode != 0 {
        let label: &[u8] = b"password:";
        for (ci, &ch) in label.iter().enumerate() {
            font8x8::draw_char_scaled(x + ci as u32 * gw, py, ch, dim, back, z);
        }
        let pl = unsafe { PW_LEN };
        let base = label.len() as u32 + 1;
        let shown = pl.min(cols.saturating_sub(base as usize + 1));
        for i in 0..shown {
            font8x8::draw_char_scaled(x + (base + i as u32) * gw, py, b'*', txt, back, z);
        }
        framebuffer::back_fill_rect(x + (base + shown as u32) * gw, py, gw / 4 + 1, gh, accent);
    } else {
        font8x8::draw_char_scaled(x, py, b'>', accent, back, z);
        let inp = unsafe { &(&*&raw const INPUT)[..INPUT_LEN] };
        let shown = inp.len().min(cols.saturating_sub(3));
        let start = inp.len() - shown;
        for (ci, &ch) in inp[start..].iter().enumerate() {
            font8x8::draw_char_scaled(x + (ci as u32 + 2) * gw, py, ch, txt, back, z);
        }
        font8x8::overlay_utf8(x + 2 * gw, py, &inp[start..], gw, 8 * z, txt, back, cols);
        framebuffer::back_fill_rect(x + (shown as u32 + 2) * gw, py, gw / 4 + 1, gh, accent);
    }
    let _ = input_str;
}
