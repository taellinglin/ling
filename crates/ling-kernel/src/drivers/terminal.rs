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
use crate::drivers::{clipboard, font8x8, framebuffer, lingfu, media, mixer, netstack, theme};
use crate::fs::{lingfs, users};

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

fn run(line: &str) {
    let (cmd, arg) = split_cmd(line.trim());
    match cmd {
        "" => {},
        "help" => {
            push(Cls::Normal, b"commands:");
            push(Cls::Normal, b"  help  clear  echo <t>  ls [dir]  cat <file>");
            push(Cls::Normal, b"  date  theme dark|light  copy <t>  paste");
            push(Cls::Normal, b"  dns <host>   curl <url>   bring <url>");
            push(Cls::Normal, b"  play <file.wav>   stop");
            push(Cls::Normal, b"  lingfu sync|search <q>|install <name>");
            push(Cls::Normal, b"  ling run <file.ling>|eval <src>");
            push(Cls::Normal, b"  cd <dir>   sudo <cmd>   su [user]   ling-life");
            push(Cls::Dim, b"  up/down: command history");
        },
        "clear" => unsafe {
            COUNT = 0;
            HEAD = 0;
            SCROLL = 0;
        },
        "echo" => push(Cls::Normal, arg.as_bytes()),
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
        "cd" => {
            if arg.is_empty() || arg == "/" || arg == "~" || arg == ".." {
                unsafe { CWD_LEN = 0 };
            } else if dir_exists(arg) {
                let b = arg.as_bytes();
                let n = b.len().min(64);
                unsafe {
                    CWD[..n].copy_from_slice(&b[..n]);
                    CWD_LEN = n;
                }
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
            push(Cls::Normal, b"  lingfu sync              refresh the catalog from the repo");
            push(Cls::Normal, b"  lingfu search <query>    find packages");
            push(Cls::Normal, b"  lingfu install <name>    download + install a package");
            push(Cls::Normal, b"  lingfu update            re-sync the catalog");
            push(Cls::Dim, b"  new/build/run/test need the ling compiler (host-only for now)");
        },
        "new" | "init" | "build" | "run" | "test" | "check" | "clean" | "doc" | "fmt"
        | "bench" | "tree" | "publish" | "add" | "manifest" | "wizard" => {
            push(Cls::Err, b"lingfu: that verb needs the ling compiler/toolchain,");
            push(Cls::Err, b"which doesn't run inside LingOS yet (no userspace/loader).");
            push(Cls::Dim, b"Package verbs that DO work here: sync, search, install, update.");
        },
        "sync" | "update" => {
            push(Cls::Dim, b"contacting repo...");
            lingfu::begin_capture();
            lingfu::sync();
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

/// `ling run <file.ling>` / `ling eval <source>` -- run a Ling program with
/// the in-kernel subset interpreter (crate::ling). Output and any error are
/// printed into this window. This is a real interpreter for the language
/// core, not the full compiler (no AOT/native codegen in-kernel yet).
fn run_ling(arg: &str) {
    let (sub, rest) = split_cmd(arg);
    match sub {
        "" | "help" | "--help" | "-h" => {
            push(Cls::Accent, b"ling -- run Ling programs (in-kernel subset interpreter)");
            push(Cls::Normal, b"  ling run <file.ling>     run a program from lingfs");
            push(Cls::Normal, b"  ling eval <source>       run a one-line program");
            push(Cls::Dim, b"  subset: bind, +-*/%, comparisons, if/else, while, fn, strings, print");
        },
        "eval" => {
            if rest.is_empty() {
                push(Cls::Err, b"usage: ling eval <source>");
            } else {
                run_ling_source(rest);
            }
        },
        "demo" => {
            // A built-in program exercising the interpreter end-to-end
            // (functions, recursion, if/return, while, arithmetic, strings).
            run_ling_source(concat!(
                "fn sq(n) { n * n }\n",
                "fn fib(n) { if n < 2 { return n } return fib(n - 1) + fib(n - 2) }\n",
                "bind x = 9\n",
                "print(\"ling running in LingOS\")\n",
                "print(sq(x))\n",
                "print(6 * 7)\n",
                "bind i = 0\n",
                "while i < 5 { print(i) bind i = i + 1 }\n",
                "print(fib(10))\n",
                "if x > 4 { print(\"x is big\") } else { print(\"x is small\") }\n",
            ));
        },
        "run" => {
            if rest.is_empty() {
                push(Cls::Err, b"usage: ling run <file.ling>");
                return;
            }
            let mut pbuf = [0u8; 128];
            let path = resolve_path(rest, &mut pbuf);
            static mut SRCBUF: [u8; 32 * 1024] = [0; 32 * 1024];
            let sb = unsafe { &mut *&raw mut SRCBUF };
            match lingfs::read_file_all(path, sb) {
                Ok(Some(len)) => {
                    let src = core::str::from_utf8(&sb[..len]).unwrap_or("");
                    run_ling_source(src);
                },
                _ => push(Cls::Err, b"ling: no such file"),
            }
        },
        _ => push(Cls::Err, b"ling: unknown subcommand (try: ling run <file> | ling eval <src>)"),
    }
}

fn run_ling_source(src: &str) {
    let (out, err) = crate::ling::run_source(src);
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
                    if users::verify(user, pw) {
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
                    if users::verify(target, pw) {
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
                history_push(&tmp[..n]);
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

    // Input prompt on the bottom row. In sudo/su password mode, show a
    // "password:" label and mask the entry with asterisks.
    let py = y + (grid_rows as u32 - 1) * gh;
    let accent = theme::color(theme::SLOT_ACCENT);
    let txt = theme::color(theme::SLOT_TEXT);
    let pw_mode = unsafe { PW_MODE };
    if pw_mode != 0 {
        let label: &[u8] = b"password:";
        for (ci, &ch) in label.iter().enumerate() {
            font8x8::draw_char_scaled(x + ci as u32 * gw, py, ch, theme::color(theme::SLOT_DIM), panel, z);
        }
        let pl = unsafe { PW_LEN };
        let base = label.len() as u32 + 1;
        let shown = pl.min(cols.saturating_sub(base as usize + 1));
        for i in 0..shown {
            font8x8::draw_char_scaled(x + (base + i as u32) * gw, py, b'*', txt, panel, z);
        }
        framebuffer::back_fill_rect(x + (base + shown as u32) * gw, py, gw / 4 + 1, gh, accent);
    } else {
        font8x8::draw_char_scaled(x, py, b'>', accent, panel, z);
        let inp = unsafe { &(&*&raw const INPUT)[..INPUT_LEN] };
        let shown = inp.len().min(cols.saturating_sub(3));
        let start = inp.len() - shown;
        for (ci, &ch) in inp[start..].iter().enumerate() {
            font8x8::draw_char_scaled(x + (ci as u32 + 2) * gw, py, ch, txt, panel, z);
        }
        framebuffer::back_fill_rect(x + (shown as u32 + 2) * gw, py, gw / 4 + 1, gh, accent);
    }
    let _ = input_str;
}
