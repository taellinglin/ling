//! AHCI (SATA) disk driver — the modern block-storage interface most
//! current hardware and hypervisors default to (VirtualBox's default disk
//! controller is SATA/AHCI, not legacy IDE — confirmed as the actual cause
//! of `lingfs` working in QEMU, where the test disk was explicitly
//! attached via legacy IDE, but not in a VirtualBox VM whose disk was on
//! SATA: `ata.rs`'s PIO driver can only ever see an IDE-attached disk).
//!
//! Found via PCI (class 0x01 mass-storage, subclass 0x06 SATA, prog-if
//! 0x01 AHCI) rather than a fixed port range, since its MMIO register
//! window (the "ABAR") is a PCI BAR assigned by firmware. Every SATA disk on
//! the controller is brought up (a PC has several ports: an SSD and an HDD,
//! say); the first one is the "primary" disk the old single-disk API
//! (`read_sector` / `write_sector`) means. One command slot per port, polled,
//! LBA48.
//!
//! **What LingOS may write.** Each disk has write windows (sector ranges); a
//! write outside them is refused here, whatever the caller. `fs::blockdev`
//! opens the LingOS partition (or the whole disk when it is LingOS's own /
//! blank); the disk installer opens the one disk the user confirmed and
//! closes it again when done. With no window open nothing is written.
use crate::arch::pci;
use alloc::string::String;
use alloc::vec::Vec;
use core::ptr;

pub const SECTOR_SIZE: usize = 512;

// HBA (host bus adapter) generic registers, byte offsets from ABAR.
const GHC: usize = 0x04;
const PI: usize = 0x0C;
const PORT_BASE: usize = 0x100;
const PORT_SIZE: usize = 0x80;

// Per-port register byte offsets, from PORT_BASE + port*PORT_SIZE.
const PX_CLB: usize = 0x00;
const PX_CLBU: usize = 0x04;
const PX_FB: usize = 0x08;
const PX_FBU: usize = 0x0C;
const PX_IS: usize = 0x10;
const PX_CMD: usize = 0x18;
const PX_TFD: usize = 0x20;
const PX_SIG: usize = 0x24;
const PX_SSTS: usize = 0x28;
const PX_SCTL: usize = 0x2C;
const PX_SERR: usize = 0x30;
const PX_CI: usize = 0x38;

const CMD_ST: u32 = 1 << 0;
const CMD_FRE: u32 = 1 << 4;
const CMD_FR: u32 = 1 << 14;
const CMD_CR: u32 = 1 << 15;

const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;
const ATA_CMD_FLUSH_CACHE_EXT: u8 = 0xEA;
const ATA_CMD_IDENTIFY: u8 = 0xEC;
const FIS_TYPE_REG_H2D: u8 = 0x27;

const SATA_SIG_ATA: u32 = 0x0000_0101;

/// Sectors one multi-sector command moves (the 64 KiB bounce buffer).
pub const MAX_RUN: usize = 128;

/// One SATA disk.
struct Port {
    /// AHCI port number (what UEFI's `Sata(port, ...)` device path names)
    port: usize,
    /// one 4 KiB DMA page: command list @0, FIS receive @1024, command table @1536
    mem: u64,
    /// 64 KiB DMA bounce buffer for multi-sector transfers (allocated on first use)
    bounce: u64,
    sectors: u64,
    model: String,
    /// sector ranges [start, end) that may be written
    win: Vec<(u64, u64)>,
}

static mut ABAR: usize = 0;
static mut PORTS: Vec<Port> = Vec::new();
static mut INITED: bool = false;

fn ports() -> &'static mut Vec<Port> {
    unsafe { &mut *(&raw mut PORTS) }
}

fn reg_read(offset: usize) -> u32 {
    unsafe { ptr::read_volatile((ABAR + offset) as *const u32) }
}

fn reg_write(offset: usize, val: u32) {
    unsafe { ptr::write_volatile((ABAR + offset) as *mut u32, val) };
}

fn preg_read(port: usize, offset: usize) -> u32 {
    reg_read(PORT_BASE + port * PORT_SIZE + offset)
}

fn preg_write(port: usize, offset: usize, val: u32) {
    reg_write(PORT_BASE + port * PORT_SIZE + offset, val);
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

/// Find the AHCI controller (if any) via PCI and bring up every port with a
/// plain SATA disk behind it. Returns `Err` if there is no controller or no
/// disk, so callers can fall back to `ata.rs`.
pub fn init() -> Result<(), ()> {
    if unsafe { INITED } {
        return if ports().is_empty() { Err(()) } else { Ok(()) };
    }
    unsafe { INITED = true };
    let loc = pci::find(0x01, 0x06, 0x01).ok_or(())?;
    let abar = loc.bar(5);
    if abar == 0 {
        return Err(());
    }
    // memory decode + bus master (firmware normally leaves them on)
    let cmd = loc.read32(0x04) & 0xFFFF;
    pci::write32(loc.bus, loc.device, loc.function, 0x04, cmd | 0x0002 | 0x0004);
    unsafe { ABAR = abar as usize };

    // AE: AHCI Enable — some controllers boot in a legacy-compatible mode
    // until this is set.
    reg_write(GHC, reg_read(GHC) | (1 << 31));

    let pi = reg_read(PI);
    let cap = reg_read(0x00);
    crate::console_write(alloc::format!("ahci: controller at {:02x}:{:02x}.{}, ABAR 0x{abar:x}, ports 0x{pi:x}\n", loc.bus, loc.device, loc.function).as_bytes());
    for port in 0..32usize {
        if pi & (1 << port) == 0 {
            continue;
        }
        if preg_read(port, PX_SSTS) & 0x0F != 3 {
            continue; // no device presence + PHY communication on this port
        }
        let Some(mem) = dma_page(0) else {
            crate::console_write(b"ahci: no DMA memory below 4 GiB\n");
            break;
        };
        stop_command_engine(port);
        preg_write(port, PX_CLB, mem as u32);
        preg_write(port, PX_CLBU, 0);
        preg_write(port, PX_FB, (mem + 1024) as u32);
        preg_write(port, PX_FBU, 0);
        // spin up + power on (staggered spin-up controllers), FIS receive on
        let mut c = preg_read(port, PX_CMD);
        if cap & (1 << 27) != 0 {
            c |= 1 << 1; // SUD
        }
        c |= 1 << 2; // POD
        preg_write(port, PX_CMD, c | CMD_FRE);
        preg_write(port, PX_SERR, u32::MAX);
        // The signature arrives with the device's first D2H register FIS. UEFI
        // firmware that stopped the port on exit (OVMF does) leaves it unset
        // (all ones): reset the link (COMRESET) so the device sends it again.
        let ready = |port: usize| preg_read(port, PX_TFD) & 0x88 == 0 && preg_read(port, PX_SIG) != u32::MAX;
        if !ready(port) {
            preg_write(port, PX_CMD, preg_read(port, PX_CMD) & !CMD_FRE);
            preg_write(port, PX_SCTL, (preg_read(port, PX_SCTL) & !0x0F) | 1);
            crate::arch::timer::delay_us(2_000);
            preg_write(port, PX_SCTL, preg_read(port, PX_SCTL) & !0x0F);
            spin_until(1_000_000, || preg_read(port, PX_SSTS) & 0x0F == 3);
            preg_write(port, PX_SERR, u32::MAX);
            preg_write(port, PX_CMD, preg_read(port, PX_CMD) | CMD_FRE);
            spin_until(AHCI_TIMEOUT_US, || ready(port));
        }
        let sig = preg_read(port, PX_SIG);
        if sig != SATA_SIG_ATA {
            crate::console_write(alloc::format!("ahci: port {port}: signature 0x{sig:x}, not a disk\n").as_bytes());
            preg_write(port, PX_CMD, preg_read(port, PX_CMD) & !CMD_FRE);
            crate::mm::frame::free_frames(mem, 0);
            continue; // ATAPI or something else — not a plain disk
        }
        preg_write(port, PX_IS, u32::MAX);
        start_command_engine(port);
        ports().push(Port { port, mem, bounce: 0, sectors: 0, model: String::new(), win: Vec::new() });
        identify(ports().len() - 1);
        let p = ports().last().unwrap();
        crate::console_write(alloc::format!("ahci: port {port}: {} ({} MiB)\n", p.model, p.sectors / 2048).as_bytes());
    }
    if ports().is_empty() {
        return Err(());
    }
    Ok(())
}

/// Bound on every AHCI register-bit poll below — a controller that never
/// clears FR/CR/BSY/DRQ (removed mid-command, or simply not present behind
/// a misdetected PCI BAR) must not hang the caller forever the way an
/// unbounded `while ... { pause() }` would. See `ata.rs`'s `ATA_TIMEOUT_US`
/// for the same reasoning on the legacy PIO path.
const AHCI_TIMEOUT_US: u64 = 5_000_000;

/// Poll `cond` until it holds or `timeout_us` passes, WITHOUT yielding to other
/// tasks (`timer::poll_until` does yield). A command owns its port's single
/// command slot and bounce buffer from issue to completion: if a background
/// task (messenger, SSH) yielded mid-command, the desktop could issue on the
/// same port and each would read the other's sectors -- seen as a lingfs tree
/// read returning the Data partition's FAT directory. Commands take
/// microseconds to milliseconds, so not yielding costs nothing visible.
fn spin_until(timeout_us: u64, mut cond: impl FnMut() -> bool) -> bool {
    let end = crate::arch::timer::now_us() + timeout_us;
    loop {
        if cond() {
            return true;
        }
        if crate::arch::timer::now_us() >= end {
            return false;
        }
        core::hint::spin_loop();
    }
}

fn stop_command_engine(port: usize) {
    let mut cmd = preg_read(port, PX_CMD);
    cmd &= !CMD_ST;
    cmd &= !CMD_FRE;
    preg_write(port, PX_CMD, cmd);
    spin_until(AHCI_TIMEOUT_US, || preg_read(port, PX_CMD) & (CMD_FR | CMD_CR) == 0);
}

fn start_command_engine(port: usize) {
    spin_until(AHCI_TIMEOUT_US, || preg_read(port, PX_CMD) & CMD_CR == 0);
    let mut cmd = preg_read(port, PX_CMD);
    cmd |= CMD_FRE;
    cmd |= CMD_ST;
    preg_write(port, PX_CMD, cmd);
}

/// Issue one command at slot 0 of disk `i` (index into PORTS): `count`
/// sectors at `lba` to or from the physical buffer `phys` (identity-mapped,
/// below 4 GiB). `count` 0 = a non-data command (cache flush). Polled.
fn issue(i: usize, ata_cmd: u8, lba: u64, count: u32, phys: u64, write: bool) -> Result<(), ()> {
    let Some(p) = ports().get(i) else { return Err(()) };
    let port = p.port;
    let mem = p.mem;
    let bytes = count * SECTOR_SIZE as u32;
    if phys + bytes as u64 > 1 << 32 || bytes > 4 << 20 {
        return Err(());
    }
    unsafe {
        // Command header (slot 0): 32 bytes at the start of the command list.
        let hdr = mem as *mut u8;
        let prdtl = if bytes > 0 { 1u32 } else { 0 };
        let cfl_w_prdtl: u32 = 5 // CFL: 20-byte FIS / 4 = 5 dwords
            | if write { 1 << 6 } else { 0 }
            | (prdtl << 16);
        ptr::write_volatile(hdr as *mut u32, cfl_w_prdtl);
        ptr::write_volatile(hdr.add(4) as *mut u32, 0); // PRDBC, set by controller
        let ctba = mem + 1536;
        ptr::write_volatile(hdr.add(8) as *mut u32, ctba as u32);
        ptr::write_volatile(hdr.add(12) as *mut u32, 0);

        // Command FIS (Register H2D, 20 bytes) at the start of the command table.
        let cfis = ctba as *mut u8;
        for k in 0..0x90 {
            ptr::write_volatile(cfis.add(k), 0);
        }
        ptr::write_volatile(cfis, FIS_TYPE_REG_H2D);
        ptr::write_volatile(cfis.add(1), 1 << 7); // bit7 = command (not control)
        ptr::write_volatile(cfis.add(2), ata_cmd);
        ptr::write_volatile(cfis.add(4), lba as u8);
        ptr::write_volatile(cfis.add(5), (lba >> 8) as u8);
        ptr::write_volatile(cfis.add(6), (lba >> 16) as u8);
        // device: LBA mode (IDENTIFY takes 0)
        ptr::write_volatile(cfis.add(7), if ata_cmd == ATA_CMD_IDENTIFY { 0 } else { 1 << 6 });
        ptr::write_volatile(cfis.add(8), (lba >> 24) as u8);
        ptr::write_volatile(cfis.add(9), (lba >> 32) as u8);
        ptr::write_volatile(cfis.add(10), (lba >> 40) as u8);
        ptr::write_volatile(cfis.add(12), count as u8); // sector count (0 = 65536 for EXT; we never send 0 with data)
        ptr::write_volatile(cfis.add(13), (count >> 8) as u8);

        if bytes > 0 {
            // PRDT entry 0, at command-table offset 0x80.
            let prdt = cfis.add(0x80);
            ptr::write_volatile(prdt as *mut u32, phys as u32);
            ptr::write_volatile(prdt.add(4) as *mut u32, 0);
            ptr::write_volatile(prdt.add(8) as *mut u32, 0);
            ptr::write_volatile(prdt.add(12) as *mut u32, (bytes - 1) | (1 << 31));
        }
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

    // Wait for the port to be idle (BSY/DRQ clear in the task-file data
    // register) before issuing — don't step on an in-flight command.
    if !spin_until(AHCI_TIMEOUT_US, || preg_read(port, PX_TFD) & 0x88 == 0) {
        return Err(());
    }

    preg_write(port, PX_IS, u32::MAX); // clear any stale interrupt-status bits
    preg_write(port, PX_CI, 1); // issue slot 0

    let mut failed_is = 0u32;
    let completed = spin_until(AHCI_TIMEOUT_US * 4, || {
        // TFES, HBFS, HBDS, IFS, INFS, OFS: the command is dead, stop waiting
        let is = preg_read(port, PX_IS);
        if is & IS_FATAL != 0 {
            failed_is = is;
            return true;
        }
        preg_read(port, PX_CI) & 1 == 0
    });
    if failed_is != 0 || !completed {
        crate::console_write(
            alloc::format!(
                "ahci: port {port}: cmd 0x{ata_cmd:02x} lba {lba} x{count} {}: IS {:08x} TFD {:08x} SERR {:08x} SSTS {:08x} CMD {:08x}\n",
                if completed { "error" } else { "timed out" },
                if failed_is != 0 { failed_is } else { preg_read(port, PX_IS) },
                preg_read(port, PX_TFD),
                preg_read(port, PX_SERR),
                preg_read(port, PX_SSTS),
                preg_read(port, PX_CMD)
            )
            .as_bytes(),
        );
        recover(port);
        return Err(());
    }
    Ok(())
}

/// PxIS bits that end a command with an error (AHCI 1.3.1 section 6.1.2).
const IS_FATAL: u32 = (1 << 30) | (1 << 29) | (1 << 28) | (1 << 27) | (1 << 26) | (1 << 24);

/// Error recovery (AHCI 1.3.1 section 6.2.2): stop the port, clear the
/// errors, and when the drive still shows BSY/DRQ -- it never finished the
/// failed command -- reset the link (COMRESET) so it starts over; then start
/// the port again. Without the COMRESET a wedged drive refuses every later
/// command (seen on the Z590: after one failed write, "cannot read the disk").
fn recover(port: usize) {
    stop_command_engine(port);
    preg_write(port, PX_SERR, u32::MAX);
    preg_write(port, PX_IS, u32::MAX);
    if preg_read(port, PX_TFD) & 0x88 != 0 {
        preg_write(port, PX_SCTL, (preg_read(port, PX_SCTL) & !0x0F) | 1);
        crate::arch::timer::delay_us(2_000);
        preg_write(port, PX_SCTL, preg_read(port, PX_SCTL) & !0x0F);
        spin_until(1_000_000, || preg_read(port, PX_SSTS) & 0x0F == 3);
        preg_write(port, PX_SERR, u32::MAX);
        // FIS receive on, so the drive's first register FIS clears BSY
        preg_write(port, PX_CMD, preg_read(port, PX_CMD) | CMD_FRE);
        spin_until(AHCI_TIMEOUT_US, || preg_read(port, PX_TFD) & 0x88 == 0);
        preg_write(port, PX_SERR, u32::MAX);
        preg_write(port, PX_IS, u32::MAX);
        crate::console_write(
            alloc::format!("ahci: port {port}: link reset, TFD now {:08x} SSTS {:08x}\n", preg_read(port, PX_TFD), preg_read(port, PX_SSTS)).as_bytes(),
        );
    }
    start_command_engine(port);
}

/// `issue`, tried up to three times (the port is recovered between tries):
/// one transient link error should not fail a whole install.
fn issue_retry(i: usize, ata_cmd: u8, lba: u64, count: u32, phys: u64, write: bool) -> Result<(), ()> {
    for attempt in 0..3 {
        if issue(i, ata_cmd, lba, count, phys, write).is_ok() {
            if attempt > 0 {
                crate::console_write(alloc::format!("ahci: lba {lba} ok on try {}\n", attempt + 1).as_bytes());
            }
            return Ok(());
        }
        crate::arch::timer::delay_us(50_000);
    }
    Err(())
}

/// IDENTIFY DEVICE: model name and size (LBA48 count, else LBA28).
fn identify(i: usize) {
    let Some(page) = dma_page(0) else { return };
    if issue(i, ATA_CMD_IDENTIFY, 0, 1, page, false).is_ok() {
        let id = unsafe { core::slice::from_raw_parts(page as *const u8, 512) };
        let w = |k: usize| u16::from_le_bytes([id[k * 2], id[k * 2 + 1]]);
        let mut m = Vec::new();
        for k in 27..47 {
            let v = w(k);
            m.push((v >> 8) as u8);
            m.push(v as u8);
        }
        let model = String::from(String::from_utf8_lossy(&m).trim());
        let lba48 = w(83) & (1 << 10) != 0;
        let n48 = w(100) as u64 | (w(101) as u64) << 16 | (w(102) as u64) << 32 | (w(103) as u64) << 48;
        let n28 = w(60) as u64 | (w(61) as u64) << 16;
        let p = &mut ports()[i];
        p.model = model;
        p.sectors = if lba48 && n48 != 0 { n48 } else { n28 };
    }
    crate::mm::frame::free_frames(page, 0);
}

fn index_of(port: usize) -> Option<usize> {
    ports().iter().position(|p| p.port == port)
}

/// The SATA disks found: (AHCI port number, model, size in sectors).
pub fn disks() -> Vec<(usize, String, u64)> {
    let _ = init();
    ports().iter().map(|p| (p.port, p.model.clone(), p.sectors)).collect()
}

/// Read `buf.len() / 512` sectors from the disk on AHCI port `port` at `lba`.
pub fn read_port(port: usize, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
    let i = index_of(port).ok_or(())?;
    if buf.len() % SECTOR_SIZE != 0 {
        return Err(());
    }
    let sectors = ports()[i].sectors;
    if sectors != 0 && lba + (buf.len() / SECTOR_SIZE) as u64 > sectors {
        return Err(());
    }
    let bounce = bounce(i)?;
    let mut at = lba;
    for chunk in buf.chunks_mut(MAX_RUN * SECTOR_SIZE) {
        let n = (chunk.len() / SECTOR_SIZE) as u32;
        issue_retry(i, ATA_CMD_READ_DMA_EXT, at, n, bounce, false)?;
        unsafe { ptr::copy_nonoverlapping(bounce as *const u8, chunk.as_mut_ptr(), chunk.len()) };
        at += n as u64;
    }
    Ok(())
}

/// Write sectors to the disk on AHCI port `port` at `lba` -- only inside its
/// write windows.
pub fn write_port(port: usize, lba: u64, buf: &[u8]) -> Result<(), ()> {
    let i = index_of(port).ok_or(())?;
    if buf.len() % SECTOR_SIZE != 0 {
        return Err(());
    }
    let end = lba + (buf.len() / SECTOR_SIZE) as u64;
    if !ports()[i].win.iter().any(|&(lo, hi)| lba >= lo && end <= hi) {
        return Err(());
    }
    let bounce = bounce(i)?;
    let mut at = lba;
    for chunk in buf.chunks(MAX_RUN * SECTOR_SIZE) {
        let n = (chunk.len() / SECTOR_SIZE) as u32;
        unsafe { ptr::copy_nonoverlapping(chunk.as_ptr(), bounce as *mut u8, chunk.len()) };
        issue_retry(i, ATA_CMD_WRITE_DMA_EXT, at, n, bounce, true)?;
        at += n as u64;
    }
    Ok(())
}

/// FLUSH CACHE EXT: everything written so far is on the media.
pub fn flush_port(port: usize) -> Result<(), ()> {
    let i = index_of(port).ok_or(())?;
    issue_retry(i, ATA_CMD_FLUSH_CACHE_EXT, 0, 0, 0, false)
}

fn bounce(i: usize) -> Result<u64, ()> {
    let p = &mut ports()[i];
    if p.bounce == 0 {
        p.bounce = dma_page(4).ok_or(())?;
    }
    Ok(p.bounce)
}

/// Allow writes to sectors [start, end) of the disk on port `port` (added to
/// its windows).
pub fn add_write_window(port: usize, start: u64, end: u64) {
    if let Some(i) = index_of(port) {
        ports()[i].win.push((start, end));
    }
}

/// Close every write window of the disk on port `port`.
pub fn clear_write_windows(port: usize) {
    if let Some(i) = index_of(port) {
        ports()[i].win.clear();
    }
}

/// The open windows of the disk on port `port` (to put back after an install).
pub fn write_windows(port: usize) -> Vec<(u64, u64)> {
    index_of(port).map(|i| ports()[i].win.clone()).unwrap_or_default()
}

/// The primary disk's AHCI port (the first one found).
pub fn primary_port() -> Option<usize> {
    ports().first().map(|p| p.port)
}

pub fn read_sector(lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
    // single sectors go straight into the caller's buffer (kernel memory is
    // identity-mapped below 4 GiB), no bounce copy
    let p = ports().first().ok_or(())?;
    if p.sectors != 0 && lba as u64 >= p.sectors {
        return Err(());
    }
    issue(0, ATA_CMD_READ_DMA_EXT, lba as u64, 1, buf.as_mut_ptr() as u64, false)
}

pub fn write_sector(lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
    let p = ports().first().ok_or(())?;
    if !p.win.iter().any(|&(lo, hi)| lba as u64 >= lo && (lba as u64) < hi) {
        return Err(());
    }
    issue(0, ATA_CMD_WRITE_DMA_EXT, lba as u64, 1, buf.as_ptr() as u64, true)
}

pub fn available() -> bool {
    !ports().is_empty()
}
