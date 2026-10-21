//! xHCI host-controller driver for the RP1 DWC3 controllers on the Pi 5:
//! enumerates USB keyboards and mice (boot protocol), keeps their interrupt
//! endpoints polled, recovers endpoints the controller halts (a wireless
//! keyboard whose dongle hiccups when it sleeps), and hot-plugs: devices can be
//! unplugged and plugged back in — or drop off the bus and return on their own
//! — and are set up again without a reboot.
//!
//! **DMA.** The controller lives on RP1 across PCIe, so every pointer we hand it
//! (ring bases, contexts, data buffers) must be the address *RP1* uses to reach
//! the Pi's RAM; on this hardware the firmware's inbound window is 1:1
//! (`DMA_OFFSET` = 0, confirmed: events land in our ring). The pages come from
//! `paging::dma_alloc_page`, the uncached pool, so CPU and controller stay
//! coherent without cache maintenance — only a DSB before ringing a doorbell.
//!
//! **Events.** One event ring per controller carries transfer completions for
//! every device, command completions and port changes. Everything funnels
//! through [`Ctl::dispatch`], so a wait for one command's completion still
//! delivers other devices' reports (nothing typed during a hot-plug is lost),
//! and port changes seen mid-operation are queued for [`poll`] rather than
//! handled re-entrantly.
//!
//! All waits are bounded and every step prints a status line, because this can
//! only be tested on real hardware (no serial adapter → debugging by HDMI photo).

use crate::arch::mmio::{read32, write32};
use crate::arch::timer::{delay_us, now_ms};

/// Offset added to a CPU-physical address to get the bus address RP1's DMA uses
/// to reach system RAM: 1:1 on this hardware (see the module doc).
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
const TRB_EV_TRANSFER: u32 = 32;
const TRB_EV_CMD_COMPLETE: u32 = 33;
const TRB_EV_PORT_STATUS: u32 = 34;

const RING_TRBS: usize = 64; // TRBs per ring segment (64 * 16 = 1 KiB, fits a page)

const MAX_DEVS: usize = 4; // devices per controller (it has 2-4 root ports)
const MAX_HID: usize = 8; // keyboard/mouse endpoints per controller (combo dongles have two)

#[inline]
fn dma(phys: u64) -> u64 {
    phys + DMA_OFFSET
}

#[inline]
fn dsb() {
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

fn out(s: &[u8]) {
    crate::console_write(s);
}

unsafe fn wr64(addr: usize, val: u64) {
    write32(addr, (val & 0xFFFF_FFFF) as u32);
    write32(addr + 4, (val >> 32) as u32);
}

/// Allocate one zeroed 4 KiB page from the uncached DMA pool; returns its
/// CPU-physical address (== virtual: identity map). Pages are frame-aligned,
/// satisfying every xHCI alignment.
fn alloc_page() -> Option<u64> {
    let p = crate::arch::paging::dma_alloc_page()?;
    for i in 0..512u64 {
        unsafe { core::ptr::write_volatile((p + i * 8) as *mut u64, 0) };
    }
    Some(p)
}

/// Return a page once the controller can no longer touch it (slot disabled).
fn free_page(p: u64) {
    crate::arch::paging::dma_free_page(p);
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
            max_ports: (hcsparams1 >> 24) & 0xFF,
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
            core::ptr::write_volatile((x.erst + 8) as *mut u32, RING_TRBS as u32);
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
        if self.event_deq >= RING_TRBS {
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
    /// Set by a transfer error: the controller halted the endpoint and it
    /// must be reset (done by [`poll`], outside the event loop).
    halted: bool,
    last_cc: u32,
    fails: u8,
}

/// One HID interrupt-IN endpoint found in a configuration descriptor.
#[derive(Clone, Copy)]
struct HidEp {
    iface: u32,
    proto: u32,
    addr: u32,
    mps: u32,
    interval: u32,
}

struct Ctl {
    x: Xhci,
    name: &'static [u8],
    devs: [Option<Dev>; MAX_DEVS],
    hids: [Option<Hid>; MAX_HID],
    /// Ports whose status-change events arrived; serviced by [`poll`].
    port_pending: u32,
    last_scan_ms: u64,
}

static mut CTLS: [Option<Ctl>; 2] = [None, None];

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
        let x = &self.x;
        let h = self.hids[i].as_mut().unwrap();
        let cc = (status >> 24) & 0xFF;
        if cc == 1 || cc == 13 {
            let mut cur = [0u8; 8];
            let n = (h.mps as usize).min(8);
            for (k, b) in cur.iter_mut().enumerate().take(n) {
                *b = unsafe { core::ptr::read_volatile((h.rbuf + k as u64) as *const u8) };
            }
            match h.kind {
                HidKind::Kbd => {
                    decode_report(&h.prev, &cur);
                    unsafe { HELD = cur };
                },
                HidKind::Mouse => mouse_report(&cur, n),
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
        out(if h.kind == HidKind::Kbd { b": keyboard endpoint halted (" } else { b": mouse endpoint halted (" });
        report_cc(h.last_cc);
        if h.fails > 6 {
            out(b"  giving up on it -- unplug and re-plug the device\n");
            if h.kind == HidKind::Kbd {
                unsafe { HELD = [0; 8] };
            }
            free_page(h.ring);
            free_page(h.rbuf);
            self.hids[i] = None;
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
        hid_queue(&self.x, &mut h);
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
        for h in self.hids.iter_mut() {
            if let Some(hh) = *h {
                if hh.slot == d.slot {
                    if hh.kind == HidKind::Kbd {
                        unsafe { HELD = [0; 8] };
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
            if connected && (!have || changed) {
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
                }
            }
        }
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
    *slot = Some(Ctl { x, name, devs: [None; MAX_DEVS], hids: [None; MAX_HID], port_pending: 0, last_scan_ms: now_ms() });
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
    let mut off = 0usize;
    while off + 2 <= total {
        let blen = rd(off) as usize;
        let btype = rd(off + 1);
        if blen == 0 {
            break;
        }
        if btype == 4 {
            let is_boot_hid = rd(off + 5) == 3 && matches!(rd(off + 7), 1 | 2);
            cur_iface = rd(off + 2);
            cur_proto = rd(off + 7);
            cur_taken = !is_boot_hid;
        } else if btype == 5 && !cur_taken && n < MAX_HID {
            let addr = rd(off + 2);
            if addr & 0x80 != 0 && rd(off + 3) & 0x3 == 3 {
                let mps = rd(off + 4) | (rd(off + 5) << 8);
                eps[n] = Some(HidEp { iface: cur_iface, proto: cur_proto, addr, mps, interval: rd(off + 6) });
                n += 1;
                cur_taken = true;
            }
        }
        off += blen;
    }
    if n == 0 {
        // Not a keyboard/mouse: leave it addressed but unused (a hub, a drive).
        out(b"  no boot keyboard/mouse interface (device left idle)\n");
        return;
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
        // SET_PROTOCOL(boot) — fixed 8-byte keyboard / 3-4 byte mouse reports —
        // and SET_IDLE(0) so reports only arrive on change. Best effort: some
        // devices STALL these and still send boot-format reports.
        let boot = ctl.control(di, 0x21, 0x0B, 0, e.iface as u16, 0, 0).is_some();
        let _ = ctl.control(di, 0x21, 0x0A, 0, e.iface as u16, 0, 0);
        let kind = if e.proto == 1 { HidKind::Kbd } else { HidKind::Mouse };
        out(if kind == HidKind::Kbd { b"  keyboard interface" } else { b"  mouse interface" });
        out(if boot { b" (boot protocol)\n" } else { b" (report protocol)\n" });
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
            halted: false,
            last_cc: 0,
            fails: 0,
        };
        hid_queue(&ctl.x, &mut h); // arm the first interrupt transfer
        ctl.hids[hi] = Some(h);
        out(if kind == HidKind::Kbd { b"  -> USB KEYBOARD READY\n" } else { b"  -> USB MOUSE READY\n" });
    }
}

// ── Polling: keyboards + mice, recovery, hot-plug ────────────────────────────

// Decoded-byte queue (a key press can yield several bytes, e.g. an arrow).
const KQ_LEN: usize = 64;
static mut KQ: [u8; KQ_LEN] = [0; KQ_LEN];
static mut KQ_HEAD: usize = 0;
static mut KQ_TAIL: usize = 0;

/// Keys currently held (the last keyboard report): modifiers + 6 usages.
static mut HELD: [u8; 8] = [0; 8];
/// Absolute pointer (in the framebuffer's safe-area coordinates), buttons,
/// accumulated wheel clicks; `MOUSE_INIT` centres it on first use.
static mut MOUSE: (i32, i32, u8) = (0, 0, 0);
static mut MOUSE_WHEEL: i32 = 0;
static mut MOUSE_INIT: bool = false;
static mut IN_POLL: bool = false;

fn kq_push(b: u8) {
    unsafe {
        let next = (KQ_HEAD + 1) % KQ_LEN;
        if next != KQ_TAIL {
            KQ[KQ_HEAD] = b;
            KQ_HEAD = next;
        }
    }
}

fn kq_pop() -> Option<u8> {
    unsafe {
        if KQ_HEAD == KQ_TAIL {
            return None;
        }
        let b = KQ[KQ_TAIL];
        KQ_TAIL = (KQ_TAIL + 1) % KQ_LEN;
        Some(b)
    }
}

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
fn hid_queue(x: &Xhci, h: &mut Hid) {
    trb_write(h.ring, h.enq, dma(h.rbuf), h.mps, (TRB_NORMAL << 10) | (1 << 5) | h.cycle);
    h.enq += 1;
    if h.enq == RING_TRBS - 1 {
        trb_write(h.ring, h.enq, dma(h.ring), 0, (TRB_LINK << 10) | (1 << 1) | h.cycle);
        h.enq = 0;
        h.cycle ^= 1;
    }
    dsb();
    unsafe { write32(x.db + h.slot as usize * 4, h.dci) };
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
    for ctl in ctls.iter_mut().flatten() {
        ctl.drain();
        for i in 0..MAX_HID {
            if ctl.hids[i].is_some_and(|h| h.halted) {
                ctl.recover(i);
            }
        }
        let scan = now.saturating_sub(ctl.last_scan_ms) >= 1000;
        if scan {
            ctl.last_scan_ms = now;
        }
        if ctl.port_pending != 0 || scan {
            ctl.service_ports(scan);
        }
    }
    unsafe { IN_POLL = false };
}

/// Boot mouse report: buttons, dx, dy (signed), then wheel if the endpoint
/// sends a fourth byte. Moves the absolute pointer, clamped to the screen.
fn mouse_report(r: &[u8; 8], n: usize) {
    let (w, h) = (
        crate::drivers::framebuffer::width().max(1) as i32,
        crate::drivers::framebuffer::height().max(1) as i32,
    );
    unsafe {
        if !MOUSE_INIT {
            MOUSE = (w / 2, h / 2, 0);
            MOUSE_INIT = true;
        }
        let dx = r[1] as i8 as i32;
        let dy = r[2] as i8 as i32;
        MOUSE.0 = (MOUSE.0 + dx).clamp(0, w - 1);
        MOUSE.1 = (MOUSE.1 + dy).clamp(0, h - 1);
        MOUSE.2 = r[0] & 0x07;
        if n >= 4 {
            MOUSE_WHEEL += r[3] as i8 as i32;
        }
    }
}

/// Pointer state after draining pending reports: (x, y, buttons bit0 L / bit1 R
/// / bit2 M). None until a USB mouse is plugged in.
pub fn mouse_state() -> Option<(i32, i32, u8)> {
    poll();
    if !mouse_ready() {
        return None;
    }
    unsafe {
        if !MOUSE_INIT {
            let (w, h) = (
                crate::drivers::framebuffer::width() as i32,
                crate::drivers::framebuffer::height() as i32,
            );
            MOUSE = (w / 2, h / 2, 0);
            MOUSE_INIT = true;
        }
        Some(MOUSE)
    }
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

/// US-layout HID usage → ASCII (None for keys with no single-byte meaning).
fn usage_to_ascii(u: u8, shift: bool) -> Option<u8> {
    const DIGITS: &[u8; 10] = b"1234567890";
    const SHIFTED: &[u8; 10] = b"!@#$%^&*()";
    let pick = |plain: u8, shifted: u8| if shift { shifted } else { plain };
    Some(match u {
        0x04..=0x1d => {
            let c = b'a' + (u - 0x04);
            if shift {
                c - 32
            } else {
                c
            }
        },
        0x1e..=0x27 => {
            let i = (u - 0x1e) as usize;
            if shift {
                SHIFTED[i]
            } else {
                DIGITS[i]
            }
        },
        0x28 => b'\r',
        0x29 => 0x1b,
        0x2a => 0x08,
        0x2b => b'\t',
        0x2c => b' ',
        0x2d => pick(b'-', b'_'),
        0x2e => pick(b'=', b'+'),
        0x2f => pick(b'[', b'{'),
        0x30 => pick(b']', b'}'),
        0x31 => pick(b'\\', b'|'),
        0x33 => pick(b';', b':'),
        0x34 => pick(b'\'', b'"'),
        0x35 => pick(b'`', b'~'),
        0x36 => pick(b',', b'<'),
        0x37 => pick(b'.', b'>'),
        0x38 => pick(b'/', b'?'),
        _ => return None,
    })
}

/// Turn a boot keyboard report into bytes: every key code present now but not in
/// the previous report is a fresh press.
fn decode_report(prev: &[u8; 8], cur: &[u8; 8]) {
    let shift = cur[0] & 0x22 != 0; // LShift | RShift
    let ctrl = cur[0] & 0x11 != 0; // LCtrl | RCtrl
    for &u in &cur[2..8] {
        if u < 4 || prev[2..8].contains(&u) {
            continue; // 0 = none, 1..3 = error/rollover codes
        }
        let seq: &[u8] = match u {
            0x4f => b"\x1b[C", // Right
            0x50 => b"\x1b[D", // Left
            0x51 => b"\x1b[B", // Down
            0x52 => b"\x1b[A", // Up
            _ => {
                if let Some(a) = usage_to_ascii(u, shift) {
                    // Ctrl+letter → its control code (Ctrl+Q = 0x11), like a terminal.
                    kq_push(if ctrl && a.is_ascii_alphabetic() { a & 0x1F } else { a });
                }
                continue;
            },
        };
        for &b in seq {
            kq_push(b);
        }
    }
}

/// Non-blocking: the next byte typed on any USB keyboard, if any.
pub fn kbd_poll_byte() -> Option<u8> {
    if let Some(b) = kq_pop() {
        return Some(b);
    }
    poll();
    kq_pop()
}

/// The last keyboard report (modifiers, reserved, 6 usages), after draining
/// pending reports — all zero with no USB keyboard.
pub fn held_report() -> [u8; 8] {
    poll();
    unsafe { HELD }
}
