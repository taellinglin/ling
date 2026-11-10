//! NVMe SSDs (NVM Express 1.x, PCI class 01:08:02) -- the only disk most
//! current PCs have. Polled, no interrupts: one admin queue pair and one I/O
//! queue pair of 16 entries, one 4 KiB bounce page, PRP1-only transfers.
//!
//! **What LingOS may write.** An NVMe drive in a PC almost always holds
//! another operating system. lingfs therefore never takes a whole NVMe disk:
//! `fs::blockdev` looks for a GPT partition whose *type* is LingOS's own
//! (`LINGOS_PART_TYPE`, below) and maps lingfs into it. This driver enforces
//! that independently: `write_512` refuses any sector outside the window
//! `set_write_window` opened (the LingOS partition), so a bug elsewhere can
//! not reach Windows/Linux data. With no window open, nothing is written.
//!
//! Making the partition (from Linux, in free space on the drive):
//! `sudo sgdisk -n 0:0:+256M -t 0:8F6E4C2A-1D3B-4A57-9C8E-4C494E474653 -c 0:LingOS /dev/nvme0n1`
//! (at least 33 MiB: lingfs keeps two ~16 MiB slots).

use crate::arch::pci::{self, Location};
use crate::arch::timer;
use alloc::format;
use alloc::string::String;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

/// GPT partition type GUID 8F6E4C2A-1D3B-4A57-9C8E-4C494E474653 in its on-disk
/// byte order (the first three fields little-endian). The last six bytes spell
/// "LINGFS".
pub const LINGOS_PART_TYPE: [u8; 16] =
    [0x2A, 0x4C, 0x6E, 0x8F, 0x3B, 0x1D, 0x57, 0x4A, 0x9C, 0x8E, 0x4C, 0x49, 0x4E, 0x47, 0x46, 0x53];
pub const LINGOS_PART_TYPE_STR: &str = "8F6E4C2A-1D3B-4A57-9C8E-4C494E474653";

// controller registers
const REG_CAP: usize = 0x00;
const REG_VS: usize = 0x08;
const REG_INTMS: usize = 0x0C;
const REG_CC: usize = 0x14;
const REG_CSTS: usize = 0x1C;
const REG_AQA: usize = 0x24;
const REG_ASQ: usize = 0x28;
const REG_ACQ: usize = 0x30;

const QDEPTH: u16 = 16;
const CMD_TIMEOUT_US: u64 = 3_000_000;

struct Queue {
    sq: u64,
    cq: u64,
    qid: u16,
    tail: u16,
    head: u16,
    phase: bool,
}

struct Ctrl {
    loc: Location,
    base: u64,
    dstrd: u32,
    admin: Queue,
    io: Queue,
    bounce: u64,
    cid: u16,
    nsid: u32,
    /// namespace block size (512 or 4096) and size in those blocks
    lba_size: u32,
    lba_count: u64,
    model: String,
}

static mut CTRL: Option<Ctrl> = None;
static mut PROBED: bool = false;
static mut NOTE: String = String::new();
/// Sectors (512-byte units) that may be written: [start, end) ranges.
static mut WIN: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();

fn in_window(s: u64, n: u64) -> bool {
    unsafe { (*(&raw const WIN)).iter().any(|&(lo, hi)| s >= lo && s + n <= hi) }
}

fn ctrl() -> Option<&'static mut Ctrl> {
    unsafe { (*(&raw mut CTRL)).as_mut() }
}

fn note(s: String) {
    crate::console_write(b"nvme: ");
    crate::console_write(s.as_bytes());
    crate::console_write(b"\n");
    unsafe { *(&raw mut NOTE) = s };
}

/// One line about the drive / why there is none (for `df`, `nvme`).
pub fn status() -> String {
    probe();
    match ctrl() {
        Some(c) => format!(
            "{} at {:02x}:{:02x}.{}, {} MiB in {}-byte blocks",
            c.model,
            c.loc.bus,
            c.loc.device,
            c.loc.function,
            c.lba_count * c.lba_size as u64 / (1024 * 1024),
            c.lba_size
        ),
        None => unsafe { (*(&raw const NOTE)).clone() },
    }
}

pub fn present() -> bool {
    probe();
    ctrl().is_some()
}

/// Total 512-byte sectors on the namespace.
pub fn sectors_512() -> u64 {
    ctrl().map(|c| c.lba_count * (c.lba_size as u64 / 512)).unwrap_or(0)
}

/// Allow writes to 512-byte sectors [start, end) only (the LingOS partition).
pub fn set_write_window(start: u64, end: u64) {
    unsafe { *(&raw mut WIN) = alloc::vec![(start, end)] };
}

/// Also allow writes to [start, end) (the LingOS Data partition).
pub fn add_write_window(start: u64, end: u64) {
    unsafe { (*(&raw mut WIN)).push((start, end)) };
}

/// The open windows (the disk installer puts them back when it is done).
pub fn write_windows() -> alloc::vec::Vec<(u64, u64)> {
    unsafe { (*(&raw const WIN)).clone() }
}

pub fn set_write_windows(w: alloc::vec::Vec<(u64, u64)>) {
    unsafe { *(&raw mut WIN) = w };
}

/// The drive's model line (for the disk installer's listing).
pub fn model() -> String {
    ctrl().map(|c| c.model.clone()).unwrap_or_default()
}

/// 2^order zeroed frames below 4 GiB (identity-mapped, DMA-safe).
fn dma_page(order: usize) -> Option<u64> {
    let p = crate::mm::frame::alloc_frames(order)?;
    let len = 4096u64 << order;
    if p == 0 || p + len > 1 << 32 {
        return None;
    }
    unsafe { core::ptr::write_bytes(p as *mut u8, 0, len as usize) };
    Some(p)
}

impl Ctrl {
    fn r32(&self, off: usize) -> u32 {
        unsafe { read_volatile((self.base + off as u64) as *const u32) }
    }
    fn w32(&self, off: usize, v: u32) {
        unsafe { write_volatile((self.base + off as u64) as *mut u32, v) }
    }
    fn r64(&self, off: usize) -> u64 {
        self.r32(off) as u64 | (self.r32(off + 4) as u64) << 32
    }
    fn w64(&self, off: usize, v: u64) {
        self.w32(off, v as u32);
        self.w32(off + 4, (v >> 32) as u32);
    }
    fn sq_doorbell(&self, qid: u16) -> usize {
        0x1000 + (2 * qid as usize) * (4 << self.dstrd)
    }
    fn cq_doorbell(&self, qid: u16) -> usize {
        0x1000 + (2 * qid as usize + 1) * (4 << self.dstrd)
    }

    /// Submit one command on `admin` or the I/O queue and poll for its
    /// completion. `cdw` = CDW10..CDW15. Err(status) on an error status or timeout.
    fn submit(&mut self, admin: bool, opcode: u8, nsid: u32, prp1: u64, cdw: [u32; 6]) -> Result<u32, String> {
        self.cid = self.cid.wrapping_add(1);
        let cid = self.cid;
        let (sq, qid, tail) = {
            let q = if admin { &self.admin } else { &self.io };
            (q.sq, q.qid, q.tail)
        };
        let e = sq + tail as u64 * 64;
        unsafe {
            core::ptr::write_bytes(e as *mut u8, 0, 64);
            write_volatile(e as *mut u32, opcode as u32 | (cid as u32) << 16);
            write_volatile((e + 4) as *mut u32, nsid);
            write_volatile((e + 24) as *mut u64, prp1);
            for (i, v) in cdw.iter().enumerate() {
                write_volatile((e + 40 + i as u64 * 4) as *mut u32, *v);
            }
        }
        fence(Ordering::SeqCst);
        let ntail = (tail + 1) % QDEPTH;
        if admin {
            self.admin.tail = ntail;
        } else {
            self.io.tail = ntail;
        }
        self.w32(self.sq_doorbell(qid), ntail as u32);
        let end = timer::now_us() + CMD_TIMEOUT_US;
        loop {
            let (cq, head, phase) = {
                let q = if admin { &self.admin } else { &self.io };
                (q.cq, q.head, q.phase)
            };
            let c = cq + head as u64 * 16;
            let dw3 = unsafe { read_volatile((c + 12) as *const u32) };
            if ((dw3 >> 16) & 1 == 1) == phase {
                let dw0 = unsafe { read_volatile(c as *const u32) };
                let nhead = (head + 1) % QDEPTH;
                {
                    let q = if admin { &mut self.admin } else { &mut self.io };
                    q.head = nhead;
                    if nhead == 0 {
                        q.phase = !q.phase;
                    }
                }
                self.w32(self.cq_doorbell(qid), nhead as u32);
                if dw3 & 0xFFFF != cid as u32 {
                    continue; // a stale completion; ours is next
                }
                let status = (dw3 >> 17) & 0x7FFF;
                if status != 0 {
                    return Err(format!("command 0x{opcode:02x} failed, status 0x{status:x}"));
                }
                return Ok(dw0);
            }
            if timer::now_us() >= end {
                return Err(format!("command 0x{opcode:02x} timed out (CSTS 0x{:x})", self.r32(REG_CSTS)));
            }
            core::hint::spin_loop();
        }
    }
}

fn bar0_64(loc: &Location) -> u64 {
    let lo = loc.read32(0x10);
    if lo & 1 != 0 {
        return 0;
    }
    let hi = if (lo >> 1) & 3 == 2 { loc.read32(0x14) } else { 0 };
    (hi as u64) << 32 | (lo & 0xFFFF_FFF0) as u64
}

fn wait_ready(c: &Ctrl, want: bool, timeout_us: u64) -> bool {
    let end = timer::now_us() + timeout_us;
    loop {
        let csts = c.r32(REG_CSTS);
        if csts & 2 != 0 && want {
            return false; // controller fatal status
        }
        if (csts & 1 == 1) == want {
            return true;
        }
        if timer::now_us() >= end {
            return false;
        }
        timer::delay_us(100);
    }
}

/// Find and start the first NVMe controller (once).
pub fn probe() {
    if unsafe { PROBED } {
        return;
    }
    unsafe { PROBED = true };
    match start() {
        Ok(c) => {
            let s = format!(
                "{} ({} MiB, {}-byte blocks)",
                c.model,
                c.lba_count * c.lba_size as u64 / (1024 * 1024),
                c.lba_size
            );
            unsafe { *(&raw mut CTRL) = Some(c) };
            note(s);
        },
        Err(e) => note(e),
    }
}

fn start() -> Result<Ctrl, String> {
    let loc = pci::find_all_class(0x01, 0x08)
        .into_iter()
        .find(|l| (l.read32(0x08) >> 8) & 0xFF == 0x02)
        .ok_or_else(|| String::from("no NVMe drive"))?;
    let phys = bar0_64(&loc);
    if phys == 0 {
        return Err(String::from("NVMe BAR0 is not a memory BAR"));
    }
    // memory decode + bus master, legacy interrupts off (we poll)
    let cmd = loc.read32(0x04) & 0xFFFF;
    pci::write32(loc.bus, loc.device, loc.function, 0x04, cmd | 0x0002 | 0x0004 | 0x0400);
    let base = crate::arch::paging::map_mmio(phys, 0x4000).ok_or_else(|| format!("NVMe BAR0 0x{phys:x} could not be mapped"))?;
    let mut c = Ctrl {
        loc,
        base,
        dstrd: 0,
        admin: Queue { sq: 0, cq: 0, qid: 0, tail: 0, head: 0, phase: true },
        io: Queue { sq: 0, cq: 0, qid: 1, tail: 0, head: 0, phase: true },
        bounce: 0,
        cid: 0,
        nsid: 1,
        lba_size: 512,
        lba_count: 0,
        model: String::new(),
    };
    let cap = c.r64(REG_CAP);
    if cap == u64::MAX {
        return Err(String::from("NVMe registers read all-ones (device asleep?)"));
    }
    c.dstrd = ((cap >> 32) & 0xF) as u32;
    let to_us = ((cap >> 24) & 0xFF).max(1) * 500_000;
    // MQES is zero-based: 0xFFFF means 65536 entries (the Z590's Micron NVMe
    // reports exactly that), so widen before adding -- in u16 it wrapped to 0.
    let mqes = (cap & 0xFFFF) as u32 + 1;
    if mqes < QDEPTH as u32 || (cap >> 37) & 1 == 0 || (cap >> 48) & 0xF != 0 {
        return Err(format!("NVMe controller unsupported (CAP 0x{cap:x})"));
    }
    // reset: disable, wait not-ready
    let cc = c.r32(REG_CC);
    if cc & 1 != 0 {
        c.w32(REG_CC, cc & !1);
    }
    if !wait_ready(&c, false, to_us) {
        return Err(format!("NVMe did not stop (CSTS 0x{:x})", c.r32(REG_CSTS)));
    }
    let oom = || String::from("NVMe: no memory below 4 GiB for the queues");
    c.admin.sq = dma_page(0).ok_or_else(oom)?;
    c.admin.cq = dma_page(0).ok_or_else(oom)?;
    c.io.sq = dma_page(0).ok_or_else(oom)?;
    c.io.cq = dma_page(0).ok_or_else(oom)?;
    c.bounce = dma_page(0).ok_or_else(oom)?;
    c.w32(REG_INTMS, 0xFFFF_FFFF);
    c.w32(REG_AQA, ((QDEPTH as u32 - 1) << 16) | (QDEPTH as u32 - 1));
    c.w64(REG_ASQ, c.admin.sq);
    c.w64(REG_ACQ, c.admin.cq);
    // NVM command set, 4 KiB pages, round robin, SQE 64 B (6), CQE 16 B (4)
    c.w32(REG_CC, (6 << 16) | (4 << 20) | 1);
    if !wait_ready(&c, true, to_us) {
        return Err(format!("NVMe did not become ready (CSTS 0x{:x})", c.r32(REG_CSTS)));
    }
    // Identify controller: model name
    c.submit(true, 0x06, 0, c.bounce, [1, 0, 0, 0, 0, 0])?;
    let id = unsafe { core::slice::from_raw_parts(c.bounce as *const u8, 4096) };
    c.model = String::from_utf8_lossy(&id[24..64]).trim().into();
    let vs = c.r32(REG_VS);
    c.model = format!("{} (NVMe {}.{})", c.model, vs >> 16, (vs >> 8) & 0xFF);
    // Identify namespace 1: size, block format
    c.submit(true, 0x06, 1, c.bounce, [0, 0, 0, 0, 0, 0])?;
    let ns = unsafe { core::slice::from_raw_parts(c.bounce as *const u8, 4096) };
    c.lba_count = u64::from_le_bytes(ns[0..8].try_into().unwrap());
    let fmt = (ns[26] & 0xF) as usize;
    let lbaf = u32::from_le_bytes(ns[128 + fmt * 4..132 + fmt * 4].try_into().unwrap());
    let lbads = (lbaf >> 16) & 0xFF;
    if c.lba_count == 0 || !(9..=12).contains(&lbads) {
        return Err(format!("NVMe namespace 1 unusable (size {}, block 2^{lbads})", c.lba_count));
    }
    c.lba_size = 1 << lbads;
    // I/O queue pair 1 (polled: no interrupt enable)
    let (iocq, iosq) = (c.io.cq, c.io.sq);
    c.submit(true, 0x05, 0, iocq, [((QDEPTH as u32 - 1) << 16) | 1, 1, 0, 0, 0, 0])?;
    c.submit(true, 0x01, 0, iosq, [((QDEPTH as u32 - 1) << 16) | 1, (1 << 16) | 1, 0, 0, 0, 0])?;
    Ok(c)
}

/// Read one namespace block into the bounce page.
fn read_block(c: &mut Ctrl, lba: u64) -> Result<(), ()> {
    if lba >= c.lba_count {
        return Err(());
    }
    let b = c.bounce;
    c.submit(false, 0x02, c.nsid, b, [lba as u32, (lba >> 32) as u32, 0, 0, 0, 0]).map_err(|_| ())?;
    Ok(())
}

/// Write the bounce page to one namespace block (FUA: on the media when it returns).
fn write_block(c: &mut Ctrl, lba: u64) -> Result<(), ()> {
    if lba >= c.lba_count {
        return Err(());
    }
    let b = c.bounce;
    c.submit(false, 0x01, c.nsid, b, [lba as u32, (lba >> 32) as u32, 1 << 30, 0, 0, 0]).map_err(|_| ())?;
    Ok(())
}

/// Read 512-byte sector `s` (any sector; reading is harmless).
pub fn read_512(s: u64, buf: &mut [u8; 512]) -> Result<(), ()> {
    let c = ctrl().ok_or(())?;
    let per = c.lba_size as u64 / 512;
    read_block(c, s / per)?;
    let off = (s % per) as usize * 512;
    let src = unsafe { core::slice::from_raw_parts((c.bounce + off as u64) as *const u8, 512) };
    buf.copy_from_slice(src);
    Ok(())
}

/// Write 512-byte sector `s` -- only inside the window (the LingOS partition).
pub fn write_512(s: u64, buf: &[u8; 512]) -> Result<(), ()> {
    if !in_window(s, 1) {
        return Err(());
    }
    let c = ctrl().ok_or(())?;
    let per = c.lba_size as u64 / 512;
    // a whole block can't straddle the window edge: the partition is block-aligned
    if per > 1 {
        read_block(c, s / per)?; // read-modify-write a 4 KiB block
    }
    let off = (s % per) as usize * 512;
    let dst = unsafe { core::slice::from_raw_parts_mut((c.bounce + off as u64) as *mut u8, 512) };
    dst.copy_from_slice(buf);
    write_block(c, s / per)
}

/// Read `buf.len() / 512` sectors from `s` (a 4 KiB page per command when
/// they line up, sector by sector otherwise).
pub fn read_sectors(mut s: u64, buf: &mut [u8]) -> Result<(), ()> {
    if buf.len() % 512 != 0 {
        return Err(());
    }
    let mut done = 0;
    while done < buf.len() {
        let c = ctrl().ok_or(())?;
        let per = c.lba_size as u64 / 512;
        let left = buf.len() - done;
        if per == 1 && left >= 4096 && s + 8 <= c.lba_count {
            // 8 sectors through the 4 KiB bounce page in one command
            let b = c.bounce;
            c.submit(false, 0x02, c.nsid, b, [s as u32, (s >> 32) as u32, 7, 0, 0, 0]).map_err(|_| ())?;
            let src = unsafe { core::slice::from_raw_parts(b as *const u8, 4096) };
            buf[done..done + 4096].copy_from_slice(src);
            s += 8;
            done += 4096;
        } else if per == 8 && s % 8 == 0 && left >= 4096 {
            read_block(c, s / 8)?;
            let src = unsafe { core::slice::from_raw_parts(c.bounce as *const u8, 4096) };
            buf[done..done + 4096].copy_from_slice(src);
            s += 8;
            done += 4096;
        } else {
            let mut one = [0u8; 512];
            read_512(s, &mut one)?;
            buf[done..done + 512].copy_from_slice(&one);
            s += 1;
            done += 512;
        }
    }
    Ok(())
}

/// Write `buf.len() / 512` sectors at `s` -- all inside one write window.
pub fn write_sectors(mut s: u64, buf: &[u8]) -> Result<(), ()> {
    if buf.len() % 512 != 0 || !in_window(s, (buf.len() / 512) as u64) {
        return Err(());
    }
    let mut done = 0;
    while done < buf.len() {
        let c = ctrl().ok_or(())?;
        let per = c.lba_size as u64 / 512;
        let left = buf.len() - done;
        let fits = if per == 1 { s + 8 <= c.lba_count } else { s / 8 < c.lba_count };
        if (per == 1 || (per == 8 && s % 8 == 0)) && left >= 4096 && fits {
            let dst = unsafe { core::slice::from_raw_parts_mut(c.bounce as *mut u8, 4096) };
            dst.copy_from_slice(&buf[done..done + 4096]);
            let (lba, nlb) = if per == 1 { (s, 7u32) } else { (s / 8, 0u32) };
            let b = c.bounce;
            c.submit(false, 0x01, c.nsid, b, [lba as u32, (lba >> 32) as u32, nlb | 1 << 30, 0, 0, 0]).map_err(|_| ())?;
            s += 8;
            done += 4096;
        } else {
            let mut one = [0u8; 512];
            one.copy_from_slice(&buf[done..done + 512]);
            write_512(s, &one)?;
            s += 1;
            done += 512;
        }
    }
    Ok(())
}

/// Bytes `[off, off+len)` of the namespace (GPT parsing).
fn read_bytes(off: u64, out: &mut [u8]) -> Result<(), ()> {
    let mut s = [0u8; 512];
    let mut done = 0usize;
    while done < out.len() {
        let pos = off + done as u64;
        read_512(pos / 512, &mut s)?;
        let o = (pos % 512) as usize;
        let n = (512 - o).min(out.len() - done);
        out[done..done + n].copy_from_slice(&s[o..o + n]);
        done += n;
    }
    Ok(())
}

/// `nvme`: the drive, the LingOS partition (or why there is none), where lingfs is.
pub fn command(_arg: &str) -> String {
    let mut s = format!("nvme: {}\n", status());
    if present() {
        match find_lingos_partition() {
            Ok((first, count, name)) => s.push_str(&format!("  LingOS partition \"{name}\": {} MiB at sector {first}\n", count / 2048)),
            Err(e) => {
                s.push_str(&format!("  {e}\n"));
                s.push_str(&format!(
                    "  to give LingOS storage, from Linux make a partition in free space:\n  sudo sgdisk -n 0:0:+256M -t 0:{LINGOS_PART_TYPE_STR} -c 0:LingOS /dev/nvme0n1\n"
                ));
            },
        }
    }
    let dev = crate::fs::blockdev::active_driver_name();
    if crate::fs::blockdev::is_ram() {
        s.push_str(&format!("  lingfs: {dev} (RAM -- {})\n", crate::fs::blockdev::ram_reason()));
    } else {
        s.push_str(&format!("  lingfs: {dev}\n"));
    }
    s
}

/// The LingOS partition: (first 512-byte sector, sector count, its GPT name).
/// Err explains why there is none (no GPT, no partition of LingOS's type).
pub fn find_lingos_partition() -> Result<(u64, u64, String), String> {
    let c = ctrl().ok_or_else(|| String::from("no NVMe drive"))?;
    let bs = c.lba_size as u64;
    let total = c.lba_count;
    let mut hdr = [0u8; 92];
    read_bytes(bs, &mut hdr).map_err(|_| String::from("could not read the partition table"))?;
    if &hdr[0..8] != b"EFI PART" {
        return Err(String::from("the drive has no GPT partition table"));
    }
    let ent_lba = u64::from_le_bytes(hdr[72..80].try_into().unwrap());
    let n = u32::from_le_bytes(hdr[80..84].try_into().unwrap()).min(256) as u64;
    let esz = u32::from_le_bytes(hdr[84..88].try_into().unwrap()) as u64;
    if esz < 128 || esz > 1024 {
        return Err(format!("odd GPT entry size {esz}"));
    }
    let mut e = [0u8; 128];
    for i in 0..n {
        read_bytes(ent_lba * bs + i * esz, &mut e).map_err(|_| String::from("could not read the partition entries"))?;
        if e[0..16] != LINGOS_PART_TYPE {
            continue;
        }
        let first = u64::from_le_bytes(e[32..40].try_into().unwrap());
        let last = u64::from_le_bytes(e[40..48].try_into().unwrap());
        if first == 0 || last < first || last >= total {
            return Err(format!("the LingOS partition's range {first}..{last} is invalid"));
        }
        let name: String = char::decode_utf16(e[56..128].chunks(2).map(|p| u16::from_le_bytes([p[0], p[1]])).take_while(|&u| u != 0))
            .map(|r| r.unwrap_or('?'))
            .collect();
        let per = bs / 512;
        return Ok((first * per, (last - first + 1) * per, name));
    }
    Err(format!("no LingOS partition (GPT type {LINGOS_PART_TYPE_STR})"))
}
