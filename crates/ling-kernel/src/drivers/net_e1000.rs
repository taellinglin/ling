//! Intel 82540EM ("e1000") Gigabit Ethernet driver — the default NIC model
//! both QEMU (`-nic model=e1000`) and VirtualBox ("Intel PRO/1000" adapter
//! type in VM network settings) actually emulate, making it the one NIC
//! worth writing first for this project (see the networking plan this was
//! built from: "E1000 being both QEMU's and VirtualBox's actual default NIC
//! ... makes E1000 the clear driver choice"). Same shape as `ahci.rs`: free
//! functions (no trait), found via PCI (class 0x02 Ethernet, no vendor/
//! device check — a VM has exactly one NIC in practice), static
//! preallocated descriptor rings and packet buffers (no heap), bounded
//! polls via `arch::timer` rather than trusting hardware to always respond.
//!
//! Real limitation, disclosed rather than hidden: no interrupt handling
//! (IRQ line exists on real e1000 hardware, but this driver never unmasks
//! it) — `receive()` is pure polling, same idiom as every other driver in
//! this kernel. Fine for a cooperative, single-task-at-a-time kernel with
//! no preemption to lose while spinning.
//!
//! **Current status, honestly: partially verified, not yet fully working.**
//! Confirmed via QEMU (`-nic user,model=e1000`) + serial log: the PCI device
//! is found (vendor/device 8086:100e — genuinely an 82540EM, not a
//! misidentified device), BAR0 resolves to a plausible, correctly-behaving
//! MMIO address, reset completes, the real MAC reads correctly via EERD/
//! EEPROM (matches QEMU's monitor exactly — an earlier apparent mismatch
//! was a debug-print bug, not a driver bug: the MAC bytes were printed with
//! `print_decimal` and misread as hex), `STATUS.LU` (link up) reads true,
//! `transmit()`'s frame content is byte-perfect in the TX buffer (verified
//! via a raw memory dump against the exact expected ARP request bytes), and
//! the descriptor ring mechanics run (`TDH` advances past the descriptor
//! and `ICR.TXQE` fires, both real device-driven effects — nothing in this
//! driver writes `TDH` itself). Real bugs found and fixed along the way:
//! frame padded to Ethernet's 60-byte minimum; descriptor `status` reads
//! switched to `read_volatile`/`addr_of!` (a plain field read is fair game
//! for the compiler to hoist across a polling loop); `TXDCTL`/`RXDCTL` now
//! configured (some driver references treat this as required, not optional).
//!
//! **RESOLVED (2026-08-26)** -- the long-standing "byte-perfect frame,
//! TDH advances, TXQE fires, yet 0 bytes ever reach the backend" mystery
//! had two stacked root causes, found via the same filter-dump setup that
//! originally proved the silence:
//!
//! 1. **PCI bus mastering was never enabled.** Without PCI_COMMAND.BM the
//!    device cannot DMA; QEMU's PCI core makes its descriptor reads
//!    return zeros, so the model "processed" all-zero descriptors (TDH
//!    still advances, TXQE still fires -- the exact observed signals) and
//!    emitted zero-length frames the capture never saw, while the real
//!    frame sat unread in guest memory. SeaBIOS only sets BM on devices
//!    it boots from -- disk worked, NIC didn't. `init()` now sets
//!    MEM+BM explicitly.
//! 2. **The RX wait was ~50x shorter than intended.** TSC calibration
//!    under QEMU TCG runs tens of times fast, so `arp_selftest`'s "2s"
//!    poll was ~20-40ms of wall clock -- shorter than QEMU's deferred RX
//!    delivery. The budget is now 30s of kernel time.
//!
//! With both fixed: `nettest`'s ARP selftest PASSES against QEMU SLIRP --
//! request on the wire (pcap-verified), gateway reply received and parsed
//! by `receive()`. TX and RX are confirmed working end to end; the IP
//! stack can build on this for real.
use crate::arch::{pci, timer};
use core::ptr;

// Register byte offsets from the MMIO BAR0 base.
const REG_CTRL: usize = 0x0000;
const REG_STATUS: usize = 0x0008;
const REG_EERD: usize = 0x0014;
const REG_ICR: usize = 0x00C0;
const REG_IMC: usize = 0x00D8;
const REG_RCTL: usize = 0x0100;
const REG_TCTL: usize = 0x0400;
const REG_TIPG: usize = 0x0410;
const REG_TXDCTL: usize = 0x3828;
const REG_RXDCTL: usize = 0x2828;
const REG_RDBAL: usize = 0x2800;
const REG_RDBAH: usize = 0x2804;
const REG_RDLEN: usize = 0x2808;
const REG_RDH: usize = 0x2810;
const REG_RDT: usize = 0x2818;
const REG_TDBAL: usize = 0x3800;
const REG_TDBAH: usize = 0x3804;
const REG_TDLEN: usize = 0x3808;
const REG_TDH: usize = 0x3810;
const REG_TDT: usize = 0x3818;
const REG_MTA: usize = 0x5200;
const REG_RAL0: usize = 0x5400;
const REG_RAH0: usize = 0x5404;

const CTRL_RST: u32 = 1 << 26;
const CTRL_SLU: u32 = 1 << 6;
const CTRL_ASDE: u32 = 1 << 5;
const STATUS_LU: u32 = 1 << 1;

const RCTL_EN: u32 = 1 << 1;
const RCTL_UPE: u32 = 1 << 3;
const RCTL_MPE: u32 = 1 << 4;
const RCTL_BAM: u32 = 1 << 15;
const RCTL_BSIZE_2048: u32 = 0; // bits 16-17 = 00, BSEX (bit 25) = 0
const RCTL_SECRC: u32 = 1 << 26;

const TCTL_EN: u32 = 1 << 1;
const TCTL_PSP: u32 = 1 << 3;
const TCTL_CT: u32 = 0x0F << 4; // collision threshold 15, standard value
const TCTL_COLD: u32 = 0x40 << 12; // collision distance, standard full-duplex value

const RX_DESC_COUNT: usize = 32;
const TX_DESC_COUNT: usize = 8;
const BUF_SIZE: usize = 2048;

const RXD_STAT_DD: u8 = 1 << 0;
const TXD_CMD_EOP: u8 = 1 << 0;
const TXD_CMD_IFCS: u8 = 1 << 1;
const TXD_CMD_RS: u8 = 1 << 3;
const TXD_STAT_DD: u8 = 1 << 0;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct RxDesc {
    addr: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct TxDesc {
    addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

#[repr(C, align(16))]
struct RxRing([RxDesc; RX_DESC_COUNT]);
#[repr(C, align(16))]
struct TxRing([TxDesc; TX_DESC_COUNT]);
#[repr(C, align(16))]
struct PacketBuf([u8; BUF_SIZE]);

static mut RX_RING: RxRing = RxRing(
    [RxDesc { addr: 0, length: 0, checksum: 0, status: 0, errors: 0, special: 0 };
        RX_DESC_COUNT],
);
static mut TX_RING: TxRing = TxRing(
    [TxDesc { addr: 0, length: 0, cso: 0, cmd: 0, status: 0, css: 0, special: 0 };
        TX_DESC_COUNT],
);
static mut RX_BUFS: [PacketBuf; RX_DESC_COUNT] = [const { PacketBuf([0; BUF_SIZE]) }; RX_DESC_COUNT];
static mut TX_BUFS: [PacketBuf; TX_DESC_COUNT] = [const { PacketBuf([0; BUF_SIZE]) }; TX_DESC_COUNT];
static mut RX_TAIL: usize = 0;
static mut TX_TAIL: usize = 0;

static mut MMIO_BASE: usize = 0;
static mut MAC: [u8; 6] = [0; 6];

fn reg_read(offset: usize) -> u32 {
    unsafe { ptr::read_volatile((MMIO_BASE + offset) as *const u32) }
}
fn reg_write(offset: usize, val: u32) {
    unsafe { ptr::write_volatile((MMIO_BASE + offset) as *mut u32, val) };
}

/// Read one 16-bit EEPROM word via EERD -- the 82540EM's "microwire"-era
/// register layout (START=bit0, DONE=bit4, ADDR=bits8-15, DATA=bits16-31;
/// newer e1000e-family chips use a different DONE bit position, but QEMU
/// emulates the 82540EM specifically). Bounded poll: returns 0 (not a
/// crash) if the EEPROM never responds, since this is the MAC-address read
/// path and a garbled-but-present MAC is more debuggable than a hang.
fn eeprom_read(addr: u8) -> u16 {
    reg_write(REG_EERD, ((addr as u32) << 8) | 1);
    let mut result = 0u16;
    timer::poll_until(100_000, || {
        let v = reg_read(REG_EERD);
        if v & (1 << 4) != 0 {
            result = (v >> 16) as u16;
            true
        } else {
            false
        }
    });
    result
}

/// Find the e1000, reset it, read its MAC, bring up static RX/TX rings, and
/// enable the receiver/transmitter. Returns `Err` if no PCI Ethernet
/// controller is present at all — callers should treat "no networking" as
/// a normal, expected outcome (e.g. real hardware with a different NIC),
/// not a fatal error.
static mut INITED: bool = false;

/// Idempotent wrapper: init once, remember the outcome. The polled stack
/// (`netstack`/`lingfu`) calls this on every operation rather than
/// trusting some earlier command to have run `net_init` -- a full re-init
/// mid-flow would tear down live descriptor rings.
pub fn ensure_init() -> bool {
    unsafe {
        if INITED {
            return true;
        }
    }
    let ok = init().is_ok();
    unsafe {
        INITED = ok;
    }
    ok
}

pub fn init() -> Result<(), ()> {
    let loc = pci::find(0x02, 0x00, 0x00).ok_or(())?;
    let bar0 = loc.bar(0);
    if bar0 == 0 {
        return Err(());
    }
    unsafe { MMIO_BASE = bar0 as usize };

    // Enable PCI memory space + BUS MASTERING. This was the module doc's
    // long-unexplained "frame never reaches the backend" mystery: without
    // PCI_COMMAND.BM (bit 2) the device cannot DMA, and QEMU's PCI core
    // makes its descriptor reads return zeros -- the model then happily
    // "processes" all-zero descriptors (TDH advances, ICR.TXQE fires,
    // exactly the observed device-driven effects) and emits zero-length
    // frames the filter-dump capture never sees, while the byte-perfect
    // frame sits unread in OUR buffer. SeaBIOS only sets BM on devices it
    // boots from, which is why the disk worked and the NIC didn't.
    let cmd = loc.read32(0x04);
    pci::write32(loc.bus, loc.device, loc.function, 0x04, cmd | 0x0006);

    // Full device reset, then wait for it to clear (the standard e1000
    // reset idiom) -- bounded, not trusted to always self-clear promptly.
    reg_write(REG_CTRL, reg_read(REG_CTRL) | CTRL_RST);
    timer::poll_until(50_000, || reg_read(REG_CTRL) & CTRL_RST == 0);
    timer::delay_us(1_000);

    // Mask every interrupt (this driver polls, never handles IRQs) and
    // clear any pending cause left over from before the reset.
    reg_write(REG_IMC, 0xFFFF_FFFF);
    reg_read(REG_ICR);

    // Zero the multicast table -- QEMU/VirtualBox don't require this, but a
    // real card's MTA is undefined after reset, not guaranteed zero.
    for i in 0..128 {
        reg_write(REG_MTA + i * 4, 0);
    }

    // MAC address via the EEPROM (EERD) -- RAL0/RAH0 aren't a reliable
    // shortcut here (unclear whether/when QEMU shadows the EEPROM into
    // them), so this always goes through the real EEPROM read path real
    // drivers use.
    let w0 = eeprom_read(0);
    let w1 = eeprom_read(1);
    let w2 = eeprom_read(2);
    unsafe {
        MAC = [
            w0 as u8,
            (w0 >> 8) as u8,
            w1 as u8,
            (w1 >> 8) as u8,
            w2 as u8,
            (w2 >> 8) as u8,
        ];
    }

    // Link up without real autonegotiation -- SLU forces the link-up state
    // machine to report up as soon as the phy/virtual link is ready, ASDE
    // enables auto speed detection so this works without hand-picking a
    // speed/duplex.
    reg_write(REG_CTRL, reg_read(REG_CTRL) | CTRL_SLU | CTRL_ASDE);

    setup_rx();
    setup_tx();

    // Best-effort: log-worthy but not fatal if link isn't up yet within the
    // timeout -- some emulated links settle a little after this point, and
    // a caller can check `link_up()` again later before actually sending.
    timer::poll_until(500_000, || reg_read(REG_STATUS) & STATUS_LU != 0);

    Ok(())
}

fn setup_rx() {
    unsafe {
        for i in 0..RX_DESC_COUNT {
            RX_RING.0[i] = RxDesc {
                addr: RX_BUFS[i].0.as_ptr() as u64,
                length: 0,
                checksum: 0,
                status: 0,
                errors: 0,
                special: 0,
            };
        }
        let base = RX_RING.0.as_ptr() as u64;
        reg_write(REG_RDBAL, base as u32);
        reg_write(REG_RDBAH, (base >> 32) as u32);
        reg_write(REG_RDLEN, (RX_DESC_COUNT * core::mem::size_of::<RxDesc>()) as u32);
        reg_write(REG_RDH, 0);
        reg_write(REG_RDT, (RX_DESC_COUNT - 1) as u32);
        RX_TAIL = RX_DESC_COUNT - 1;
    }
    reg_write(REG_RXDCTL, (1 << 24) | (1 << 16)); // see setup_tx's TXDCTL comment
    reg_write(
        REG_RCTL,
        RCTL_EN | RCTL_UPE | RCTL_MPE | RCTL_BAM | RCTL_BSIZE_2048 | RCTL_SECRC,
    );
}

fn setup_tx() {
    unsafe {
        for i in 0..TX_DESC_COUNT {
            TX_RING.0[i] = TxDesc {
                addr: TX_BUFS[i].0.as_ptr() as u64,
                length: 0,
                cso: 0,
                cmd: 0,
                status: TXD_STAT_DD, // "done" so the first send finds it free
                css: 0,
                special: 0,
            };
        }
        let base = TX_RING.0.as_ptr() as u64;
        reg_write(REG_TDBAL, base as u32);
        reg_write(REG_TDBAH, (base >> 32) as u32);
        reg_write(REG_TDLEN, (TX_DESC_COUNT * core::mem::size_of::<TxDesc>()) as u32);
        reg_write(REG_TDH, 0);
        reg_write(REG_TDT, 0);
        TX_TAIL = 0;
    }
    reg_write(REG_TIPG, 10 | (8 << 10) | (6 << 20)); // standard IPG timings
    // TXDCTL: some e1000 driver references treat this as required for the
    // transmit engine to actually dispatch queued packets, not merely a
    // write-back-timing tuning knob -- GRAN=1 (bit24, descriptor
    // granularity) + WTHRESH=1 (bits16-21), a commonly-cited minimal-working
    // setting. Trying this because TDH advances/ICR.TXQE fires (proving the
    // ring mechanics work) yet the frame provably never reaches the network
    // backend (confirmed via a QEMU `filter-dump` packet capture showing
    // zero bytes) -- unexplained by anything else checked so far.
    reg_write(REG_TXDCTL, (1 << 24) | (1 << 16));
    reg_write(REG_TCTL, TCTL_EN | TCTL_PSP | TCTL_CT | TCTL_COLD);
}

pub fn mac() -> [u8; 6] {
    unsafe { MAC }
}

pub fn link_up() -> bool {
    reg_read(REG_STATUS) & STATUS_LU != 0
}

fn log_hex(label: &[u8], v: u32) {
    crate::drivers::serial::write(label);
    let mut buf = [0u8; 8];
    for i in 0..8 {
        let nibble = (v >> (28 - i * 4)) & 0xF;
        buf[i as usize] = if nibble < 10 {
            b'0' + nibble as u8
        } else {
            b'a' + (nibble - 10) as u8
        };
    }
    crate::drivers::serial::write(&buf);
    crate::drivers::serial::write(b"\n");
}

pub fn debug_dump() {
    log_hex(b"net: CTRL=0x", reg_read(REG_CTRL));
    log_hex(b"net: STATUS=0x", reg_read(REG_STATUS));
    log_hex(b"net: TCTL=0x", reg_read(REG_TCTL));
    log_hex(b"net: RCTL=0x", reg_read(REG_RCTL));
    log_hex(b"net: TDBAL=0x", reg_read(REG_TDBAL));
    log_hex(b"net: TDBAH=0x", reg_read(REG_TDBAH));
    log_hex(b"net: TDLEN=0x", reg_read(REG_TDLEN));
    log_hex(b"net: TDH=0x", reg_read(REG_TDH));
    log_hex(b"net: TDT=0x", reg_read(REG_TDT));
    unsafe {
        log_hex(b"net: ring_addr=0x", TX_RING.0.as_ptr() as u32);
    }
}

/// Descriptor `status` bytes are written by the NIC's own DMA engine, not
/// by any code the compiler can see — a plain field read/write on the
/// `static mut` ring is fair game for the compiler to hoist/cache across a
/// polling loop (nothing tells it this memory changes from outside), which
/// is exactly what happened here: confirmed via `debug_dump()` showing TDH
/// genuinely advance (proof the NIC really sent the frame) while a plain
/// `TX_RING.0[i].status` read inside the completion poll loop never once
/// observed the DD bit the hardware had actually written. `read_volatile`/
/// `write_volatile` on the field's raw address (via `addr_of!`/
/// `addr_of_mut!`, required for a `#[repr(packed)]` field — a plain `&`
/// would create an unaligned reference) forces a fresh memory access every
/// time instead.
unsafe fn tx_status(i: usize) -> u8 {
    ptr::read_volatile(ptr::addr_of!(TX_RING.0[i].status))
}
unsafe fn tx_status_set(i: usize, v: u8) {
    ptr::write_volatile(ptr::addr_of_mut!(TX_RING.0[i].status), v);
}
unsafe fn rx_status(i: usize) -> u8 {
    ptr::read_volatile(ptr::addr_of!(RX_RING.0[i].status))
}
unsafe fn rx_status_set(i: usize, v: u8) {
    ptr::write_volatile(ptr::addr_of_mut!(RX_RING.0[i].status), v);
}

/// Transmit one frame. Returns `false` if `buf` is too big for a single
/// packet buffer or the next TX descriptor isn't free yet (ring full —
/// shouldn't happen at this kernel's traffic rate, but checked rather than
/// assumed).
pub fn transmit(buf: &[u8]) -> bool {
    if buf.len() > BUF_SIZE {
        return false;
    }
    unsafe {
        let i = TX_TAIL;
        if tx_status(i) & TXD_STAT_DD == 0 {
            return false; // descriptor still in flight
        }
        TX_BUFS[i].0[..buf.len()].copy_from_slice(buf);
        TX_RING.0[i].length = buf.len() as u16;
        TX_RING.0[i].cmd = TXD_CMD_EOP | TXD_CMD_IFCS | TXD_CMD_RS;
        tx_status_set(i, 0);
        TX_TAIL = (i + 1) % TX_DESC_COUNT;
        reg_write(REG_TDT, TX_TAIL as u32);
    }
    true
}

/// Poll for one received frame. Returns its length if a new packet arrived
/// (copied into `out`, truncated if `out` is smaller than the frame) —
/// `None` if nothing's waiting, the normal case on every call between real
/// packets.
pub fn receive(out: &mut [u8]) -> Option<usize> {
    unsafe {
        let next = (RX_TAIL + 1) % RX_DESC_COUNT;
        if rx_status(next) & RXD_STAT_DD == 0 {
            return None;
        }
        let len = (RX_RING.0[next].length as usize).min(out.len());
        out[..len].copy_from_slice(&RX_BUFS[next].0[..len]);
        rx_status_set(next, 0);
        RX_TAIL = next;
        reg_write(REG_RDT, RX_TAIL as u32);
        Some(len)
    }
}

/// Our claimed IPv4 address for the self-test/static-IP phase (skipping
/// DHCP — see the networking plan's phase-ordering doc) — a fixed address
/// in QEMU SLIRP's default 10.0.2.0/24 range, one the SLIRP gateway itself
/// doesn't use (.2 is the gateway, .3 is its DNS resolver).
pub const SELF_IP: [u8; 4] = [10, 0, 2, 15];
/// QEMU/VirtualBox SLIRP's default gateway address.
pub const GATEWAY_IP: [u8; 4] = [10, 0, 2, 2];

/// Hand-built ARP request/reply round trip — deliberately not going through
/// smoltcp at all, so this isolates genuinely new code (descriptor rings,
/// DMA, the reset sequence) from a full IP stack's own logic, per the
/// networking plan's phase-ordering doc ("cheapest failure-isolation
/// first"). Sends a broadcast ARP request for `GATEWAY_IP` claiming
/// `SELF_IP`, polls for a matching reply, and returns the gateway's real
/// MAC address if one arrives within the timeout.
pub fn arp_selftest() -> Option<[u8; 6]> {
    let my_mac = mac();
    // Padded to Ethernet's 60-byte minimum frame size (64 with the FCS the
    // NIC appends itself) -- a bare 42-byte ARP frame is a runt frame that
    // real switches (and, confirmed here, QEMU's SLIRP) silently drop
    // rather than reply to.
    let mut frame = [0u8; 60];
    frame[0..6].copy_from_slice(&[0xFF; 6]); // dst: broadcast
    frame[6..12].copy_from_slice(&my_mac); // src
    frame[12..14].copy_from_slice(&[0x08, 0x06]); // ethertype: ARP
    frame[14..16].copy_from_slice(&[0x00, 0x01]); // htype: Ethernet
    frame[16..18].copy_from_slice(&[0x08, 0x00]); // ptype: IPv4
    frame[18] = 6; // hlen
    frame[19] = 4; // plen
    frame[20..22].copy_from_slice(&[0x00, 0x01]); // oper: request
    frame[22..28].copy_from_slice(&my_mac); // sender MAC
    frame[28..32].copy_from_slice(&SELF_IP); // sender IP
    frame[32..38].copy_from_slice(&[0x00; 6]); // target MAC: unknown
    frame[38..42].copy_from_slice(&GATEWAY_IP); // target IP

    if !transmit(&frame) {
        return None;
    }

    let mut buf = [0u8; BUF_SIZE];
    let mut found = None;
    // 8s of *kernel* time: enough that TCG's fast-TSC miscalibration still
    // leaves a real wall-clock window for QEMU's bottom-half RX delivery
    // (the old 2s was only ~20-40ms wall and always missed), but bounded
    // so a genuinely dead gateway doesn't freeze the desktop loop that
    // calls into this synchronously. Healthy ARP replies arrive in ms.
    timer::poll_until(8_000_000, || {
        if let Some(len) = receive(&mut buf) {
            if len >= 42
                && buf[12] == 0x08
                && buf[13] == 0x06
                && buf[20] == 0x00
                && buf[21] == 0x02 // oper: reply
                && buf[28..32] == GATEWAY_IP
            {
                let mut reply_mac = [0u8; 6];
                reply_mac.copy_from_slice(&buf[22..28]);
                found = Some(reply_mac);
                return true;
            }
        }
        false
    });
    if found.is_none() {
        // Did the hardware receive *anything* at all (even noise that
        // didn't match the reply filter), or genuinely nothing? RDH only
        // ever advances via the NIC's own DMA -- nothing in this driver
        // writes it -- so this distinguishes "RX path is fundamentally
        // broken" from "nothing happened to arrive that matched."
        log_hex(b"net: arp_selftest: RDH after wait=0x", reg_read(REG_RDH));
        // Full RX-side state dump for the "reply visible in the wire
        // capture, RDH never moved" investigation: is the receiver
        // actually enabled as configured, does the ring geometry read
        // back, and did descriptor 0 get a status write despite RDH?
        log_hex(b"net: RCTL readback=0x", reg_read(REG_RCTL));
        log_hex(b"net: RDT=0x", reg_read(REG_RDT));
        log_hex(b"net: RDLEN=0x", reg_read(REG_RDLEN));
        log_hex(b"net: RDBAL=0x", reg_read(REG_RDBAL));
        unsafe {
            log_hex(b"net: ring virt=0x", RX_RING.0.as_ptr() as u32);
            log_hex(b"net: desc0 status=0x", rx_status(0) as u32);
            log_hex(b"net: desc0 len=0x", RX_RING.0[0].length as u32);
        }
    }
    found
}
