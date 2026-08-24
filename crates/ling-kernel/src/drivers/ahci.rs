//! AHCI (SATA) disk driver — the modern block-storage interface most
//! current hardware and hypervisors default to (VirtualBox's default disk
//! controller is SATA/AHCI, not legacy IDE — confirmed as the actual cause
//! of `lingfs` working in QEMU, where the test disk was explicitly
//! attached via legacy IDE, but not in a VirtualBox VM whose disk was on
//! SATA: `ata.rs`'s PIO driver can only ever see an IDE-attached disk).
//!
//! Found via PCI (class 0x01 mass-storage, subclass 0x06 SATA, prog-if
//! 0x01 AHCI) rather than a fixed port range, since its MMIO register
//! window (the "ABAR") is a PCI BAR assigned by firmware, typically well
//! above the first 1GiB of RAM (near the top of 32-bit MMIO space) — see
//! `boot32.rs`'s 4GiB identity map, added specifically so this can reach
//! it without a proper memory manager. Single port, single command slot:
//! this is a "basic" kernel with one disk, not a general AHCI stack.
use crate::arch::pci;
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
const PX_CI: usize = 0x38;

const CMD_ST: u32 = 1 << 0;
const CMD_FRE: u32 = 1 << 4;
const CMD_FR: u32 = 1 << 14;
const CMD_CR: u32 = 1 << 15;

const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;
const FIS_TYPE_REG_H2D: u8 = 0x27;

const SATA_SIG_ATA: u32 = 0x0000_0101;

#[repr(C, align(1024))]
struct CommandList([u8; 32 * 32]);
#[repr(C, align(256))]
struct FisReceive([u8; 256]);
#[repr(C, align(128))]
struct CommandTable([u8; 0x80 + 16]); // FIS+ATAPI+reserved header, + 1 PRDT entry

static mut CMD_LIST: CommandList = CommandList([0; 32 * 32]);
static mut FIS_RECV: FisReceive = FisReceive([0; 256]);
static mut CMD_TABLE: CommandTable = CommandTable([0; 0x80 + 16]);

static mut ABAR: usize = 0;
static mut PORT: usize = usize::MAX;

fn reg_read(offset: usize) -> u32 {
    unsafe { ptr::read_volatile((ABAR + offset) as *const u32) }
}

fn reg_write(offset: usize, val: u32) {
    unsafe { ptr::write_volatile((ABAR + offset) as *mut u32, val) };
}

fn port_reg_read(offset: usize) -> u32 {
    reg_read(PORT_BASE + unsafe { PORT } * PORT_SIZE + offset)
}

fn port_reg_write(offset: usize, val: u32) {
    reg_write(PORT_BASE + unsafe { PORT } * PORT_SIZE + offset, val);
}

/// Find the AHCI controller (if any) via PCI, pick the first present SATA
/// (non-ATAPI) port, and bring it up: identity-mapped command list, FIS
/// receive area, and command table (all fixed, static — no heap). Returns
/// `Err` if no AHCI controller or no usable port was found, so callers can
/// fall back to `ata.rs`.
pub fn init() -> Result<(), ()> {
    let loc = pci::find(0x01, 0x06, 0x01).ok_or(())?;
    let abar = loc.bar(5);
    if abar == 0 {
        return Err(());
    }
    unsafe { ABAR = abar as usize };

    // AE: AHCI Enable — some controllers boot in a legacy-compatible mode
    // until this is set.
    reg_write(GHC, reg_read(GHC) | (1 << 31));

    let pi = reg_read(PI);
    let mut found_port = None;
    for port in 0..32usize {
        if pi & (1 << port) == 0 {
            continue;
        }
        unsafe { PORT = port };
        let ssts = port_reg_read(PX_SSTS);
        let det = ssts & 0x0F;
        if det != 3 {
            continue; // no device presence + PHY communication on this port
        }
        if port_reg_read(PX_SIG) != SATA_SIG_ATA {
            continue; // ATAPI or something else — not a plain disk
        }
        found_port = Some(port);
        break;
    }
    let Some(port) = found_port else {
        unsafe { PORT = usize::MAX };
        return Err(());
    };
    unsafe { PORT = port };

    stop_command_engine();

    let clb = &raw const CMD_LIST as u32;
    let fb = &raw const FIS_RECV as u32;
    port_reg_write(PX_CLB, clb);
    port_reg_write(PX_CLBU, 0);
    port_reg_write(PX_FB, fb);
    port_reg_write(PX_FBU, 0);

    start_command_engine();
    Ok(())
}

/// Bound on every AHCI register-bit poll below — a controller that never
/// clears FR/CR/BSY/DRQ (removed mid-command, or simply not present behind
/// a misdetected PCI BAR) must not hang the caller forever the way an
/// unbounded `while ... { pause() }` would. See `ata.rs`'s `ATA_TIMEOUT_US`
/// for the same reasoning on the legacy PIO path.
const AHCI_TIMEOUT_US: u64 = 5_000_000;

fn stop_command_engine() {
    let mut cmd = port_reg_read(PX_CMD);
    cmd &= !CMD_ST;
    cmd &= !CMD_FRE;
    port_reg_write(PX_CMD, cmd);
    crate::arch::timer::poll_until(AHCI_TIMEOUT_US, || {
        port_reg_read(PX_CMD) & (CMD_FR | CMD_CR) == 0
    });
}

fn start_command_engine() {
    crate::arch::timer::poll_until(AHCI_TIMEOUT_US, || port_reg_read(PX_CMD) & CMD_CR == 0);
    let mut cmd = port_reg_read(PX_CMD);
    cmd |= CMD_FRE;
    cmd |= CMD_ST;
    port_reg_write(PX_CMD, cmd);
}

/// Issue a single-sector READ DMA EXT (`write` = false) or WRITE DMA EXT
/// (`write` = true) at slot 0 against `buf` (exactly `SECTOR_SIZE` bytes,
/// physical == virtual since everything's identity-mapped). Polls
/// PxCI/task-file status rather than using interrupts, matching this
/// kernel's no-interrupt-handler model elsewhere (`ata.rs`, `keyboard.rs`).
fn issue(lba: u32, buf_ptr: *mut u8, write: bool) -> Result<(), ()> {
    if unsafe { PORT } == usize::MAX {
        return Err(());
    }

    unsafe {
        // Command header (slot 0): 32 bytes at CMD_LIST[0..32].
        let hdr = (&raw mut CMD_LIST).cast::<u8>();
        let cfl_w_prdtl: u32 = 5 // CFL: 20-byte FIS / 4 = 5 dwords
            | if write { 1 << 6 } else { 0 }
            | (1u32 << 16); // PRDTL = 1
        ptr::write_volatile(hdr as *mut u32, cfl_w_prdtl);
        ptr::write_volatile(hdr.add(4) as *mut u32, 0); // PRDBC, set by controller
        let ctba = &raw const CMD_TABLE as u32;
        ptr::write_volatile(hdr.add(8) as *mut u32, ctba);
        ptr::write_volatile(hdr.add(12) as *mut u32, 0);

        // Command FIS (Register H2D, 20 bytes) at the start of the command table.
        let cfis = (&raw mut CMD_TABLE).cast::<u8>();
        for i in 0..20 {
            ptr::write_volatile(cfis.add(i), 0);
        }
        ptr::write_volatile(cfis, FIS_TYPE_REG_H2D);
        ptr::write_volatile(cfis.add(1), 1 << 7); // bit7 = command (not control)
        ptr::write_volatile(
            cfis.add(2),
            if write {
                ATA_CMD_WRITE_DMA_EXT
            } else {
                ATA_CMD_READ_DMA_EXT
            },
        );
        ptr::write_volatile(cfis.add(4), lba as u8);
        ptr::write_volatile(cfis.add(5), (lba >> 8) as u8);
        ptr::write_volatile(cfis.add(6), (lba >> 16) as u8);
        ptr::write_volatile(cfis.add(7), 1 << 6); // device: LBA mode
        ptr::write_volatile(cfis.add(8), (lba >> 24) as u8);
        ptr::write_volatile(cfis.add(9), 0);
        ptr::write_volatile(cfis.add(10), 0);
        ptr::write_volatile(cfis.add(12), 1); // sector count low = 1
        ptr::write_volatile(cfis.add(13), 0);

        // PRDT entry 0, at command-table offset 0x80.
        let prdt = cfis.add(0x80);
        ptr::write_volatile(prdt as *mut u32, buf_ptr as u32);
        ptr::write_volatile(prdt.add(4) as *mut u32, 0);
        ptr::write_volatile(prdt.add(8) as *mut u32, 0);
        ptr::write_volatile(
            prdt.add(12) as *mut u32,
            (SECTOR_SIZE as u32 - 1) | (1 << 31),
        );
    }

    // Wait for the port to be idle (BSY/DRQ clear in the task-file data
    // register) before issuing — mirrors ata.rs's wait_ready/wait_drq
    // discipline for the same reason: don't step on an in-flight command.
    // Bounded (see `AHCI_TIMEOUT_US`'s doc) rather than trusting a port
    // that's wedged, or was never actually backed by a real drive, to ever
    // clear these bits on its own.
    if !crate::arch::timer::poll_until(AHCI_TIMEOUT_US, || port_reg_read(PX_TFD) & 0x88 == 0) {
        return Err(());
    }

    port_reg_write(PX_IS, u32::MAX); // clear any stale interrupt-status bits
    port_reg_write(PX_CI, 1); // issue slot 0

    let mut task_file_error = false;
    let completed = crate::arch::timer::poll_until(AHCI_TIMEOUT_US, || {
        if port_reg_read(PX_IS) & (1 << 30) != 0 {
            task_file_error = true;
            return true; // stop polling; caller distinguishes error below
        }
        port_reg_read(PX_CI) & 1 == 0
    });
    if task_file_error || !completed {
        return Err(());
    }
    Ok(())
}

pub fn read_sector(lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
    issue(lba, buf.as_mut_ptr(), false)
}

pub fn write_sector(lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
    issue(lba, buf.as_ptr() as *mut u8, true)
}

pub fn available() -> bool {
    unsafe { PORT != usize::MAX }
}
