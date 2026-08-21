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
