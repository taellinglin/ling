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

fn repo_addr() -> ([u8; 4], u16) {
    // lingfs "/repo" holds "a.b.c.d:port"; absent/garbled -> SLIRP host.
    let mut buf = [0u8; 4096];
    if let Ok(Some(len)) = lingfs::read_file("repo", &mut buf) {
        if let Some((ip, port)) = parse_addr(&buf[..len]) {
            return (ip, port);
        }
    }
    (netstack::GATEWAY_IP, 8000)
}

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

fn print(s: &[u8]) {
    crate::console_write(s);
}

/// Fetch the catalog from the repo. Returns the number of package lines,
/// or 0 on any failure (each failure mode named on the console).
pub fn sync() -> usize {
    let (ip, port) = repo_addr();
    print(b"lingfu: syncing catalog from repo...\n");
    let body = unsafe { &mut *&raw mut CATALOG };
    match netstack::http_get(ip, port, "/catalog.txt", "lingos-repo", body) {
        Some(len) => {
            unsafe { CATALOG_LEN = len };
            let n = count();
            print(b"lingfu: catalog synced (real HTTP over the e1000)\n");
            n
        },
        None => {
            unsafe { CATALOG_LEN = 0 };
            print(b"lingfu: sync failed (no route to repo, or no server at the repo address)\n");
            print(b"lingfu: repo default is 10.0.2.2:8000 -- run an HTTP server on the QEMU host,\n");
            print(b"lingfu: or write \"a.b.c.d:port\" into lingfs /repo to point elsewhere\n");
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
        print(line);
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
    // Find "name version filename ..." line.
    let mut filename = [0u8; 64];
    let mut fn_len = 0usize;
    for line in catalog().split(|&b| b == b'\n') {
        let mut fields = line.split(|&b| b == b' ').filter(|f| !f.is_empty());
        let (Some(n), Some(_v), Some(f)) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        if n == name.as_bytes() {
            fn_len = f.len().min(filename.len());
            filename[..fn_len].copy_from_slice(&f[..fn_len]);
            break;
        }
    }
    if fn_len == 0 {
        print(b"lingfu: package not in catalog (try 'lingfu search')\n");
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
    let (ip, port) = repo_addr();
    // Single-block lingfs cap, disclosed in the module doc: bodies bigger
    // than one block fail the write below rather than silently truncating.
    static mut BLOB: [u8; 64 * 1024] = [0; 64 * 1024];
    let blob = unsafe { &mut *&raw mut BLOB };
    let Some(len) = netstack::http_get(ip, port, pathstr, "lingos-repo", blob) else {
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
