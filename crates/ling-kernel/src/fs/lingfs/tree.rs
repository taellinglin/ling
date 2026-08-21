//! Tree directory listings and entry metadata.

use crate::fs::lingfs::objects::{self, Hash, BLOCK_SIZE, ZERO_HASH};

pub const OWNER_LEN: usize = 16;
pub const MODE_FILE_DEFAULT: u16 = 0o644;
pub const MODE_DIR_DEFAULT: u16 = 0o755;

#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Kind {
    Blob = 0,
    Tree = 1,
}

#[derive(Copy, Clone)]
pub struct Meta {
    pub mode: u16,
    pub owner: [u8; OWNER_LEN],
    pub group: [u8; OWNER_LEN],
    pub modified_seq: u32,
}

pub const EMPTY_META: Meta = Meta {
    mode: 0,
    owner: [0u8; OWNER_LEN],
    group: [0u8; OWNER_LEN],
    modified_seq: 0,
};

static mut CURRENT_USER: [u8; OWNER_LEN] = *b"root\0\0\0\0\0\0\0\0\0\0\0\0";
static mut CURRENT_GROUP: [u8; OWNER_LEN] = *b"root\0\0\0\0\0\0\0\0\0\0\0\0";

fn str_to_owner_buf(s: &str) -> [u8; OWNER_LEN] {
    let mut buf = [0u8; OWNER_LEN];
    let n = s.len().min(OWNER_LEN - 1);
    buf[..n].copy_from_slice(&s.as_bytes()[..n]);
    buf
}

pub fn set_current_user(user: &str, group: &str) {
    unsafe {
        CURRENT_USER = str_to_owner_buf(user);
        CURRENT_GROUP = str_to_owner_buf(group);
    }
}

pub fn current_user() -> &'static str {
    unsafe {
        let buf = &*&raw const CURRENT_USER;
        let len = buf.iter().position(|&b| b == 0).unwrap_or(OWNER_LEN);
        core::str::from_utf8(&buf[..len]).unwrap_or("root")
    }
}

pub fn make_meta(mode: u16, seq: u32) -> Meta {
    unsafe {
        Meta {
            mode,
            owner: CURRENT_USER,
            group: CURRENT_GROUP,
            modified_seq: seq,
        }
    }
}

pub const TREE_ENTRY_NAME_LEN: usize = 55;
pub const TREE_ENTRY_BYTES: usize = TREE_ENTRY_NAME_LEN + 32 + 1 + 2 + OWNER_LEN + OWNER_LEN + 4;
pub const MAX_TREE_ENTRIES: usize = BLOCK_SIZE / TREE_ENTRY_BYTES;

pub type TreeEntries = [(([u8; TREE_ENTRY_NAME_LEN], usize), Hash, Kind, Meta); MAX_TREE_ENTRIES];
pub const EMPTY_TREE_ENTRY: (([u8; TREE_ENTRY_NAME_LEN], usize), Hash, Kind, Meta) =
    (([0u8; TREE_ENTRY_NAME_LEN], 0), ZERO_HASH, Kind::Blob, EMPTY_META);

pub fn name_eq(entry: &([u8; TREE_ENTRY_NAME_LEN], usize), name: &str) -> bool {
    let (buf, len) = entry;
    *len == name.len() && &buf[..*len] == name.as_bytes()
}

pub fn str_to_name_buf(name: &str) -> [u8; TREE_ENTRY_NAME_LEN] {
    let mut buf = [0u8; TREE_ENTRY_NAME_LEN];
    let n = name.len().min(TREE_ENTRY_NAME_LEN - 1);
    buf[..n].copy_from_slice(&name.as_bytes()[..n]);
    buf
}

pub fn put_tree(entries: &[(&str, Hash, Kind, Meta)]) -> Result<Hash, ()> {
    if entries.len() > MAX_TREE_ENTRIES {
        return Err(());
    }
    let mut buf = [0u8; BLOCK_SIZE];
    for (i, (name, hash, kind, meta)) in entries.iter().enumerate() {
        let off = i * TREE_ENTRY_BYTES;
        let name_bytes = name.as_bytes();
        let n = name_bytes.len().min(TREE_ENTRY_NAME_LEN - 1);
        buf[off..off + n].copy_from_slice(&name_bytes[..n]);
        let mut o = off + TREE_ENTRY_NAME_LEN;
        buf[o..o + 32].copy_from_slice(hash);
        o += 32;
        buf[o] = *kind as u8;
        o += 1;
        buf[o..o + 2].copy_from_slice(&meta.mode.to_le_bytes());
        o += 2;
        buf[o..o + OWNER_LEN].copy_from_slice(&meta.owner);
        o += OWNER_LEN;
        buf[o..o + OWNER_LEN].copy_from_slice(&meta.group);
        o += OWNER_LEN;
        buf[o..o + 4].copy_from_slice(&meta.modified_seq.to_le_bytes());
    }
    objects::put_object(&buf[..entries.len() * TREE_ENTRY_BYTES])
}

pub fn get_tree(hash: &Hash, out: &mut TreeEntries) -> Result<Option<usize>, ()> {
    let mut buf = [0u8; BLOCK_SIZE];
    let Some(len) = objects::get_object(hash, &mut buf)? else { return Ok(None) };
    let count = len / TREE_ENTRY_BYTES;
    for i in 0..count {
        let off = i * TREE_ENTRY_BYTES;
        let mut name = [0u8; TREE_ENTRY_NAME_LEN];
        name.copy_from_slice(&buf[off..off + TREE_ENTRY_NAME_LEN]);
        let name_len = name.iter().position(|&b| b == 0).unwrap_or(TREE_ENTRY_NAME_LEN);
        let mut o = off + TREE_ENTRY_NAME_LEN;
        let mut entry_hash = ZERO_HASH;
        entry_hash.copy_from_slice(&buf[o..o + 32]);
        o += 32;
        let kind = if buf[o] == 1 { Kind::Tree } else { Kind::Blob };
        o += 1;
        let mode = u16::from_le_bytes(buf[o..o + 2].try_into().unwrap());
        o += 2;
        let mut owner = [0u8; OWNER_LEN];
        owner.copy_from_slice(&buf[o..o + OWNER_LEN]);
        o += OWNER_LEN;
        let mut group = [0u8; OWNER_LEN];
        group.copy_from_slice(&buf[o..o + OWNER_LEN]);
        o += OWNER_LEN;
        let modified_seq = u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
        out[i] = ((name, name_len), entry_hash, kind, Meta { mode, owner, group, modified_seq });
    }
    Ok(Some(count))
}

pub fn print_mode(mode: u16, is_dir: bool) {
    crate::console_write(if is_dir { b"d" } else { b"-" });
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 0b111;
        crate::console_write(if bits & 0b100 != 0 { b"r" } else { b"-" });
        crate::console_write(if bits & 0b010 != 0 { b"w" } else { b"-" });
        crate::console_write(if bits & 0b001 != 0 { b"x" } else { b"-" });
    }
}

pub fn trim_owner(buf: &[u8; OWNER_LEN]) -> &[u8] {
    let len = buf.iter().position(|&b| b == 0).unwrap_or(OWNER_LEN);
    &buf[..len]
}

pub fn print_decimal(n: u32) {
    if n == 0 {
        crate::console_write(b"0");
        return;
    }
    let mut digits = [0u8; 10];
    let mut i = digits.len();
    let mut v = n;
    while v > 0 {
        i -= 1;
        digits[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    crate::console_write(&digits[i..]);
}

pub fn print_ls_entry(entry: &(([u8; TREE_ENTRY_NAME_LEN], usize), Hash, Kind, Meta), long: bool) {
    let ((name_buf, name_len), hash, kind, meta) = entry;
    let is_dir = *kind == Kind::Tree;
    if long {
        print_mode(meta.mode, is_dir);
        crate::console_write(b" ");
        crate::console_write(trim_owner(&meta.owner));
        crate::console_write(b":");
        crate::console_write(trim_owner(&meta.group));
        crate::console_write(b" ");
        print_decimal(objects::object_len(hash) as u32);
        crate::console_write(b" #");
        print_decimal(meta.modified_seq);
        crate::console_write(b" ");
    }
    crate::console_write(&name_buf[..*name_len]);
    if is_dir {
        crate::console_write(b"/");
    }
    crate::console_write(b"\n");
}
