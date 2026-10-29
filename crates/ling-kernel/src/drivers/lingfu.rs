//! lingfu, the package manager's real network client: catalog sync,
//! search, and install over genuine HTTP through `netstack` -- the wire
//! is real (pcap-verifiable), the server is real (any HTTP server the
//! repo URL points at), the install is the existing `.lpkg` unpack. What
//! remains a hosting decision, per packages/README.md, is who runs the
//! public repo: the default URL is QEMU SLIRP's host alias (10.0.2.2:8000
//! -- `python -m http.server 8000` beside the repo files makes the host
//! the repo), overridable by writing "a.b.c.d:port" into lingfs `/repo`.
//!
//! Catalog format, deliberately trivial (one line per package):
//! `name version filename.lpkg description words...`
//!
//! Registry installs STREAM (see `install_registry`): TLS records -> BLAKE3
//! (checked against the registry's checksum) -> gzip inflate -> tar -> disk,
//! so a download's size is bounded by the disk, not by RAM. Packages go to the
//! package volume when one is mounted (`vfs::package_volume`: the Pi's SD-card
//! data partition `media/card`, else x86's second disk `media/disk1`, both
//! FAT32 -- files up to 4 GiB, space checked up front), else into lingfs,
//! which is small (MAX_OBJECTS 4 KiB blocks = 16 MiB per volume, append-only,
//! 32 entries per folder, ~64 MiB per file). Windows/macOS/Linux native
//! binaries inside a package are not installed. No signatures yet
//! (packages/README step 5).

use crate::drivers::netstack;
use crate::fs::{lingfs, packages};
use alloc::vec::Vec;

/// The normalized catalog (and the raw JSON it is parsed from). The registry's
/// /api/packages was ~7 KiB for 47 packages with full descriptions; 64 KiB
/// leaves room for a few hundred.
const CATALOG_MAX: usize = 64 * 1024;
/// Description bytes kept per catalog entry: whole descriptions, so the
/// "LingOS app" markers (`for_lingos`) are seen even late in a long one.
const DESC_MAX: usize = 600;
static mut CATALOG: [u8; CATALOG_MAX] = [0; CATALOG_MAX];
static mut CATALOG_LEN: usize = 0;
/// Scratch for the raw HTTP response body before it's normalized into the
/// tab-delimited CATALOG (JSON from the official registry, or a plaintext
/// `catalog.txt` from a local /repo override). Separate from CATALOG so the
/// normalizer can read raw input while writing normalized output.
static mut RAW: [u8; CATALOG_MAX] = [0; CATALOG_MAX];

// Normalized catalog line format, one package per line, TAB-delimited so
// descriptions may contain spaces (and the fields stay index-stable even
// when one is empty -- splitters must NOT drop empty fields):
//   name \t meta \t filename \t description
// `meta` is the download count (official) or version (local /repo). `filename`
// is the installable `.lpkg` for a local repo, empty for the official registry
// (which has no public binary-download endpoint -- list/search work, install
// needs a local /repo). See `normalize_json` / `normalize_catalog_txt`.
const FSEP: u8 = b'\t';

fn parse_addr(s: &[u8]) -> Option<([u8; 4], u16)> {
    let mut ip = [0u8; 4];
    let mut part = 0usize;
    let mut acc: u32 = 0;
    let mut any = false;
    let mut port: u32 = 0;
    let mut in_port = false;
    for &b in s {
        match b {
            b'0'..=b'9' => {
                acc = acc * 10 + (b - b'0') as u32;
                if in_port {
                    port = acc;
                }
                any = true;
            },
            b'.' if !in_port && part < 3 => {
                ip[part] = acc.min(255) as u8;
                part += 1;
                acc = 0;
            },
            b':' if part == 3 => {
                ip[3] = acc.min(255) as u8;
                part += 1;
                acc = 0;
                in_port = true;
            },
            b'\n' | b'\r' | b' ' => break,
            _ => return None,
        }
    }
    if part == 4 && in_port && any && port > 0 && port < 65536 {
        Some((ip, port as u16))
    } else {
        None
    }
}

/// Output capture, so the graphics Terminal app can show lingfu's output in
/// its own window instead of the text console. When capturing, `print`
/// appends into `CAP` (bounded); otherwise it goes to the console as before,
/// keeping the text-mode `lsh` path unchanged.
static mut CAP: [u8; 32 * 1024] = [0; 32 * 1024];
static mut CAP_LEN: usize = 0;
static mut CAPTURING: bool = false;

pub fn begin_capture() {
    unsafe {
        CAPTURING = true;
        CAP_LEN = 0;
    }
}

pub fn end_capture() -> &'static [u8] {
    unsafe {
        CAPTURING = false;
        &(&*&raw const CAP)[..CAP_LEN]
    }
}

/// True once a catalog has been synced this session.
pub fn synced() -> bool {
    unsafe { CATALOG_LEN > 0 }
}

/// The raw synced catalog bytes (one package per line), for the GUI package
/// manager to parse and render. Empty until `sync()` succeeds.
pub fn catalog_raw() -> &'static [u8] {
    catalog()
}

fn print(s: &[u8]) {
    unsafe {
        if CAPTURING {
            let cap = &mut *&raw mut CAP;
            let room = cap.len().saturating_sub(CAP_LEN);
            let n = s.len().min(room);
            cap[CAP_LEN..CAP_LEN + n].copy_from_slice(&s[..n]);
            CAP_LEN += n;
            // Long installs from a GUI window: keep a serial trace too.
            #[cfg(target_arch = "x86_64")]
            crate::drivers::serial::write(s);
            #[cfg(target_arch = "aarch64")]
            crate::drivers::uart::write(s);
            return;
        }
    }
    crate::console_write(s);
}

/// The official package registry, fetched over HTTPS (TLS 1.3).
const OFFICIAL_HOST: &str = "fu.ling-lang.org";

/// Local repo override address from lingfs `/repo` ("a.b.c.d:port"), if set.
fn repo_override() -> Option<([u8; 4], u16)> {
    let mut rb = [0u8; 4096];
    if let Ok(Some(len)) = lingfs::read_file("repo", &mut rb) {
        return parse_addr(&rb[..len]);
    }
    None
}

/// Fetch `path`'s response BODY (HTTP headers stripped) into `out`. Uses the
/// local HTTP override in lingfs `/repo` if present, otherwise the official
/// registry over HTTPS.
fn fetch(path: &str, out: &mut [u8]) -> Option<usize> {
    if let Some((ip, port)) = repo_override() {
        return netstack::http_get(ip, port, path, "lingos-repo", out);
    }
    let mut noop = |_: &[u8]| {};
    match crate::tls::https_get(OFFICIAL_HOST, 443, path, out, &mut noop) {
        Ok(n) if n > 0 => {
            let off = crate::tls::http_body_offset(&out[..n]);
            if off > 0 {
                out.copy_within(off..n, 0);
                Some(n - off)
            } else {
                Some(n)
            }
        },
        _ => None,
    }
}

/// Fetch `path`'s response body from the official registry over HTTPS,
/// ignoring any local `/repo` override -- for assets like package avatars
/// that only the public host serves. Returns the body length in `out`.
pub fn fetch_official(path: &str, out: &mut [u8]) -> Option<usize> {
    let mut noop = |_: &[u8]| {};
    match crate::tls::https_get(OFFICIAL_HOST, 443, path, out, &mut noop) {
        Ok(n) if n > 0 => {
            let off = crate::tls::http_body_offset(&out[..n]);
            if off > 0 {
                out.copy_within(off..n, 0);
                Some(n - off)
            } else {
                Some(n)
            }
        },
        _ => None,
    }
}

/// Open a keep-alive session to the official registry (one TLS handshake for a
/// whole batch of fetches, e.g. package icons). Pair with `close_official`.
pub fn open_official() -> bool {
    let mut noop = |_: &[u8]| {};
    crate::tls::https_open(OFFICIAL_HOST, 443, &mut noop)
}

/// Fetch `path`'s body over the open official session. Returns body length.
pub fn next_official(path: &str, out: &mut [u8]) -> Option<usize> {
    match crate::tls::https_next(OFFICIAL_HOST, path, out) {
        Some(n) if n > 0 => {
            let off = crate::tls::http_body_offset(&out[..n]);
            if off > 0 {
                out.copy_within(off..n, 0);
                Some(n - off)
            } else {
                Some(n)
            }
        },
        _ => None,
    }
}

/// Close the official keep-alive session (releases the net lock).
pub fn close_official() {
    crate::tls::https_close();
}

/// Extract a JSON string field `"key":"value"` from `json`, or None.
fn json_field<'a>(json: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let n = json.len();
    let mut i = 0;
    while i + key.len() + 3 < n {
        if json[i] == b'"' && json[i + 1..].starts_with(key) && json[i + 1 + key.len()] == b'"' {
            let mut j = i + 2 + key.len();
            while j < n && (json[j] == b' ' || json[j] == b':') {
                j += 1;
            }
            if j < n && json[j] == b'"' {
                j += 1;
                let s = j;
                while j < n && json[j] != b'"' {
                    j += 1;
                }
                return Some(&json[s..j]);
            }
        }
        i += 1;
    }
    None
}

/// Length of a gzip member header at the start of `gz`: `Ok(Some(n))` once it
/// is complete, `Ok(None)` if more bytes are needed, `Err` if it isn't gzip.
fn gz_header_len(gz: &[u8]) -> Result<Option<usize>, ()> {
    if gz.len() < 10 {
        if gz.first().is_some_and(|&b| b != 0x1f) || gz.get(1).is_some_and(|&b| b != 0x8b) {
            return Err(());
        }
        return Ok(None);
    }
    if gz[0] != 0x1f || gz[1] != 0x8b || gz[2] != 8 {
        return Err(());
    }
    let flg = gz[3];
    let mut off = 10usize;
    if flg & 4 != 0 {
        // FEXTRA
        if off + 2 > gz.len() {
            return Ok(None);
        }
        let xlen = gz[off] as usize | ((gz[off + 1] as usize) << 8);
        off += 2 + xlen;
    }
    for bit in [8u8, 16] {
        // FNAME, FCOMMENT (zero-terminated)
        if flg & bit != 0 {
            match gz.get(off..).and_then(|r| r.iter().position(|&b| b == 0)) {
                Some(z) => off += z + 1,
                None => return Ok(None),
            }
        }
    }
    if flg & 2 != 0 {
        off += 2; // FHCRC
    }
    if off > gz.len() {
        return Ok(None);
    }
    Ok(Some(off))
}

/// Parse an octal field (tar header numbers), stopping at NUL/space.
fn tar_oct(b: &[u8]) -> u64 {
    let mut v = 0u64;
    for &c in b.iter().skip_while(|&&c| c == b' ') {
        if (b'0'..=b'7').contains(&c) {
            v = v * 8 + (c - b'0') as u64;
        } else {
            break;
        }
    }
    v
}

// ── Streaming registry install ───────────────────────────────────────────────
// TLS records -> BLAKE3 (the registry's `checksum` is BLAKE3 of the .tgz) ->
// streaming gzip inflate -> streaming tar parser -> each file stored block by
// block in lingfs (`ContentWriter`) -> one directory tree committed at the end.
// Nothing larger than a TLS record / 64 KiB inflate window is ever buffered,
// so a package of any size streams through; what bounds an install is the
// lingfs volume itself (append-only, `MAX_OBJECTS` 4 KiB blocks), which is
// checked BEFORE anything is written. The tree is linked into the root only
// after the download completed and the checksum matched, so a failed install
// never replaces (or half-replaces) what was there.

/// lingfs blocks always left free for the user's own files after an install.
const RESERVE_OBJECTS: usize = 128;
/// Native binaries for other OSes, skipped at install: LingOS can't run them,
/// and they are most of the bytes in packages like LingSys (Windows builds).
const FOREIGN_EXT: &[&str] = &[
    ".exe", ".dll", ".msi", ".dmg", ".pkg", ".deb", ".rpm", ".appimage", ".apk", ".so", ".dylib", ".pdb", ".lib", ".exp", ".ilk",
];

fn mb(n: u64) -> alloc::string::String {
    alloc::format!("{}.{} MB", n / 1_000_000, (n % 1_000_000) / 100_000)
}

fn printf(s: alloc::string::String) {
    print(s.as_bytes());
}

#[derive(Default)]
struct DirNode {
    files: alloc::collections::BTreeMap<alloc::string::String, lingfs::Hash>,
    subdirs: alloc::collections::BTreeSet<alloc::string::String>,
}

#[derive(PartialEq, Clone, Copy)]
enum EntryKind {
    Skip,
    File,
    LongName,
    Pax,
}

struct Unpack {
    /// Survey pass: parse + account only, write nothing.
    install: bool,
    gz_hdr: Vec<u8>,
    gz_done: bool,
    inflater: alloc::boxed::Box<miniz_oxide::inflate::stream::InflateState>,
    inflate_end: bool,
    out: Vec<u8>,
    th: [u8; 512],
    th_fill: usize,
    left: u64,
    pad: u64,
    kind: EntryKind,
    meta: Vec<u8>,
    long_name: Option<alloc::string::String>,
    path: alloc::string::String,
    writer: Option<lingfs::ContentWriter>,
    dirs: alloc::collections::BTreeMap<alloc::string::String, DirNode>,
    error: Option<alloc::string::String>,
    // accounting
    files: usize,
    kept_bytes: u64,
    kept_objs: usize,
    foreign: usize,
    foreign_bytes: u64,
    dropped: usize,
    budget_objs: usize,
    /// Installing onto a FAT volume: the staging folder files stream into
    /// (`media/<vol>/.pkg-<name>.new`), the open file, folders already made,
    /// and the bytes still free on the volume.
    media: Option<alloc::string::String>,
    mfile: Option<crate::fs::media::MediaFile>,
    made: alloc::collections::BTreeSet<alloc::string::String>,
    media_free: u64,
}

impl Unpack {
    fn new(install: bool, budget_objs: usize) -> Self {
        Unpack {
            install,
            gz_hdr: Vec::new(),
            gz_done: false,
            inflater: miniz_oxide::inflate::stream::InflateState::new_boxed(miniz_oxide::DataFormat::Raw),
            inflate_end: false,
            out: alloc::vec![0u8; 64 * 1024],
            th: [0; 512],
            th_fill: 0,
            left: 0,
            pad: 0,
            kind: EntryKind::Skip,
            meta: Vec::new(),
            long_name: None,
            path: alloc::string::String::new(),
            writer: None,
            dirs: alloc::collections::BTreeMap::new(),
            error: None,
            files: 0,
            kept_bytes: 0,
            kept_objs: 0,
            foreign: 0,
            foreign_bytes: 0,
            dropped: 0,
            budget_objs,
            media: None,
            mfile: None,
            made: alloc::collections::BTreeSet::new(),
            media_free: 0,
        }
    }

    /// Stream files into `staging` on a FAT volume with `free` bytes left.
    fn to_media(mut self, staging: alloc::string::String, free: u64) -> Self {
        self.media = Some(staging);
        self.media_free = free;
        self
    }

    fn fail(&mut self, why: alloc::string::String) {
        if self.error.is_none() {
            self.error = Some(why);
        }
    }

    /// Feed raw .tgz bytes.
    fn feed(&mut self, mut data: &[u8]) {
        if self.error.is_some() || self.inflate_end {
            return;
        }
        if !self.gz_done {
            let before = self.gz_hdr.len();
            self.gz_hdr.extend_from_slice(data);
            match gz_header_len(&self.gz_hdr) {
                Err(()) => return self.fail("the download is not a gzip archive".into()),
                Ok(None) => {
                    if self.gz_hdr.len() > 64 * 1024 {
                        self.fail("gzip header too long".into());
                    }
                    return;
                },
                Ok(Some(hl)) => {
                    self.gz_done = true;
                    let skip = hl - before; // header bytes within `data`
                    self.gz_hdr = Vec::new();
                    data = &data[skip..];
                },
            }
        }
        use miniz_oxide::inflate::stream::inflate;
        use miniz_oxide::{MZError, MZFlush, MZStatus};
        let mut out = core::mem::take(&mut self.out);
        while self.error.is_none() && !self.inflate_end {
            let r = inflate(&mut self.inflater, data, &mut out, MZFlush::None);
            data = &data[r.bytes_consumed..];
            if r.bytes_written > 0 {
                self.tar_feed(&out[..r.bytes_written]);
            }
            match r.status {
                Ok(MZStatus::StreamEnd) => self.inflate_end = true,
                Ok(_) | Err(MZError::Buf) => {},
                Err(_) => self.fail("the package's gzip data is corrupt".into()),
            }
            if (data.is_empty() && r.bytes_written < out.len()) || (r.bytes_consumed == 0 && r.bytes_written == 0) {
                break;
            }
        }
        self.out = out;
    }

    fn tar_feed(&mut self, mut data: &[u8]) {
        while !data.is_empty() && self.error.is_none() {
            if self.left > 0 {
                let take = (data.len() as u64).min(self.left) as usize;
                let (now, rest) = data.split_at(take);
                data = rest;
                self.left -= take as u64;
                match self.kind {
                    EntryKind::File => {
                        if let Some(w) = self.writer.as_mut() {
                            if w.write(now).is_err() {
                                let p = self.path.clone();
                                self.fail(alloc::format!("lingfs write failed on {p} (disk full?)"));
                            }
                        }
                        if let Some(f) = self.mfile.as_mut() {
                            if !crate::fs::media::file_write(f, now) {
                                let p = self.path.clone();
                                self.fail(alloc::format!("write failed on {p} -- the disk is full"));
                            }
                        }
                    },
                    EntryKind::LongName | EntryKind::Pax => {
                        if self.meta.len() < 64 * 1024 {
                            self.meta.extend_from_slice(now);
                        }
                    },
                    EntryKind::Skip => {},
                }
                if self.left == 0 {
                    self.end_entry();
                }
                continue;
            }
            if self.pad > 0 {
                let take = (data.len() as u64).min(self.pad) as usize;
                data = &data[take..];
                self.pad -= take as u64;
                continue;
            }
            let take = (512 - self.th_fill).min(data.len());
            self.th[self.th_fill..self.th_fill + take].copy_from_slice(&data[..take]);
            self.th_fill += take;
            data = &data[take..];
            if self.th_fill == 512 {
                self.th_fill = 0;
                self.header();
            }
        }
    }

    fn header(&mut self) {
        let h = self.th;
        if h.iter().all(|&b| b == 0) {
            return; // end-of-archive blocks (and the zero padding after them)
        }
        let cstr = |b: &[u8]| {
            let e = b.iter().position(|&c| c == 0).unwrap_or(b.len());
            alloc::string::String::from_utf8_lossy(&b[..e]).into_owned()
        };
        let mut name = cstr(&h[..100]);
        if &h[257..262] == b"ustar" {
            let prefix = cstr(&h[345..500]);
            if !prefix.is_empty() {
                name = alloc::format!("{prefix}/{name}");
            }
        }
        if let Some(l) = self.long_name.take() {
            name = l;
        }
        let size = tar_oct(&h[124..136]);
        self.left = size;
        self.pad = (512 - size % 512) % 512;
        self.kind = EntryKind::Skip;
        match h[156] {
            b'L' => {
                self.kind = EntryKind::LongName;
                self.meta.clear();
            },
            b'x' => {
                self.kind = EntryKind::Pax;
                self.meta.clear();
            },
            b'0' | 0 | b'7' => self.begin_file(name, size),
            _ => {}, // dirs are implied by their files; links/devices skipped
        }
        if self.left == 0 {
            self.end_entry();
        }
    }

    /// Normalize an archive path: no leading `./` or `/`, no `..`, no empties.
    fn clean_path(name: &str) -> Option<alloc::string::String> {
        let mut parts: Vec<&str> = Vec::new();
        for seg in name.split('/') {
            match seg {
                "" | "." => {},
                ".." => return None,
                s => parts.push(s),
            }
        }
        if parts.is_empty() {
            return None;
        }
        Some(parts.join("/"))
    }

    fn begin_file(&mut self, name: alloc::string::String, size: u64) {
        let Some(path) = Self::clean_path(&name) else { return };
        let lower = path.to_ascii_lowercase();
        if FOREIGN_EXT.iter().any(|e| lower.ends_with(e)) {
            self.foreign += 1;
            self.foreign_bytes += size;
            return;
        }
        if let Some(base) = self.media.clone() {
            return self.begin_media_file(base, path, size);
        }
        let objs = lingfs::objects_for(size as usize);
        let fits_fs = size as usize <= lingfs::MAX_FILE_BYTES
            && path.split('/').all(|s| s.len() <= lingfs::TREE_ENTRY_NAME_LEN)
            && path.split('/').count() < 15;
        if !fits_fs {
            self.dropped += 1;
            printf(alloc::format!("lingfu:   skipped {path} (name or size past lingfs limits)\n"));
            return;
        }
        if self.install {
            if self.kept_objs + objs > self.budget_objs {
                return self.fail(alloc::format!("lingfs is full -- {path} ({}) does not fit", mb(size)));
            }
            if !self.claim(&path) {
                self.dropped += 1;
                printf(alloc::format!("lingfu:   skipped {path} (its folder is full: lingfs holds {} entries per folder)\n", lingfs::MAX_TREE_ENTRIES));
                return;
            }
            self.writer = Some(lingfs::ContentWriter::new());
        }
        self.kept_objs += objs;
        self.kept_bytes += size;
        self.path = path;
        self.kind = EntryKind::File;
    }

    /// A file onto the FAT volume: its folders, a free-space check, then a
    /// streamed file (`media::file_*`). FAT32 holds files up to 4 GiB.
    fn begin_media_file(&mut self, base: alloc::string::String, path: alloc::string::String, size: u64) {
        if size > crate::fs::fat::MAX_FILE {
            self.dropped += 1;
            printf(alloc::format!("lingfu:   skipped {path}: {} is over FAT32's 4 GiB file limit\n", mb(size)));
            return;
        }
        // Whole clusters (up to 32 KiB) + a little directory overhead.
        let need = size.div_ceil(32 * 1024) * 32 * 1024 + 4096;
        if self.install {
            if need > self.media_free {
                let free = self.media_free;
                return self.fail(alloc::format!("the disk is full -- {path} ({}) does not fit in the {} left", mb(size), mb(free)));
            }
            let full = alloc::format!("{base}/{path}");
            let parent = &full[..full.rfind('/').unwrap_or(0)];
            if !self.made.contains(parent) {
                if !crate::fs::media::mkdir_p(parent) {
                    self.dropped += 1;
                    printf(alloc::format!("lingfu:   skipped {path} (could not create its folder)\n"));
                    return;
                }
                self.made.insert(alloc::string::String::from(parent));
            }
            match crate::fs::media::file_create(&full) {
                Some(f) => self.mfile = Some(f),
                None => return self.fail(alloc::format!("could not create {path} on the disk")),
            }
            self.media_free -= need;
        }
        self.kept_bytes += size;
        self.path = path;
        self.kind = EntryKind::File;
    }

    /// Reserve a tree slot for `path` (its folder chain + the file), within
    /// lingfs's per-folder entry limit. Changes nothing when it doesn't fit.
    fn claim(&mut self, path: &str) -> bool {
        let segs: Vec<&str> = path.split('/').collect();
        let fname = segs[segs.len() - 1];
        // Walk the chain checking room first.
        let mut dir = alloc::string::String::new();
        for (i, seg) in segs.iter().enumerate() {
            let node = self.dirs.get(&dir);
            let count = node.map_or(0, |n| n.files.len() + n.subdirs.len());
            let is_last = i + 1 == segs.len();
            let present = node.is_some_and(|n| if is_last { n.files.contains_key(fname) } else { n.subdirs.contains(*seg) });
            if !present && count >= lingfs::MAX_TREE_ENTRIES {
                return false;
            }
            if is_last {
                break;
            }
            if !dir.is_empty() {
                dir.push('/');
            }
            dir.push_str(seg);
        }
        // Then create the chain (the file itself is added at end_entry).
        let mut dir = alloc::string::String::new();
        for seg in &segs[..segs.len() - 1] {
            self.dirs.entry(dir.clone()).or_default().subdirs.insert(alloc::string::String::from(*seg));
            if !dir.is_empty() {
                dir.push('/');
            }
            dir.push_str(seg);
        }
        self.dirs.entry(dir).or_default();
        true
    }

    fn end_entry(&mut self) {
        match self.kind {
            EntryKind::File => {
                self.files += 1;
                if let Some(f) = self.mfile.take() {
                    if !crate::fs::media::file_finish(f) {
                        let p = self.path.clone();
                        self.fail(alloc::format!("could not finish {p} on the disk"));
                    }
                }
                if let Some(w) = self.writer.take() {
                    match w.finish() {
                        Ok(hash) => {
                            let (dir, fname) = match self.path.rfind('/') {
                                Some(i) => (&self.path[..i], &self.path[i + 1..]),
                                None => ("", &self.path[..]),
                            };
                            let (dir, fname) = (alloc::string::String::from(dir), alloc::string::String::from(fname));
                            self.dirs.entry(dir).or_default().files.insert(fname, hash);
                        },
                        Err(()) => {
                            let p = self.path.clone();
                            self.fail(alloc::format!("lingfs write failed on {p} (disk full?)"));
                        },
                    }
                }
            },
            EntryKind::LongName => {
                let e = self.meta.iter().position(|&c| c == 0).unwrap_or(self.meta.len());
                self.long_name = Some(alloc::string::String::from_utf8_lossy(&self.meta[..e]).into_owned());
            },
            EntryKind::Pax => {
                // Records "<len> key=value\n"; only `path` matters here.
                for rec in self.meta.split(|&b| b == b'\n') {
                    if let Some(sp) = rec.iter().position(|&b| b == b' ') {
                        if let Some(v) = rec[sp + 1..].strip_prefix(b"path=") {
                            self.long_name = Some(alloc::string::String::from_utf8_lossy(v).into_owned());
                        }
                    }
                }
            },
            EntryKind::Skip => {},
        }
        self.kind = EntryKind::Skip;
    }

    /// Build the directory trees bottom-up; returns the package root's tree.
    fn build_tree(&self) -> Result<lingfs::Hash, ()> {
        let mut paths: Vec<&alloc::string::String> = self.dirs.keys().collect();
        // Deepest first, so every subdir's hash exists before its parent's.
        paths.sort_by_key(|p| core::cmp::Reverse(if p.is_empty() { 0 } else { p.matches('/').count() + 1 }));
        let mut done: alloc::collections::BTreeMap<alloc::string::String, lingfs::Hash> = alloc::collections::BTreeMap::new();
        for p in paths {
            let node = &self.dirs[p];
            let mut ents: Vec<(alloc::string::String, lingfs::Hash, bool)> = Vec::new();
            for sd in &node.subdirs {
                let full = if p.is_empty() { sd.clone() } else { alloc::format!("{p}/{sd}") };
                let h = *done.get(&full).ok_or(())?;
                ents.push((sd.clone(), h, true));
            }
            for (f, h) in &node.files {
                ents.push((f.clone(), *h, false));
            }
            let refs: Vec<(&str, lingfs::Hash, bool)> = ents.iter().map(|(n, h, d)| (n.as_str(), *h, *d)).collect();
            let h = lingfs::put_dir(&refs)?;
            done.insert(p.clone(), h);
        }
        done.get("").copied().ok_or(())
    }
}

/// Lowercase hex of a 32-byte digest.
fn hex32(d: &[u8; 32]) -> alloc::string::String {
    let mut s = alloc::string::String::with_capacity(64);
    for b in d {
        s.push_str(&alloc::format!("{b:02x}"));
    }
    s
}

/// What happened to one package (for `install_all` / `upgrade_all` tallies).
#[derive(PartialEq, Clone, Copy)]
pub enum Outcome {
    Installed,
    /// No published version or no download -- nothing to install.
    Unavailable,
    /// Bigger than the free space on the install target.
    TooBig,
    Failed,
}

/// Registry metadata for `name`: (latest version, checksum). Err carries the
/// outcome when there is nothing to fetch.
fn registry_info(name: &str) -> Result<(alloc::string::String, alloc::string::String), Outcome> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.') {
        print(b"lingfu: not a package name\n");
        return Err(Outcome::Failed);
    }
    let apistr = alloc::format!("/api/package?name={name}");
    let raw = unsafe { &mut *&raw mut RAW };
    // One retry: the first HTTPS attempt to the registry occasionally fails to
    // connect (transient TLS/connect), and a second almost always succeeds.
    let alen_opt = match fetch_official(&apistr, raw) {
        some @ Some(_) => some,
        None => fetch_official(&apistr, raw),
    };
    let Some(alen) = alen_opt else {
        print(b"lingfu: could not reach the registry\n");
        return Err(Outcome::Failed);
    };
    let Some(ver) = json_field(&raw[..alen], b"version").filter(|v| !v.is_empty()) else {
        if contains(&raw[..alen], b"no published") {
            printf(alloc::format!("lingfu: {name}: no published download -- skipped\n"));
        } else {
            printf(alloc::format!("lingfu: {name}: not in the registry\n"));
        }
        return Err(Outcome::Unavailable);
    };
    let ver = alloc::string::String::from_utf8_lossy(ver).into_owned();
    let sum = json_field(&raw[..alen], b"checksum").map(|c| alloc::string::String::from_utf8_lossy(c).to_ascii_lowercase()).unwrap_or_default();
    Ok((ver, sum))
}

/// Ask for the last 4 bytes of the .tgz (a Range request): returns (HTTP
/// status, total download size, gzip ISIZE = uncompressed size mod 4 GiB).
fn probe_download(dlpath: &str) -> Option<(u16, u64, Option<u64>)> {
    let mut tail: Vec<u8> = Vec::new();
    let mut total = 0u64;
    let mut status = 0u16;
    let mut ranged = false;
    let mut noop = |_: &[u8]| {};
    let r = crate::tls::https_get_stream(
        OFFICIAL_HOST,
        443,
        dlpath,
        "Range: bytes=-4\r\n",
        &mut |i: &crate::tls::StreamInfo| {
            status = i.status;
            ranged = i.status == 206;
            total = if ranged { i.range_total.unwrap_or(0) } else { i.content_length.unwrap_or(0) };
            ranged // a server ignoring Range would send the whole file: stop
        },
        &mut |d: &[u8]| {
            tail.extend_from_slice(d);
            tail.len() <= 64
        },
        &mut noop,
    );
    r.ok()?;
    let isize = (ranged && tail.len() >= 4).then(|| {
        let t = &tail[tail.len() - 4..];
        u32::from_le_bytes([t[0], t[1], t[2], t[3]]) as u64
    });
    Some((status, total, isize))
}

/// One streaming pass over the download. Returns the unpacker (its
/// accounting / error), the BLAKE3 hex of the bytes received, and whether the
/// body arrived complete. A connection that drops mid-way is resumed with a
/// `Range: bytes=<got>-` request (up to 4 times): the hash, inflater and tar
/// parser simply carry on with the next byte.
fn stream_pass(dlpath: &str, total: u64, mut up: Unpack) -> (Unpack, alloc::string::String, bool) {
    let mut hasher = ::blake3::Hasher::new();
    let mut got = 0u64;
    let step: u64 = if total > 40_000_000 { 8_000_000 } else { 2_000_000 };
    let mut next = step;
    let mut complete = false;
    let mut resumes = 0u32;
    loop {
        let from = got;
        let range = if from == 0 { alloc::string::String::new() } else { alloc::format!("Range: bytes={from}-\r\n") };
        let mut noop = |_: &[u8]| {};
        let r = crate::tls::https_get_stream(
            OFFICIAL_HOST,
            443,
            dlpath,
            &range,
            // A resume must get the remainder (206), never the whole file again.
            &mut |i: &crate::tls::StreamInfo| if from == 0 { i.status == 200 } else { i.status == 206 },
            &mut |d: &[u8]| {
                hasher.update(d);
                got += d.len() as u64;
                if got >= next {
                    next += step;
                    printf(alloc::format!("lingfu:   {} / {}\n", mb(got), mb(total)));
                }
                up.feed(d);
                up.error.is_none()
            },
            &mut noop,
        );
        let why = match r {
            Ok(i) if i.complete && (i.status == 200 || i.status == 206) && (total == 0 || got == total) => {
                complete = true;
                break;
            },
            Ok(i) if i.status != 200 && i.status != 206 => {
                up.fail(alloc::format!("the registry answered HTTP {}", i.status));
                break;
            },
            Ok(i) => i.end,
            Err(e) => e,
        };
        if up.error.is_some() {
            break;
        }
        resumes += 1;
        if resumes > 4 || total == 0 {
            up.fail(alloc::format!("download interrupted at {} of {} ({why})", mb(got), mb(total)));
            break;
        }
        printf(alloc::format!("lingfu:   connection lost at {} ({why}) -- resuming\n", mb(got)));
    }
    (up, hex32(hasher.finalize().as_bytes()), complete)
}

/// Install a package from the official registry: resolve its latest version
/// via `/api/package?name=`, stream `/dl/<name>-<version>.tgz` through BLAKE3 +
/// gunzip + untar straight into lingfs, and -- only when the download is
/// complete and its checksum matches -- link it in as `pkg-<name>/` (keeping
/// the archive's folders) and record the version in `packages/<name>`.
pub fn install_from_registry(name: &str) -> bool {
    install_registry(name) == Outcome::Installed
}

fn install_registry(name: &str) -> Outcome {
    let vol = crate::vfs::package_volume();
    if vol.is_none() && !lingfs::is_mounted() {
        print(b"lingfu: no storage mounted\n");
        return Outcome::Failed;
    }
    printf(alloc::format!("lingfu: resolving {name} ...\n"));
    let (ver, sum) = match registry_info(name) {
        Ok(v) => v,
        Err(o) => return o,
    };
    let dlpath = alloc::format!("/dl/{name}-{ver}.tgz");

    // Size first (a 4-byte range request), so hopeless downloads are skipped
    // before any byte is written.
    let Some((status, total, isize)) = probe_download(&dlpath).or_else(|| probe_download(&dlpath)) else {
        print(b"lingfu: could not reach the registry\n");
        return Outcome::Failed;
    };
    if status == 404 {
        printf(alloc::format!("lingfu: {name} {ver}: no published download -- skipped\n"));
        return Outcome::Unavailable;
    }
    if status != 206 && status != 200 {
        printf(alloc::format!("lingfu: {name}: the registry answered HTTP {status}\n"));
        return Outcome::Failed;
    }
    let free_ram = crate::mm::frame::free_frame_count() as u64 * 4096;
    if free_ram < 8 * 1024 * 1024 {
        printf(alloc::format!("lingfu: only {} of RAM free -- close something first\n", mb(free_ram)));
        return Outcome::Failed;
    }
    if let Some(v) = vol {
        return install_to_media(name, &ver, &sum, &dlpath, total, isize, v);
    }

    // Capacity. lingfs never frees a block, so a write that later fails still
    // costs space: decide BEFORE writing.
    let free_objs = lingfs::free_objects().saturating_sub(RESERVE_OBJECTS);
    let free_bytes = free_objs as u64 * lingfs::BLOCK_SIZE as u64;
    let pkgdir = alloc::format!("pkg-{name}");
    let replacing = lingfs::path_kind(&pkgdir).is_some();
    if !replacing && lingfs::root_entry_count() >= lingfs::MAX_TREE_ENTRIES {
        printf(alloc::format!("lingfu: the storage root is full ({} entries) -- remove something first\n", lingfs::MAX_TREE_ENTRIES));
        return Outcome::Failed;
    }
    if lingfs::commits_left() < 4 {
        print(b"lingfu: this lingfs volume has used up its commit log -- it is read-only now\n");
        return Outcome::Failed;
    }
    printf(alloc::format!(
        "lingfu: {name} {ver}: {} download{}; {} free on lingfs\n",
        mb(total),
        isize.map(|n| alloc::format!(", {} unpacked", mb(n))).unwrap_or_default(),
        mb(free_bytes)
    ));
    // The unpacked size (gzip ISIZE) bounds what is stored, plus per-file
    // manifest/tree overhead (~1/100). If it might not fit, a survey pass (a
    // download that writes nothing) counts what WOULD be kept after skipping
    // foreign binaries; only then is anything written.
    let est = isize.unwrap_or(total.saturating_mul(4));
    let est_objs = (est / lingfs::BLOCK_SIZE as u64) as usize + (est / lingfs::BLOCK_SIZE as u64 / 100) as usize + 64;
    if est_objs > free_objs {
        // Compressed bytes over 4x the free space: even if most of it were
        // skippable foreign binaries the rest would not fit -- don't download
        // 100+ MB just to find that out.
        if total > free_bytes.saturating_mul(4) {
            printf(alloc::format!(
                "lingfu: {name} needs up to {} on lingfs but only {} is free -- skipped\n",
                mb(est),
                mb(free_bytes)
            ));
            return Outcome::TooBig;
        }
        print(b"lingfu: may not fit -- surveying the archive first (nothing is written)...\n");
        let (sv, hex, complete) = stream_pass(&dlpath, total, Unpack::new(false, 0));
        if let Some(e) = &sv.error {
            printf(alloc::format!("lingfu: {name}: {e}\n"));
            return Outcome::Failed;
        }
        if !complete {
            printf(alloc::format!("lingfu: {name}: download incomplete\n"));
            return Outcome::Failed;
        }
        if !sum.is_empty() && hex != sum {
            printf(alloc::format!("lingfu: {name}: CHECKSUM MISMATCH (registry {sum}, got {hex}) -- nothing installed\n"));
            return Outcome::Failed;
        }
        if sv.kept_objs + sv.files / 4 > free_objs {
            printf(alloc::format!(
                "lingfu: {name} needs {} on lingfs{} but only {} is free -- skipped\n",
                mb(sv.kept_objs as u64 * lingfs::BLOCK_SIZE as u64),
                if sv.foreign > 0 { alloc::format!(" (after skipping {} of Windows/macOS/Linux binaries)", mb(sv.foreign_bytes)) } else { alloc::string::String::new() },
                mb(free_bytes)
            ));
            return Outcome::TooBig;
        }
    }

    printf(alloc::format!("lingfu: downloading {dlpath} ...\n"));
    let (up, hex, complete) = stream_pass(&dlpath, total, Unpack::new(true, free_objs));
    if let Some(e) = &up.error {
        printf(alloc::format!("lingfu: {name}: {e} -- nothing installed\n"));
        return Outcome::Failed;
    }
    if !complete {
        printf(alloc::format!("lingfu: {name}: download incomplete -- nothing installed\n"));
        return Outcome::Failed;
    }
    if sum.is_empty() {
        print(b"lingfu: warning: the registry lists no checksum for this version\n");
    } else if hex != sum {
        printf(alloc::format!("lingfu: {name}: CHECKSUM MISMATCH (registry {sum}, got {hex}) -- nothing installed\n"));
        return Outcome::Failed;
    } else {
        print(b"lingfu: checksum ok (BLAKE3)\n");
    }
    if up.dirs.is_empty() {
        print(b"lingfu: package unpacked to no installable files\n");
        return Outcome::Failed;
    }
    let Ok(tree) = up.build_tree() else {
        print(b"lingfu: could not write the package's folders (disk full?)\n");
        return Outcome::Failed;
    };
    if lingfs::link_root_dir(&pkgdir, tree).is_err() {
        print(b"lingfu: could not link the package into storage\n");
        return Outcome::Failed;
    }
    // Record the installed version so `lingfu list` / `ls packages` / upgrade see it.
    let _ = lingfs::write_in_dir("packages", name, ver.as_bytes());
    let kept = up.files;
    printf(alloc::format!("lingfu: installed {name} ({ver}) -- {kept} files, {} in {pkgdir}/\n", mb(up.kept_bytes)));
    if up.foreign > 0 {
        printf(alloc::format!("lingfu:   skipped {} non-LingOS binaries ({})\n", up.foreign, mb(up.foreign_bytes)));
    }
    if up.dropped > 0 {
        printf(alloc::format!("lingfu:   {} files did not fit lingfs's limits (listed above)\n", up.dropped));
    }
    printf(alloc::format!("lingfu:   lingfs: {} free\n", mb(lingfs::free_objects() as u64 * lingfs::BLOCK_SIZE as u64)));
    Outcome::Installed
}

/// Install onto the package volume (a FAT disk: the Pi's SD-card data
/// partition or x86's second disk). Files stream into a hidden staging folder
/// `.pkg-<name>.new`; only a complete download whose checksum matches
/// replaces `pkg-<name>/` (FAT frees space, so a failed attempt costs nothing:
/// the staging folder is deleted). Free space is checked against the
/// package's unpacked size up front and again before every file.
fn install_to_media(name: &str, ver: &str, sum: &str, dlpath: &str, total: u64, isize: Option<u64>, vol: &str) -> Outcome {
    use crate::fs::media;
    let base = alloc::format!("media/{vol}");
    let Some((_, free)) = media::space(&base) else {
        printf(alloc::format!("lingfu: {base} is not readable\n"));
        return Outcome::Failed;
    };
    // gzip's ISIZE is the unpacked size mod 4 GiB: trust it only when it is
    // plausible (at least the compressed size, download under 2 GiB); else
    // assume the archive doubles when unpacked.
    let est = match isize {
        Some(n) if n >= total && total < (2u64 << 30) => n,
        _ => total.saturating_mul(2),
    };
    let need = est + est / 50 + 1_000_000;
    printf(alloc::format!("lingfu: {name} {ver}: {} download, ~{} unpacked; {} free on {base}\n", mb(total), mb(est), mb(free)));
    if need > free {
        printf(alloc::format!("lingfu: {name} needs about {} but only {} is free on {base} -- skipped\n", mb(need), mb(free)));
        return Outcome::TooBig;
    }
    let staging = alloc::format!("{base}/.pkg-{name}.new");
    if media::exists(&staging) {
        media::delete_tree(&staging);
    }
    if !media::mkdir_p(&staging) {
        printf(alloc::format!("lingfu: could not create {staging}\n"));
        return Outcome::Failed;
    }
    printf(alloc::format!("lingfu: downloading {dlpath} to {base} ...\n"));
    let (mut up, hex, complete) = stream_pass(dlpath, total, Unpack::new(true, 0).to_media(staging.clone(), free));
    let bad = if let Some(e) = &up.error {
        Some(alloc::format!("{e}"))
    } else if !complete {
        Some(alloc::string::String::from("download incomplete"))
    } else if !sum.is_empty() && hex != sum {
        Some(alloc::format!("CHECKSUM MISMATCH (registry {sum}, got {hex})"))
    } else if up.files == 0 {
        Some(alloc::string::String::from("the package unpacked to no installable files"))
    } else {
        None
    };
    if let Some(why) = bad {
        if let Some(f) = up.mfile.take() {
            media::file_abort(f);
        }
        media::delete_tree(&staging);
        printf(alloc::format!("lingfu: {name}: {why} -- nothing installed\n"));
        return Outcome::Failed;
    }
    if sum.is_empty() {
        print(b"lingfu: warning: the registry lists no checksum for this version\n");
    } else {
        print(b"lingfu: checksum ok (BLAKE3)\n");
    }
    let dest = alloc::format!("{base}/pkg-{name}");
    if media::exists(&dest) && !media::delete_tree(&dest) {
        printf(alloc::format!("lingfu: could not remove the old {dest} -- the new files are in {staging}\n"));
        return Outcome::Failed;
    }
    if !media::rename(&staging, &dest) {
        printf(alloc::format!("lingfu: could not move {staging} to {dest}\n"));
        return Outcome::Failed;
    }
    // The version record, next to the package (`vfs` maps packages/ here).
    let _ = media::mkdir_p(&alloc::format!("{base}/packages"));
    let _ = media::write(&alloc::format!("{base}/packages/{name}"), ver.as_bytes());
    printf(alloc::format!("lingfu: installed {name} ({ver}) -- {} files, {} in {dest}/ (pkg-{name})\n", up.files, mb(up.kept_bytes)));
    if up.foreign > 0 {
        printf(alloc::format!("lingfu:   skipped {} non-LingOS binaries ({})\n", up.foreign, mb(up.foreign_bytes)));
    }
    if up.dropped > 0 {
        printf(alloc::format!("lingfu:   {} files could not be stored (listed above)\n", up.dropped));
    }
    if let Some((_, f)) = media::space(&base) {
        printf(alloc::format!("lingfu:   {base}: {} free\n", mb(f)));
    }
    Outcome::Installed
}

/// `lingfu verify <name>`: stream the latest download through BLAKE3 + gunzip +
/// tar WITHOUT writing anything, and report the checksum verdict and what an
/// install would keep. Works for packages far bigger than the disk.
pub fn verify(name: &str) -> bool {
    let (ver, sum) = match registry_info(name) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let dlpath = alloc::format!("/dl/{name}-{ver}.tgz");
    let Some((status, total, _isize)) = probe_download(&dlpath).or_else(|| probe_download(&dlpath)) else {
        print(b"lingfu: could not reach the registry\n");
        return false;
    };
    if status == 404 {
        printf(alloc::format!("lingfu: {name} {ver}: no published download\n"));
        return false;
    }
    printf(alloc::format!("lingfu: verifying {dlpath} ({}) -- nothing is written\n", mb(total)));
    let (sv, hex, complete) = stream_pass(&dlpath, total, Unpack::new(false, 0));
    if let Some(e) = &sv.error {
        printf(alloc::format!("lingfu: {name}: {e}\n"));
        return false;
    }
    if !complete {
        printf(alloc::format!("lingfu: {name}: download incomplete\n"));
        return false;
    }
    let ok = sum.is_empty() || hex == sum;
    printf(alloc::format!(
        "lingfu: {name} {ver}: checksum {} (BLAKE3 {hex}); {} files, {} would be installed{}\n",
        if sum.is_empty() { "not published" } else if ok { "ok" } else { "MISMATCH" },
        sv.files,
        mb(sv.kept_bytes),
        if sv.foreign > 0 { alloc::format!(", {} of foreign binaries skipped", mb(sv.foreign_bytes)) } else { alloc::string::String::new() }
    ));
    ok
}

/// The version lingfu recorded for an installed package (`packages/<name>`,
/// on the package volume or lingfs -- `vfs` looks in both).
pub fn installed_version(name: &str) -> Option<alloc::string::String> {
    let b = crate::vfs::read(&alloc::format!("packages/{name}"))?;
    Some(alloc::string::String::from_utf8_lossy(&b[..b.len().min(64)]).trim().into())
}

/// Packages lingfu installed (those with a `packages/<name>` version record).
pub fn installed_packages() -> Vec<alloc::string::String> {
    crate::vfs::list("packages").into_iter().filter(|(_, d, _)| !d).map(|(n, _, _)| n).collect()
}

/// Bring every installed package up to the registry's latest version.
/// Returns (updated, failed).
pub fn upgrade_all() -> (usize, usize) {
    if repo_override().is_some() {
        print(b"lingfu: upgrade checks the official registry; /repo is set, so nothing to do\n");
        return (0, 0);
    }
    let names = installed_packages();
    if names.is_empty() {
        print(b"lingfu: no packages installed yet\n");
        return (0, 0);
    }
    let (mut updated, mut failed, mut current) = (0usize, 0usize, 0usize);
    for name in &names {
        let have = installed_version(name).unwrap_or_default();
        match registry_info(name) {
            Ok((latest, _)) if latest == have => {
                current += 1;
                printf(alloc::format!("lingfu: {name} {have} is up to date\n"));
            },
            Ok((latest, _)) => {
                printf(alloc::format!("lingfu: {name} {have} -> {latest}\n"));
                if install_registry(name) == Outcome::Installed {
                    updated += 1;
                } else {
                    failed += 1;
                }
            },
            Err(Outcome::Unavailable) => {},
            Err(_) => failed += 1,
        }
    }
    printf(alloc::format!("lingfu: upgrade done -- {updated} updated, {current} up to date, {failed} failed\n"));
    (updated, failed)
}

/// Install every catalog package that isn't installed yet, skipping the ones
/// with no download and whatever doesn't fit the disk.
/// Returns (installed, failed); skips are reported but not counted as failures.
pub fn install_all() -> (usize, usize) {
    if repo_override().is_some() {
        print(b"lingfu: install --all works against the official registry; /repo is set\n");
        return (0, 0);
    }
    if !ensure_synced() {
        return (0, 0);
    }
    let names: Vec<alloc::string::String> = entries().into_iter().map(|(n, _, _)| n).collect();
    let (mut ok, mut failed, mut skipped, mut already) = (0usize, 0usize, 0usize, 0usize);
    for (i, name) in names.iter().enumerate() {
        if installed_version(name).is_some() || is_installed(name) {
            already += 1;
            continue;
        }
        printf(alloc::format!("lingfu: [{}/{}] {name}\n", i + 1, names.len()));
        match install_registry(name) {
            Outcome::Installed => ok += 1,
            Outcome::Unavailable | Outcome::TooBig => skipped += 1,
            Outcome::Failed => failed += 1,
        }
    }
    printf(alloc::format!(
        "lingfu: install --all done -- {ok} installed, {already} already here, {skipped} skipped, {failed} failed; lingfs {} free\n",
        mb(lingfs::free_objects() as u64 * lingfs::BLOCK_SIZE as u64)
    ));
    (ok, failed)
}

/// `lingfu sync`: refresh the catalog, then upgrade what is installed.
pub fn sync_and_upgrade() -> usize {
    let n = sync();
    if n > 0 {
        printf(alloc::format!("lingfu: {n} packages in the catalog\n"));
        if lingfs::is_mounted() {
            upgrade_all();
        }
    }
    n
}

/// Append one printable-ASCII byte to CATALOG at `w`, returning the new `w`.
/// Non-ASCII (the registry carries Thai/CJK descriptions) is dropped -- the
/// font is 8x8 ASCII, and the layout/list renderers skip it anyway.
fn cat_push(cat: &mut [u8], w: usize, b: u8) -> usize {
    if w < cat.len() && (0x20..=0x7e).contains(&b) {
        cat[w] = b;
        return w + 1;
    }
    w
}

fn cat_push_sep(cat: &mut [u8], w: usize) -> usize {
    if w < cat.len() {
        cat[w] = FSEP;
        return w + 1;
    }
    w
}

/// Find `"key"` used as an object key (followed, after optional space, by ':')
/// in `obj`, returning the index just past the ':'.
fn json_key_pos(obj: &[u8], key: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    'outer: while i + key.len() + 2 < obj.len() {
        if obj[i] == b'"' && obj[i + 1..].starts_with(key) && obj[i + 1 + key.len()] == b'"' {
            let mut j = i + 2 + key.len();
            while j < obj.len() && (obj[j] == b' ' || obj[j] == b'\t') {
                j += 1;
            }
            if j < obj.len() && obj[j] == b':' {
                return Some(j + 1);
            }
            i += 1;
            continue 'outer;
        }
        i += 1;
    }
    None
}

/// Copy the JSON string value of `key` from `obj` into CATALOG at `w`
/// (unescaping the common escapes, ASCII-filtered, capped at `max` bytes).
/// Returns the new write cursor.
fn json_copy_string(obj: &[u8], key: &[u8], cat: &mut [u8], mut w: usize, max: usize) -> usize {
    let Some(mut p) = json_key_pos(obj, key) else { return w };
    while p < obj.len() && obj[p] != b'"' {
        if obj[p] == b',' || obj[p] == b'}' {
            return w; // value wasn't a string
        }
        p += 1;
    }
    if p >= obj.len() {
        return w;
    }
    p += 1; // past opening quote
    let start = w;
    while p < obj.len() && obj[p] != b'"' {
        let b = if obj[p] == b'\\' && p + 1 < obj.len() {
            p += 1;
            match obj[p] {
                b'n' | b't' | b'r' => b' ',
                b'u' => {
                    // \uXXXX -> skip the 4 hex digits, emit nothing.
                    p += 4;
                    p += 1;
                    continue;
                },
                other => other,
            }
        } else {
            obj[p]
        };
        if w - start >= max {
            break;
        }
        w = cat_push(cat, w, b);
        p += 1;
    }
    w
}

/// Copy the JSON number value of `key` from `obj` into CATALOG at `w`.
fn json_copy_number(obj: &[u8], key: &[u8], cat: &mut [u8], mut w: usize) -> usize {
    let Some(mut p) = json_key_pos(obj, key) else { return w };
    while p < obj.len() && (obj[p] == b' ' || obj[p] == b'\t') {
        p += 1;
    }
    while p < obj.len() && obj[p].is_ascii_digit() {
        w = cat_push(cat, w, obj[p]);
        p += 1;
    }
    w
}

/// Parse the official registry JSON into normalized CATALOG lines. The
/// endpoint returns `{"packages":[{name,description,downloads},...]}` (an
/// object wrapping the array), so we position at the array and walk its
/// top-level `{...}` elements -- string-aware, so braces or brackets inside a
/// description don't confuse element boundaries. Returns line count.
fn normalize_json(raw: &[u8]) -> usize {
    let cat = unsafe { &mut *&raw mut CATALOG };
    let n = raw.len();
    let mut w = 0usize;

    // Position `i` at the start of the packages array: just past the first
    // top-level '[' (works for both {"packages":[...]} and a bare [...]).
    let mut i = 0usize;
    {
        let mut in_str = false;
        let mut found = false;
        while i < n {
            let b = raw[i];
            if in_str {
                if b == b'\\' {
                    i += 1;
                } else if b == b'"' {
                    in_str = false;
                }
            } else if b == b'"' {
                in_str = true;
            } else if b == b'[' {
                i += 1;
                found = true;
                break;
            }
            i += 1;
        }
        if !found {
            unsafe { CATALOG_LEN = 0 };
            return 0;
        }
    }

    // Walk array elements. Each package object runs from its '{' to the
    // matching '}', tracking strings so quoted braces don't miscount depth.
    loop {
        // Skip to the next '{' (string-aware); stop at the array's ']'.
        let mut in_str = false;
        while i < n {
            let b = raw[i];
            if in_str {
                if b == b'\\' {
                    i += 1;
                } else if b == b'"' {
                    in_str = false;
                }
            } else if b == b'"' {
                in_str = true;
            } else if b == b'{' {
                break;
            } else if b == b']' {
                i = n; // end of array
                break;
            }
            i += 1;
        }
        if i >= n {
            break;
        }
        let obj_start = i;
        let mut j = i + 1;
        let mut depth = 1i32;
        let mut in_str = false;
        while j < n && depth > 0 {
            let b = raw[j];
            if in_str {
                if b == b'\\' {
                    j += 1;
                } else if b == b'"' {
                    in_str = false;
                }
            } else {
                match b {
                    b'"' => in_str = true,
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {},
                }
            }
            j += 1;
        }
        let obj = &raw[obj_start..j.min(n)];
        i = j;

        // name \t downloads \t (empty filename) \t description
        let before = w;
        w = json_copy_string(obj, b"name", cat, w, 48);
        if w == before {
            continue; // no name -> skip
        }
        w = cat_push_sep(cat, w);
        w = json_copy_number(obj, b"downloads", cat, w);
        w = cat_push_sep(cat, w);
        w = cat_push_sep(cat, w); // filename: empty for the official registry
        w = json_copy_string(obj, b"description", cat, w, DESC_MAX);
        if w < cat.len() {
            cat[w] = b'\n';
            w += 1;
        }
    }
    unsafe { CATALOG_LEN = w };
    count()
}

/// Parse a local `/repo` plaintext catalog ("name version filename desc...",
/// space-delimited) into normalized tab-delimited CATALOG lines.
fn normalize_catalog_txt(raw: &[u8]) -> usize {
    let cat = unsafe { &mut *&raw mut CATALOG };
    let mut w = 0usize;
    for line in raw.split(|&b| b == b'\n') {
        let line = if line.last() == Some(&b'\r') { &line[..line.len() - 1] } else { line };
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split(|&b| b == b' ').filter(|f| !f.is_empty());
        let (Some(name), ver, file) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        for &b in name {
            w = cat_push(cat, w, b);
        }
        w = cat_push_sep(cat, w);
        for &b in ver.unwrap_or(b"") {
            w = cat_push(cat, w, b);
        }
        w = cat_push_sep(cat, w);
        for &b in file.unwrap_or(b"") {
            w = cat_push(cat, w, b);
        }
        w = cat_push_sep(cat, w);
        // The rest of the line (after the 3 fields) is the description.
        let mut seen = 0;
        let mut k = 0;
        while k < line.len() && seen < 3 {
            while k < line.len() && line[k] == b' ' {
                k += 1;
            }
            while k < line.len() && line[k] != b' ' {
                k += 1;
            }
            seen += 1;
        }
        while k < line.len() && line[k] == b' ' {
            k += 1;
        }
        for &b in &line[k..] {
            w = cat_push(cat, w, b);
        }
        if w < cat.len() {
            cat[w] = b'\n';
            w += 1;
        }
    }
    unsafe { CATALOG_LEN = w };
    count()
}

/// Sync the catalog. A local `/repo` override (if set) serves a plaintext
/// `catalog.txt`; otherwise the official registry's `/api/packages` JSON is
/// fetched over HTTPS. Returns the number of package lines, 0 on any failure.
pub fn sync() -> usize {
    let raw = unsafe { &mut *&raw mut RAW };
    let (path, local) = if repo_override().is_some() {
        print(b"lingfu: syncing catalog from /repo (HTTP)...\n");
        ("/catalog.txt", true)
    } else {
        print(b"lingfu: syncing catalog from fu.ling-lang.org (HTTPS)...\n");
        ("/api/packages", false)
    };
    match fetch(path, raw) {
        Some(len) => {
            let n = if local { normalize_catalog_txt(&raw[..len]) } else { normalize_json(&raw[..len]) };
            if n == 0 {
                print(b"lingfu: synced, but the catalog was empty or unrecognized.\n");
            } else {
                print(b"lingfu: catalog synced\n");
            }
            n
        },
        None => {
            unsafe { CATALOG_LEN = 0 };
            print(b"lingfu: sync failed (TLS/connect error, or no catalog served).\n");
            print(b"lingfu: point elsewhere by writing \"a.b.c.d:port\" into lingfs /repo (HTTP).\n");
            0
        },
    }
}

fn catalog() -> &'static [u8] {
    unsafe { &(&*&raw const CATALOG)[..CATALOG_LEN] }
}

fn count() -> usize {
    catalog().split(|&b| b == b'\n').filter(|l| !l.is_empty()).count()
}

/// Print catalog lines, optionally only those containing `query`.
pub fn list(query: &str) -> usize {
    if unsafe { CATALOG_LEN } == 0 {
        print(b"lingfu: no catalog -- run 'lingfu sync' first (and see 'lingfu list' for installed)\n");
        return 0;
    }
    let mut shown = 0;
    for line in catalog().split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        if !query.is_empty() && !contains(line, query.as_bytes()) {
            continue;
        }
        print(b"  ");
        // Render the tab field separators as spaces for the console.
        let mut buf = [0u8; 160];
        let m = line.len().min(buf.len());
        for i in 0..m {
            buf[i] = if line[i] == FSEP { b' ' } else { line[i] };
        }
        print(&buf[..m]);
        print(b"\n");
        shown += 1;
    }
    if shown == 0 {
        print(b"lingfu: no catalog entry matches\n");
    }
    shown
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if hay.len() < needle.len() {
        return false;
    }
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Download and install one catalog package by name: resolve its filename
/// from the synced catalog, HTTP-GET it, land the blob in lingfs, unpack
/// via the existing `.lpkg` path. Returns true on a completed install.
pub fn install(name: &str) -> bool {
    // `lingfu install --all` (every front end funnels through here): install
    // every catalog package that fits; true when none failed.
    if name == "--all" {
        let (_ok, failed) = install_all();
        return failed == 0;
    }
    // Official registry (no local /repo override): resolve the version and pull
    // the source tarball from /dl, unpacking it into lingfs. A local /repo
    // still serves the catalog.txt-listed `.lpkg` blobs (the branch below).
    if repo_override().is_none() {
        return install_from_registry(name);
    }
    if unsafe { CATALOG_LEN } == 0 && sync() == 0 {
        return false;
    }
    // Find the "name \t meta \t filename \t desc" line; field 2 is the
    // installable filename (empty for the official registry -- no public
    // binary-download endpoint, only a local /repo serves .lpkg blobs).
    let mut filename = [0u8; 64];
    let mut fn_len = 0usize;
    let mut found = false;
    for line in catalog().split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split(|&b| b == FSEP);
        let (Some(n), _meta, file) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        if n == name.as_bytes() {
            found = true;
            let f = file.unwrap_or(b"");
            fn_len = f.len().min(filename.len());
            filename[..fn_len].copy_from_slice(&f[..fn_len]);
            break;
        }
    }
    if !found {
        print(b"lingfu: package not in catalog (try 'lingfu search')\n");
        return false;
    }
    if fn_len == 0 {
        print(b"lingfu: this catalog lists packages but serves no downloadable file.\n");
        print(b"lingfu: the public registry has no binary-download API yet -- point at a\n");
        print(b"lingfu: local .lpkg repo by writing \"a.b.c.d:port\" into lingfs /repo.\n");
        return false;
    }
    let Ok(fname) = core::str::from_utf8(&filename[..fn_len]) else { return false };

    let mut path = [0u8; 72];
    path[0] = b'/';
    path[1..1 + fn_len].copy_from_slice(&filename[..fn_len]);
    let Ok(pathstr) = core::str::from_utf8(&path[..1 + fn_len]) else { return false };

    print(b"lingfu: downloading ");
    print(fname.as_bytes());
    print(b" ...\n");
    // Single-block lingfs cap, disclosed in the module doc: bodies bigger
    // than one block fail the write below rather than silently truncating.
    static mut BLOB: [u8; 64 * 1024] = [0; 64 * 1024];
    let blob = unsafe { &mut *&raw mut BLOB };
    let Some(len) = fetch(pathstr, blob) else {
        print(b"lingfu: download failed\n");
        return false;
    };
    if lingfs::write_in_dir("catalog", fname, &blob[..len]).is_err() {
        print(b"lingfu: could not store the blob in lingfs (package too big for a single-block file? multi-block files are queued work)\n");
        return false;
    }
    print(b"lingfu: downloaded, installing...\n");
    // packages::install expects the blob's lingfs name; the catalog dir
    // convention matches the local-install flow's.
    let mut full = [0u8; 96];
    let prefix = b"catalog/";
    full[..prefix.len()].copy_from_slice(prefix);
    full[prefix.len()..prefix.len() + fn_len].copy_from_slice(&filename[..fn_len]);
    let Ok(fullstr) = core::str::from_utf8(&full[..prefix.len() + fn_len]) else { return false };
    if packages::install(fullstr) {
        print(b"lingfu: installed (see 'lingfu list' / 'ls packages')\n");
        true
    } else {
        print(b"lingfu: blob downloaded but unpack failed (malformed .lpkg?)\n");
        false
    }
}

// ── Package manager views (both desktops' Packages windows) ─────────────────

/// The synced catalog as (name, meta, description) rows; `meta` is the
/// download count on the official registry, the version on a local repo.
pub fn entries() -> Vec<(alloc::string::String, alloc::string::String, alloc::string::String)> {
    use alloc::string::String;
    let mut out = Vec::new();
    for line in catalog().split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let mut f = line.split(|&b| b == FSEP);
        let s = |b: Option<&[u8]>| String::from_utf8_lossy(b.unwrap_or(b"")).into_owned();
        let name = s(f.next());
        let meta = s(f.next());
        let _file = f.next();
        let desc = s(f.next());
        if !name.is_empty() {
            out.push((name, meta, desc));
        }
    }
    out
}

/// Sync the catalog once (the registry over HTTPS) unless it is already here.
pub fn ensure_synced() -> bool {
    synced() || sync() > 0
}

/// Is `name` unpacked under pkg-<name>/ on storage?
pub fn is_installed(name: &str) -> bool {
    crate::vfs::is_dir(&alloc::format!("pkg-{name}"))
}

/// Does the installed package have a Ling entry (what the start menus run)?
pub fn is_app(name: &str) -> bool {
    crate::lingc::locate(&alloc::format!("pkg-{name}")).is_some()
}

/// Installed packages that are runnable, by name (for the start menus).
pub fn installed_apps() -> Vec<alloc::string::String> {
    let mut out = Vec::new();
    for (n, is_dir, _) in crate::vfs::list("") {
        if is_dir {
            if let Some(name) = n.strip_prefix("pkg-") {
                if is_app(name) {
                    out.push(alloc::string::String::from(name));
                }
            }
        }
    }
    out
}

/// What a package's description says about LingOS: (mentions LingOS, fits
/// x86-64, fits the Raspberry Pi). The registry carries no platform field, so
/// this is the convention: "LingOS" in the description marks a LingOS app;
/// "x86-64"/"x86_64"/"amd64" or "rpi5"/"Raspberry Pi"/"aarch64"/"arm64"
/// narrow it to one machine; naming neither means both.
pub fn for_lingos(desc: &str) -> (bool, bool, bool) {
    let d = desc.to_ascii_lowercase();
    let lingos = d.contains("lingos") || d.contains("ling os");
    let x86 = ["x86-64", "x86_64", "x86", "amd64"].iter().any(|w| d.contains(w));
    let pi = ["rpi5", "rpi 5", "rpi", "raspberry", "aarch64", "arm64"].iter().any(|w| d.contains(w));
    (lingos, x86 || !pi, pi || !x86)
}
