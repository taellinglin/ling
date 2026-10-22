//! USB mass storage -- a flash drive -- as a block device: the Bulk-Only
//! Transport (CBW / data / CSW) carrying SCSI commands over the xHCI driver's
//! bulk endpoints (`arch::xhci::msd_*`). Plugging a drive in makes the xHCI
//! driver call [`attach`], which identifies the drive, waits for it to be
//! ready, reads its capacity and mounts its FAT volume as media/usb0
//! (`fs::media`); pulling it calls [`detach`], which unmounts it.

use crate::arch::xhci;
use crate::console_write as out;

static mut TAG: u32 = 1;
static mut BLOCKS: u32 = 0;
static mut READY: bool = false;

pub fn ready() -> bool {
    unsafe { READY }
}

/// Capacity in 512-byte sectors (0 without a drive).
pub fn sectors() -> u32 {
    unsafe { BLOCKS }
}

fn dec(label: &[u8], mut v: u64, tail: &[u8]) {
    out(label);
    let mut b = [0u8; 20];
    let mut i = b.len();
    if v == 0 {
        i -= 1;
        b[i] = b'0';
    }
    while v > 0 {
        i -= 1;
        b[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    out(&b[i..]);
    out(tail);
}

fn cbw(cmd: &[u8], len: u32, dir_in: bool) -> [u8; 31] {
    let mut w = [0u8; 31];
    w[0..4].copy_from_slice(&0x4342_5355u32.to_le_bytes());
    let tag = unsafe {
        TAG = TAG.wrapping_add(1);
        TAG
    };
    w[4..8].copy_from_slice(&tag.to_le_bytes());
    w[8..12].copy_from_slice(&len.to_le_bytes());
    w[12] = if dir_in { 0x80 } else { 0 };
    w[13] = 0; // LUN 0
    w[14] = cmd.len() as u8;
    w[15..15 + cmd.len()].copy_from_slice(cmd);
    w
}

/// One SCSI command: CBW, an optional data phase in or out, CSW. Returns the
/// CSW status (0 passed, 1 failed -- then REQUEST SENSE says why).
fn command(cmd: &[u8], data_in: Option<&mut [u8]>, data_out: Option<&[u8]>) -> Result<u8, ()> {
    let len = data_in.as_ref().map(|b| b.len()).or(data_out.map(|b| b.len())).unwrap_or(0) as u32;
    let dir_in = data_in.is_some();
    let w = cbw(cmd, len, dir_in);
    if xhci::msd_bulk_out(&w).is_err() {
        xhci::msd_clear_stall(false);
        return Err(());
    }
    if let Some(buf) = data_in {
        if let Err(cc) = xhci::msd_bulk_in(buf) {
            if cc == 6 {
                xhci::msd_clear_stall(true);
            } else {
                return Err(());
            }
        }
    } else if let Some(buf) = data_out {
        if let Err(cc) = xhci::msd_bulk_out(buf) {
            if cc == 6 {
                xhci::msd_clear_stall(false);
            } else {
                return Err(());
            }
        }
    }
    let mut csw = [0u8; 13];
    let mut got = xhci::msd_bulk_in(&mut csw);
    if got.is_err() {
        xhci::msd_clear_stall(true);
        got = xhci::msd_bulk_in(&mut csw);
    }
    let n = got.map_err(|_| ())?;
    if n < 13 || u32::from_le_bytes([csw[0], csw[1], csw[2], csw[3]]) != 0x5342_5355 {
        return Err(());
    }
    let status = csw[12];
    if status == 2 {
        // Phase error: the class reset, then both endpoints.
        xhci::msd_reset();
        xhci::msd_clear_stall(true);
        xhci::msd_clear_stall(false);
        return Err(());
    }
    Ok(status)
}

fn request_sense() -> Option<(u8, u8, u8)> {
    let mut sense = [0u8; 18];
    if command(&[0x03, 0, 0, 0, 18, 0], Some(&mut sense), None).ok()? == 0 {
        Some((sense[2] & 0xF, sense[12], sense[13]))
    } else {
        None
    }
}

/// The xHCI driver found a mass-storage device: bring it up and mount it.
pub fn attach() {
    unsafe {
        READY = false;
        BLOCKS = 0;
    }
    out(b"usb storage: ");
    let mut inq = [0u8; 36];
    match command(&[0x12, 0, 0, 0, 36, 0], Some(&mut inq), None) {
        Ok(0) => {
            let trim = |b: &[u8]| -> alloc::string::String { alloc::string::String::from_utf8_lossy(b).trim().into() };
            out(trim(&inq[8..16]).as_bytes());
            out(b" ");
            out(trim(&inq[16..32]).as_bytes());
            out(b"\n");
        },
        _ => out(b"INQUIRY failed\n"),
    }
    let mut ready = false;
    for _ in 0..30 {
        if command(&[0, 0, 0, 0, 0, 0], None, None) == Ok(0) {
            ready = true;
            break;
        }
        let _ = request_sense(); // clears "unit attention" / "becoming ready"
        crate::arch::timer::delay_us(100_000);
    }
    if !ready {
        out(b"usb storage: the drive never reported ready\n");
        return;
    }
    let mut cap = [0u8; 8];
    for _ in 0..3 {
        if command(&[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], Some(&mut cap), None) == Ok(0) {
            break;
        }
        let _ = request_sense();
    }
    let last = u32::from_be_bytes([cap[0], cap[1], cap[2], cap[3]]);
    let bsize = u32::from_be_bytes([cap[4], cap[5], cap[6], cap[7]]);
    if bsize != 512 || last == 0 {
        dec(b"usb storage: unsupported block size ", bsize as u64, b"\n");
        return;
    }
    unsafe {
        BLOCKS = last.wrapping_add(1);
        READY = true;
    }
    dec(b"usb storage: ", (last as u64 + 1) / 2048, b" MiB\n");
    if crate::fs::media::mount(crate::fs::media::Device::Usb, "usb0").is_err() {
        out(b"usb storage: no FAT volume found on the drive (FAT16/FAT32 are supported)\n");
    }
}

/// The drive was unplugged.
pub fn detach() {
    unsafe {
        READY = false;
        BLOCKS = 0;
    }
    crate::fs::media::unmount(crate::fs::media::Device::Usb);
    out(b"usb storage: drive removed\n");
}

fn rw(write: bool, lba: u32, buf: &mut [u8]) -> Result<(), ()> {
    if !ready() || buf.len() % 512 != 0 {
        return Err(());
    }
    let mut at = lba;
    for chunk in buf.chunks_mut(4096) {
        let n = (chunk.len() / 512) as u32;
        let cmd = [if write { 0x2A } else { 0x28 }, 0, (at >> 24) as u8, (at >> 16) as u8, (at >> 8) as u8, at as u8, 0, (n >> 8) as u8, n as u8, 0];
        let mut ok = false;
        for _ in 0..3 {
            let r = if write { command(&cmd, None, Some(chunk)) } else { command(&cmd, Some(chunk), None) };
            if r == Ok(0) {
                ok = true;
                break;
            }
            let _ = request_sense();
        }
        if !ok {
            return Err(());
        }
        at += n;
    }
    Ok(())
}

pub fn read_sectors(lba: u32, buf: &mut [u8]) -> Result<(), ()> {
    rw(false, lba, buf)
}

pub fn write_sectors(lba: u32, buf: &[u8]) -> Result<(), ()> {
    let mut tmp = alloc::vec::Vec::from(buf);
    rw(true, lba, &mut tmp)
}

/// Console `usb`: what is attached and mounted.
pub fn status() {
    if ready() {
        dec(b"usb storage: drive present, ", sectors() as u64 / 2048, b" MiB\n");
    } else {
        out(b"usb storage: no drive\n");
    }
    out(crate::fs::media::status().as_bytes());
}
