//! lingfs: a git-style, content-addressed object store as LingOS's root filesystem.

pub mod commit;
pub mod objects;
pub mod path;
pub mod tree;

pub use commit::{
    commit, commit_count, commit_root_at, load_commits, load_superblock, next_seq, root,
    save_commits, save_superblock, CommitEntry, COMMITS, MAGIC,
};
pub use objects::{
    find, get_object, load_index, object_len, put_object, save_dirty_index, save_index, Hash,
    IndexEntry, BLOCK_SIZE, DATA_REGION_START, EMPTY_ENTRY, INDEX_BLOCKS,
    INDEX_ENTRY_BYTES, LINGFS_BASE_LBA, MAX_COMMITS, MAX_OBJECTS, OBJECTS,
    SECTORS_PER_BLOCK, ZERO_HASH,
};
pub use path::{cwd, lookup_dir, resolve, set_cwd, split_path, CWD_LEN, RESOLVE_BUF};
pub use tree::{
    current_user, get_tree, make_meta, name_eq, print_ls_entry, print_mode, put_tree,
    set_current_user, str_to_name_buf, Kind, Meta, TreeEntries, EMPTY_META, EMPTY_TREE_ENTRY,
    MAX_TREE_ENTRIES, MODE_DIR_DEFAULT, MODE_FILE_DEFAULT, OWNER_LEN, TREE_ENTRY_BYTES,
    TREE_ENTRY_NAME_LEN,
};

pub fn format() -> Result<(), ()> {
    unsafe {
        objects::OBJECTS.object_count = 0;
        objects::OBJECTS.next_free_block = 0;
        objects::OBJECTS.index = [EMPTY_ENTRY; MAX_OBJECTS];
        objects::OBJECTS.hash_slots = [0; MAX_OBJECTS];
        objects::OBJECTS.dirty_blocks = [false; INDEX_BLOCKS];

        commit::COMMITS.commit_count = 0;
        commit::COMMITS.next_seq = 0;
        commit::COMMITS.commits = [commit::EMPTY_COMMIT; MAX_COMMITS];
        commit::COMMITS.mounted = true;
    }
    save_superblock()?;
    save_index()?;
    save_commits()?;
    Ok(())
}

pub fn mount() -> Result<(), ()> {
    match load_superblock() {
        Ok((object_count, next_free_block, commit_count, next_seq)) => {
            unsafe {
                objects::OBJECTS.object_count = object_count;
                objects::OBJECTS.next_free_block = next_free_block;
                commit::COMMITS.commit_count = commit_count;
                commit::COMMITS.next_seq = next_seq;
            }
            load_index()?;
            load_commits()?;
            unsafe { commit::COMMITS.mounted = true };
            Ok(())
        },
        Err(()) => format(),
    }
}

pub fn upsert_root_entry(name: &str, hash: Hash, kind: Kind, default_meta: Meta) -> Result<(), ()> {
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
        entries[count] = ((str_to_name_buf(name), name.len()), hash, kind, default_meta);
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
    let new_root = put_tree(&refs[..count])?;
    commit(new_root)
}

pub fn write_file(name: &str, content: &[u8]) -> Result<(), ()> {
    let mut rbuf = [0u8; RESOLVE_BUF];
    let name = resolve(name, &mut rbuf);
    if let Some((dirname, fname)) = split_path(name) {
        return write_in_dir(dirname, fname, content);
    }
    let blob_hash = put_object(content)?;
    upsert_root_entry(name, blob_hash, Kind::Blob, make_meta(MODE_FILE_DEFAULT, next_seq()))
}

// -- Multi-block ("big") files -----------------------------------------------
// The single-block cap (4KiB) made wallpapers, real WAVs, and any package
// past toy size impossible. Big files are content-addressed like
// everything else: the data is chunked into ordinary blob objects and a
// MANIFEST blob lists their hashes -- "LBIG" (leaf: total_len u32 + up to
// 127 hashes = ~508KiB) with one level of indirection, "LBG2" (its
// entries are hashes of LBIG leaves), lifting the cap to ~64MiB. Root
// directory entries stay Kind::Blob; readers distinguish by the magic.
// The one disclosed collision hazard: a small file legitimately BEGINNING
// with the magic bytes would misread through `read_file_all` -- so
// `write_file_any` wraps such content in a manifest even when it fits a
// block, making the magic unambiguous on disk.

const BIG_MAGIC: &[u8; 4] = b"LBIG";
const BIG2_MAGIC: &[u8; 4] = b"LBG2";
const BIG_HEADER: usize = 8; // magic + total_len u32
const HASHES_PER_MANIFEST: usize = (BLOCK_SIZE - BIG_HEADER) / 32; // 127

fn build_leaf_manifest(chunk: &[u8]) -> Result<Hash, ()> {
    // `chunk` here is a run of data up to 127 blocks; store its blocks and
    // one LBIG manifest describing them.
    let mut manifest = [0u8; BLOCK_SIZE];
    manifest[..4].copy_from_slice(BIG_MAGIC);
    manifest[4..8].copy_from_slice(&(chunk.len() as u32).to_le_bytes());
    let mut n = 0usize;
    let mut off = 0usize;
    while off < chunk.len() {
        let take = (chunk.len() - off).min(BLOCK_SIZE);
        let h = put_object(&chunk[off..off + take])?;
        manifest[BIG_HEADER + n * 32..BIG_HEADER + (n + 1) * 32].copy_from_slice(&h);
        n += 1;
        off += take;
    }
    put_object(&manifest[..BIG_HEADER + n * 32])
}

/// Store `content` as an object and return the hash of its root -- a plain
/// blob for small content, or an LBIG/LBG2 manifest chain for anything
/// over one block (up to ~64MiB). Shared by the root and dir writers so a
/// big file works the same wherever it lives. `read_file_all` transparently
/// follows the manifest when it sees the magic.
fn store_content_hash(content: &[u8]) -> Result<Hash, ()> {
    let needs_manifest = content.len() > BLOCK_SIZE
        || content.get(..4) == Some(&BIG_MAGIC[..])
        || content.get(..4) == Some(&BIG2_MAGIC[..]);
    if !needs_manifest {
        return put_object(content);
    }
    let leaf_span = HASHES_PER_MANIFEST * BLOCK_SIZE;
    if content.len() <= leaf_span {
        build_leaf_manifest(content)
    } else {
        if content.len() > leaf_span * HASHES_PER_MANIFEST {
            return Err(()); // ~64MiB two-level cap, stated not hidden
        }
        let mut m2 = [0u8; BLOCK_SIZE];
        m2[..4].copy_from_slice(BIG2_MAGIC);
        m2[4..8].copy_from_slice(&(content.len() as u32).to_le_bytes());
        let mut n = 0usize;
        let mut off = 0usize;
        while off < content.len() {
            let take = (content.len() - off).min(leaf_span);
            let h = build_leaf_manifest(&content[off..off + take])?;
            m2[BIG_HEADER + n * 32..BIG_HEADER + (n + 1) * 32].copy_from_slice(&h);
            n += 1;
            off += take;
        }
        put_object(&m2[..BIG_HEADER + n * 32])
    }
}

/// Write a root file of any size (up to ~64MiB): small content is a plain
/// blob, larger content is chunked behind manifests. For a one-level
/// dir/file path use `write_in_dir_any`.
pub fn write_file_any(name: &str, content: &[u8]) -> Result<(), ()> {
    let mut rbuf = [0u8; RESOLVE_BUF];
    let name = resolve(name, &mut rbuf);
    if let Some((dirname, fname)) = split_path(name) {
        return write_in_dir_any(dirname, fname, content);
    }
    let root_hash = store_content_hash(content)?;
    upsert_root_entry(name, root_hash, Kind::Blob, make_meta(MODE_FILE_DEFAULT, next_seq()))
}

/// Write a file of any size into a one-level directory (`dir/file`). The
/// dir entry points at the content's manifest (or plain blob), so
/// `read_file_all("dir/file")` transparently reassembles it. One level
/// only, matching lingfs's directory model.
pub fn write_in_dir_any(dirname: &str, fname: &str, content: &[u8]) -> Result<(), ()> {
    let mut entries: TreeEntries = [EMPTY_TREE_ENTRY; MAX_TREE_ENTRIES];
    let mut count = match lookup_dir(dirname)? {
        Some((existing, n)) => {
            entries = existing;
            n
        },
        None => 0,
    };
    let hash = store_content_hash(content)?;
    // Replace an existing same-named entry, else append.
    let mut slot = None;
    for i in 0..count {
        if name_eq(&entries[i].0, fname) {
            slot = Some(i);
            break;
        }
    }
    let i = match slot {
        Some(i) => i,
        None => {
            if count >= MAX_TREE_ENTRIES {
                return Err(());
            }
            count += 1;
            count - 1
        },
    };
    entries[i].0 = (str_to_name_buf(fname), fname.len());
    entries[i].1 = hash;
    entries[i].2 = Kind::Blob;
    entries[i].3 = make_meta(MODE_FILE_DEFAULT, next_seq());
    let refs: [(&str, Hash, Kind, Meta); MAX_TREE_ENTRIES] = {
        let mut r = [("", ZERO_HASH, Kind::Blob, EMPTY_META); MAX_TREE_ENTRIES];
        for j in 0..count {
            let (buf, len) = &entries[j].0;
            r[j] = (
                core::str::from_utf8(&buf[..*len]).unwrap_or(""),
                entries[j].1,
                entries[j].2,
                entries[j].3,
            );
        }
        r
    };
    let subtree = put_tree(&refs[..count])?;
    upsert_root_entry(dirname, subtree, Kind::Tree, make_meta(MODE_DIR_DEFAULT, next_seq()))
}

fn read_leaf_manifest(manifest: &[u8], mlen: usize, out: &mut [u8], got: &mut usize) -> Result<(), ()> {
    let total = u32::from_le_bytes([manifest[4], manifest[5], manifest[6], manifest[7]]) as usize;
    let count = (mlen - BIG_HEADER) / 32;
    let mut remaining = total;
    for i in 0..count {
        let mut h: Hash = ZERO_HASH;
        h.copy_from_slice(&manifest[BIG_HEADER + i * 32..BIG_HEADER + (i + 1) * 32]);
        let mut block = [0u8; BLOCK_SIZE];
        let Ok(Some(blen)) = get_object(&h, &mut block) else { return Err(()) };
        let want = remaining.min(blen);
        let take = want.min(out.len().saturating_sub(*got));
        out[*got..*got + take].copy_from_slice(&block[..take]);
        *got += take;
        remaining -= want;
        if take < want {
            return Ok(()); // caller's buffer full -- truncated read, reported via return length
        }
    }
    Ok(())
}

/// Read a file of any size into `out`, following manifests when present.
/// Returns the number of bytes written to `out` (truncated if `out` is
/// smaller than the file), `Ok(None)` if the file doesn't exist.
pub fn read_file_all(name: &str, out: &mut [u8]) -> Result<Option<usize>, ()> {
    let mut root = [0u8; BLOCK_SIZE];
    let Some(rlen) = read_file(name, &mut root)? else {
        return Ok(None);
    };
    if rlen >= BIG_HEADER && &root[..4] == BIG_MAGIC {
        let mut got = 0usize;
        read_leaf_manifest(&root, rlen, out, &mut got)?;
        return Ok(Some(got));
    }
    if rlen >= BIG_HEADER && &root[..4] == BIG2_MAGIC {
        let mut got = 0usize;
        let count = (rlen - BIG_HEADER) / 32;
        for i in 0..count {
            let mut h: Hash = ZERO_HASH;
            h.copy_from_slice(&root[BIG_HEADER + i * 32..BIG_HEADER + (i + 1) * 32]);
            let mut leaf = [0u8; BLOCK_SIZE];
            let Ok(Some(llen)) = get_object(&h, &mut leaf) else { return Err(()) };
            if llen < BIG_HEADER || &leaf[..4] != BIG_MAGIC {
                return Err(());
            }
            read_leaf_manifest(&leaf, llen, out, &mut got)?;
            if got >= out.len() {
                break;
            }
        }
        return Ok(Some(got));
    }
    let take = rlen.min(out.len());
    out[..take].copy_from_slice(&root[..take]);
    Ok(Some(take))
}

/// Logical size of a file (manifest-aware): what `read_file_all` would
/// return given a large-enough buffer.
pub fn file_size(name: &str) -> Option<usize> {
    let mut root = [0u8; BLOCK_SIZE];
    let Ok(Some(rlen)) = read_file(name, &mut root) else { return None };
    if rlen >= BIG_HEADER && (&root[..4] == BIG_MAGIC || &root[..4] == BIG2_MAGIC) {
        return Some(u32::from_le_bytes([root[4], root[5], root[6], root[7]]) as usize);
    }
    Some(rlen)
}

pub fn write_dir(dirname: &str, files: &[(&str, &[u8])]) -> Result<(), ()> {
    if files.len() > MAX_TREE_ENTRIES {
        return Err(());
    }
    let mut refs: [(&str, Hash, Kind, Meta); MAX_TREE_ENTRIES] =
        [("", ZERO_HASH, Kind::Blob, EMPTY_META); MAX_TREE_ENTRIES];
    for (i, (fname, content)) in files.iter().enumerate() {
        let blob_hash = put_object(content)?;
        refs[i] = (fname, blob_hash, Kind::Blob, make_meta(MODE_FILE_DEFAULT, next_seq()));
    }
    let subtree_hash = put_tree(&refs[..files.len()])?;
    upsert_root_entry(dirname, subtree_hash, Kind::Tree, make_meta(MODE_DIR_DEFAULT, next_seq()))
}

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
            make_meta(MODE_FILE_DEFAULT, next_seq()),
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
    upsert_root_entry(dirname, subtree_hash, Kind::Tree, make_meta(MODE_DIR_DEFAULT, next_seq()))
}

pub fn read_file(name: &str, out: &mut [u8; BLOCK_SIZE]) -> Result<Option<usize>, ()> {
    let mut rbuf = [0u8; RESOLVE_BUF];
    let name = resolve(name, &mut rbuf);
    if let Some((dirname, fname)) = split_path(name) {
        let Some((entries, count)) = lookup_dir(dirname)? else {
            return Ok(None);
        };
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
    let Some(count) = get_tree(&current_root, &mut entries)? else {
        return Ok(None);
    };
    for i in 0..count {
        if name_eq(&entries[i].0, name) {
            return get_object(&entries[i].1, out);
        }
    }
    Ok(None)
}

fn is_hidden(name_buf: &[u8; TREE_ENTRY_NAME_LEN], name_len: usize) -> bool {
    name_len > 0 && name_buf[0] == b'.'
}

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

/// Structured directory listing for graphical callers (the WM's file
/// manager): the same walk as `print_ls`, but handing back one entry per
/// call instead of printing -- index-driven so a caller with no allocator
/// and no closures (the WM's fixed-size row loop) can enumerate. Hidden
/// (dot-prefixed) entries are skipped, matching `ls` without `-a`.
/// Returns `(name_len_copied_into_name_out, is_dir)`, or `None` past the
/// end / on any filesystem error (a graphical list has no error channel
/// better than "shows empty" -- the shell's `ls` remains the diagnosing
/// tool).
pub fn list_entry(path: &str, index: usize, name_out: &mut [u8]) -> Option<(usize, bool)> {
    let mut rbuf = [0u8; RESOLVE_BUF];
    let effective: &str = if path.is_empty() { "" } else { resolve(path, &mut rbuf) };
    let (entries, count) = if !effective.is_empty() {
        match lookup_dir(effective) {
            Ok(Some((entries, count))) => (entries, count),
            _ => return None,
        }
    } else {
        let current_root = root();
        if current_root == ZERO_HASH {
            return None;
        }
        let mut entries: TreeEntries = [EMPTY_TREE_ENTRY; MAX_TREE_ENTRIES];
        match get_tree(&current_root, &mut entries) {
            Ok(Some(count)) => (entries, count),
            _ => return None,
        }
    };

    let mut visible = 0usize;
    for i in 0..count {
        let ((name_buf, name_len), _, kind, _) = &entries[i];
        if is_hidden(name_buf, *name_len) {
            continue;
        }
        if visible == index {
            let n = (*name_len).min(name_out.len());
            name_out[..n].copy_from_slice(&name_buf[..n]);
            return Some((n, matches!(kind, Kind::Tree)));
        }
        visible += 1;
    }
    None
}

const BUILTIN_COMMANDS: [&str; 9] =
    ["help", "clear", "ls", "cat", "write", "hostname", "about", "selftest", "theme"];

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

    let count_before = unsafe { objects::OBJECTS.object_count };
    let Ok(hash2) = put_object(content) else { return false };
    if hash1 != hash2 || unsafe { objects::OBJECTS.object_count } != count_before {
        return false;
    }

    let Ok(tree_hash) = put_tree(&[("hello.txt", hash1, Kind::Blob, make_meta(MODE_FILE_DEFAULT, next_seq()))])
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
