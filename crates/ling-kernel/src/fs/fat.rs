//! FAT16 / FAT32 -- what USB flash drives and SD cards carry -- read and
//! write: directories with long names, file read / write / create, folders,
//! delete, rename. One [`Volume`] per mount, over any 512-byte-sector device
//! (`fs::media` supplies the device I/O; the volume adds its partition
//! offset). Synchronous and simple: every FAT change is written through to
//! every FAT copy at once, so another machine reads the stick correctly after
//! an unplug. FAT12 (tiny volumes) is not supported.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub const SECTOR: usize = 512;

/// Sector I/O of the device a volume lives on (absolute LBAs).
pub trait Disk {
    fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), ()>;
    fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), ()>;
}

const ATTR_LABEL: u8 = 0x08;
const ATTR_DIR: u8 = 0x10;
const ATTR_LFN: u8 = 0x0F;
const MAX_LFN_PARTS: usize = 20; // 20 * 13 = 260 chars

pub struct Volume {
    part_lba: u32,
    fat32: bool,
    spc: u32,
    reserved: u32,
    nfats: u32,
    fat_size: u32,
    root_cluster: u32,
    root_lba: u32,
    root_sectors: u32,
    data_start: u32,
    clusters: u32,
    pub label: String,
    hint: u32,
    cache_lba: u32,
    cache: [u8; SECTOR],
    cache_ok: bool,
}

/// A directory entry as a program sees it, plus where it lives on disk so it
/// can be updated or deleted.
#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u32,
    pub cluster: u32,
    short: [u8; 11],
    /// Absolute sector + byte offset of the 8.3 entry, and of its LFN entries.
    loc: (u32, usize),
    lfn: Vec<(u32, usize)>,
}

/// A directory: a cluster chain, or (FAT16) the fixed root region (0).
#[derive(Clone, Copy)]
struct Dir {
    cluster: u32,
}

fn le16(b: &[u8], o: usize) -> u32 {
    b[o] as u32 | ((b[o + 1] as u32) << 8)
}
fn le32(b: &[u8], o: usize) -> u32 {
    le16(b, o) | (le16(b, o + 2) << 16)
}
fn put16(b: &mut [u8], o: usize, v: u32) {
    b[o] = v as u8;
    b[o + 1] = (v >> 8) as u8;
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    put16(b, o, v & 0xFFFF);
    put16(b, o + 2, v >> 16);
}

fn split_path(path: &str) -> (&str, &str) {
    let p = path.trim_matches('/');
    match p.rfind('/') {
        Some(i) => (&p[..i], &p[i + 1..]),
        None => ("", p),
    }
}

/// The 8.3 checksum LFN entries carry.
fn checksum(short: &[u8; 11]) -> u8 {
    let mut s: u8 = 0;
    for &c in short {
        s = s.rotate_right(1).wrapping_add(c);
    }
    s
}

impl Volume {
    pub fn mount(d: &mut dyn Disk, part_lba: u32) -> Option<Volume> {
        let mut bs = [0u8; SECTOR];
        d.read(part_lba, &mut bs).ok()?;
        if bs[510] != 0x55 || bs[511] != 0xAA || le16(&bs, 11) != 512 {
            return None;
        }
        let spc = bs[13] as u32;
        if spc == 0 || spc & (spc - 1) != 0 {
            return None;
        }
        let reserved = le16(&bs, 14);
        let nfats = bs[16] as u32;
        let root_entries = le16(&bs, 17);
        let total = match le16(&bs, 19) {
            0 => le32(&bs, 32),
            t => t,
        };
        let fat16_size = le16(&bs, 22);
        let fat32_format = fat16_size == 0;
        let fat_size = if fat32_format { le32(&bs, 36) } else { fat16_size };
        if nfats == 0 || fat_size == 0 || total == 0 || reserved == 0 {
            return None;
        }
        let root_sectors = (root_entries * 32).div_ceil(512);
        let root_lba = reserved + nfats * fat_size;
        let data_start = root_lba + root_sectors;
        if data_start >= total {
            return None;
        }
        let clusters = (total - data_start) / spc;
        let fat32 = fat32_format || clusters >= 65525;
        if !fat32 && clusters < 4085 {
            return None; // FAT12
        }
        let root_cluster = if fat32 { le32(&bs, 44) } else { 0 };
        let lbl = if fat32 { &bs[71..82] } else { &bs[43..54] };
        let label = String::from(String::from_utf8_lossy(lbl).trim_matches(|c: char| c == ' ' || c == '\0'));
        let label = if label == "NO NAME" { String::new() } else { label };
        Some(Volume {
            part_lba,
            fat32,
            spc,
            reserved,
            nfats,
            fat_size,
            root_cluster,
            root_lba,
            root_sectors,
            data_start,
            clusters,
            label,
            hint: 2,
            cache_lba: 0,
            cache: [0; SECTOR],
            cache_ok: false,
        })
    }

    pub fn is_fat32(&self) -> bool {
        self.fat32
    }

    /// Total and free space in bytes (free: one pass over the FAT).
    pub fn space(&mut self, d: &mut dyn Disk) -> (u64, u64) {
        let mut free = 0u64;
        for c in 2..self.clusters + 2 {
            if self.fat_get(d, c) == Ok(0) {
                free += 1;
            }
        }
        let cb = self.cluster_bytes() as u64;
        (self.clusters as u64 * cb, free * cb)
    }

    fn cluster_bytes(&self) -> usize {
        self.spc as usize * SECTOR
    }

    fn cluster_lba(&self, c: u32) -> u32 {
        self.part_lba + self.data_start + (c - 2) * self.spc
    }

    fn eoc(&self, v: u32) -> bool {
        if self.fat32 {
            v >= 0x0FFF_FFF8
        } else {
            v >= 0xFFF8
        }
    }

    fn eoc_value(&self) -> u32 {
        if self.fat32 {
            0x0FFF_FFFF
        } else {
            0xFFFF
        }
    }

    fn valid_cluster(&self, c: u32) -> bool {
        c >= 2 && c < self.clusters + 2
    }

    /// Load the FAT sector holding cluster `c`'s entry; returns its byte offset.
    fn fat_load(&mut self, d: &mut dyn Disk, c: u32) -> Result<usize, ()> {
        if !self.valid_cluster(c) {
            return Err(());
        }
        let (bytes, per) = if self.fat32 { (4, 128) } else { (2, 256) };
        let lba = self.part_lba + self.reserved + c / per;
        if !self.cache_ok || self.cache_lba != lba {
            let mut buf = [0u8; SECTOR];
            d.read(lba, &mut buf)?;
            self.cache = buf;
            self.cache_lba = lba;
            self.cache_ok = true;
        }
        Ok((c % per) as usize * bytes)
    }

    fn fat_get(&mut self, d: &mut dyn Disk, c: u32) -> Result<u32, ()> {
        let off = self.fat_load(d, c)?;
        Ok(if self.fat32 { le32(&self.cache, off) & 0x0FFF_FFFF } else { le16(&self.cache, off) })
    }

    fn fat_set(&mut self, d: &mut dyn Disk, c: u32, v: u32) -> Result<(), ()> {
        let off = self.fat_load(d, c)?;
        if self.fat32 {
            let keep = le32(&self.cache, off) & 0xF000_0000;
            put32(&mut self.cache, off, keep | (v & 0x0FFF_FFFF));
        } else {
            put16(&mut self.cache, off, v & 0xFFFF);
        }
        let rel = self.cache_lba - self.part_lba - self.reserved;
        for k in 0..self.nfats {
            let cache = self.cache;
            d.write(self.part_lba + self.reserved + k * self.fat_size + rel, &cache)?;
        }
        Ok(())
    }

    fn alloc(&mut self, d: &mut dyn Disk) -> Option<u32> {
        let n = self.clusters;
        for i in 0..n {
            let c = 2 + (self.hint.saturating_sub(2) + i) % n;
            if self.fat_get(d, c) == Ok(0) {
                self.fat_set(d, c, self.eoc_value()).ok()?;
                self.hint = c + 1;
                return Some(c);
            }
        }
        None
    }

    fn free_chain(&mut self, d: &mut dyn Disk, mut c: u32) {
        let mut guard = 0;
        while self.valid_cluster(c) && !self.eoc(c) && guard < 4_000_000 {
            let Ok(next) = self.fat_get(d, c) else { return };
            let _ = self.fat_set(d, c, 0);
            if c < self.hint {
                self.hint = c;
            }
            c = next;
            guard += 1;
        }
    }

    fn zero_cluster(&self, d: &mut dyn Disk, c: u32) -> Result<(), ()> {
        let z = vec![0u8; self.cluster_bytes()];
        d.write(self.cluster_lba(c), &z)
    }

    fn root(&self) -> Dir {
        Dir { cluster: self.root_cluster }
    }

    fn dir_of(&self, e: &Entry) -> Dir {
        Dir { cluster: if e.cluster == 0 { self.root_cluster } else { e.cluster } }
    }

    /// A directory's sectors in order; `extend` appends a zeroed cluster first
    /// (not possible for the FAT16 root).
    fn dir_sectors(&mut self, d: &mut dyn Disk, dir: Dir, extend: bool) -> Result<Vec<u32>, ()> {
        if dir.cluster == 0 {
            if extend {
                return Err(());
            }
            return Ok((0..self.root_sectors).map(|i| self.part_lba + self.root_lba + i).collect());
        }
        let mut out = Vec::new();
        let mut c = dir.cluster;
        let mut last = c;
        let mut guard = 0;
        while self.valid_cluster(c) && !self.eoc(c) && guard < 1_000_000 {
            for s in 0..self.spc {
                out.push(self.cluster_lba(c) + s);
            }
            last = c;
            c = self.fat_get(d, c)?;
            guard += 1;
        }
        if extend {
            let n = self.alloc(d).ok_or(())?;
            self.zero_cluster(d, n)?;
            self.fat_set(d, last, n)?;
            for s in 0..self.spc {
                out.push(self.cluster_lba(n) + s);
            }
        }
        Ok(out)
    }

    fn short_display(e: &[u8]) -> String {
        let mut base: Vec<u8> = e[0..8].iter().copied().take_while(|&b| b != b' ').collect();
        if base.first() == Some(&0x05) {
            base[0] = 0xE5;
        }
        let ext: Vec<u8> = e[8..11].iter().copied().take_while(|&b| b != b' ').collect();
        let nt = e[12];
        let mut s = String::from_utf8_lossy(&base).into_owned();
        if nt & 0x08 != 0 {
            s = s.to_ascii_lowercase();
        }
        if !ext.is_empty() {
            let mut x = String::from_utf8_lossy(&ext).into_owned();
            if nt & 0x10 != 0 {
                x = x.to_ascii_lowercase();
            }
            s.push('.');
            s.push_str(&x);
        }
        s
    }

    /// Every entry of a directory (dot entries and volume labels left out).
    fn read_dir(&mut self, d: &mut dyn Disk, dir: Dir) -> Result<Vec<Entry>, ()> {
        let secs = self.dir_sectors(d, dir, false)?;
        let mut out = Vec::new();
        let mut parts: [Option<[u16; 13]>; MAX_LFN_PARTS] = [None; MAX_LFN_PARTS];
        let mut lfn_locs: Vec<(u32, usize)> = Vec::new();
        let mut buf = [0u8; SECTOR];
        for lba in secs {
            d.read(lba, &mut buf)?;
            for i in 0..16 {
                let e = &buf[i * 32..i * 32 + 32];
                let first = e[0];
                if first == 0 {
                    return Ok(out);
                }
                if first == 0xE5 {
                    parts = [None; MAX_LFN_PARTS];
                    lfn_locs.clear();
                    continue;
                }
                let attr = e[11];
                if attr == ATTR_LFN {
                    let seq = (e[0] & 0x1F) as usize;
                    if e[0] & 0x40 != 0 {
                        parts = [None; MAX_LFN_PARTS];
                        lfn_locs.clear();
                    }
                    if (1..=MAX_LFN_PARTS).contains(&seq) {
                        let mut ch = [0u16; 13];
                        let offs = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
                        for (k, o) in offs.iter().enumerate() {
                            ch[k] = le16(e, *o) as u16;
                        }
                        parts[seq - 1] = Some(ch);
                        lfn_locs.push((lba, i * 32));
                    }
                    continue;
                }
                if attr & ATTR_LABEL != 0 {
                    parts = [None; MAX_LFN_PARTS];
                    lfn_locs.clear();
                    continue;
                }
                let mut name = String::new();
                if parts[0].is_some() {
                    let mut units: Vec<u16> = Vec::new();
                    for p in parts.iter() {
                        let Some(p) = p else { break };
                        for &u in p.iter() {
                            if u == 0 || u == 0xFFFF {
                                break;
                            }
                            units.push(u);
                        }
                    }
                    name = char::decode_utf16(units.iter().copied()).map(|r| r.unwrap_or('?')).collect();
                }
                if name.is_empty() {
                    name = Self::short_display(e);
                }
                let mut short = [0u8; 11];
                short.copy_from_slice(&e[0..11]);
                let cluster = le16(e, 26) | if self.fat32 { le16(e, 20) << 16 } else { 0 };
                let is_dir = attr & ATTR_DIR != 0;
                if name != "." && name != ".." {
                    out.push(Entry {
                        name,
                        is_dir,
                        size: le32(e, 28),
                        cluster,
                        short,
                        loc: (lba, i * 32),
                        lfn: core::mem::take(&mut lfn_locs),
                    });
                }
                parts = [None; MAX_LFN_PARTS];
                lfn_locs.clear();
            }
        }
        Ok(out)
    }

    fn find_in(&mut self, d: &mut dyn Disk, dir: Dir, name: &str) -> Result<Option<Entry>, ()> {
        Ok(self.read_dir(d, dir)?.into_iter().find(|e| e.name.eq_ignore_ascii_case(name)))
    }

    fn root_entry(&self) -> Entry {
        Entry { name: String::new(), is_dir: true, size: 0, cluster: self.root_cluster, short: [b' '; 11], loc: (0, 0), lfn: Vec::new() }
    }

    /// The entry a `/`-separated path names (the root for "").
    pub fn lookup(&mut self, d: &mut dyn Disk, path: &str) -> Result<Option<Entry>, ()> {
        let mut cur = self.root_entry();
        for comp in path.split('/').filter(|c| !c.is_empty() && *c != ".") {
            if !cur.is_dir {
                return Ok(None);
            }
            let dir = self.dir_of(&cur);
            match self.find_in(d, dir, comp)? {
                Some(e) => cur = e,
                None => return Ok(None),
            }
        }
        Ok(Some(cur))
    }

    /// `(name, is_dir, size)` of a folder's entries.
    pub fn list(&mut self, d: &mut dyn Disk, path: &str) -> Result<Vec<(String, bool, usize)>, ()> {
        let e = self.lookup(d, path)?.ok_or(())?;
        if !e.is_dir {
            return Err(());
        }
        let dir = self.dir_of(&e);
        Ok(self.read_dir(d, dir)?.into_iter().map(|e| (e.name, e.is_dir, e.size as usize)).collect())
    }

    pub fn read_file(&mut self, d: &mut dyn Disk, path: &str) -> Result<Vec<u8>, ()> {
        let e = self.lookup(d, path)?.ok_or(())?;
        if e.is_dir {
            return Err(());
        }
        let size = e.size as usize;
        let cb = self.cluster_bytes();
        let mut out: Vec<u8> = Vec::with_capacity(size);
        let mut buf = vec![0u8; cb];
        let mut c = e.cluster;
        let mut guard = 0;
        while self.valid_cluster(c) && !self.eoc(c) && out.len() < size && guard < 4_000_000 {
            d.read(self.cluster_lba(c), &mut buf)?;
            let take = (size - out.len()).min(cb);
            out.extend_from_slice(&buf[..take]);
            c = self.fat_get(d, c)?;
            guard += 1;
        }
        Ok(out)
    }

    /// Write `data` into a fresh cluster chain; its first cluster (0 if empty).
    fn write_chain(&mut self, d: &mut dyn Disk, data: &[u8]) -> Result<u32, ()> {
        if data.is_empty() {
            return Ok(0);
        }
        let cb = self.cluster_bytes();
        let (mut first, mut prev) = (0u32, 0u32);
        let mut buf = vec![0u8; cb];
        for chunk in data.chunks(cb) {
            let c = self.alloc(d).ok_or(())?;
            if prev != 0 {
                self.fat_set(d, prev, c)?;
            } else {
                first = c;
            }
            buf.fill(0);
            buf[..chunk.len()].copy_from_slice(chunk);
            d.write(self.cluster_lba(c), &buf)?;
            prev = c;
        }
        Ok(first)
    }

    fn update_entry(&mut self, d: &mut dyn Disk, e: &Entry, cluster: u32, size: u32) -> Result<(), ()> {
        let mut buf = [0u8; SECTOR];
        d.read(e.loc.0, &mut buf)?;
        let o = e.loc.1;
        put16(&mut buf, o + 26, cluster & 0xFFFF);
        if self.fat32 {
            put16(&mut buf, o + 20, cluster >> 16);
        }
        put32(&mut buf, o + 28, size);
        d.write(e.loc.0, &buf)
    }

    fn mark_deleted(&mut self, d: &mut dyn Disk, e: &Entry) -> Result<(), ()> {
        let mut locs = e.lfn.clone();
        locs.push(e.loc);
        let mut buf = [0u8; SECTOR];
        let mut loaded = u32::MAX;
        for (lba, off) in locs {
            if loaded != lba {
                if loaded != u32::MAX {
                    d.write(loaded, &buf)?;
                }
                d.read(lba, &mut buf)?;
                loaded = lba;
            }
            buf[off] = 0xE5;
        }
        if loaded != u32::MAX {
            d.write(loaded, &buf)?;
        }
        Ok(())
    }

    /// An 8.3 name for `name`, unique among `taken`: the uppercased stem and
    /// extension when they fit, else `STEM~N`.
    fn short_for(name: &str, taken: &[Entry]) -> [u8; 11] {
        let (stem, ext) = match name.rfind('.') {
            Some(i) if i > 0 => (&name[..i], &name[i + 1..]),
            _ => (name, ""),
        };
        let clean = |s: &str, max: usize| -> (Vec<u8>, bool) {
            let mut out = Vec::new();
            let mut lossy = false;
            for ch in s.chars() {
                if out.len() >= max {
                    lossy = true;
                    break;
                }
                let b = ch.to_ascii_uppercase();
                if b.is_ascii_alphanumeric() || "!#$%&'()-@^_`{}~".contains(b) {
                    out.push(b as u8);
                } else if b == ' ' || b == '.' {
                    lossy = true;
                } else {
                    out.push(b'_');
                    lossy = true;
                }
            }
            (out, lossy)
        };
        let (mut base, lossy_b) = clean(stem, 8);
        let (ext3, lossy_e) = clean(ext, 3);
        if base.is_empty() {
            base = b"FILE".to_vec();
        }
        let fits = !lossy_b && !lossy_e && stem.len() <= 8 && ext.len() <= 3 && stem.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            && ext.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        let make = |b: &[u8]| -> [u8; 11] {
            let mut s = [b' '; 11];
            s[..b.len()].copy_from_slice(b);
            s[8..8 + ext3.len()].copy_from_slice(&ext3);
            s
        };
        let free = |s: &[u8; 11]| !taken.iter().any(|e| &e.short == s);
        if fits {
            let s = make(&base);
            if free(&s) {
                return s;
            }
        }
        for n in 1..1000u32 {
            let tail = alloc::format!("~{n}");
            let keep = base.len().min(8 - tail.len());
            let mut b = base[..keep].to_vec();
            b.extend_from_slice(tail.as_bytes());
            let s = make(&b);
            if free(&s) {
                return s;
            }
        }
        make(&base)
    }

    /// Add an entry (with long-name entries) to a directory.
    fn create(&mut self, d: &mut dyn Disk, dir: Dir, name: &str, attr: u8, cluster: u32, size: u32) -> Result<Entry, ()> {
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.len() > 255 {
            return Err(());
        }
        let existing = self.read_dir(d, dir)?;
        let short = Self::short_for(name, &existing);
        let units: Vec<u16> = name.encode_utf16().collect();
        let n_lfn = units.len().div_ceil(13).min(MAX_LFN_PARTS).max(1);
        let slots = n_lfn + 1;
        // A run of `slots` free entries, growing the directory if needed.
        let mut secs = self.dir_sectors(d, dir, false)?;
        let mut positions: Vec<(u32, usize)> = Vec::new();
        let mut buf = [0u8; SECTOR];
        let mut grown = false;
        loop {
            positions.clear();
            'scan: for &lba in &secs {
                d.read(lba, &mut buf)?;
                for i in 0..16 {
                    let f = buf[i * 32];
                    if f == 0 || f == 0xE5 {
                        positions.push((lba, i * 32));
                        if positions.len() == slots {
                            break 'scan;
                        }
                    } else {
                        positions.clear();
                    }
                }
            }
            if positions.len() == slots {
                break;
            }
            if grown {
                return Err(());
            }
            secs = self.dir_sectors(d, dir, true)?;
            grown = true;
        }
        let sum = checksum(&short);
        // LFN entries first (highest sequence first), then the 8.3 entry.
        for (k, (lba, off)) in positions.iter().enumerate() {
            d.read(*lba, &mut buf)?;
            let e = &mut buf[*off..*off + 32];
            if k < n_lfn {
                let seq = n_lfn - k; // 1-based logical index
                e.fill(0xFF);
                e[0] = seq as u8 | if k == 0 { 0x40 } else { 0 };
                e[11] = ATTR_LFN;
                e[12] = 0;
                e[13] = sum;
                e[26] = 0;
                e[27] = 0;
                let offs = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
                let base = (seq - 1) * 13;
                let mut ended = false;
                for (j, o) in offs.iter().enumerate() {
                    let idx = base + j;
                    let u = if idx < units.len() {
                        units[idx]
                    } else if !ended {
                        ended = true;
                        0
                    } else {
                        0xFFFF
                    };
                    put16(e, *o, u as u32);
                }
            } else {
                e.fill(0);
                e[..11].copy_from_slice(&short);
                e[11] = attr;
                put16(e, 26, cluster & 0xFFFF);
                if self.fat32 {
                    put16(e, 20, cluster >> 16);
                }
                put32(e, 28, size);
            }
            d.write(*lba, &buf)?;
        }
        let loc = positions[slots - 1];
        Ok(Entry { name: String::from(name), is_dir: attr & ATTR_DIR != 0, size, cluster, short, loc, lfn: positions[..n_lfn].to_vec() })
    }

    /// Write (create or replace) a file.
    pub fn write_file(&mut self, d: &mut dyn Disk, path: &str, data: &[u8]) -> Result<(), ()> {
        let (parent, name) = split_path(path);
        let pe = self.lookup(d, parent)?.ok_or(())?;
        if !pe.is_dir {
            return Err(());
        }
        let dir = self.dir_of(&pe);
        match self.find_in(d, dir, name)? {
            Some(e) if e.is_dir => Err(()),
            Some(e) => {
                self.free_chain(d, e.cluster);
                let c = self.write_chain(d, data)?;
                self.update_entry(d, &e, c, data.len() as u32)
            },
            None => {
                let c = self.write_chain(d, data)?;
                self.create(d, dir, name, 0, c, data.len() as u32)?;
                Ok(())
            },
        }
    }

    pub fn mkdir(&mut self, d: &mut dyn Disk, path: &str) -> Result<(), ()> {
        let (parent, name) = split_path(path);
        let pe = self.lookup(d, parent)?.ok_or(())?;
        if !pe.is_dir {
            return Err(());
        }
        let dir = self.dir_of(&pe);
        if self.find_in(d, dir, name)?.is_some() {
            return Err(());
        }
        let c = self.alloc(d).ok_or(())?;
        self.zero_cluster(d, c)?;
        let mut buf = [0u8; SECTOR];
        let parent_c = if pe.cluster == self.root_cluster { 0 } else { pe.cluster };
        for (i, (nm, cl)) in [(b".          ", c), (b"..         ", parent_c)].iter().enumerate() {
            let e = &mut buf[i * 32..i * 32 + 32];
            e[..11].copy_from_slice(*nm);
            e[11] = ATTR_DIR;
            put16(e, 26, cl & 0xFFFF);
            if self.fat32 {
                put16(e, 20, cl >> 16);
            }
        }
        d.write(self.cluster_lba(c), &buf)?;
        self.create(d, dir, name, ATTR_DIR, c, 0)?;
        Ok(())
    }

    /// Delete a file or an empty folder.
    pub fn delete(&mut self, d: &mut dyn Disk, path: &str) -> Result<(), ()> {
        let e = self.lookup(d, path)?.ok_or(())?;
        if e.loc == (0, 0) {
            return Err(()); // the root
        }
        if e.is_dir {
            let dir = self.dir_of(&e);
            if !self.read_dir(d, dir)?.is_empty() {
                return Err(());
            }
        }
        self.mark_deleted(d, &e)?;
        if e.cluster != 0 {
            self.free_chain(d, e.cluster);
        }
        Ok(())
    }

    /// Rename or move within the volume (the data stays where it is).
    pub fn rename(&mut self, d: &mut dyn Disk, from: &str, to: &str) -> Result<(), ()> {
        let e = self.lookup(d, from)?.ok_or(())?;
        if e.loc == (0, 0) {
            return Err(());
        }
        let (parent, name) = split_path(to);
        let pe = self.lookup(d, parent)?.ok_or(())?;
        if !pe.is_dir {
            return Err(());
        }
        let dir = self.dir_of(&pe);
        if self.find_in(d, dir, name)?.is_some() {
            return Err(());
        }
        let attr = if e.is_dir { ATTR_DIR } else { 0 };
        self.create(d, dir, name, attr, e.cluster, e.size)?;
        self.mark_deleted(d, &e)?;
        if e.is_dir && e.cluster != 0 {
            // The folder's ".." now points at its new parent.
            let mut buf = [0u8; SECTOR];
            let lba = self.cluster_lba(e.cluster);
            d.read(lba, &mut buf)?;
            if &buf[32..43] == b"..         " {
                let parent_c = if pe.cluster == self.root_cluster { 0 } else { pe.cluster };
                put16(&mut buf, 32 + 26, parent_c & 0xFFFF);
                if self.fat32 {
                    put16(&mut buf, 32 + 20, parent_c >> 16);
                }
                d.write(lba, &buf)?;
            }
        }
        Ok(())
    }
}

/// A file being written cluster by cluster (`Volume::stream_*`): nothing
/// larger than one cluster is buffered, so a file of any size up to FAT's
/// 4 GiB limit streams to disk. The directory entry appears only at
/// `stream_finish`; `stream_abort` gives the clusters back.
pub struct StreamFile {
    first: u32,
    last: u32,
    size: u64,
    buf: Vec<u8>,
}

/// FAT32's largest file (the directory entry's size field is 32-bit).
pub const MAX_FILE: u64 = u32::MAX as u64;

impl Volume {
    pub fn stream_begin(&self) -> StreamFile {
        StreamFile { first: 0, last: 0, size: 0, buf: Vec::with_capacity(self.cluster_bytes()) }
    }

    fn stream_flush(&mut self, d: &mut dyn Disk, f: &mut StreamFile) -> Result<(), ()> {
        if f.buf.is_empty() {
            return Ok(());
        }
        let c = self.alloc(d).ok_or(())?; // disk full
        if f.last != 0 {
            self.fat_set(d, f.last, c)?;
        } else {
            f.first = c;
        }
        f.last = c;
        f.buf.resize(self.cluster_bytes(), 0);
        d.write(self.cluster_lba(c), &f.buf)?;
        f.buf.clear();
        Ok(())
    }

    /// Append bytes. Err when the disk is full or the file passes 4 GiB.
    pub fn stream_write(&mut self, d: &mut dyn Disk, f: &mut StreamFile, mut data: &[u8]) -> Result<(), ()> {
        if f.size + data.len() as u64 > MAX_FILE {
            return Err(());
        }
        f.size += data.len() as u64;
        let cb = self.cluster_bytes();
        while !data.is_empty() {
            let take = (cb - f.buf.len()).min(data.len());
            f.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if f.buf.len() == cb {
                self.stream_flush(d, f)?;
            }
        }
        Ok(())
    }

    /// Write the last partial cluster and create (or replace) `path`'s entry.
    pub fn stream_finish(&mut self, d: &mut dyn Disk, mut f: StreamFile, path: &str) -> Result<(), ()> {
        self.stream_flush(d, &mut f)?;
        let (parent, name) = split_path(path);
        let pe = self.lookup(d, parent)?.ok_or(())?;
        if !pe.is_dir {
            self.free_chain(d, f.first);
            return Err(());
        }
        let dir = self.dir_of(&pe);
        let r = match self.find_in(d, dir, name)? {
            Some(e) if e.is_dir => Err(()),
            Some(e) => {
                if e.cluster != 0 {
                    self.free_chain(d, e.cluster);
                }
                self.update_entry(d, &e, f.first, f.size as u32)
            },
            None => self.create(d, dir, name, 0, f.first, f.size as u32).map(|_| ()),
        };
        if r.is_err() && f.first != 0 {
            self.free_chain(d, f.first);
        }
        r
    }

    /// Drop a half-written file: its clusters go back to the free pool.
    pub fn stream_abort(&mut self, d: &mut dyn Disk, f: StreamFile) {
        if f.first != 0 {
            self.free_chain(d, f.first);
        }
    }
}

// ── random-access reads of big files (a language model's weights) ──────────

/// A file opened for reading anywhere in it without holding it in memory:
/// its cluster chain walked once into runs of consecutive sectors
/// `(file offset, first absolute LBA, bytes)`, so a read at any offset of a
/// multi-GB file costs no FAT walk. Valid while the file is not rewritten.
pub struct RangeFile {
    pub size: u64,
    runs: Vec<(u64, u32, u64)>,
}

impl Volume {
    /// Open `path` (a file) for [`RangeFile::read_at`].
    pub fn open_range(&mut self, d: &mut dyn Disk, path: &str) -> Result<RangeFile, ()> {
        let e = self.lookup(d, path)?.ok_or(())?;
        if e.is_dir {
            return Err(());
        }
        let size = e.size as u64;
        let cb = self.cluster_bytes() as u64;
        let mut runs: Vec<(u64, u32, u64)> = Vec::new();
        let mut c = e.cluster;
        let mut off = 0u64;
        let mut guard = 0u32;
        while off < size && self.valid_cluster(c) && !self.eoc(c) && guard < 4_000_000 {
            let lba = self.cluster_lba(c);
            match runs.last_mut() {
                // this cluster follows the last run on the disk: extend it
                Some(r) if r.1 as u64 + r.2 / SECTOR as u64 == lba as u64 => r.2 += cb,
                _ => runs.push((off, lba, cb)),
            }
            off += cb;
            c = self.fat_get(d, c)?;
            guard += 1;
        }
        if off < size {
            return Err(()); // the chain is shorter than the file
        }
        Ok(RangeFile { size, runs })
    }
}

impl RangeFile {
    /// Fill `buf` with the bytes at `offset..offset + buf.len()` (all of them
    /// inside the file). Whole sectors go straight into `buf`.
    pub fn read_at(&self, d: &mut dyn Disk, offset: u64, buf: &mut [u8]) -> Result<(), ()> {
        let end = offset.checked_add(buf.len() as u64).ok_or(())?;
        if end > self.size {
            return Err(());
        }
        let mut pos = offset;
        let mut done = 0usize;
        // the run holding `offset`
        let mut i = match self.runs.binary_search_by(|r| r.0.cmp(&offset)) {
            Ok(i) => i,
            Err(i) => i.checked_sub(1).ok_or(())?,
        };
        let mut tmp = [0u8; SECTOR];
        while done < buf.len() {
            let (roff, lba, rlen) = *self.runs.get(i).ok_or(())?;
            let in_run = pos - roff;
            if in_run >= rlen {
                i += 1;
                continue;
            }
            let take = ((rlen - in_run) as usize).min(buf.len() - done);
            let sec = lba + (in_run / SECTOR as u64) as u32;
            let skip = (in_run % SECTOR as u64) as usize;
            let out = &mut buf[done..done + take];
            if skip == 0 && take >= SECTOR {
                // the whole sectors straight in, a partial last one through tmp
                let whole = take / SECTOR * SECTOR;
                d.read(sec, &mut out[..whole])?;
                if whole < take {
                    d.read(sec + (whole / SECTOR) as u32, &mut tmp)?;
                    let n = take - whole;
                    out[whole..].copy_from_slice(&tmp[..n]);
                }
            } else {
                // a partial first sector, then recurse on the rest of this run
                d.read(sec, &mut tmp)?;
                let n = (SECTOR - skip).min(take);
                out[..n].copy_from_slice(&tmp[skip..skip + n]);
                pos += n as u64;
                done += n;
                continue;
            }
            pos += take as u64;
            done += take;
            i += 1;
        }
        Ok(())
    }
}
