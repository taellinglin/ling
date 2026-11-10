//! `install-disk` -- install LingOS from the USB stick it is running from onto
//! a hard disk, from the desktop Terminal.
//!
//! The installed disk (GPT, 512-byte sectors):
//!
//! | # | partition     | type                       | size                 |
//! |---|---------------|----------------------------|----------------------|
//! | 1 | "LingOS EFI"  | EFI system (FAT32)         | 512 MiB              |
//! | 2 | "LingOS"      | LingOS (8F6E4C2A-...)      | 1 GiB (256 MiB < 16 GiB disks) |
//! | 3 | "LingOS Data" | basic data (FAT32)         | the rest (up to 2 TiB) |
//!
//! The ESP carries `\EFI\BOOT\BOOTX64.EFI` (lingboot, the removable-media path
//! every UEFI firmware looks for, so no boot entry needs writing) and
//! `\boot\wm.elf` (the kernel), both copied from the stick and read back to
//! check. lingfs formats the LingOS partition at the first boot from the disk
//! (`fs::blockdev` finds it by type); the Data partition becomes `media/data`.
//!
//! Safety: listing reads only. `install-disk <n>` shows what will be erased and
//! asks for `ERASE`; only then is a write window opened on that one disk (the
//! drivers refuse writes outside their windows), and it is closed when the
//! install ends. The disk LingOS booted from and the USB stick are refused.
//!
//! The work runs in steps from the Terminal's draw (`pump`), a few dozen ms a
//! frame, so the desktop keeps drawing and the progress shows.

use crate::fs::disks::{self, DiskId, Table};
use crate::fs::fat;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Line kinds for the Terminal: normal, dim, error, ok, accent.
pub const NORMAL: u8 = 0;
pub const DIM: u8 = 1;
pub const ERR: u8 = 2;
pub const OK: u8 = 3;
pub const ACCENT: u8 = 4;

pub type Out = Vec<(u8, String)>;

fn say(o: &mut Out, k: u8, s: impl Into<String>) {
    o.push((k, s.into()));
}

const MIB: u64 = 2048; // sectors
const ESP_SECTORS: u64 = 512 * MIB;
const MIN_DISK: u64 = 2 * 1024 * MIB;
const CHUNK: usize = 1 << 20;
const STEP_BUDGET_MS: u64 = 40;

/// The boot files on the stick.
struct Source {
    efi: String,
    kernel: String,
    efi_size: u64,
    kernel_size: u64,
}

struct Layout {
    total: u64,
    esp: (u64, u64),
    ling: (u64, u64),
    data: Option<(u64, u64)>,
}

/// A FAT32 volume being made: where it is and its geometry.
#[derive(Clone, Copy)]
struct Fat32 {
    first: u64,
    total: u32,
    spc: u32,
    reserved: u32,
    fatsz: u32,
    clusters: u32,
}

impl Fat32 {
    fn new(first: u64, total: u64, spc: u32) -> Option<Fat32> {
        let total = u32::try_from(total).ok()?;
        let reserved = 32u32;
        let tmp1 = total.checked_sub(reserved)?;
        let tmp2 = (256 * spc + 2) / 2;
        let fatsz = tmp1.div_ceil(tmp2);
        let data_start = reserved + 2 * fatsz;
        let clusters = total.checked_sub(data_start)? / spc;
        if clusters < 65525 || clusters > 0x0FFF_FFF5 || (fatsz as u64) * 128 < clusters as u64 + 2 {
            return None;
        }
        Some(Fat32 { first, total, spc, reserved, fatsz, clusters })
    }

    fn cluster_bytes(&self) -> u64 {
        self.spc as u64 * 512
    }

    fn cluster_lba(&self, c: u32) -> u64 {
        self.first + (self.reserved + 2 * self.fatsz) as u64 + (c as u64 - 2) * self.spc as u64
    }

    /// The reserved area: boot sector, FSInfo, their backups at 6 and 7.
    fn reserved_area(&self, label: &[u8; 11], volid: u32, free: u32, next_free: u32) -> Vec<u8> {
        let mut r = vec![0u8; self.reserved as usize * 512];
        let mut bs = [0u8; 512];
        bs[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        bs[3..11].copy_from_slice(b"LINGOS  ");
        bs[11..13].copy_from_slice(&512u16.to_le_bytes());
        bs[13] = self.spc as u8;
        bs[14..16].copy_from_slice(&(self.reserved as u16).to_le_bytes());
        bs[16] = 2;
        bs[21] = 0xF8;
        bs[24..26].copy_from_slice(&63u16.to_le_bytes());
        bs[26..28].copy_from_slice(&255u16.to_le_bytes());
        bs[28..32].copy_from_slice(&(self.first.min(u32::MAX as u64) as u32).to_le_bytes());
        bs[32..36].copy_from_slice(&self.total.to_le_bytes());
        bs[36..40].copy_from_slice(&self.fatsz.to_le_bytes());
        bs[44..48].copy_from_slice(&2u32.to_le_bytes()); // root directory cluster
        bs[48..50].copy_from_slice(&1u16.to_le_bytes()); // FSInfo sector
        bs[50..52].copy_from_slice(&6u16.to_le_bytes()); // backup boot sector
        bs[64] = 0x80;
        bs[66] = 0x29;
        bs[67..71].copy_from_slice(&volid.to_le_bytes());
        bs[71..82].copy_from_slice(label);
        bs[82..90].copy_from_slice(b"FAT32   ");
        // boot code: not a bootable volume -- print nothing, wait (int 18h asks the BIOS for the next device)
        bs[90..92].copy_from_slice(&[0xCD, 0x18]);
        bs[510] = 0x55;
        bs[511] = 0xAA;
        let mut fsi = [0u8; 512];
        fsi[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
        fsi[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
        fsi[488..492].copy_from_slice(&free.to_le_bytes());
        fsi[492..496].copy_from_slice(&next_free.to_le_bytes());
        fsi[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
        for at in [0usize, 6] {
            r[at * 512..at * 512 + 512].copy_from_slice(&bs);
            r[(at + 1) * 512..(at + 1) * 512 + 512].copy_from_slice(&fsi);
            // sector 2 of each boot area ends with the signature too
            r[(at + 2) * 512 + 510] = 0x55;
            r[(at + 2) * 512 + 511] = 0xAA;
        }
        r
    }
}

/// Where the ESP's clusters go: 2 root, 3 EFI, 4 EFI/BOOT, 5 boot, then the
/// two files back to back.
#[derive(Clone, Copy)]
struct EspFiles {
    efi_c0: u32,
    efi_n: u32,
    k_c0: u32,
    k_n: u32,
}

const EOC: u32 = 0x0FFF_FFFF;

impl EspFiles {
    fn entry(&self, c: u32) -> u32 {
        match c {
            0 => 0x0FFF_FFF8,
            1 => EOC,
            2..=5 => EOC,
            _ => {
                for (c0, n) in [(self.efi_c0, self.efi_n), (self.k_c0, self.k_n)] {
                    if c >= c0 && c < c0 + n {
                        return if c == c0 + n - 1 { EOC } else { c + 1 };
                    }
                }
                0
            },
        }
    }
    fn used_end(&self) -> u32 {
        self.k_c0 + self.k_n
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Zero,
    Gpt,
    EspReserved,
    EspFat(u64),
    EspDirs,
    Copy(usize, u64),
    LingZero,
    DataReserved,
    DataFat(u64),
    DataRoot,
    Flush,
    VerifyGpt,
    VerifyOpen,
    Verify(usize, u64),
    Done,
}

struct Job {
    id: DiskId,
    label: String,
    lay: Layout,
    src: Source,
    esp: Fat32,
    files: EspFiles,
    data: Option<Fat32>,
    step: Step,
    ranges: [Option<crate::fs::media::MediaRange>; 2],
    hasher: Option<::blake3::Hasher>,
    src_hash: [[u8; 32]; 2],
    verify: Option<(fat::Volume, [Option<fat::RangeFile>; 2])>,
    saved_windows: Vec<(u64, u64)>,
    released: bool,
    started: u64,
    last_tenth: u64,
    buf: Vec<u8>,
}

struct Plan {
    id: DiskId,
    with_data: bool,
    sectors: u64,
    s01: Vec<u8>,
}

enum State {
    Idle,
    Confirm(Box<Plan>),
    Running(Box<Job>),
}

static mut STATE: State = State::Idle;

fn state() -> &'static mut State {
    unsafe { &mut *(&raw mut STATE) }
}

/// The Terminal is waiting for the ERASE answer.
pub fn awaiting() -> bool {
    matches!(state(), State::Confirm(_))
}

pub fn running() -> bool {
    matches!(state(), State::Running(_))
}

// ── listing ─────────────────────────────────────────────────────────────────

fn find_source() -> Option<Source> {
    for vol in crate::fs::media::names() {
        if !vol.starts_with("usb") {
            continue;
        }
        let efi = format!("media/{vol}/efi/boot/bootx64.efi");
        let kernel = format!("media/{vol}/boot/wm.elf");
        let (Some(a), Some(b)) = (crate::fs::media::open_range(&efi), crate::fs::media::open_range(&kernel)) else { continue };
        if a.size() == 0 || b.size() == 0 {
            continue;
        }
        return Some(Source { efi, kernel, efi_size: a.size(), kernel_size: b.size() });
    }
    None
}

fn disk_line(i: usize, d: &disks::DiskInfo) -> String {
    let model = if d.model.is_empty() { String::from("(no model name)") } else { d.model.clone() };
    let at = match d.id {
        DiskId::Sata(p) => format!(" (SATA port {p})"),
        _ => String::new(),
    };
    format!("[{i}] {:<4} {}  {}{at}", d.id.kind(), model, disks::size_str(d.sectors))
}

/// The partitions of `id`, one line each; `foreign` collects what looks like
/// another operating system's data.
fn describe_parts(o: &mut Out, id: DiskId, foreign: &mut Vec<String>) {
    match disks::table(id) {
        Err(e) => say(o, ERR, format!("      partition table: {e}")),
        Ok(Table::None) => {
            let mut s = [0u8; 512];
            let blank = disks::read(id, 0, &mut s).is_ok() && s.iter().all(|&b| b == 0);
            if blank {
                say(o, DIM, "      no partition table (blank start of disk)");
            } else {
                let whole = disks::Part { number: 0, type_guid: [0; 16], uniq: [0; 16], mbr_type: 0, first: 0, count: disks::sectors(id), name: String::new() };
                let (what, f) = disks::probe(id, &whole);
                say(o, NORMAL, format!("      no partition table; the whole disk holds: {what}"));
                if f {
                    foreign.push(what);
                }
            }
        },
        Ok(Table::Mbr { parts, .. }) => {
            say(o, NORMAL, format!("      MBR, {} partition(s):", parts.len()));
            for p in &parts {
                let (what, f) = disks::probe(id, p);
                say(o, NORMAL, format!("        {}  {:<16} {:>10}  {what}", p.number, disks::type_name(p), disks::size_str(p.count)));
                if f {
                    foreign.push(format!("partition {} ({what})", p.number));
                }
            }
        },
        Ok(Table::Gpt { parts, .. }) => {
            say(o, NORMAL, format!("      GPT, {} partition(s):", parts.len()));
            for p in &parts {
                let (what, f) = disks::probe(id, p);
                let name = if p.name.is_empty() { String::new() } else { format!("\"{}\"  ", p.name) };
                say(o, NORMAL, format!("        {}  {name}{:<18} {:>10}  {what}", p.number, disks::type_name(p), disks::size_str(p.count)));
                if f {
                    foreign.push(format!("partition {} {}({what})", p.number, name));
                }
            }
        },
    }
}

fn list(o: &mut Out) {
    let ds = disks::list();
    let boot = disks::boot_disk();
    let sys = crate::fs::blockdev::system_disk();
    say(o, ACCENT, "Disks this computer has (install-disk <n> installs LingOS onto one):");
    if ds.is_empty() {
        say(o, ERR, "  no disks found (SATA in AHCI mode, NVMe and USB sticks are supported;");
        say(o, ERR, "  a SATA controller set to RAID / Intel RST in the firmware is not)");
        return;
    }
    for (i, d) in ds.iter().enumerate() {
        say(o, NORMAL, String::new());
        let mut line = disk_line(i, d);
        if Some(d.id) == boot || (boot.is_none() && d.id == DiskId::Usb) {
            line.push_str("   <- LingOS is running from this disk");
        } else if Some(d.id) == sys {
            line.push_str("   <- LingOS keeps its files here now");
        }
        say(o, ACCENT, format!("  {line}"));
        let mut foreign = Vec::new();
        describe_parts(o, d.id, &mut foreign);
        if !foreign.is_empty() {
            say(o, ERR, "      holds another operating system's data");
        }
    }
    say(o, NORMAL, String::new());
    match find_source() {
        Some(s) => say(o, DIM, format!("install from: {} ({}) + {} ({} MiB)", s.efi, size_bytes(s.efi_size), s.kernel, s.kernel_size >> 20)),
        None => say(o, ERR, "no LingOS boot files found on a USB stick (efi/boot/bootx64.efi + boot/wm.elf)"),
    }
}

fn size_bytes(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{} MiB", n >> 20)
    } else {
        format!("{} KiB", n.div_ceil(1024))
    }
}

fn layout(total: u64, with_data: bool) -> Layout {
    let esp = (MIB, ESP_SECTORS);
    let ling_n = if total >= 16 * 1024 * MIB { 1024 * MIB } else { 256 * MIB };
    let ling = (esp.0 + esp.1, ling_n);
    let data_first = ling.0 + ling.1;
    // FAT and the 32-bit sector numbers it uses: the Data partition ends by 2 TiB
    let end = ((total - 34).min(u32::MAX as u64 + 1) / MIB) * MIB;
    let data = if with_data && end > data_first + 600 * MIB { Some((data_first, end - data_first)) } else { None };
    Layout { total, esp, ling, data }
}

fn data_spc(sectors: u64) -> u32 {
    let gib = sectors / (1024 * MIB);
    if gib < 8 {
        8
    } else if gib < 16 {
        16
    } else if gib < 32 {
        32
    } else {
        64
    }
}

/// `install-disk <n>`: what will happen; then wait for ERASE.
fn prepare(o: &mut Out, arg: &str) {
    let ds = disks::list();
    // `install-disk <n> data=<m>`: the Data partition on another disk. Not
    // written by the installer yet -- the design is: partition <m> gets a
    // GPT basic-data partition named "LingOS Data" (FAT32), and the system
    // disk none; `fs::blockdev` already looks for it on every disk.
    let mut words = arg.split_whitespace();
    let first = words.next().unwrap_or("");
    let mut with_data = true;
    if let Some(w) = words.next() {
        if w == "nodata" {
            with_data = false;
        } else if w.starts_with("data=") {
            say(o, ERR, "data=<disk>: putting the Data partition on another disk is not done by install-disk yet.");
            say(o, NORMAL, "Install with 'install-disk <n>' (the Data partition goes on the same disk), or make the");
            say(o, NORMAL, "partition yourself, e.g. from Linux in free space on the other disk:");
            say(o, NORMAL, format!("  sudo sgdisk -n 0:0:0 -t 0:{} -c 0:\"{}\" /dev/nvme0n1", disks::guid_str(&disks::TYPE_BASIC_DATA), disks::DATA_NAME));
            say(o, NORMAL, "  sudo mkfs.vfat -F 32 -n 'LINGOS DATA' /dev/nvme0n1p<new>");
            say(o, NORMAL, "LingOS then mounts it as media/data when the system disk has none:");
            say(o, NORMAL, "install with 'install-disk <n> nodata' to leave the system disk without one.");
            return;
        } else {
            say(o, ERR, format!("unknown option {w} (options: nodata)"));
            return;
        }
    }
    let Ok(n) = first.parse::<usize>() else {
        say(o, ERR, "usage: install-disk          (list the disks)");
        say(o, ERR, "       install-disk <n>      (install LingOS onto disk n)");
        say(o, ERR, "       install-disk <n> nodata  (... without a Data partition on it)");
        return;
    };
    let Some(d) = ds.get(n) else {
        say(o, ERR, format!("there is no disk [{n}] -- 'install-disk' lists them"));
        return;
    };
    let id = d.id;
    if id == DiskId::Usb {
        say(o, ERR, "refused: that is the USB stick LingOS is running and installing from.");
        return;
    }
    if disks::boot_disk() == Some(id) {
        say(o, ERR, "refused: LingOS booted from this disk. Boot from the USB stick to reinstall it.");
        return;
    }
    if d.block != 512 {
        say(o, ERR, format!("refused: this drive uses {}-byte sectors; the installer only handles 512-byte ones yet.", d.block));
        return;
    }
    if d.sectors < MIN_DISK {
        say(o, ERR, format!("refused: the disk is {}; LingOS needs at least 2 GiB.", disks::size_str(d.sectors)));
        return;
    }
    let Some(src) = find_source() else {
        say(o, ERR, "no LingOS boot files on a USB stick: need efi/boot/bootx64.efi and boot/wm.elf on media/usb0");
        return;
    };
    let lay = layout(d.sectors, with_data);
    if Fat32::new(lay.esp.0, lay.esp.1, 8).map(|f| f.clusters as u64 * 4096).unwrap_or(0) < src.efi_size + src.kernel_size + (1 << 20) {
        say(o, ERR, "the kernel does not fit the 512 MiB EFI partition");
        return;
    }
    say(o, ACCENT, format!("Install LingOS onto {}", disk_line(n, d)));
    say(o, NORMAL, "This ERASES THE WHOLE DISK. Everything on it now:");
    let mut foreign = Vec::new();
    describe_parts(o, id, &mut foreign);
    if !foreign.is_empty() {
        say(o, ERR, "");
        say(o, ERR, "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
        say(o, ERR, "!!  WARNING: this disk holds ANOTHER OPERATING SYSTEM'S DATA:");
        for f in &foreign {
            say(o, ERR, format!("!!    {f}"));
        }
        say(o, ERR, "!!  Windows / Linux on it will NOT start again and its files are LOST.");
        say(o, ERR, "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
    }
    if crate::fs::blockdev::system_disk() == Some(id) {
        say(o, ERR, "LingOS keeps its files on this disk right now: this session's files move to RAM and are lost.");
    }
    say(o, NORMAL, "It becomes (GPT):");
    say(o, NORMAL, format!("  1  \"{}\"   EFI system, FAT32   {:>10}  lingboot + the kernel", disks::ESP_NAME, disks::size_str(lay.esp.1)));
    say(o, NORMAL, format!("  2  \"{}\"       LingOS            {:>10}  accounts, settings (lingfs)", disks::LINGOS_NAME, disks::size_str(lay.ling.1)));
    match lay.data {
        Some((_, c)) => say(o, NORMAL, format!("  3  \"{}\"  basic data, FAT32  {:>10}  your files, packages, models (media/data)", disks::DATA_NAME, disks::size_str(c))),
        None if !with_data => say(o, DIM, "  (no Data partition: the rest of the disk stays unpartitioned)"),
        None => say(o, DIM, "  (no room for a Data partition)"),
    }
    if lay.total > u32::MAX as u64 + 1 {
        say(o, DIM, format!("  ({} past 2 TiB stays unpartitioned: FAT32 ends there)", disks::size_str(lay.total - (u32::MAX as u64 + 1))));
    }
    say(o, DIM, format!("from {} and {}", src.efi, src.kernel));
    let mut s01 = vec![0u8; 1024];
    if disks::read(id, 0, &mut s01).is_err() {
        say(o, ERR, "cannot read the disk");
        return;
    }
    say(o, ACCENT, format!("Type ERASE and press Enter to erase disk [{n}] ({}, {}).", d.model, disks::size_str(d.sectors)));
    say(o, ACCENT, "Anything else cancels.");
    *state() = State::Confirm(Box::new(Plan { id, with_data, sectors: d.sectors, s01 }));
}

/// The line typed after the ERASE prompt.
pub fn answer(line: &str) -> Out {
    let mut o = Vec::new();
    let State::Confirm(plan) = core::mem::replace(state(), State::Idle) else { return o };
    if line.trim_end() != "ERASE" {
        say(&mut o, OK, "cancelled -- nothing was written.");
        return o;
    }
    // the same disk, unchanged since the prompt
    let mut s01 = vec![0u8; 1024];
    if disks::sectors(plan.id) != plan.sectors || disks::read(plan.id, 0, &mut s01).is_err() || s01 != plan.s01 {
        say(&mut o, ERR, "the disk changed since the prompt -- nothing was written. Run install-disk again.");
        return o;
    }
    let Some(src) = find_source() else {
        say(&mut o, ERR, "the USB stick's boot files are gone -- nothing was written.");
        return o;
    };
    match start(plan.id, plan.with_data, src) {
        Ok(job) => {
            say(&mut o, ACCENT, format!("installing onto {} -- keep the USB stick in and do not switch off", job.label));
            *state() = State::Running(job);
        },
        Err(e) => say(&mut o, ERR, e),
    }
    o
}

fn start(id: DiskId, with_data: bool, src: Source) -> Result<Box<Job>, String> {
    let total = disks::sectors(id);
    let lay = layout(total, with_data);
    let esp = Fat32::new(lay.esp.0, lay.esp.1, 8).ok_or("cannot lay out the EFI partition")?;
    let cb = esp.cluster_bytes();
    let efi_n = src.efi_size.div_ceil(cb).max(1) as u32;
    let k_n = src.kernel_size.div_ceil(cb).max(1) as u32;
    let files = EspFiles { efi_c0: 6, efi_n, k_c0: 6 + efi_n, k_n };
    if files.used_end() > esp.clusters + 2 {
        return Err(String::from("the kernel does not fit the EFI partition"));
    }
    let data = match lay.data {
        Some((f, c)) => Some(Fat32::new(f, c, data_spc(c)).ok_or("cannot lay out the Data partition")?),
        None => None,
    };
    let label = disks::list().into_iter().find(|d| d.id == id).map(|d| format!("{} disk {} ({})", id.kind(), d.model, disks::size_str(d.sectors))).unwrap_or_default();
    let a = crate::fs::media::open_range(&src.efi).ok_or("cannot open the boot loader on the stick")?;
    let b = crate::fs::media::open_range(&src.kernel).ok_or("cannot open the kernel on the stick")?;
    // from here on this disk is the installer's: lingfs lets go of it, and the
    // one write window is the whole disk
    let released = crate::fs::blockdev::release_disk(id);
    crate::fs::media::unmount(crate::fs::media::Device::Disk(id));
    let saved_windows = match id {
        DiskId::Sata(p) => {
            let w = crate::drivers::ahci::write_windows(p as usize);
            crate::drivers::ahci::clear_write_windows(p as usize);
            crate::drivers::ahci::add_write_window(p as usize, 0, total);
            w
        },
        DiskId::Nvme => {
            let w = crate::drivers::nvme::write_windows();
            crate::drivers::nvme::set_write_window(0, total);
            w
        },
        DiskId::Usb => return Err(String::from("refused: the USB stick")),
    };
    Ok(Box::new(Job {
        id,
        label,
        lay,
        src,
        esp,
        files,
        data,
        step: Step::Zero,
        ranges: [Some(a), Some(b)],
        hasher: None,
        src_hash: [[0; 32]; 2],
        verify: None,
        saved_windows,
        released,
        started: crate::arch::timer::now_ms(),
        last_tenth: u64::MAX,
        buf: vec![0u8; CHUNK],
    }))
}

/// Close the installer's write window on the disk (and give lingfs back the
/// windows it had, if it still uses the disk).
fn close_windows(job: &Job) {
    let keep = if job.released { Vec::new() } else { job.saved_windows.clone() };
    match job.id {
        DiskId::Sata(p) => {
            crate::drivers::ahci::clear_write_windows(p as usize);
            for (a, b) in keep {
                crate::drivers::ahci::add_write_window(p as usize, a, b);
            }
        },
        DiskId::Nvme => crate::drivers::nvme::set_write_windows(keep),
        DiskId::Usb => {},
    }
}

// ── the work ────────────────────────────────────────────────────────────────

fn random_guid() -> [u8; 16] {
    let mut g = [0u8; 16];
    if !crate::crypto::random_bytes(&mut g) {
        let t = crate::arch::timer::now_us().to_le_bytes();
        g[..8].copy_from_slice(&t);
        g[8..].copy_from_slice(&t);
    }
    g[7] = (g[7] & 0x0F) | 0x40; // version 4 (the third field's high nibble)
    g[8] = (g[8] & 0x3F) | 0x80;
    g
}

fn dirent(name: &[u8; 11], attr: u8, nt: u8, cluster: u32, size: u32) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(name);
    e[11] = attr;
    e[12] = nt;
    let date: u16 = ((2026 - 1980) << 9) | (11 << 5) | 9;
    for o in [16usize, 18, 24] {
        e[o..o + 2].copy_from_slice(&date.to_le_bytes());
    }
    e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
    e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    e
}

/// One slice of the FAT area (both copies, `n` sectors from `at`).
fn fat_slice(f: &Fat32, at: u64, n: u64, entry: impl Fn(u32) -> u32) -> Vec<u8> {
    let mut v = vec![0u8; (n * 512) as usize];
    for k in 0..n {
        let fs = (at + k) % f.fatsz as u64; // sector within one FAT copy
        let c0 = (fs * 128) as u32;
        let s = &mut v[(k * 512) as usize..(k * 512 + 512) as usize];
        // past the clusters in use every entry is 0: skip the loop
        for j in 0..128u32 {
            let c = c0 + j;
            let e = if c < f.clusters + 2 { entry(c) } else { 0 };
            if e != 0 {
                s[j as usize * 4..j as usize * 4 + 4].copy_from_slice(&e.to_le_bytes());
            }
        }
    }
    v
}

fn wr(job: &Job, lba: u64, data: &[u8]) -> Result<(), String> {
    disks::write(job.id, lba, data).map_err(|_| format!("writing sector {lba} failed"))
}

fn progress(job: &mut Job, o: &mut Out, what: &str, done: u64, total: u64) {
    let tenth = done * 10 / total.max(1);
    if tenth != job.last_tenth {
        job.last_tenth = tenth;
        say(o, DIM, format!("  {what} {}%  ({} of {} MiB)", tenth * 10, done >> 20, total >> 20));
    }
}

/// Do one piece of `job`; Ok(true) when finished.
fn work(job: &mut Job, o: &mut Out) -> Result<bool, String> {
    match job.step {
        Step::Zero => {
            say(o, NORMAL, "1/6 clearing the old partition table");
            // the first MiB (MBR, GPT, boot code) and the backup GPT at the end
            let z = vec![0u8; (MIB * 512) as usize];
            wr(job, 0, &z)?;
            wr(job, job.lay.total - 34, &z[..34 * 512])?;
            job.step = Step::Gpt;
        },
        Step::Gpt => {
            say(o, NORMAL, "2/6 writing the GPT partition table");
            let mut parts = vec![
                disks::NewPart { type_guid: disks::TYPE_ESP, uniq: random_guid(), first: job.lay.esp.0, last: job.lay.esp.0 + job.lay.esp.1 - 1, name: disks::ESP_NAME },
                disks::NewPart { type_guid: disks::TYPE_LINGOS, uniq: random_guid(), first: job.lay.ling.0, last: job.lay.ling.0 + job.lay.ling.1 - 1, name: disks::LINGOS_NAME },
            ];
            if let Some((f, c)) = job.lay.data {
                parts.push(disks::NewPart { type_guid: disks::TYPE_BASIC_DATA, uniq: random_guid(), first: f, last: f + c - 1, name: disks::DATA_NAME });
            }
            let (primary, ents, backup_hdr) = disks::build_gpt(job.lay.total, random_guid(), &parts);
            wr(job, 0, &primary)?;
            wr(job, job.lay.total - 33, &ents)?;
            wr(job, job.lay.total - 1, &backup_hdr)?;
            job.step = Step::EspReserved;
        },
        Step::EspReserved => {
            say(o, NORMAL, "3/6 making the EFI partition (FAT32)");
            let f = job.esp;
            let used = job.files.used_end() - 2;
            let mut volid = [0u8; 4];
            crate::crypto::random_bytes(&mut volid);
            let r = f.reserved_area(b"LINGOS EFI ", u32::from_le_bytes(volid), f.clusters - used, job.files.used_end());
            wr(job, f.first, &r)?;
            job.step = Step::EspFat(0);
        },
        Step::EspFat(at) => {
            let f = job.esp;
            let total = 2 * f.fatsz as u64;
            let n = (total - at).min(CHUNK as u64 / 512);
            let files = job.files;
            let v = fat_slice(&f, at, n, |c| files.entry(c));
            wr(job, f.first + f.reserved as u64 + at, &v)?;
            job.step = if at + n >= total { Step::EspDirs } else { Step::EspFat(at + n) };
        },
        Step::EspDirs => {
            let f = job.esp;
            let cb = f.cluster_bytes() as usize;
            let fl = job.files;
            let dir = |entries: &[[u8; 32]]| -> Vec<u8> {
                let mut c = vec![0u8; cb];
                for (i, e) in entries.iter().enumerate() {
                    c[i * 32..i * 32 + 32].copy_from_slice(e);
                }
                c
            };
            let root = dir(&[
                dirent(b"LINGOS EFI ", 0x08, 0, 0, 0),
                dirent(b"EFI        ", 0x10, 0, 3, 0),
                dirent(b"BOOT       ", 0x10, 0x08, 5, 0),
            ]);
            let efi = dir(&[dirent(b".          ", 0x10, 0, 3, 0), dirent(b"..         ", 0x10, 0, 0, 0), dirent(b"BOOT       ", 0x10, 0, 4, 0)]);
            let efiboot = dir(&[
                dirent(b".          ", 0x10, 0, 4, 0),
                dirent(b"..         ", 0x10, 0, 3, 0),
                dirent(b"BOOTX64 EFI", 0x20, 0, fl.efi_c0, job.src.efi_size as u32),
            ]);
            let boot = dir(&[
                dirent(b".          ", 0x10, 0, 5, 0),
                dirent(b"..         ", 0x10, 0, 0, 0),
                dirent(b"WM      ELF", 0x20, 0x18, fl.k_c0, job.src.kernel_size as u32),
            ]);
            let mut all = root;
            all.extend_from_slice(&efi);
            all.extend_from_slice(&efiboot);
            all.extend_from_slice(&boot);
            wr(job, f.cluster_lba(2), &all)?;
            say(o, NORMAL, format!("4/6 copying lingboot ({}) and the kernel ({} MiB)", size_bytes(job.src.efi_size), job.src.kernel_size >> 20));
            job.hasher = Some(::blake3::Hasher::new());
            job.last_tenth = u64::MAX;
            job.step = Step::Copy(0, 0);
        },
        Step::Copy(which, off) => {
            let size = if which == 0 { job.src.efi_size } else { job.src.kernel_size };
            let n = ((size - off) as usize).min(CHUNK);
            let mut buf = core::mem::take(&mut job.buf);
            let ok = job.ranges[which].as_ref().map(|r| r.read_at(off, &mut buf[..n])).unwrap_or(false);
            if !ok {
                job.buf = buf;
                return Err(format!("reading {} from the USB stick failed at {} MiB", if which == 0 { &job.src.efi } else { &job.src.kernel }, off >> 20));
            }
            if let Some(h) = job.hasher.as_mut() {
                h.update(&buf[..n]);
            }
            let padded = n.div_ceil(512) * 512;
            buf[n..padded].fill(0);
            let c0 = if which == 0 { job.files.efi_c0 } else { job.files.k_c0 };
            let lba = job.esp.cluster_lba(c0) + off / 512;
            let r = wr(job, lba, &buf[..padded]);
            job.buf = buf;
            r?;
            let off = off + n as u64;
            if which == 1 {
                progress(job, o, "kernel", off, size);
            }
            if off >= size {
                job.src_hash[which] = *job.hasher.take().unwrap_or_else(::blake3::Hasher::new).finalize().as_bytes();
                if which == 0 {
                    job.hasher = Some(::blake3::Hasher::new());
                    job.step = Step::Copy(1, 0);
                } else {
                    job.ranges = [None, None];
                    job.step = Step::LingZero;
                }
            } else {
                job.step = Step::Copy(which, off);
            }
        },
        Step::LingZero => {
            say(o, NORMAL, "5/6 preparing the LingOS partition and the Data partition");
            // lingfs formats a volume it finds no superblock in: clear both
            // slots' headers (whatever an earlier install left)
            let z = vec![0u8; CHUNK];
            let slot = crate::fs::lingfs::objects::SLOT_SECTORS as u64;
            wr(job, job.lay.ling.0, &z)?;
            if slot + 2048 <= job.lay.ling.1 {
                wr(job, job.lay.ling.0 + slot, &z)?;
            }
            job.step = if job.data.is_some() { Step::DataReserved } else { Step::Flush };
        },
        Step::DataReserved => {
            let f = job.data.unwrap();
            let mut volid = [0u8; 4];
            crate::crypto::random_bytes(&mut volid);
            let r = f.reserved_area(b"LINGOS DATA", u32::from_le_bytes(volid), f.clusters - 1, 3);
            wr(job, f.first, &r)?;
            job.last_tenth = u64::MAX;
            job.step = Step::DataFat(0);
        },
        Step::DataFat(at) => {
            let f = job.data.unwrap();
            let total = 2 * f.fatsz as u64;
            let n = (total - at).min(CHUNK as u64 / 512);
            let v = if at % f.fatsz as u64 == 0 || (at + n) > f.fatsz as u64 && at < f.fatsz as u64 {
                fat_slice(&f, at, n, |c| if c <= 2 { [0x0FFF_FFF8, EOC, EOC][c as usize] } else { 0 })
            } else {
                vec![0u8; (n * 512) as usize]
            };
            wr(job, f.first + f.reserved as u64 + at, &v)?;
            if total > 64 * MIB {
                progress(job, o, "Data partition tables", (at + n) * 512, total * 512);
            }
            job.step = if at + n >= total { Step::DataRoot } else { Step::DataFat(at + n) };
        },
        Step::DataRoot => {
            let f = job.data.unwrap();
            let mut root = vec![0u8; f.cluster_bytes() as usize];
            root[..32].copy_from_slice(&dirent(b"LINGOS DATA", 0x08, 0, 0, 0));
            wr(job, f.cluster_lba(2), &root)?;
            job.step = Step::Flush;
        },
        Step::Flush => {
            disks::flush(job.id).map_err(|_| String::from("the disk did not confirm its cache flush"))?;
            say(o, NORMAL, "6/6 checking what was written");
            job.step = Step::VerifyGpt;
        },
        Step::VerifyGpt => {
            let parts = match disks::table(job.id) {
                Ok(Table::Gpt { parts, .. }) => parts,
                _ => return Err(String::from("check failed: the new GPT does not read back")),
            };
            let want = if job.lay.data.is_some() { 3 } else { 2 };
            if parts.len() != want
                || parts[0].first != job.lay.esp.0
                || parts[1].type_guid != disks::TYPE_LINGOS
                || parts[1].first != job.lay.ling.0
            {
                return Err(String::from("check failed: the GPT reads back different"));
            }
            // the backup header too
            let mut h = [0u8; 512];
            disks::read(job.id, job.lay.total - 1, &mut h).map_err(|_| String::from("cannot read the backup GPT"))?;
            if &h[0..8] != b"EFI PART" {
                return Err(String::from("check failed: no backup GPT header"));
            }
            say(o, DIM, format!("  GPT ok: {} partitions, backup header ok", parts.len()));
            job.step = Step::VerifyOpen;
        },
        Step::VerifyOpen => {
            let mut io = disks::DiskIo(job.id);
            let mut v = fat::Volume::mount(&mut io, job.esp.first as u32).ok_or("check failed: the EFI partition does not mount")?;
            let a = v.open_range(&mut io, "EFI/BOOT/BOOTX64.EFI").map_err(|_| String::from("check failed: \\EFI\\BOOT\\BOOTX64.EFI is missing"))?;
            let b = v.open_range(&mut io, "boot/wm.elf").map_err(|_| String::from("check failed: \\boot\\wm.elf is missing"))?;
            if a.size != job.src.efi_size || b.size != job.src.kernel_size {
                return Err(String::from("check failed: a copied file has the wrong size"));
            }
            if let Some(d) = job.data {
                if fat::Volume::mount(&mut io, d.first as u32).is_none() {
                    return Err(String::from("check failed: the Data partition does not mount"));
                }
            }
            job.verify = Some((v, [Some(a), Some(b)]));
            job.hasher = Some(::blake3::Hasher::new());
            job.last_tenth = u64::MAX;
            job.step = Step::Verify(0, 0);
        },
        Step::Verify(which, off) => {
            let size = if which == 0 { job.src.efi_size } else { job.src.kernel_size };
            let n = ((size - off) as usize).min(CHUNK);
            let mut buf = core::mem::take(&mut job.buf);
            let mut io = disks::DiskIo(job.id);
            let ok = job.verify.as_ref().and_then(|v| v.1[which].as_ref()).map(|r| r.read_at(&mut io, off, &mut buf[..n]).is_ok()).unwrap_or(false);
            if ok {
                if let Some(h) = job.hasher.as_mut() {
                    h.update(&buf[..n]);
                }
            }
            job.buf = buf;
            if !ok {
                return Err(format!("check failed: reading the copy back failed at {} MiB", off >> 20));
            }
            let off = off + n as u64;
            if which == 1 {
                progress(job, o, "reading back", off, size);
            }
            if off >= size {
                let got = *job.hasher.take().unwrap_or_else(::blake3::Hasher::new).finalize().as_bytes();
                if got != job.src_hash[which] {
                    return Err(format!("check failed: {} on the disk differs from the stick's", if which == 0 { "BOOTX64.EFI" } else { "wm.elf" }));
                }
                let hex: String = got[..8].iter().map(|b| format!("{b:02x}")).collect();
                say(o, DIM, format!("  {} matches the stick (BLAKE3 {hex}...)", if which == 0 { "BOOTX64.EFI" } else { "wm.elf" }));
                if which == 0 {
                    job.hasher = Some(::blake3::Hasher::new());
                    job.last_tenth = u64::MAX;
                    job.step = Step::Verify(1, 0);
                } else {
                    job.verify = None;
                    job.step = Step::Done;
                }
            } else {
                job.step = Step::Verify(which, off);
            }
        },
        Step::Done => return Ok(true),
    }
    Ok(false)
}

// ── update-boot ─────────────────────────────────────────────────────────────

/// `update-boot`: on an installed LingOS, replace the boot files in the
/// system disk's "LingOS EFI" partition with the stick's newer ones
/// (\boot\wm.elf and \EFI\BOOT\BOOTX64.EFI). Nothing else is touched: the
/// accounts, files and Data partition stay. Each file is written under a new
/// name first and read back; only a matching copy replaces the old one, so a
/// failed update leaves the previous kernel bootable.
pub fn update_boot() -> Out {
    let mut o = Out::new();
    if running() || awaiting() {
        say(&mut o, ERR, "update-boot: an install is in progress");
        return o;
    }
    if !crate::fs::blockdev::installed_boot() {
        say(&mut o, ERR, "update-boot: this LingOS was not started from an installed disk (boot it from the disk, with the stick plugged in)");
        return o;
    }
    let Some(sys) = crate::fs::blockdev::system_disk() else {
        say(&mut o, ERR, "update-boot: no system disk");
        return o;
    };
    let DiskId::Sata(port) = sys else {
        say(&mut o, ERR, "update-boot: only a SATA system disk is supported");
        return o;
    };
    let Some(src) = find_source() else {
        say(&mut o, ERR, "update-boot: no LingOS boot files on a USB stick (efi/boot/bootx64.efi + boot/wm.elf)");
        return o;
    };
    let esp = match disks::table(sys) {
        Ok(Table::Gpt { parts, .. }) => parts.into_iter().find(|p| p.type_guid == disks::TYPE_ESP && p.name.trim() == disks::ESP_NAME),
        _ => None,
    };
    let Some(esp) = esp else {
        say(&mut o, ERR, "update-boot: the system disk has no \"LingOS EFI\" partition");
        return o;
    };
    if esp.first + esp.count > u32::MAX as u64 {
        say(&mut o, ERR, "update-boot: the EFI partition is beyond 2 TiB");
        return o;
    }
    // a write window over exactly the ESP, closed again below
    let saved = crate::drivers::ahci::write_windows(port as usize);
    crate::drivers::ahci::add_write_window(port as usize, esp.first, esp.first + esp.count);
    let r = update_boot_files(&mut o, sys, esp.first as u32, &src);
    crate::fs::media::unmount_name("lingos-efi");
    crate::drivers::ahci::clear_write_windows(port as usize);
    for (a, b) in saved {
        crate::drivers::ahci::add_write_window(port as usize, a, b);
    }
    match r {
        Ok(()) => say(&mut o, OK, "update-boot: done -- restart to run the new kernel"),
        Err(e) => say(&mut o, ERR, format!("update-boot: {e} -- the previous boot files are still in place")),
    }
    o
}

fn update_boot_files(o: &mut Out, sys: DiskId, esp_lba: u32, src: &Source) -> Result<(), String> {
    use crate::fs::media;
    media::mount_part_keep(media::Device::Disk(sys), "lingos-efi", esp_lba).map_err(|_| String::from("the EFI partition has no FAT volume"))?;
    // temporary names stay 8.3 (the ESP's FAT is read by firmware too)
    let d = "media/lingos-efi";
    for (from, to, new, old, what) in [
        (&src.kernel, format!("{d}/boot/wm.elf"), format!("{d}/boot/WM.NEW"), format!("{d}/boot/WM.OLD"), "kernel"),
        (&src.efi, format!("{d}/EFI/BOOT/BOOTX64.EFI"), format!("{d}/EFI/BOOT/BOOTX64.NEW"), format!("{d}/EFI/BOOT/BOOTX64.OLD"), "lingboot"),
    ] {
        let to = to.as_str();
        // streamed in 1 MiB pieces: the kernel is ~140 MiB and two whole
        // copies in the heap ran it out of memory
        let src_f = media::open_range(from).ok_or_else(|| format!("cannot read {from}"))?;
        let size = src_f.size();
        let _ = media::delete(&new);
        let mut out = media::file_create(&new).ok_or_else(|| format!("cannot create the new {what}"))?;
        let mut buf = vec![0u8; 1 << 20];
        let mut h_src = ::blake3::Hasher::new();
        let mut off = 0u64;
        while off < size {
            let n = core::cmp::min(buf.len() as u64, size - off) as usize;
            if !src_f.read_at(off, &mut buf[..n]) {
                media::file_abort(out);
                return Err(format!("cannot read {from}"));
            }
            h_src.update(&buf[..n]);
            if !media::file_write(&mut out, &buf[..n]) {
                media::file_abort(out);
                return Err(format!("writing the new {what} failed"));
            }
            off += n as u64;
        }
        drop(src_f);
        if !media::file_finish(out) {
            return Err(format!("writing the new {what} failed"));
        }
        let want = h_src.finalize();
        let back = media::open_range(&new).ok_or_else(|| format!("cannot read back the new {what}"))?;
        let mut h_back = ::blake3::Hasher::new();
        let mut off = 0u64;
        let ok = back.size() == size
            && loop {
                if off >= size {
                    break true;
                }
                let n = core::cmp::min(buf.len() as u64, size - off) as usize;
                if !back.read_at(off, &mut buf[..n]) {
                    break false;
                }
                h_back.update(&buf[..n]);
                off += n as u64;
            };
        drop(back);
        if !ok || h_back.finalize() != want {
            let _ = media::delete(&new);
            return Err(format!("the new {what} read back different"));
        }
        let _ = media::delete(&old);
        if media::exists(to) && !media::rename(to, &old) {
            let _ = media::delete(&new);
            return Err(format!("could not set the old {what} aside"));
        }
        if !media::rename(&new, to) {
            let _ = media::rename(&old, to);
            return Err(format!("could not put the new {what} in place"));
        }
        let _ = media::delete(&old);
        say(o, DIM, format!("  {what}: {size} bytes written and checked (BLAKE3 {})", &want.to_hex()[..16]));
    }
    Ok(())
}

/// Called every frame by the Terminal: run the install a slice at a time.
pub fn pump() -> Out {
    let mut o = Vec::new();
    let State::Running(job) = state() else { return o };
    let t0 = crate::arch::timer::now_ms();
    loop {
        match work(job, &mut o) {
            Ok(false) => {},
            Ok(true) => {
                close_windows(job);
                let secs = (crate::arch::timer::now_ms() - job.started) / 1000;
                say(&mut o, OK, format!("LingOS is installed on {} ({} s).", job.label, secs));
                say(&mut o, OK, "Restart, remove the USB stick, and pick this disk in the firmware boot menu");
                say(&mut o, OK, "(F8 on ASUS boards; F12, F11 or Esc on others) -- it is listed as a UEFI disk.");
                if job.released {
                    say(&mut o, ERR, "This session's files were in RAM since the install began: restart now.");
                }
                *state() = State::Idle;
                return o;
            },
            Err(e) => {
                close_windows(job);
                say(&mut o, ERR, format!("install FAILED: {e}"));
                say(&mut o, ERR, "The disk is not bootable now. Run install-disk again to retry.");
                *state() = State::Idle;
                return o;
            },
        }
        if crate::arch::timer::now_ms().wrapping_sub(t0) >= STEP_BUDGET_MS {
            break;
        }
    }
    o
}

/// `install-disk [n]` from the Terminal.
pub fn command(arg: &str) -> Out {
    let mut o = Vec::new();
    if running() {
        say(&mut o, ERR, "an install is running -- wait for it to finish");
        return o;
    }
    *state() = State::Idle;
    if arg.trim().is_empty() {
        list(&mut o);
    } else {
        prepare(&mut o, arg);
    }
    o
}
