//! Block device selection. x86: try AHCI (SATA) first, fall back to legacy
//! ATA PIO -- so the disk works whether it is attached via modern SATA/AHCI
//! (VirtualBox's default, most real current hardware) or legacy IDE (QEMU's
//! `-drive if=ide`, old hardware). Raspberry Pi: the SD card (`arch::sdcard`).
//! `lingfs` calls only this module, never a driver directly.

#[cfg(target_arch = "x86_64")]
use crate::drivers::ata;

#[cfg(target_arch = "x86_64")]
pub const SECTOR_SIZE: usize = ata::SECTOR_SIZE;
#[cfg(target_arch = "aarch64")]
pub const SECTOR_SIZE: usize = crate::arch::sdcard::SECTOR_SIZE;

#[cfg(target_arch = "x86_64")]
static mut USE_AHCI: bool = false;
#[cfg(target_arch = "x86_64")]
static mut INITED: bool = false;
/// lingfs lives in RAM this boot (see `init`).
#[cfg(target_arch = "x86_64")]
static mut USE_RAM: bool = false;
#[cfg(target_arch = "x86_64")]
static mut WHY_RAM: alloc::string::String = alloc::string::String::new();
/// lingfs lives in a LingOS GPT partition (on a SATA disk or the NVMe
/// drive): the disk, its first sector and size.
#[cfg(target_arch = "x86_64")]
static mut PART: Option<(crate::fs::disks::DiskId, u64, u64)> = None;
/// The "LingOS Data" FAT partition on the same disk (mounted as media/data).
#[cfg(target_arch = "x86_64")]
static mut DATA_PART: Option<(crate::fs::disks::DiskId, u64, u64)> = None;
/// lingfs is on the whole primary SATA/IDE disk (the old raw layout).
#[cfg(target_arch = "x86_64")]
static mut RAW_DISK: bool = false;

/// x86 picks the disk (AHCI port 0, else the legacy ATA master), then decides
/// whether LingOS may keep its files on it. lingfs writes raw sectors from
/// LBA `LINGFS_BASE_LBA` (128 MiB in) and formats a volume it can't find, so
/// on someone else's disk -- a Windows or Linux install, GPT partitions -- that
/// would destroy data. It may use the disk only if the disk is LingOS's own
/// (the disk-boot kernel header at LBA 17, or a lingfs superblock) or entirely
/// blank. Otherwise -- and when there is no disk this kernel can drive at all
/// (an NVMe-only PC) -- lingfs gets a RAM disk: the session works (accounts,
/// settings, files) and is forgotten at power-off. The installer still sees
/// the physical disk through `raw_read` / `raw_write`.
#[cfg(target_arch = "x86_64")]
pub fn init() {
    if unsafe { INITED } {
        return;
    }
    unsafe {
        USE_AHCI = crate::drivers::ahci::init().is_ok();
        INITED = true;
        decide();
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn decide() {
    use alloc::format;
    use alloc::string::String;
    let why = &mut *(&raw mut WHY_RAM);
    // An installed LingOS (`install-disk`): a disk with a LingOS GPT partition.
    // The disk this boot came from wins, then SATA disks, then NVMe.
    if let Some((id, first, count, name)) = lingos_gpt_partition() {
        use crate::fs::disks::DiskId;
        PART = Some((id, first, count));
        USE_RAM = false;
        why.clear();
        open_windows(id, first, count);
        crate::console_write(
            format!("storage: lingfs on the {} disk's LingOS partition \"{name}\" ({} MiB at sector {first})\n", id.kind(), count / 2048).as_bytes(),
        );
        // the "LingOS Data" FAT partition: on this disk, else on another
        // SATA / NVMe disk (made by hand, e.g. from Linux, when the system
        // disk is small or the big disk is elsewhere)
        let mut cands = alloc::vec![id];
        for d in crate::fs::disks::list() {
            if d.id != id && d.id != DiskId::Usb && d.block == 512 {
                cands.push(d.id);
            }
        }
        // The first one found is media/data; Data partitions on the OTHER disks
        // (the user's 680 GB LINGDATA on the NVMe next to an install on SATA)
        // become media/lingdata, media/lingdata2 -- each gets a write window
        // over exactly its own partition.
        for did in cands {
            let Some((df, dc)) = crate::fs::disks::data_partition(did) else { continue };
            if df + dc > u32::MAX as u64 {
                continue; // FAT here uses 32-bit sector numbers
            }
            match did {
                DiskId::Sata(p) => crate::drivers::ahci::add_write_window(p as usize, df, df + dc),
                DiskId::Nvme => crate::drivers::nvme::add_write_window(df, df + dc),
                DiskId::Usb => {},
            }
            if DATA_PART.is_none() {
                DATA_PART = Some((did, df, dc));
                if did != id {
                    crate::console_write(format!("storage: LingOS Data partition on the {} disk\n", did.kind()).as_bytes());
                }
            } else {
                (*(&raw mut EXTRA_DATA)).push((did, df, dc));
                crate::console_write(format!("storage: another Data partition on the {} disk ({} MiB)\n", did.kind(), dc / 2048).as_bytes());
            }
        }
        return;
    }
    let mut s0 = [0u8; SECTOR_SIZE];
    let sata = if raw_read(0, &mut s0).is_err() {
        Err(String::from("no SATA/IDE disk"))
    } else if disk_is_lingos(&s0) {
        Ok(())
    } else {
        Err(String::from("the SATA/IDE disk holds other data, so LingOS leaves it untouched"))
    };
    USE_RAM = false;
    why.clear();
    if sata.is_ok() {
        RAW_DISK = true;
        if let Some(p) = crate::drivers::ahci::primary_port().filter(|_| USE_AHCI) {
            crate::drivers::ahci::add_write_window(p, 0, u64::MAX);
        }
        crate::console_write(if USE_AHCI { b"storage: lingfs on the SATA disk (ahci0)\n" } else { b"storage: lingfs on the IDE disk (ata0)\n" });
        return;
    }
    // an NVMe drive: only its LingOS partition, never the rest of the drive
    let nv = nvme_partition();
    match nv {
        Ok((first, count, ref name)) => {
            PART = Some((crate::fs::disks::DiskId::Nvme, first, count));
            crate::drivers::nvme::set_write_window(first, first + count);
            crate::console_write(
                format!("storage: lingfs on the NVMe drive's LingOS partition \"{name}\" ({} MiB at sector {first})\n", count / 2048).as_bytes(),
            );
        },
        Err(e) => {
            USE_RAM = true;
            *why = format!("{}; {e}", sata.unwrap_err());
            crate::console_write(b"storage: RAM disk for this session -- ");
            crate::console_write(why.as_bytes());
            crate::console_write(b"\n");
        },
    }
}

/// Open the write window over the LingOS partition (nothing else of the disk).
#[cfg(target_arch = "x86_64")]
fn open_windows(id: crate::fs::disks::DiskId, first: u64, count: u64) {
    use crate::fs::disks::DiskId;
    match id {
        DiskId::Sata(p) => crate::drivers::ahci::add_write_window(p as usize, first, first + count),
        DiskId::Nvme => crate::drivers::nvme::set_write_window(first, first + count),
        DiskId::Usb => {},
    }
}

/// A LingOS GPT partition big enough for lingfs's two slots on a SATA disk or
/// the NVMe drive -- the one on the disk lingboot started from first.
#[cfg(target_arch = "x86_64")]
fn lingos_gpt_partition() -> Option<(crate::fs::disks::DiskId, u64, u64, alloc::string::String)> {
    use crate::fs::disks::{self, DiskId};
    use crate::fs::lingfs::objects::SLOT_SECTORS;
    let need = 2 * SLOT_SECTORS as u64;
    let boot = disks::boot_disk();
    let mut found = alloc::vec::Vec::new();
    for d in disks::list() {
        if d.id == DiskId::Usb || d.block != 512 {
            continue;
        }
        if let Some((first, count, name)) = disks::lingos_partition(d.id) {
            if count >= need {
                found.push((d.id, first, count.min(u32::MAX as u64), name));
            }
        }
    }
    if let Some(b) = boot {
        if let Some(i) = found.iter().position(|f| f.0 == b) {
            return Some(found.swap_remove(i));
        }
    }
    // SATA before NVMe: an NVMe partition made by hand from Linux is still
    // found by the NVMe path in `decide` (after the old raw SATA rule)
    let i = found.iter().position(|f| matches!(f.0, DiskId::Sata(_)))?;
    Some(found.swap_remove(i))
}

/// The disk lingfs lives on this boot (None: RAM, or the IDE disk).
#[cfg(target_arch = "x86_64")]
pub fn system_disk() -> Option<crate::fs::disks::DiskId> {
    init();
    unsafe {
        if USE_RAM {
            return None;
        }
        if let Some((id, _, _)) = PART {
            return Some(id);
        }
        if RAW_DISK && USE_AHCI {
            return crate::drivers::ahci::primary_port().map(|p| crate::fs::disks::DiskId::Sata(p as u8));
        }
    }
    None
}

/// LingOS was started from a disk `install-disk` made, and lingfs lives in
/// that disk's LingOS partition (an installed system, not the live stick).
#[cfg(target_arch = "x86_64")]
pub fn installed_boot() -> bool {
    init();
    match unsafe { PART } {
        Some((id, _, _)) if !unsafe { USE_RAM } => crate::fs::disks::boot_disk() == Some(id),
        _ => false,
    }
}

/// The "LingOS Data" partition on the system disk: (disk, first, count).
#[cfg(target_arch = "x86_64")]
pub fn data_partition() -> Option<(crate::fs::disks::DiskId, u64, u64)> {
    init();
    unsafe { DATA_PART }
}

#[cfg(target_arch = "x86_64")]
static mut EXTRA_DATA: alloc::vec::Vec<(crate::fs::disks::DiskId, u64, u64)> = alloc::vec::Vec::new();

/// Data partitions on other disks than `data_partition()`'s (media/lingdata...).
#[cfg(target_arch = "x86_64")]
pub fn extra_data_partitions() -> alloc::vec::Vec<(crate::fs::disks::DiskId, u64, u64)> {
    init();
    unsafe { (*(&raw const EXTRA_DATA)).clone() }
}

/// The disk installer is about to erase the disk lingfs lives on: from now
/// on lingfs is a RAM disk (this session's changes are lost; the user
/// restarts right after installing). The installer replaces the disk's
/// write windows with its own.
#[cfg(target_arch = "x86_64")]
pub fn release_disk(id: crate::fs::disks::DiskId) -> bool {
    init();
    if system_disk() != Some(id) {
        return false;
    }
    unsafe {
        USE_RAM = true;
        PART = None;
        DATA_PART = None;
        RAW_DISK = false;
        *(&raw mut WHY_RAM) = alloc::string::String::from("LingOS was reinstalled onto its own disk this session; restart");
    }
    true
}

/// The NVMe LingOS partition, big enough for lingfs's two slots.
#[cfg(target_arch = "x86_64")]
fn nvme_partition() -> Result<(u64, u64, alloc::string::String), alloc::string::String> {
    use crate::drivers::nvme;
    use crate::fs::lingfs::objects::SLOT_SECTORS;
    use alloc::format;
    if !nvme::present() {
        return Err(alloc::string::String::from("no NVMe drive"));
    }
    let (first, count, name) = nvme::find_lingos_partition().map_err(|e| format!("NVMe: {e}"))?;
    let need = 2 * SLOT_SECTORS as u64;
    if count < need {
        return Err(format!("NVMe: the LingOS partition is {} MiB; lingfs needs at least {} MiB", count / 2048, need.div_ceil(2048)));
    }
    Ok((first, count.min(u32::MAX as u64), name))
}

/// lingfs sector -> disk sector inside the LingOS partition (None outside it).
#[cfg(target_arch = "x86_64")]
fn part_map(lba: u32) -> Option<u64> {
    let (_, first, count) = unsafe { PART }?;
    let rel = (lba as u64).checked_sub(crate::fs::lingfs::LINGFS_BASE_LBA as u64)?;
    if rel >= count {
        return None;
    }
    Some(first + rel)
}

/// The disk is LingOS's own, or blank.
#[cfg(target_arch = "x86_64")]
fn disk_is_lingos(s0: &[u8; SECTOR_SIZE]) -> bool {
    use crate::fs::lingfs::{commit::MAGIC, objects::SLOT_SECTORS, LINGFS_BASE_LBA};
    let mut s = [0u8; SECTOR_SIZE];
    // the disk-boot kernel header (bootloader/pack_header.py, "LNGK")
    if raw_read(17, &mut s).is_ok() && u32::from_le_bytes([s[0], s[1], s[2], s[3]]) == 0x474E_4B4C {
        return true;
    }
    // a lingfs volume in either slot
    for lba in [LINGFS_BASE_LBA, LINGFS_BASE_LBA + SLOT_SECTORS] {
        if raw_read(lba, &mut s).is_ok() && u64::from_le_bytes(s[0..8].try_into().unwrap()) == MAGIC {
            return true;
        }
    }
    // blank: no MBR/boot code, no GPT header, nothing where lingfs would go
    let blank = |b: &[u8]| b.iter().all(|&x| x == 0) || b.iter().all(|&x| x == 0xFF);
    if !blank(s0) {
        return false;
    }
    for lba in [1, 17, LINGFS_BASE_LBA] {
        if raw_read(lba, &mut s).is_err() || !blank(&s) {
            return false;
        }
    }
    true
}

#[cfg(target_arch = "aarch64")]
pub fn init() {}

/// lingfs is in RAM this boot (nothing will survive a restart).
#[cfg(target_arch = "x86_64")]
pub fn is_ram() -> bool {
    init();
    unsafe { USE_RAM }
}
#[cfg(target_arch = "aarch64")]
pub fn is_ram() -> bool {
    false
}

/// Why lingfs is in RAM ("" when it is on a disk).
#[cfg(target_arch = "x86_64")]
pub fn ram_reason() -> alloc::string::String {
    init();
    unsafe { (*(&raw const WHY_RAM)).clone() }
}
#[cfg(target_arch = "aarch64")]
pub fn ram_reason() -> alloc::string::String {
    alloc::string::String::new()
}

/// The physical disk's name for the installer ("none" when nothing answers).
#[cfg(target_arch = "x86_64")]
pub fn physical_name() -> &'static str {
    init();
    if raw_read(0, &mut [0u8; SECTOR_SIZE]).is_err() {
        return "none";
    }
    if unsafe { USE_AHCI } {
        "ahci0"
    } else {
        "ata0"
    }
}

/// The physical disk, bypassing the RAM-disk decision (the installer only).
#[cfg(target_arch = "x86_64")]
pub fn raw_read(lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
    init(); // (INITED is set before `decide` reads through here)
    if unsafe { USE_AHCI } {
        crate::drivers::ahci::read_sector(lba, buf)
    } else {
        ata::read_sector(lba, buf)
    }
}

#[cfg(target_arch = "x86_64")]
pub fn raw_write(lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
    init();
    if unsafe { USE_AHCI } {
        crate::drivers::ahci::write_sector(lba, buf)
    } else {
        ata::write_sector(lba, buf)
    }
}

#[cfg(target_arch = "x86_64")]
pub fn read_sector(lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
    init();
    if unsafe { USE_RAM } {
        ram::read(lba, buf);
        return Ok(());
    }
    if let Some((id, _, _)) = unsafe { PART } {
        // below lingfs's base (the disk-boot header etc.) there is nothing here
        return match part_map(lba) {
            Some(s) => crate::fs::disks::read(id, s, buf),
            None => Err(()),
        };
    }
    raw_read(lba, buf)
}

#[cfg(target_arch = "x86_64")]
pub fn write_sector(lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
    init();
    if unsafe { USE_RAM } {
        ram::write(lba, buf);
        return Ok(());
    }
    if let Some((id, _, _)) = unsafe { PART } {
        return match part_map(lba) {
            Some(s) => crate::fs::disks::write(id, s, buf),
            None => Err(()),
        };
    }
    raw_write(lba, buf)
}

/// A sparse RAM disk: 4 KiB pages allocated on first write, zeros elsewhere.
#[cfg(target_arch = "x86_64")]
mod ram {
    use super::SECTOR_SIZE;
    use alloc::boxed::Box;
    use alloc::collections::BTreeMap;

    const PER: u32 = (4096 / SECTOR_SIZE) as u32;
    static mut PAGES: BTreeMap<u32, Box<[u8; 4096]>> = BTreeMap::new();

    fn pages() -> &'static mut BTreeMap<u32, Box<[u8; 4096]>> {
        unsafe { &mut *(&raw mut PAGES) }
    }

    pub fn read(lba: u32, buf: &mut [u8; SECTOR_SIZE]) {
        let off = (lba % PER) as usize * SECTOR_SIZE;
        match pages().get(&(lba / PER)) {
            Some(p) => buf.copy_from_slice(&p[off..off + SECTOR_SIZE]),
            None => buf.fill(0),
        }
    }

    pub fn write(lba: u32, buf: &[u8; SECTOR_SIZE]) {
        let off = (lba % PER) as usize * SECTOR_SIZE;
        let p = pages().entry(lba / PER).or_insert_with(|| Box::new([0u8; 4096]));
        p[off..off + SECTOR_SIZE].copy_from_slice(buf);
    }
}

#[cfg(target_arch = "aarch64")]
pub fn read_sector(lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
    crate::arch::sdcard::read_sector(lba, buf)
}

#[cfg(target_arch = "aarch64")]
pub fn write_sector(lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
    crate::arch::sdcard::write_sector(lba, buf)
}

/// Which backend `read_sector`/`write_sector` actually dispatch to right
/// now -- used to populate the synthetic `dev/` directory `lingfs` seeds on
/// mount, so `ls dev` reflects the disk controller this boot actually
/// detected rather than a hardcoded guess.
pub fn active_driver_name() -> &'static str {
    init();
    #[cfg(target_arch = "x86_64")]
    {
        if unsafe { USE_RAM } {
            return "ram0";
        }
        match unsafe { PART } {
            Some((crate::fs::disks::DiskId::Nvme, _, _)) => return "nvme0",
            Some(_) => return "ahci0",
            None => {},
        }
        if unsafe { USE_AHCI } {
            return "ahci0";
        }
        return "ata0";
    }
    #[allow(unreachable_code)]
    "sd0"
}
