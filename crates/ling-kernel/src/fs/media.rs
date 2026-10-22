//! Removable and extra volumes under `media/<name>/`: a USB flash drive on the
//! Pi (`drivers::usbmsd`; mounted when it is plugged in, unmounted when it is
//! pulled) and a second ATA disk on x86 (`media/disk1`). FAT16/32 through
//! `fs::fat`; programs reach them through `crate::vfs` like any other path.

use crate::fs::fat::{Disk, Volume, SECTOR};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq)]
pub enum Device {
    Usb,
    AtaSlave,
}

struct DevIo(Device);

impl Disk for DevIo {
    fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), ()> {
        match self.0 {
            Device::Usb => {
                #[cfg(target_arch = "aarch64")]
                return crate::drivers::usbmsd::read_sectors(lba, buf);
                #[allow(unreachable_code)]
                {
                    let _ = (lba, buf);
                    Err(())
                }
            },
            Device::AtaSlave => {
                #[cfg(target_arch = "x86_64")]
                return crate::drivers::ata::read_slave(lba, buf);
                #[allow(unreachable_code)]
                {
                    let _ = (lba, buf);
                    Err(())
                }
            },
        }
    }

    fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), ()> {
        match self.0 {
            Device::Usb => {
                #[cfg(target_arch = "aarch64")]
                return crate::drivers::usbmsd::write_sectors(lba, buf);
                #[allow(unreachable_code)]
                {
                    let _ = (lba, buf);
                    Err(())
                }
            },
            Device::AtaSlave => {
                #[cfg(target_arch = "x86_64")]
                return crate::drivers::ata::write_slave(lba, buf);
                #[allow(unreachable_code)]
                {
                    let _ = (lba, buf);
                    Err(())
                }
            },
        }
    }
}

struct Mount {
    name: String,
    dev: Device,
    vol: Volume,
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
    let looks_bpb = (s0[0] == 0xEB || s0[0] == 0xE9) && le16(&s0, 11) == 512 && s0[13] != 0;
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
    let vol = Volume::mount(&mut io, part).ok_or(())?;
    let label = if vol.label.is_empty() { String::from(name) } else { vol.label.clone() };
    let fat = if vol.is_fat32() { "FAT32" } else { "FAT16" };
    crate::console_write(format!("media: {label} ({fat}) mounted as media/{name}\n").as_bytes());
    mounts().push(Mount { name: String::from(name), dev, vol });
    Ok(label)
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

fn with<R>(path: &str, f: impl FnOnce(&mut Volume, &mut DevIo, &str) -> R) -> Option<R> {
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
    with(p, |v, io, rest| matches!(v.lookup(io, rest), Ok(Some(_)))).unwrap_or(false)
}

pub fn is_dir(path: &str) -> bool {
    let p = path.trim_matches('/');
    if p == "media" {
        return !mounts().is_empty();
    }
    with(p, |v, io, rest| matches!(v.lookup(io, rest), Ok(Some(e)) if e.is_dir)).unwrap_or(false)
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
        s.push_str(&format!("media/{}: {} -- {} MiB free of {} MiB\n", m.name, m.vol.label, free >> 20, total >> 20));
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
}
