//! Every whole disk this kernel can drive (x86): SATA disks on the AHCI
//! controller, the NVMe drive, the USB stick -- one sector API over them, their
//! partition tables (GPT and MBR), and what each partition seems to hold. The
//! disk installer (`drivers::installer`) and `fs::blockdev`'s choice of where
//! lingfs lives both work from here. LBAs are 512-byte sectors throughout;
//! a GPT on a 4K-sector NVMe drive is read in its own block size and
//! converted.
//!
//! Writing goes through each driver's write windows (`ahci::add_write_window`,
//! `nvme::add_write_window`): nothing here can write where no window is open.

use crate::drivers::{ahci, nvme};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiskId {
    /// the SATA disk on this AHCI port
    Sata(u8),
    Nvme,
    /// the USB stick (`drivers::usbmsd`; one at a time)
    Usb,
}

impl DiskId {
    pub fn kind(&self) -> &'static str {
        match self {
            DiskId::Sata(_) => "SATA",
            DiskId::Nvme => "NVMe",
            DiskId::Usb => "USB",
        }
    }
}

pub struct DiskInfo {
    pub id: DiskId,
    pub model: String,
    /// size in 512-byte sectors
    pub sectors: u64,
    /// logical block size (4096 on a 4K-native NVMe drive)
    pub block: u32,
}

/// Every disk present, SATA ports in order, then NVMe, then the USB stick.
pub fn list() -> Vec<DiskInfo> {
    let mut out = Vec::new();
    for (port, model, sectors) in ahci::disks() {
        out.push(DiskInfo { id: DiskId::Sata(port as u8), model, sectors, block: 512 });
    }
    if nvme::present() {
        let s = nvme::sectors_512();
        let model = nvme::model();
        let block = if nvme::status().contains("4096-byte") { 4096 } else { 512 };
        out.push(DiskInfo { id: DiskId::Nvme, model, sectors: s, block });
    }
    if crate::drivers::usbmsd::ready() {
        let model = crate::drivers::usbmsd::model();
        out.push(DiskInfo { id: DiskId::Usb, model, sectors: crate::drivers::usbmsd::sectors() as u64, block: 512 });
    }
    out
}

pub fn sectors(id: DiskId) -> u64 {
    match id {
        DiskId::Sata(p) => ahci::disks().iter().find(|d| d.0 == p as usize).map(|d| d.2).unwrap_or(0),
        DiskId::Nvme => nvme::sectors_512(),
        DiskId::Usb => crate::drivers::usbmsd::sectors() as u64,
    }
}

/// Read `buf.len() / 512` sectors at `lba`.
pub fn read(id: DiskId, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
    match id {
        DiskId::Sata(p) => ahci::read_port(p as usize, lba, buf),
        DiskId::Nvme => nvme::read_sectors(lba, buf),
        DiskId::Usb => {
            if lba + (buf.len() / 512) as u64 > u32::MAX as u64 {
                return Err(());
            }
            crate::drivers::usbmsd::read_sectors(lba as u32, buf)
        },
    }
}

/// Write sectors at `lba` (refused by the driver outside its write windows;
/// the USB stick is never written through here).
pub fn write(id: DiskId, lba: u64, buf: &[u8]) -> Result<(), ()> {
    match id {
        DiskId::Sata(p) => ahci::write_port(p as usize, lba, buf),
        DiskId::Nvme => nvme::write_sectors(lba, buf),
        DiskId::Usb => Err(()),
    }
}

/// Everything written so far is on the media (NVMe writes are FUA already).
pub fn flush(id: DiskId) -> Result<(), ()> {
    match id {
        DiskId::Sata(p) => ahci::flush_port(p as usize),
        _ => Ok(()),
    }
}

/// `fat::Disk` over a whole disk (absolute LBAs), for mounting a partition.
pub struct DiskIo(pub DiskId);

impl crate::fs::fat::Disk for DiskIo {
    fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), ()> {
        read(self.0, lba as u64, buf)
    }
    fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), ()> {
        write(self.0, lba as u64, buf)
    }
}

// ── partition tables ────────────────────────────────────────────────────────

pub const TYPE_ESP: [u8; 16] = guid(0xC12A7328, 0xF81F, 0x11D2, [0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B]);
pub const TYPE_BASIC_DATA: [u8; 16] = guid(0xEBD0A0A2, 0xB9E5, 0x4433, [0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7]);
pub const TYPE_MS_RESERVED: [u8; 16] = guid(0xE3C9E316, 0x0B5C, 0x4DB8, [0x81, 0x7D, 0xF9, 0x2D, 0xF0, 0x02, 0x15, 0xAE]);
pub const TYPE_WIN_RECOVERY: [u8; 16] = guid(0xDE94BBA4, 0x06D1, 0x4D40, [0xA1, 0x6A, 0xBF, 0xD5, 0x01, 0x79, 0xD6, 0xAC]);
pub const TYPE_LINUX: [u8; 16] = guid(0x0FC63DAF, 0x8483, 0x4772, [0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D, 0xE4]);
pub const TYPE_LINUX_SWAP: [u8; 16] = guid(0x0657FD6D, 0xA4AB, 0x43C4, [0x84, 0xE5, 0x09, 0x33, 0xC8, 0x4B, 0x4F, 0x4F]);
pub const TYPE_LINUX_LVM: [u8; 16] = guid(0xE6D6D379, 0xF507, 0x44C2, [0xA2, 0x3C, 0x23, 0x8F, 0x2A, 0x3D, 0xF9, 0x28]);
pub const TYPE_BIOS_BOOT: [u8; 16] = guid(0x21686148, 0x6449, 0x6E6F, [0x74, 0x4E, 0x65, 0x65, 0x64, 0x45, 0x46, 0x49]);
pub const TYPE_LINGOS: [u8; 16] = crate::drivers::nvme::LINGOS_PART_TYPE;

/// A GUID in its on-disk byte order (first three fields little-endian).
pub const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let a = a.to_le_bytes();
    let b = b.to_le_bytes();
    let c = c.to_le_bytes();
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]
}

pub fn guid_str(g: &[u8; 16]) -> String {
    format!(
        "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        u32::from_le_bytes([g[0], g[1], g[2], g[3]]),
        u16::from_le_bytes([g[4], g[5]]),
        u16::from_le_bytes([g[6], g[7]]),
        g[8],
        g[9],
        g[10],
        g[11],
        g[12],
        g[13],
        g[14],
        g[15]
    )
}

#[derive(Clone)]
pub struct Part {
    /// 1-based partition number
    pub number: u32,
    /// GPT type GUID (zero for MBR partitions)
    pub type_guid: [u8; 16],
    /// GPT unique partition GUID (zero for MBR)
    pub uniq: [u8; 16],
    /// MBR partition type byte (0 for GPT)
    pub mbr_type: u8,
    /// first 512-byte sector and count
    pub first: u64,
    pub count: u64,
    pub name: String,
}

pub enum Table {
    /// no partition table (blank, or a filesystem on the whole disk)
    None,
    Mbr { signature: u32, parts: Vec<Part> },
    Gpt { disk_guid: [u8; 16], parts: Vec<Part> },
}

fn block_of(id: DiskId) -> u64 {
    if id == DiskId::Nvme && nvme::status().contains("4096-byte") {
        4096
    } else {
        512
    }
}

fn read_bytes(id: DiskId, off: u64, out: &mut [u8]) -> Result<(), ()> {
    let first = off / 512;
    let last = (off + out.len() as u64).div_ceil(512);
    let mut buf = vec![0u8; ((last - first) * 512) as usize];
    read(id, first, &mut buf)?;
    let o = (off % 512) as usize;
    out.copy_from_slice(&buf[o..o + out.len()]);
    Ok(())
}

/// The disk's partition table.
pub fn table(id: DiskId) -> Result<Table, String> {
    let mut s0 = [0u8; 512];
    read(id, 0, &mut s0).map_err(|_| String::from("cannot read the disk"))?;
    let bs = block_of(id);
    let per = bs / 512;
    let mut hdr = [0u8; 92];
    if read_bytes(id, bs, &mut hdr).is_ok() && &hdr[0..8] == b"EFI PART" {
        let ent_lba = u64::from_le_bytes(hdr[72..80].try_into().unwrap());
        let n = u32::from_le_bytes(hdr[80..84].try_into().unwrap()).min(256) as u64;
        let esz = u32::from_le_bytes(hdr[84..88].try_into().unwrap()) as u64;
        if !(128..=1024).contains(&esz) {
            return Err(format!("odd GPT entry size {esz}"));
        }
        let mut ents = vec![0u8; (n * esz) as usize];
        read_bytes(id, ent_lba * bs, &mut ents).map_err(|_| String::from("cannot read the GPT entries"))?;
        let mut parts = Vec::new();
        for i in 0..n as usize {
            let e = &ents[i * esz as usize..i * esz as usize + 128];
            if e[0..16].iter().all(|&b| b == 0) {
                continue;
            }
            let first = u64::from_le_bytes(e[32..40].try_into().unwrap());
            let last = u64::from_le_bytes(e[40..48].try_into().unwrap());
            if last < first {
                continue;
            }
            let name: String = char::decode_utf16(e[56..128].chunks(2).map(|p| u16::from_le_bytes([p[0], p[1]])).take_while(|&u| u != 0))
                .map(|r| r.unwrap_or('?'))
                .collect();
            parts.push(Part {
                number: i as u32 + 1,
                type_guid: e[0..16].try_into().unwrap(),
                uniq: e[16..32].try_into().unwrap(),
                mbr_type: 0,
                first: first * per,
                count: (last - first + 1) * per,
                name,
            });
        }
        return Ok(Table::Gpt { disk_guid: hdr[56..72].try_into().unwrap(), parts });
    }
    if s0[510] == 0x55 && s0[511] == 0xAA {
        // a FAT/NTFS boot sector on the whole disk is not an MBR
        let bpb = (s0[0] == 0xEB || s0[0] == 0xE9) && u16::from_le_bytes([s0[11], s0[12]]) == 512 && s0[13] != 0;
        let fs_sig = &s0[3..11] == b"NTFS    " || &s0[3..11] == b"EXFAT   " || &s0[82..87] == b"FAT32" || &s0[54..57] == b"FAT";
        if bpb && fs_sig {
            return Ok(Table::None);
        }
        let mut parts = Vec::new();
        let mut sane = true;
        for i in 0..4 {
            let e = &s0[446 + i * 16..462 + i * 16];
            if e[0] != 0 && e[0] != 0x80 {
                sane = false;
            }
            if e[4] == 0 {
                continue;
            }
            let first = u32::from_le_bytes(e[8..12].try_into().unwrap()) as u64;
            let count = u32::from_le_bytes(e[12..16].try_into().unwrap()) as u64;
            if first == 0 || count == 0 {
                continue;
            }
            parts.push(Part { number: i as u32 + 1, type_guid: [0; 16], uniq: [0; 16], mbr_type: e[4], first, count, name: String::new() });
        }
        if sane && !parts.is_empty() {
            return Ok(Table::Mbr { signature: u32::from_le_bytes(s0[440..444].try_into().unwrap()), parts });
        }
    }
    Ok(Table::None)
}

/// The LingOS partition (GPT type `TYPE_LINGOS`) on `id`: (first, count, name).
pub fn lingos_partition(id: DiskId) -> Option<(u64, u64, String)> {
    match table(id).ok()? {
        Table::Gpt { parts, .. } => parts.into_iter().find(|p| p.type_guid == TYPE_LINGOS).map(|p| (p.first, p.count, p.name)),
        _ => None,
    }
}

/// The "LingOS Data" FAT partition next to the LingOS partition on `id`.
pub fn data_partition(id: DiskId) -> Option<(u64, u64)> {
    match table(id).ok()? {
        Table::Gpt { parts, .. } => parts
            .into_iter()
            .find(|p| p.type_guid == TYPE_BASIC_DATA && is_data_name(&p.name))
            .map(|p| (p.first, p.count)),
        _ => None,
    }
}

pub const DATA_NAME: &str = "LingOS Data";

/// GPT names that mark the LingOS Data partition: the installer's "LingOS
/// Data", and "LINGDATA" / "LingOS-Data" made by hand (e.g. from Linux).
pub fn is_data_name(n: &str) -> bool {
    let n = n.trim();
    n.eq_ignore_ascii_case(DATA_NAME) || n.eq_ignore_ascii_case("LINGDATA") || n.eq_ignore_ascii_case("LingOS-Data")
}
pub const ESP_NAME: &str = "LingOS EFI";
pub const LINGOS_NAME: &str = "LingOS";

// ── which disk LingOS booted from ───────────────────────────────────────────

/// What lingboot said about the partition it was started from (the
/// Multiboot2 command line `lingboot bootdev=<kind> bootpart=gpt:<hex>` or
/// `bootpart=mbr:<signature hex>`). None under GRUB / an older lingboot.
pub enum BootPart {
    Gpt([u8; 16]),
    Mbr(u32),
}

pub fn boot_part() -> Option<BootPart> {
    let cl = crate::arch::bootmodule::cmdline()?;
    for tok in cl.split_whitespace() {
        if let Some(v) = tok.strip_prefix("bootpart=gpt:") {
            if v.len() == 32 {
                let mut g = [0u8; 16];
                for i in 0..16 {
                    g[i] = u8::from_str_radix(&v[i * 2..i * 2 + 2], 16).ok()?;
                }
                return Some(BootPart::Gpt(g));
            }
        }
        if let Some(v) = tok.strip_prefix("bootpart=mbr:") {
            return u32::from_str_radix(v, 16).ok().map(BootPart::Mbr);
        }
    }
    None
}

/// The device kind lingboot booted from ("usb", "sata", "nvme", ...).
pub fn boot_kind() -> Option<String> {
    let cl = crate::arch::bootmodule::cmdline()?;
    cl.split_whitespace().find_map(|t| t.strip_prefix("bootdev=").map(String::from))
}

/// The disk holding the partition LingOS was started from, when lingboot said.
pub fn boot_disk() -> Option<DiskId> {
    let bp = boot_part()?;
    for d in list() {
        match (&bp, table(d.id)) {
            (BootPart::Gpt(g), Ok(Table::Gpt { parts, .. })) if parts.iter().any(|p| &p.uniq == g) => return Some(d.id),
            (BootPart::Mbr(sig), Ok(Table::Mbr { signature, .. })) if *sig != 0 && signature == *sig => return Some(d.id),
            _ => {},
        }
    }
    // a whole-disk FAT stick has no table: lingboot reports it as USB
    if boot_kind().as_deref() == Some("usb") && crate::drivers::usbmsd::ready() {
        return Some(DiskId::Usb);
    }
    None
}

// ── what a partition holds ──────────────────────────────────────────────────

/// One line about the filesystem at sector `first`, and whether it looks
/// like another operating system's data (Windows, Linux).
pub fn probe(id: DiskId, p: &Part) -> (String, bool) {
    let mut b = [0u8; 4096];
    if read(id, p.first, &mut b).is_err() {
        return (String::from("unreadable"), false);
    }
    let s0 = &b[..512];
    if p.type_guid == TYPE_LINGOS {
        return (String::from("LingOS files (lingfs)"), false);
    }
    if &s0[3..11] == b"NTFS    " {
        return (String::from("NTFS (Windows)"), true);
    }
    if &s0[3..11] == b"-FVE-FS-" {
        return (String::from("BitLocker-encrypted (Windows)"), true);
    }
    if &s0[3..11] == b"EXFAT   " {
        return (String::from("exFAT"), true);
    }
    // ext2/3/4: superblock at byte 1024, magic 0xEF53 at +56
    if b[1024 + 56] == 0x53 && b[1024 + 57] == 0xEF {
        let incompat = u32::from_le_bytes(b[1024 + 96..1024 + 100].try_into().unwrap());
        let kind = if incompat & 0x40 != 0 { "ext4" } else { "ext2/ext3" };
        return (format!("{kind} (Linux)"), true);
    }
    if &b[4086..4096] == b"SWAPSPACE2" {
        return (String::from("Linux swap"), true);
    }
    if &b[512..520] == b"LABELONE" {
        return (String::from("Linux LVM"), true);
    }
    if &s0[0..6] == b"XFSB\0\0" || &s0[0..4] == b"XFSB" {
        return (String::from("XFS (Linux)"), true);
    }
    if p.type_guid == TYPE_MS_RESERVED {
        return (String::from("Microsoft reserved"), true);
    }
    if p.type_guid == TYPE_WIN_RECOVERY {
        return (String::from("Windows recovery"), true);
    }
    let fat32 = &s0[82..87] == b"FAT32";
    let fat16 = &s0[54..59] == b"FAT16" || &s0[54..59] == b"FAT12";
    if fat32 || fat16 {
        let kind = if fat32 { "FAT32" } else { "FAT16" };
        let label = {
            let l = if fat32 { &s0[71..82] } else { &s0[43..54] };
            String::from(String::from_utf8_lossy(l).trim())
        };
        let mut os = Vec::new();
        if p.first < u32::MAX as u64 {
            let mut io = DiskIo(id);
            if let Some(mut v) = crate::fs::fat::Volume::mount(&mut io, p.first as u32) {
                let mut has = |path: &str| matches!(v.lookup(&mut io, path), Ok(Some(_)));
                if has("EFI/Microsoft") {
                    os.push("Windows Boot Manager");
                }
                if has("EFI/debian") {
                    os.push("Debian (GRUB)");
                }
                if has("EFI/ubuntu") {
                    os.push("Ubuntu (GRUB)");
                }
                if has("EFI/fedora") {
                    os.push("Fedora");
                }
                if has("boot/wm.elf") {
                    os.push("LingOS");
                } else if has("EFI/BOOT/BOOTX64.EFI") {
                    os.push("a UEFI boot loader");
                }
            }
        }
        let esp = p.type_guid == TYPE_ESP || p.mbr_type == 0xEF;
        let mut s = format!("{kind}{}{}", if label.is_empty() || label == "NO NAME" { String::new() } else { format!(" \"{label}\"") }, if esp { ", EFI system partition" } else { "" });
        let foreign = os.iter().any(|o| *o != "LingOS");
        if !os.is_empty() {
            s.push_str(" with ");
            s.push_str(&os.join(", "));
        }
        return (s, foreign);
    }
    if b.iter().all(|&x| x == 0) {
        return (String::from("empty"), false);
    }
    let t = if p.mbr_type != 0 {
        match p.mbr_type {
            0x07 => "NTFS/exFAT type",
            0x83 => "Linux type",
            0x82 => "Linux swap type",
            0x8E => "Linux LVM type",
            0xEE => "GPT protective",
            _ => "unknown type",
        }
    } else if p.type_guid == TYPE_LINUX {
        "Linux type"
    } else if p.type_guid == TYPE_BASIC_DATA {
        "Windows basic data type"
    } else {
        "unrecognised contents"
    };
    (format!("data ({t})"), matches!(p.mbr_type, 0x07 | 0x83 | 0x82 | 0x8E) || p.type_guid == TYPE_LINUX)
}

pub fn type_name(p: &Part) -> String {
    if p.mbr_type != 0 {
        return format!("MBR type 0x{:02X}", p.mbr_type);
    }
    let t = &p.type_guid;
    String::from(if *t == TYPE_ESP {
        "EFI system"
    } else if *t == TYPE_BASIC_DATA {
        "basic data"
    } else if *t == TYPE_MS_RESERVED {
        "Microsoft reserved"
    } else if *t == TYPE_WIN_RECOVERY {
        "Windows recovery"
    } else if *t == TYPE_LINUX {
        "Linux filesystem"
    } else if *t == TYPE_LINUX_SWAP {
        "Linux swap"
    } else if *t == TYPE_LINUX_LVM {
        "Linux LVM"
    } else if *t == TYPE_BIOS_BOOT {
        "BIOS boot"
    } else if *t == TYPE_LINGOS {
        "LingOS"
    } else {
        return guid_str(t);
    })
}

/// "931.5 GiB" / "512 MiB".
pub fn size_str(sectors: u64) -> String {
    let mib = sectors / 2048;
    if mib >= 1024 {
        let tenths = sectors * 10 / (2048 * 1024);
        format!("{}.{} GiB", tenths / 10, tenths % 10)
    } else {
        format!("{mib} MiB")
    }
}

// ── writing a GPT ───────────────────────────────────────────────────────────

/// CRC-32 (IEEE, reflected), as GPT uses.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

pub struct NewPart {
    pub type_guid: [u8; 16],
    pub uniq: [u8; 16],
    pub first: u64,
    pub last: u64,
    pub name: &'static str,
}

/// The protective MBR, both GPT headers and both entry arrays for a
/// 512-byte-sector disk of `total` sectors (written by the caller).
/// Returns (primary sectors 0..34, backup entries at total-33 .. total-1,
/// backup header at total-1).
pub fn build_gpt(total: u64, disk_guid: [u8; 16], parts: &[NewPart]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    const N: usize = 128;
    let mut ents = vec![0u8; N * 128];
    for (i, p) in parts.iter().enumerate() {
        let e = &mut ents[i * 128..(i + 1) * 128];
        e[0..16].copy_from_slice(&p.type_guid);
        e[16..32].copy_from_slice(&p.uniq);
        e[32..40].copy_from_slice(&p.first.to_le_bytes());
        e[40..48].copy_from_slice(&p.last.to_le_bytes());
        for (k, u) in p.name.encode_utf16().take(36).enumerate() {
            e[56 + k * 2..58 + k * 2].copy_from_slice(&u.to_le_bytes());
        }
    }
    let ents_crc = crc32(&ents);
    let last_usable = total - 34;
    let header = |my: u64, alt: u64, ent_lba: u64| -> Vec<u8> {
        let mut h = vec![0u8; 512];
        h[0..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&92u32.to_le_bytes());
        h[24..32].copy_from_slice(&my.to_le_bytes());
        h[32..40].copy_from_slice(&alt.to_le_bytes());
        h[40..48].copy_from_slice(&34u64.to_le_bytes());
        h[48..56].copy_from_slice(&last_usable.to_le_bytes());
        h[56..72].copy_from_slice(&disk_guid);
        h[72..80].copy_from_slice(&ent_lba.to_le_bytes());
        h[80..84].copy_from_slice(&(N as u32).to_le_bytes());
        h[84..88].copy_from_slice(&128u32.to_le_bytes());
        h[88..92].copy_from_slice(&ents_crc.to_le_bytes());
        let c = crc32(&h[..92]);
        h[16..20].copy_from_slice(&c.to_le_bytes());
        h
    };
    let mut primary = vec![0u8; 34 * 512];
    // protective MBR: one 0xEE partition over the whole disk
    {
        let m = &mut primary[..512];
        let e = &mut m[446..462];
        e[0] = 0;
        e[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
        e[4] = 0xEE;
        e[5..8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
        e[8..12].copy_from_slice(&1u32.to_le_bytes());
        let n = (total - 1).min(u32::MAX as u64) as u32;
        e[12..16].copy_from_slice(&n.to_le_bytes());
        m[510] = 0x55;
        m[511] = 0xAA;
    }
    primary[512..1024].copy_from_slice(&header(1, total - 1, 2));
    primary[1024..1024 + N * 128].copy_from_slice(&ents);
    let backup_hdr = header(total - 1, 1, total - 33);
    (primary, ents, backup_hdr)
}
