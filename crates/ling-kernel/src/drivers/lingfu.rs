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
//! Real limits, disclosed: package payloads are capped by lingfs's
//! single-block file size today (~4KiB) -- multi-block files are queued
//! work; a too-big download fails cleanly rather than truncating. No
//! signatures yet (packages/README step 5); the catalog says so.

use crate::drivers::netstack;
use crate::fs::{lingfs, packages};

const CATALOG_MAX: usize = 16 * 1024;
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
static mut CAP: [u8; 8 * 1024] = [0; 8 * 1024];
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
        w = json_copy_string(obj, b"description", cat, w, 96);
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
