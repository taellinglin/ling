//! Raspberry Pi 5 wired Ethernet: the Cadence GEM MAC inside the RP1 (RP1
//! address 0x4010_0000, reached through the PCIe window `rp1` found for the
//! USB controllers) and the BCM54213PE PHY on its MDIO bus. Polled, like the
//! x86 e1000 driver whose interface this mirrors (`netstack`'s NIC on the
//! Pi): 32-bit descriptor rings and frame buffers in the uncached DMA pool.
//!
//! The MAC's transmit clock is left as the firmware set it (the gigabit rate),
//! so the PHY is asked for 1000BASE-T first; a 10/100 link is reported but
//! may not transmit until the RP1 clock is retuned for it. Everything prints
//! its progress once at init so a photo of the console says where a real
//! board stops.

use crate::arch::mmio::{read32, write32};
use crate::arch::paging::dma_alloc_pages;
use crate::arch::rp1;
use crate::arch::timer;
use crate::console_write as out;

const GEM_RP1: usize = 0x4010_0000;

// Cadence GEM registers.
const NCR: usize = 0x000;
const NCFGR: usize = 0x004;
const NSR: usize = 0x008;
const DMACFGR: usize = 0x010;
const TSR: usize = 0x014;
const RBQP: usize = 0x018;
const TBQP: usize = 0x01C;
const RSR: usize = 0x020;
const ISR: usize = 0x024;
const IDR: usize = 0x02C;
const MAN: usize = 0x034;
const SA1B: usize = 0x088;
const SA1T: usize = 0x08C;
const MID: usize = 0x0FC;
const DCFG6: usize = 0x294;
const TBQP1: usize = 0x440; // queues 1.. : 0x440 + 4*(q-1)
const RBQP1: usize = 0x480;

const NCR_MPE: u32 = 1 << 4;
const NCR_TXEN: u32 = 1 << 3;
const NCR_RXEN: u32 = 1 << 2;
const NCR_CLRSTAT: u32 = 1 << 5;
const NCR_TSTART: u32 = 1 << 9;
const NCFGR_SPD: u32 = 1 << 0;
const NCFGR_FD: u32 = 1 << 1;
const NCFGR_GBIT: u32 = 1 << 10;
const NCFGR_PAE: u32 = 1 << 13;
const NCFGR_DRFCS: u32 = 1 << 17;
const NCFGR_MDC_96: u32 = 5 << 18;
const NSR_IDLE: u32 = 1 << 2;

const RX_DESCS: usize = 32;
const TX_DESCS: usize = 8;
pub const BUF_SIZE: usize = 1536;
const RX_OWNED: u32 = 1 << 0; // software owns (frame written)
const RX_WRAP: u32 = 1 << 1;
const TX_USED: u32 = 1 << 31;
const TX_WRAP: u32 = 1 << 30;
const TX_LAST: u32 = 1 << 15;

struct Gem {
    base: usize,
    rx_ring: u64,
    tx_ring: u64,
    rx_bufs: u64,
    tx_bufs: u64,
    rx_head: usize,
    tx_tail: usize,
    mac: [u8; 6],
    phy: u32,
    speed: u32,
}

static mut GEM: Option<Gem> = None;
static mut INITED: bool = false;
static mut INIT_OK: bool = false;

fn gem() -> Option<&'static mut Gem> {
    unsafe { (*(&raw mut GEM)).as_mut() }
}

fn rd(base: usize, off: usize) -> u32 {
    unsafe { read32(base + off) }
}

fn wr(base: usize, off: usize, v: u32) {
    unsafe { write32(base + off, v) }
}

fn dsb() {
    unsafe { core::arch::asm!("dsb sy", options(nostack)) };
}

fn hex(label: &[u8], v: u64, nibbles: usize) {
    out(label);
    out(b"0x");
    let mut b = [0u8; 16];
    for i in 0..nibbles {
        let d = ((v >> ((nibbles - 1 - i) * 4)) & 0xF) as u8;
        b[i] = if d < 10 { b'0' + d } else { b'a' + d - 10 };
    }
    out(&b[..nibbles]);
    out(b"\n");
}

fn mac_str(m: &[u8; 6]) -> [u8; 17] {
    let mut s = [b':'; 17];
    for (i, b) in m.iter().enumerate() {
        let h = |d: u8| if d < 10 { b'0' + d } else { b'a' + d - 10 };
        s[i * 3] = h(b >> 4);
        s[i * 3 + 1] = h(b & 0xF);
    }
    s
}

// ── MDIO / PHY ───────────────────────────────────────────────────────────────

fn mdio_idle(base: usize) -> bool {
    timer::poll_until(50_000, || rd(base, NSR) & NSR_IDLE != 0)
}

fn mdio_read(base: usize, phy: u32, reg: u32) -> Option<u16> {
    if !mdio_idle(base) {
        return None;
    }
    wr(base, MAN, (1 << 30) | (2 << 28) | ((phy & 31) << 23) | ((reg & 31) << 18) | (2 << 16));
    if !mdio_idle(base) {
        return None;
    }
    Some(rd(base, MAN) as u16)
}

fn mdio_write(base: usize, phy: u32, reg: u32, v: u16) -> bool {
    if !mdio_idle(base) {
        return false;
    }
    wr(base, MAN, (1 << 30) | (1 << 28) | ((phy & 31) << 23) | ((reg & 31) << 18) | (2 << 16) | v as u32);
    mdio_idle(base)
}

/// Broadcom PHYs in RGMII-ID mode need their internal clock delays on: the
/// RXD-to-RXC skew (AUX_CTL shadow 7) and the GTXCLK delay (shadow 0x1C
/// register 3), the way Linux's broadcom PHY driver sets them.
fn bcm_rgmii_delays(base: usize, phy: u32) {
    mdio_write(base, phy, 0x18, 0x0007 | (7 << 12));
    if let Some(misc) = mdio_read(base, phy, 0x18) {
        mdio_write(base, phy, 0x18, 0x0007 | 0x8000 | misc | 0x0100);
    }
    mdio_write(base, phy, 0x1C, 0x03 << 10);
    if let Some(clk) = mdio_read(base, phy, 0x1C) {
        mdio_write(base, phy, 0x1C, 0x8000 | (0x03 << 10) | ((clk & 0x3FF) | (1 << 9)));
    }
}

/// Link up (BMSR bit 2 is latched low: read twice).
fn phy_link(base: usize, phy: u32) -> bool {
    let _ = mdio_read(base, phy, 1);
    mdio_read(base, phy, 1).map(|s| s & 0x4 != 0).unwrap_or(false)
}

/// Negotiated (speed Mb/s, full duplex) once the link is up.
fn phy_speed(base: usize, phy: u32) -> (u32, bool) {
    let gbsr = mdio_read(base, phy, 10).unwrap_or(0);
    let gbcr = mdio_read(base, phy, 9).unwrap_or(0);
    if gbsr & 0x0800 != 0 && gbcr & 0x0200 != 0 {
        return (1000, true);
    }
    if gbsr & 0x0400 != 0 && gbcr & 0x0100 != 0 {
        return (1000, false);
    }
    let lp = mdio_read(base, phy, 5).unwrap_or(0) & mdio_read(base, phy, 4).unwrap_or(0);
    if lp & 0x0100 != 0 {
        (100, true)
    } else if lp & 0x0080 != 0 {
        (100, false)
    } else if lp & 0x0040 != 0 {
        (10, true)
    } else {
        (10, false)
    }
}

// ── MAC ──────────────────────────────────────────────────────────────────────

pub fn ensure_init() -> bool {
    unsafe {
        if INITED {
            return INIT_OK;
        }
        INITED = true;
    }
    let ok = init().is_ok();
    unsafe { INIT_OK = ok };
    ok
}

/// A MAC address: what the bootloader programmed into the MAC, else the
/// Raspberry Pi OUI with the board serial, else a locally administered one.
fn choose_mac(base: usize) -> [u8; 6] {
    let lo = rd(base, SA1B);
    let hi = rd(base, SA1T);
    let m = [lo as u8, (lo >> 8) as u8, (lo >> 16) as u8, (lo >> 24) as u8, hi as u8, (hi >> 8) as u8];
    let zero = m.iter().all(|&b| b == 0);
    let ones = m.iter().all(|&b| b == 0xFF);
    if !zero && !ones && m[0] & 1 == 0 {
        return m;
    }
    if let Some(serial) = crate::arch::mailbox::board_serial() {
        return [0xd8, 0x3a, 0xdd, (serial >> 16) as u8, (serial >> 8) as u8, serial as u8];
    }
    [0x02, 0x4c, 0x69, 0x6e, 0x67, 0x4f]
}

/// Why the last `init` gave up ("" after a success): for Settings > Network.
static mut LAST_ERR: &str = "";

pub fn last_error() -> &'static str {
    unsafe { LAST_ERR }
}

fn fail(why: &'static str) -> Result<(), ()> {
    unsafe { LAST_ERR = why };
    Err(())
}

pub fn init() -> Result<(), ()> {
    let Some(win) = rp1::rp1_window() else {
        out(b"eth: no RP1 (the wired port is a Pi 5 feature)\n");
        return fail("no RP1 southbridge (Pi 5 only)");
    };
    let base = rp1::at(win, GEM_RP1);
    out(b"Ethernet (RP1 GEM):\n");
    let id = rd(base, MID);
    hex(b"  module id=", id as u64, 8);
    if id == 0 || id == 0xFFFF_FFFF {
        out(b"  no GEM at the RP1 address\n");
        return fail("no GEM controller at the RP1 address");
    }
    // Quiesce: MAC off, statistics and status cleared, interrupts masked.
    wr(base, NCR, 0);
    wr(base, NCR, NCR_CLRSTAT);
    wr(base, TSR, 0xFF);
    wr(base, RSR, 0xF);
    wr(base, IDR, 0xFFFF_FFFF);
    let _ = rd(base, ISR);

    let mac = choose_mac(base);
    wr(base, SA1B, u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]));
    wr(base, SA1T, u32::from_le_bytes([mac[4], mac[5], 0, 0]));
    out(b"  MAC ");
    out(&mac_str(&mac));
    out(b"\n");

    // Gigabit, full duplex, MDC = pclk/96, strip the FCS, honour pause frames.
    wr(base, NCFGR, NCFGR_GBIT | NCFGR_FD | NCFGR_MDC_96 | NCFGR_DRFCS | NCFGR_PAE);
    wr(base, NCR, NCR_MPE);

    // The PHY: first address that answers with a vendor id.
    let mut phy = 32u32;
    let mut id1 = 0u16;
    for a in 0..32 {
        if let Some(v) = mdio_read(base, a, 2) {
            if v != 0 && v != 0xFFFF {
                phy = a;
                id1 = v;
                break;
            }
        }
    }
    if phy == 32 {
        out(b"  no PHY answered on the MDIO bus\n");
        return fail("no PHY answered on the MDIO bus");
    }
    let id2 = mdio_read(base, phy, 3).unwrap_or(0);
    out(b"  PHY at address ");
    out(&[b'0' + (phy / 10) as u8, b'0' + (phy % 10) as u8]);
    hex(b", id=", ((id1 as u64) << 16) | id2 as u64, 8);
    if id1 == 0x600d || id1 == 0xae02 || id1 == 0x0362 {
        bcm_rgmii_delays(base, phy);
    }
    // Auto-negotiate everything; the MAC follows what the partner gives.
    mdio_write(base, phy, 4, 0x01E1);
    mdio_write(base, phy, 9, 0x0300);
    mdio_write(base, phy, 0, 0x1200);
    let linked = timer::poll_until(6_000_000, || phy_link(base, phy));
    let (speed, fd) = if linked { phy_speed(base, phy) } else { (0, false) };
    if linked {
        out(b"  link up: ");
        out(match speed {
            1000 => b"1000 Mb/s" as &[u8],
            100 => b"100 Mb/s",
            _ => b"10 Mb/s",
        });
        out(if fd { b" full duplex\n" } else { b" half duplex\n" });
        let mut cfg = NCFGR_MDC_96 | NCFGR_DRFCS | NCFGR_PAE;
        if speed == 1000 {
            cfg |= NCFGR_GBIT;
        } else {
            out(b"  (10/100: the transmit clock is still at the gigabit rate -- sending may not work yet)\n");
            if speed == 100 {
                cfg |= NCFGR_SPD;
            }
        }
        if fd {
            cfg |= NCFGR_FD;
        }
        wr(base, NCFGR, cfg);
    } else {
        out(b"  no link (no cable, or the partner is still negotiating)\n");
    }

    // Rings and buffers in uncached memory.
    let rings = dma_alloc_pages(2).ok_or(())?;
    let (rx_ring, tx_ring) = (rings, rings + 4096);
    let rx_bufs = dma_alloc_pages(RX_DESCS * BUF_SIZE / 4096).ok_or(())?;
    let tx_bufs = dma_alloc_pages(TX_DESCS * BUF_SIZE / 4096).ok_or(())?;
    let dummy = dma_alloc_pages(1).ok_or(())?;
    unsafe {
        for i in 0..RX_DESCS {
            let d = (rx_ring + (i * 8) as u64) as *mut u32;
            let addr = (rx_bufs + (i * BUF_SIZE) as u64) as u32 & !3;
            core::ptr::write_volatile(d, addr | if i == RX_DESCS - 1 { RX_WRAP } else { 0 });
            core::ptr::write_volatile(d.add(1), 0);
        }
        for i in 0..TX_DESCS {
            let d = (tx_ring + (i * 8) as u64) as *mut u32;
            core::ptr::write_volatile(d, (tx_bufs + (i * BUF_SIZE) as u64) as u32);
            core::ptr::write_volatile(d.add(1), TX_USED | if i == TX_DESCS - 1 { TX_WRAP } else { 0 });
        }
        // Unused queues point at one "used" descriptor each way.
        let dtx = dummy as *mut u32;
        core::ptr::write_volatile(dtx, 0);
        core::ptr::write_volatile(dtx.add(1), TX_USED | TX_WRAP);
        let drx = (dummy + 64) as *mut u32;
        core::ptr::write_volatile(drx, RX_OWNED | RX_WRAP);
        core::ptr::write_volatile(drx.add(1), 0);
    }
    dsb();
    wr(base, RBQP, rx_ring as u32);
    wr(base, TBQP, tx_ring as u32);
    let queue_mask = 0x1 | (rd(base, DCFG6) & 0xFF);
    for q in 1..16 {
        if queue_mask & (1 << q) != 0 {
            wr(base, TBQP1 + 4 * (q - 1), dummy as u32);
            wr(base, RBQP1 + 4 * (q - 1), (dummy + 64) as u32);
        }
    }
    // DMA: 16-beat bursts, full packet buffers, 1536-byte receive buffers.
    wr(base, DMACFGR, 0x10 | (3 << 8) | (1 << 10) | ((BUF_SIZE as u32 / 64) << 16));
    wr(base, NCR, NCR_MPE | NCR_TXEN | NCR_RXEN);
    unsafe {
        core::ptr::write(
            &raw mut GEM,
            Some(Gem { base, rx_ring, tx_ring, rx_bufs, tx_bufs, rx_head: 0, tx_tail: 0, mac, phy, speed }),
        );
    }
    out(b"  MAC running\n");
    Ok(())
}

pub fn mac() -> [u8; 6] {
    gem().map(|g| g.mac).unwrap_or([0; 6])
}

pub fn link_up() -> bool {
    gem().map(|g| phy_link(g.base, g.phy)).unwrap_or(false)
}

pub fn speed() -> u32 {
    gem().map(|g| g.speed).unwrap_or(0)
}

pub fn transmit(buf: &[u8]) -> bool {
    let Some(g) = gem() else { return false };
    if buf.len() > BUF_SIZE || buf.is_empty() {
        return false;
    }
    let i = g.tx_tail;
    let d = (g.tx_ring + (i * 8) as u64) as *mut u32;
    let used = || unsafe { core::ptr::read_volatile(d.add(1)) } & TX_USED != 0;
    if !used() && !timer::poll_until(20_000, used) {
        return false; // still in flight
    }
    unsafe {
        let dst = (g.tx_bufs + (i * BUF_SIZE) as u64) as *mut u8;
        core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, buf.len());
        dsb();
        let wrap = if i == TX_DESCS - 1 { TX_WRAP } else { 0 };
        core::ptr::write_volatile(d.add(1), (buf.len() as u32 & 0x3FFF) | TX_LAST | wrap);
    }
    dsb();
    wr(g.base, NCR, rd(g.base, NCR) | NCR_TSTART);
    g.tx_tail = (i + 1) % TX_DESCS;
    true
}

pub fn receive(out: &mut [u8]) -> Option<usize> {
    let g = gem()?;
    let i = g.rx_head;
    let d = (g.rx_ring + (i * 8) as u64) as *mut u32;
    dsb();
    let w0 = unsafe { core::ptr::read_volatile(d) };
    if w0 & RX_OWNED == 0 {
        let rsr = rd(g.base, RSR);
        if rsr & 0x5 != 0 {
            wr(g.base, RSR, rsr); // buffer-not-available / overrun noted
        }
        return None;
    }
    let w1 = unsafe { core::ptr::read_volatile(d.add(1)) };
    let len = ((w1 & 0x1FFF) as usize).min(out.len()).min(BUF_SIZE);
    unsafe {
        let src = (g.rx_bufs + (i * BUF_SIZE) as u64) as *const u8;
        core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), len);
        core::ptr::write_volatile(d, w0 & !RX_OWNED); // hand the buffer back
    }
    dsb();
    g.rx_head = (i + 1) % RX_DESCS;
    Some(len)
}

pub fn debug_dump() {
    let Some(g) = gem() else {
        out(b"eth: not initialised\n");
        return;
    };
    hex(b"eth: NCR=", rd(g.base, NCR) as u64, 8);
    hex(b"eth: NCFGR=", rd(g.base, NCFGR) as u64, 8);
    hex(b"eth: NSR=", rd(g.base, NSR) as u64, 8);
    hex(b"eth: RSR=", rd(g.base, RSR) as u64, 8);
    hex(b"eth: TSR=", rd(g.base, TSR) as u64, 8);
    hex(b"eth: ISR=", rd(g.base, ISR) as u64, 8);
    hex(b"eth: rx head desc w0=", unsafe { core::ptr::read_volatile((g.rx_ring + (g.rx_head * 8) as u64) as *const u32) } as u64, 8);
    out(if link_up() { b"eth: link up\n" } else { b"eth: link down\n" });
}

// ── IP configuration (what the netstack reads; DHCP fills it in) ─────────────

static mut CFG_IP: [u8; 4] = [0, 0, 0, 0];
static mut CFG_GW: [u8; 4] = [0, 0, 0, 0];
static mut CFG_MASK: [u8; 4] = [255, 255, 255, 0];

pub fn self_ip() -> [u8; 4] {
    unsafe { CFG_IP }
}
pub fn gateway_ip() -> [u8; 4] {
    unsafe { CFG_GW }
}
pub fn subnet_mask() -> [u8; 4] {
    unsafe { CFG_MASK }
}
pub fn set_ip_config(ip: [u8; 4], mask: [u8; 4], gw: [u8; 4]) {
    unsafe {
        CFG_IP = ip;
        CFG_MASK = mask;
        CFG_GW = gw;
    }
}

/// Broadcast an ARP request for the gateway and wait for its reply: the
/// cheapest proof that frames go out and come back.
pub fn arp_selftest() -> Option<[u8; 6]> {
    let my_mac = mac();
    let mut frame = [0u8; 60];
    frame[0..6].copy_from_slice(&[0xFF; 6]);
    frame[6..12].copy_from_slice(&my_mac);
    frame[12..14].copy_from_slice(&[0x08, 0x06]);
    frame[14..16].copy_from_slice(&[0x00, 0x01]);
    frame[16..18].copy_from_slice(&[0x08, 0x00]);
    frame[18] = 6;
    frame[19] = 4;
    frame[20..22].copy_from_slice(&[0x00, 0x01]);
    frame[22..28].copy_from_slice(&my_mac);
    frame[28..32].copy_from_slice(&self_ip());
    frame[38..42].copy_from_slice(&gateway_ip());
    if !transmit(&frame) {
        return None;
    }
    let mut buf = [0u8; BUF_SIZE];
    let mut found = None;
    timer::poll_until(3_000_000, || {
        if let Some(len) = receive(&mut buf) {
            if len >= 42 && buf[12] == 0x08 && buf[13] == 0x06 && buf[20] == 0x00 && buf[21] == 0x02 && buf[28..32] == gateway_ip() {
                let mut reply_mac = [0u8; 6];
                reply_mac.copy_from_slice(&buf[22..28]);
                found = Some(reply_mac);
                return true;
            }
        }
        false
    });
    found
}
