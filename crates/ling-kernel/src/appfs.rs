//! Read-only files baked into the kernel image.
//!
//! `ling build` turns a kernel project's `[includes]` into `include_bytes!`
//! calls that [`register`] each file under its path relative to the project's
//! `embed_root` — e.g. `assets/font.ttf` for an app whose sources ask for exactly
//! that. Registration runs right after `.bss` is zeroed and before the program
//! starts, so this is a fixed static table: no heap needed yet.
//!
//! It stands in for a filesystem until LingOS can read the SD card: the Ling
//! runtime's `file_exists` / `read_file` / `font_load` / `fs_list` look here.

const MAX_FILES: usize = 4096;

static mut FILES: [(&str, &[u8]); MAX_FILES] = [("", &[]); MAX_FILES];
static mut COUNT: usize = 0;

/// Called by the generated `embedded::register()` for each baked-in file.
pub fn register(name: &'static str, data: &'static [u8]) {
    unsafe {
        if COUNT < MAX_FILES {
            (*(&raw mut FILES))[COUNT] = (name, data);
            COUNT += 1;
        }
    }
}

fn files() -> &'static [(&'static str, &'static [u8])] {
    unsafe { &(&*(&raw const FILES))[..COUNT] }
}

/// Strip leading `./` and turn `\` into `/`-equivalent matching.
fn same_path(stored: &str, asked: &str) -> bool {
    let mut a = asked;
    while let Some(rest) = a.strip_prefix("./") {
        a = rest;
    }
    if stored.len() != a.len() {
        return false;
    }
    stored.bytes().zip(a.bytes()).all(|(s, q)| s == q || (s == b'/' && q == b'\\'))
}

/// The contents of an embedded file, if `path` names one.
pub fn get(path: &str) -> Option<&'static [u8]> {
    files().iter().find(|(n, _)| same_path(n, path)).map(|(_, d)| *d)
}

pub fn exists(path: &str) -> bool {
    get(path).is_some()
}

/// Every embedded file's path, in registration order.
pub fn each_file(mut f: impl FnMut(&str)) {
    for (n, _) in files() {
        f(n);
    }
}

/// Is `path` a directory — i.e. a prefix of some embedded file?
pub fn is_dir(path: &str) -> bool {
    let p = path.trim_start_matches("./").trim_end_matches('/');
    if p.is_empty() || p == "." {
        return true;
    }
    files().iter().any(|(n, _)| n.len() > p.len() && n.starts_with(p) && n.as_bytes()[p.len()] == b'/')
}

/// Immediate children (files and sub-directories) of directory `path`.
pub fn list(path: &str, mut each: impl FnMut(&str, bool)) {
    let p = path.trim_start_matches("./").trim_end_matches('/');
    let prefix_len = if p.is_empty() || p == "." { 0 } else { p.len() + 1 };
    let mut seen_dirs: [&str; 64] = [""; 64];
    let mut nd = 0;
    for (n, _) in files() {
        if prefix_len > 0 && !(n.starts_with(p) && n.len() > p.len() && n.as_bytes()[p.len()] == b'/') {
            continue;
        }
        let rest = &n[prefix_len..];
        match rest.find('/') {
            None => each(rest, false),
            Some(i) => {
                let d = &rest[..i];
                if !seen_dirs[..nd].contains(&d) {
                    if nd < seen_dirs.len() {
                        seen_dirs[nd] = d;
                        nd += 1;
                    }
                    each(d, true);
                }
            },
        }
    }
}
