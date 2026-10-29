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
/// The running app's folder in the image (`apps/<name>`): its embedded files
/// by their own relative names, beside any unprefixed ones.
static mut IMG: Option<String> = None;
/// Where the running app's `projects/...` paths live (`home/<user>/Projects/<App>`).
static mut PROJECTS: Option<String> = None;

/// A program's whole path scope (data root, image folder, projects folder).
pub struct Scope(Option<String>, Option<String>, Option<String>);

/// Is there writable storage (a mounted lingfs volume)?
pub fn ready() -> bool {
    crate::fs::lingfs::is_mounted()
}

/// The signed-in user's home folder ("ling", the image's built-in home, until
/// storage is mounted and a user signed in).
/// With the SD card's data partition mounted (`media/card`), the files under
/// `home/` live there: `home/ling/Music/x` is `media/card/home/ling/Music/x`.
/// Reads fall back to lingfs (anything stored before the partition existed),
/// listings merge both. Accounts, the keyring and settings stay in lingfs.
fn card_path(r: &str) -> Option<String> {
    if (r == "home" || r.starts_with("home/")) && crate::fs::media::has("card") {
        return Some(format!("media/card/{r}"));
    }
    // Installed packages (`pkg-<name>/...`, the `packages/<name>` version
    // records) live on the package volume when there is one -- lingfs is a
    // few MiB, packages are often 100+ MB. Same fallback as home/: reads
    // still find a package lingfs got before the volume existed.
    if is_package_path(r) {
        if let Some(v) = package_volume() {
            return Some(format!("media/{v}/{r}"));
        }
    }
    None
}

fn is_package_path(r: &str) -> bool {
    r.starts_with("pkg-") || r == "packages" || r.starts_with("packages/")
}

/// The FAT volume packages install to: the Pi's SD-card data partition
/// (`media/card`), else x86's second disk (`media/disk1`); None = lingfs.
pub fn package_volume() -> Option<&'static str> {
    if crate::fs::media::has("card") {
        Some("card")
    } else if crate::fs::media::has("disk1") {
        Some("disk1")
    } else {
        None
    }
}

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

/// `apps/<name>` when the image holds that app's files, else nothing.
pub fn set_image_root(r: &str) {
    let r = norm(r);
    unsafe { *(&raw mut IMG) = if !r.is_empty() && crate::appfs::is_dir(&r) { Some(r) } else { None } };
}

pub fn set_projects_root(r: &str) {
    unsafe { *(&raw mut PROJECTS) = if r.is_empty() { None } else { Some(norm(r)) } };
}

/// The projects folder of the running app, if it has one.
pub fn projects_root() -> Option<String> {
    unsafe { (*(&raw const PROJECTS)).clone() }
}

/// Clear the scope for a launched program, returning the caller's to restore.
pub fn take_scope() -> Scope {
    unsafe { Scope((*(&raw mut ROOT)).take(), (*(&raw mut IMG)).take(), (*(&raw mut PROJECTS)).take()) }
}

pub fn restore_scope(s: Scope) {
    unsafe {
        *(&raw mut ROOT) = s.0;
        *(&raw mut IMG) = s.1;
        *(&raw mut PROJECTS) = s.2;
    }
}

/// `n` without the running program's data root (`<root>/x` -> `x`): apps
/// build paths from their `fs_cwd()`, which must still find bundled files.
fn under_root(n: &str) -> Option<String> {
    let r = root();
    if r.is_empty() {
        return None;
    }
    n.strip_prefix(&format!("{r}/")).map(String::from)
}

/// The image path of `n` (or of `n` relative to the data root) inside the
/// running app's folder, when it exists.
fn img(n: &str) -> Option<String> {
    let base = unsafe { (*(&raw const IMG)).as_ref()? };
    let mut cands: Vec<String> = alloc::vec![join(base, n)];
    if let Some(rel) = under_root(n) {
        cands.push(join(base, &rel));
        cands.push(rel);
    }
    cands.into_iter().find(|p| crate::appfs::exists(p) || crate::appfs::is_dir(p))
}

/// `projects/...` of the running app, moved to its Projects folder.
fn projects_path(n: &str) -> Option<String> {
    let base = unsafe { (*(&raw const PROJECTS)).as_ref()? };
    if n == "projects" {
        Some(base.clone())
    } else {
        n.strip_prefix("projects/").map(|rest| join(base, rest))
    }
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
    // `<data root>/projects/...` is the app's Projects folder too (apps build
    // project paths from their `fs_cwd()`).
    if let Some(rel) = under_root(&n) {
        if let Some(p) = projects_path(&rel) {
            return p;
        }
    }
    if n == "home" || n.starts_with("home/") || n.starts_with("pkg-") || n == "packages" || n.starts_with("packages/") {
        return n;
    }
    if let Some(p) = projects_path(&n) {
        return p;
    }
    if ready() && crate::fs::lingfs::path_kind(&n).is_some() {
        return n;
    }
    if crate::appfs::exists(&n) || crate::appfs::is_dir(&n) {
        return n;
    }
    if let Some(p) = img(&n) {
        return p;
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
    if let Some(c) = card_path(&r) {
        if let Some(v) = crate::fs::media::read(&c) {
            return unseal_private(&r, v);
        }
    }
    if ready() {
        if let Some(v) = lingfs_read(&r) {
            return unseal_private(&r, v);
        }
        if r != n {
            if let Some(v) = lingfs_read(&n) {
                return unseal_private(&n, v);
            }
        }
    }
    crate::appfs::get(&n)
        .or_else(|| crate::appfs::get(&r))
        .or_else(|| img(&n).and_then(|p| crate::appfs::get(&p)))
        .map(|b| b.to_vec())
}

/// Write a file (creating its folders); false without storage.
pub fn write(p: &str, data: &[u8]) -> bool {
    if crate::fs::media::is_media_path(&norm(p)) {
        return crate::fs::media::write(&norm(p), data);
    }
    let r = resolve(p);
    if let Some(c) = card_path(&r) {
        if let Some(i) = c.rfind('/') {
            crate::fs::media::mkdir_p(&c[..i]);
        }
        if crate::keyring::is_private_path(&r) {
            let Some(blob) = crate::keyring::seal("vfs-private", r.as_bytes(), data) else { return false };
            return crate::fs::media::write(&c, &blob);
        }
        return crate::fs::media::write(&c, data);
    }
    if !ready() {
        return false;
    }
    if r.is_empty() {
        return false;
    }
    if let Some(i) = r.rfind('/') {
        let _ = crate::fs::lingfs::mkdir_p(&r[..i]);
    }
    if crate::keyring::is_private_path(&r) {
        // sealed under the signed-in user's keyring; refused while locked
        let Some(blob) = crate::keyring::seal("vfs-private", r.as_bytes(), data) else { return false };
        return crate::fs::lingfs::write_path_any(&r, &blob).is_ok();
    }
    crate::fs::lingfs::write_path_any(&r, data).is_ok()
}

/// A private file's bytes come back decrypted (None while the keyring is
/// locked or on a wrong key); a file written there before the keyring
/// existed is plain and passes through.
fn unseal_private(path: &str, bytes: Vec<u8>) -> Option<Vec<u8>> {
    if !crate::keyring::is_private_path(path) || !crate::keyring::is_sealed(&bytes) {
        return Some(bytes);
    }
    crate::keyring::open("vfs-private", path.as_bytes(), &bytes)
}

pub fn exists(p: &str) -> bool {
    let n = norm(p);
    if crate::fs::media::is_media_path(&n) {
        return crate::fs::media::exists(&n);
    }
    let r = resolve(&n);
    if card_path(&r).is_some_and(|c| crate::fs::media::exists(&c)) {
        return true;
    }
    if ready() && (crate::fs::lingfs::path_kind(&r).is_some() || crate::fs::lingfs::path_kind(&n).is_some()) {
        return true;
    }
    crate::appfs::exists(&n) || crate::appfs::is_dir(&n) || crate::appfs::exists(&r) || img(&n).is_some() || (is_model_path(&n) && locate_model(&n).is_some())
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
    if card_path(&r).is_some_and(|c| crate::fs::media::is_dir(&c)) {
        return true;
    }
    if ready() && (crate::fs::lingfs::path_kind(&r) == Some(true) || crate::fs::lingfs::path_kind(&n) == Some(true)) {
        return true;
    }
    crate::appfs::is_dir(&n) || crate::appfs::is_dir(&r) || img(&n).is_some_and(|p| crate::appfs::is_dir(&p))
}

pub fn mkdir(p: &str) -> bool {
    if crate::fs::media::is_media_path(&norm(p)) {
        return crate::fs::media::mkdir(&norm(p));
    }
    if let Some(c) = card_path(&resolve(p)) {
        return crate::fs::media::mkdir_p(&c);
    }
    ready() && crate::fs::lingfs::mkdir_p(&resolve(p)).is_ok()
}

/// Delete a file or an empty folder, on removable media or the SD card's
/// lingfs (the image's built-in files stay).
pub fn delete(p: &str) -> bool {
    let n = norm(p);
    if crate::fs::media::is_media_path(&n) {
        return crate::fs::media::delete(&n);
    }
    if is_dir(&n) && !list(&n).is_empty() {
        return false;
    }
    if let Some(c) = card_path(&resolve(&n)) {
        if crate::fs::media::exists(&c) {
            return crate::fs::media::delete(&c);
        }
    }
    if !ready() {
        return false;
    }
    let r = resolve(&n);
    if crate::fs::lingfs::remove_path(&r).unwrap_or(false) {
        return true;
    }
    r != n && crate::fs::lingfs::remove_path(&n).unwrap_or(false)
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
    if let Some(c) = card_path(&r) {
        for (name, d, size) in crate::fs::media::list(&c) {
            push_entry(&mut items, &name, d, size);
        }
    }
    // The root also shows the packages installed on the package volume.
    if n.is_empty() {
        if let Some(v) = package_volume() {
            for (name, d, size) in crate::fs::media::list(&format!("media/{v}")) {
                if is_package_path(&name) {
                    push_entry(&mut items, &name, d, size);
                }
            }
        }
    }
    if ready() {
        lingfs_list_into(&r, &mut items);
        if r != n {
            lingfs_list_into(&n, &mut items);
        }
    }
    let mut dirs: Vec<String> = alloc::vec![n.clone()];
    if r != n {
        dirs.push(r.clone());
    }
    if let Some(p) = img(&n) {
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }
    for dir in &dirs {
        let base = dir.clone();
        crate::appfs::list(dir, |name, is_dir| {
            let size = if is_dir { 0 } else { crate::appfs::get(&join(&base, name)).map(|d| d.len()).unwrap_or(0) };
            push_entry(&mut items, name, is_dir, size);
        });
    }
    items.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.to_lowercase().cmp(&y.0.to_lowercase())));
    items
}

// ── big files read in pieces (language models) ─────────────────────────────

/// Is `n` a model path the search below applies to: a relative path with a
/// `models` folder in it (`assets/models/Qwen3-0.6B-Q8_0.gguf`, `models/x`)?
fn is_model_path(n: &str) -> bool {
    !crate::fs::media::is_media_path(n) && !n.starts_with("home/") && n.split('/').any(|s| s.eq_ignore_ascii_case("models"))
}

/// Where a model file an app names by a relative path actually is, on a FAT
/// volume (`media/<vol>/...`). Models are hundreds of MB to GBs, so they live
/// on the SD card's data partition (Pi) or the second disk (x86), never in
/// lingfs or the image. The search, first hit wins:
///   1. the path as programs resolve it: the app's data folder
///      (`home/<user>/Programs/<App>/assets/models/...`, on the card on the Pi)
///   2. an installed package that carries it: `media/<vol>/pkg-*/<path>`
///   3. the volume's root: `media/<vol>/<path>` (`media/card/assets/models/...`)
///   4. a plain models folder: `media/<vol>/models/<file name>`
/// over every mounted volume (card, disk1, usb0, ...). An absolute
/// `media/...` or `home/...` path (Settings > AI > Browse) is taken as is.
pub fn locate_model(p: &str) -> Option<String> {
    let n = norm(p);
    if crate::fs::media::is_media_path(&n) {
        return crate::fs::media::exists(&n).then_some(n);
    }
    let r = resolve(&n);
    if let Some(c) = card_path(&r) {
        if crate::fs::media::exists(&c) && !crate::fs::media::is_dir(&c) {
            return Some(c);
        }
    }
    let rel = under_root(&n).unwrap_or(n);
    if !is_model_path(&rel) {
        return None;
    }
    let base = basename(&rel);
    for vol in crate::fs::media::names() {
        let v = format!("media/{vol}");
        for (name, d, _) in crate::fs::media::list(&v) {
            if d && name.starts_with("pkg-") {
                let c = format!("{v}/{name}/{rel}");
                if crate::fs::media::exists(&c) {
                    return Some(c);
                }
            }
        }
        for c in [format!("{v}/{rel}"), format!("{v}/models/{base}")] {
            if crate::fs::media::exists(&c) && !crate::fs::media::is_dir(&c) {
                return Some(c);
            }
        }
    }
    None
}
