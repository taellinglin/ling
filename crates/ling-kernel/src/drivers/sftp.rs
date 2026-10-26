//! SFTP (protocol version 3, `draft-ietf-secsh-filexfer-02`) as an SSH
//! subsystem: `sftp ling@pi`, WinSCP, FileZilla and `scp -s` can list,
//! upload, download, rename and delete files on LingOS.
//!
//! The SSH layer (`sshd`) hands every CHANNEL_DATA payload to [`Sftp::feed`]
//! and writes back the response packets it returns. Paths are LingOS
//! storage paths: `/home/ling/Music/x.wav`, `/media/usb0/...` for a USB
//! drive; a relative path (and `.`) starts in the user's home folder. Files
//! under `home/<user>/Private/` go through the keyring like everywhere else
//! (sealed on write, opened on read; refused while it is locked).
//!
//! An open file is held in memory: reads serve slices of it, writes patch it,
//! and it is stored on CLOSE (lingfs files are written whole). Files up to
//! lingfs's ~64 MiB cap.

use crate::fs::{lingfs, media};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const FXP_INIT: u8 = 1;
const FXP_VERSION: u8 = 2;
const FXP_OPEN: u8 = 3;
const FXP_CLOSE: u8 = 4;
const FXP_READ: u8 = 5;
const FXP_WRITE: u8 = 6;
const FXP_LSTAT: u8 = 7;
const FXP_FSTAT: u8 = 8;
const FXP_SETSTAT: u8 = 9;
const FXP_FSETSTAT: u8 = 10;
const FXP_OPENDIR: u8 = 11;
const FXP_READDIR: u8 = 12;
const FXP_REMOVE: u8 = 13;
const FXP_MKDIR: u8 = 14;
const FXP_RMDIR: u8 = 15;
const FXP_REALPATH: u8 = 16;
const FXP_STAT: u8 = 17;
const FXP_RENAME: u8 = 18;
const FXP_READLINK: u8 = 19;
const FXP_SYMLINK: u8 = 20;
const FXP_STATUS: u8 = 101;
const FXP_HANDLE: u8 = 102;
const FXP_DATA: u8 = 103;
const FXP_NAME: u8 = 104;
const FXP_ATTRS: u8 = 105;
const FXP_EXTENDED: u8 = 200;

const FX_OK: u32 = 0;
const FX_EOF: u32 = 1;
const FX_NO_SUCH_FILE: u32 = 2;
const FX_PERMISSION_DENIED: u32 = 3;
const FX_FAILURE: u32 = 4;
const FX_BAD_MESSAGE: u32 = 5;
const FX_OP_UNSUPPORTED: u32 = 8;

const PF_READ: u32 = 0x01;
const PF_WRITE: u32 = 0x02;
const PF_APPEND: u32 = 0x04;
const PF_CREAT: u32 = 0x08;
const PF_TRUNC: u32 = 0x10;
const PF_EXCL: u32 = 0x20;

const ATTR_SIZE: u32 = 0x01;
const ATTR_PERMISSIONS: u32 = 0x04;
const ATTR_ACMODTIME: u32 = 0x08;

/// lingfs's whole-file cap; a bigger upload fails at the write, not at CLOSE.
const MAX_FILE: usize = 64 * 1024 * 1024;
/// A packet bigger than this is malformed (clients send ≤ ~256 KiB).
const MAX_PACKET: usize = 512 * 1024;

enum Handle {
    File { path: String, data: Vec<u8>, dirty: bool, writable: bool },
    Dir { entries: Vec<(String, bool, u64)>, pos: usize },
}

pub struct Sftp {
    inbuf: Vec<u8>,
    handles: Vec<Option<Handle>>,
    home: String,
}

// ── wire helpers ─────────────────────────────────────────────────────────────

fn get_u32(b: &[u8], p: &mut usize) -> Option<u32> {
    let v = b.get(*p..*p + 4)?;
    *p += 4;
    Some(u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
}

fn get_u64(b: &[u8], p: &mut usize) -> Option<u64> {
    let hi = get_u32(b, p)? as u64;
    let lo = get_u32(b, p)? as u64;
    Some((hi << 32) | lo)
}

fn get_bytes<'a>(b: &'a [u8], p: &mut usize) -> Option<&'a [u8]> {
    let n = get_u32(b, p)? as usize;
    let v = b.get(*p..*p + n)?;
    *p += n;
    Some(v)
}

fn get_str(b: &[u8], p: &mut usize) -> Option<String> {
    get_bytes(b, p).map(|v| String::from_utf8_lossy(v).into_owned())
}

fn put_u32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_be_bytes());
}

fn put_u64(o: &mut Vec<u8>, v: u64) {
    o.extend_from_slice(&v.to_be_bytes());
}

fn put_str(o: &mut Vec<u8>, s: &[u8]) {
    put_u32(o, s.len() as u32);
    o.extend_from_slice(s);
}

/// Wrap a body (type byte onward) with its length.
fn packet(body: Vec<u8>) -> Vec<u8> {
    let mut o = Vec::with_capacity(4 + body.len());
    put_u32(&mut o, body.len() as u32);
    o.extend_from_slice(&body);
    o
}

fn status(id: u32, code: u32, msg: &str) -> Vec<u8> {
    let mut b = alloc::vec![FXP_STATUS];
    put_u32(&mut b, id);
    put_u32(&mut b, code);
    put_str(&mut b, msg.as_bytes());
    put_str(&mut b, b"en");
    packet(b)
}

fn mtime() -> u32 {
    crate::lingapp::wall_time() as u32
}

fn put_attrs(o: &mut Vec<u8>, is_dir: bool, size: u64) {
    put_u32(o, ATTR_SIZE | ATTR_PERMISSIONS | ATTR_ACMODTIME);
    put_u64(o, size);
    put_u32(o, if is_dir { 0o40755 } else { 0o100644 });
    let t = mtime();
    put_u32(o, t);
    put_u32(o, t);
}

fn longname(name: &str, is_dir: bool, size: u64, owner: &str) -> String {
    format!(
        "{}    1 {:<8} {:<8} {:>12} Jan  1 00:00 {}",
        if is_dir { "drwxr-xr-x" } else { "-rw-r--r--" },
        owner,
        owner,
        size,
        name
    )
}

// ── storage routing ──────────────────────────────────────────────────────────

fn in_home(p: &str) -> bool {
    p == "home" || p.starts_with("home/")
}

fn read_file(p: &str) -> Result<Vec<u8>, u32> {
    if in_home(p) {
        return crate::vfs::read(p).ok_or(FX_NO_SUCH_FILE);
    }
    if media::is_media_path(p) {
        return media::read(p).ok_or(FX_NO_SUCH_FILE);
    }
    let size = lingfs::file_size(p).ok_or(FX_NO_SUCH_FILE)?;
    let mut buf = alloc::vec![0u8; size.max(1) + 16];
    let n = match lingfs::read_file_all(p, &mut buf) {
        Ok(Some(n)) => n,
        _ => return Err(FX_NO_SUCH_FILE),
    };
    buf.truncate(n);
    if crate::keyring::is_private_path(p) && crate::keyring::is_sealed(&buf) {
        return crate::keyring::open("vfs-private", p.as_bytes(), &buf).ok_or(FX_PERMISSION_DENIED);
    }
    Ok(buf)
}

fn write_file(p: &str, data: &[u8]) -> Result<(), u32> {
    if data.len() > MAX_FILE {
        return Err(FX_FAILURE);
    }
    if in_home(p) {
        return if crate::vfs::write(p, data) { Ok(()) } else { Err(FX_FAILURE) };
    }
    if media::is_media_path(p) {
        return if media::write(p, data) { Ok(()) } else { Err(FX_FAILURE) };
    }
    if !lingfs::is_mounted() {
        return Err(FX_PERMISSION_DENIED);
    }
    if let Some(i) = p.rfind('/') {
        let _ = lingfs::mkdir_p(&p[..i]);
    }
    if crate::keyring::is_private_path(p) {
        let blob = crate::keyring::seal("vfs-private", p.as_bytes(), data).ok_or(FX_PERMISSION_DENIED)?;
        return lingfs::write_path_any(p, &blob).map_err(|_| FX_FAILURE);
    }
    lingfs::write_path_any(p, data).map_err(|_| FX_FAILURE)
}

/// `Some(is_dir)` when the path exists.
fn kind(p: &str) -> Option<bool> {
    if p.is_empty() {
        return Some(true);
    }
    if p == "media" {
        return Some(true);
    }
    if in_home(p) {
        if crate::vfs::is_dir(p) {
            return Some(true);
        }
        return if crate::vfs::exists(p) { Some(false) } else { None };
    }
    if media::is_media_path(p) {
        if media::is_dir(p) {
            return Some(true);
        }
        return if media::exists(p) { Some(false) } else { None };
    }
    lingfs::path_kind(p)
}

fn size_of(p: &str) -> u64 {
    if in_home(p) {
        return crate::vfs::read(p).map(|v| v.len() as u64).unwrap_or(0);
    }
    if media::is_media_path(p) {
        return media::read(p).map(|v| v.len() as u64).unwrap_or(0);
    }
    lingfs::file_size(p).unwrap_or(0) as u64
}

fn list(p: &str) -> Option<Vec<(String, bool, u64)>> {
    if p == "media" {
        return Some(media::names().into_iter().map(|n| (n, true, 0)).collect());
    }
    if media::is_media_path(p) {
        return Some(media::list(p).into_iter().map(|(n, d, s)| (n, d, s as u64)).collect());
    }
    if in_home(p) && crate::vfs::is_dir(p) {
        return Some(crate::vfs::list(p).into_iter().map(|(n, d, s)| (n, d, s as u64)).collect());
    }
    if lingfs::path_kind(p) != Some(true) {
        return None;
    }
    let mut out = Vec::new();
    let mut i = 0usize;
    loop {
        let mut nb = [0u8; 128];
        let Some((n, is_dir)) = lingfs::list_entry(p, i, &mut nb) else { break };
        i += 1;
        let name = String::from_utf8_lossy(&nb[..n]).into_owned();
        let full = if p.is_empty() { name.clone() } else { format!("{p}/{name}") };
        let size = if is_dir { 0 } else { lingfs::file_size(&full).unwrap_or(0) as u64 };
        out.push((name, is_dir, size));
        if i > 4096 {
            break;
        }
    }
    if p.is_empty() && !media::names().is_empty() {
        out.push((String::from("media"), true, 0));
    }
    Some(out)
}

fn remove(p: &str) -> Result<(), u32> {
    if in_home(p) {
        return if crate::vfs::delete(p) { Ok(()) } else { Err(FX_FAILURE) };
    }
    if media::is_media_path(p) {
        return if media::delete(p) { Ok(()) } else { Err(FX_FAILURE) };
    }
    match lingfs::remove_path(p) {
        Ok(true) => Ok(()),
        Ok(false) => Err(FX_NO_SUCH_FILE),
        Err(_) => Err(FX_FAILURE),
    }
}

impl Sftp {
    pub fn new(user: &str) -> Sftp {
        Sftp { inbuf: Vec::new(), handles: Vec::new(), home: format!("home/{user}") }
    }

    /// A client path as a LingOS path (no leading slash; "" is the root).
    fn resolve(&self, raw: &str) -> String {
        let start = if raw.starts_with('/') { String::new() } else { self.home.clone() };
        let mut parts: Vec<&str> = start.split('/').filter(|s| !s.is_empty()).collect();
        for seg in raw.split('/') {
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

    fn new_handle(&mut self, h: Handle) -> Vec<u8> {
        let i = match self.handles.iter().position(|x| x.is_none()) {
            Some(i) => {
                self.handles[i] = Some(h);
                i
            },
            None => {
                self.handles.push(Some(h));
                self.handles.len() - 1
            },
        };
        format!("h{i}").into_bytes()
    }

    fn handle_index(&self, h: &[u8]) -> Option<usize> {
        let s = core::str::from_utf8(h).ok()?.strip_prefix('h')?;
        let i: usize = s.parse().ok()?;
        (i < self.handles.len() && self.handles[i].is_some()).then_some(i)
    }

    /// Flush every open file (the client vanished without CLOSE).
    pub fn finish(&mut self) {
        for slot in self.handles.iter_mut() {
            if let Some(Handle::File { path, data, dirty: true, .. }) = slot.take() {
                let _ = write_file(&path, &data);
            }
        }
    }

    /// Bytes from the channel in, complete response packets out.
    pub fn feed(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.inbuf.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            if self.inbuf.len() < 4 {
                break;
            }
            let len = u32::from_be_bytes([self.inbuf[0], self.inbuf[1], self.inbuf[2], self.inbuf[3]]) as usize;
            if len == 0 || len > MAX_PACKET {
                self.inbuf.clear();
                out.push(status(0, FX_BAD_MESSAGE, "packet too large"));
                break;
            }
            if self.inbuf.len() < 4 + len {
                break;
            }
            let pkt: Vec<u8> = self.inbuf[4..4 + len].to_vec();
            self.inbuf.drain(..4 + len);
            out.push(self.handle(&pkt));
        }
        out
    }

    fn handle(&mut self, pkt: &[u8]) -> Vec<u8> {
        let ty = pkt[0];
        let mut p = 1usize;
        if ty == FXP_INIT {
            let mut b = alloc::vec![FXP_VERSION];
            put_u32(&mut b, 3);
            return packet(b);
        }
        let Some(id) = get_u32(pkt, &mut p) else { return status(0, FX_BAD_MESSAGE, "short packet") };
        let bad = || status(id, FX_BAD_MESSAGE, "malformed request");
        match ty {
            FXP_REALPATH => {
                let Some(raw) = get_str(pkt, &mut p) else { return bad() };
                let r = self.resolve(&raw);
                let shown = format!("/{r}");
                let mut b = alloc::vec![FXP_NAME];
                put_u32(&mut b, id);
                put_u32(&mut b, 1);
                put_str(&mut b, shown.as_bytes());
                put_str(&mut b, shown.as_bytes());
                put_attrs(&mut b, kind(&r).unwrap_or(true), 0);
                packet(b)
            },
            FXP_STAT | FXP_LSTAT => {
                let Some(raw) = get_str(pkt, &mut p) else { return bad() };
                let r = self.resolve(&raw);
                match kind(&r) {
                    Some(is_dir) => {
                        let mut b = alloc::vec![FXP_ATTRS];
                        put_u32(&mut b, id);
                        put_attrs(&mut b, is_dir, if is_dir { 0 } else { size_of(&r) });
                        packet(b)
                    },
                    None => status(id, FX_NO_SUCH_FILE, "no such file"),
                }
            },
            FXP_FSTAT => {
                let Some(h) = get_bytes(pkt, &mut p) else { return bad() };
                let Some(i) = self.handle_index(h) else { return status(id, FX_FAILURE, "bad handle") };
                let mut b = alloc::vec![FXP_ATTRS];
                put_u32(&mut b, id);
                match &self.handles[i] {
                    Some(Handle::File { data, .. }) => put_attrs(&mut b, false, data.len() as u64),
                    _ => put_attrs(&mut b, true, 0),
                }
                packet(b)
            },
            FXP_SETSTAT | FXP_FSETSTAT => status(id, FX_OK, "ok"), // no permissions / times to set
            FXP_OPENDIR => {
                let Some(raw) = get_str(pkt, &mut p) else { return bad() };
                let r = self.resolve(&raw);
                match list(&r) {
                    Some(entries) => {
                        let h = self.new_handle(Handle::Dir { entries, pos: 0 });
                        let mut b = alloc::vec![FXP_HANDLE];
                        put_u32(&mut b, id);
                        put_str(&mut b, &h);
                        packet(b)
                    },
                    None => status(id, FX_NO_SUCH_FILE, "no such folder"),
                }
            },
            FXP_READDIR => {
                let Some(h) = get_bytes(pkt, &mut p) else { return bad() };
                let Some(i) = self.handle_index(h) else { return status(id, FX_FAILURE, "bad handle") };
                let owner = self.home.trim_start_matches("home/").to_string();
                let Some(Handle::Dir { entries, pos }) = &mut self.handles[i] else {
                    return status(id, FX_FAILURE, "not a folder");
                };
                if *pos >= entries.len() {
                    return status(id, FX_EOF, "end");
                }
                let end = (*pos + 64).min(entries.len());
                let mut b = alloc::vec![FXP_NAME];
                put_u32(&mut b, id);
                put_u32(&mut b, (end - *pos) as u32);
                for (name, is_dir, size) in &entries[*pos..end] {
                    put_str(&mut b, name.as_bytes());
                    put_str(&mut b, longname(name, *is_dir, *size, &owner).as_bytes());
                    put_attrs(&mut b, *is_dir, *size);
                }
                *pos = end;
                packet(b)
            },
            FXP_OPEN => {
                let (Some(raw), Some(flags)) = (get_str(pkt, &mut p), get_u32(pkt, &mut p)) else { return bad() };
                let r = self.resolve(&raw);
                if r.is_empty() || kind(&r) == Some(true) {
                    return status(id, FX_FAILURE, "that is a folder");
                }
                let exists = kind(&r).is_some();
                if flags & PF_EXCL != 0 && exists {
                    return status(id, FX_FAILURE, "file exists");
                }
                let writable = flags & (PF_WRITE | PF_APPEND) != 0;
                let data = if exists && flags & PF_TRUNC == 0 {
                    match read_file(&r) {
                        Ok(v) => v,
                        Err(code) => return status(id, code, "cannot read the file"),
                    }
                } else if exists || flags & PF_CREAT != 0 {
                    Vec::new()
                } else {
                    return status(id, FX_NO_SUCH_FILE, "no such file");
                };
                if !writable && flags & PF_READ == 0 {
                    return bad();
                }
                // a new or truncated file exists from OPEN on (as clients expect)
                let dirty = writable && (!exists || flags & PF_TRUNC != 0);
                let h = self.new_handle(Handle::File { path: r, data, dirty, writable });
                let mut b = alloc::vec![FXP_HANDLE];
                put_u32(&mut b, id);
                put_str(&mut b, &h);
                packet(b)
            },
            FXP_READ => {
                let (Some(h), Some(off), Some(len)) = (get_bytes(pkt, &mut p), get_u64(pkt, &mut p), get_u32(pkt, &mut p))
                else {
                    return bad();
                };
                let Some(i) = self.handle_index(h) else { return status(id, FX_FAILURE, "bad handle") };
                let Some(Handle::File { data, .. }) = &self.handles[i] else {
                    return status(id, FX_FAILURE, "not a file");
                };
                let off = off as usize;
                if off >= data.len() {
                    return status(id, FX_EOF, "end of file");
                }
                let end = (off + (len as usize).min(64 * 1024)).min(data.len());
                let mut b = alloc::vec![FXP_DATA];
                put_u32(&mut b, id);
                put_str(&mut b, &data[off..end]);
                packet(b)
            },
            FXP_WRITE => {
                let (Some(h), Some(off), Some(chunk)) = (get_bytes(pkt, &mut p), get_u64(pkt, &mut p), get_bytes(pkt, &mut p))
                else {
                    return bad();
                };
                let Some(i) = self.handle_index(h) else { return status(id, FX_FAILURE, "bad handle") };
                let Some(Handle::File { data, dirty, writable, .. }) = &mut self.handles[i] else {
                    return status(id, FX_FAILURE, "not a file");
                };
                if !*writable {
                    return status(id, FX_PERMISSION_DENIED, "opened read-only");
                }
                let off = off as usize;
                let end = off + chunk.len();
                if end > MAX_FILE {
                    return status(id, FX_FAILURE, "file too large for LingOS storage (64 MiB)");
                }
                if data.len() < end {
                    data.resize(end, 0);
                }
                data[off..end].copy_from_slice(chunk);
                *dirty = true;
                status(id, FX_OK, "ok")
            },
            FXP_CLOSE => {
                let Some(h) = get_bytes(pkt, &mut p) else { return bad() };
                let Some(i) = self.handle_index(h) else { return status(id, FX_FAILURE, "bad handle") };
                match self.handles[i].take() {
                    Some(Handle::File { path, data, dirty: true, .. }) => match write_file(&path, &data) {
                        Ok(()) => {
                            crate::console_write(b"sftp: stored ");
                            crate::console_write(path.as_bytes());
                            crate::console_write(b"\n");
                            status(id, FX_OK, "ok")
                        },
                        Err(code) => status(id, code, "could not store the file"),
                    },
                    _ => status(id, FX_OK, "ok"),
                }
            },
            FXP_REMOVE => {
                let Some(raw) = get_str(pkt, &mut p) else { return bad() };
                let r = self.resolve(&raw);
                if kind(&r) != Some(false) {
                    return status(id, FX_NO_SUCH_FILE, "no such file");
                }
                match remove(&r) {
                    Ok(()) => status(id, FX_OK, "ok"),
                    Err(code) => status(id, code, "could not remove"),
                }
            },
            FXP_RMDIR => {
                let Some(raw) = get_str(pkt, &mut p) else { return bad() };
                let r = self.resolve(&raw);
                if kind(&r) != Some(true) || r.is_empty() {
                    return status(id, FX_NO_SUCH_FILE, "no such folder");
                }
                if list(&r).is_some_and(|l| !l.is_empty()) {
                    return status(id, FX_FAILURE, "folder not empty");
                }
                match remove(&r) {
                    Ok(()) => status(id, FX_OK, "ok"),
                    Err(code) => status(id, code, "could not remove"),
                }
            },
            FXP_MKDIR => {
                let Some(raw) = get_str(pkt, &mut p) else { return bad() };
                let r = self.resolve(&raw);
                if kind(&r).is_some() {
                    return status(id, FX_FAILURE, "already exists");
                }
                let ok = if in_home(&r) { crate::vfs::mkdir(&r) } else if media::is_media_path(&r) { media::mkdir(&r) } else { lingfs::mkdir_p(&r).is_ok() };
                if ok {
                    status(id, FX_OK, "ok")
                } else {
                    status(id, FX_FAILURE, "could not create the folder")
                }
            },
            FXP_RENAME => {
                let (Some(a), Some(b)) = (get_str(pkt, &mut p), get_str(pkt, &mut p)) else { return bad() };
                let (from, to) = (self.resolve(&a), self.resolve(&b));
                if kind(&from) != Some(false) {
                    return status(id, FX_FAILURE, "only files can be renamed");
                }
                if kind(&to).is_some() {
                    return status(id, FX_FAILURE, "the target exists");
                }
                if media::is_media_path(&from) && media::is_media_path(&to) && media::rename(&from, &to) {
                    return status(id, FX_OK, "ok");
                }
                match read_file(&from).and_then(|d| write_file(&to, &d)).and_then(|_| remove(&from)) {
                    Ok(()) => status(id, FX_OK, "ok"),
                    Err(code) => status(id, code, "rename failed"),
                }
            },
            FXP_READLINK | FXP_SYMLINK | FXP_EXTENDED => status(id, FX_OP_UNSUPPORTED, "not supported"),
            _ => status(id, FX_OP_UNSUPPORTED, "not supported"),
        }
    }
}
