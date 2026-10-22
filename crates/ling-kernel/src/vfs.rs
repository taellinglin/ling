//! Files for programs: lingfs (the disk on x86; the SD card on the Pi once it
//! is mounted) in front of the image's read-only `appfs`. Paths are
//! `/`-separated from the filesystem root. A program's relative paths resolve
//! under its own data folder (`set_root`): LingStationX's `projects/…` and
//! `settings.txt` land in `home/<user>/Programs/LingStationX/`, while its
//! bundled assets still come from the image by their embedded names, and a
//! `ling run` script works beside its own file.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

static mut ROOT: Option<String> = None;

/// Is there writable storage (a mounted lingfs volume)?
pub fn ready() -> bool {
    crate::fs::lingfs::is_mounted()
}

/// The signed-in user's home folder ("ling", the image's built-in home, until
/// storage is mounted and a user signed in).
pub fn home() -> String {
    let u = crate::fs::lingfs::current_user();
    if u.is_empty() || !ready() {
        String::from("home/ling")
    } else {
        format!("home/{u}")
    }
}

/// The running program's data folder (relative paths resolve under it).
pub fn root() -> String {
    unsafe { (*(&raw const ROOT)).clone() }.unwrap_or_else(home)
}

pub fn set_root(r: &str) {
    unsafe { *(&raw mut ROOT) = if r.is_empty() { None } else { Some(norm(r)) } };
}

pub fn take_root() -> Option<String> {
    unsafe { (*(&raw mut ROOT)).take() }
}

pub fn restore_root(r: Option<String>) {
    unsafe { *(&raw mut ROOT) = r };
}

/// Strip `./` and leading `/`, collapse `.` and `..` segments, `\` → `/`.
pub fn norm(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split(['/', '\\']) {
        match seg {
            "" | "." => {},
            ".." => {
                parts.pop();
            },
            s => parts.push(s),
        }
    }
    parts.join("/")
}

pub fn join(a: &str, b: &str) -> String {
    if a.is_empty() {
        norm(b)
    } else if b.is_empty() {
        norm(a)
    } else {
        norm(&format!("{a}/{b}"))
    }
}

pub fn parent(p: &str) -> String {
    let n = norm(p);
    match n.rfind('/') {
        Some(i) => String::from(&n[..i]),
        None => String::new(),
    }
}

/// Where a program's path lives: the home tree, packages and any existing
/// lingfs path as given; the image's own files by their embedded name;
/// anything else under the program's data root.
pub fn resolve(p: &str) -> String {
    let n = norm(p);
    if n.is_empty() {
        return root();
    }
    if n == "home" || n.starts_with("home/") || n.starts_with("pkg-") || n == "packages" || n.starts_with("packages/") {
        return n;
    }
    if ready() && crate::fs::lingfs::path_kind(&n).is_some() {
        return n;
    }
    if crate::appfs::exists(&n) || crate::appfs::is_dir(&n) {
        return n;
    }
    let r = root();
    if r.is_empty() || n == r || n.starts_with(&format!("{r}/")) {
        n
    } else {
        format!("{r}/{n}")
    }
}

fn lingfs_read(path: &str) -> Option<Vec<u8>> {
    use crate::fs::lingfs;
    if lingfs::path_kind(path) != Some(false) {
        return None;
    }
    let size = lingfs::file_size(path)?;
    let mut buf = alloc::vec![0u8; size];
    match lingfs::read_file_all(path, &mut buf) {
        Ok(Some(n)) => {
            buf.truncate(n);
            Some(buf)
        },
        _ => None,
    }
}

/// A file's bytes: the program's copy on storage first, then the image's.
pub fn read(p: &str) -> Option<Vec<u8>> {
    let n = norm(p);
    if crate::fs::media::is_media_path(&n) {
        return crate::fs::media::read(&n);
    }
    let r = resolve(&n);
    if ready() {
        if let Some(v) = lingfs_read(&r) {
            return Some(v);
        }
        if r != n {
            if let Some(v) = lingfs_read(&n) {
                return Some(v);
            }
        }
    }
    crate::appfs::get(&n).or_else(|| crate::appfs::get(&r)).map(|b| b.to_vec())
}

/// Write a file (creating its folders); false without storage.
pub fn write(p: &str, data: &[u8]) -> bool {
    if crate::fs::media::is_media_path(&norm(p)) {
        return crate::fs::media::write(&norm(p), data);
    }
    if !ready() {
        return false;
    }
    let r = resolve(p);
    if r.is_empty() {
        return false;
    }
    if let Some(i) = r.rfind('/') {
        let _ = crate::fs::lingfs::mkdir_p(&r[..i]);
    }
    crate::fs::lingfs::write_path_any(&r, data).is_ok()
}

pub fn exists(p: &str) -> bool {
    let n = norm(p);
    if crate::fs::media::is_media_path(&n) {
        return crate::fs::media::exists(&n);
    }
    let r = resolve(&n);
    if ready() && (crate::fs::lingfs::path_kind(&r).is_some() || crate::fs::lingfs::path_kind(&n).is_some()) {
        return true;
    }
    crate::appfs::exists(&n) || crate::appfs::is_dir(&n) || crate::appfs::exists(&r)
}

pub fn is_dir(p: &str) -> bool {
    let n = norm(p);
    if n.is_empty() {
        return true;
    }
    if crate::fs::media::is_media_path(&n) {
        return crate::fs::media::is_dir(&n);
    }
    let r = resolve(&n);
    if ready() && (crate::fs::lingfs::path_kind(&r) == Some(true) || crate::fs::lingfs::path_kind(&n) == Some(true)) {
        return true;
    }
    crate::appfs::is_dir(&n) || crate::appfs::is_dir(&r)
}

pub fn mkdir(p: &str) -> bool {
    if crate::fs::media::is_media_path(&norm(p)) {
        return crate::fs::media::mkdir(&norm(p));
    }
    ready() && crate::fs::lingfs::mkdir_p(&resolve(p)).is_ok()
}

/// Delete a file or an empty folder (removable media; the image's files and
/// lingfs's are kept).
pub fn delete(p: &str) -> bool {
    let n = norm(p);
    if crate::fs::media::is_media_path(&n) {
        return crate::fs::media::delete(&n);
    }
    false
}

pub fn basename(p: &str) -> String {
    let n = norm(p);
    match n.rfind('/') {
        Some(i) => String::from(&n[i + 1..]),
        None => n,
    }
}

/// Where `cp`/`mv` put `src` for a `dst` argument: inside `dst` when it is an
/// existing folder, else at `dst` itself.
pub fn target_for(src: &str, dst: &str) -> String {
    if is_dir(dst) && !norm(dst).is_empty() || norm(dst).is_empty() {
        join(dst, &basename(src))
    } else {
        norm(dst)
    }
}

/// Copy a file, or a folder with everything in it.
pub fn copy(from: &str, to: &str) -> bool {
    let (a, b) = (norm(from), norm(to));
    if a == b {
        return true;
    }
    if is_dir(&a) {
        if !is_dir(&b) && !mkdir(&b) {
            return false;
        }
        let mut ok = true;
        for (name, _, _) in list(&a) {
            ok &= copy(&join(&a, &name), &join(&b, &name));
        }
        return ok;
    }
    match read(&a) {
        Some(data) => write(&b, &data),
        None => false,
    }
}

/// Move / rename: in place inside one removable volume, else copy + delete.
pub fn rename(from: &str, to: &str) -> bool {
    let (a, b) = (norm(from), norm(to));
    if crate::fs::media::is_media_path(&a) && crate::fs::media::is_media_path(&b) && crate::fs::media::rename(&a, &b) {
        return true;
    }
    copy(&a, &b) && delete_tree(&a)
}

fn delete_tree(p: &str) -> bool {
    if is_dir(p) {
        for (name, _, _) in list(p) {
            delete_tree(&join(p, &name));
        }
    }
    delete(p)
}

fn push_entry(items: &mut Vec<(String, bool, usize)>, name: &str, is_dir: bool, size: usize) {
    if name.is_empty() || name.starts_with('.') || items.iter().any(|(n, _, _)| n == name) {
        return;
    }
    items.push((String::from(name), is_dir, size));
}

fn lingfs_list_into(dir: &str, items: &mut Vec<(String, bool, usize)>) {
    use crate::fs::lingfs;
    let mut i = 0usize;
    let mut nb = [0u8; 96];
    while let Some((len, is_dir)) = lingfs::list_entry(dir, i, &mut nb) {
        i += 1;
        if len == 0 {
            continue;
        }
        let Ok(name) = core::str::from_utf8(&nb[..len.min(nb.len())]) else { continue };
        let name = String::from(name);
        let size = if is_dir { 0 } else { lingfs::file_size(&join(dir, &name)).unwrap_or(0) };
        push_entry(items, &name, is_dir, size);
        if i > 4096 {
            break;
        }
    }
}

/// A folder's entries `(name, is_dir, size)`: storage and image merged, folders
/// first, names case-insensitive; dotfiles hidden.
pub fn list(p: &str) -> Vec<(String, bool, usize)> {
    let n = norm(p);
    if crate::fs::media::is_media_path(&n) {
        return crate::fs::media::list(&n);
    }
    let r = resolve(&n);
    let mut items: Vec<(String, bool, usize)> = Vec::new();
    if n.is_empty() && !crate::fs::media::names().is_empty() {
        items.push((String::from("media"), true, 0));
    }
    if ready() {
        lingfs_list_into(&r, &mut items);
        if r != n {
            lingfs_list_into(&n, &mut items);
        }
    }
    for dir in [n.as_str(), r.as_str()] {
        let base = String::from(dir);
        crate::appfs::list(dir, |name, is_dir| {
            let size = if is_dir { 0 } else { crate::appfs::get(&join(&base, name)).map(|d| d.len()).unwrap_or(0) };
            push_entry(&mut items, name, is_dir, size);
        });
        if r == n {
            break;
        }
    }
    items.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.to_lowercase().cmp(&y.0.to_lowercase())));
    items
}
