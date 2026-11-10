//! xHCI host-controller driver for PCs (x86_64): USB keyboards, mice and flash
//! drives on any PCI xHCI controller (class 0C/03/30) -- the Intel PCH one on a
//! desktop board, QEMU's `qemu-xhci`, add-in ASMedia / Renesas cards.
//!
//! **Why a copy of the Pi driver.** This file is `arch/aarch64/xhci.rs` with
//! its platform layer swapped: the controller logic (rings, enumeration, HID
//! boot protocol, endpoint recovery, hot-plug, the bulk endpoints
//! `drivers::usbmsd` drives) is the same, line for line where possible. Sharing
//! one file would have meant cfg-splitting the Pi driver (its MMIO helpers,
//! DSB barriers, DMA pool and input delivery all differ) while other work on
//! it is in flight; a copy keeps the Pi build untouched. Keep fixes in step.
//!
//! **What differs on x86.**
//! - *Finding it:* a PCI scan ([`init`]); memory decoding + bus mastering are
//!   switched on and INTx is masked (we poll, like the Pi).
//! - *MMIO:* RAM and MMIO below 4 GiB are identity-mapped by `paging`. A 64-bit
//!   BAR above 4 GiB (common on Intel 500-series boards with "Above 4G
//!   decoding") is identity-mapped 4 KiB at a time, uncached, into the kernel
//!   PML4's first slot -- shared by every process address space, so a syscall
//!   that polls the keyboard can still reach it. Beyond 512 GiB it is refused.
//! - *DMA:* frames from `mm::frame` (below 4 GiB); x86 caches are coherent with
//!   PCI bus masters, so a fence before each doorbell is all the ordering needed.
//! - *BIOS handoff:* the USB Legacy Support capability (USBLEGSUP) -- the
//!   firmware's SMM code drives the controller for "legacy USB" PS/2
//!   emulation until it is asked to let go; without the handoff it fights us.
//!   Intel PCHs that still pair the xHCI with EHCI get their ports routed to
//!   the xHCI (XUSB2PR / USB3_PSSEN).
//! - *Input:* a USB keyboard's reports become PS/2 set-1 scancodes fed into
//!   `drivers::keyboard`'s ring (the same bytes the 8042 delivers), so every
//!   consumer -- the console, the WM, the lingapp pump, Ctrl chords, layouts,
//!   F1..F4 -- works unchanged; held keys auto-repeat like a PS/2 keyboard's
//!   typematic. Mouse reports move `drivers::mouse`'s pointer.
//!
//! **Events.** One event ring per controller carries transfer completions for
//! every device, command completions and port changes. Everything funnels
//! through [`Ctl::dispatch`], so a wait for one command's completion still
//! delivers other devices' reports (nothing typed during a hot-plug is lost),
//! and port changes seen mid-operation are queued for [`poll`] rather than
//! handled re-entrantly.
//!
//! All waits are bounded and every step prints a status line.

use crate::arch::timer::{delay_us, now_ms};

#[inline]
unsafe fn read32(addr: usize) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}

#[inline]
unsafe fn write32(addr: usize, val: u32) {
    core::ptr::write_volatile(addr as *mut u32, val)
}

/// Bus address = CPU-physical address on a PC (no IOMMU in use).
const DMA_OFFSET: u64 = 0x0;

// Operational / runtime / doorbell register offsets.
const OP_USBCMD: usize = 0x00;
const OP_USBSTS: usize = 0x04;
const OP_CONFIG: usize = 0x38;
const OP_DCBAAP: usize = 0x30;
const OP_CRCR: usize = 0x18;
const OP_PORTSC0: usize = 0x400;
const OP_PORT_STRIDE: usize = 0x10;

const CMD_RUN: u32 = 1 << 0;
const CMD_HCRST: u32 = 1 << 1;
const STS_HCH: u32 = 1 << 0;
const STS_CNR: u32 = 1 << 11;
const STS_HSE: u32 = 1 << 2; // host system error: the controller stopped on a bad DMA
const STS_HCE: u32 = 1 << 12; // host controller error: internal fault, stopped
const PLS_U0: u32 = 0;
const PLS_U3: u32 = 3;

const PORTSC_CCS: u32 = 1 << 0;
const PORTSC_PED: u32 = 1 << 1; // RW1CS: writing 1 DISABLES the port -- never write it back
const PORTSC_PR: u32 = 1 << 4;
const PORTSC_PLS_MASK: u32 = 0xF << 5;
const PORTSC_PP: u32 = 1 << 9;
const PORTSC_LWS: u32 = 1 << 16; // link-state write strobe: PLS is only written with this set
const PORTSC_CSC: u32 = 1 << 17;
const PORTSC_PRC: u32 = 1 << 21;
const PORTSC_PLC: u32 = 1 << 22;
const PORTSC_WPR: u32 = 1 << 31;
// Write-1-to-clear change bits live in PORTSC; preserve non-RW1C bits when writing.
const PORTSC_RW1CS: u32 = 0x00FE_0000; // CSC,PEC,WRC,OCC,PRC,PLC,CEC (bits 17..23)
const PLS_RESUME: u32 = 15;

// TRB types.
const TRB_NORMAL: u32 = 1;
const TRB_SETUP: u32 = 2;
const TRB_DATA: u32 = 3;
const TRB_STATUS: u32 = 4;
const TRB_LINK: u32 = 6;
const TRB_ENABLE_SLOT: u32 = 9;
const TRB_DISABLE_SLOT: u32 = 10;
const TRB_ADDRESS_DEVICE: u32 = 11;
const TRB_CONFIGURE_ENDPOINT: u32 = 12;
const TRB_EVALUATE_CONTEXT: u32 = 13;
const TRB_RESET_ENDPOINT: u32 = 14;
const TRB_SET_TR_DEQUEUE: u32 = 16;
const TRB_ISOCH: u32 = 5;
const TRB_STOP_ENDPOINT: u32 = 15;
const TRB_EV_TRANSFER: u32 = 32;
const TRB_EV_CMD_COMPLETE: u32 = 33;
const TRB_EV_PORT_STATUS: u32 = 34;

const RING_TRBS: usize = 64; // TRBs per ring segment (64 * 16 = 1 KiB, fits a page)
/// The event ring fills its whole page: a streaming USB audio device posts a
/// completion every few ms, and the ring is only drained once a frame.
const EVENT_TRBS: usize = 256;

// A PC's PCH controller has 10-26 root ports and boards wire internal devices
// to some (Bluetooth, the RGB controller), all of which take a slot here.
const MAX_DEVS: usize = 16;
const MAX_HID: usize = 16; // keyboard/mouse endpoints per controller (combo dongles have two)
const MAX_CTLS: usize = 4;

#[inline]
fn dma(phys: u64) -> u64 {
    phys + DMA_OFFSET
}

/// Make every earlier store (TRBs, contexts) visible before a doorbell write.
#[inline]
fn dsb() {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

fn out(s: &[u8]) {
    crate::console_write(s);
}

unsafe fn wr64(addr: usize, val: u64) {
    write32(addr, (val & 0xFFFF_FFFF) as u32);
    write32(addr + 4, (val >> 32) as u32);
}

/// Allocate one zeroed 4 KiB frame; returns its physical address (== virtual:
/// identity map). Frames are page-aligned, satisfying every xHCI alignment, and
/// must sit below 4 GiB (identity-mapped, and reachable by 32-bit controllers).
fn alloc_page() -> Option<u64> {
    let p = crate::mm::frame::alloc_frames(0)?;
    if p + 4096 > 1 << 32 {
        crate::mm::frame::free_frames(p, 0);
        out(b"  xhci: frame above 4 GiB, not usable for DMA\n");
        return None;
    }
    for i in 0..512u64 {
        unsafe { core::ptr::write_volatile((p + i * 8) as *mut u64, 0) };
    }
    Some(p)
}

/// Return a page once the controller can no longer touch it (slot disabled).
fn free_page(p: u64) {
    crate::mm::frame::free_frames(p, 0);
}

fn trb_write(ring: u64, idx: usize, param: u64, status: u32, control: u32) {
    let a = ring + (idx * 16) as u64;
    unsafe {
        core::ptr::write_volatile(a as *mut u64, param);
        core::ptr::write_volatile((a + 8) as *mut u32, status);
        core::ptr::write_volatile((a + 12) as *mut u32, control);
    }
}

// ── Controller ───────────────────────────────────────────────────────────────

struct Xhci {
    cap: usize,
    op: usize,
    run: usize,
    db: usize,
    ctx_size: usize, // 32 or 64
    max_ports: u32,
    dcbaa: u64,
    cmd_ring: u64,
    cmd_enq: usize,
    cmd_cycle: u32,
    event_ring: u64,
    event_deq: usize,
    event_cycle: u32,
    erst: u64,
}

impl Xhci {
    fn op_rd(&self, off: usize) -> u32 {
        unsafe { read32(self.op + off) }
    }
    fn op_wr(&self, off: usize, v: u32) {
        unsafe { write32(self.op + off, v) };
    }
    fn portsc(&self, p: u32) -> usize {
        self.op + OP_PORTSC0 + (p as usize - 1) * OP_PORT_STRIDE
    }

    /// Reset the controller and build the command ring, event ring and DCBAA,
    /// then set it running. Returns None on any failure (after printing why).
    fn init(cap: usize) -> Option<Xhci> {
        let caplength = (unsafe { read32(cap) } & 0xFF) as usize;
        let hcsparams1 = unsafe { read32(cap + 0x04) };
        let hccparams1 = unsafe { read32(cap + 0x10) };
        let dboff = (unsafe { read32(cap + 0x14) } & !0x3) as usize;
        let rtsoff = (unsafe { read32(cap + 0x18) } & !0x1F) as usize;
        let ctx_size = if hccparams1 & (1 << 2) != 0 { 64 } else { 32 };

        let mut x = Xhci {
            cap,
            op: cap + caplength,
            run: cap + rtsoff,
            db: cap + dboff,
            ctx_size,
            // port_pending is a u32 bitmask of ports 1..=31
            max_ports: ((hcsparams1 >> 24) & 0xFF).min(31),
            dcbaa: 0,
            cmd_ring: 0,
            cmd_enq: 0,
            cmd_cycle: 1,
            event_ring: 0,
            event_deq: 0,
            event_cycle: 1,
            erst: 0,
        };

        // Halt, then reset.
        let cmd = x.op_rd(OP_USBCMD);
        if cmd & CMD_RUN != 0 {
            x.op_wr(OP_USBCMD, cmd & !CMD_RUN);
            wait_set(x.op + OP_USBSTS, STS_HCH, 20_000);
        }
        x.op_wr(OP_USBCMD, CMD_HCRST);
        if !wait_clear(x.op + OP_USBCMD, CMD_HCRST, 20_000)
            || !wait_clear(x.op + OP_USBSTS, STS_CNR, 20_000)
        {
            out(b"  xhci: controller reset timeout\n");
            return None;
        }

        // DCBAA, command ring, event ring + ERST.
        x.dcbaa = alloc_page()?;
        x.cmd_ring = alloc_page()?;
        x.event_ring = alloc_page()?;
        x.erst = alloc_page()?;

        // Scratchpad buffers. If HCSPARAMS2 asks for any, DCBAA[0] MUST point to
        // an array of page pointers the controller uses for internal state —
        // without it the controller silently never completes a command.
        // HCSPARAMS2: Max Scratchpad Bufs = (bits[31:27] << 5) | bits[25:21].
        let hcs2 = unsafe { read32(cap + 0x08) };
        let max_sp = (((hcs2 >> 27) & 0x1F) << 5) | ((hcs2 >> 21) & 0x1F);
        hex(b"  max_scratchpad_bufs=", max_sp as u64, 2);
        if max_sp > 0 {
            let sp_array = alloc_page()?;
            for i in 0..max_sp as u64 {
                let buf = alloc_page()?;
                unsafe { core::ptr::write_volatile((sp_array + i * 8) as *mut u64, dma(buf)) };
            }
            unsafe { core::ptr::write_volatile(x.dcbaa as *mut u64, dma(sp_array)) };
        }

        let max_slots = hcsparams1 & 0xFF;
        x.op_wr(OP_CONFIG, max_slots); // MaxSlotsEn

        unsafe { wr64(x.op + OP_DCBAAP, dma(x.dcbaa)) };

        // Command ring: dequeue = base, cycle state 1.
        unsafe { wr64(x.op + OP_CRCR, dma(x.cmd_ring) | 1) };

        // Event ring: one segment described by ERST[0].
        unsafe {
            core::ptr::write_volatile(x.erst as *mut u64, dma(x.event_ring));
            core::ptr::write_volatile((x.erst + 8) as *mut u32, EVENT_TRBS as u32);
            core::ptr::write_volatile((x.erst + 12) as *mut u32, 0);
        }
        // Interrupter 0 runtime regs: IMAN@+0x20, ERSTSZ@+0x28, ERSTBA@+0x30,
        // ERDP@+0x38. Spec order: ERSTSZ, ERDP, then ERSTBA LAST (writing ERSTBA
        // arms the interrupter). Enable the interrupter (IMAN.IE) — some
        // controllers won't advance/post events until it's enabled.
        unsafe {
            write32(x.run + 0x28, 1);
            wr64(x.run + 0x38, dma(x.event_ring));
            wr64(x.run + 0x30, dma(x.erst));
            write32(x.run + 0x20, 0x2); // IMAN.IE
        }

        dsb();
        // Run (RUN | INTE: enable the controller's interrupter system).
        x.op_wr(OP_USBCMD, CMD_RUN | (1 << 2));
        if !wait_clear(x.op + OP_USBSTS, STS_HCH, 20_000) {
            out(b"  xhci: controller did not start (still halted)\n");
            return None;
        }
        out(b"  xhci: running (rings up, DCBAA set)\n");
        Some(x)
    }

    /// Enqueue a command TRB and ring the command doorbell. Returns the TRB's
    /// bus address — its completion event carries it, which is how a waiter
    /// recognises its own completion. The ring wraps with a Link TRB (toggle
    /// cycle), so unlimited hot-plugs don't run off the end.
    fn ring_command(&mut self, param: u64, control: u32) -> u64 {
        let addr = self.cmd_ring + (self.cmd_enq * 16) as u64;
        trb_write(self.cmd_ring, self.cmd_enq, param, 0, control | self.cmd_cycle);
        self.cmd_enq += 1;
        if self.cmd_enq == RING_TRBS - 1 {
            trb_write(self.cmd_ring, self.cmd_enq, dma(self.cmd_ring), 0, (TRB_LINK << 10) | (1 << 1) | self.cmd_cycle);
            self.cmd_enq = 0;
            self.cmd_cycle ^= 1;
        }
        dsb();
        unsafe { write32(self.db, 0) }; // doorbell 0 = command ring
        dma(addr)
    }

    /// Non-blocking: dequeue one event if the controller has posted one.
    fn poll_event(&mut self) -> Option<(u64, u32, u32)> {
        let a = self.event_ring + (self.event_deq * 16) as u64;
        let control = unsafe { core::ptr::read_volatile((a + 12) as *const u32) };
        if (control & 1) != self.event_cycle {
            return None;
        }
        let param = unsafe { core::ptr::read_volatile(a as *const u64) };
        let status = unsafe { core::ptr::read_volatile((a + 8) as *const u32) };
        self.event_deq += 1;
        if self.event_deq >= EVENT_TRBS {
            self.event_deq = 0;
            self.event_cycle ^= 1;
        }
        let erdp = dma(self.event_ring + (self.event_deq * 16) as u64) | (1 << 3);
        unsafe { wr64(self.run + 0x38, erdp) };
        Some((param, status, control))
    }

    /// Dump controller state for diagnosis: USBSTS (decoded), CRCR, and the raw
    /// first event-ring TRB — tells us whether the controller faulted on DMA
    /// (HSE), wrote an event we missed (EINT), or did nothing at all.
    fn dump_state(&self, tag: &[u8]) {
        let sts = self.op_rd(OP_USBSTS);
        out(tag);
        hex(b" USBSTS=", sts as u64, 8);
        if sts & (1 << 2) != 0 {
            out(b"    HSE set: host system error -> bad DMA address\n");
        }
        if sts & (1 << 3) != 0 {
            out(b"    EINT set: an event WAS written (poll/cycle bug)\n");
        }
        if sts & STS_HCH != 0 {
            out(b"    HCH set: controller halted\n");
        }
        let crcr = unsafe { read32(self.op + OP_CRCR) };
        hex(b"    CRCR.lo=", crcr as u64, 8);
        hex(b"    USBCMD=", self.op_rd(OP_USBCMD) as u64, 8);
        hex(b"    IMAN=", unsafe { read32(self.run + 0x20) } as u64, 8);
        hex(b"    ERDP.lo=", unsafe { read32(self.run + 0x38) } as u64, 8);
    }
}

fn wait_clear(addr: usize, mask: u32, tries: u32) -> bool {
    for _ in 0..tries {
        if unsafe { read32(addr) } & mask == 0 {
            return true;
        }
        delay_us(50);
    }
    false
}

fn wait_set(addr: usize, mask: u32, tries: u32) -> bool {
    for _ in 0..tries {
        if unsafe { read32(addr) } & mask != 0 {
            return true;
        }
        delay_us(50);
    }
    false
}

fn hex(label: &[u8], val: u64, nibbles: usize) {
    out(label);
    out(b"0x");
    let mut buf = [0u8; 16];
    let n = nibbles.min(16);
    for i in 0..n {
        let nib = (val >> ((n - 1 - i) * 4)) & 0xF;
        buf[i] = if nib < 10 { b'0' + nib as u8 } else { b'a' + (nib - 10) as u8 };
    }
    out(&buf[..n]);
    out(b"\n");
}

fn report_cc(cc: u32) {
    match cc {
        1 => out(b"success\n"),
        4 => out(b"USB transaction error\n"),
        5 => out(b"TRB error\n"),
        6 => out(b"stall error\n"),
        _ => hex(b"completion code=", cc as u64, 2),
    }
}

fn dec(label: &[u8], v: u32) {
    out(label);
    let mut b = [0u8; 10];
    let mut n = 0;
    let mut v = v;
    if v == 0 {
        b[0] = b'0';
        n = 1;
    }
    while v > 0 && n < b.len() {
        b[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    b[..n].reverse();
    out(&b[..n]);
}

/// Reset root-hub port `p` (1-based) if a device is connected; returns its
/// PORTSC once the port is enabled. (The port isn't enabled yet, so writing
/// PORTSC back with PED clear is harmless here.)
fn reset_port(x: &Xhci, p: u32) -> Option<u32> {
    let off = x.portsc(p);
    let mut psc = unsafe { read32(off) };
    if psc & PORTSC_PP == 0 {
        unsafe { write32(off, (psc & !PORTSC_RW1CS) | PORTSC_PP) };
        delay_us(20_000);
        psc = unsafe { read32(off) };
    }
    if psc & PORTSC_CCS == 0 {
        return None;
    }
    // Reset the port: write PR, keep PP, clear RW1C change bits.
    let base = (psc & !PORTSC_RW1CS) | PORTSC_PP;
    unsafe { write32(off, base | PORTSC_PR) };
    if !wait_clear(off, PORTSC_PR, 20_000) {
        out(b"  port reset timeout\n");
        return None;
    }
    delay_us(20_000);
    let psc2 = unsafe { read32(off) };
    dec(b"  port ", p);
    hex(b" after reset PORTSC=", psc2 as u64, 8);
    if psc2 & PORTSC_PED == 0 {
        out(b"  port did not enable after reset\n");
        return None;
    }
    Some(psc2)
}

// ── Devices ──────────────────────────────────────────────────────────────────

/// One addressed USB device: its slot, port and the pages the controller
/// holds for it (freed again when it is unplugged).
#[derive(Clone, Copy)]
struct Dev {
    slot: u8,
    port: u32,
    speed: u32,
    ep0_ring: u64,
    ep0_enq: usize,
    ep0_cycle: u32,
    input_ctx: u64,
    dev_ctx: u64,
    buf: u64,
}

#[derive(Clone, Copy, PartialEq)]
enum HidKind {
    Kbd,
    Mouse,
    /// A gamepad / joystick: decoded with `pad` (drivers::hidpad).
    Pad,
}

/// One boot-protocol HID interrupt-IN endpoint being polled.
#[derive(Clone, Copy)]
struct Hid {
    kind: HidKind,
    slot: u8,
    port: u32,
    dci: u32,
    ep_addr: u32,
    ring: u64,
    enq: usize,
    cycle: u32,
    rbuf: u64,
    mps: u32,
    prev: [u8; 8],
    /// Where a gamepad's sticks and buttons sit in its report.
    pad: crate::drivers::hidpad::Layout,
    /// Set by a transfer error: the controller halted the endpoint and it
    /// must be reset (done by [`poll`], outside the event loop).
    halted: bool,
    last_cc: u32,
    fails: u8,
    /// Report requests queued / completed: HID_SLOTS stay in flight, each
    /// with its own 64-byte slot of `rbuf`, so the controller keeps polling
    /// the device at its interval and no state change is lost between two
    /// slow frames (a lost "Shift held" report turns a capital lowercase).
    fill: u32,
    done: u32,
}

/// One HID interrupt-IN endpoint found in a configuration descriptor.
#[derive(Clone, Copy)]
struct HidEp {
    iface: u32,
    /// 1 keyboard, 2 mouse (boot protocol), 0 another HID device (a gamepad
    /// if its report descriptor says so), 0xFF a wired Xbox 360 controller.
    proto: u32,
    addr: u32,
    mps: u32,
    interval: u32,
    /// The HID report descriptor's length (proto 0).
    report_len: u32,
}

/// One bulk endpoint of a mass-storage device (its ring state lives here).
#[derive(Clone, Copy)]
struct BulkEp {
    dci: u32,
    ep_addr: u32,
    ring: u64,
    enq: usize,
    cycle: u32,
}

/// A USB mass-storage device (one per controller): its two bulk endpoints and
/// the DMA page the class driver's transfers go through.
#[derive(Clone, Copy)]
struct Msd {
    slot: u8,
    di: usize,
    iface: u32,
    ep_in: BulkEp,
    ep_out: BulkEp,
    data: u64,
}

struct Ctl {
    x: Xhci,
    name: &'static [u8],
    devs: [Option<Dev>; MAX_DEVS],
    hids: [Option<Hid>; MAX_HID],
    /// Ports whose status-change events arrived; serviced by [`poll`].
    port_pending: u32,
    last_scan_ms: u64,
    /// Per port: no enumeration attempt from the periodic scan before this
    /// time (back-off after a failure or a watchdog re-enumeration).
    port_retry: [u64; 32],
    msd: Option<Msd>,
    /// A USB Audio Class device (one per controller), see drivers::usbaudio.
    audio: Option<AudioDev>,
}

static mut CTLS: [Option<Ctl>; MAX_CTLS] = [const { None }; MAX_CTLS];
/// A drive arrived / left: handed to drivers::usbmsd by poll(), outside the
/// controller borrow its transfers need.
static mut MSD_ATTACH: bool = false;
static mut MSD_DETACH: bool = false;
/// The same hand-off for a USB audio device (drivers::usbaudio).
static mut AUDIO_ATTACH: bool = false;
static mut AUDIO_DETACH: bool = false;

/// What a waiter is looking for in the event stream.
#[derive(Clone, Copy)]
enum Want {
    /// The completion of the command TRB at this bus address.
    Cmd(u64),
    /// A transfer event for (slot, endpoint DCI) — a control transfer's end.
    Xfer(u8, u32),
}

impl Ctl {
    /// Handle every event already posted; returns when the ring is empty.
    fn drain(&mut self) {
        while let Some(ev) = self.x.poll_event() {
            self.dispatch(ev, None);
        }
    }

    /// Handle events until the `want`ed one arrives (returned), or `tries`
    /// idle polls of 50µs pass. Other devices' reports are delivered meanwhile.
    fn wait(&mut self, want: Want, tries: u32) -> Option<(u64, u32, u32)> {
        let mut idle = 0u32;
        while idle < tries {
            match self.x.poll_event() {
                Some(ev) => {
                    if let Some(hit) = self.dispatch(ev, Some(want)) {
                        return Some(hit);
                    }
                },
                None => {
                    delay_us(50);
                    idle += 1;
                },
            }
        }
        None
    }

    /// Route one event; `Some` when it is the one a waiter asked for.
    fn dispatch(&mut self, ev: (u64, u32, u32), want: Option<Want>) -> Option<(u64, u32, u32)> {
        let (param, status, control) = ev;
        match (control >> 10) & 0x3F {
            TRB_EV_CMD_COMPLETE => {
                if let Some(Want::Cmd(addr)) = want {
                    if param == addr {
                        return Some(ev);
                    }
                }
                hex(b"  xhci: stray command completion, TRB ", param, 16);
                None
            },
            TRB_EV_TRANSFER => {
                let slot = ((control >> 24) & 0xFF) as u8;
                let dci = (control >> 16) & 0x1F;
                if let Some(Want::Xfer(s, d)) = want {
                    if s == slot && d == dci {
                        return Some(ev);
                    }
                }
                if let Some(a) = self.audio.as_mut().filter(|a| a.slot == slot) {
                    if a.on_event(dci, param, status) {
                        return None;
                    }
                }
                self.on_transfer(slot, dci, status);
                None
            },
            TRB_EV_PORT_STATUS => {
                let port = ((param >> 24) & 0xFF) as u32;
                if (1..=31).contains(&port) {
                    self.port_pending |= 1 << port;
                }
                None
            },
            _ => None,
        }
    }

    /// A transfer completed on one of our polled endpoints: decode the report
    /// and re-arm, or note that the controller halted the endpoint.
    fn on_transfer(&mut self, slot: u8, dci: u32, status: u32) {
        let Some(i) = self.hids.iter().position(|h| h.is_some_and(|h| h.slot == slot && h.dci == dci)) else {
            return;
        };
        // Keys the OTHER keyboard interfaces hold: gaming keyboards expose
        // several, and a key reported on two of them must type once.
        let mut others = [0u8; 8];
        let mut n_oth = 0usize;
        let mut other_mods = 0u8;
        for (j, o) in self.hids.iter().enumerate() {
            let Some(o) = o else { continue };
            if j == i || o.kind != HidKind::Kbd {
                continue;
            }
            other_mods |= o.prev[0];
            for &u in &o.prev[2..8] {
                if u >= 4 && n_oth < others.len() && !others[..n_oth].contains(&u) {
                    others[n_oth] = u;
                    n_oth += 1;
                }
            }
        }
        let x = &self.x;
        let h = self.hids[i].as_mut().unwrap();
        let cc = (status >> 24) & 0xFF;
        if cc == 1 || cc == 13 {
            // completions come back in the order the requests were queued
            let buf = h.rbuf + (h.done % HID_SLOTS) as u64 * HID_SLOT_BYTES;
            h.done = h.done.wrapping_add(1);
            let mut cur = [0u8; 8];
            let n = (h.mps as usize).min(8);
            for (k, b) in cur.iter_mut().enumerate().take(n) {
                *b = unsafe { core::ptr::read_volatile((buf + k as u64) as *const u8) };
            }
            // Once per kind: proof on the console that reports really arrive
            // over USB (and not through the firmware's PS/2 emulation).
            let bit = 1u8 << (h.kind as u8);
            if unsafe { FIRST_REPORT } & bit == 0 {
                unsafe { FIRST_REPORT |= bit };
                out(self.name);
                out(match h.kind {
                    HidKind::Kbd => b": first USB keyboard report\n",
                    HidKind::Mouse => b": first USB mouse report\n",
                    HidKind::Pad => b": first USB gamepad report\n",
                });
            }
            match h.kind {
                HidKind::Kbd => {
                    decode_report(&h.prev, &cur, &others[..n_oth], other_mods);
                    // HELD (auto-repeat) = every interface's keys together
                    let mut all = cur;
                    all[0] |= other_mods;
                    for &u in &others[..n_oth] {
                        if !all[2..8].contains(&u) {
                            if let Some(slot) = all[2..8].iter().position(|&x| x < 4) {
                                all[2 + slot] = u;
                            }
                        }
                    }
                    unsafe { HELD = all };
                },
                HidKind::Mouse => mouse_report(&cur, n),
                HidKind::Pad => {
                    // The whole report (up to 64 bytes); the TRB's residual says how much came.
                    let got = (h.mps as usize).saturating_sub((status & 0xFF_FFFF) as usize).min(64);
                    let mut rep = [0u8; 64];
                    for (k, b) in rep.iter_mut().enumerate().take(got) {
                        *b = unsafe { core::ptr::read_volatile((buf + k as u64) as *const u8) };
                    }
                    let st = unsafe { &mut *(&raw mut PAD) };
                    crate::drivers::hidpad::decode(&h.pad, &rep[..got], st);
                },
            }
            h.prev = cur;
            h.fails = 0;
            hid_queue(x, h);
        } else {
            // Any other completion halts the endpoint (USB transaction error
            // when a device drops the link, stall, babble...). Re-queuing on a
            // halted endpoint would be ignored forever — exactly the "lights
            // come back but nothing types" symptom — so recover it instead.
            h.halted = true;
            h.last_cc = cc;
        }
    }

    /// Bring a halted endpoint back: Reset Endpoint, point its dequeue at our
    /// next TRB, clear a stall on the device, and re-arm.
    fn recover(&mut self, i: usize) {
        let Some(mut h) = self.hids[i] else { return };
        h.fails += 1;
        out(self.name);
        out(match h.kind {
            HidKind::Kbd => b": keyboard endpoint halted (",
            HidKind::Mouse => b": mouse endpoint halted (",
            HidKind::Pad => b": gamepad endpoint halted (",
        });
        report_cc(h.last_cc);
        if h.fails > 6 {
            out(b"  resetting its port and enumerating the device again\n");
            let port = h.port;
            self.remove_port(port);
            self.port_retry[port as usize & 31] = now_ms() + 2000;
            return;
        }
        let sl = (h.slot as u32) << 24;
        let a = self.x.ring_command(0, (TRB_RESET_ENDPOINT << 10) | (h.dci << 16) | sl);
        match self.wait(Want::Cmd(a), 20_000) {
            Some((_, s, _)) if matches!((s >> 24) & 0xFF, 1 | 19) => {}, // 19: wasn't halted after all
            Some((_, s, _)) => {
                out(b"  reset endpoint: ");
                report_cc((s >> 24) & 0xFF);
            },
            None => out(b"  reset endpoint: no completion\n"),
        }
        let deq = dma(h.ring + (h.enq * 16) as u64) | h.cycle as u64;
        let a = self.x.ring_command(deq, (TRB_SET_TR_DEQUEUE << 10) | (h.dci << 16) | sl);
        match self.wait(Want::Cmd(a), 20_000) {
            Some((_, s, _)) if (s >> 24) & 0xFF == 1 => {},
            Some((_, s, _)) => {
                out(b"  set dequeue pointer: ");
                report_cc((s >> 24) & 0xFF);
            },
            None => out(b"  set dequeue pointer: no completion\n"),
        }
        if h.last_cc == 6 {
            // CLEAR_FEATURE(ENDPOINT_HALT) so the device's data toggle resets too.
            if let Some(di) = self.devs.iter().position(|d| d.is_some_and(|d| d.slot == h.slot)) {
                let _ = self.control(di, 0x02, 1, 0, h.ep_addr as u16, 0, 0);
            }
        }
        h.halted = false;
        // the dequeue pointer skipped every request still queued: start over
        h.done = h.fill;
        hid_arm_all(&self.x, &mut h);
        self.hids[i] = Some(h);
        out(b"  endpoint re-armed\n");
    }

    /// A control transfer on EP0 (Setup/Data/Status) of device `di`. For an IN
    /// request the data lands in `buf_dma`. Returns bytes transferred, or None
    /// on failure.
    fn control(&mut self, di: usize, bm_rt: u8, b_req: u8, w_val: u16, w_idx: u16, buf_dma: u64, len: u16) -> Option<u32> {
        let mut d = self.devs[di]?;
        let is_in = bm_rt & 0x80 != 0;
        let setup = (bm_rt as u64)
            | ((b_req as u64) << 8)
            | ((w_val as u64) << 16)
            | ((w_idx as u64) << 32)
            | ((len as u64) << 48);
        let trt = if len == 0 {
            0u32
        } else if is_in {
            3
        } else {
            2
        };
        ep0_push(&mut d, setup, 8, (TRB_SETUP << 10) | (trt << 16) | (1 << 6));
        if len > 0 {
            let dir = if is_in { 1u32 } else { 0 };
            ep0_push(&mut d, buf_dma, len as u32, (TRB_DATA << 10) | (dir << 16));
        }
        let status_dir = if len > 0 && is_in { 0u32 } else { 1 };
        ep0_push(&mut d, 0, 0, (TRB_STATUS << 10) | (status_dir << 16) | (1 << 5));
        self.devs[di] = Some(d);
        dsb();
        unsafe { write32(self.x.db + d.slot as usize * 4, 1) }; // EP0 doorbell, DCI 1
        let (_p, status, _ctl) = self.wait(Want::Xfer(d.slot, 1), 40_000)?;
        let code = (status >> 24) & 0xFF;
        if code != 1 && code != 13 {
            out(b"    control xfer ");
            report_cc(code);
            return None;
        }
        Some(len as u32 - (status & 0xFF_FFFF))
    }

    /// Update EP0's Max Packet Size in the device context via an Evaluate
    /// Context command. Needed because Address Device sets a provisional MPS of 8
    /// (safe for the first 8-byte descriptor read); the real value comes from
    /// bMaxPacketSize0 and must be programmed before a larger control transfer,
    /// or the device babbles. The input context is reused (only the EP0 add-flag set).
    fn set_ep0_mps(&mut self, di: usize, mps: u32) -> bool {
        let Some(d) = self.devs[di] else { return false };
        let cs = self.x.ctx_size as u64;
        unsafe {
            core::ptr::write_volatile(d.input_ctx as *mut u32, 0); // drop flags
            core::ptr::write_volatile((d.input_ctx + 0x04) as *mut u32, 0b10); // add A1 = EP0 only
        }
        let ep0 = d.input_ctx + 2 * cs;
        unsafe {
            core::ptr::write_volatile((ep0 + 4) as *mut u32, (mps << 16) | (4 << 3) | (3 << 1));
            core::ptr::write_volatile((ep0 + 8) as *mut u64, dma(d.ep0_ring) | 1);
            core::ptr::write_volatile((ep0 + 16) as *mut u32, mps.min(8));
        }
        dsb();
        let a = self.x.ring_command(dma(d.input_ctx), (TRB_EVALUATE_CONTEXT << 10) | ((d.slot as u32) << 24));
        matches!(self.wait(Want::Cmd(a), 40_000), Some((_, s, _)) if (s >> 24) & 0xFF == 1)
    }

    /// Forget device `di`: drop its endpoints, disable its slot, free its pages.
    fn remove_dev(&mut self, di: usize) {
        let Some(d) = self.devs[di] else { return };
        if let Some(m) = self.msd {
            if m.slot == d.slot {
                free_page(m.ep_in.ring);
                free_page(m.ep_out.ring);
                free_page(m.data);
                self.msd = None;
                unsafe { MSD_DETACH = true };
            }
        }
        if self.audio.as_ref().is_some_and(|a| a.slot == d.slot) {
            // the slot is disabled below; the rings are freed after that
            let a = self.audio.take().unwrap();
            let cmd = self.x.ring_command(0, (TRB_DISABLE_SLOT << 10) | ((d.slot as u32) << 24));
            let _ = self.wait(Want::Cmd(cmd), 20_000);
            a.free();
            unsafe {
                core::ptr::write_volatile((self.x.dcbaa + d.slot as u64 * 8) as *mut u64, 0);
                AUDIO_DETACH = true;
            }
            free_page(d.input_ctx);
            free_page(d.dev_ctx);
            free_page(d.ep0_ring);
            free_page(d.buf);
            self.devs[di] = None;
            out(self.name);
            dec(b": USB audio device on port ", d.port);
            out(b" removed\n");
            return;
        }
        for h in self.hids.iter_mut() {
            if let Some(hh) = *h {
                if hh.slot == d.slot {
                    if hh.kind == HidKind::Kbd {
                        unsafe { HELD = [0; 8] };
                    }
                    if hh.kind == HidKind::Pad {
                        unsafe { PAD = crate::drivers::hidpad::PadState::default() };
                    }
                    free_page(hh.ring);
                    free_page(hh.rbuf);
                    *h = None;
                }
            }
        }
        let a = self.x.ring_command(0, (TRB_DISABLE_SLOT << 10) | ((d.slot as u32) << 24));
        let _ = self.wait(Want::Cmd(a), 20_000);
        unsafe { core::ptr::write_volatile((self.x.dcbaa + d.slot as u64 * 8) as *mut u64, 0) };
        free_page(d.input_ctx);
        free_page(d.dev_ctx);
        free_page(d.ep0_ring);
        free_page(d.buf);
        self.devs[di] = None;
        out(self.name);
        dec(b": device on port ", d.port);
        out(b" removed\n");
    }

    fn remove_port(&mut self, port: u32) {
        for di in 0..MAX_DEVS {
            if self.devs[di].is_some_and(|d| d.port == port) {
                self.remove_dev(di);
            }
        }
    }

    /// Clear the change bits of every port (after boot enumeration has dealt
    /// with the connects/resets they describe), so hot-plug starts clean.
    fn clear_port_changes(&mut self) {
        for p in 1..=self.x.max_ports {
            let off = self.x.portsc(p);
            let psc = unsafe { read32(off) };
            unsafe { write32(off, portsc_base(psc) | PORTSC_RW1CS) };
        }
        self.drain();
        self.port_pending = 0;
    }

    /// Act on the ports that changed (or on all of them, for the periodic
    /// scan): finish remote-wakeup resumes, drop unplugged devices, set up
    /// newly plugged ones.
    fn service_ports(&mut self, scan: bool) {
        let pending = core::mem::take(&mut self.port_pending);
        for p in 1..=self.x.max_ports {
            if !scan && pending & (1 << p) == 0 {
                continue;
            }
            let off = self.x.portsc(p);
            let psc = unsafe { read32(off) };
            let base = portsc_base(psc);
            if psc & PORTSC_PLC != 0 {
                if (psc & PORTSC_PLS_MASK) >> 5 == PLS_RESUME {
                    // Remote wakeup (a key pressed on a suspended device): the
                    // host must finish the resume by moving the port to U0.
                    delay_us(20_000);
                    unsafe { write32(off, (base & !PORTSC_PLS_MASK) | PORTSC_LWS | PORTSC_PLC) };
                    out(self.name);
                    dec(b": port ", p);
                    out(b" resumed from suspend\n");
                } else {
                    unsafe { write32(off, base | PORTSC_PLC) };
                }
            }
            if psc & PORTSC_PRC != 0 {
                unsafe { write32(off, base | PORTSC_PRC) };
            }
            let connected = psc & PORTSC_CCS != 0;
            let changed = psc & PORTSC_CSC != 0;
            if changed {
                unsafe { write32(off, base | PORTSC_CSC) };
            }
            let have = self.devs.iter().any(|d| d.is_some_and(|d| d.port == p));
            if have && (!connected || changed) {
                // Gone, or went away and came back (re-enumerate from scratch).
                self.remove_port(p);
            }
            let now = now_ms();
            let backoff = !changed && now < self.port_retry[p as usize & 31];
            if connected && (!have || changed) && !backoff {
                delay_us(100_000); // debounce
                if unsafe { read32(off) } & PORTSC_CCS != 0 {
                    out(self.name);
                    dec(b": device plugged into port ", p);
                    out(b"\n");
                    if let Some(psc2) = reset_port(&self.x, p) {
                        enumerate_device(self, p, psc2);
                    }
                    // The reset just raised PRC (and maybe CSC): clear them.
                    let psc3 = unsafe { read32(off) };
                    unsafe { write32(off, portsc_base(psc3) | PORTSC_PRC | PORTSC_CSC) };
                    self.drain();
                    self.port_pending &= !(1 << p);
                    if !self.devs.iter().any(|d| d.is_some_and(|d| d.port == p)) {
                        // Did not come up: try again in a while, not every second.
                        self.port_retry[p as usize & 31] = now + 5000;
                    }
                }
            }
        }
    }
}

impl Ctl {
    /// Once a second: catch what no event told us about. A controller that
    /// halted or hit a host-system error gets re-initialised (returns true);
    /// an endpoint the controller shows halted / in error is recovered, a
    /// stopped one gets its doorbell rung, a disabled one has its port
    /// enumerated again; a port left suspended is resumed; a port that went
    /// disabled with a device still attached is enumerated again. Any of
    /// those used to leave a keyboard or mouse silent until a reboot.
    fn watchdog(&mut self, now: u64) -> bool {
        let sts = self.x.op_rd(OP_USBSTS);
        if sts & (STS_HCH | STS_HSE | STS_HCE) != 0 {
            out(self.name);
            hex(b": watchdog: controller stopped, USBSTS=", sts as u64, 8);
            return true;
        }
        for i in 0..MAX_HID {
            let Some(h) = self.hids[i] else { continue };
            if h.halted {
                continue;
            }
            let Some(d) = self.devs.iter().flatten().find(|d| d.slot == h.slot).copied() else { continue };
            let epc = d.dev_ctx + h.dci as u64 * self.x.ctx_size as u64;
            let state = unsafe { read32(epc as usize) } & 7;
            match state {
                2 | 4 => {
                    out(self.name);
                    out(b": watchdog: endpoint halted -> recovering\n");
                    let hh = self.hids[i].as_mut().unwrap();
                    hh.halted = true;
                    hh.last_cc = 0;
                },
                3 => {
                    out(self.name);
                    out(b": watchdog: endpoint stopped -> ringing its doorbell\n");
                    dsb();
                    unsafe { write32(self.x.db + h.slot as usize * 4, h.dci) };
                },
                0 => {
                    out(self.name);
                    dec(b": watchdog: endpoint disabled -> enumerating port ", h.port);
                    out(b" again\n");
                    self.remove_port(h.port);
                    self.port_retry[h.port as usize & 31] = now + 1000;
                },
                _ => {},
            }
        }
        for p in 1..=self.x.max_ports {
            let off = self.x.portsc(p);
            let psc = unsafe { read32(off) };
            if psc & PORTSC_CCS == 0 {
                continue;
            }
            let base = portsc_base(psc);
            let pls = (psc & PORTSC_PLS_MASK) >> 5;
            let have = self.devs.iter().any(|d| d.is_some_and(|d| d.port == p));
            if pls == PLS_U3 {
                out(self.name);
                dec(b": watchdog: port ", p);
                out(b" is suspended -> resuming\n");
                unsafe { write32(off, (base & !PORTSC_PLS_MASK) | (PLS_RESUME << 5) | PORTSC_LWS) };
                delay_us(20_000);
                unsafe { write32(off, (base & !PORTSC_PLS_MASK) | (PLS_U0 << 5) | PORTSC_LWS) };
                delay_us(10_000);
            } else if have && psc & PORTSC_PED == 0 && pls != PLS_RESUME {
                out(self.name);
                dec(b": watchdog: port ", p);
                out(b" disabled with a device attached -> enumerating it again\n");
                self.remove_port(p);
                self.port_retry[p as usize & 31] = now + 1000;
            }
        }
        false
    }
}

/// Throw a stopped controller away and bring it up again from scratch (its
/// devices get enumerated anew) -- the only way back from a halted controller.
fn reinit_controller(idx: usize) {
    let ctls = unsafe { &mut *(&raw mut CTLS) };
    let Some(ctl) = ctls[idx].take() else { return };
    let (cap, name) = (ctl.x.cap, ctl.name);
    if ctl.msd.is_some() {
        unsafe { MSD_DETACH = true };
    }
    unsafe { HELD = [0; 8] };
    out(name);
    out(b": re-initialising the controller\n");
    // Init resets the controller first, so the old pages are safe to free after.
    enumerate(cap, name);
    for d in ctl.devs.iter().flatten() {
        for p in [d.input_ctx, d.dev_ctx, d.ep0_ring, d.buf] {
            free_page(p);
        }
    }
    for h in ctl.hids.iter().flatten() {
        free_page(h.ring);
        free_page(h.rbuf);
    }
    if let Some(m) = ctl.msd {
        free_page(m.ep_in.ring);
        free_page(m.ep_out.ring);
        free_page(m.data);
    }
    if let Some(a) = ctl.audio {
        a.free();
        unsafe { AUDIO_DETACH = true };
    }
    for p in [ctl.x.dcbaa, ctl.x.cmd_ring, ctl.x.event_ring, ctl.x.erst] {
        free_page(p);
    }
}

/// PORTSC with everything that would act on a write masked out: the RW1C
/// change bits, PED (writing 1 disables the port), PR/WPR (resets), LWS.
fn portsc_base(psc: u32) -> u32 {
    (psc & !(PORTSC_RW1CS | PORTSC_PED | PORTSC_PR | PORTSC_WPR | PORTSC_LWS)) | PORTSC_PP
}

/// Append a TRB to a device's EP0 ring (wrapping with a Link TRB).
fn ep0_push(d: &mut Dev, param: u64, status: u32, control: u32) {
    trb_write(d.ep0_ring, d.ep0_enq, param, status, control | d.ep0_cycle);
    d.ep0_enq += 1;
    if d.ep0_enq == RING_TRBS - 1 {
        trb_write(d.ep0_ring, d.ep0_enq, dma(d.ep0_ring), 0, (TRB_LINK << 10) | (1 << 1) | d.ep0_cycle);
        d.ep0_enq = 0;
        d.ep0_cycle ^= 1;
    }
}

/// Bring up controller `cap_base` (if not already running), enumerate what's
/// plugged in now, and leave it watching its ports for hot-plug.
pub fn enumerate(cap_base: usize, name: &'static [u8]) {
    let ctls = unsafe { &mut *(&raw mut CTLS) };
    if let Some(c) = ctls.iter().flatten().find(|c| c.x.cap == cap_base) {
        // Already running (the `usb` console command after boot): just report.
        out(name);
        out(b": running; ");
        dec(b"devices=", c.devs.iter().flatten().count() as u32);
        dec(b" keyboard/mouse endpoints=", c.hids.iter().flatten().count() as u32);
        out(b" (hot-plug active)\n");
        return;
    }
    out(name);
    out(b" xHCI init:\n");
    let Some(x) = Xhci::init(cap_base) else { return };
    let Some(slot) = ctls.iter_mut().find(|c| c.is_none()) else {
        out(b"  no controller slot free\n");
        return;
    };
    *slot = Some(Ctl { x, name, devs: [None; MAX_DEVS], hids: [None; MAX_HID], port_pending: 0, last_scan_ms: now_ms(), port_retry: [0; 32], msd: None, audio: None });
    let ctl = slot.as_mut().unwrap();
    // Power every port so later connects raise events even on empty ports.
    for p in 1..=ctl.x.max_ports {
        let off = ctl.x.portsc(p);
        let psc = unsafe { read32(off) };
        if psc & PORTSC_PP == 0 {
            unsafe { write32(off, portsc_base(psc)) };
        }
    }
    delay_us(20_000);
    for p in 1..=ctl.x.max_ports {
        if let Some(psc) = reset_port(&ctl.x, p) {
            enumerate_device(ctl, p, psc);
        }
    }
    ctl.clear_port_changes();
    if ctl.devs.iter().all(|d| d.is_none()) {
        out(name);
        out(b": nothing connected -- plug a keyboard or mouse in any time\n");
    }
}

fn abort_dev(ctl: &mut Ctl, di: usize, msg: &[u8]) {
    out(msg);
    ctl.remove_dev(di);
}

/// Address one device on root port `port`, read its descriptors, configure
/// all its boot keyboard/mouse endpoints at once, and start polling them.
fn enumerate_device(ctl: &mut Ctl, port: u32, psc: u32) {
    let speed = (psc >> 10) & 0xF; // 1 FS, 2 LS, 3 HS, 4+ SS
    let a = ctl.x.ring_command(0, TRB_ENABLE_SLOT << 10);
    let slot = match ctl.wait(Want::Cmd(a), 40_000) {
        Some((_, status, control)) if (status >> 24) & 0xFF == 1 => ((control >> 24) & 0xFF) as u8,
        Some((_, status, _)) => {
            out(b"  enable_slot: ");
            report_cc((status >> 24) & 0xFF);
            return;
        },
        None => {
            out(b"  enable_slot: NO completion event (timeout)\n");
            ctl.x.dump_state(b"  at timeout");
            return;
        },
    };
    dec(b"  slot assigned: ", slot as u32);
    out(b"\n");

    let (Some(input_ctx), Some(dev_ctx), Some(ep0_ring), Some(buf)) = (alloc_page(), alloc_page(), alloc_page(), alloc_page()) else {
        out(b"  out of DMA pages\n");
        return;
    };
    let Some(di) = ctl.devs.iter().position(|d| d.is_none()) else {
        out(b"  too many devices on this controller\n");
        return;
    };
    ctl.devs[di] = Some(Dev { slot, port, speed, ep0_ring, ep0_enq: 0, ep0_cycle: 1, input_ctx, dev_ctx, buf });
    let cs = ctl.x.ctx_size as u64;
    // Provisional EP0 max packet by speed (HS 64, SS 512, FS/LS 8 until read).
    let mps_default: u32 = match speed {
        3 => 64,
        s if s >= 4 => 512,
        _ => 8,
    };

    // --- Address Device -----------------------------------------------------
    // Input Control Context: add slot (A0) + EP0 (A1).
    unsafe { core::ptr::write_volatile((input_ctx + 0x04) as *mut u32, 0b11) };
    let slot_ctx = input_ctx + cs;
    unsafe {
        core::ptr::write_volatile(slot_ctx as *mut u32, (1u32 << 27) | (speed << 20));
        core::ptr::write_volatile((slot_ctx + 4) as *mut u32, port << 16);
    }
    let ep0 = input_ctx + 2 * cs;
    unsafe {
        core::ptr::write_volatile((ep0 + 4) as *mut u32, (mps_default << 16) | (4 << 3) | (3 << 1));
        core::ptr::write_volatile((ep0 + 8) as *mut u64, dma(ep0_ring) | 1); // TR deq | DCS
        core::ptr::write_volatile((ep0 + 16) as *mut u32, 8); // avg TRB length
    }
    unsafe { core::ptr::write_volatile((ctl.x.dcbaa + slot as u64 * 8) as *mut u64, dma(dev_ctx)) };
    dsb();

    let a = ctl.x.ring_command(dma(input_ctx), (TRB_ADDRESS_DEVICE << 10) | ((slot as u32) << 24));
    match ctl.wait(Want::Cmd(a), 40_000) {
        Some((_, status, _)) if (status >> 24) & 0xFF == 1 => {},
        Some((_, status, _)) => {
            out(b"  address_device: ");
            report_cc((status >> 24) & 0xFF);
            ctl.remove_dev(di);
            return;
        },
        None => return abort_dev(ctl, di, b"  address_device: no completion event\n"),
    }
    out(b"  device addressed\n");

    // --- GET_DESCRIPTOR(device): 8 bytes first to learn bMaxPacketSize0 ------
    let rd = |o: usize| -> u32 { (unsafe { core::ptr::read_volatile((buf + o as u64) as *const u8) }) as u32 };
    if ctl.control(di, 0x80, 6, 0x0100, 0, dma(buf), 8).is_none() {
        return abort_dev(ctl, di, b"  get device descriptor (8) failed\n");
    }
    let mps0 = rd(7);
    hex(b"  bMaxPacketSize0=", mps0 as u64, 2);
    if speed < 3 && (mps0 == 16 || mps0 == 32 || mps0 == 64) && !ctl.set_ep0_mps(di, mps0) {
        return abort_dev(ctl, di, b"  set EP0 max-packet failed\n");
    }
    if ctl.control(di, 0x80, 6, 0x0100, 0, dma(buf), 18).is_none() {
        return abort_dev(ctl, di, b"  get device descriptor (18) failed\n");
    }
    hex(b"    idVendor=", (rd(8) | (rd(9) << 8)) as u64, 4);
    hex(b"    idProduct=", (rd(10) | (rd(11) << 8)) as u64, 4);
    // kept for a class driver that names the device (the buffer is reused below)
    let dev_ids = ((rd(8) | (rd(9) << 8)) as u16, (rd(10) | (rd(11) << 8)) as u16, rd(14) as u8, rd(15) as u8);

    // --- GET_DESCRIPTOR(config): 9-byte header, then the full thing ---------
    if ctl.control(di, 0x80, 6, 0x0200, 0, dma(buf), 9).is_none() {
        return abort_dev(ctl, di, b"  get config header failed\n");
    }
    let total = (rd(2) | (rd(3) << 8)).min(4096) as u16;
    let cfg_val = rd(5);
    if ctl.control(di, 0x80, 6, 0x0200, 0, dma(buf), total).is_none() {
        return abort_dev(ctl, di, b"  get full config failed\n");
    }

    // Walk the config: every boot-protocol HID interface (protocol 1 keyboard,
    // 2 mouse) and its first interrupt-IN endpoint.
    let total = total as usize;
    let mut eps: [Option<HidEp>; MAX_HID] = [None; MAX_HID];
    let mut n = 0usize;
    let (mut cur_iface, mut cur_proto, mut cur_taken) = (0u32, 0u32, true);
    let mut cur_report_len = 0u32;
    // Bulk-only mass storage (class 8, SCSI transparent, protocol 0x50).
    let (mut msd_iface, mut in_msd) = (0u32, false);
    let (mut msd_in, mut msd_out): (Option<(u32, u32)>, Option<(u32, u32)>) = (None, None);
    // USB Audio Class: an AudioStreaming interface (class 1, subclass 2).
    let mut has_audio = false;
    let mut off = 0usize;
    while off + 2 <= total {
        let blen = rd(off) as usize;
        let btype = rd(off + 1);
        if blen == 0 {
            break;
        }
        if btype == 4 {
            // HID interfaces: boot keyboard / mouse (protocol 1 / 2), or any
            // other HID device (a gamepad, decided by its report descriptor
            // later); a wired Xbox 360 controller is vendor class FF/5D/01.
            let class = rd(off + 5);
            let is_hid = class == 3 && matches!(rd(off + 7), 0 | 1 | 2);
            let is_xbox = class == 0xFF && rd(off + 6) == 0x5D && rd(off + 7) == 0x01;
            cur_iface = rd(off + 2);
            cur_proto = if is_xbox { 0xFF } else { rd(off + 7) };
            cur_taken = !(is_hid || is_xbox);
            cur_report_len = 0;
            in_msd = rd(off + 5) == 8 && rd(off + 6) == 6 && rd(off + 7) == 0x50;
            if class == 1 && rd(off + 6) == 2 {
                has_audio = true;
            }
            if in_msd {
                msd_iface = cur_iface;
            }
        } else if btype == 5 && in_msd {
            let addr = rd(off + 2);
            if rd(off + 3) & 0x3 == 2 {
                let mps = rd(off + 4) | (rd(off + 5) << 8);
                if addr & 0x80 != 0 {
                    if msd_in.is_none() {
                        msd_in = Some((addr, mps));
                    }
                } else if msd_out.is_none() {
                    msd_out = Some((addr, mps));
                }
            }
        } else if btype == 0x21 && !cur_taken && blen >= 9 && rd(off + 6) == 0x22 {
            // The HID descriptor: how long the report descriptor is.
            cur_report_len = rd(off + 7) | (rd(off + 8) << 8);
        } else if btype == 5 && !cur_taken && n < MAX_HID {
            let addr = rd(off + 2);
            if addr & 0x80 != 0 && rd(off + 3) & 0x3 == 3 {
                let mps = rd(off + 4) | (rd(off + 5) << 8);
                eps[n] = Some(HidEp { iface: cur_iface, proto: cur_proto, addr, mps, interval: rd(off + 6), report_len: cur_report_len });
                n += 1;
                cur_taken = true;
            }
        }
        off += blen;
    }
    if n == 0 {
        if let (Some(i), Some(o)) = (msd_in, msd_out) {
            if ctl.msd.is_some() {
                out(b"  a second USB drive: one at a time is supported (left idle)\n");
                return;
            }
            configure_msd(ctl, di, cfg_val as u16, msd_iface, i, o);
            return;
        }
        if has_audio {
            if ctl.audio.is_some() {
                out(b"  a second USB audio device: one at a time is supported (left idle)\n");
                return;
            }
            let mut cfg = alloc::vec![0u8; total];
            for (k, b) in cfg.iter_mut().enumerate() {
                *b = rd(k) as u8;
            }
            configure_audio(ctl, di, cfg_val as u16, cfg, dev_ids);
            return;
        }
        // Not a keyboard/mouse/drive: leave it addressed but unused (a hub).
        out(b"  no boot keyboard/mouse or storage interface (device left idle)\n");
        return;
    }

    if has_audio {
        out(b"  (this device also has an audio function: only its HID part is used)\n");
    }

    // --- SET_CONFIGURATION --------------------------------------------------
    if ctl.control(di, 0x00, 9, cfg_val as u16, 0, 0, 0).is_none() {
        return abort_dev(ctl, di, b"  SET_CONFIGURATION failed\n");
    }

    // --- Configure all the interrupt-IN endpoints in one command -----------
    let mut add_flags = 1u32;
    let mut max_dci = 1u32;
    let mut rings = [0u64; MAX_HID];
    for (i, e) in eps.iter().flatten().enumerate() {
        let dci = (e.addr & 0x0F) * 2 + 1;
        add_flags |= 1 << dci;
        max_dci = max_dci.max(dci);
        let Some(ring) = alloc_page() else { return abort_dev(ctl, di, b"  out of DMA pages\n") };
        rings[i] = ring;
        let mps = if e.mps == 0 { 8 } else { e.mps & 0x7FF };
        let interval_field = if speed >= 3 {
            // HS/SS: bInterval is already an exponent (2^(b-1) microframes).
            e.interval.max(1).saturating_sub(1).min(15)
        } else {
            // FS/LS: frames → 125us exponent = 3 + floor(log2(bInterval)).
            let mut iv = 0u32;
            let mut bi = e.interval.max(1);
            while bi > 1 {
                bi >>= 1;
                iv += 1;
            }
            (iv + 3).clamp(3, 15)
        };
        // Input-context index 0 is the Input Control Context, 1 the slot, so the
        // endpoint with DCI n sits at index n+1.
        let epc = input_ctx + (dci as u64 + 1) * cs;
        unsafe {
            core::ptr::write_volatile(epc as *mut u32, interval_field << 16);
            core::ptr::write_volatile((epc + 4) as *mut u32, (mps << 16) | (7 << 3) | (3 << 1)); // Int-IN, CErr 3
            core::ptr::write_volatile((epc + 8) as *mut u64, dma(ring) | 1); // TR deq | DCS
            core::ptr::write_volatile((epc + 16) as *mut u32, (mps << 16) | mps.min(8));
        }
    }
    unsafe {
        core::ptr::write_volatile(input_ctx as *mut u32, 0);
        core::ptr::write_volatile((input_ctx + 0x04) as *mut u32, add_flags);
        let sctx = input_ctx + cs;
        core::ptr::write_volatile(sctx as *mut u32, (max_dci << 27) | (speed << 20));
        core::ptr::write_volatile((sctx + 4) as *mut u32, port << 16);
    }
    dsb();
    let a = ctl.x.ring_command(dma(input_ctx), (TRB_CONFIGURE_ENDPOINT << 10) | ((slot as u32) << 24));
    match ctl.wait(Want::Cmd(a), 40_000) {
        Some((_, s, _)) if (s >> 24) & 0xFF == 1 => out(b"  endpoints configured\n"),
        Some((_, s, _)) => {
            out(b"  configure endpoint: ");
            report_cc((s >> 24) & 0xFF);
            for r in rings.iter().take(n) {
                free_page(*r);
            }
            ctl.remove_dev(di);
            return;
        },
        None => {
            for r in rings.iter().take(n) {
                free_page(*r);
            }
            return abort_dev(ctl, di, b"  configure endpoint: no event\n");
        },
    }

    for (i, e) in eps.iter().flatten().enumerate() {
        let mut pad = crate::drivers::hidpad::Layout::EMPTY;
        let kind = match e.proto {
            1 => HidKind::Kbd,
            2 => HidKind::Mouse,
            0xFF => {
                pad = crate::drivers::hidpad::Layout::XBOX360;
                HidKind::Pad
            },
            _ => {
                // Another HID device: a gamepad / joystick if its report
                // descriptor says so (read with GET_DESCRIPTOR(Report)).
                let rl = e.report_len.clamp(1, 2048) as u16;
                let mut lay = None;
                if ctl.control(di, 0x81, 6, 0x2200, e.iface as u16, dma(buf), rl).is_some() {
                    let mut desc = alloc::vec![0u8; rl as usize];
                    for (k, b) in desc.iter_mut().enumerate() {
                        *b = unsafe { core::ptr::read_volatile((buf + k as u64) as *const u8) };
                    }
                    lay = crate::drivers::hidpad::parse_descriptor(&desc);
                }
                match lay {
                    Some(l) => {
                        pad = l;
                        HidKind::Pad
                    },
                    None => {
                        out(b"  a HID interface that is not a keyboard, mouse or gamepad (left idle)\n");
                        continue;
                    },
                }
            },
        };
        let boot = if kind == HidKind::Pad {
            // Gamepads speak the report protocol; an Xbox 360 pad is not HID at all.
            if e.proto != 0xFF {
                let _ = ctl.control(di, 0x21, 0x0A, 0, e.iface as u16, 0, 0); // SET_IDLE(0)
            }
            false
        } else {
            // SET_PROTOCOL(boot) — fixed 8-byte keyboard / 3-4 byte mouse reports —
            // and SET_IDLE(0) so reports only arrive on change. Best effort: some
            // devices STALL these and still send boot-format reports.
            let b = ctl.control(di, 0x21, 0x0B, 0, e.iface as u16, 0, 0).is_some();
            let _ = ctl.control(di, 0x21, 0x0A, 0, e.iface as u16, 0, 0);
            b
        };
        out(match kind {
            HidKind::Kbd => b"  keyboard interface",
            HidKind::Mouse => b"  mouse interface",
            HidKind::Pad => {
                if pad.xbox360 {
                    b"  Xbox 360 controller"
                } else {
                    b"  gamepad interface"
                }
            },
        });
        if kind == HidKind::Pad {
            dec(b": axes=", pad.n_axes as u32);
            dec(b" buttons=", pad.n_buttons as u32);
            out(if pad.hat.is_some() { b" hat\n" } else { b"\n" });
        } else {
            out(if boot { b" (boot protocol)\n" } else { b" (report protocol)\n" });
        }
        let Some(rbuf) = alloc_page() else { return abort_dev(ctl, di, b"  out of DMA pages\n") };
        let mps = if e.mps == 0 { 8 } else { e.mps & 0x7FF };
        let Some(hi) = ctl.hids.iter().position(|h| h.is_none()) else {
            free_page(rbuf);
            out(b"  too many keyboard/mouse endpoints\n");
            break;
        };
        let mut h = Hid {
            kind,
            slot,
            port,
            dci: (e.addr & 0x0F) * 2 + 1,
            ep_addr: e.addr,
            ring: rings[i],
            enq: 0,
            cycle: 1,
            rbuf,
            mps,
            prev: [0; 8],
            pad,
            halted: false,
            last_cc: 0,
            fails: 0,
            fill: 0,
            done: 0,
        };
        hid_arm_all(&ctl.x, &mut h); // queue the interrupt transfers
        ctl.hids[hi] = Some(h);
        if kind == HidKind::Pad {
            unsafe {
                PAD = crate::drivers::hidpad::PadState::default();
                PAD.connected = true;
                PAD.xbox = pad.xbox360;
            }
        }
        out(match kind {
            HidKind::Kbd => b"  -> USB KEYBOARD READY\n",
            HidKind::Mouse => b"  -> USB MOUSE READY\n",
            HidKind::Pad => b"  -> USB GAMEPAD READY\n",
        });
    }
}

// ── Polling: keyboards + mice, recovery, hot-plug ────────────────────────────

/// Keys currently held (the last keyboard report): modifiers + 6 usages.
static mut HELD: [u8; 8] = [0; 8];
/// The gamepad (one at a time), as its last report left it.
static mut PAD: crate::drivers::hidpad::PadState = crate::drivers::hidpad::PadState { connected: false, axes: [0.0; 8], buttons: 0, hat: -1, xbox: false };

/// The gamepad's state after draining pending reports.
pub fn pad_state() -> crate::drivers::hidpad::PadState {
    poll();
    unsafe { *(&raw const PAD) }
}
/// Wheel clicks accumulated since the last [`mouse_take_wheel`].
static mut MOUSE_WHEEL: i32 = 0;
static mut IN_POLL: bool = false;
/// Which HID kinds have logged their first report (bit = `HidKind as u8`).
static mut FIRST_REPORT: u8 = 0;

fn any_hid(kind: HidKind) -> bool {
    let ctls = unsafe { &*(&raw const CTLS) };
    ctls.iter().flatten().any(|c| c.hids.iter().flatten().any(|h| h.kind == kind))
}

/// True while a USB keyboard is plugged in and being polled.
pub fn keyboard_ready() -> bool {
    any_hid(HidKind::Kbd)
}

/// True while a USB mouse is plugged in and being polled.
pub fn mouse_ready() -> bool {
    any_hid(HidKind::Mouse)
}

/// Queue the next interrupt-IN transfer and ring the endpoint doorbell. The ring
/// is circular: when the producer reaches the last slot it writes a Link TRB
/// (Toggle Cycle) back to the start and flips its cycle bit.
/// Bulk-only mass storage: SET_CONFIGURATION, both bulk endpoints in one
/// Configure Endpoint command, then the device is handed to drivers::usbmsd
/// (from poll(), outside this controller borrow).
fn configure_msd(ctl: &mut Ctl, di: usize, cfg_val: u16, iface: u32, ep_in: (u32, u32), ep_out: (u32, u32)) {
    let Some(d) = ctl.devs[di] else { return };
    let (slot, port, speed, input_ctx) = (d.slot, d.port, d.speed, d.input_ctx);
    if ctl.control(di, 0x00, 9, cfg_val, 0, 0, 0).is_none() {
        return abort_dev(ctl, di, b"  SET_CONFIGURATION failed\n");
    }
    let cs = ctl.x.ctx_size as u64;
    let (Some(ring_in), Some(ring_out), Some(data)) = (alloc_page(), alloc_page(), alloc_page()) else {
        return abort_dev(ctl, di, b"  out of DMA pages\n");
    };
    let mut add_flags = 1u32;
    let mut max_dci = 1u32;
    for (addr, mps, ring, is_in) in [(ep_in.0, ep_in.1, ring_in, true), (ep_out.0, ep_out.1, ring_out, false)] {
        let dci = (addr & 0x0F) * 2 + if is_in { 1 } else { 0 };
        add_flags |= 1 << dci;
        max_dci = max_dci.max(dci);
        let mps = if mps == 0 { 512 } else { mps & 0x7FF };
        let ep_type = if is_in { 6u32 } else { 2 }; // bulk IN / bulk OUT
        let epc = input_ctx + (dci as u64 + 1) * cs;
        unsafe {
            core::ptr::write_volatile(epc as *mut u32, 0);
            core::ptr::write_volatile((epc + 4) as *mut u32, (mps << 16) | (ep_type << 3) | (3 << 1));
            core::ptr::write_volatile((epc + 8) as *mut u64, dma(ring) | 1);
            core::ptr::write_volatile((epc + 16) as *mut u32, 3072);
        }
    }
    unsafe {
        core::ptr::write_volatile(input_ctx as *mut u32, 0);
        core::ptr::write_volatile((input_ctx + 0x04) as *mut u32, add_flags);
        let sctx = input_ctx + cs;
        core::ptr::write_volatile(sctx as *mut u32, (max_dci << 27) | (speed << 20));
        core::ptr::write_volatile((sctx + 4) as *mut u32, port << 16);
    }
    dsb();
    let a = ctl.x.ring_command(dma(input_ctx), (TRB_CONFIGURE_ENDPOINT << 10) | ((slot as u32) << 24));
    match ctl.wait(Want::Cmd(a), 40_000) {
        Some((_, s, _)) if (s >> 24) & 0xFF == 1 => {},
        Some((_, s, _)) => {
            out(b"  configure bulk endpoints: ");
            report_cc((s >> 24) & 0xFF);
            free_page(ring_in);
            free_page(ring_out);
            free_page(data);
            ctl.remove_dev(di);
            return;
        },
        None => {
            free_page(ring_in);
            free_page(ring_out);
            free_page(data);
            return abort_dev(ctl, di, b"  configure bulk endpoints: no event\n");
        },
    }
    let mk = |addr: u32, ring: u64, is_in: bool| BulkEp { dci: (addr & 0x0F) * 2 + if is_in { 1 } else { 0 }, ep_addr: addr, ring, enq: 0, cycle: 1 };
    ctl.msd = Some(Msd { slot, di, iface, ep_in: mk(ep_in.0, ring_in, true), ep_out: mk(ep_out.0, ring_out, false), data });
    unsafe { MSD_ATTACH = true };
    out(b"  -> USB STORAGE (bulk endpoints ready)\n");
}

impl Ctl {
    /// One bulk transfer of `len` bytes at `data` on `ep`; the completion code
    /// and residual, or None without an event (10 s).
    fn bulk(&mut self, slot: u8, ep: &mut BulkEp, data: u64, len: u32) -> Option<(u32, u32)> {
        trb_write(ep.ring, ep.enq, data, len, (TRB_NORMAL << 10) | (1 << 5) | (1 << 2) | ep.cycle);
        ep.enq += 1;
        if ep.enq == RING_TRBS - 1 {
            trb_write(ep.ring, ep.enq, dma(ep.ring), 0, (TRB_LINK << 10) | (1 << 1) | ep.cycle);
            ep.enq = 0;
            ep.cycle ^= 1;
        }
        dsb();
        unsafe { write32(self.x.db + slot as usize * 4, ep.dci) };
        let (_, status, _) = self.wait(Want::Xfer(slot, ep.dci), 200_000)?;
        Some(((status >> 24) & 0xFF, status & 0xFF_FFFF))
    }

    /// A stalled / halted bulk endpoint back to running: Reset Endpoint, the
    /// dequeue pointer at our enqueue, CLEAR_FEATURE(ENDPOINT_HALT).
    fn bulk_clear(&mut self, slot: u8, di: usize, ep: &mut BulkEp) {
        let sl = (slot as u32) << 24;
        let a = self.x.ring_command(0, (TRB_RESET_ENDPOINT << 10) | (ep.dci << 16) | sl);
        let _ = self.wait(Want::Cmd(a), 20_000);
        let deq = dma(ep.ring + (ep.enq * 16) as u64) | ep.cycle as u64;
        let a = self.x.ring_command(deq, (TRB_SET_TR_DEQUEUE << 10) | (ep.dci << 16) | sl);
        let _ = self.wait(Want::Cmd(a), 20_000);
        let _ = self.control(di, 0x02, 1, 0, ep.ep_addr as u16, 0, 0);
    }
}

fn with_msd<R>(f: impl FnOnce(&mut Ctl, &mut Msd) -> R) -> Option<R> {
    let ctls = unsafe { &mut *(&raw mut CTLS) };
    for ctl in ctls.iter_mut().flatten() {
        if let Some(mut m) = ctl.msd.take() {
            let r = f(ctl, &mut m);
            ctl.msd = Some(m);
            return Some(r);
        }
    }
    None
}

/// Is a mass-storage device attached (bulk endpoints configured)?
pub fn msd_present() -> bool {
    let ctls = unsafe { &*(&raw const CTLS) };
    ctls.iter().flatten().any(|c| c.msd.is_some())
}

/// Send `buf` (up to a page) on the drive's bulk-OUT endpoint: bytes sent, or
/// the completion code (6 = stall; 0 = no device / no event).
pub fn msd_bulk_out(buf: &[u8]) -> Result<usize, u32> {
    with_msd(|ctl, m| {
        let n = buf.len().min(4096);
        unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), m.data as *mut u8, n) };
        dsb();
        let mut ep = m.ep_out;
        let r = ctl.bulk(m.slot, &mut ep, dma(m.data), n as u32);
        m.ep_out = ep;
        match r {
            Some((1 | 13, resid)) => Ok(n - (resid as usize).min(n)),
            Some((cc, _)) => Err(cc),
            None => Err(0),
        }
    })
    .unwrap_or(Err(0))
}

/// Receive into `buf` (up to a page) from the drive's bulk-IN endpoint.
pub fn msd_bulk_in(buf: &mut [u8]) -> Result<usize, u32> {
    with_msd(|ctl, m| {
        let n = buf.len().min(4096);
        let mut ep = m.ep_in;
        let r = ctl.bulk(m.slot, &mut ep, dma(m.data), n as u32);
        m.ep_in = ep;
        match r {
            Some((1 | 13, resid)) => {
                let got = n - (resid as usize).min(n);
                dsb();
                unsafe { core::ptr::copy_nonoverlapping(m.data as *const u8, buf.as_mut_ptr(), got) };
                Ok(got)
            },
            Some((cc, _)) => Err(cc),
            None => Err(0),
        }
    })
    .unwrap_or(Err(0))
}

pub fn msd_clear_stall(dir_in: bool) -> bool {
    with_msd(|ctl, m| {
        let mut ep = if dir_in { m.ep_in } else { m.ep_out };
        ctl.bulk_clear(m.slot, m.di, &mut ep);
        if dir_in {
            m.ep_in = ep;
        } else {
            m.ep_out = ep;
        }
    })
    .is_some()
}

/// Bulk-Only Mass Storage Reset (class request) after a phase error.
pub fn msd_reset() -> bool {
    with_msd(|ctl, m| ctl.control(m.di, 0x21, 0xFF, 0, m.iface as u16, 0, 0).is_some()).unwrap_or(false)
}

// ── Isochronous endpoints: USB audio (drivers::usbaudio) ─────────────────────
//
// One TD per service interval, each a single Isoch TRB with SIA set (Start
// Isoch ASAP: the controller puts every TD in the interval right after the
// previous one, so frame IDs never have to be computed), on a one-segment ring
// closed by a Link TRB. The class driver refills the ring from the audio pump
// (once a frame) and keeps a fixed amount of audio queued; progress is learnt
// from completion events, which carry the address of the TRB they complete,
// so only every few TDs needs IOC. An empty ring raises Ring Underrun (OUT) /
// Ring Overrun (IN) -- the endpoint keeps running and the next doorbell resumes
// it ASAP. Isochronous endpoints never halt (xHCI 4.10.2).

/// Allocate 2^order zeroed, physically contiguous pages below 4 GiB.
fn alloc_pages(order: usize) -> Option<u64> {
    let p = crate::mm::frame::alloc_frames(order)?;
    let bytes = 4096u64 << order;
    if p + bytes > 1 << 32 {
        crate::mm::frame::free_frames(p, order);
        out(b"  xhci: frames above 4 GiB, not usable for DMA\n");
        return None;
    }
    for i in 0..bytes / 8 {
        unsafe { core::ptr::write_volatile((p + i * 8) as *mut u64, 0) };
    }
    Some(p)
}

fn order_for(bytes: u64) -> usize {
    let pages = bytes.div_ceil(4096).max(1);
    let mut o = 0;
    while (1u64 << o) < pages {
        o += 1;
    }
    o
}

/// One isochronous transfer ring; TD slot i's data lives at `buf + i * slot_bytes`.
#[derive(Clone, Copy)]
struct IsoRing {
    dci: u32,
    is_in: bool,
    ring: u64,
    ring_order: usize,
    trbs: usize,
    enq: usize,
    cycle: u32,
    buf: u64,
    buf_order: usize,
    slot_bytes: u32,
    mps: u32,
    /// TDs queued / known to be complete.
    sent: u64,
    done: u64,
    ioc_every: u32,
    since_ioc: u32,
    last_event_ms: u64,
    events: u32,
    underruns: u32,
    missed: u32,
    errors: u32,
    last_err_cc: u32,
    resyncs: u32,
    /// The watchdog rang the doorbell for a silent ring (stage 1).
    kicked: bool,
    /// Feedback IN: the newest packet (raw little-endian value, its length,
    /// and a counter that moves when a new one arrives).
    fb_raw: u32,
    fb_len: u32,
    fb_seq: u32,
}

impl IsoRing {
    fn new(dci: u32, is_in: bool, mps: u32, ring_order: usize, slot_bytes: u32, ioc_every: u32) -> Option<IsoRing> {
        let trbs = (4096usize << ring_order) / 16;
        let ring = alloc_pages(ring_order)?;
        let buf_order = order_for((trbs as u64 - 1) * slot_bytes as u64);
        let Some(buf) = alloc_pages(buf_order) else {
            crate::mm::frame::free_frames(ring, ring_order);
            return None;
        };
        Some(IsoRing {
            dci,
            is_in,
            ring,
            ring_order,
            trbs,
            enq: 0,
            cycle: 1,
            buf,
            buf_order,
            slot_bytes,
            mps,
            sent: 0,
            done: 0,
            ioc_every: ioc_every.max(1),
            since_ioc: 0,
            last_event_ms: now_ms(),
            events: 0,
            underruns: 0,
            missed: 0,
            errors: 0,
            last_err_cc: 0,
            resyncs: 0,
            kicked: false,
            fb_raw: 0,
            fb_len: 0,
            fb_seq: 0,
        })
    }

    fn free(&self) {
        crate::mm::frame::free_frames(self.ring, self.ring_order);
        crate::mm::frame::free_frames(self.buf, self.buf_order);
    }

    /// TD positions 0..usable(); the last TRB is the Link.
    fn usable(&self) -> usize {
        self.trbs - 1
    }

    fn queued(&self) -> u64 {
        self.sent - self.done
    }

    fn index_of(&self, param: u64) -> Option<usize> {
        let base = dma(self.ring);
        if param < base || param >= base + (self.usable() * 16) as u64 || (param - base) % 16 != 0 {
            return None;
        }
        Some(((param - base) / 16) as usize)
    }

    /// The TD at ring index `idx` completed (and with it every earlier one).
    fn completed_through(&mut self, idx: usize) {
        let u = self.usable();
        let pending = ((self.enq + u - (idx + 1) % u) % u) as u64;
        let d = self.sent - pending.min(self.queued());
        if d > self.done {
            self.done = d;
        }
    }

    fn on_event(&mut self, param: u64, status: u32) {
        let cc = (status >> 24) & 0xFF;
        self.events = self.events.wrapping_add(1);
        self.last_event_ms = now_ms();
        self.kicked = false;
        let idx = self.index_of(param);
        match cc {
            1 | 13 => {
                if let Some(i) = idx {
                    self.completed_through(i);
                    if self.is_in {
                        let got = self.mps.saturating_sub(status & 0xFF_FFFF).min(4);
                        let a = self.buf + i as u64 * self.slot_bytes as u64;
                        let mut v = 0u32;
                        for k in 0..got {
                            v |= (unsafe { core::ptr::read_volatile((a + k as u64) as *const u8) } as u32) << (8 * k);
                        }
                        self.fb_raw = v;
                        self.fb_len = got;
                        self.fb_seq = self.fb_seq.wrapping_add(1);
                    }
                }
            },
            // Ring Underrun (OUT) / Overrun (IN): everything queued has run.
            14 | 15 => {
                self.underruns = self.underruns.wrapping_add(1);
                self.done = self.sent;
            },
            // Missed Service: a TD was too late for its interval and was skipped.
            23 => {
                self.missed = self.missed.wrapping_add(1);
                if let Some(i) = idx {
                    self.completed_through(i);
                }
            },
            // Stopped (our own Stop Endpoint): nothing to count.
            26 | 27 => {},
            _ => {
                self.errors = self.errors.wrapping_add(1);
                self.last_err_cc = cc;
                if let Some(i) = idx {
                    self.completed_through(i);
                }
            },
        }
    }

    /// Queue one TD of `len` bytes from its slot (already written). The
    /// caller rings the doorbell.
    fn push_td(&mut self, len: u32) {
        let i = self.enq;
        let addr = dma(self.buf + i as u64 * self.slot_bytes as u64);
        // High-bandwidth HS endpoints split a TD into up to 3 transactions:
        // TBC stays 0 below SuperSpeed, TLBPC = packets - 1.
        let packets = if self.mps == 0 { 1 } else { len.div_ceil(self.mps).max(1) };
        self.since_ioc += 1;
        let ioc = self.since_ioc >= self.ioc_every;
        if ioc {
            self.since_ioc = 0;
        }
        let mut control = (TRB_ISOCH << 10) | ((packets - 1).min(15) << 16) | (1 << 31) | self.cycle;
        if ioc {
            control |= 1 << 5;
        }
        if self.sent == self.done {
            self.last_event_ms = now_ms(); // the ring was idle: the watchdog starts now
        }
        trb_write(self.ring, i, addr, len & 0x1_FFFF, control);
        self.enq += 1;
        self.sent += 1;
        if self.enq == self.usable() {
            trb_write(self.ring, self.enq, dma(self.ring), 0, (TRB_LINK << 10) | (1 << 1) | self.cycle);
            self.enq = 0;
            self.cycle ^= 1;
        }
    }
}

/// The USB audio device on a controller: its configuration descriptor (for
/// the class driver to parse) and, once streaming, its isochronous rings.
struct AudioDev {
    slot: u8,
    di: usize,
    port: u32,
    speed: u32,
    vid: u16,
    pid: u16,
    i_manufacturer: u8,
    i_product: u8,
    cfg: alloc::vec::Vec<u8>,
    out: Option<IsoRing>,
    fb: Option<IsoRing>,
}

impl AudioDev {
    fn free(&self) {
        if let Some(r) = self.out {
            r.free();
        }
        if let Some(r) = self.fb {
            r.free();
        }
    }

    /// A transfer event on this device: true when it was one of our rings.
    fn on_event(&mut self, dci: u32, param: u64, status: u32) -> bool {
        for r in [self.out.as_mut(), self.fb.as_mut()].into_iter().flatten() {
            if r.dci == dci {
                r.on_event(param, status);
                return true;
            }
        }
        false
    }
}

/// SET_CONFIGURATION, then hand the device to drivers::usbaudio (from poll(),
/// outside this controller borrow), which picks the stream and opens it.
fn configure_audio(ctl: &mut Ctl, di: usize, cfg_val: u16, cfg: alloc::vec::Vec<u8>, ids: (u16, u16, u8, u8)) {
    let Some(d) = ctl.devs[di] else { return };
    if ctl.control(di, 0x00, 9, cfg_val, 0, 0, 0).is_none() {
        return abort_dev(ctl, di, b"  SET_CONFIGURATION failed\n");
    }
    ctl.audio = Some(AudioDev { slot: d.slot, di, port: d.port, speed: d.speed, vid: ids.0, pid: ids.1, i_manufacturer: ids.2, i_product: ids.3, cfg, out: None, fb: None });
    unsafe { AUDIO_ATTACH = true };
    out(b"  -> USB AUDIO (configured; class driver attaches next)\n");
}

fn audio_ctl() -> Option<&'static mut Ctl> {
    let ctls = unsafe { &mut *(&raw mut CTLS) };
    ctls.iter_mut().flatten().find(|c| c.audio.is_some())
}

/// What the class driver needs to know about the attached device.
pub struct AudioDevInfo {
    pub vid: u16,
    pub pid: u16,
    pub i_manufacturer: u8,
    pub i_product: u8,
    /// PORTSC speed: 1 full, 2 low, 3 high, 4+ SuperSpeed.
    pub speed: u32,
    pub port: u32,
    pub controller: &'static [u8],
    pub cfg: alloc::vec::Vec<u8>,
}

pub fn audio_info() -> Option<AudioDevInfo> {
    let ctl = audio_ctl()?;
    let a = ctl.audio.as_ref()?;
    Some(AudioDevInfo { vid: a.vid, pid: a.pid, i_manufacturer: a.i_manufacturer, i_product: a.i_product, speed: a.speed, port: a.port, controller: ctl.name, cfg: a.cfg.clone() })
}

/// A control request to the audio device. OUT data is sent from `data`; IN
/// data lands in it. Bytes transferred, or None (stall / no device).
pub fn audio_control(bm_rt: u8, req: u8, val: u16, idx: u16, data: &mut [u8]) -> Option<usize> {
    let ctl = audio_ctl()?;
    let di = ctl.audio.as_ref()?.di;
    let d = ctl.devs[di]?;
    let n = data.len().min(4096);
    let is_in = bm_rt & 0x80 != 0;
    if !is_in && n > 0 {
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), d.buf as *mut u8, n) };
        dsb();
    }
    let got = (ctl.control(di, bm_rt, req, val, idx, if n > 0 { dma(d.buf) } else { 0 }, n as u16)? as usize).min(n);
    if is_in {
        dsb();
        unsafe { core::ptr::copy_nonoverlapping(d.buf as *const u8, data.as_mut_ptr(), got) };
    }
    Some(got)
}

/// A string descriptor of the audio device (US English), as ASCII.
pub fn audio_string(index: u8) -> Option<alloc::string::String> {
    if index == 0 {
        return None;
    }
    let mut b = [0u8; 255];
    let n = audio_control(0x80, 6, 0x0300 | index as u16, 0x0409, &mut b)?;
    let n = n.min(b[0] as usize);
    let mut s = alloc::string::String::new();
    let mut i = 2;
    while i + 1 < n {
        let c = b[i] as u16 | (b[i + 1] as u16) << 8;
        s.push(if (0x20..0x7F).contains(&c) { c as u8 as char } else { '?' });
        i += 2;
    }
    let t = s.trim();
    if t.is_empty() { None } else { Some(alloc::string::String::from(t)) }
}

/// Add the stream's isochronous endpoints to the device (Configure Endpoint:
/// this is where the controller reserves the bus bandwidth -- completion code
/// 22 means there is not enough). Must precede SET_INTERFACE. `out_slot` is
/// the most bytes one OUT packet will carry (power of two, <= 4096).
pub fn audio_open(
    data: crate::drivers::usbaudio_desc::IsoEndpoint,
    fb: Option<crate::drivers::usbaudio_desc::IsoEndpoint>,
    out_slot: u32,
) -> Result<(), u32> {
    use crate::drivers::usbaudio_desc as ud;
    let ctl = audio_ctl().ok_or(0u32)?;
    let a = ctl.audio.as_ref().ok_or(0u32)?;
    if a.out.is_some() {
        return Ok(());
    }
    let (di, slot) = (a.di, a.slot);
    let d = ctl.devs[di].ok_or(0u32)?;
    let hs = d.speed >= 3;
    let cs = ctl.x.ctx_size as u64;
    let input_ctx = d.input_ctx;

    let mk = |e: &ud::IsoEndpoint, slot_bytes: u32, ring_order: usize| -> Option<IsoRing> {
        let dci = (e.addr as u32 & 0x0F) * 2 + if e.is_in() { 1 } else { 0 };
        let pps = ud::packets_per_sec(hs, e.interval);
        // IOC every ~8 ms of OUT data / ~16 ms of feedback: ~190 events/s, so
        // the 256-entry event ring (shared with keyboard and mouse) survives
        // a main loop that stops draining it for over a second.
        let ioc = if e.is_in() { pps / 62 } else { pps / 125 };
        IsoRing::new(dci, e.is_in(), e.mps(), ring_order, slot_bytes, ioc)
    };
    // OUT: 1024 TDs at high speed (128 ms of 125 µs packets), 256 at full speed.
    let Some(ro) = mk(&data, out_slot.clamp(16, 4096), if hs { 2 } else { 0 }) else {
        out(b"  usbaudio: out of DMA pages for the stream ring\n");
        return Err(0);
    };
    let rf = match fb {
        Some(e) => match mk(&e, e.mps().next_power_of_two().clamp(16, 4096), 0) {
            Some(r) => Some(r),
            None => {
                ro.free();
                return Err(0);
            },
        },
        None => None,
    };

    let mut add = 1u32; // slot context
    let mut max_dci = 1u32;
    for (e, r) in [(Some(data), Some(ro)), (fb, rf)] {
        let (Some(e), Some(r)) = (e, r) else { continue };
        add |= 1 << r.dci;
        max_dci = max_dci.max(r.dci);
        let mps = e.mps();
        let burst = if hs { e.extra_tx() } else { 0 };
        let esit = mps * (burst + 1);
        let interval = ud::xhci_interval(hs, e.interval);
        let ep_type = if e.is_in() { 5u32 } else { 1 }; // Isoch IN / Isoch OUT
        let epc = input_ctx + (r.dci as u64 + 1) * cs;
        unsafe {
            core::ptr::write_volatile(epc as *mut u32, (interval << 16) | ((esit >> 16) << 24));
            core::ptr::write_volatile((epc + 4) as *mut u32, (mps << 16) | (burst << 8) | (ep_type << 3)); // CErr 0 for isoch
            core::ptr::write_volatile((epc + 8) as *mut u64, dma(r.ring) | 1);
            core::ptr::write_volatile((epc + 16) as *mut u32, ((esit & 0xFFFF) << 16) | esit.min(0xFFFF));
        }
        out(b"  usbaudio: endpoint ");
        hex(b"", e.addr as u64, 2);
        dec(b"    dci=", r.dci);
        dec(b" type=", ep_type);
        dec(b" mps=", mps);
        dec(b" burst=", burst);
        dec(b" esit=", esit);
        dec(b" interval-exp=", interval);
        dec(b" ring-trbs=", r.trbs as u32);
        dec(b" slot-bytes=", r.slot_bytes);
        dec(b" ioc-every=", r.ioc_every);
        out(b"\n");
    }
    unsafe {
        core::ptr::write_volatile(input_ctx as *mut u32, 0);
        core::ptr::write_volatile((input_ctx + 0x04) as *mut u32, add);
        let sctx = input_ctx + cs;
        core::ptr::write_volatile(sctx as *mut u32, (max_dci << 27) | (d.speed << 20));
        core::ptr::write_volatile((sctx + 4) as *mut u32, d.port << 16);
    }
    dsb();
    let cmd = ctl.x.ring_command(dma(input_ctx), (TRB_CONFIGURE_ENDPOINT << 10) | ((slot as u32) << 24));
    let cc = match ctl.wait(Want::Cmd(cmd), 40_000) {
        Some((_, s, _)) => (s >> 24) & 0xFF,
        None => 0,
    };
    if cc != 1 {
        out(b"  usbaudio: configure isochronous endpoints: ");
        match cc {
            0 => out(b"no completion event\n"),
            22 => out(b"BANDWIDTH ERROR (not enough periodic bandwidth on this port)\n"),
            17 => out(b"parameter error (endpoint context rejected)\n"),
            _ => report_cc(cc),
        }
        ro.free();
        if let Some(r) = rf {
            r.free();
        }
        return Err(cc);
    }
    out(b"  usbaudio: isochronous endpoints configured (bandwidth reserved)\n");
    let a = ctl.audio.as_mut().unwrap();
    a.out = Some(ro);
    a.fb = rf;
    Ok(())
}

/// Ring state for the class driver's pacing.
#[derive(Clone, Copy, Default)]
pub struct IsoStats {
    pub queued: u64,
    pub capacity: u64,
    pub sent: u64,
    pub done: u64,
    pub events: u32,
    pub underruns: u32,
    pub missed: u32,
    pub errors: u32,
    pub last_err_cc: u32,
    pub resyncs: u32,
    pub slot_bytes: u32,
    pub ep_state: u32,
}

fn ep_state(ctl: &Ctl, slot: u8, dci: u32) -> u32 {
    let Some(d) = ctl.devs.iter().flatten().find(|d| d.slot == slot) else { return 0 };
    unsafe { read32((d.dev_ctx + dci as u64 * ctl.x.ctx_size as u64) as usize) & 7 }
}

/// (OUT ring, feedback ring) statistics.
pub fn audio_stats() -> Option<(IsoStats, Option<IsoStats>)> {
    let ctl = audio_ctl()?;
    let a = ctl.audio.as_ref()?;
    let st = |r: &IsoRing| IsoStats {
        queued: r.queued(),
        capacity: r.usable() as u64 - 1,
        sent: r.sent,
        done: r.done,
        events: r.events,
        underruns: r.underruns,
        missed: r.missed,
        errors: r.errors,
        last_err_cc: r.last_err_cc,
        resyncs: r.resyncs,
        slot_bytes: r.slot_bytes,
        ep_state: ep_state(ctl, a.slot, r.dci),
    };
    let o = st(a.out.as_ref()?);
    Some((o, a.fb.as_ref().map(st)))
}

/// Queue OUT packets: `fill(slot)` writes one packet into its DMA slot and
/// returns its length; called `n` times (fewer if the ring is full). The
/// doorbell is rung once at the end. Returns packets queued.
pub fn audio_out_push(n: usize, fill: &mut dyn FnMut(&mut [u8]) -> usize) -> usize {
    let Some(ctl) = audio_ctl() else { return 0 };
    let db = ctl.x.db;
    let Some(a) = ctl.audio.as_mut() else { return 0 };
    let slot = a.slot;
    let Some(r) = a.out.as_mut() else { return 0 };
    let mut k = 0;
    while k < n && r.queued() + 1 < r.usable() as u64 {
        let p = r.buf + r.enq as u64 * r.slot_bytes as u64;
        let s = unsafe { core::slice::from_raw_parts_mut(p as *mut u8, r.slot_bytes as usize) };
        let len = fill(s).min(r.slot_bytes as usize);
        r.push_td(len as u32);
        k += 1;
    }
    // Ring even with nothing new: a controller that fetches only part of
    // the ring ahead (QEMU takes 16 TDs) continues on a doorbell. Harmless
    // on a running endpoint.
    if k > 0 || r.queued() > 0 {
        dsb();
        unsafe { write32(db + slot as usize * 4, r.dci) };
    }
    k
}

/// Keep `target` feedback requests queued. Returns the newest feedback
/// packet: (raw value, length, sequence number).
pub fn audio_fb_service(target: usize) -> Option<(u32, u32, u32)> {
    let ctl = audio_ctl()?;
    let db = ctl.x.db;
    let a = ctl.audio.as_mut()?;
    let slot = a.slot;
    let r = a.fb.as_mut()?;
    let target = (target as u64).min(r.usable() as u64 - 2);
    let mut k = 0;
    while r.queued() < target {
        r.push_td(r.mps.min(r.slot_bytes));
        k += 1;
    }
    if k > 0 || r.queued() > 0 {
        dsb();
        unsafe { write32(db + slot as usize * 4, r.dci) };
    }
    Some((r.fb_raw, r.fb_len, r.fb_seq))
}

/// A ring with TDs queued but no completion for `stall_ms`: the controller
/// and our bookkeeping disagree (a lost event, a stopped endpoint). Stop the
/// endpoint, point its dequeue at our enqueue and start counting afresh.
/// Returns true if a resync was done.
pub fn audio_watchdog(stall_ms: u64) -> bool {
    let Some(ctl) = audio_ctl() else { return false };
    let now = now_ms();
    let Some(a) = ctl.audio.as_ref() else { return false };
    let slot = a.slot;
    let mut stuck = [None, None];
    for (i, r) in [a.out, a.fb].iter().enumerate() {
        if let Some(r) = r {
            if r.queued() > 0 && now.saturating_sub(r.last_event_ms) > stall_ms {
                stuck[i] = Some(r.dci);
            }
        }
    }
    let mut did = false;
    for (i, dci) in stuck.iter().enumerate() {
        let Some(dci) = *dci else { continue };
        let state = ep_state(ctl, slot, dci);
        // Stage 1: ring the doorbell and give it another period.
        let db = ctl.x.db;
        if let Some(a) = ctl.audio.as_mut() {
            if let Some(r) = (if i == 0 { a.out.as_mut() } else { a.fb.as_mut() }) {
                if !r.kicked {
                    r.kicked = true;
                    r.last_event_ms = now;
                    dsb();
                    unsafe { write32(db + slot as usize * 4, dci) };
                    continue;
                }
            }
        }
        // Stage 2: stop the endpoint and move its dequeue to our enqueue.
        out(ctl.name);
        dec(b": usbaudio watchdog: no completions on dci ", dci);
        dec(b" (endpoint state ", state);
        out(b") -> resync\n");
        let sl = (slot as u32) << 24;
        if state == 1 {
            let c = ctl.x.ring_command(0, (TRB_STOP_ENDPOINT << 10) | (dci << 16) | sl);
            if ctl.wait(Want::Cmd(c), 5_000).is_none() {
                out(b"  stop endpoint: no completion\n");
            }
        }
        let Some(a) = ctl.audio.as_ref() else { return did };
        let Some(r) = (if i == 0 { a.out } else { a.fb }) else { continue };
        let deq = dma(r.ring + (r.enq * 16) as u64) | r.cycle as u64;
        let c = ctl.x.ring_command(deq, (TRB_SET_TR_DEQUEUE << 10) | (dci << 16) | sl);
        match ctl.wait(Want::Cmd(c), 5_000) {
            Some((_, s, _)) if (s >> 24) & 0xFF == 1 => {},
            Some((_, s, _)) => {
                out(b"  set dequeue pointer: ");
                report_cc((s >> 24) & 0xFF);
            },
            None => out(b"  set dequeue pointer: no completion\n"),
        }
        let Some(a) = ctl.audio.as_mut() else { return did };
        if let Some(r) = (if i == 0 { a.out.as_mut() } else { a.fb.as_mut() }) {
            r.done = r.sent;
            r.since_ioc = 0;
            r.resyncs += 1;
            r.kicked = false;
            r.last_event_ms = now_ms();
        }
        dsb();
        unsafe { write32(ctl.x.db + slot as usize * 4, dci) };
        did = true;
    }
    did
}

const HID_SLOTS: u32 = 8;
const HID_SLOT_BYTES: u64 = 64;

/// Queue report requests until HID_SLOTS are in flight.
fn hid_arm_all(x: &Xhci, h: &mut Hid) {
    while h.fill.wrapping_sub(h.done) < HID_SLOTS {
        hid_queue(x, h);
    }
}

/// Queue one report request into the next slot of `rbuf`.
fn hid_queue(x: &Xhci, h: &mut Hid) {
    let buf = h.rbuf + (h.fill % HID_SLOTS) as u64 * HID_SLOT_BYTES;
    h.fill = h.fill.wrapping_add(1);
    let len = h.mps.min(HID_SLOT_BYTES as u32);
    trb_write(h.ring, h.enq, dma(buf), len, (TRB_NORMAL << 10) | (1 << 5) | h.cycle);
    h.enq += 1;
    if h.enq == RING_TRBS - 1 {
        trb_write(h.ring, h.enq, dma(h.ring), 0, (TRB_LINK << 10) | (1 << 1) | h.cycle);
        h.enq = 0;
        h.cycle ^= 1;
    }
    dsb();
    unsafe { write32(x.db + h.slot as usize * 4, h.dci) };
}

/// The scheduler's hook: deliver whatever the keyboard / mouse reported
/// since the last frame (each HID endpoint has one request in flight, so a
/// key tapped between two slow frames would otherwise be missed).
pub fn service() {
    poll();
}

/// Service every controller: deliver pending reports, recover halted
/// endpoints, and handle port changes (plus a 1 s scan as a safety net for a
/// missed event). Cheap when nothing happened.
fn poll() {
    unsafe {
        if IN_POLL {
            return; // console output during a hot-plug must not re-enter
        }
        IN_POLL = true;
    }
    let ctls = unsafe { &mut *(&raw mut CTLS) };
    let now = now_ms();
    let mut reinit = [false; MAX_CTLS];
    for (idx, slot) in ctls.iter_mut().enumerate() {
        let Some(ctl) = slot.as_mut() else { continue };
        ctl.drain();
        for i in 0..MAX_HID {
            if ctl.hids[i].is_some_and(|h| h.halted) {
                ctl.recover(i);
            }
        }
        let scan = now.saturating_sub(ctl.last_scan_ms) >= 1000;
        if scan {
            ctl.last_scan_ms = now;
            reinit[idx] = ctl.watchdog(now);
        }
        if ctl.port_pending != 0 || scan {
            ctl.service_ports(scan);
        }
    }
    for (idx, r) in reinit.iter().enumerate() {
        if *r {
            reinit_controller(idx);
        }
    }
    let (attach, detach) = unsafe { (core::mem::replace(&mut *(&raw mut MSD_ATTACH), false), core::mem::replace(&mut *(&raw mut MSD_DETACH), false)) };
    let (a_attach, a_detach) = unsafe { (core::mem::replace(&mut *(&raw mut AUDIO_ATTACH), false), core::mem::replace(&mut *(&raw mut AUDIO_DETACH), false)) };
    typematic(now);
    unsafe { IN_POLL = false };
    if detach {
        crate::drivers::usbmsd::detach();
    }
    if attach {
        crate::drivers::usbmsd::attach();
    }
    if a_detach {
        crate::drivers::usbaudio::detach();
    }
    if a_attach {
        crate::drivers::usbaudio::attach();
    }
}

/// Boot mouse report: buttons, dx, dy (signed), then wheel if the endpoint
/// sends a fourth byte. Moves the same pointer the PS/2 mouse drives.
fn mouse_report(r: &[u8; 8], n: usize) {
    let dx = r[1] as i8 as i32;
    let dy = r[2] as i8 as i32;
    crate::drivers::mouse::usb_report(dx, dy, r[0] & 0x07);
    if n >= 4 {
        unsafe { MOUSE_WHEEL += r[3] as i8 as i32 };
    }
}

/// Pointer state after draining pending reports: (x, y, buttons bit0 L / bit1 R
/// / bit2 M). None until a USB mouse is plugged in.
pub fn mouse_state() -> Option<(i32, i32, u8)> {
    poll();
    if !mouse_ready() {
        return None;
    }
    use crate::drivers::mouse;
    Some((mouse::x(), mouse::y(), mouse::buttons()))
}

/// Wheel clicks since the last call (positive = away from the user).
pub fn mouse_take_wheel() -> i32 {
    poll();
    unsafe { core::mem::take(&mut *(&raw mut MOUSE_WHEEL)) }
}

/// Is the key with HID usage `usage` held right now? Modifier usages 0xE0..0xE7
/// (LCtrl, LShift, LAlt, LGUI, RCtrl, RShift, RAlt, RGUI) read the modifier byte.
pub fn key_held(usage: u8) -> bool {
    poll();
    let held = unsafe { HELD };
    if (0xE0..=0xE7).contains(&usage) {
        return held[0] & (1 << (usage - 0xE0)) != 0;
    }
    held[2..8].contains(&usage)
}

// ── Keyboard: HID usages -> PS/2 scancodes ───────────────────────────────────

/// HID usage -> PS/2 scancode-set-1 make code, `true` when it needs the 0xE0
/// prefix. Layout-neutral: `drivers::keyboard` (and `kbdlayout`) turn the
/// scancodes into characters exactly as for a PS/2 keyboard.
fn usage_scancode(u: u8) -> Option<(bool, u8)> {
    const LETTERS: [u8; 26] = [
        0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, // a..m
        0x31, 0x18, 0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C, // n..z
    ];
    // 0x28..=0x38: Enter Esc Backspace Tab Space - = [ ] \ #(non-US) ; ' ` , . /
    const PUNCT: [u8; 17] = [0x1C, 0x01, 0x0E, 0x0F, 0x39, 0x0C, 0x0D, 0x1A, 0x1B, 0x2B, 0x2B, 0x27, 0x28, 0x29, 0x33, 0x34, 0x35];
    // 0x59..=0x63: keypad 1..9, 0, .
    const KEYPAD: [u8; 11] = [0x4F, 0x50, 0x51, 0x4B, 0x4C, 0x4D, 0x47, 0x48, 0x49, 0x52, 0x53];
    let plain = |c: u8| Some((false, c));
    let ext = |c: u8| Some((true, c));
    match u {
        0x04..=0x1D => plain(LETTERS[(u - 0x04) as usize]),
        0x1E..=0x27 => plain(0x02 + (u - 0x1E)), // 1..9, 0
        0x28..=0x38 => plain(PUNCT[(u - 0x28) as usize]),
        0x39 => plain(0x3A),                     // Caps Lock
        0x3A..=0x43 => plain(0x3B + (u - 0x3A)), // F1..F10
        0x44 => plain(0x57),                     // F11
        0x45 => plain(0x58),                     // F12
        0x46 => ext(0x37),                       // Print Screen
        0x47 => plain(0x46),                     // Scroll Lock
        0x49 => ext(0x52),                       // Insert
        0x4A => ext(0x47),                       // Home
        0x4B => ext(0x49),                       // Page Up
        0x4C => ext(0x53),                       // Delete
        0x4D => ext(0x4F),                       // End
        0x4E => ext(0x51),                       // Page Down
        0x4F => ext(0x4D),                       // Right
        0x50 => ext(0x4B),                       // Left
        0x51 => ext(0x50),                       // Down
        0x52 => ext(0x48),                       // Up
        0x53 => plain(0x45),                     // Num Lock
        0x54 => ext(0x35),                       // keypad /
        0x55 => plain(0x37),                     // keypad *
        0x56 => plain(0x4A),                     // keypad -
        0x57 => plain(0x4E),                     // keypad +
        0x58 => ext(0x1C),                       // keypad Enter
        0x59..=0x63 => plain(KEYPAD[(u - 0x59) as usize]),
        0x64 => plain(0x56), // non-US \ |
        0x65 => ext(0x5D),   // Menu
        _ => None,
    }
}

/// The modifier byte as PS/2 keys: (bits of the report's modifier byte, extended,
/// make code). Left and right Ctrl are one key (keyboard.rs tracks only 0x1D),
/// and so are the two Shifts, so letting go of one while the other is still
/// down can't clear the modifier.
const MODS: [(u8, bool, u8); 6] = [
    (0x11, false, 0x1D), // Ctrl
    (0x22, false, 0x2A), // Shift
    (0x04, false, 0x38), // left Alt
    (0x40, true, 0x38),  // right Alt (AltGr)
    (0x08, true, 0x5B),  // left GUI: the Super key (Applications menu)
    (0x80, true, 0x5C),  // right GUI
];

/// Push one scancode (with its 0xE0 prefix) into keyboard.rs's ring. The IRQ1
/// handler produces into the same ring, so interrupts are held off for the
/// two bytes: they stay together and the ring's head is never raced.
fn emit(extended: bool, code: u8) {
    let on = crate::arch::cpu::irqs_enabled();
    unsafe { crate::arch::cpu::cli() };
    if extended {
        crate::drivers::keyboard::irq_scancode(0xE0);
    }
    crate::drivers::keyboard::irq_scancode(code);
    if on {
        unsafe { crate::arch::cpu::sti() };
    }
}

/// Typematic: like a PS/2 keyboard, the last key pressed repeats its make code
/// while held -- after REPEAT_DELAY_MS, every REPEAT_RATE_MS.
const REPEAT_DELAY_MS: u64 = 500;
const REPEAT_RATE_MS: u64 = 33;
static mut REPEAT_USAGE: u8 = 0;
static mut REPEAT_AT: u64 = 0;
/// Held modifiers are re-sent about once a second: keyboard.rs forgets
/// Ctrl / Shift after 2 s without a scancode (its focus-loss guard, which a
/// PS/2 keyboard's own typematic keeps fed), and a USB keyboard sends nothing
/// while a modifier is just held.
static mut MOD_KEEPALIVE_AT: u64 = 0;

/// Turn a boot keyboard report into scancodes: modifiers pressed first, then
/// releases and fresh presses of the six key slots, then modifiers released --
/// so a chord that arrives in one report (Ctrl+C) still reads as Ctrl, C.
///
/// `others` / `other_mods`: what this keyboard's other interfaces already hold.
/// Those keys were pressed through them, so they are neither pressed nor
/// released again from this report (no doubled characters).
fn decode_report(prev: &[u8; 8], cur: &[u8; 8], others: &[u8], other_mods: u8) {
    let now = now_ms();
    for &(bits, e, code) in MODS.iter() {
        if prev[0] & bits == 0 && cur[0] & bits != 0 && other_mods & bits == 0 {
            emit(e, code);
            unsafe { MOD_KEEPALIVE_AT = now + 1000 };
        }
    }
    for &u in &prev[2..8] {
        if u >= 4 && !cur[2..8].contains(&u) && !others.contains(&u) {
            if let Some((e, code)) = usage_scancode(u) {
                emit(e, code | 0x80);
            }
            if unsafe { REPEAT_USAGE } == u {
                unsafe { REPEAT_USAGE = 0 };
            }
        }
    }
    for &u in &cur[2..8] {
        if u < 4 || prev[2..8].contains(&u) || others.contains(&u) {
            continue; // 0 = none, 1..3 = error/rollover codes; or held via another interface
        }
        if let Some((e, code)) = usage_scancode(u) {
            emit(e, code);
            unsafe {
                REPEAT_USAGE = u;
                REPEAT_AT = now + REPEAT_DELAY_MS;
            }
        }
    }
    for &(bits, e, code) in MODS.iter() {
        if prev[0] & bits != 0 && cur[0] & bits == 0 && other_mods & bits == 0 {
            emit(e, code | 0x80);
        }
    }
}

/// A USB keyboard is delivering reports. The firmware's PS/2 emulation of it
/// (USB legacy support) must then not also type every key through port 0x60.
pub fn usb_keyboard_live() -> bool {
    unsafe { FIRST_REPORT & (1 << (HidKind::Kbd as u8)) != 0 }
}

/// Auto-repeat and the modifier keep-alive; run from every [`poll`].
fn typematic(now: u64) {
    unsafe {
        let held = HELD;
        let u = REPEAT_USAGE;
        if u != 0 {
            if !held[2..8].contains(&u) {
                REPEAT_USAGE = 0;
            } else if now >= REPEAT_AT {
                if let Some((e, code)) = usage_scancode(u) {
                    emit(e, code);
                }
                REPEAT_AT = now + REPEAT_RATE_MS;
            }
        }
        if held[0] != 0 && now >= MOD_KEEPALIVE_AT {
            for &(bits, e, code) in MODS.iter() {
                if held[0] & bits != 0 && (bits & 0x33) != 0 {
                    emit(e, code); // Ctrl / Shift only: a repeated Super would reopen the menu
                }
            }
            MOD_KEEPALIVE_AT = now + 1000;
        }
    }
}

/// The last keyboard report (modifiers, reserved, 6 usages), after draining
/// pending reports — all zero with no USB keyboard.
pub fn held_report() -> [u8; 8] {
    poll();
    unsafe { HELD }
}

/// Forget all keyboard state at a session boundary (called from
/// `keyboard::drain`): zero the held report, stop auto-repeat, and reset every
/// keyboard endpoint's edge-detection `prev` to a cleared report, so a key-down
/// whose key-up was lost can't stay "held". The next real report re-sends the
/// keys (and modifiers) that really are down.
pub fn reset_key_state() {
    unsafe {
        HELD = [0; 8];
        REPEAT_USAGE = 0;
        let ctls = &mut *(&raw mut CTLS);
        for ctl in ctls.iter_mut().flatten() {
            for h in ctl.hids.iter_mut().flatten() {
                if h.kind == HidKind::Kbd {
                    h.prev = [0; 8];
                }
            }
        }
    }
}

// ── PCI discovery, BIOS handoff, bring-up ────────────────────────────────────

const CTL_NAMES: [&[u8]; MAX_CTLS] = [b"xhci0", b"xhci1", b"xhci2", b"xhci3"];

/// Every PCI function with class `class`/`sub`/`prog_if`.
fn pci_find_all(class: u8, sub: u8, prog_if: u8) -> alloc::vec::Vec<crate::arch::pci::Location> {
    use crate::arch::pci::{read32, Location};
    let want = ((class as u32) << 24) | ((sub as u32) << 16) | ((prog_if as u32) << 8);
    let mut v = alloc::vec::Vec::new();
    for bus in 0..=255u8 {
        for device in 0..32u8 {
            if read32(bus, device, 0, 0) & 0xFFFF == 0xFFFF {
                continue;
            }
            let multi = (read32(bus, device, 0, 0x0C) >> 16) & 0x80 != 0;
            for function in 0..if multi { 8 } else { 1 } {
                if read32(bus, device, function, 0) & 0xFFFF == 0xFFFF {
                    continue;
                }
                if read32(bus, device, function, 0x08) & 0xFFFF_FF00 == want {
                    v.push(Location { bus, device, function });
                }
            }
        }
    }
    v
}

/// Take the controller from the firmware: USB Legacy Support (extended
/// capability 1). Set the OS-owned semaphore, wait (1 s) for the BIOS-owned one
/// to clear -- forcing it if the firmware never answers -- and switch off every
/// SMI the controller could raise to the firmware's SMM handler.
fn bios_handoff(cap: usize) {
    let hcc1 = unsafe { read32(cap + 0x10) };
    let mut off = ((hcc1 >> 16) & 0xFFFF) as usize * 4;
    let mut guard = 0;
    while off != 0 && guard < 64 {
        guard += 1;
        let at = cap + off;
        let v = unsafe { read32(at) };
        if v & 0xFF == 1 {
            const BIOS_OWNED: u32 = 1 << 16;
            const OS_OWNED: u32 = 1 << 24;
            if v & BIOS_OWNED != 0 {
                out(b"  usb legacy: firmware owns the controller, asking for it\n");
                unsafe { write32(at, v | OS_OWNED) };
                if wait_clear(at, BIOS_OWNED, 20_000) {
                    out(b"  usb legacy: handed over\n");
                } else {
                    out(b"  usb legacy: firmware did not let go in 1 s -- taking it\n");
                    let v2 = unsafe { read32(at) };
                    unsafe { write32(at, (v2 & !BIOS_OWNED) | OS_OWNED) };
                }
            } else {
                unsafe { write32(at, v | OS_OWNED) };
                out(b"  usb legacy: not firmware-owned\n");
            }
            // USBLEGCTLSTS: keep only the reserved bits (clears every SMI
            // enable), and write 1 to the three RW1C SMI event bits.
            let ctl = unsafe { read32(at + 4) };
            const KEEP: u32 = (0x7 << 1) | (0xFF << 5) | (0x7 << 17);
            unsafe { write32(at + 4, (ctl & KEEP) | (0x7 << 29)) };
            hex(b"  usb legacy: SMIs off, USBLEGCTLSTS was ", ctl as u64, 8);
            return;
        }
        let next = ((v >> 8) & 0xFF) as usize;
        if next == 0 {
            break;
        }
        off += next * 4;
    }
    out(b"  usb legacy: no USBLEGSUP capability (nothing to hand over)\n");
}

/// Intel PCHs from the 7 to the 9 series pair the xHCI with EHCI controllers
/// and boot with the USB 2 pins routed to EHCI (and SuperSpeed off): route
/// every switchable port to the xHCI, as Linux's `usb_enable_intel_xhci_ports`
/// does. Only done when an Intel EHCI is actually present -- newer PCHs (the
/// 500 series) have no EHCI and these config offsets mean something else.
fn intel_route_ports(loc: crate::arch::pci::Location) {
    use crate::arch::pci::{read32, write32};
    if loc.read32(0) & 0xFFFF != 0x8086 {
        return;
    }
    let ehci = pci_find_all(0x0C, 0x03, 0x20).iter().any(|e| e.read32(0) & 0xFFFF == 0x8086);
    if !ehci {
        return;
    }
    let (b, d, f) = (loc.bus, loc.device, loc.function);
    let ss = read32(b, d, f, 0xDC); // USB3PRM: which ports may switch
    write32(b, d, f, 0xD8, ss); // USB3_PSSEN
    let hs = read32(b, d, f, 0xD4); // XUSB2PRM
    write32(b, d, f, 0xD0, hs); // XUSB2PR
    hex(b"  intel pch: USB 2 ports routed from EHCI to xHCI, mask ", hs as u64, 4);
}

/// Find every xHCI controller on the PCI bus, take it from the firmware and
/// bring it up (devices plugged in now are enumerated; hot-plug after that is
/// handled by [`poll`]). Called once from `init()`.
pub fn init() {
    let locs = pci_find_all(0x0C, 0x03, 0x30);
    if locs.is_empty() {
        out(b"usb: no xHCI controller\n");
        return;
    }
    for (i, loc) in locs.iter().take(MAX_CTLS).enumerate() {
        let name = CTL_NAMES[i];
        let id = loc.read32(0);
        out(name);
        dec(b": PCI ", loc.bus as u32);
        dec(b":", loc.device as u32);
        dec(b".", loc.function as u32);
        hex(b" id ", id as u64, 8);
        // memory decoding + bus mastering on; INTx off (polled driver)
        let cmd = loc.read32(0x04) & 0xFFFF;
        crate::arch::pci::write32(loc.bus, loc.device, loc.function, 0x04, cmd | 0x2 | 0x4 | (1 << 10));
        let lo = loc.read32(0x10);
        if lo & 1 != 0 {
            out(b"  BAR0 is an I/O BAR -- not an xHCI register window, skipped\n");
            continue;
        }
        let mut base = (lo & 0xFFFF_FFF0) as u64;
        if (lo >> 1) & 3 == 2 {
            base |= (loc.read32(0x14) as u64) << 32;
        }
        hex(b"  BAR0 at ", base, 12);
        if base == 0 {
            out(b"  BAR0 unassigned by the firmware -- skipped\n");
            continue;
        }
        // 64 KiB covers every register block of the controllers we know
        // (Intel's BAR is 64 KiB; QEMU's 16 KiB sits below 4 GiB anyway).
        let Some(virt) = crate::arch::paging::map_mmio(base, 0x10000) else {
            out(b"  BAR0 above 512 GiB cannot be mapped -- controller skipped\n");
            continue;
        };
        let cap = virt as usize;
        bios_handoff(cap);
        intel_route_ports(*loc);
        enumerate(cap, name);
    }
}
