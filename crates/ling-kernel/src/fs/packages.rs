//! `lingpkg`: package format and installer supporting both v1 and v2 packages.
//!
//! Format v2 (`LPK2`, little-endian):
//! ```text
//! magic:      4 bytes, "LPK2"
//! pubkey:     32 bytes (Ed25519 public key; all zeros = unsigned)
//! signature:  64 bytes (Ed25519 signature over offset 100..EOF)
//! name:       u16 len, then that many bytes
//! version:    u16 len, then that many bytes
//! files:      u16 count, then that many entries:
//!               kind:    u8 (0 = data/config, 1 = executable binary -> /bin/)
//!               name:    u16 len, then bytes
//!               content: u32 len, then bytes
//! ```

use crate::fs::lingfs::{self, BLOCK_SIZE};

const MAGIC_V1: &[u8; 4] = b"LPK1";
const MAGIC_V2: &[u8; 4] = b"LPK2";
const PKG_DIR_PREFIX: &str = "pkg-";
const PACKAGES_DIR: &str = "packages";
const BIN_DIR: &str = "bin";
const MAX_FILES: usize = 32;

fn read_u16(buf: &[u8], off: usize) -> Option<(u16, usize)> {
    let b = buf.get(off..off + 2)?;
    Some((u16::from_le_bytes([b[0], b[1]]), off + 2))
}

fn read_u32(buf: &[u8], off: usize) -> Option<(u32, usize)> {
    let b = buf.get(off..off + 4)?;
    Some((u32::from_le_bytes([b[0], b[1], b[2], b[3]]), off + 4))
}

fn read_bytes(buf: &[u8], off: usize, len: usize) -> Option<(&[u8], usize)> {
    let b = buf.get(off..off + len)?;
    Some((b, off + len))
}

/// Parse and install a `.lpkg` blob already stored at `blob_name` in lingfs.
pub fn install(blob_name: &str) -> bool {
    let mut raw = [0u8; BLOCK_SIZE];
    let Ok(Some(len)) = lingfs::read_file(blob_name, &mut raw) else { return false };
    install_bytes(&raw[..len])
}

/// Parse and install a `.lpkg` image directly from memory.
pub fn install_from_ptr(ptr: *const u8, len: usize) -> bool {
    if ptr.is_null() || len == 0 || len > BLOCK_SIZE * (MAX_FILES + 1) {
        return false;
    }
    let buf = unsafe { core::slice::from_raw_parts(ptr, len) };
    install_bytes(buf)
}

fn install_bytes(buf: &[u8]) -> bool {
    if buf.len() < 4 {
        return false;
    }

    if &buf[0..4] == MAGIC_V1 {
        return install_v1(buf);
    } else if &buf[0..4] == MAGIC_V2 {
        return install_v2(buf);
    }
    false
}

fn install_v1(buf: &[u8]) -> bool {
    let mut off = 4usize;

    let Some((name_len, o)) = read_u16(buf, off) else { return false };
    off = o;
    let Some((name_bytes, o)) = read_bytes(buf, off, name_len as usize) else { return false };
    off = o;
    let Ok(name) = core::str::from_utf8(name_bytes) else { return false };

    let Some((ver_len, o)) = read_u16(buf, off) else { return false };
    off = o;
    let Some((ver_bytes, o)) = read_bytes(buf, off, ver_len as usize) else { return false };
    off = o;

    let Some((file_count, o)) = read_u16(buf, off) else { return false };
    off = o;
    if file_count as usize > MAX_FILES {
        return false;
    }

    let mut pkg_dir_buf = [0u8; 48];
    let prefix = PKG_DIR_PREFIX.as_bytes();
    let n = prefix.len() + name.len().min(pkg_dir_buf.len() - prefix.len());
    pkg_dir_buf[..prefix.len()].copy_from_slice(prefix);
    pkg_dir_buf[prefix.len()..n].copy_from_slice(&name.as_bytes()[..n - prefix.len()]);
    let Ok(pkg_dir) = core::str::from_utf8(&pkg_dir_buf[..n]) else { return false };

    for _ in 0..file_count {
        let Some((fname_len, o)) = read_u16(buf, off) else { return false };
        off = o;
        let Some((fname_bytes, o)) = read_bytes(buf, off, fname_len as usize) else { return false };
        off = o;
        let Ok(fname) = core::str::from_utf8(fname_bytes) else { return false };

        let Some((content_len, o)) = read_u32(buf, off) else { return false };
        off = o;
        let Some((content, o)) = read_bytes(buf, off, content_len as usize) else { return false };
        off = o;

        if lingfs::write_in_dir(pkg_dir, fname, content).is_err() {
            return false;
        }
    }

    lingfs::write_in_dir(PACKAGES_DIR, name, ver_bytes).is_ok()
}

fn install_v2(buf: &[u8]) -> bool {
    if buf.len() < 100 {
        return false;
    }

    let pubkey = &buf[4..36];
    let signature = &buf[36..100];
    let payload = &buf[100..];

    // If signed (pubkey non-zero), verify signature over payload
    let is_signed = pubkey.iter().any(|&b| b != 0);
    if is_signed && !crate::ed25519::verify(pubkey, payload, signature) {
        return false;
    }

    let mut off = 100usize;

    let Some((name_len, o)) = read_u16(buf, off) else { return false };
    off = o;
    let Some((name_bytes, o)) = read_bytes(buf, off, name_len as usize) else { return false };
    off = o;
    let Ok(name) = core::str::from_utf8(name_bytes) else { return false };

    let Some((ver_len, o)) = read_u16(buf, off) else { return false };
    off = o;
    let Some((ver_bytes, o)) = read_bytes(buf, off, ver_len as usize) else { return false };
    off = o;

    let Some((file_count, o)) = read_u16(buf, off) else { return false };
    off = o;
    if file_count as usize > MAX_FILES {
        return false;
    }

    let mut pkg_dir_buf = [0u8; 48];
    let prefix = PKG_DIR_PREFIX.as_bytes();
    let n = prefix.len() + name.len().min(pkg_dir_buf.len() - prefix.len());
    pkg_dir_buf[..prefix.len()].copy_from_slice(prefix);
    pkg_dir_buf[prefix.len()..n].copy_from_slice(&name.as_bytes()[..n - prefix.len()]);
    let Ok(pkg_dir) = core::str::from_utf8(&pkg_dir_buf[..n]) else { return false };

    for _ in 0..file_count {
        let kind = match buf.get(off) {
            Some(&k) => k,
            None => return false,
        };
        off += 1;

        let Some((fname_len, o)) = read_u16(buf, off) else { return false };
        off = o;
        let Some((fname_bytes, o)) = read_bytes(buf, off, fname_len as usize) else { return false };
        off = o;
        let Ok(fname) = core::str::from_utf8(fname_bytes) else { return false };

        let Some((content_len, o)) = read_u32(buf, off) else { return false };
        off = o;
        let Some((content, o)) = read_bytes(buf, off, content_len as usize) else { return false };
        off = o;

        let target_dir = if kind == 1 { BIN_DIR } else { pkg_dir };
        if lingfs::write_in_dir(target_dir, fname, content).is_err() {
            return false;
        }
    }

    lingfs::write_in_dir(PACKAGES_DIR, name, ver_bytes).is_ok()
}
