//! Removable and extra volumes under `media/<name>/`: a USB flash drive on the
//! Pi (`drivers::usbmsd`; mounted when it is plugged in, unmounted when it is
//! pulled) and a second ATA disk on x86 (`media/disk1`). FAT16/32 through
//! `fs::fat`, exFAT (files over 4 GiB) through `fs::exfat`; programs reach
//! them through `crate::vfs` like any other path.

use crate::fs::exfat;
use crate::fs::fat::{self, Disk, SECTOR};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq)]
pub enum Device {
    Usb,
    AtaSlave,
    /// A FAT partition on the Pi's own SD card (the data partition).
    SdCard,
    /// x86: a FAT partition on a whole disk (the installed system's
    /// "LingOS Data" partition; the disk installer's new ESP while it checks it).
    #[cfg(target_arch = "x86_64")]
    Disk(crate::fs::disks::DiskId),
}

struct DevIo(Device);

impl Disk for DevIo {
    fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), ()> {
        if self.0 == Device::SdCard {
            #[cfg(target_arch = "aarch64")]
            return crate::arch::sdcard::read_sectors(lba, buf);
            #[allow(unreachable_code)]
            return Err(());
        }
        match self.0 {
            Device::Usb => {
                #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
                return crate::drivers::usbmsd::read_sectors(lba, buf);
                #[allow(unreachable_code)]
                {
                    let _ = (lba, buf);
                    Err(())
                }
            },
            #[cfg(target_arch = "x86_64")]
            Device::Disk(id) => crate::fs::disks::read(id, lba as u64, buf),
            Device::AtaSlave => {
                #[cfg(target_arch = "x86_64")]
                return crate::drivers::ata::read_slave(lba, buf);
                #[allow(unreachable_code)]
                {
                    let _ = (lba, buf);
                    Err(())
                }
            },
            Device::SdCard => Err(()),
        }
    }

    fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), ()> {
        if self.0 == Device::SdCard {
            #[cfg(target_arch = "aarch64")]
            {
                let mut s = [0u8; crate::arch::sdcard::SECTOR_SIZE];
                for (i, chunk) in buf.chunks(s.len()).enumerate() {
                    s[..chunk.len()].copy_from_slice(chunk);
                    crate::arch::sdcard::write_sector(lba + i as u32, &s)?;
                }
                return Ok(());
            }
            #[allow(unreachable_code)]
            return Err(());
        }
        match self.0 {
            Device::Usb => {
                #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
                return crate::drivers::usbmsd::write_sectors(lba, buf);
                #[allow(unreachable_code)]
                {
                    let _ = (lba, buf);
                    Err(())
                }
            },
            #[cfg(target_arch = "x86_64")]
            Device::Disk(id) => crate::fs::disks::write(id, lba as u64, buf),
            Device::AtaSlave => {
                #[cfg(target_arch = "x86_64")]
                return crate::drivers::ata::write_slave(lba, buf);
                #[allow(unreachable_code)]
                {
                    let _ = (lba, buf);
                    Err(())
                }
            },
            Device::SdCard => Err(()),
        }
    }
}

/// A mounted volume: FAT16/32 or exFAT (told apart by the boot sector), with
/// one set of operations.
enum Vol {
    Fat(fat::Volume),
    Ex(exfat::Volume),
}

/// A file being streamed onto either kind of volume.
enum Stream {
    Fat(fat::StreamFile),
    Ex(exfat::StreamFile),
}

impl Vol {
    fn open(io: &mut dyn Disk, part: u32) -> Option<Vol> {
        let mut s = [0u8; SECTOR];
        io.read(part, &mut s).ok()?;
        if &s[3..11] == b"EXFAT   " {
            exfat::Volume::mount(io, part).map(Vol::Ex)
        } else {
            fat::Volume::mount(io, part).map(Vol::Fat)
        }
    }

    fn label(&self) -> &str {
        match self {
            Vol::Fat(v) => &v.label,
            Vol::Ex(v) => &v.label,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Vol::Fat(v) if v.is_fat32() => "FAT32",
            Vol::Fat(_) => "FAT16",
            Vol::Ex(v) if v.is_read_only() => "exFAT, TexFAT: read-only",
            Vol::Ex(_) => "exFAT",
        }
    }

    fn list(&mut self, io: &mut dyn Disk, p: &str) -> Result<Vec<(String, bool, usize)>, ()> {
        match self {
            Vol::Fat(v) => v.list(io, p),
            Vol::Ex(v) => v.list(io, p),
        }
    }

    fn read_file(&mut self, io: &mut dyn Disk, p: &str) -> Result<Vec<u8>, ()> {
        match self {
            Vol::Fat(v) => v.read_file(io, p),
            Vol::Ex(v) => v.read_file(io, p),
        }
    }

    fn write_file(&mut self, io: &mut dyn Disk, p: &str, data: &[u8]) -> Result<(), ()> {
        match self {
            Vol::Fat(v) => v.write_file(io, p, data),
            Vol::Ex(v) => v.write_file(io, p, data),
        }
    }

    /// Some(is_dir) when `p` exists.
    fn is_dir(&mut self, io: &mut dyn Disk, p: &str) -> Option<bool> {
        match self {
            Vol::Fat(v) => v.lookup(io, p).ok().flatten().map(|e| e.is_dir),
            Vol::Ex(v) => v.lookup(io, p).ok().flatten().map(|e| e.is_dir),
        }
    }

    fn mkdir(&mut self, io: &mut dyn Disk, p: &str) -> Result<(), ()> {
        match self {
            Vol::Fat(v) => v.mkdir(io, p),
            Vol::Ex(v) => v.mkdir(io, p),
        }
    }

    fn delete(&mut self, io: &mut dyn Disk, p: &str) -> Result<(), ()> {
        match self {
            Vol::Fat(v) => v.delete(io, p),
            Vol::Ex(v) => v.delete(io, p),
        }
    }

    fn rename(&mut self, io: &mut dyn Disk, from: &str, to: &str) -> Result<(), ()> {
        match self {
            Vol::Fat(v) => v.rename(io, from, to),
            Vol::Ex(v) => v.rename(io, from, to),
        }
    }

    fn space(&mut self, io: &mut dyn Disk) -> (u64, u64) {
        match self {
            Vol::Fat(v) => v.space(io),
            Vol::Ex(v) => v.space(io),
        }
    }

    fn open_range(&mut self, io: &mut dyn Disk, p: &str) -> Result<fat::RangeFile, ()> {
        match self {
            Vol::Fat(v) => v.open_range(io, p),
            Vol::Ex(v) => v.open_range(io, p),
        }
    }

    fn max_file(&self) -> u64 {
        match self {
            Vol::Fat(_) => fat::MAX_FILE,
            Vol::Ex(v) => v.max_file(),
        }
    }

    fn stream_begin(&self) -> Stream {
        match self {
            Vol::Fat(v) => Stream::Fat(v.stream_begin()),
            Vol::Ex(v) => Stream::Ex(v.stream_begin()),
        }
    }

    fn stream_write(&mut self, io: &mut dyn Disk, f: &mut Stream, data: &[u8]) -> Result<(), ()> {
        match (self, f) {
            (Vol::Fat(v), Stream::Fat(f)) => v.stream_write(io, f, data),
            (Vol::Ex(v), Stream::Ex(f)) => v.stream_write(io, f, data),
            _ => Err(()),
        }
    }

    fn stream_finish(&mut self, io: &mut dyn Disk, f: Stream, p: &str) -> Result<(), ()> {
        match (self, f) {
            (Vol::Fat(v), Stream::Fat(f)) => v.stream_finish(io, f, p),
            (Vol::Ex(v), Stream::Ex(f)) => v.stream_finish(io, f, p),
            _ => Err(()),
        }
    }

    fn stream_abort(&mut self, io: &mut dyn Disk, f: Stream) {
        match (self, f) {
            (Vol::Fat(v), Stream::Fat(f)) => v.stream_abort(io, f),
            (Vol::Ex(v), Stream::Ex(f)) => v.stream_abort(io, f),
            _ => {},
        }
    }
}

struct Mount {
    name: String,
    dev: Device,
    vol: Vol,
}

static mut MOUNTS: Vec<Mount> = Vec::new();

fn mounts() -> &'static mut Vec<Mount> {
    unsafe { &mut *(&raw mut MOUNTS) }
}

fn le16(b: &[u8], o: usize) -> u32 {
    b[o] as u32 | ((b[o + 1] as u32) << 8)
}

/// Mount the device's FAT volume (first partition of its MBR, or the whole
/// device when it is formatted without one) as `media/<name>`. Returns the
/// volume label.
pub fn mount(dev: Device, name: &str) -> Result<String, ()> {
    unmount(dev);
    let mut io = DevIo(dev);
    let mut s0 = [0u8; SECTOR];
    io.read(0, &mut s0)?;
    if s0[510] != 0x55 || s0[511] != 0xAA {
        return Err(());
    }
    let looks_bpb = ((s0[0] == 0xEB || s0[0] == 0xE9) && le16(&s0, 11) == 512 && s0[13] != 0) || &s0[3..11] == b"EXFAT   ";
    let mut part = 0u32;
    if !looks_bpb {
        for i in 0..4 {
            let e = &s0[446 + i * 16..446 + (i + 1) * 16];
            if e[4] != 0 {
                let start = le16(e, 8) | (le16(e, 10) << 16);
                if start > 0 {
                    part = start;
                    break;
                }
            }
        }
        if part == 0 {
            return Err(());
        }
    }
    let vol = Vol::open(&mut io, part).ok_or(())?;
    let label = if vol.label().is_empty() { String::from(name) } else { String::from(vol.label()) };
    let fat = vol.kind();
    crate::console_write(format!("media: {label} ({fat}) mounted as media/{name}\n").as_bytes());
    mounts().push(Mount { name: String::from(name), dev, vol });
    Ok(label)
}

/// Mount the FAT / exFAT volume that starts at sector `part_lba` of `dev`.
pub fn mount_part(dev: Device, name: &str, part_lba: u32) -> Result<String, ()> {
    unmount(dev);
    let mut io = DevIo(dev);
    let vol = Vol::open(&mut io, part_lba).ok_or(())?;
    let label = if vol.label().is_empty() { String::from(name) } else { String::from(vol.label()) };
    let fat = vol.kind();
    crate::console_write(format!("media: {label} ({fat}) mounted as media/{name}\n").as_bytes());
    mounts().push(Mount { name: String::from(name), dev, vol });
    Ok(label)
}

/// Like `mount_part`, but other volumes of the same device stay mounted
/// (update-boot opens the system disk's ESP next to its media/data).
#[cfg(target_arch = "x86_64")]
pub fn mount_part_keep(dev: Device, name: &str, part_lba: u32) -> Result<String, ()> {
    if has(name) {
        return Err(());
    }
    let mut io = DevIo(dev);
    let vol = Vol::open(&mut io, part_lba).ok_or(())?;
    let label = if vol.label().is_empty() { String::from(name) } else { String::from(vol.label()) };
    mounts().push(Mount { name: String::from(name), dev, vol });
    Ok(label)
}

/// Forget the volume mounted under `name` (only that one).
pub fn unmount_name(name: &str) -> bool {
    let before = mounts().len();
    mounts().retain(|m| m.name != name);
    mounts().len() != before
}

/// Is a volume mounted under this name?
pub fn has(name: &str) -> bool {
    mounts().iter().any(|m| m.name == name)
}

/// `mkdir -p` on a media path.
pub fn mkdir_p(path: &str) -> bool {
    let p = path.trim_matches('/');
    let Some(rest) = p.strip_prefix("media/") else { return false };
    let mut parts = rest.split('/');
    let Some(volname) = parts.next() else { return false };
    let mut cur = format!("media/{volname}");
    for seg in parts {
        if seg.is_empty() {
            continue;
        }
        cur.push('/');
        cur.push_str(seg);
        if !is_dir(&cur) && !mkdir(&cur) {
            return false;
        }
    }
    true
}

/// Forget a device's volume (it was unplugged). Nothing is written: every
/// change was written through when it was made.
pub fn unmount(dev: Device) -> bool {
    let before = mounts().len();
    mounts().retain(|m| m.dev != dev);
    let gone = mounts().len() != before;
    if gone {
        crate::console_write(b"media: volume unmounted\n");
    }
    gone
}

pub fn is_mounted(dev: Device) -> bool {
    mounts().iter().any(|m| m.dev == dev)
}

pub fn names() -> Vec<String> {
    mounts().iter().map(|m| m.name.clone()).collect()
}

/// Is `path` under `media`?
pub fn is_media_path(path: &str) -> bool {
    let p = path.trim_start_matches('/');
    p == "media" || p.starts_with("media/")
}

/// `media/<name>/rest` -> (mount index, rest).
fn split(path: &str) -> Option<(usize, String)> {
    let p = path.trim_matches('/');
    let rest = p.strip_prefix("media/")?;
    let (name, inner) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    let i = mounts().iter().position(|m| m.name == name)?;
    Some((i, String::from(inner)))
}

fn with<R>(path: &str, f: impl FnOnce(&mut Vol, &mut DevIo, &str) -> R) -> Option<R> {
    let (i, rest) = split(path)?;
    let m = &mut mounts()[i];
    let mut io = DevIo(m.dev);
    Some(f(&mut m.vol, &mut io, &rest))
}

pub fn list(path: &str) -> Vec<(String, bool, usize)> {
    let p = path.trim_matches('/');
    if p == "media" {
        return mounts().iter().map(|m| (m.name.clone(), true, 0)).collect();
    }
    with(p, |v, io, rest| v.list(io, rest).unwrap_or_default()).unwrap_or_default()
}

pub fn read(path: &str) -> Option<Vec<u8>> {
    with(path, |v, io, rest| v.read_file(io, rest).ok()).flatten()
}

pub fn write(path: &str, data: &[u8]) -> bool {
    with(path, |v, io, rest| v.write_file(io, rest, data).is_ok()).unwrap_or(false)
}

pub fn exists(path: &str) -> bool {
    let p = path.trim_matches('/');
    if p == "media" {
        return !mounts().is_empty();
    }
    with(p, |v, io, rest| v.is_dir(io, rest).is_some()).unwrap_or(false)
}

pub fn is_dir(path: &str) -> bool {
    let p = path.trim_matches('/');
    if p == "media" {
        return !mounts().is_empty();
    }
    with(p, |v, io, rest| v.is_dir(io, rest) == Some(true)).unwrap_or(false)
}

pub fn mkdir(path: &str) -> bool {
    with(path, |v, io, rest| v.mkdir(io, rest).is_ok()).unwrap_or(false)
}

pub fn delete(path: &str) -> bool {
    with(path, |v, io, rest| v.delete(io, rest).is_ok()).unwrap_or(false)
}

/// Rename / move inside one volume (across volumes the caller copies).
pub fn rename(from: &str, to: &str) -> bool {
    let (Some((a, ra)), Some((b, rb))) = (split(from), split(to)) else { return false };
    if a != b {
        return false;
    }
    let m = &mut mounts()[a];
    let mut io = DevIo(m.dev);
    m.vol.rename(&mut io, &ra, &rb).is_ok()
}

/// "label, used/total" for a console status line.
pub fn status() -> String {
    let mut s = String::new();
    for m in mounts().iter_mut() {
        let mut io = DevIo(m.dev);
        let (total, free) = m.vol.space(&mut io);
        s.push_str(&format!("media/{}: {} ({}) -- {} MiB free of {} MiB\n", m.name, m.vol.label(), m.vol.kind(), free >> 20, total >> 20));
    }
    if s.is_empty() {
        s.push_str("media: nothing mounted\n");
    }
    s
}

/// x86: a FAT-formatted second ATA disk (QEMU `-drive ...,index=1`, a second
/// IDE/SATA disk) becomes media/disk1 at boot.
#[cfg(target_arch = "x86_64")]
pub fn probe_x86() {
    if crate::drivers::ata::slave_sectors().is_some() {
        let _ = mount(Device::AtaSlave, "disk1");
    }
    // an installed system (`install-disk`): its "LingOS Data" partition
    mount_data();
}

/// Mount the system disk's "LingOS Data" FAT partition as media/data.
#[cfg(target_arch = "x86_64")]
pub fn mount_data() {
    if has("data") {
        return;
    }
    if let Some((id, first, _)) = crate::fs::blockdev::data_partition() {
        if mount_part(Device::Disk(id), "data", first as u32).is_err() {
            crate::console_write(b"media: the LingOS Data partition has no FAT or exFAT volume\n");
        }
    }
    // Data partitions on the other disks: media/lingdata, media/lingdata2, ...
    for (k, (id, first, _)) in crate::fs::blockdev::extra_data_partitions().into_iter().enumerate() {
        let name = if k == 0 { String::from("lingdata") } else { format!("lingdata{}", k + 1) };
        if has(&name) {
            continue;
        }
        // mount_part unmounts the device first: only mount a disk not mounted yet
        if is_mounted(Device::Disk(id)) {
            continue;
        }
        if mount_part(Device::Disk(id), &name, first as u32).is_err() {
            crate::console_write(format!("media: the Data partition for media/{name} has no FAT or exFAT volume\n").as_bytes());
        }
    }
}

/// `(total, free)` bytes of the volume holding `path` (`media/<name>/...`).
/// One pass over the FAT, so call it once per job, not per file.
pub fn space(path: &str) -> Option<(u64, u64)> {
    with(path, |v, io, _| v.space(io))
}

/// The largest file the volume holding `path` takes: 4 GiB - 1 on FAT,
/// the volume's size on exFAT; 0 when nothing is mounted there.
pub fn max_file(path: &str) -> u64 {
    with(path, |v, _, _| v.max_file()).unwrap_or(0)
}

/// A file being streamed onto a volume (see `fat::StreamFile`, `exfat::StreamFile`).
pub struct MediaFile {
    path: String,
    f: Option<Stream>,
}

/// Start streaming a new file at `path` (its folder must exist).
pub fn file_create(path: &str) -> Option<MediaFile> {
    let f = with(path, |v, _, _| v.stream_begin())?;
    Some(MediaFile { path: String::from(path.trim_matches('/')), f: Some(f) })
}

/// Append to a streamed file; false when the disk is full or (FAT) it passes 4 GiB.
pub fn file_write(m: &mut MediaFile, data: &[u8]) -> bool {
    let Some(f) = m.f.as_mut() else { return false };
    with(&m.path, |v, io, _| v.stream_write(io, f, data).is_ok()).unwrap_or(false)
}

/// Finish a streamed file (its directory entry appears now).
pub fn file_finish(mut m: MediaFile) -> bool {
    let Some(f) = m.f.take() else { return false };
    let path = m.path.clone();
    with(&path, |v, io, rest| v.stream_finish(io, f, rest).is_ok()).unwrap_or(false)
}

/// Abandon a streamed file, freeing what it had written.
pub fn file_abort(mut m: MediaFile) {
    if let Some(f) = m.f.take() {
        let path = m.path.clone();
        with(&path, |v, io, _| v.stream_abort(io, f));
    }
}

/// Delete `path` and everything under it.
pub fn delete_tree(path: &str) -> bool {
    if is_dir(path) {
        for (name, _, _) in list(path) {
            delete_tree(&format!("{}/{}", path.trim_end_matches('/'), name));
        }
    }
    delete(path)
}

/// A file on a volume opened for random-access reads (a language model's
/// weights, read in pieces): see `fat::RangeFile` (exFAT files use it too).
pub struct MediaRange {
    name: String,
    dev: Device,
    f: crate::fs::fat::RangeFile,
}

/// Open `media/<name>/...` for [`range_read`]; None when it is not a file.
pub fn open_range(path: &str) -> Option<MediaRange> {
    let (i, rest) = split(path)?;
    let m = &mut mounts()[i];
    let mut io = DevIo(m.dev);
    let f = m.vol.open_range(&mut io, &rest).ok()?;
    Some(MediaRange { name: m.name.clone(), dev: m.dev, f })
}

impl MediaRange {
    pub fn size(&self) -> u64 {
        self.f.size
    }

    /// The bytes at `offset..offset + buf.len()`; fails once the volume is
    /// gone (a USB stick pulled).
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> bool {
        if !mounts().iter().any(|m| m.name == self.name && m.dev == self.dev) {
            return false;
        }
        let mut io = DevIo(self.dev);
        self.f.read_at(&mut io, offset, buf).is_ok()
    }
}
