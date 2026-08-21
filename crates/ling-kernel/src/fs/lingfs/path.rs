//! Path parsing, current working directory, and directory traversal.

use crate::fs::lingfs::commit;
use crate::fs::lingfs::objects::ZERO_HASH;
use crate::fs::lingfs::tree::{
    get_tree, name_eq, Kind, TreeEntries, EMPTY_TREE_ENTRY, MAX_TREE_ENTRIES,
};

pub const CWD_LEN: usize = 32;
static mut CWD: [u8; CWD_LEN] = [0u8; CWD_LEN];
static mut CWD_ACTUAL_LEN: usize = 0;

pub const RESOLVE_BUF: usize = 96;

pub fn cwd() -> &'static str {
    unsafe {
        let buf = &*&raw const CWD;
        let len = core::ptr::read(&raw const CWD_ACTUAL_LEN);
        core::str::from_utf8(&buf[..len]).unwrap_or("")
    }
}

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

pub fn split_path(path: &str) -> Option<(&str, &str)> {
    let slash = path.find('/')?;
    Some((&path[..slash], &path[slash + 1..]))
}

fn copy_into<'a>(s: &str, buf: &'a mut [u8; RESOLVE_BUF]) -> &'a str {
    let n = s.len().min(buf.len());
    buf[..n].copy_from_slice(&s.as_bytes()[..n]);
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

pub fn resolve<'a>(name: &str, buf: &'a mut [u8; RESOLVE_BUF]) -> &'a str {
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

pub fn lookup_dir(dirname: &str) -> Result<Option<(TreeEntries, usize)>, ()> {
    let current_root = commit::root();
    if current_root == ZERO_HASH {
        return Ok(None);
    }
    let mut root_entries: TreeEntries = [EMPTY_TREE_ENTRY; MAX_TREE_ENTRIES];
    let Some(root_count) = get_tree(&current_root, &mut root_entries)? else {
        return Ok(None);
    };

    // Support recursive path resolution if dirname contains slashes (e.g. "a/b")
    let mut curr_tree = root_entries;
    let mut curr_count = root_count;
    let mut remaining = dirname;

    while !remaining.is_empty() {
        let (segment, next) = match remaining.find('/') {
            Some(i) => (&remaining[..i], &remaining[i + 1..]),
            None => (remaining, ""),
        };

        if segment.is_empty() {
            remaining = next;
            continue;
        }

        let mut found = false;
        for i in 0..curr_count {
            if name_eq(&curr_tree[i].0, segment) && curr_tree[i].2 == Kind::Tree {
                let mut sub: TreeEntries = [EMPTY_TREE_ENTRY; MAX_TREE_ENTRIES];
                let Some(n) = get_tree(&curr_tree[i].1, &mut sub)? else {
                    return Ok(None);
                };
                curr_tree = sub;
                curr_count = n;
                found = true;
                break;
            }
        }
        if !found {
            return Ok(None);
        }
        remaining = next;
    }

    Ok(Some((curr_tree, curr_count)))
}
