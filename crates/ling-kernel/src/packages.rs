//! `lingpkg`: a minimal local package format and installer — no network
//! (see `packages/README.md`'s roadmap: the real public-repository step is
//! still blocked on a NIC driver/TCP stack that doesn't exist), no
//! signing yet (needs the no_std `ling-crypto` port), and no way to
//! *execute* an installed program (this kernel has no process model —
//! every package here is data/config, installed onto lingfs, not a
//! runnable binary). Still a real, useful `dpkg`-style local install: a
//! `.lpkg` blob already sitting in lingfs (however it got there — typed in
//! via `write`, or, the more realistic path, a Multiboot2 module the same
//! way the disk-boot bootloader payload arrives) gets unpacked into its
//! own one-level lingfs directory and recorded in `packages/`.
//!
//! Format (all integers little-endian):
//! ```text
//! magic:    4 bytes, "LPK1"
//! name:     u16 len, then that many bytes
//! version:  u16 len, then that many bytes
//! files:    u16 count, then that many entries:
//!             name: u16 len, then bytes
//!             content: u32 len, then bytes
//! ```
use crate::lingfs::{self, BLOCK_SIZE};

const MAGIC: &[u8; 4] = b"LPK1";
const PKG_DIR_PREFIX: &str = "pkg-";
const PACKAGES_DIR: &str = "packages";
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

/// Parse and install a `.lpkg` blob already stored at `blob_name` in
/// lingfs: writes every contained file into a one-level `pkg-<name>`
/// directory and records `<name> -> version` under `packages/`. Returns
/// `false` on a malformed blob, missing source blob, or a lingfs write
/// failure.
pub fn install(blob_name: &str) -> bool {
    let mut raw = [0u8; BLOCK_SIZE];
    let Ok(Some(len)) = lingfs::read_file(blob_name, &mut raw) else { return false };
    install_bytes(&raw[..len])
}

/// Parse and install a `.lpkg` image directly from memory (e.g. a
/// Multiboot2 module's bytes — see `ling_kernel_pkg_install_module`) —
/// the realistic way package bytes actually get onto a system with no
/// network stack and no way to type binary content at a keyboard, the same
/// mechanism the disk-boot bootloader payload already arrives through.
pub fn install_from_ptr(ptr: *const u8, len: usize) -> bool {
    // Sanity bound, not a real format limit: MAX_FILES files at up to one
    // lingfs block each, plus slack for the format's own header/name
    // fields -- big enough for any package that could actually install
    // (lingfs caps a single object at `BLOCK_SIZE`), small enough to
    // reject a garbage (ptr, len) without touching wild memory.
    if ptr.is_null() || len == 0 || len > BLOCK_SIZE * (MAX_FILES + 1) {
        return false;
    }
    let buf = unsafe { core::slice::from_raw_parts(ptr, len) };
    install_bytes(buf)
}

fn install_bytes(buf: &[u8]) -> bool {
    if buf.len() < 4 || &buf[0..4] != MAGIC {
        return false;
    }
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
