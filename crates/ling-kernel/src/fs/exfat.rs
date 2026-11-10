//! exFAT volumes -- what big USB drives, SDXC cards and data partitions that
//! must hold files over 4 GiB (Stable Diffusion checkpoints) carry -- read and
//! write, after Microsoft's exFAT specification: the boot region, the FAT and
//! files without a FAT chain (NoFatChain), the allocation bitmap (the record of
//! which clusters are in use), the up-case table (names compare without regard
//! to case), directory entry sets (File + Stream Extension + File Name entries
//! with their SetChecksum and NameHash), ValidDataLength, the volume label.
//!
//! The same shape as `fs::fat::Volume` so `fs::media` can hold either. Every
//! change is written through at once (no dirty data is kept between calls);
//! VolumeDirty is set in the boot sector while a change is in flight. New
//! files are placed in one run of free clusters (NoFatChain) whenever there is
//! room, with a FAT chain only when the free space is fragmented. TexFAT (two
//! FATs) volumes mount read-only. Sector numbers on the device are 32-bit
//! (`fat::Disk`), so a volume must end below 2 TiB on its disk.

use crate::fs::fat::{Disk, RangeFile, SECTOR};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

const T_BITMAP: u8 = 0x81;
const T_UPCASE: u8 = 0x82;
const T_LABEL: u8 = 0x83;
const T_FILE: u8 = 0x85;
const T_STREAM: u8 = 0xC0;
const T_NAME: u8 = 0xC1;
const ATTR_DIR: u16 = 0x10;
const ATTR_ARCHIVE: u16 = 0x20;
const F_ALLOC: u8 = 0x01;
const F_NOFAT: u8 = 0x02;
const EOC: u32 = 0xFFFF_FFFF;
const MAX_NAME: usize = 255;
/// Largest single disk transfer.
const IO_CHUNK: usize = 64 * 1024;

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn le64(b: &[u8], o: usize) -> u64 {
    le32(b, o) as u64 | ((le32(b, o + 4) as u64) << 32)
}
fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

fn split_path(path: &str) -> (&str, &str) {
    let p = path.trim_matches('/');
    match p.rfind('/') {
        Some(i) => (&p[..i], &p[i + 1..]),
        None => ("", p),
    }
}

fn rd(d: &mut dyn Disk, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
    if lba + (buf.len() / SECTOR) as u64 > u32::MAX as u64 {
        return Err(());
    }
    d.read(lba as u32, buf)
}

fn wr(d: &mut dyn Disk, lba: u64, buf: &[u8]) -> Result<(), ()> {
    if lba + (buf.len() / SECTOR) as u64 > u32::MAX as u64 {
        return Err(());
    }
    d.write(lba as u32, buf)
}

/// SetChecksum over a directory entry set (bytes 2..4 of the first entry,
/// the checksum itself, left out).
fn set_checksum(set: &[u8]) -> u16 {
    let mut c: u16 = 0;
    for (i, &b) in set.iter().enumerate() {
        if i == 2 || i == 3 {
            continue;
        }
        c = (if c & 1 != 0 { 0x8000u16 } else { 0 }).wrapping_add(c >> 1).wrapping_add(b as u16);
    }
    c
}

/// A name exFAT accepts: 1..=255 UTF-16 units, none of `"*/:<>?\|` or controls.
fn name_ok(name: &str) -> bool {
    let n = name.encode_utf16().count();
    n > 0 && n <= MAX_NAME && name != "." && name != ".." && !name.chars().any(|c| (c as u32) < 0x20 || "\"*/:<>?\\|".contains(c))
}

/// Seconds since 1970 as an exFAT (DOS-style) timestamp, local = UTC.
fn stamp(t: u64) -> u32 {
    let days = (t / 86400) as i64;
    let secs = (t % 86400) as u32;
    // civil_from_days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = (yoe + era * 400 + if month <= 2 { 1 } else { 0 }) as u32;
    if !(1980..2108).contains(&year) {
        return 1 << 21 | 1 << 16; // 1980-01-01
    }
    ((year - 1980) << 25) | (month << 21) | (day << 16) | ((secs / 3600) << 11) | (((secs / 60) % 60) << 5) | ((secs % 60) / 2)
}

fn now_stamp() -> u32 {
    stamp(crate::clock::unix_now())
}

/// A file or folder as found in its parent folder. `set` is where its
/// directory entry set lives (device LBA + byte offset of each 32-byte
/// entry); empty for the root folder.
#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    /// DataLength: the file's size in bytes.
    pub size: u64,
    /// ValidDataLength: bytes past this read as zeros.
    valid: u64,
    first: u32,
    nofat: bool,
    set: Vec<(u64, usize)>,
}

/// The clusters a file holds: one run (`nofat`), or a FAT chain.
#[derive(Clone, Copy, Default)]
struct Alloc {
    first: u32,
    last: u32,
    count: u32,
    nofat: bool,
}

pub struct Volume {
    part_lba: u64,
    /// log2 of 512-byte device sectors per cluster
    spc_shift: u32,
    fat_lba: u64,
    heap_lba: u64,
    clusters: u32,
    root: u32,
    /// device LBAs of the allocation bitmap's sectors, in order
    bm_secs: Vec<u64>,
    upcase: Vec<u16>,
    pub label: String,
    read_only: bool,
    hint: u32,
    /// VolumeDirty was already set when mounted (left as found)
    was_dirty: bool,
    /// we set VolumeDirty and clear it when the change is done
    dirty_set: bool,
    /// streamed files being written (VolumeDirty stays set meanwhile)
    streams: u32,
    fat_cache: (u64, [u8; SECTOR], bool),
    bm_cache: (u64, [u8; SECTOR], bool),
}

impl Volume {
    /// Mount the exFAT volume whose boot sector is at `part_lba`.
    pub fn mount(d: &mut dyn Disk, part_lba: u32) -> Option<Volume> {
        let mut bs = [0u8; SECTOR];
        d.read(part_lba, &mut bs).ok()?;
        if &bs[3..11] != b"EXFAT   " || bs[510] != 0x55 || bs[511] != 0xAA || bs[11..64].iter().any(|&b| b != 0) {
            return None;
        }
        let bps = bs[108] as u32;
        let spcs = bs[109] as u32;
        if !(9..=12).contains(&bps) || bps + spcs > 25 {
            return None;
        }
        let sec_shift = bps - 9;
        let fat_off = le32(&bs, 80) as u64;
        let fat_len = le32(&bs, 84) as u64;
        let heap_off = le32(&bs, 88) as u64;
        let clusters = le32(&bs, 92);
        let root = le32(&bs, 96);
        let flags = le16(&bs, 106);
        let nfats = bs[110];
        if !(1..=2).contains(&nfats) || clusters == 0 || clusters > 0xFFFF_FFF5 || fat_len == 0 {
            return None;
        }
        let active = if nfats == 2 && flags & 1 != 0 { 1u64 } else { 0 };
        let part = part_lba as u64;
        let mut v = Volume {
            part_lba: part,
            spc_shift: sec_shift + spcs,
            fat_lba: part + ((fat_off + active * fat_len) << sec_shift),
            heap_lba: part + (heap_off << sec_shift),
            clusters,
            root,
            bm_secs: Vec::new(),
            upcase: (0..=0xFFFFu32).map(|c| c as u16).collect(),
            label: String::new(),
            read_only: nfats == 2,
            hint: 2,
            was_dirty: flags & 2 != 0,
            dirty_set: false,
            streams: 0,
            fat_cache: (0, [0; SECTOR], false),
            bm_cache: (0, [0; SECTOR], false),
        };
        if !v.valid_c(root) {
            return None;
        }
        // the root folder's system entries: bitmap, up-case table, label
        let root_e = v.root_entry();
        let (buf, _) = v.dir_load(d, &root_e).ok()?;
        let (mut bm, mut up) = (None, None);
        for e in buf.chunks(32) {
            match e[0] {
                0 => break,
                T_BITMAP if (e[1] & 1) as u64 == active => bm = Some((le32(e, 20), le64(e, 24))),
                T_UPCASE => up = Some((le32(e, 20), le64(e, 24))),
                T_LABEL => {
                    let n = (e[1] as usize).min(11);
                    let units: Vec<u16> = (0..n).map(|i| le16(e, 2 + 2 * i)).collect();
                    v.label = char::decode_utf16(units).map(|r| r.unwrap_or('?')).collect();
                },
                _ => {},
            }
        }
        let (bm_first, bm_len) = bm?;
        if bm_len < (clusters as u64).div_ceil(8) {
            return None;
        }
        let need = bm_len.div_ceil(SECTOR as u64) as usize;
        for (s, n) in v.runs_of(d, bm_first, false, bm_len).ok()? {
            for k in 0..(n as u64) << v.spc_shift {
                if v.bm_secs.len() < need {
                    v.bm_secs.push(v.clu_lba(s) + k);
                }
            }
        }
        if v.bm_secs.len() < need {
            return None;
        }
        match up {
            Some((first, len)) if len >= 2 && len <= 2 * 65536 * 2 => {
                let raw = v.read_runs(d, first, false, len).ok()?;
                let mut idx = 0usize;
                let mut i = 0usize;
                while i + 1 < raw.len() && idx < 65536 {
                    let u = le16(&raw, i);
                    if u == 0xFFFF && i + 3 < raw.len() {
                        idx += le16(&raw, i + 2) as usize; // a run left as it is
                        i += 4;
                    } else {
                        v.upcase[idx] = u;
                        idx += 1;
                        i += 2;
                    }
                }
            },
            _ => {
                for c in b'a'..=b'z' {
                    v.upcase[c as usize] = (c - 32) as u16;
                }
            },
        }
        Some(v)
    }

    /// TexFAT (two FATs) volumes are not written to.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// The largest file a volume can hold (exFAT has no 4 GiB limit).
    pub fn max_file(&self) -> u64 {
        self.clusters as u64 * self.cb()
    }

    fn cb(&self) -> u64 {
        (SECTOR as u64) << self.spc_shift
    }

    fn clu_lba(&self, c: u32) -> u64 {
        self.heap_lba + (((c - 2) as u64) << self.spc_shift)
    }

    fn valid_c(&self, c: u32) -> bool {
        c >= 2 && c - 2 < self.clusters
    }

    // ── VolumeDirty ─────────────────────────────────────────────────────────

    fn set_dirty_flag(&mut self, d: &mut dyn Disk, on: bool) -> Result<(), ()> {
        let mut bs = [0u8; SECTOR];
        rd(d, self.part_lba, &mut bs)?;
        let f = le16(&bs, 106);
        put16(&mut bs, 106, if on { f | 2 } else { f & !2 });
        wr(d, self.part_lba, &bs)
    }

    /// Before a change: VolumeDirty on (a cut-off change is then checked by
    /// the next chkdsk / fsck). Err on a read-only volume.
    fn begin(&mut self, d: &mut dyn Disk) -> Result<(), ()> {
        if self.read_only {
            return Err(());
        }
        if !self.dirty_set && !self.was_dirty {
            self.set_dirty_flag(d, true)?;
            self.dirty_set = true;
        }
        Ok(())
    }

    /// After a change (and once no streamed file is open): VolumeDirty off.
    fn end(&mut self, d: &mut dyn Disk) {
        if self.dirty_set && self.streams == 0 && self.set_dirty_flag(d, false).is_ok() {
            self.dirty_set = false;
        }
    }

    // ── FAT ─────────────────────────────────────────────────────────────────

    fn fat_load(&mut self, d: &mut dyn Disk, c: u32) -> Result<usize, ()> {
        if !self.valid_c(c) {
            return Err(());
        }
        let lba = self.fat_lba + c as u64 / 128;
        if !self.fat_cache.2 || self.fat_cache.0 != lba {
            let mut b = [0u8; SECTOR];
            rd(d, lba, &mut b)?;
            self.fat_cache = (lba, b, true);
        }
        Ok((c % 128) as usize * 4)
    }

    fn fat_get(&mut self, d: &mut dyn Disk, c: u32) -> Result<u32, ()> {
        let o = self.fat_load(d, c)?;
        Ok(le32(&self.fat_cache.1, o))
    }

    /// FAT entries of `start..start+len`: a chain through the run ending in
    /// `then` (Some), or zeros (None). One write per FAT sector touched.
    fn fat_run(&mut self, d: &mut dyn Disk, start: u32, len: u32, then: Option<u32>) -> Result<(), ()> {
        let mut pending = false;
        for i in 0..len {
            let c = start + i;
            let lba = self.fat_lba + c as u64 / 128;
            if pending && self.fat_cache.0 != lba {
                let (l, b) = (self.fat_cache.0, self.fat_cache.1);
                wr(d, l, &b)?;
            }
            let o = self.fat_load(d, c)?;
            let v = match then {
                None => 0,
                Some(t) => {
                    if i + 1 < len {
                        c + 1
                    } else {
                        t
                    }
                },
            };
            put32(&mut self.fat_cache.1, o, v);
            pending = true;
        }
        if pending {
            let (l, b) = (self.fat_cache.0, self.fat_cache.1);
            wr(d, l, &b)?;
        }
        Ok(())
    }

    fn fat_put(&mut self, d: &mut dyn Disk, c: u32, v: u32) -> Result<(), ()> {
        let o = self.fat_load(d, c)?;
        put32(&mut self.fat_cache.1, o, v);
        let (l, b) = (self.fat_cache.0, self.fat_cache.1);
        wr(d, l, &b)
    }

    // ── allocation bitmap ───────────────────────────────────────────────────

    fn bm_load(&mut self, d: &mut dyn Disk, c: u32) -> Result<(usize, u8), ()> {
        if !self.valid_c(c) {
            return Err(());
        }
        let idx = (c - 2) as usize;
        let byte = idx / 8;
        let lba = *self.bm_secs.get(byte / SECTOR).ok_or(())?;
        if !self.bm_cache.2 || self.bm_cache.0 != lba {
            let mut b = [0u8; SECTOR];
            rd(d, lba, &mut b)?;
            self.bm_cache = (lba, b, true);
        }
        Ok((byte % SECTOR, 1 << (idx % 8)))
    }

    fn bm_get(&mut self, d: &mut dyn Disk, c: u32) -> Result<bool, ()> {
        let (o, m) = self.bm_load(d, c)?;
        Ok(self.bm_cache.1[o] & m != 0)
    }

    /// Mark `start..start+len` in use (or free). One write per bitmap sector.
    fn bm_set(&mut self, d: &mut dyn Disk, start: u32, len: u32, used: bool) -> Result<(), ()> {
        let mut pending = false;
        for c in start..start + len {
            let lba = *self.bm_secs.get((c - 2) as usize / 8 / SECTOR).ok_or(())?;
            if pending && self.bm_cache.0 != lba {
                let (l, b) = (self.bm_cache.0, self.bm_cache.1);
                wr(d, l, &b)?;
            }
            let (o, m) = self.bm_load(d, c)?;
            if used {
                self.bm_cache.1[o] |= m;
            } else {
                self.bm_cache.1[o] &= !m;
            }
            pending = true;
        }
        if pending {
            let (l, b) = (self.bm_cache.0, self.bm_cache.1);
            wr(d, l, &b)?;
        }
        if !used && start < self.hint {
            self.hint = start;
        }
        Ok(())
    }

    /// The first run of `want` free clusters at or after `from` (wrapping to
    /// the start once), else the longest free run seen; None when full.
    fn find_free(&mut self, d: &mut dyn Disk, want: u32, from: u32) -> Result<Option<(u32, u32)>, ()> {
        let end = self.clusters + 2;
        let from = if self.valid_c(from) { from } else { 2 };
        let mut best = (0u32, 0u32);
        for (lo, hi) in [(from, end), (2, from)] {
            let (mut rs, mut rl) = (0u32, 0u32);
            let mut c = lo;
            while c < hi {
                // whole bytes in use: skip 8 at once
                if (c - 2) % 8 == 0 && c + 8 <= hi {
                    let (o, _) = self.bm_load(d, c)?;
                    if self.bm_cache.1[o] == 0xFF {
                        rl = 0;
                        c += 8;
                        continue;
                    }
                }
                if self.bm_get(d, c)? {
                    rl = 0;
                } else {
                    if rl == 0 {
                        rs = c;
                    }
                    rl += 1;
                    if rl > best.1 {
                        best = (rs, rl);
                    }
                    if rl >= want {
                        return Ok(Some((rs, rl)));
                    }
                }
                c += 1;
            }
        }
        Ok(if best.1 > 0 { Some(best) } else { None })
    }

    /// Add `n` clusters to `a`: right after it while that stays one run, else
    /// it becomes a FAT chain. A new file looks for a free run of `prefer`.
    fn alloc_grow(&mut self, d: &mut dyn Disk, a: &mut Alloc, n: u32, prefer: u32) -> Result<(), ()> {
        let mut need = n;
        if need == 0 {
            return Ok(());
        }
        if a.count > 0 && a.nofat {
            let mut k = 0;
            while k < need && self.valid_c(a.last + 1 + k) && !self.bm_get(d, a.last + 1 + k)? {
                k += 1;
            }
            if k == need {
                self.bm_set(d, a.last + 1, need, true)?;
                a.last += need;
                a.count += need;
                self.hint = a.last + 1;
                return Ok(());
            }
            // the clusters after it are taken: chain what it has
            self.fat_run(d, a.first, a.count, Some(EOC))?;
            a.nofat = false;
        }
        while need > 0 {
            let from = if a.count > 0 { a.last + 1 } else { self.hint };
            let want = if a.count == 0 { need.max(prefer) } else { need };
            let (s, len) = self.find_free(d, want, from)?.ok_or(())?;
            let len = len.min(need);
            self.bm_set(d, s, len, true)?;
            if a.count == 0 {
                a.first = s;
                a.nofat = len == need;
                if !a.nofat {
                    self.fat_run(d, s, len, Some(EOC))?;
                }
            } else {
                self.fat_run(d, s, len, Some(EOC))?;
                self.fat_put(d, a.last, s)?;
            }
            a.last = s + len - 1;
            a.count += len;
            need -= len;
            self.hint = a.last + 1;
        }
        Ok(())
    }

    /// Give a file's clusters back.
    fn alloc_free(&mut self, d: &mut dyn Disk, a: &Alloc) -> Result<(), ()> {
        if a.count == 0 || a.first == 0 {
            return Ok(());
        }
        if a.nofat {
            return self.bm_set(d, a.first, a.count, false);
        }
        let runs = self.chain_runs(d, a.first, u32::MAX)?;
        for (s, l) in runs {
            self.bm_set(d, s, l, false)?;
            self.fat_run(d, s, l, None)?;
        }
        Ok(())
    }

    /// A FAT chain from `first` as runs of consecutive clusters, at most
    /// `max` clusters; a free (0) entry inside a chain is read as "the next
    /// cluster" (some formatters leave system files' FAT entries empty).
    fn chain_runs(&mut self, d: &mut dyn Disk, first: u32, max: u32) -> Result<Vec<(u32, u32)>, ()> {
        let mut runs: Vec<(u32, u32)> = Vec::new();
        let mut c = first;
        let mut n = 0u32;
        while self.valid_c(c) && n < max && n <= self.clusters {
            match runs.last_mut() {
                Some(r) if r.0 + r.1 == c => r.1 += 1,
                _ => runs.push((c, 1)),
            }
            n += 1;
            let next = self.fat_get(d, c)?;
            c = if next == 0 && max != u32::MAX { c + 1 } else { next };
        }
        Ok(runs)
    }

    /// The cluster runs holding `bytes` of a file (u64::MAX: to the chain's end).
    fn runs_of(&mut self, d: &mut dyn Disk, first: u32, nofat: bool, bytes: u64) -> Result<Vec<(u32, u32)>, ()> {
        if first == 0 || bytes == 0 {
            return Ok(Vec::new());
        }
        let n = if bytes == u64::MAX { u32::MAX } else { u32::try_from(bytes.div_ceil(self.cb())).map_err(|_| ())? };
        if nofat {
            if !self.valid_c(first) || (first - 2) as u64 + n as u64 > self.clusters as u64 {
                return Err(());
            }
            return Ok(vec![(first, n)]);
        }
        let runs = self.chain_runs(d, first, n)?;
        if n != u32::MAX && runs.iter().map(|r| r.1 as u64).sum::<u64>() < n as u64 {
            return Err(()); // the chain is shorter than the file
        }
        Ok(runs)
    }

    fn alloc_of(&mut self, d: &mut dyn Disk, e: &Entry) -> Result<Alloc, ()> {
        if e.first == 0 {
            return Ok(Alloc::default());
        }
        if e.nofat {
            let count = e.size.div_ceil(self.cb()) as u32;
            if count == 0 {
                return Ok(Alloc::default());
            }
            return Ok(Alloc { first: e.first, last: e.first + count - 1, count, nofat: true });
        }
        let runs = self.chain_runs(d, e.first, u32::MAX)?;
        let count = runs.iter().map(|r| r.1).sum();
        let last = runs.last().map(|r| r.0 + r.1 - 1).unwrap_or(e.first);
        Ok(Alloc { first: e.first, last, count, nofat: false })
    }

    /// Read the runs into memory (`bytes` long).
    fn read_runs(&mut self, d: &mut dyn Disk, first: u32, nofat: bool, bytes: u64) -> Result<Vec<u8>, ()> {
        let runs = self.runs_of(d, first, nofat, bytes)?;
        let mut out = Vec::new();
        for (s, n) in runs {
            let mut lba = self.clu_lba(s);
            let mut left = (n as u64) << self.spc_shift;
            while left > 0 && (out.len() as u64) < bytes {
                let k = left.min((IO_CHUNK / SECTOR) as u64) as usize;
                let at = out.len();
                out.resize(at + k * SECTOR, 0);
                rd(d, lba, &mut out[at..])?;
                lba += k as u64;
                left -= k as u64;
            }
        }
        out.truncate(bytes.min(out.len() as u64) as usize);
        Ok(out)
    }

    fn zero_cluster(&self, d: &mut dyn Disk, c: u32) -> Result<(), ()> {
        let cb = self.cb() as usize;
        let z = vec![0u8; cb.min(IO_CHUNK)];
        let mut lba = self.clu_lba(c);
        let mut left = cb;
        while left > 0 {
            let k = left.min(z.len());
            wr(d, lba, &z[..k])?;
            lba += (k / SECTOR) as u64;
            left -= k;
        }
        Ok(())
    }

    // ── directories ─────────────────────────────────────────────────────────

    fn root_entry(&self) -> Entry {
        Entry { name: String::new(), is_dir: true, size: 0, valid: 0, first: self.root, nofat: false, set: Vec::new() }
    }

    /// A folder's bytes and the device LBA of each of its sectors.
    fn dir_load(&mut self, d: &mut dyn Disk, dir: &Entry) -> Result<(Vec<u8>, Vec<u64>), ()> {
        let bytes = if dir.set.is_empty() { u64::MAX } else { dir.size };
        let runs = self.runs_of(d, dir.first, dir.nofat, bytes)?;
        let mut secs = Vec::new();
        for (s, n) in runs {
            let base = self.clu_lba(s);
            for k in 0..(n as u64) << self.spc_shift {
                secs.push(base + k);
            }
        }
        if bytes != u64::MAX {
            secs.truncate(bytes.div_ceil(SECTOR as u64) as usize);
        }
        let mut buf = vec![0u8; secs.len() * SECTOR];
        let mut i = 0;
        while i < secs.len() {
            // consecutive sectors in one transfer
            let mut j = i + 1;
            while j < secs.len() && secs[j] == secs[j - 1] + 1 && (j - i) * SECTOR < IO_CHUNK {
                j += 1;
            }
            rd(d, secs[i], &mut buf[i * SECTOR..j * SECTOR])?;
            i = j;
        }
        Ok((buf, secs))
    }

    /// The files and folders of a loaded folder (entry sets that check out).
    fn parse_dir(buf: &[u8], secs: &[u64]) -> Vec<Entry> {
        let slots = buf.len() / 32;
        let loc = |j: usize| (secs[j * 32 / SECTOR], (j * 32) % SECTOR);
        let mut out = Vec::new();
        let mut i = 0;
        while i < slots {
            let t = buf[i * 32];
            if t == 0 {
                break;
            }
            if t != T_FILE {
                i += 1;
                continue;
            }
            let sc = buf[i * 32 + 1] as usize;
            if sc < 2 || i + sc >= slots {
                i += 1;
                continue;
            }
            let set = &buf[i * 32..(i + 1 + sc) * 32];
            let nl = set[32 + 3] as usize;
            let nents = nl.div_ceil(15);
            if set[32] != T_STREAM || nl == 0 || sc < 1 + nents || set_checksum(set) != le16(set, 2) || (0..nents).any(|k| set[64 + k * 32] != T_NAME) {
                i += 1;
                continue;
            }
            let mut units = Vec::with_capacity(nl);
            for k in 0..nl {
                units.push(le16(set, 64 + (k / 15) * 32 + 2 + (k % 15) * 2));
            }
            let name: String = char::decode_utf16(units).map(|r| r.unwrap_or('?')).collect();
            let attr = le16(set, 4);
            out.push(Entry {
                name,
                is_dir: attr & ATTR_DIR != 0,
                size: le64(set, 32 + 24),
                valid: le64(set, 32 + 8),
                first: le32(set, 32 + 20),
                nofat: set[33] & F_NOFAT != 0,
                set: (i..=i + sc).map(loc).collect(),
            });
            i += 1 + sc;
        }
        out
    }

    fn read_dir(&mut self, d: &mut dyn Disk, dir: &Entry) -> Result<Vec<Entry>, ()> {
        let (buf, secs) = self.dir_load(d, dir)?;
        Ok(Self::parse_dir(&buf, &secs))
    }

    fn up(&self, u: u16) -> u16 {
        self.upcase[u as usize]
    }

    fn name_eq(&self, a: &str, b: &str) -> bool {
        let (mut x, mut y) = (a.encode_utf16(), b.encode_utf16());
        loop {
            match (x.next(), y.next()) {
                (None, None) => return true,
                (Some(p), Some(q)) if self.up(p) == self.up(q) => {},
                _ => return false,
            }
        }
    }

    fn name_hash(&self, units: &[u16]) -> u16 {
        let mut h: u16 = 0;
        for &u in units {
            let c = self.up(u);
            for b in [c as u8, (c >> 8) as u8] {
                h = (if h & 1 != 0 { 0x8000u16 } else { 0 }).wrapping_add(h >> 1).wrapping_add(b as u16);
            }
        }
        h
    }

    fn find_in(&mut self, d: &mut dyn Disk, dir: &Entry, name: &str) -> Result<Option<Entry>, ()> {
        Ok(self.read_dir(d, dir)?.into_iter().find(|e| self.name_eq(&e.name, name)))
    }

    /// The entry a `/`-separated path names (the root for "").
    pub fn lookup(&mut self, d: &mut dyn Disk, path: &str) -> Result<Option<Entry>, ()> {
        let mut cur = self.root_entry();
        for comp in path.split('/').filter(|c| !c.is_empty() && *c != ".") {
            if !cur.is_dir {
                return Ok(None);
            }
            match self.find_in(d, &cur, comp)? {
                Some(e) => cur = e,
                None => return Ok(None),
            }
        }
        Ok(Some(cur))
    }

    fn read_set(&self, d: &mut dyn Disk, locs: &[(u64, usize)]) -> Result<Vec<u8>, ()> {
        let mut out = vec![0u8; locs.len() * 32];
        let mut buf = [0u8; SECTOR];
        let mut loaded = u64::MAX;
        for (k, &(lba, off)) in locs.iter().enumerate() {
            if loaded != lba {
                rd(d, lba, &mut buf)?;
                loaded = lba;
            }
            out[k * 32..k * 32 + 32].copy_from_slice(&buf[off..off + 32]);
        }
        Ok(out)
    }

    fn write_set(&self, d: &mut dyn Disk, locs: &[(u64, usize)], set: &[u8]) -> Result<(), ()> {
        let mut buf = [0u8; SECTOR];
        let mut k = 0;
        while k < locs.len() {
            let lba = locs[k].0;
            rd(d, lba, &mut buf)?;
            while k < locs.len() && locs[k].0 == lba {
                let off = locs[k].1;
                buf[off..off + 32].copy_from_slice(&set[k * 32..k * 32 + 32]);
                k += 1;
            }
            wr(d, lba, &buf)?;
        }
        Ok(())
    }

    /// Read an entry set, change it, fix its checksum, write it back.
    fn patch_set(&self, d: &mut dyn Disk, locs: &[(u64, usize)], f: impl FnOnce(&mut [u8])) -> Result<(), ()> {
        let mut s = self.read_set(d, locs)?;
        f(&mut s);
        let ck = set_checksum(&s);
        put16(&mut s, 2, ck);
        self.write_set(d, locs, &s)
    }

    /// The stream extension's allocation fields.
    fn set_stream(s: &mut [u8], a: &Alloc, size: u64) {
        let st = &mut s[32..64];
        if a.count == 0 {
            st[1] = F_ALLOC;
            put32(st, 20, 0);
        } else {
            st[1] = F_ALLOC | if a.nofat { F_NOFAT } else { 0 };
            put32(st, 20, a.first);
        }
        put64(st, 8, size);
        put64(st, 24, size);
    }

    /// A complete entry set for `name` from a File and a Stream entry template.
    fn build_set(&self, name: &str, file: &[u8], stream: &[u8]) -> Result<Vec<u8>, ()> {
        if !name_ok(name) {
            return Err(());
        }
        let units: Vec<u16> = name.encode_utf16().collect();
        let nents = units.len().div_ceil(15);
        let mut s = vec![0u8; (2 + nents) * 32];
        s[..32].copy_from_slice(&file[..32]);
        s[32..64].copy_from_slice(&stream[..32]);
        s[0] = T_FILE;
        s[1] = (1 + nents) as u8;
        s[32] = T_STREAM;
        s[32 + 3] = units.len() as u8;
        let h = self.name_hash(&units);
        put16(&mut s, 32 + 4, h);
        for k in 0..nents {
            let e = &mut s[64 + k * 32..96 + k * 32];
            e[0] = T_NAME;
            e[1] = 0;
            for j in 0..15 {
                let u = units.get(k * 15 + j).copied().unwrap_or(0);
                put16(e, 2 + j * 2, u);
            }
        }
        let ck = set_checksum(&s);
        put16(&mut s, 2, ck);
        Ok(s)
    }

    fn new_set(&self, name: &str, attr: u16, a: &Alloc, size: u64) -> Result<Vec<u8>, ()> {
        let mut file = [0u8; 32];
        put16(&mut file, 4, attr);
        let t = now_stamp();
        put32(&mut file, 8, t);
        put32(&mut file, 12, t);
        put32(&mut file, 16, t);
        let mut s = self.build_set(name, &file, &[0u8; 32])?;
        Self::set_stream(&mut s, a, size);
        let ck = set_checksum(&s);
        put16(&mut s, 2, ck);
        Ok(s)
    }

    /// One more cluster for a folder (zeroed; a subfolder's own entry set
    /// gets the new size).
    fn grow_dir(&mut self, d: &mut dyn Disk, dir: &mut Entry) -> Result<(), ()> {
        let cb = self.cb();
        if dir.set.is_empty() {
            // the root: always a FAT chain, its length is the chain's
            let runs = self.chain_runs(d, dir.first, u32::MAX)?;
            let last = runs.last().map(|r| r.0 + r.1 - 1).ok_or(())?;
            let (c, _) = self.find_free(d, 1, last + 1)?.ok_or(())?;
            self.zero_cluster(d, c)?;
            self.bm_set(d, c, 1, true)?;
            self.fat_put(d, c, EOC)?;
            self.fat_put(d, last, c)?;
            return Ok(());
        }
        let mut a = self.alloc_of(d, dir)?;
        self.alloc_grow(d, &mut a, 1, 1)?;
        self.zero_cluster(d, a.last)?;
        let size = dir.size + cb;
        self.patch_set(d, &dir.set, |s| Self::set_stream(s, &a, size))?;
        dir.size = size;
        dir.valid = size;
        dir.first = a.first;
        dir.nofat = a.nofat;
        Ok(())
    }

    /// Put an entry set into a folder (growing it when it has no room).
    fn create_in(&mut self, d: &mut dyn Disk, dir: &mut Entry, set: &[u8]) -> Result<Vec<(u64, usize)>, ()> {
        let n = set.len() / 32;
        for _ in 0..4 {
            let (buf, secs) = self.dir_load(d, dir)?;
            let slots = buf.len() / 32;
            let mut run = 0;
            for i in 0..slots {
                if buf[i * 32] & 0x80 == 0 {
                    run += 1;
                    if run == n {
                        let locs: Vec<(u64, usize)> = (i + 1 - n..=i).map(|j| (secs[j * 32 / SECTOR], (j * 32) % SECTOR)).collect();
                        self.write_set(d, &locs, set)?;
                        return Ok(locs);
                    }
                } else {
                    run = 0;
                }
            }
            self.grow_dir(d, dir)?;
        }
        Err(())
    }

    /// Clear InUse on every entry of a set.
    fn mark_deleted(&self, d: &mut dyn Disk, e: &Entry) -> Result<(), ()> {
        let mut s = self.read_set(d, &e.set)?;
        for k in 0..e.set.len() {
            s[k * 32] &= 0x7F;
        }
        self.write_set(d, &e.set, &s)
    }

    /// The folder `parent` names and the last component of `path`.
    fn parent_of(&mut self, d: &mut dyn Disk, path: &str) -> Result<(Entry, String), ()> {
        let (parent, name) = split_path(path);
        let pe = self.lookup(d, parent)?.ok_or(())?;
        if !pe.is_dir || !name_ok(name) {
            return Err(());
        }
        Ok((pe, String::from(name)))
    }

    // ── the operations `fs::media` uses ────────────────────────────────────

    /// Total and free space in bytes (one pass over the bitmap).
    pub fn space(&mut self, d: &mut dyn Disk) -> (u64, u64) {
        let mut used = 0u64;
        let mut left = self.clusters as u64;
        let mut buf = [0u8; SECTOR];
        for k in 0..self.bm_secs.len() {
            if left == 0 || rd(d, self.bm_secs[k], &mut buf).is_err() {
                break;
            }
            for &b in buf.iter() {
                if left == 0 {
                    break;
                }
                let bits = left.min(8);
                let m = if bits == 8 { 0xFF } else { (1u16 << bits) as u8 - 1 };
                used += (b & m).count_ones() as u64;
                left -= bits;
            }
        }
        let cb = self.cb();
        (self.clusters as u64 * cb, (self.clusters as u64 - used) * cb)
    }

    /// `(name, is_dir, size)` of a folder's entries.
    pub fn list(&mut self, d: &mut dyn Disk, path: &str) -> Result<Vec<(String, bool, usize)>, ()> {
        let e = self.lookup(d, path)?.ok_or(())?;
        if !e.is_dir {
            return Err(());
        }
        Ok(self.read_dir(d, &e)?.into_iter().map(|e| (e.name, e.is_dir, e.size as usize)).collect())
    }

    pub fn read_file(&mut self, d: &mut dyn Disk, path: &str) -> Result<Vec<u8>, ()> {
        let r = self.open_range(d, path)?;
        let n = usize::try_from(r.size).map_err(|_| ())?;
        let mut out = Vec::new();
        out.try_reserve_exact(n).map_err(|_| ())?;
        out.resize(n, 0);
        r.read_at(d, 0, &mut out)?;
        Ok(out)
    }

    /// Write `data` into newly allocated clusters (one run when possible).
    fn write_data(&mut self, d: &mut dyn Disk, data: &[u8]) -> Result<Alloc, ()> {
        let mut a = Alloc::default();
        let n = u32::try_from((data.len() as u64).div_ceil(self.cb())).map_err(|_| ())?;
        if n == 0 {
            return Ok(a);
        }
        if self.alloc_grow(d, &mut a, n, n).is_err() {
            let _ = self.alloc_free(d, &a);
            return Err(());
        }
        let r = (|| -> Result<(), ()> {
            let runs = self.runs_of(d, a.first, a.nofat, data.len() as u64)?;
            let mut pos = 0usize;
            for (s, cnt) in runs {
                let mut lba = self.clu_lba(s);
                let mut room = ((cnt as u64) << self.spc_shift) as usize * SECTOR;
                while room > 0 && pos < data.len() {
                    let whole = ((data.len() - pos) / SECTOR * SECTOR).min(room).min(IO_CHUNK);
                    if whole > 0 {
                        wr(d, lba, &data[pos..pos + whole])?;
                        pos += whole;
                        lba += (whole / SECTOR) as u64;
                        room -= whole;
                    } else {
                        let k = data.len() - pos;
                        let mut tail = [0u8; SECTOR];
                        tail[..k].copy_from_slice(&data[pos..]);
                        wr(d, lba, &tail)?;
                        pos += k;
                        room -= SECTOR;
                    }
                }
            }
            Ok(())
        })();
        if r.is_err() {
            let _ = self.alloc_free(d, &a);
            return Err(());
        }
        Ok(a)
    }

    /// Point an existing file's entry set at new contents (the old clusters
    /// are freed by the caller first).
    fn replace_contents(&mut self, d: &mut dyn Disk, e: &Entry, a: &Alloc, size: u64) -> Result<(), ()> {
        let t = now_stamp();
        self.patch_set(d, &e.set, |s| {
            Self::set_stream(s, a, size);
            put32(s, 12, t);
            put32(s, 16, t);
        })
    }

    /// Write (create or replace) a file.
    pub fn write_file(&mut self, d: &mut dyn Disk, path: &str, data: &[u8]) -> Result<(), ()> {
        self.begin(d)?;
        let r = self.write_file_in(d, path, data);
        self.end(d);
        r
    }

    fn write_file_in(&mut self, d: &mut dyn Disk, path: &str, data: &[u8]) -> Result<(), ()> {
        let (mut pe, name) = self.parent_of(d, path)?;
        match self.find_in(d, &pe, &name)? {
            Some(e) if e.is_dir => Err(()),
            Some(e) => {
                let old = self.alloc_of(d, &e)?;
                self.alloc_free(d, &old)?;
                let a = self.write_data(d, data)?;
                self.replace_contents(d, &e, &a, data.len() as u64)
            },
            None => {
                let a = self.write_data(d, data)?;
                let set = self.new_set(&name, ATTR_ARCHIVE, &a, data.len() as u64)?;
                if self.create_in(d, &mut pe, &set).is_err() {
                    let _ = self.alloc_free(d, &a);
                    return Err(());
                }
                Ok(())
            },
        }
    }

    pub fn mkdir(&mut self, d: &mut dyn Disk, path: &str) -> Result<(), ()> {
        self.begin(d)?;
        let r = (|| {
            let (mut pe, name) = self.parent_of(d, path)?;
            if self.find_in(d, &pe, &name)?.is_some() {
                return Err(());
            }
            let mut a = Alloc::default();
            self.alloc_grow(d, &mut a, 1, 1)?;
            let cb = self.cb();
            let made = self.zero_cluster(d, a.first).and_then(|_| self.new_set(&name, ATTR_DIR, &a, cb)).and_then(|set| self.create_in(d, &mut pe, &set));
            if made.is_err() {
                let _ = self.alloc_free(d, &a);
                return Err(());
            }
            Ok(())
        })();
        self.end(d);
        r
    }

    /// Delete a file or an empty folder.
    pub fn delete(&mut self, d: &mut dyn Disk, path: &str) -> Result<(), ()> {
        let e = self.lookup(d, path)?.ok_or(())?;
        if e.set.is_empty() {
            return Err(()); // the root
        }
        if e.is_dir && !self.read_dir(d, &e)?.is_empty() {
            return Err(());
        }
        self.begin(d)?;
        let r = (|| {
            let a = self.alloc_of(d, &e)?;
            self.mark_deleted(d, &e)?;
            self.alloc_free(d, &a)
        })();
        self.end(d);
        r
    }

    /// Rename or move within the volume (the data stays where it is).
    pub fn rename(&mut self, d: &mut dyn Disk, from: &str, to: &str) -> Result<(), ()> {
        let e = self.lookup(d, from)?.ok_or(())?;
        if e.set.is_empty() {
            return Err(());
        }
        let (f, t) = (from.trim_matches('/'), to.trim_matches('/'));
        if e.is_dir && t.len() > f.len() && t.as_bytes()[f.len()] == b'/' && self.name_eq(&t[..f.len()], f) {
            return Err(()); // into itself
        }
        let (mut pe, name) = self.parent_of(d, to)?;
        if self.find_in(d, &pe, &name)?.is_some() {
            return Err(());
        }
        self.begin(d)?;
        let r = (|| {
            let old = self.read_set(d, &e.set)?;
            let set = self.build_set(&name, &old[..32], &old[32..64])?;
            self.create_in(d, &mut pe, &set)?;
            // the old set may have moved nowhere: create_in only appends
            self.mark_deleted(d, &e)
        })();
        self.end(d);
        r
    }
}

// ── streamed writes of big files ───────────────────────────────────────────

/// A file being written piece by piece (`Volume::stream_*`): at most 64 KiB
/// is buffered; its clusters are placed in one run while the space after it
/// is free. The directory entry appears at `stream_finish`.
pub struct StreamFile {
    a: Alloc,
    size: u64,
    flushed: u64,
    buf: Vec<u8>,
    chunk: usize,
    started: bool,
}

impl Volume {
    pub fn stream_begin(&self) -> StreamFile {
        let chunk = (self.cb() as usize).min(IO_CHUNK);
        StreamFile { a: Alloc::default(), size: 0, flushed: 0, buf: Vec::with_capacity(chunk), chunk, started: false }
    }

    fn stream_start(&mut self, d: &mut dyn Disk, f: &mut StreamFile) -> Result<(), ()> {
        if !f.started {
            self.begin(d)?;
            self.streams += 1;
            f.started = true;
        }
        Ok(())
    }

    fn stream_flush(&mut self, d: &mut dyn Disk, f: &mut StreamFile) -> Result<(), ()> {
        if f.buf.is_empty() {
            return Ok(());
        }
        self.stream_start(d, f)?;
        let cb = self.cb();
        if f.flushed % cb == 0 {
            // a fresh file looks for a free run of 64 MiB to start in
            let prefer = ((64u64 << 20) / cb).max(1) as u32;
            self.alloc_grow(d, &mut f.a, 1, prefer)?;
        }
        let lba = self.clu_lba(f.a.last) + (f.flushed % cb) / SECTOR as u64;
        let len = f.buf.len().div_ceil(SECTOR) * SECTOR;
        f.buf.resize(len, 0);
        wr(d, lba, &f.buf)?;
        f.flushed += len as u64;
        f.buf.clear();
        Ok(())
    }

    /// Append bytes. Err when the disk is full.
    pub fn stream_write(&mut self, d: &mut dyn Disk, f: &mut StreamFile, mut data: &[u8]) -> Result<(), ()> {
        f.size += data.len() as u64;
        while !data.is_empty() {
            let take = (f.chunk - f.buf.len()).min(data.len());
            f.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if f.buf.len() == f.chunk {
                self.stream_flush(d, f)?;
            }
        }
        Ok(())
    }

    /// Write the last piece and create (or replace) `path`'s entry.
    pub fn stream_finish(&mut self, d: &mut dyn Disk, mut f: StreamFile, path: &str) -> Result<(), ()> {
        let r = (|| {
            self.stream_start(d, &mut f)?;
            self.stream_flush(d, &mut f)?;
            let (mut pe, name) = self.parent_of(d, path)?;
            match self.find_in(d, &pe, &name)? {
                Some(e) if e.is_dir => Err(()),
                Some(e) => {
                    let old = self.alloc_of(d, &e)?;
                    self.alloc_free(d, &old)?;
                    self.replace_contents(d, &e, &f.a, f.size)
                },
                None => {
                    let set = self.new_set(&name, ATTR_ARCHIVE, &f.a, f.size)?;
                    self.create_in(d, &mut pe, &set).map(|_| ())
                },
            }
        })();
        if r.is_err() {
            let _ = self.alloc_free(d, &f.a);
        }
        if f.started {
            self.streams -= 1;
        }
        self.end(d);
        r
    }

    /// Drop a half-written file: its clusters go back to the free pool.
    pub fn stream_abort(&mut self, d: &mut dyn Disk, f: StreamFile) {
        let _ = self.alloc_free(d, &f.a);
        if f.started {
            self.streams -= 1;
        }
        self.end(d);
    }
}

// ── random-access reads (Stable Diffusion checkpoints, model weights) ─────

impl Volume {
    /// Open `path` (a file) for [`RangeFile::read_at`]: its clusters as runs
    /// of consecutive sectors; bytes past ValidDataLength read as zeros.
    pub fn open_range(&mut self, d: &mut dyn Disk, path: &str) -> Result<RangeFile, ()> {
        let e = self.lookup(d, path)?.ok_or(())?;
        if e.is_dir {
            return Err(());
        }
        let runs = self.runs_of(d, e.first, e.nofat, e.size)?;
        let mut out: Vec<(u64, u32, u64)> = Vec::new();
        let mut off = 0u64;
        for (s, n) in runs {
            let lba = self.clu_lba(s);
            let bytes = (n as u64) << self.spc_shift << 9;
            if lba + (bytes >> 9) > u32::MAX as u64 {
                return Err(());
            }
            match out.last_mut() {
                Some(r) if r.1 as u64 + (r.2 >> 9) == lba => r.2 += bytes,
                _ => out.push((off, lba as u32, bytes)),
            }
            off += bytes;
        }
        Ok(RangeFile::from_runs(e.size, e.valid.min(e.size), out))
    }
}
