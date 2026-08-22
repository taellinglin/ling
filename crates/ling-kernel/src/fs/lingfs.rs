//! lingfs: a git-style, content-addressed object store as LingOS's root
//! filesystem (the boot partition stays FAT32 — GRUB and the RPi firmware
//! only know how to read that, so it isn't replaceable).
//!
//! Objects (blobs and trees) are identified by the BLAKE3 hash of their
//! plaintext content and are never modified in place — writing "the same"
//! content twice is a no-op (dedup for free), and a crash mid-write can
//! only ever leave a dangling *new* object, never corrupt an existing one,
//! since nothing already reachable from the root is ever overwritten. A
//! tree is a directory listing of (name, hash, kind) entries, exactly
//! git's blob/tree model. `set_root`/the commit log give real history:
//! every root change is appended, not replacing the log entry before it.
//!
//! This is the base layer only — no compression or encryption yet (see
//! the LingOS README/packages roadmap for what layers on next). Fixed-size
//! arrays throughout: `ling-kernel` has no heap allocator.
//!
//! Reads/writes go through `blockdev`, which tries AHCI (SATA) first and
//! falls back to legacy ATA PIO — so this works whether the disk is
//! attached via modern SATA (VirtualBox's default, most current real
//! hardware) or legacy IDE, without lingfs itself needing to know which.
use crate::fs::blockdev as ata;
use crate::fs::blockdev::SECTOR_SIZE;

pub const BLOCK_SIZE: usize = 4096;
const SECTORS_PER_BLOCK: usize = BLOCK_SIZE / SECTOR_SIZE;

/// Cap on distinct objects this volume can hold — 4096 objects * 4KiB each
/// is 16MiB of unique content, a reasonable ceiling for a first cut. Raise
/// this (and the on-disk layout math below) once that's actually cramped.
pub const MAX_OBJECTS: usize = 4096;
/// Cap on root-change history entries kept in the commit log.
pub const MAX_COMMITS: usize = 1024;

const MAGIC: u64 = 0x53_46_47_4E_4C_47_4E_4C; // "LNGLNGFS" little-endian-ish tag

pub type Hash = [u8; 32];
const ZERO_HASH: Hash = [0u8; 32];

#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Kind {
    Blob = 0,
    Tree = 1,
}

// ── Per-entry metadata: owner/group/mode/modified ────────────────────────
// Arch-`ls -l`-flavored, but honest about what it isn't: there's no
// process/privilege model in this kernel yet, so `mode`/`owner`/`group`
// are stored and displayed, not *enforced* — nothing currently stops a
// program from reading/writing regardless of these bits. `modified_seq` is
// a monotonically increasing logical write counter (incremented on every
// commit), not a wall-clock timestamp — there's no RTC driver, so there's
// no way to know real time yet.

pub const OWNER_LEN: usize = 16;
/// rw-r--r-- — the default for a new file.
pub const MODE_FILE_DEFAULT: u16 = 0o644;
/// rwxr-xr-x — the default for a new directory.
pub const MODE_DIR_DEFAULT: u16 = 0o755;

#[derive(Copy, Clone)]
pub struct Meta {
    pub mode: u16,
    pub owner: [u8; OWNER_LEN],
    pub group: [u8; OWNER_LEN],
    pub modified_seq: u32,
}

const EMPTY_META: Meta = Meta {
    mode: 0,
    owner: [0u8; OWNER_LEN],
    group: [0u8; OWNER_LEN],
    modified_seq: 0,
};

fn str_to_owner_buf(s: &str) -> [u8; OWNER_LEN] {
    let mut buf = [0u8; OWNER_LEN];
    let n = s.len().min(OWNER_LEN - 1);
    buf[..n].copy_from_slice(&s.as_bytes()[..n]);
    buf
}

/// The user/group new writes get attributed to. No login/session system
/// yet — this is one global, defaulting to `root`/`root`, changed only by
/// whatever a future `su`-equivalent command sets it to.
static mut CURRENT_USER: [u8; OWNER_LEN] = *b"root\0\0\0\0\0\0\0\0\0\0\0\0";
static mut CURRENT_GROUP: [u8; OWNER_LEN] = *b"root\0\0\0\0\0\0\0\0\0\0\0\0";

pub fn set_current_user(user: &str, group: &str) {
    unsafe {
        CURRENT_USER = str_to_owner_buf(user);
        CURRENT_GROUP = str_to_owner_buf(group);
    }
}

/// The user new lingfs writes are currently attributed to (`"root"` until
/// something calls `set_current_user`, e.g. a successful `login`).
pub fn current_user() -> &'static str {
    unsafe {
        let buf = &*&raw const CURRENT_USER;
        let len = buf.iter().position(|&b| b == 0).unwrap_or(OWNER_LEN);
        core::str::from_utf8(&buf[..len]).unwrap_or("root")
    }
}

fn make_meta(mode: u16) -> Meta {
    unsafe {
        Meta {
            mode,
            owner: CURRENT_USER,
            group: CURRENT_GROUP,
            modified_seq: next_seq(),
        }
    }
}

#[derive(Copy, Clone)]
struct IndexEntry {
    hash: Hash,
    block: u32, // data-region block index (not absolute LBA — see block_to_lba)
    len: u16,   // valid bytes in that block (<= BLOCK_SIZE)
    used: bool,
}

const EMPTY_ENTRY: IndexEntry = IndexEntry { hash: ZERO_HASH, block: 0, len: 0, used: false };

#[derive(Copy, Clone)]
struct CommitEntry {
    root: Hash,
    parent: u32, // index into the commit log, u32::MAX for "no parent" (the first commit)
}

const EMPTY_COMMIT: CommitEntry = CommitEntry { root: ZERO_HASH, parent: u32::MAX };

struct Volume {
    mounted: bool,
    object_count: u32,
    next_free_block: u32,
    commit_count: u32,
    next_seq: u32,
    index: [IndexEntry; MAX_OBJECTS],
    commits: [CommitEntry; MAX_COMMITS],
}

static mut VOLUME: Volume = Volume {
    mounted: false,
    object_count: 0,
    next_free_block: 0,
    commit_count: 0,
    next_seq: 0,
    index: [EMPTY_ENTRY; MAX_OBJECTS],
    commits: [EMPTY_COMMIT; MAX_COMMITS],
};

/// The next value `make_meta` will stamp a written entry with, then
/// advance — a monotonically increasing logical "when" ordering across
/// this volume's whole lifetime (persisted in the superblock), since
/// there's no RTC to get a real timestamp from.
fn next_seq() -> u32 {
    unsafe {
        let seq = VOLUME.next_seq;
        VOLUME.next_seq += 1;
        seq
    }
}

// ── On-disk layout ───────────────────────────────────────────────────────
// Block 0:                     superblock
// Blocks 1..=INDEX_BLOCKS:      object index (IndexEntry array, packed)
// Blocks after that:           commit log (CommitEntry array, packed)
// Remaining blocks:            object data, one object per block, appended
//                               from block 0 of that region upward

const INDEX_ENTRY_BYTES: usize = 32 + 4 + 2 + 1; // hash + block + len + used flag, packed
const INDEX_BLOCKS: usize = (MAX_OBJECTS * INDEX_ENTRY_BYTES).div_ceil(BLOCK_SIZE);
const COMMIT_ENTRY_BYTES: usize = 32 + 4;
const COMMIT_BLOCKS: usize = (MAX_COMMITS * COMMIT_ENTRY_BYTES).div_ceil(BLOCK_SIZE);
const DATA_REGION_START: u32 = 1 + INDEX_BLOCKS as u32 + COMMIT_BLOCKS as u32;

/// First sector the lingfs volume itself occupies. LBA 0..LINGFS_BASE_LBA
/// is reserved for the disk-boot bootloader (MBR stage1 + stage2 + the
/// flattened kernel image it loads) — without this offset, lingfs's own
/// superblock lands at LBA 0, exactly where the BIOS loads the MBR from,
/// and installing a bootloader would corrupt the filesystem (or vice
/// versa). 8192 sectors (4MiB) leaves generous room for the kernel binary;
/// see packages/README.md for the exact reserved layout.
const LINGFS_BASE_LBA: u32 = 8192;

fn block_to_lba(block: u32) -> u32 {
    LINGFS_BASE_LBA + (DATA_REGION_START + block) * SECTORS_PER_BLOCK as u32
}

fn read_block(block_lba_base: u32, out: &mut [u8; BLOCK_SIZE]) -> Result<(), ()> {
    for i in 0..SECTORS_PER_BLOCK {
        let mut sector = [0u8; SECTOR_SIZE];
        ata::read_sector(block_lba_base + i as u32, &mut sector)?;
        out[i * SECTOR_SIZE..(i + 1) * SECTOR_SIZE].copy_from_slice(&sector);
    }
    Ok(())
}

fn write_block(block_lba_base: u32, data: &[u8; BLOCK_SIZE]) -> Result<(), ()> {
    for i in 0..SECTORS_PER_BLOCK {
        let mut sector = [0u8; SECTOR_SIZE];
        sector.copy_from_slice(&data[i * SECTOR_SIZE..(i + 1) * SECTOR_SIZE]);
        ata::write_sector(block_lba_base + i as u32, &sector)?;
    }
    Ok(())
}

// ── Superblock ───────────────────────────────────────────────────────────

fn load_superblock() -> Result<(u32, u32, u32, u32), ()> {
    let mut block = [0u8; BLOCK_SIZE];
    read_block(LINGFS_BASE_LBA, &mut block)?;
    let magic = u64::from_le_bytes(block[0..8].try_into().unwrap());
    if magic != MAGIC {
        return Err(());
    }
    let object_count = u32::from_le_bytes(block[8..12].try_into().unwrap());
    let next_free_block = u32::from_le_bytes(block[12..16].try_into().unwrap());
    let commit_count = u32::from_le_bytes(block[16..20].try_into().unwrap());
    let next_seq = u32::from_le_bytes(block[20..24].try_into().unwrap());
    Ok((object_count, next_free_block, commit_count, next_seq))
}

fn save_superblock() -> Result<(), ()> {
    unsafe {
        let mut block = [0u8; BLOCK_SIZE];
        block[0..8].copy_from_slice(&MAGIC.to_le_bytes());
        block[8..12].copy_from_slice(&VOLUME.object_count.to_le_bytes());
        block[12..16].copy_from_slice(&VOLUME.next_free_block.to_le_bytes());
        block[16..20].copy_from_slice(&VOLUME.commit_count.to_le_bytes());
        block[20..24].copy_from_slice(&VOLUME.next_seq.to_le_bytes());
        write_block(LINGFS_BASE_LBA, &block)
    }
}

/// Format a fresh, empty volume (wipes the superblock/index/commit-log
/// region — object data blocks are left alone, but with no index pointing
/// at them they're unreachable, same as `git gc`ing everything).
pub fn format() -> Result<(), ()> {
    unsafe {
        VOLUME.object_count = 0;
        VOLUME.next_free_block = 0;
        VOLUME.commit_count = 0;
        VOLUME.next_seq = 0;
        VOLUME.index = [EMPTY_ENTRY; MAX_OBJECTS];
        VOLUME.commits = [EMPTY_COMMIT; MAX_COMMITS];
        VOLUME.mounted = true;
    }
    save_superblock()?;
    save_index()?;
    save_commits()?;
    Ok(())
}

/// Mount the volume: read the superblock, and if its magic matches, load
/// the index and commit log into RAM. Formats a fresh volume if the magic
/// doesn't match (first boot on blank media) rather than refusing to boot.
pub fn mount() -> Result<(), ()> {
    match load_superblock() {
        Ok((object_count, next_free_block, commit_count, next_seq)) => {
            unsafe {
                VOLUME.object_count = object_count;
                VOLUME.next_free_block = next_free_block;
                VOLUME.commit_count = commit_count;
                VOLUME.next_seq = next_seq;
            }
            load_index()?;
            load_commits()?;
            unsafe { VOLUME.mounted = true };
            Ok(())
        },
        Err(()) => format(),
    }
}

// ── Index persistence (packed IndexEntry array across INDEX_BLOCKS) ──────

fn load_index() -> Result<(), ()> {
    let mut buf = [0u8; BLOCK_SIZE];
    for b in 0..INDEX_BLOCKS {
        read_block(block_index_lba(b), &mut buf)?;
        for slot in 0..(BLOCK_SIZE / INDEX_ENTRY_BYTES) {
            let idx = b * (BLOCK_SIZE / INDEX_ENTRY_BYTES) + slot;
            if idx >= MAX_OBJECTS {
                break;
            }
            let off = slot * INDEX_ENTRY_BYTES;
            let mut hash = ZERO_HASH;
            hash.copy_from_slice(&buf[off..off + 32]);
            let block = u32::from_le_bytes(buf[off + 32..off + 36].try_into().unwrap());
            let len = u16::from_le_bytes(buf[off + 36..off + 38].try_into().unwrap());
            let used = buf[off + 38] != 0;
            unsafe { VOLUME.index[idx] = IndexEntry { hash, block, len, used } };
        }
    }
    Ok(())
}

fn save_index() -> Result<(), ()> {
    for b in 0..INDEX_BLOCKS {
        let mut buf = [0u8; BLOCK_SIZE];
        for slot in 0..(BLOCK_SIZE / INDEX_ENTRY_BYTES) {
            let idx = b * (BLOCK_SIZE / INDEX_ENTRY_BYTES) + slot;
            if idx >= MAX_OBJECTS {
                break;
            }
            let e = unsafe { VOLUME.index[idx] };
            let off = slot * INDEX_ENTRY_BYTES;
            buf[off..off + 32].copy_from_slice(&e.hash);
            buf[off + 32..off + 36].copy_from_slice(&e.block.to_le_bytes());
            buf[off + 36..off + 38].copy_from_slice(&e.len.to_le_bytes());
            buf[off + 38] = e.used as u8;
        }
        write_block(block_index_lba(b), &buf)?;
    }
    Ok(())
}

fn block_index_lba(b: usize) -> u32 {
    LINGFS_BASE_LBA + (1 + b as u32) * SECTORS_PER_BLOCK as u32
}

// ── Commit log persistence ────────────────────────────────────────────────

fn commit_block_lba(b: usize) -> u32 {
    LINGFS_BASE_LBA + (1 + INDEX_BLOCKS as u32 + b as u32) * SECTORS_PER_BLOCK as u32
}

fn load_commits() -> Result<(), ()> {
    let mut buf = [0u8; BLOCK_SIZE];
    for b in 0..COMMIT_BLOCKS {
        read_block(commit_block_lba(b), &mut buf)?;
        for slot in 0..(BLOCK_SIZE / COMMIT_ENTRY_BYTES) {
            let idx = b * (BLOCK_SIZE / COMMIT_ENTRY_BYTES) + slot;
            if idx >= MAX_COMMITS {
                break;
            }
            let off = slot * COMMIT_ENTRY_BYTES;
            let mut root = ZERO_HASH;
            root.copy_from_slice(&buf[off..off + 32]);
            let parent = u32::from_le_bytes(buf[off + 32..off + 36].try_into().unwrap());
            unsafe { VOLUME.commits[idx] = CommitEntry { root, parent } };
        }
    }
    Ok(())
}

fn save_commits() -> Result<(), ()> {
    for b in 0..COMMIT_BLOCKS {
        let mut buf = [0u8; BLOCK_SIZE];
        for slot in 0..(BLOCK_SIZE / COMMIT_ENTRY_BYTES) {
            let idx = b * (BLOCK_SIZE / COMMIT_ENTRY_BYTES) + slot;
            if idx >= MAX_COMMITS {
                break;
            }
            let e = unsafe { VOLUME.commits[idx] };
            let off = slot * COMMIT_ENTRY_BYTES;
            buf[off..off + 32].copy_from_slice(&e.root);
            buf[off + 32..off + 36].copy_from_slice(&e.parent.to_le_bytes());
        }
        write_block(commit_block_lba(b), &buf)?;
    }
    Ok(())
}

// ── Object store ──────────────────────────────────────────────────────────

fn find(hash: &Hash) -> Option<usize> {
    for i in 0..MAX_OBJECTS {
        let e = unsafe { VOLUME.index[i] };
        if e.used && &e.hash == hash {
            return Some(i);
        }
    }
    None
}

/// Store `data` (<= `BLOCK_SIZE` bytes) as a content-addressed object.
/// Returns its BLAKE3 hash. Writing identical content twice is a no-op the
/// second time (dedup) — the existing object is reused, nothing is
/// written to disk again.
pub fn put_object(data: &[u8]) -> Result<Hash, ()> {
    if data.len() > BLOCK_SIZE {
        return Err(()); // a single object is one block in this v1 format
    }
    let hash = crate::hash::blake3(data);
    if find(&hash).is_some() {
        return Ok(hash); // dedup: identical content already stored
    }
    unsafe {
        if VOLUME.object_count as usize >= MAX_OBJECTS {
            return Err(());
        }
        let block = VOLUME.next_free_block;
        let mut buf = [0u8; BLOCK_SIZE];
        buf[..data.len()].copy_from_slice(data);
        write_block(block_to_lba(block), &buf)?;

        let slot = VOLUME.object_count as usize;
        VOLUME.index[slot] = IndexEntry { hash, block, len: data.len() as u16, used: true };
        VOLUME.object_count += 1;
        VOLUME.next_free_block += 1;
    }
    save_index()?;
    save_superblock()?;
    Ok(hash)
}

/// Read back the object with the given hash into `out`, returning how many
/// bytes of it are valid. `None` if no such object exists.
pub fn get_object(hash: &Hash, out: &mut [u8; BLOCK_SIZE]) -> Result<Option<usize>, ()> {
    let Some(slot) = find(hash) else { return Ok(None) };
    let e = unsafe { VOLUME.index[slot] };
    read_block(block_to_lba(e.block), out)?;
    Ok(Some(e.len as usize))
}

// ── Tree objects: a directory listing, git-style ─────────────────────────

const TREE_ENTRY_NAME_LEN: usize = 55;
// name + hash + kind + mode + owner + group + modified_seq
const TREE_ENTRY_BYTES: usize = TREE_ENTRY_NAME_LEN + 32 + 1 + 2 + OWNER_LEN + OWNER_LEN + 4;
const MAX_TREE_ENTRIES: usize = BLOCK_SIZE / TREE_ENTRY_BYTES;

/// Build and store a tree object from `entries` (name, content hash, kind,
/// metadata). Names longer than 54 bytes are truncated — this is a "basic"
/// filesystem, not a general-purpose one.
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
    put_object(&buf[..entries.len() * TREE_ENTRY_BYTES])
}

/// Read a tree object back into a fixed-size scratch array, returning how
/// many entries it has. `out` must hold at least that many.
pub fn get_tree(
    hash: &Hash,
    out: &mut [(([u8; TREE_ENTRY_NAME_LEN], usize), Hash, Kind, Meta); MAX_TREE_ENTRIES],
) -> Result<Option<usize>, ()> {
    let mut buf = [0u8; BLOCK_SIZE];
    let Some(len) = get_object(hash, &mut buf)? else { return Ok(None) };
    let count = len / TREE_ENTRY_BYTES;
    for i in 0..count {
        let off = i * TREE_ENTRY_BYTES;
        let mut name = [0u8; TREE_ENTRY_NAME_LEN];
        name.copy_from_slice(&buf[off..off + TREE_ENTRY_NAME_LEN]);
        let name_len = name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(TREE_ENTRY_NAME_LEN);
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
        out[i] = (
            (name, name_len),
            entry_hash,
            kind,
            Meta { mode, owner, group, modified_seq },
        );
    }
    Ok(Some(count))
}

type TreeEntries = [(([u8; TREE_ENTRY_NAME_LEN], usize), Hash, Kind, Meta); MAX_TREE_ENTRIES];
const EMPTY_TREE_ENTRY: (([u8; TREE_ENTRY_NAME_LEN], usize), Hash, Kind, Meta) = (
    ([0u8; TREE_ENTRY_NAME_LEN], 0),
    ZERO_HASH,
    Kind::Blob,
    EMPTY_META,
);

fn name_eq(entry: &([u8; TREE_ENTRY_NAME_LEN], usize), name: &str) -> bool {
    let (buf, len) = entry;
    *len == name.len() && &buf[..*len] == name.as_bytes()
}

/// Replace-or-add a single (name, hash, kind, meta) entry in the root tree
/// and commit the result — the shared "git add && git commit" step behind
/// both `write_file` (a `Kind::Blob` entry) and `write_dir` (a `Kind::Tree`
/// entry for a synthetic subdirectory like `dev/` or `bin/`). Replacing an
/// existing entry keeps its original owner/mode but stamps a fresh
/// `modified_seq`.
fn upsert_root_entry(name: &str, hash: Hash, kind: Kind, default_meta: Meta) -> Result<(), ()> {
    let mut entries: TreeEntries = [EMPTY_TREE_ENTRY; MAX_TREE_ENTRIES];
    let mut count = 0usize;
    let current_root = root();
    if current_root != ZERO_HASH {
        if let Some(n) = get_tree(&current_root, &mut entries)? {
            count = n;
        }
    }

    let mut replaced = false;
    for i in 0..count {
        if name_eq(&entries[i].0, name) {
            entries[i].1 = hash;
            entries[i].2 = kind;
            entries[i].3.modified_seq = next_seq();
            replaced = true;
            break;
        }
    }
    if !replaced {
        if count >= MAX_TREE_ENTRIES {
            return Err(());
        }
        entries[count] = (
            (str_to_name_buf(name), name.len()),
            hash,
            kind,
            default_meta,
        );
        count += 1;
    }

    let refs: [(&str, Hash, Kind, Meta); MAX_TREE_ENTRIES] = {
        let mut r = [("", ZERO_HASH, Kind::Blob, EMPTY_META); MAX_TREE_ENTRIES];
        for i in 0..count {
            let (buf, len) = &entries[i].0;
            // Safe: names are only ever written from valid UTF-8 (`&str`)
            // in this module, truncated at a byte length `put_tree` chose —
            // never split mid-codepoint by anything currently calling it,
            // since every name so far is plain ASCII.
            r[i] = (
                core::str::from_utf8(&buf[..*len]).unwrap_or(""),
                entries[i].1,
                entries[i].2,
                entries[i].3,
            );
        }
        r
    };
    let new_root = put_tree(&refs[..count])?;
    commit(new_root)
}

/// Create or overwrite a top-level file: stores `content` as a blob, then
/// points a root-tree entry named `name` at it — a real, if minimal,
/// `git add && git commit` equivalent.
/// `name` is resolved against `cwd` first (see `resolve`), same as
/// `read_file` — this is why `write_in_dir` (not just `upsert_root_entry`)
/// is needed here: a `cd`-relative or explicit `"dev/newfile"`-style write
/// has to land inside that one-level subdirectory, not as a literal
/// slash-containing root entry name.
pub fn write_file(name: &str, content: &[u8]) -> Result<(), ()> {
    let mut rbuf = [0u8; RESOLVE_BUF];
    let name = resolve(name, &mut rbuf);
    if let Some((dirname, fname)) = split_path(name) {
        return write_in_dir(dirname, fname, content);
    }
    let blob_hash = put_object(content)?;
    upsert_root_entry(name, blob_hash, Kind::Blob, make_meta(MODE_FILE_DEFAULT))
}

/// Create or overwrite a synthetic one-level subdirectory (`dev/`, `bin/`):
/// each `(name, content)` pair becomes a blob, gathered into a subtree, and
/// the root tree gets a `Kind::Tree` entry named `dirname` pointing at it.
/// Only one level deep — enough for `dev`/`bin`, not a general path
/// resolver.
pub fn write_dir(dirname: &str, files: &[(&str, &[u8])]) -> Result<(), ()> {
    if files.len() > MAX_TREE_ENTRIES {
        return Err(());
    }
    let mut refs: [(&str, Hash, Kind, Meta); MAX_TREE_ENTRIES] =
        [("", ZERO_HASH, Kind::Blob, EMPTY_META); MAX_TREE_ENTRIES];
    for (i, (fname, content)) in files.iter().enumerate() {
        let blob_hash = put_object(content)?;
        refs[i] = (fname, blob_hash, Kind::Blob, make_meta(MODE_FILE_DEFAULT));
    }
    let subtree_hash = put_tree(&refs[..files.len()])?;
    upsert_root_entry(
        dirname,
        subtree_hash,
        Kind::Tree,
        make_meta(MODE_DIR_DEFAULT),
    )
}

/// Replace-or-add a single `(fname, content)` blob inside a one-level
/// subdirectory, creating that subdirectory first if it doesn't exist yet
/// — unlike `write_dir` (which replaces the whole subdirectory at once,
/// right for `dev`/`bin`'s "reseed everything this boot" use), this is for
/// directories that grow one entry at a time (e.g. `users/`, one blob per
/// account).
pub fn write_in_dir(dirname: &str, fname: &str, content: &[u8]) -> Result<(), ()> {
    let mut entries: TreeEntries = [EMPTY_TREE_ENTRY; MAX_TREE_ENTRIES];
    let mut count = match lookup_dir(dirname)? {
        Some((existing, n)) => {
            entries = existing;
            n
        },
        None => 0,
    };

    let blob_hash = put_object(content)?;
    let mut replaced = false;
    for i in 0..count {
        if name_eq(&entries[i].0, fname) {
            entries[i].1 = blob_hash;
            entries[i].3.modified_seq = next_seq();
            replaced = true;
            break;
        }
    }
    if !replaced {
        if count >= MAX_TREE_ENTRIES {
            return Err(());
        }
        entries[count] = (
            (str_to_name_buf(fname), fname.len()),
            blob_hash,
            Kind::Blob,
            make_meta(MODE_FILE_DEFAULT),
        );
        count += 1;
    }

    let refs: [(&str, Hash, Kind, Meta); MAX_TREE_ENTRIES] = {
        let mut r = [("", ZERO_HASH, Kind::Blob, EMPTY_META); MAX_TREE_ENTRIES];
        for i in 0..count {
            let (buf, len) = &entries[i].0;
            r[i] = (
                core::str::from_utf8(&buf[..*len]).unwrap_or(""),
                entries[i].1,
                entries[i].2,
                entries[i].3,
            );
        }
        r
    };
    let subtree_hash = put_tree(&refs[..count])?;
    upsert_root_entry(
        dirname,
        subtree_hash,
        Kind::Tree,
        make_meta(MODE_DIR_DEFAULT),
    )
}

/// Split `"dev/ata0"` into `Some(("dev", "ata0"))`; a name with no `/`
/// (the common case — a plain top-level file) is `None`.
fn split_path(path: &str) -> Option<(&str, &str)> {
    let slash = path.find('/')?;
    Some((&path[..slash], &path[slash + 1..]))
}

// ── Current directory (one level deep, matching lingfs itself) ──────────

const CWD_LEN: usize = 32;
static mut CWD: [u8; CWD_LEN] = [0u8; CWD_LEN];
static mut CWD_ACTUAL_LEN: usize = 0; // 0 = root

/// The shell's current directory (`""` for root) — set by `cd`
/// (`ling_kernel_cd`), read for the prompt (`ling_kernel_cwd`).
pub fn cwd() -> &'static str {
    unsafe {
        let buf = &*&raw const CWD;
        let len = core::ptr::read(&raw const CWD_ACTUAL_LEN);
        core::str::from_utf8(&buf[..len]).unwrap_or("")
    }
}

/// Change directory. `""`/`"/"`/`".."` return to root; anything else must
/// name an existing one-level directory (a `Kind::Tree` root entry) —
/// lingfs is one level deep today, so there's nowhere deeper to `cd` into
/// yet. Returns `false` if `dir` doesn't exist or isn't a directory.
pub fn set_cwd(dir: &str) -> bool {
    let dir = dir.strip_suffix('/').unwrap_or(dir);
    if dir.is_empty() || dir == "." || dir == ".." {
        unsafe { CWD_ACTUAL_LEN = 0 };
        return true;
    }
    match lookup_dir(dir) {
        Ok(Some(_)) => {
            unsafe {
                let n = dir.len().min(CWD_LEN);
                let buf: &mut [u8; CWD_LEN] = &mut *&raw mut CWD;
                buf[..n].copy_from_slice(&dir.as_bytes()[..n]);
                CWD_ACTUAL_LEN = n;
            }
            true
        },
        _ => false,
    }
}

const RESOLVE_BUF: usize = 96;

fn copy_into<'a>(s: &str, buf: &'a mut [u8; RESOLVE_BUF]) -> &'a str {
    let n = s.len().min(buf.len());
    buf[..n].copy_from_slice(&s.as_bytes()[..n]);
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

/// Resolve a shell-facing name against the current directory before
/// looking it up: a leading `/` is always root-relative regardless of
/// `cwd` (how internal bookkeeping like `/hostname` stays reachable no
/// matter where the user has `cd`'d to); a name already containing a
/// later `/` (`dev/ata0`) is an explicit one-level path, left alone; a
/// bare name with no `/` at all resolves relative to `cwd` if one is set,
/// exactly like a real shell.
fn resolve<'a>(name: &str, buf: &'a mut [u8; RESOLVE_BUF]) -> &'a str {
    if let Some(rest) = name.strip_prefix('/') {
        return copy_into(rest, buf);
    }
    let dir = cwd();
    if name.contains('/') || dir.is_empty() {
        return copy_into(name, buf);
    }
    let mut n = dir.len().min(buf.len());
    buf[..n].copy_from_slice(&dir.as_bytes()[..n]);
    if n < buf.len() {
        buf[n] = b'/';
        n += 1;
    }
    let rn = name.len().min(buf.len() - n);
    buf[n..n + rn].copy_from_slice(&name.as_bytes()[..rn]);
    n += rn;
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

/// Look up a `Kind::Tree` entry named `dirname` in the root and return its
/// entries. `Ok(None)` if the root has no such tree entry (or `dirname`
/// names a plain file, not a directory).
fn lookup_dir(dirname: &str) -> Result<Option<(TreeEntries, usize)>, ()> {
    let current_root = root();
    if current_root == ZERO_HASH {
        return Ok(None);
    }
    let mut root_entries: TreeEntries = [EMPTY_TREE_ENTRY; MAX_TREE_ENTRIES];
    let Some(root_count) = get_tree(&current_root, &mut root_entries)? else { return Ok(None) };
    for i in 0..root_count {
        if name_eq(&root_entries[i].0, dirname) && root_entries[i].2 == Kind::Tree {
            let mut sub: TreeEntries = [EMPTY_TREE_ENTRY; MAX_TREE_ENTRIES];
            let Some(n) = get_tree(&root_entries[i].1, &mut sub)? else { return Ok(None) };
            return Ok(Some((sub, n)));
        }
    }
    Ok(None)
}

fn str_to_name_buf(name: &str) -> [u8; TREE_ENTRY_NAME_LEN] {
    let mut buf = [0u8; TREE_ENTRY_NAME_LEN];
    let n = name.len().min(TREE_ENTRY_NAME_LEN - 1);
    buf[..n].copy_from_slice(&name.as_bytes()[..n]);
    buf
}

/// Read a file's content into `out`, returning its length. `name` is
/// resolved against `cwd` first (see `resolve`) — a plain top-level name
/// (`"hostname"`), one level into a synthetic directory (`"dev/ata0"`), or
/// root-explicit (`"/hostname"`, bypassing `cwd`). `None` if no such
/// file/entry exists.
pub fn read_file(name: &str, out: &mut [u8; BLOCK_SIZE]) -> Result<Option<usize>, ()> {
    let mut rbuf = [0u8; RESOLVE_BUF];
    let name = resolve(name, &mut rbuf);
    if let Some((dirname, fname)) = split_path(name) {
        let Some((entries, count)) = lookup_dir(dirname)? else { return Ok(None) };
        for i in 0..count {
            if name_eq(&entries[i].0, fname) {
                return get_object(&entries[i].1, out);
            }
        }
        return Ok(None);
    }

    let current_root = root();
    if current_root == ZERO_HASH {
        return Ok(None);
    }
    let mut entries: TreeEntries = [EMPTY_TREE_ENTRY; MAX_TREE_ENTRIES];
    let Some(count) = get_tree(&current_root, &mut entries)? else { return Ok(None) };
    for i in 0..count {
        if name_eq(&entries[i].0, name) {
            return get_object(&entries[i].1, out);
        }
    }
    Ok(None)
}

/// Print `n` as decimal ASCII (no leading zeros; "0" for zero) — the one
/// bit of number-formatting `ls -l` needs that a `no_std`/no-`alloc` kernel
/// doesn't get for free.
fn print_decimal(n: u32) {
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

/// `-rwxr-xr-x`-style permission string (`d`/`-` first for directory vs.
/// file), Arch/`ls -l`-flavored. Stored and displayed only — see this
/// module's metadata doc comment for why it isn't enforced yet.
fn print_mode(mode: u16, is_dir: bool) {
    crate::console_write(if is_dir { b"d" } else { b"-" });
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 0b111;
        crate::console_write(if bits & 0b100 != 0 { b"r" } else { b"-" });
        crate::console_write(if bits & 0b010 != 0 { b"w" } else { b"-" });
        crate::console_write(if bits & 0b001 != 0 { b"x" } else { b"-" });
    }
}

fn trim_owner(buf: &[u8; OWNER_LEN]) -> &[u8] {
    let len = buf.iter().position(|&b| b == 0).unwrap_or(OWNER_LEN);
    &buf[..len]
}

/// Byte length of the object `hash` refers to (a blob's real content
/// length, or a tree's raw packed-entry length) — 0 if it's somehow not in
/// the index. Used only for `ls -l`'s size column.
fn object_len(hash: &Hash) -> u16 {
    match find(hash) {
        Some(slot) => unsafe { VOLUME.index[slot].len },
        None => 0,
    }
}

fn print_ls_entry(entry: &(([u8; TREE_ENTRY_NAME_LEN], usize), Hash, Kind, Meta), long: bool) {
    let ((name_buf, name_len), hash, kind, meta) = entry;
    let is_dir = *kind == Kind::Tree;
    if long {
        print_mode(meta.mode, is_dir);
        crate::console_write(b" ");
        crate::console_write(trim_owner(&meta.owner));
        crate::console_write(b":");
        crate::console_write(trim_owner(&meta.group));
        crate::console_write(b" ");
        print_decimal(object_len(hash) as u32);
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

fn is_hidden(name_buf: &[u8; TREE_ENTRY_NAME_LEN], name_len: usize) -> bool {
    name_len > 0 && name_buf[0] == b'.'
}

/// Print a directory's entries to the console — backs the shell's `ls`
/// (`path == ""` for the root, which now includes the synthetic `dev`/
/// `bin` directories `seed_defaults` maintains) and `ls <dirname>` for one
/// level into one of those. `show_all` includes dot-prefixed entries
/// (hidden by default, like real `ls`, though nothing creates one yet —
/// forward-compatible for e.g. a future `.lingrc`); `long` is `ls -l`'s
/// `-rwxr-xr-x owner:group size #modified_seq name` format (see this
/// module's metadata doc comment for what `modified_seq` is and isn't).
/// Directory entries get a trailing `/`, same visual cue `ls` gives on a
/// real Unix system. Prints "(empty)" for a truly empty/uncommitted root
/// or a directory name that doesn't exist, so that's visibly distinct from
/// a read error.
pub fn print_ls(path: &str, show_all: bool, long: bool) {
    let mut rbuf = [0u8; RESOLVE_BUF];
    let effective: &str = if path.is_empty() {
        cwd()
    } else {
        resolve(path, &mut rbuf)
    };
    let (entries, count) = if !effective.is_empty() {
        match lookup_dir(effective) {
            Ok(Some((entries, count))) => (entries, count),
            _ => {
                crate::console_write(b"(empty)\n");
                return;
            },
        }
    } else {
        let current_root = root();
        if current_root == ZERO_HASH {
            crate::console_write(b"(empty)\n");
            return;
        }
        let mut entries: TreeEntries = [EMPTY_TREE_ENTRY; MAX_TREE_ENTRIES];
        match get_tree(&current_root, &mut entries) {
            Ok(Some(count)) => (entries, count),
            _ => {
                crate::console_write(b"(empty)\n");
                return;
            },
        }
    };

    let mut shown = 0usize;
    for i in 0..count {
        let ((name_buf, name_len), _, _, _) = &entries[i];
        if !show_all && is_hidden(name_buf, *name_len) {
            continue;
        }
        print_ls_entry(&entries[i], long);
        shown += 1;
    }
    if shown == 0 {
        crate::console_write(b"(empty)\n");
    }
}

/// Built-in shell command names — kept here (not derived from anything
/// live) so `bin/` reflects what the shell actually accepts; update this
/// list alongside the `if cmd == "..."` chain in each kernel target's
/// `main.ling`.
const BUILTIN_COMMANDS: [&str; 9] = [
    "help", "clear", "ls", "cat", "write", "hostname", "about", "selftest", "theme",
];

/// (Re)seed the synthetic `dev/` and `bin/` directories from what this boot
/// actually detected — called once after every successful `mount()`, not
/// just on first format, so `dev/` reflects the disk controller found this
/// time even if it differs from a previous boot (e.g. AHCI on real hardware
/// vs. ATA under plain QEMU `if=ide`). Content is a one-line human-readable
/// description, not machine-parsed by anything yet.
pub fn seed_defaults() -> Result<(), ()> {
    let driver = crate::fs::blockdev::active_driver_name();
    let desc: &[u8] = if driver == "ahci0" {
        b"AHCI (SATA) block device, detected via PCI enumeration\n"
    } else {
        b"ATA PIO block device (legacy IDE fallback)\n"
    };
    write_dir("dev", &[(driver, desc)])?;

    let mut bin_entries: [(&str, &[u8]); BUILTIN_COMMANDS.len()] =
        [("", b"" as &[u8]); BUILTIN_COMMANDS.len()];
    for (i, name) in BUILTIN_COMMANDS.iter().enumerate() {
        bin_entries[i] = (name, b"built-in shell command\n");
    }
    write_dir("bin", &bin_entries)?;
    Ok(())
}

// ── Root pointer + commit log (git-like history) ─────────────────────────

/// The current root tree's hash (all-zero if nothing has been committed
/// yet).
pub fn root() -> Hash {
    unsafe {
        if VOLUME.commit_count == 0 {
            ZERO_HASH
        } else {
            VOLUME.commits[(VOLUME.commit_count - 1) as usize].root
        }
    }
}

/// Append a new root to the commit log (parented on the previous root) —
/// this is `git commit`: the old root is still there, still valid, still
/// reachable by walking the log backwards; nothing was overwritten.
pub fn commit(new_root: Hash) -> Result<(), ()> {
    unsafe {
        if VOLUME.commit_count as usize >= MAX_COMMITS {
            return Err(());
        }
        let parent = if VOLUME.commit_count == 0 {
            u32::MAX
        } else {
            VOLUME.commit_count - 1
        };
        VOLUME.commits[VOLUME.commit_count as usize] = CommitEntry { root: new_root, parent };
        VOLUME.commit_count += 1;
    }
    save_commits()?;
    save_superblock()?;
    Ok(())
}

/// Number of commits in the log so far.
pub fn commit_count() -> u32 {
    unsafe { VOLUME.commit_count }
}

/// The root hash at a given commit-log index (for walking history).
pub fn commit_root_at(index: u32) -> Option<Hash> {
    if index >= unsafe { VOLUME.commit_count } {
        return None;
    }
    Some(unsafe { VOLUME.commits[index as usize].root })
}

/// End-to-end smoke test against the real ATA disk: mount (formatting if
/// blank), store a blob, read it back byte-for-byte, confirm writing the
/// same content again dedupes (object count doesn't grow), build a tree
/// referencing it, commit it as the root, and confirm the root survives a
/// second `get_tree`. Exercises every piece of this module through the
/// same calls a real caller would use — not just unit-testing the parts in
/// isolation.
pub fn self_test() -> bool {
    if mount().is_err() {
        return false;
    }

    let content = b"hello lingfs";
    let Ok(hash1) = put_object(content) else { return false };

    let mut out = [0u8; BLOCK_SIZE];
    let Ok(Some(len)) = get_object(&hash1, &mut out) else { return false };
    if &out[..len] != content {
        return false;
    }

    let count_before = unsafe { VOLUME.object_count };
    let Ok(hash2) = put_object(content) else { return false };
    if hash1 != hash2 || unsafe { VOLUME.object_count } != count_before {
        return false; // dedup should have kicked in — no new object written
    }

    let Ok(tree_hash) = put_tree(&[("hello.txt", hash1, Kind::Blob, make_meta(MODE_FILE_DEFAULT))])
    else {
        return false;
    };
    if commit(tree_hash).is_err() {
        return false;
    }
    if root() != tree_hash {
        return false;
    }

    let mut entries: TreeEntries = [EMPTY_TREE_ENTRY; MAX_TREE_ENTRIES];
    let Ok(Some(n)) = get_tree(&tree_hash, &mut entries) else { return false };
    if n != 1 {
        return false;
    }
    let ((name, name_len), entry_hash, kind, _) = entries[0];
    &name[..name_len] == b"hello.txt" && entry_hash == hash1 && kind == Kind::Blob
}
