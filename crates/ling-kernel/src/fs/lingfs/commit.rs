//! Commit log, volume state, and superblock persistence.

use crate::fs::lingfs::objects::{
    self, read_block, write_block, Hash, BLOCK_SIZE, COMMIT_BLOCKS,
    COMMIT_ENTRY_BYTES, INDEX_BLOCKS, LINGFS_BASE_LBA, MAX_COMMITS, ZERO_HASH,
};

pub const MAGIC: u64 = 0x53_46_47_4E_4C_47_4E_4C; // "LNGLNGFS"

#[derive(Copy, Clone)]
pub struct CommitEntry {
    pub root: Hash,
    pub parent: u32,
}

pub const EMPTY_COMMIT: CommitEntry = CommitEntry {
    root: ZERO_HASH,
    parent: u32::MAX,
};

pub struct CommitState {
    pub mounted: bool,
    pub commit_count: u32,
    pub next_seq: u32,
    pub commits: [CommitEntry; MAX_COMMITS],
}

pub static mut COMMITS: CommitState = CommitState {
    mounted: false,
    commit_count: 0,
    next_seq: 0,
    commits: [EMPTY_COMMIT; MAX_COMMITS],
};

pub fn next_seq() -> u32 {
    unsafe {
        let seq = COMMITS.next_seq;
        COMMITS.next_seq += 1;
        seq
    }
}

pub fn commit_block_lba(b: usize) -> u32 {
    LINGFS_BASE_LBA + (1 + INDEX_BLOCKS as u32 + b as u32) * objects::SECTORS_PER_BLOCK as u32
}

pub fn load_superblock() -> Result<(u32, u32, u32, u32), ()> {
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

pub fn save_superblock() -> Result<(), ()> {
    unsafe {
        let mut block = [0u8; BLOCK_SIZE];
        block[0..8].copy_from_slice(&MAGIC.to_le_bytes());
        block[8..12].copy_from_slice(&objects::OBJECTS.object_count.to_le_bytes());
        block[12..16].copy_from_slice(&objects::OBJECTS.next_free_block.to_le_bytes());
        block[16..20].copy_from_slice(&COMMITS.commit_count.to_le_bytes());
        block[20..24].copy_from_slice(&COMMITS.next_seq.to_le_bytes());
        write_block(LINGFS_BASE_LBA, &block)
    }
}

pub fn load_commits() -> Result<(), ()> {
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
            unsafe { COMMITS.commits[idx] = CommitEntry { root, parent } };
        }
    }
    Ok(())
}

pub fn save_commits() -> Result<(), ()> {
    for b in 0..COMMIT_BLOCKS {
        let mut buf = [0u8; BLOCK_SIZE];
        for slot in 0..(BLOCK_SIZE / COMMIT_ENTRY_BYTES) {
            let idx = b * (BLOCK_SIZE / COMMIT_ENTRY_BYTES) + slot;
            if idx >= MAX_COMMITS {
                break;
            }
            let e = unsafe { COMMITS.commits[idx] };
            let off = slot * COMMIT_ENTRY_BYTES;
            buf[off..off + 32].copy_from_slice(&e.root);
            buf[off + 32..off + 36].copy_from_slice(&e.parent.to_le_bytes());
        }
        write_block(commit_block_lba(b), &buf)?;
    }
    Ok(())
}

pub fn root() -> Hash {
    unsafe {
        if COMMITS.commit_count == 0 {
            ZERO_HASH
        } else {
            COMMITS.commits[(COMMITS.commit_count - 1) as usize].root
        }
    }
}

pub fn commit(new_root: Hash) -> Result<(), ()> {
    unsafe {
        if COMMITS.commit_count as usize >= MAX_COMMITS {
            return Err(());
        }
        let parent = if COMMITS.commit_count == 0 {
            u32::MAX
        } else {
            COMMITS.commit_count - 1
        };
        COMMITS.commits[COMMITS.commit_count as usize] = CommitEntry {
            root: new_root,
            parent,
        };
        COMMITS.commit_count += 1;
    }
    save_commits()?;
    save_superblock()?;
    Ok(())
}

pub fn commit_count() -> u32 {
    unsafe { COMMITS.commit_count }
}

pub fn commit_root_at(index: u32) -> Option<Hash> {
    if index >= unsafe { COMMITS.commit_count } {
        return None;
    }
    Some(unsafe { COMMITS.commits[index as usize].root })
}
