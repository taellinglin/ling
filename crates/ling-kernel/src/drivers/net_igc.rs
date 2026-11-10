//! Intel I225/I226 2.5 GbE ("igc") driver -- the wired NIC on the user's
//! ASUS TUF Z590-PLUS WIFI (I225-V, 8086:15F3 rev 03 = B3 stepping, PCI
//! 05:00.0, BAR0 32-bit 1 MiB). Same shape and API as `net_e1000` (free
//! functions, polling, no interrupts); `drivers::netdev` picks whichever NIC
//! is present and `netstack` runs over it. The IP configuration lives in
//! `net_e1000`'s statics for both (like `rp1_eth` does for the Pi's Wi-Fi).
//!
//! Every register offset, bit and sequence below comes from the Linux igc
//! driver (drivers/net/ethernet/intel/igc, torvalds/linux master, fetched
//! 2026-11-08); the file each constant came from is noted next to it.
//! igc has NO legacy descriptors: one RX and one TX queue with ADVANCED
//! descriptors (igc_base.h `union igc_adv_rx_desc` / `igc_adv_tx_desc`).
//!
//! **Test model.** QEMU has no I225, but its `-device igb` (82576, 8086:10C9)
//! has the same queue registers, SRRCTL and advanced descriptors. The few
//! differences are isolated in [`Model`]: interrupt-mask register offsets
//! (82576 keeps the legacy ICR/IMC at 0xC0/0xD8, QEMU hw/net/e1000x_regs.h),
//! the PHY's MDIO address (QEMU igb answers only PHY 1, igb_core.c
//! `igb_set_mdic`; igc talks to PHY 0 -- `phy->addr` is never set in igc), and
//! the I225-only steps (PHY reset via CTRL.PHY_RST + PHPM, EEE off, RFCTL
//! errata bit, the 2.5G STATUS bit). The 82576 path exists to test the
//! ring/descriptor logic end to end; it is not meant as a real-82576 driver.
//!
//! Sequence (igc_main.c / igc_base.c / igc_mac.c / igc_phy.c):
//!   PCI MEM+BM -> igc_reset_hw_base (master disable, IMC, RCTL=0, TCTL=PSP,
//!   10 ms, CTRL.RST, EECD.AUTO_RD) -> igc_get_hw_control (CTRL_EXT.DRV_LOAD)
//!   -> MAC from RAL0/RAH0 -> igc_phy_hw_reset -> igc_set_eee_i225(off) ->
//!   igc_setup_copper_link_base (SLU) + power-up/restart autoneg ->
//!   igc_setup_tctl -> igc_setup_rctl -> igc_configure_tx_ring ->
//!   igc_configure_rx_ring -> RDT (igc_alloc_rx_buffers).
use crate::arch::{pci, timer};
use core::ptr;
use core::sync::atomic::{fence, Ordering};

// --- Register offsets (igc_regs.h) ----------------------------------------
const CTRL: usize = 0x00000; // IGC_CTRL
const STATUS: usize = 0x00008; // IGC_STATUS
const EECD: usize = 0x00010; // IGC_EECD
const CTRL_EXT: usize = 0x00018; // IGC_CTRL_EXT
const MDIC: usize = 0x00020; // IGC_MDIC
const RCTL: usize = 0x00100; // IGC_RCTL
const TCTL: usize = 0x00400; // IGC_TCTL
const I225_PHPM: usize = 0x00E14; // IGC_I225_PHPM
const IPCNFG: usize = 0x00E38; // IGC_IPCNFG
const EEER: usize = 0x00E30; // IGC_EEER
const EIMC: usize = 0x01528; // IGC_EIMC (QEMU igb_regs.h E1000_EIMC: same)
const RFCTL: usize = 0x05008; // IGC_RFCTL
const MTA: usize = 0x05200; // IGC_MTA (128 regs: igc_base.c mta_reg_count)
const RAL0: usize = 0x05400; // IGC_RAL(0)
const RAH0: usize = 0x05404; // IGC_RAH(0)
const SWSM: usize = 0x05B50; // IGC_SWSM
const SW_FW_SYNC: usize = 0x05B5C; // IGC_SW_FW_SYNC
const MANC: usize = 0x05820; // IGC_MANC
// queue 0 (IGC_*(n) = base + n*0x40)
const RDBAL: usize = 0x0C000; // IGC_RDBAL(0)
const RDBAH: usize = 0x0C004; // IGC_RDBAH(0)
const RDLEN: usize = 0x0C008; // IGC_RDLEN(0)
const SRRCTL: usize = 0x0C00C; // IGC_SRRCTL(0)
const RDH: usize = 0x0C010; // IGC_RDH(0)
const RDT: usize = 0x0C018; // IGC_RDT(0)
const RXDCTL: usize = 0x0C028; // IGC_RXDCTL(0)
const TDBAL: usize = 0x0E000; // IGC_TDBAL(0)
const TDBAH: usize = 0x0E004; // IGC_TDBAH(0)
const TDLEN: usize = 0x0E008; // IGC_TDLEN(0)
const TDH: usize = 0x0E010; // IGC_TDH(0)
const TDT: usize = 0x0E018; // IGC_TDT(0)
const TXDCTL: usize = 0x0E028; // IGC_TXDCTL(0)

// --- Bits (igc_defines.h unless noted) ------------------------------------
const CTRL_GIO_MASTER_DISABLE: u32 = 0x0000_0004;
const CTRL_SLU: u32 = 0x0000_0040;
const CTRL_FRCSPD: u32 = 0x0000_0800;
const CTRL_FRCDPX: u32 = 0x0000_1000;
const CTRL_RST: u32 = 0x0400_0000; // "Global reset"
const CTRL_PHY_RST: u32 = 0x8000_0000;
const CTRL_EXT_DRV_LOAD: u32 = 0x1000_0000;
const STATUS_FD: u32 = 0x0000_0001;
const STATUS_LU: u32 = 0x0000_0002;
const STATUS_SPEED_100: u32 = 0x0000_0040;
const STATUS_SPEED_1000: u32 = 0x0000_0080;
const STATUS_SPEED_2500: u32 = 0x0040_0000; // I225: set together with SPEED_1000
const STATUS_GIO_MASTER_ENABLE: u32 = 0x0008_0000;
const EECD_AUTO_RD: u32 = 0x0000_0200;
const RAH_AV: u32 = 0x8000_0000;
const SWSM_SMBI: u32 = 0x0000_0001;
const SWSM_SWESMBI: u32 = 0x0000_0002;
const SWFW_PHY0_SM: u32 = 0x2;
const MANC_BLK_PHY_RST_ON_IDE: u32 = 0x0004_0000;
const PHY_RST_COMP: u32 = 0x0100; // IGC_PHY_RST_COMP (in I225_PHPM)
const IPCNFG_EEE_ALL_AN: u32 = 0x10 | 0x08 | 0x04; // 2.5G | 1G | 100M AN
const EEER_LPI: u32 = 0x0001_0000 | 0x0002_0000 | 0x0004_0000; // TX_LPI | RX_LPI | LPI_FC
const RFCTL_IPV6_EX_DIS: u32 = 0x0001_0000;

const MDIC_REG_SHIFT: u32 = 16;
const MDIC_PHY_SHIFT: u32 = 21;
const MDIC_OP_WRITE: u32 = 0x0400_0000;
const MDIC_OP_READ: u32 = 0x0800_0000;
const MDIC_READY: u32 = 0x1000_0000;
const MDIC_ERROR: u32 = 0x4000_0000;
const PHY_CONTROL: u32 = 0x00;
const MII_CR_RESTART_AUTO_NEG: u16 = 0x0200;
const MII_CR_POWER_DOWN: u16 = 0x0800;
const MII_CR_AUTO_NEG_EN: u16 = 0x1000;

const RCTL_EN: u32 = 0x0000_0002;
const RCTL_SBP: u32 = 0x0000_0004;
const RCTL_UPE: u32 = 0x0000_0008;
const RCTL_MPE: u32 = 0x0000_0010;
const RCTL_LPE: u32 = 0x0000_0020;
const RCTL_LBM_TCVR: u32 = 0x0000_00C0; // covers LBM_MAC too
const RCTL_MO_MASK: u32 = 3 << 12; // IGC_RCTL_MO_SHIFT 12
const RCTL_BAM: u32 = 0x0000_8000;
const RCTL_SZ_MASK: u32 = 0x0003_0000; // IGC_RCTL_SZ_256 = both size bits
const RCTL_SECRC: u32 = 0x0400_0000;

const TCTL_EN: u32 = 0x0000_0002;
const TCTL_PSP: u32 = 0x0000_0008;
const TCTL_CT: u32 = 0x0000_0FF0;
const TCTL_RTLC: u32 = 0x0100_0000;
const COLLISION_THRESHOLD: u32 = 15; // IGC_COLLISION_THRESHOLD, IGC_CT_SHIFT 4

// igc_base.h
const SRRCTL_BSIZEPKT_MASK: u32 = 0x7F; // GENMASK(6,0), 1 KiB units
const SRRCTL_BSIZEHDR_MASK: u32 = 0x3F << 8; // GENMASK(13,8), 64 B units
const SRRCTL_DESCTYPE_MASK: u32 = 0x7 << 25; // GENMASK(27,25)
const SRRCTL_DESCTYPE_ADV_ONEBUF: u32 = 1 << 25;
const ADVTXD_DTYP_DATA: u32 = 0x0030_0000;
const ADVTXD_DCMD_EOP: u32 = 0x0100_0000;
const ADVTXD_DCMD_IFCS: u32 = 0x0200_0000;
const ADVTXD_DCMD_RS: u32 = 0x0800_0000;
const ADVTXD_DCMD_DEXT: u32 = 0x2000_0000;
const ADVTXD_PAYLEN_SHIFT: u32 = 14;
const RXD_STAT_DD: u32 = 0x01; // IGC_RXD_STAT_DD
const RXD_STAT_EOP: u32 = 0x02; // IGC_RXD_STAT_EOP

// igc.h
const RXDCTL_QUEUE_ENABLE: u32 = 0x0200_0000;
const TXDCTL_QUEUE_ENABLE: u32 = 1 << 25; // GENMASK(25,25)
const RXDCTL_PTHRESH: u32 = 8;
const RXDCTL_HTHRESH: u32 = 8;
const TXDCTL_PTHRESH: u32 = 8;
const TXDCTL_HTHRESH: u32 = 1;
// Linux uses WTHRESH 4 (RX) / 16 (TX) and relies on the interrupt/ITR timer
// to flush delayed write-backs. This driver polls with interrupts masked, so
// a lone packet's write-back must not wait for company: WTHRESH = 1.
const WTHRESH_POLLED: u32 = 1;

const RX_DESC_COUNT: usize = 64; // 64 * 16 B = 1 KiB ring (128 B multiple)
const TX_DESC_COUNT: usize = 16; // 16 * 16 B = 256 B ring
const BUF_SIZE: usize = 2048; // IGC_RXBUFFER_2048
const RX_HDR_LEN: u32 = 256; // IGC_RX_HDR_LEN (header split unused)

/// What we are driving.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Model {
    /// I225/I226 family (the real target).
    I225,
    /// 82576, as QEMU's `-device igb` emulates it (test vehicle).
    I82576,
}

/// Device ids this driver takes, with a name for the status line.
/// igc_hw.h IGC_DEV_ID_*; blank-NVM ids (0x15FD, 0x125F) are left out --
/// they have no MAC to read. 0x10C9 is QEMU's igb (e1000_hw.h
/// E1000_DEV_ID_82576, QEMU igb.c).
const IDS: &[(u16, Model, &str)] = &[
    (0x15F2, Model::I225, "I225-LM"),
    (0x15F3, Model::I225, "I225-V"),
    (0x15F8, Model::I225, "I225-I"),
    (0x15F7, Model::I225, "I220-V"),
    (0x3100, Model::I225, "I225-K"),
    (0x3101, Model::I225, "I225-K2"),
    (0x3102, Model::I225, "I226-K"),
    (0x5502, Model::I225, "I225-LMvP"),
    (0x5503, Model::I225, "I226-LMvP"),
    (0x0D9F, Model::I225, "I225-IT"),
    (0x125B, Model::I225, "I226-LM"),
    (0x125C, Model::I225, "I226-V"),
    (0x125D, Model::I225, "I226-IT"),
    (0x125E, Model::I225, "I221-V"),
    (0x10C9, Model::I82576, "82576 (QEMU igb)"),
];

/// `Some` if this driver (not `net_e1000`) owns `vendor:device`.
pub fn model_for(vendor: u16, device: u16) -> Option<(Model, &'static str)> {
    if vendor != 0x8086 {
        return None;
    }
    IDS.iter().find(|(d, _, _)| *d == device).map(|(_, m, n)| (*m, *n))
}

static mut MMIO: usize = 0;
static mut MODEL: Model = Model::I225;
static mut NAME: &str = "";
static mut LOC: (u8, u8, u8) = (0, 0, 0);
static mut DEV_ID: u16 = 0;
static mut MAC: [u8; 6] = [0; 6];
static mut UP: bool = false;
static mut TRIED: bool = false;
static mut LAST_ERROR: &str = "";

// DMA memory (mm::frame: identity-mapped, below 4 GiB, coherent on x86).
static mut RX_RING: usize = 0;
static mut TX_RING: usize = 0;
static mut RX_BUFS: usize = 0;
static mut TX_BUFS: usize = 0;
static mut RX_TAIL: usize = 0;
static mut TX_TAIL: usize = 0;

fn rd(off: usize) -> u32 {
    unsafe { ptr::read_volatile((MMIO + off) as *const u32) }
}
fn wr(off: usize, v: u32) {
    unsafe { ptr::write_volatile((MMIO + off) as *mut u32, v) }
}
/// "wrfl()": a read that pushes posted writes out.
fn flush() {
    let _ = rd(STATUS);
}

fn icr_imc() -> (usize, usize) {
    match unsafe { MODEL } {
        Model::I225 => (0x01500, 0x0150C), // IGC_ICR, IGC_IMC
        Model::I82576 => (0x000C0, 0x000D8), // E1000_ICR, E1000_IMC
    }
}
fn phy_addr() -> u32 {
    match unsafe { MODEL } {
        Model::I225 => 0,
        Model::I82576 => 1,
    }
}

pub fn is_up() -> bool {
    unsafe { UP }
}
pub fn mac() -> [u8; 6] {
    unsafe { MAC }
}
/// Why the last `init` failed ("" if it didn't, or never ran).
pub fn last_error() -> &'static str {
    unsafe { LAST_ERROR }
}
pub fn name() -> &'static str {
    unsafe { NAME }
}
pub fn pci_location() -> (u8, u8, u8) {
    unsafe { LOC }
}
pub fn device_id() -> u16 {
    unsafe { DEV_ID }
}

pub fn link_up() -> bool {
    is_up() && rd(STATUS) & STATUS_LU != 0
}

/// Link speed in Mb/s (igc_mac.c igc_get_speed_and_duplex_copper), 0 = down.
pub fn speed_mbps() -> u32 {
    if !link_up() {
        return 0;
    }
    let s = rd(STATUS);
    if s & STATUS_SPEED_1000 != 0 {
        if unsafe { MODEL } == Model::I225 && s & STATUS_SPEED_2500 != 0 {
            2500
        } else {
            1000
        }
    } else if s & STATUS_SPEED_100 != 0 {
        100
    } else {
        10
    }
}
pub fn full_duplex() -> bool {
    is_up() && rd(STATUS) & STATUS_FD != 0
}

fn log(s: &[u8]) {
    crate::drivers::serial::write(s);
}
fn fail(why: &'static str) -> Result<(), ()> {
    unsafe { LAST_ERROR = why };
    log(b"net: igc: ");
    log(why.as_bytes());
    log(b"\n");
    Err(())
}

/// Init once, remember the outcome (also a failure: PCI is not hot-plugged,
/// so a missing or broken controller stays that way).
pub fn ensure_init() -> bool {
    unsafe {
        if UP {
            return true;
        }
        if TRIED {
            return false;
        }
        TRIED = true;
    }
    init().is_ok()
}

// --- SW/FW semaphore + MDIO (igc_i225.c, igc_mac.c, igc_phy.c) ----------

fn put_hw_semaphore() {
    wr(SWSM, rd(SWSM) & !(SWSM_SMBI | SWSM_SWESMBI));
}

/// igc_get_hw_semaphore_i225: SMBI (reading SWSM sets it when it was clear),
/// then SWESMBI (latches if firmware isn't holding it). Bounded; clears a
/// stuck SMBI once, like Linux's `clear_semaphore_once`.
fn get_hw_semaphore() -> bool {
    let mut got = timer::poll_until(50_000, || rd(SWSM) & SWSM_SMBI == 0);
    if !got {
        put_hw_semaphore();
        got = timer::poll_until(50_000, || rd(SWSM) & SWSM_SMBI == 0);
        if !got {
            return false;
        }
    }
    let ok = timer::poll_until(50_000, || {
        wr(SWSM, rd(SWSM) | SWSM_SWESMBI);
        rd(SWSM) & SWSM_SWESMBI != 0
    });
    if !ok {
        put_hw_semaphore();
    }
    ok
}

/// igc_acquire_swfw_sync_i225(IGC_SWFW_PHY0_SM): up to 200 x 5 ms.
fn acquire_phy() -> bool {
    for _ in 0..200 {
        if !get_hw_semaphore() {
            return false;
        }
        let sync = rd(SW_FW_SYNC);
        if sync & (SWFW_PHY0_SM | (SWFW_PHY0_SM << 16)) == 0 {
            wr(SW_FW_SYNC, sync | SWFW_PHY0_SM);
            put_hw_semaphore();
            return true;
        }
        put_hw_semaphore();
        timer::delay_us(5_000);
    }
    false
}

fn release_phy() {
    if !get_hw_semaphore() {
        return;
    }
    wr(SW_FW_SYNC, rd(SW_FW_SYNC) & !SWFW_PHY0_SM);
    put_hw_semaphore();
}

/// One MDIC transaction (igc_read/write_phy_reg_mdic): 1920 x 50 us max.
fn mdic(op: u32, reg: u32, data: u16) -> Option<u16> {
    wr(MDIC, data as u32 | (reg << MDIC_REG_SHIFT) | (phy_addr() << MDIC_PHY_SHIFT) | op);
    let mut v = 0;
    let ready = timer::poll_until(1920 * 50, || {
        v = rd(MDIC);
        v & MDIC_READY != 0
    });
    if !ready || v & MDIC_ERROR != 0 {
        return None;
    }
    Some(v as u16)
}

/// igc_phy_hw_reset: CTRL.PHY_RST for reset_delay_us (100), then wait for
/// I225_PHPM.PHY_RST_COMP. Skipped when manageability blocks PHY resets
/// (igc_check_reset_block). Clears the "firmware left the PHY page select
/// non-zero" state Linux resets the PHY at probe for.
fn phy_reset() {
    if rd(MANC) & MANC_BLK_PHY_RST_ON_IDE != 0 {
        log(b"net: igc: PHY reset blocked by manageability, skipped\n");
        return;
    }
    if !acquire_phy() {
        log(b"net: igc: PHY semaphore busy, PHY reset skipped\n");
        return;
    }
    let ctrl = rd(CTRL);
    wr(CTRL, ctrl | CTRL_PHY_RST);
    flush();
    timer::delay_us(100);
    wr(CTRL, ctrl);
    flush();
    timer::delay_us(150);
    if !timer::poll_until(10_000, || rd(I225_PHPM) & PHY_RST_COMP != 0) {
        log(b"net: igc: PHY reset completion timed out\n");
    }
    timer::delay_us(150);
    release_phy();
}

/// igc_power_up_phy_copper + the restart step of igc_copper_link_autoneg:
/// clear PHY_CONTROL.POWER_DOWN (a previous OS may have powered the PHY down
/// at shutdown), set AUTO_NEG_EN and RESTART_AUTO_NEG. The advertisement
/// registers are left at their reset defaults (all speeds incl. 2.5G).
fn phy_power_up_autoneg() {
    if !acquire_phy() {
        log(b"net: igc: PHY semaphore busy, autoneg not restarted\n");
        return;
    }
    match mdic(MDIC_OP_READ, PHY_CONTROL, 0) {
        Some(c) => {
            let n = (c & !MII_CR_POWER_DOWN) | MII_CR_AUTO_NEG_EN | MII_CR_RESTART_AUTO_NEG;
            if mdic(MDIC_OP_WRITE, PHY_CONTROL, n).is_none() {
                log(b"net: igc: MDIO write of PHY_CONTROL failed\n");
            }
        },
        None => log(b"net: igc: MDIO read of PHY_CONTROL failed\n"),
    }
    release_phy();
}

// --- Descriptors (igc_base.h) --------------------------------------------
// Both kinds are 16 bytes = two little-endian u64 words.
// RX read:  [pkt_addr, hdr_addr]
// RX wb:    [rss/info, status_error(u32) | length(u16)<<32 | vlan<<48]
// TX read:  [buffer_addr, cmd_type_len(u32) | olinfo_status(u32)<<32]
// TX wb:    [rsvd, nxtseq_seed | status<<32]

unsafe fn desc(ring: usize, i: usize) -> *mut u64 {
    (ring + i * 16) as *mut u64
}

unsafe fn rx_arm(i: usize) {
    let d = desc(RX_RING, i);
    ptr::write_volatile(d, (RX_BUFS + i * BUF_SIZE) as u64);
    ptr::write_volatile(d.add(1), 0);
}

fn alloc_zeroed(order: usize) -> Option<usize> {
    let p = crate::mm::frame::alloc_frames(order)? as usize;
    unsafe { ptr::write_bytes(p as *mut u8, 0, 4096 << order) };
    Some(p)
}

pub fn init() -> Result<(), ()> {
    unsafe {
        UP = false;
        LAST_ERROR = "";
    }
    let found = pci::find_all_class(0x02, 0x00).into_iter().find_map(|l| {
        let id = l.read32(0x00);
        model_for(id as u16, (id >> 16) as u16).map(|m| (l, (id >> 16) as u16, m))
    });
    let Some((loc, dev, (model, name))) = found else {
        unsafe { LAST_ERROR = "no I225/I226 controller" };
        return Err(());
    };
    unsafe {
        MODEL = model;
        NAME = name;
        LOC = (loc.bus, loc.device, loc.function);
        DEV_ID = dev;
    }

    // BAR0: memory BAR, 32- or 64-bit (type bits 2:1 == 10b -> BAR1 is the
    // high half). The I225-V on the target board: 32-bit at 0xA1B00000.
    let lo = loc.read32(0x10);
    if lo & 1 != 0 {
        return fail("BAR0 is an I/O BAR, expected memory");
    }
    let mut base = (lo & 0xFFFF_FFF0) as u64;
    if (lo >> 1) & 3 == 2 {
        base |= (loc.read32(0x14) as u64) << 32;
    }
    if base == 0 {
        return fail("BAR0 not assigned by firmware");
    }
    // Registers used here end below 0x10000; map 128 KiB. Below 4 GiB this is
    // the identity map; above it map_mmio adds UC 4K pages (None past 512 GiB).
    let Some(va) = crate::arch::paging::map_mmio(base, 0x20000) else {
        return fail("BAR0 is above 512 GiB and cannot be mapped");
    };
    unsafe { MMIO = va as usize };

    // PCI command: memory decoding (bit 1) + bus mastering (bit 2).
    let cmd = loc.read32(0x04);
    pci::write32(loc.bus, loc.device, loc.function, 0x04, (cmd & 0xFFFF) | 0x0006);

    if rd(STATUS) == 0xFFFF_FFFF {
        return fail("registers read all-ones (device in D3 or not decoding)");
    }

    // --- igc_reset_hw_base -------------------------------------------------
    // igc_disable_pcie_master: MASTER_DISABLE_TIMEOUT 800 x ~2 ms.
    wr(CTRL, rd(CTRL) | CTRL_GIO_MASTER_DISABLE);
    if !timer::poll_until(1_600_000, || rd(STATUS) & STATUS_GIO_MASTER_ENABLE == 0) {
        log(b"net: igc: PCIe master disable timed out (continuing)\n");
    }
    let (icr, imc) = icr_imc();
    wr(imc, 0xFFFF_FFFF);
    wr(EIMC, 0xFFFF_FFFF);
    wr(RCTL, 0);
    wr(TCTL, TCTL_PSP);
    flush();
    timer::delay_us(10_000);
    wr(CTRL, rd(CTRL) | CTRL_RST);
    timer::delay_us(1_000);
    // igc_get_auto_rd_done: AUTO_READ_DONE_TIMEOUT 10 x 1-2 ms; not fatal
    // (QEMU's igb never sets AUTO_RD).
    if !timer::poll_until(20_000, || rd(EECD) & EECD_AUTO_RD != 0) && model == Model::I225 {
        log(b"net: igc: NVM auto-read not done (continuing)\n");
    }
    wr(imc, 0xFFFF_FFFF);
    wr(EIMC, 0xFFFF_FFFF);
    let _ = rd(icr);

    // igc_get_hw_control: tell firmware a driver owns the device.
    wr(CTRL_EXT, rd(CTRL_EXT) | CTRL_EXT_DRV_LOAD);

    // MAC: the NVM auto-load puts it in RAR0 (igc_read_mac_addr reads RAL/RAH).
    let ral = rd(RAL0);
    let rah = rd(RAH0);
    let m = [ral as u8, (ral >> 8) as u8, (ral >> 16) as u8, (ral >> 24) as u8, rah as u8, (rah >> 8) as u8];
    if m == [0; 6] || m == [0xFF; 6] || m[0] & 1 != 0 {
        return fail("no valid MAC in RAL0/RAH0 (blank NVM?)");
    }
    unsafe { MAC = m };
    // igc_set_default_mac_filter: RAR0 = our MAC with Address Valid.
    wr(RAL0, ral);
    wr(RAH0, (rah & 0xFFFF) | RAH_AV);
    for i in 0..128 {
        wr(MTA + i * 4, 0);
    }

    if model == Model::I225 {
        phy_reset();
        // igc_set_eee_i225 with eee_enable = false (`ethtool --set-eee off`):
        // the usual workaround for I225 link drops; we gain nothing from LPI.
        wr(IPCNFG, rd(IPCNFG) & !IPCNFG_EEE_ALL_AN);
        wr(EEER, rd(EEER) & !EEER_LPI);
        // igc_rx_fifo_flush_base: "disable IPv6 options as per hardware errata"
        wr(RFCTL, rd(RFCTL) | RFCTL_IPV6_EX_DIS);
    }

    // igc_setup_copper_link_base (autoneg): SLU, no forced speed/duplex.
    wr(CTRL, (rd(CTRL) & !(CTRL_FRCSPD | CTRL_FRCDPX)) | CTRL_SLU);
    phy_power_up_autoneg();

    // DMA memory.
    let (Some(rings), Some(rxb), Some(txb)) = (alloc_zeroed(0), alloc_zeroed(5), alloc_zeroed(3)) else {
        return fail("out of memory for descriptor rings");
    };
    unsafe {
        RX_RING = rings; // 1 KiB at +0
        TX_RING = rings + 2048; // 256 B at +2 KiB (128-byte aligned)
        RX_BUFS = rxb; // 64 x 2 KiB
        TX_BUFS = txb; // 16 x 2 KiB
    }

    // --- igc_setup_tctl -----------------------------------------------------
    wr(TXDCTL, 0);
    let tctl = (rd(TCTL) & !TCTL_CT) | TCTL_PSP | TCTL_RTLC | (COLLISION_THRESHOLD << 4) | TCTL_EN;
    wr(TCTL, tctl);

    // --- igc_setup_rctl -----------------------------------------------------
    wr(RXDCTL, 0);
    let mut rctl = rd(RCTL);
    rctl &= !(RCTL_MO_MASK | RCTL_LBM_TCVR | RCTL_SBP | RCTL_SZ_MASK | RCTL_LPE | RCTL_UPE);
    // MPE (all multicast) as net_e1000 does; unicast filtered by RAR0.
    rctl |= RCTL_EN | RCTL_BAM | RCTL_MPE | RCTL_SECRC;
    wr(RCTL, rctl);

    // --- igc_configure_tx_ring ----------------------------------------------
    unsafe {
        wr(TXDCTL, 0);
        flush();
        wr(TDLEN, (TX_DESC_COUNT * 16) as u32);
        wr(TDBAL, TX_RING as u32);
        wr(TDBAH, (TX_RING as u64 >> 32) as u32);
        wr(TDH, 0);
        wr(TDT, 0);
        TX_TAIL = 0;
    }
    wr(TXDCTL, TXDCTL_PTHRESH | (TXDCTL_HTHRESH << 8) | (WTHRESH_POLLED << 16) | TXDCTL_QUEUE_ENABLE);
    if !timer::poll_until(10_000, || rd(TXDCTL) & TXDCTL_QUEUE_ENABLE != 0) {
        return fail("TX queue did not enable");
    }

    // --- igc_configure_rx_ring ----------------------------------------------
    unsafe {
        wr(RXDCTL, 0);
        wr(RDBAL, RX_RING as u32);
        wr(RDBAH, (RX_RING as u64 >> 32) as u32);
        wr(RDLEN, (RX_DESC_COUNT * 16) as u32);
        wr(RDH, 0);
        wr(RDT, 0);
        let mut srrctl = rd(SRRCTL);
        srrctl &= !(SRRCTL_BSIZEPKT_MASK | SRRCTL_BSIZEHDR_MASK | SRRCTL_DESCTYPE_MASK);
        srrctl |= (RX_HDR_LEN / 64) << 8; // IGC_SRRCTL_BSIZEHDR
        srrctl |= BUF_SIZE as u32 / 1024; // IGC_SRRCTL_BSIZEPKT
        srrctl |= SRRCTL_DESCTYPE_ADV_ONEBUF;
        wr(SRRCTL, srrctl);
        for i in 0..RX_DESC_COUNT {
            rx_arm(i);
        }
    }
    fence(Ordering::SeqCst);
    wr(RXDCTL, RXDCTL_PTHRESH | (RXDCTL_HTHRESH << 8) | (WTHRESH_POLLED << 16) | RXDCTL_QUEUE_ENABLE);
    if !timer::poll_until(10_000, || rd(RXDCTL) & RXDCTL_QUEUE_ENABLE != 0) {
        return fail("RX queue did not enable");
    }
    // igc_alloc_rx_buffers: hand the hardware all but one descriptor.
    unsafe {
        RX_TAIL = RX_DESC_COUNT - 1;
        wr(RDT, RX_TAIL as u32);
        UP = true;
    }

    // Autoneg takes ~2-4 s on a real 2.5G link. Bounded: with no cable this
    // just times out and the caller reports "no link".
    let lu = timer::poll_until(5_000_000, || rd(STATUS) & STATUS_LU != 0);
    log(b"net: igc: ");
    log(name.as_bytes());
    log(if lu { b" up, link UP\n" } else { b" up, NO LINK (cable unplugged?)\n" });
    Ok(())
}

/// Transmit one frame (same contract as `net_e1000::transmit`). Free slots
/// are tracked against the hardware head (TDH) rather than descriptor DD
/// write-backs, so delayed write-back can never wedge the ring.
pub fn transmit(buf: &[u8]) -> bool {
    if !is_up() || buf.len() > BUF_SIZE || buf.is_empty() {
        return false;
    }
    unsafe {
        let i = TX_TAIL;
        let next = (i + 1) % TX_DESC_COUNT;
        if next == rd(TDH) as usize {
            return false; // ring full
        }
        let b = TX_BUFS + i * BUF_SIZE;
        ptr::copy_nonoverlapping(buf.as_ptr(), b as *mut u8, buf.len());
        let len = buf.len() as u32;
        let cmd = len | ADVTXD_DTYP_DATA | ADVTXD_DCMD_DEXT | ADVTXD_DCMD_IFCS | ADVTXD_DCMD_EOP | ADVTXD_DCMD_RS;
        let olinfo = len << ADVTXD_PAYLEN_SHIFT;
        let d = desc(TX_RING, i);
        ptr::write_volatile(d, b as u64);
        ptr::write_volatile(d.add(1), cmd as u64 | ((olinfo as u64) << 32));
        fence(Ordering::SeqCst);
        TX_TAIL = next;
        wr(TDT, next as u32);
    }
    crate::kstats::net_tx(buf.len());
    true
}

/// Poll for one received frame (same contract as `net_e1000::receive`).
pub fn receive(out: &mut [u8]) -> Option<usize> {
    if !is_up() {
        return None;
    }
    unsafe {
        let next = (RX_TAIL + 1) % RX_DESC_COUNT;
        let d = desc(RX_RING, next);
        let hi = ptr::read_volatile(d.add(1));
        let status = hi as u32;
        if status & RXD_STAT_DD == 0 {
            return None;
        }
        fence(Ordering::Acquire);
        let flen = ((hi >> 32) & 0xFFFF) as usize;
        let mut got = None;
        // One 2 KiB buffer holds any non-jumbo frame (LPE off), so a frame
        // without EOP is malformed: drop it.
        if status & RXD_STAT_EOP != 0 {
            let len = flen.min(out.len()).min(BUF_SIZE);
            ptr::copy_nonoverlapping((RX_BUFS + next * BUF_SIZE) as *const u8, out.as_mut_ptr(), len);
            got = Some(len);
        }
        // write-back overwrote both words: re-arm with the buffer address
        rx_arm(next);
        fence(Ordering::SeqCst);
        RX_TAIL = next;
        wr(RDT, next as u32);
        if let Some(n) = got {
            crate::kstats::net_rx(n);
        }
        got
    }
}

/// Serial dump of the ring state, for "link up but nothing flows" reports.
pub fn debug_dump() {
    if !is_up() {
        log(b"net: igc: not initialised\n");
        return;
    }
    for (label, off) in [
        (&b"CTRL"[..], CTRL),
        (b"STATUS", STATUS),
        (b"RCTL", RCTL),
        (b"TCTL", TCTL),
        (b"RXDCTL", RXDCTL),
        (b"TXDCTL", TXDCTL),
        (b"RDH", RDH),
        (b"RDT", RDT),
        (b"TDH", TDH),
        (b"TDT", TDT),
        (b"SRRCTL", SRRCTL),
    ] {
        let s = alloc::format!("net: igc: {}=0x{:08x}\n", core::str::from_utf8(label).unwrap_or("?"), rd(off));
        log(s.as_bytes());
    }
}
