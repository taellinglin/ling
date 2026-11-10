//! Unix-style permission bits for files under `home/`: what `chmod` sets over SSH / SFTP, what
//! `ls -l`, `stat` and SFTP listings show, and what makes a file read-only.
//!
//! LingOS files carry no permission bits of their own, so the modes live in a small table in
//! `home/<user>/.modes` (a hidden file: `vfs::list` skips dotfiles). Only a mode that differs from
//! the default (0755 folders, 0644 files) is stored. A file with no owner-write bit is read-only:
//! `vfs::write` and `vfs::delete` refuse it for everyone -- the desktop, apps, SSH and SFTP alike --
//! so `chmod 444 file` really protects it until `chmod 644 file`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

static mut TABLE: Vec<(String, u32)> = Vec::new();
static mut LOADED: bool = false;

const MAX_ENTRIES: usize = 2000;
pub const DEFAULT_DIR: u32 = 0o755;
pub const DEFAULT_FILE: u32 = 0o644;

fn table() -> &'static mut Vec<(String, u32)> {
    unsafe { &mut *(&raw mut TABLE) }
}

fn file_path() -> String {
    let u = crate::fs::lingfs::current_user();
    format!("home/{}/.modes", if u.is_empty() { "ling" } else { u })
}

fn ensure_loaded() {
    unsafe {
        if LOADED || !crate::vfs::ready() {
            return;
        }
        // set first: reading goes through vfs, which asks `writable` about other files
        LOADED = true;
    }
    let Some(bytes) = crate::vfs::read(&file_path()) else { return };
    let Ok(text) = core::str::from_utf8(&bytes) else { return };
    for line in text.lines() {
        let Some((m, p)) = line.split_once('\t') else { continue };
        if let Ok(mode) = u32::from_str_radix(m.trim(), 8) {
            if !p.is_empty() && table().len() < MAX_ENTRIES {
                table().push((String::from(p), mode & 0o7777));
            }
        }
    }
}

fn save() -> bool {
    let mut out = String::new();
    for (p, m) in table().iter() {
        out.push_str(&format!("{m:o}\t{p}\n"));
    }
    crate::vfs::write(&file_path(), out.as_bytes())
}

/// An account was renamed (call after the current user is the new name):
/// entries under `home/<old>/` move to `home/<new>/` and the table is saved
/// into the new home's `.modes`.
pub fn rename_home(old_user: &str, new_user: &str) {
    ensure_loaded();
    let (a, b) = (format!("home/{old_user}/"), format!("home/{new_user}/"));
    let mut changed = false;
    for (p, _) in table().iter_mut() {
        if let Some(rest) = p.strip_prefix(a.as_str()) {
            *p = format!("{b}{rest}");
            changed = true;
        }
    }
    if changed {
        let _ = save();
    }
}

fn key(path: &str) -> String {
    crate::vfs::norm(path)
}

/// The mode of `path` (the default for its kind when none was set).
pub fn get(path: &str, is_dir: bool) -> u32 {
    ensure_loaded();
    let k = key(path);
    table().iter().find(|(p, _)| *p == k).map(|(_, m)| *m).unwrap_or(if is_dir { DEFAULT_DIR } else { DEFAULT_FILE })
}

/// Set the mode of `path` (the low 12 bits of `mode`) and keep it.
pub fn set(path: &str, mode: u32, is_dir: bool) -> bool {
    ensure_loaded();
    let k = key(path);
    let mode = mode & 0o7777;
    let default = if is_dir { DEFAULT_DIR } else { DEFAULT_FILE };
    table().retain(|(p, _)| *p != k);
    if mode != default {
        if table().len() >= MAX_ENTRIES {
            return false;
        }
        table().push((k, mode));
    }
    save()
}

/// Is `path` open for writing and deleting (its owner-write bit is set)?
pub fn writable(path: &str) -> bool {
    ensure_loaded();
    let k = key(path);
    match table().iter().find(|(p, _)| *p == k) {
        Some((_, m)) => m & 0o200 != 0,
        None => true,
    }
}

/// A deleted path (and everything under it) forgets its mode.
pub fn forget(path: &str) {
    ensure_loaded();
    let k = key(path);
    let under = format!("{k}/");
    let before = table().len();
    table().retain(|(p, _)| *p != k && !p.starts_with(&under));
    if table().len() != before {
        let _ = save();
    }
}

/// A moved / renamed path (and everything under it) keeps its mode at the new place.
pub fn moved(from: &str, to: &str) {
    ensure_loaded();
    let (a, b) = (key(from), key(to));
    let under = format!("{a}/");
    let mut changed = false;
    for (p, _) in table().iter_mut() {
        if *p == a {
            *p = b.clone();
            changed = true;
        } else if let Some(rest) = p.strip_prefix(&under) {
            *p = format!("{b}/{rest}");
            changed = true;
        }
    }
    if changed {
        let _ = save();
    }
}

/// "drwxr-xr-x" / "-rw-r--r--" for `ls -l`.
pub fn symbolic(mode: u32, is_dir: bool) -> String {
    let mut s = String::with_capacity(10);
    s.push(if is_dir { 'd' } else { '-' });
    for sh in [6, 3, 0] {
        let m = (mode >> sh) & 7;
        s.push(if m & 4 != 0 { 'r' } else { '-' });
        s.push(if m & 2 != 0 { 'w' } else { '-' });
        s.push(if m & 1 != 0 { 'x' } else { '-' });
    }
    s
}

/// Parse a chmod argument: octal ("644", "0755") or symbolic ("u+w", "a-w", "go-rwx", "+x", "u=rw",
/// comma-separated), applied to `current`. None when it makes no sense.
pub fn parse_chmod(spec: &str, current: u32) -> Option<u32> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    if spec.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
        return u32::from_str_radix(spec, 8).ok().filter(|m| *m <= 0o7777);
    }
    let mut mode = current;
    for part in spec.split(',') {
        let op_at = part.find(['+', '-', '='])?;
        let who = &part[..op_at];
        let op = part.as_bytes()[op_at];
        let perms = &part[op_at + 1..];
        let mut bits = 0u32;
        for c in perms.chars() {
            bits |= match c {
                'r' => 4,
                'w' => 2,
                'x' => 1,
                _ => return None,
            };
        }
        let who = if who.is_empty() { "a" } else { who };
        let mut mask = 0u32;
        for c in who.chars() {
            mask |= match c {
                'u' => bits << 6,
                'g' => bits << 3,
                'o' => bits,
                'a' => (bits << 6) | (bits << 3) | bits,
                _ => return None,
            };
        }
        let who_mask = {
            let mut m = 0u32;
            for c in who.chars() {
                m |= match c {
                    'u' => 0o700,
                    'g' => 0o070,
                    'o' => 0o007,
                    'a' => 0o777,
                    _ => return None,
                };
            }
            m
        };
        mode = match op {
            b'+' => mode | mask,
            b'-' => mode & !mask,
            _ => (mode & !who_mask) | mask,
        };
    }
    Some(mode & 0o7777)
}
