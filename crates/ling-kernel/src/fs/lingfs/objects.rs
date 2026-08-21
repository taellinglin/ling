//! Object store and O(1) hash index table.

use crate::fs::blockdev as ata;
use crate::fs::blockdev::SECTOR_SIZE;

pub const BLOCK_SIZE: usize = 4096;
pub const SECTORS_PER_BLOCK: usize = BLOCK_SIZE / SECTOR_SIZE;
pub const MAX_OBJECTS: usize = 4096;

pub type Hash = [u8; 32];
pub const ZERO_HASH: Hash = [0u8; 32];

pub const INDEX_ENTRY_BYTES: usize = 32 + 4 + 2 + 1; // 39 bytes
pub const INDEX_BLOCKS: usize = (MAX_OBJECTS * INDEX_ENTRY_BYTES).div_ceil(BLOCK_SIZE);
pub const COMMIT_ENTRY_BYTES: usize = 32 + 4;
pub const MAX_COMMITS: usize = 1024;
pub const COMMIT_BLOCKS: usize = (MAX_COMMITS * COMMIT_ENTRY_BYTES).div_ceil(BLOCK_SIZE);
pub const DATA_REGION_START: u32 = 1 + INDEX_BLOCKS as u32 + COMMIT_BLOCKS as u32;
pub const LINGFS_BASE_LBA: u32 = 8192;

#[derive(Copy, Clone)]
pub struct IndexEntry {
    pub hash: Hash,
    pub block: u32,
    pub len: u16,
    pub used: bool,
}

pub const EMPTY_ENTRY: IndexEntry = IndexEntry {
    hash: ZERO_HASH,
    block: 0,
    len: 0,
    used: false,
};

pub struct ObjectTable {
    pub object_count: u32,
    pub next_free_block: u32,
    pub index: [IndexEntry; MAX_OBJECTS],
    // Open-addressed hash lookup table: maps BLAKE3 prefix to index slot
    // 0 = empty, (slot + 1) = slot index
    pub hash_slots: [u16; MAX_OBJECTS],
    pub dirty_blocks: [bool; INDEX_BLOCKS],
}

pub static mut OBJECTS: ObjectTable = ObjectTable {
    object_count: 0,
    next_free_block: 0,
    index: [EMPTY_ENTRY; MAX_OBJECTS],
    hash_slots: [0; MAX_OBJECTS],
    dirty_blocks: [false; INDEX_BLOCKS],
};

pub fn block_to_lba(block: u32) -> u32 {
    LINGFS_BASE_LBA + (DATA_REGION_START + block) * SECTORS_PER_BLOCK as u32
}

pub fn block_index_lba(b: usize) -> u32 {
    LINGFS_BASE_LBA + (1 + b as u32) * SECTORS_PER_BLOCK as u32
}

pub fn read_block(block_lba_base: u32, out: &mut [u8; BLOCK_SIZE]) -> Result<(), ()> {
    for i in 0..SECTORS_PER_BLOCK {
        let mut sector = [0u8; SECTOR_SIZE];
        ata::read_sector(block_lba_base + i as u32, &mut sector)?;
        out[i * SECTOR_SIZE..(i + 1) * SECTOR_SIZE].copy_from_slice(&sector);
    }
    Ok(())
}

pub fn write_block(block_lba_base: u32, data: &[u8; BLOCK_SIZE]) -> Result<(), ()> {
    for i in 0..SECTORS_PER_BLOCK {
        let mut sector = [0u8; SECTOR_SIZE];
        sector.copy_from_slice(&data[i * SECTOR_SIZE..(i + 1) * SECTOR_SIZE]);
        ata::write_sector(block_lba_base + i as u32, &sector)?;
    }
    Ok(())
}

fn hash_to_slot(hash: &Hash) -> usize {
    let prefix = u64::from_le_bytes(hash[0..8].try_into().unwrap());
    (prefix as usize) % MAX_OBJECTS
}

pub fn rebuild_hash_index() {
    unsafe {
        OBJECTS.hash_slots = [0; MAX_OBJECTS];
        for slot in 0..MAX_OBJECTS {
            if OBJECTS.index[slot].used {
                let start = hash_to_slot(&OBJECTS.index[slot].hash);
                let mut pos = start;
                for _ in 0..MAX_OBJECTS {
                    if OBJECTS.hash_slots[pos] == 0 {
                        OBJECTS.hash_slots[pos] = (slot + 1) as u16;
                        break;
                    }
                    pos = (pos + 1) % MAX_OBJECTS;
                }
            }
        }
    }
}

pub fn find(hash: &Hash) -> Option<usize> {
    unsafe {
        let start = hash_to_slot(hash);
        let mut pos = start;
        for _ in 0..MAX_OBJECTS {
            let entry_num = OBJECTS.hash_slots[pos];
            if entry_num == 0 {
                return None;
            }
            let slot = (entry_num - 1) as usize;
            if OBJECTS.index[slot].used && &OBJECTS.index[slot].hash == hash {
                return Some(slot);
            }
            pos = (pos + 1) % MAX_OBJECTS;
        }
        None
    }
}

pub fn load_index() -> Result<(), ()> {
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
            unsafe {
                OBJECTS.index[idx] = IndexEntry { hash, block, len, used };
                OBJECTS.dirty_blocks[b] = false;
            }
        }
    }
    rebuild_hash_index();
    Ok(())
}

pub fn save_index() -> Result<(), ()> {
    for b in 0..INDEX_BLOCKS {
        save_index_block(b)?;
    }
    Ok(())
}

pub fn save_dirty_index() -> Result<(), ()> {
    for b in 0..INDEX_BLOCKS {
        if unsafe { OBJECTS.dirty_blocks[b] } {
            save_index_block(b)?;
            unsafe { OBJECTS.dirty_blocks[b] = false };
        }
    }
    Ok(())
}

fn save_index_block(b: usize) -> Result<(), ()> {
    let mut buf = [0u8; BLOCK_SIZE];
    for slot in 0..(BLOCK_SIZE / INDEX_ENTRY_BYTES) {
        let idx = b * (BLOCK_SIZE / INDEX_ENTRY_BYTES) + slot;
        if idx >= MAX_OBJECTS {
            break;
        }
        let e = unsafe { OBJECTS.index[idx] };
        let off = slot * INDEX_ENTRY_BYTES;
        buf[off..off + 32].copy_from_slice(&e.hash);
        buf[off + 32..off + 36].copy_from_slice(&e.block.to_le_bytes());
        buf[off + 36..off + 38].copy_from_slice(&e.len.to_le_bytes());
        buf[off + 38] = e.used as u8;
    }
    write_block(block_index_lba(b), &buf)
}

pub fn put_object(data: &[u8]) -> Result<Hash, ()> {
    if data.len() > BLOCK_SIZE {
        return Err(());
    }
    let hash = crate::hash::blake3(data);
    if find(&hash).is_some() {
        return Ok(hash);
    }
    unsafe {
        if OBJECTS.object_count as usize >= MAX_OBJECTS {
            return Err(());
        }
        let block = OBJECTS.next_free_block;
        let mut buf = [0u8; BLOCK_SIZE];
        buf[..data.len()].copy_from_slice(data);
        write_block(block_to_lba(block), &buf)?;

        let slot = OBJECTS.object_count as usize;
        OBJECTS.index[slot] = IndexEntry {
            hash,
            block,
            len: data.len() as u16,
            used: true,
        };
        OBJECTS.object_count += 1;
        OBJECTS.next_free_block += 1;

        let b = slot / (BLOCK_SIZE / INDEX_ENTRY_BYTES);
        if b < INDEX_BLOCKS {
            OBJECTS.dirty_blocks[b] = true;
        }

        // Insert into open-addressed table
        let start = hash_to_slot(&hash);
        let mut pos = start;
        for _ in 0..MAX_OBJECTS {
            if OBJECTS.hash_slots[pos] == 0 {
                OBJECTS.hash_slots[pos] = (slot + 1) as u16;
                break;
            }
            pos = (pos + 1) % MAX_OBJECTS;
        }
    }
    save_dirty_index()?;
    Ok(hash)
}

pub fn get_object(hash: &Hash, out: &mut [u8; BLOCK_SIZE]) -> Result<Option<usize>, ()> {
    let Some(slot) = find(hash) else { return Ok(None) };
    let e = unsafe { OBJECTS.index[slot] };
    read_block(block_to_lba(e.block), out)?;
    Ok(Some(e.len as usize))
}

pub fn object_len(hash: &Hash) -> u16 {
    match find(hash) {
        Some(slot) => unsafe { OBJECTS.index[slot].len },
        None => 0,
    }
}
