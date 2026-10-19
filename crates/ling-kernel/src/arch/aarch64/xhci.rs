//! Minimal xHCI host-controller driver for the RP1 DWC3 controllers on the Pi 5
//! — enough to enumerate one HID device (the mouse/keyboard) and, later, read
//! its boot reports.
//!
//! **DMA.** The controller lives on RP1 across PCIe, so every pointer we hand it
//! (ring bases, contexts, data buffers) must be the address *RP1* uses to reach
//! the Pi's RAM: the firmware maps system RAM into the PCIe inbound window at
//! `0x10_0000_0000`, so `dma = cpu_phys + DMA_OFFSET` (confirmed against the
//! smallkirby bare-metal RP1 Ethernet driver). The MMU is off, so our DMA pages
//! are uncached and therefore coherent with the controller — we only need a DSB
//! barrier before ringing a doorbell so our writes land before it reads them.
//!
//! All waits are bounded and every step prints a status line, because this can
//! only be tested on real hardware (no serial adapter → debugging by HDMI photo).

use crate::arch::mmio::{read32, write32};
use crate::mm::frame;

/// Offset added to a CPU-physical address to get the bus address RP1's DMA uses
/// to reach system RAM (the PCIe inbound window). smallkirby's board needed
/// +0x10_0000_0000, but on this hardware that made the controller post events
/// (EINT set) that never landed in our ring — so the firmware's inbound window
/// is 1:1 here. Testing 0; if events now appear at the ring, this is correct.
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
const PORTSC_PED: u32 = 1 << 1;
const PORTSC_PR: u32 = 1 << 4;
const PORTSC_PP: u32 = 1 << 9;
// Write-1-to-clear change bits live in PORTSC; preserve non-RW1C bits when writing.
const PORTSC_RW1CS: u32 = 0x00FE_0000; // CSC,PEC,WRC,OCC,PRC,PLC,CEC (bits 17..23)

// TRB types.
const TRB_NORMAL: u32 = 1;
const TRB_SETUP: u32 = 2;
const TRB_DATA: u32 = 3;
const TRB_STATUS: u32 = 4;
const TRB_ENABLE_SLOT: u32 = 9;
const TRB_ADDRESS_DEVICE: u32 = 11;
const TRB_CONFIGURE_ENDPOINT: u32 = 12;
const TRB_EVALUATE_CONTEXT: u32 = 13;
const TRB_EV_TRANSFER: u32 = 32;
const TRB_EV_CMD_COMPLETE: u32 = 33;

const RING_TRBS: usize = 64; // TRBs per ring segment (64 * 16 = 1 KiB, fits a page)

#[inline]
fn dma(phys: u64) -> u64 {
    phys + DMA_OFFSET
}

#[inline]
fn dsb() {
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

unsafe fn wr64(addr: usize, val: u64) {
    write32(addr, (val & 0xFFFF_FFFF) as u32);
    write32(addr + 4, (val >> 32) as u32);
}

/// Allocate one zeroed 4 KiB page; returns its CPU-physical address (== virtual,
/// MMU off). Pages are frame-aligned, satisfying every xHCI alignment.
fn alloc_page() -> Option<u64> {
    let p = frame::alloc_frames(0)?;
    for i in 0..512u64 {
        unsafe { core::ptr::write_volatile((p + i * 8) as *mut u64, 0) };
    }
    Some(p)
}

fn trb_write(ring: u64, idx: usize, param: u64, status: u32, control: u32) {
    let a = ring + (idx * 16) as u64;
    unsafe {
        core::ptr::write_volatile(a as *mut u64, param);
        core::ptr::write_volatile((a + 8) as *mut u32, status);
        core::ptr::write_volatile((a + 12) as *mut u32, control);
    }
}

struct Xhci {
    cap: usize,
    op: usize,
    run: usize,
    db: usize,
    ctx_size: usize, // 32 or 64
    dcbaa: u64,
    cmd_ring: u64,
    cmd_enq: usize,
    cmd_cycle: u32,
    event_ring: u64,
    event_deq: usize,
    event_cycle: u32,
    erst: u64,
    // Set once a device is addressed:
    slot: u8,
    ep0_ring: u64,
    ep0_enq: usize,
}

impl Xhci {
    fn op_rd(&self, off: usize) -> u32 {
        unsafe { read32(self.op + off) }
    }
    fn op_wr(&self, off: usize, v: u32) {
        unsafe { write32(self.op + off, v) };
    }

    /// Reset the controller and build the command ring, event ring and DCBAA,
    /// then set it running. Returns None on any failure (after printing why).
    fn init(cap: usize) -> Option<Xhci> {
        let caplength = (unsafe { read32(cap) } & 0xFF) as usize;
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
            dcbaa: 0,
            cmd_ring: 0,
            cmd_enq: 0,
            cmd_cycle: 1,
            event_ring: 0,
            event_deq: 0,
            event_cycle: 1,
            erst: 0,
            slot: 0,
            ep0_ring: 0,
            ep0_enq: 0,
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
            crate::console_write(b"  xhci: controller reset timeout\n");
            return None;
        }

        // DCBAA, command ring, event ring + ERST.
        x.dcbaa = alloc_page()?;
        x.cmd_ring = alloc_page()?;
        x.event_ring = alloc_page()?;
        x.erst = alloc_page()?;

        // Scratchpad buffers. If HCSPARAMS2 asks for any, DCBAA[0] MUST point to
        // an array of page pointers the controller uses for internal state —
        // without it the controller silently never completes a command (exactly
        // our symptom: command ring not running, no events). HCSPARAMS2:
        // Max Scratchpad Bufs = (bits[31:27] << 5) | bits[25:21].
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

        let max_slots = unsafe { read32(cap + 0x04) } & 0xFF;
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
        hex(b"  rtsoff=", rtsoff as u64, 8);
        hex(b"  dboff=", dboff as u64, 8);
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
            crate::console_write(b"  xhci: controller did not start (still halted)\n");
            return None;
        }
        crate::console_write(b"  xhci: running (rings up, DCBAA set)\n");
        Some(x)
    }

    /// Ring the command doorbell for the TRB just enqueued, advancing the ring.
    fn ring_command(&mut self, param: u64, control: u32) {
        trb_write(self.cmd_ring, self.cmd_enq, param, 0, control | self.cmd_cycle);
        self.cmd_enq += 1;
        // Leave room for the implicit wrap; a Link TRB would be needed near the
        // end, but enumeration issues far fewer than RING_TRBS-1 commands.
        dsb();
        unsafe { write32(self.db, 0) }; // doorbell 0 = command ring
    }

    /// Drain the event ring until an event of type `want` arrives, returning
    /// (param, status, control), or None on timeout. Events of other types
    /// (notably Port Status Change, which the controller posts as ports come up)
    /// are consumed and skipped — otherwise a stale PSC event is mistaken for the
    /// command/transfer completion we're actually waiting on.
    fn wait_for(&mut self, want: u32, tries: u32) -> Option<(u64, u32, u32)> {
        let mut spins = 0u32;
        while spins < tries {
            let a = self.event_ring + (self.event_deq * 16) as u64;
            let control = unsafe { core::ptr::read_volatile((a + 12) as *const u32) };
            if (control & 1) == self.event_cycle {
                let param = unsafe { core::ptr::read_volatile(a as *const u64) };
                let status = unsafe { core::ptr::read_volatile((a + 8) as *const u32) };
                self.event_deq += 1;
                if self.event_deq >= RING_TRBS {
                    self.event_deq = 0;
                    self.event_cycle ^= 1;
                }
                let erdp = dma(self.event_ring + (self.event_deq * 16) as u64) | (1 << 3);
                unsafe { wr64(self.run + 0x38, erdp) };
                if ((control >> 10) & 0x3F) == want {
                    return Some((param, status, control));
                }
                continue; // consumed another event type; keep looking
            }
            crate::arch::timer::delay_us(50);
            spins += 1;
        }
        None
    }

    fn enable_slot(&mut self) -> Option<u8> {
        self.ring_command(0, TRB_ENABLE_SLOT << 10);
        match self.wait_for(TRB_EV_CMD_COMPLETE, 40_000) {
            Some((_p, status, control)) => {
                let cc = (status >> 24) & 0xFF;
                if cc != 1 {
                    crate::console_write(b"  enable_slot: ");
                    report_cc(cc);
                    return None;
                }
                Some(((control >> 24) & 0xFF) as u8)
            },
            None => {
                crate::console_write(b"  enable_slot: NO completion event (timeout)\n");
                self.dump_state(b"  at timeout");
                None
            },
        }
    }

    /// A control transfer on EP0 (Setup/Data/Status). For an IN request the data
    /// lands in `buf_dma`. Returns bytes transferred, or None on failure. The EP0
    /// ring doesn't wrap during enumeration (a handful of TRBs), so cycle stays 1.
    fn control(
        &mut self,
        bm_rt: u8,
        b_req: u8,
        w_val: u16,
        w_idx: u16,
        buf_dma: u64,
        len: u16,
    ) -> Option<u32> {
        let c = 1u32;
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
        trb_write(self.ep0_ring, self.ep0_enq, setup, 8, (TRB_SETUP << 10) | (trt << 16) | (1 << 6) | c);
        self.ep0_enq += 1;
        if len > 0 {
            let dir = if is_in { 1u32 } else { 0 };
            trb_write(self.ep0_ring, self.ep0_enq, buf_dma, len as u32, (TRB_DATA << 10) | (dir << 16) | c);
            self.ep0_enq += 1;
        }
        let status_dir = if len > 0 && is_in { 0u32 } else { 1 };
        trb_write(self.ep0_ring, self.ep0_enq, 0, 0, (TRB_STATUS << 10) | (status_dir << 16) | (1 << 5) | c);
        self.ep0_enq += 1;
        dsb();
        unsafe { write32(self.db + self.slot as usize * 4, 1) }; // EP0 doorbell, DCI 1
        let (_p, status, _ctl) = self.wait_for(TRB_EV_TRANSFER, 40_000)?;
        let code = (status >> 24) & 0xFF;
        if code != 1 && code != 13 {
            crate::console_write(b"    control xfer ");
            report_cc(code);
            return None;
        }
        Some(len as u32 - (status & 0xFF_FFFF))
    }

    /// Update EP0's Max Packet Size in the device context via an Evaluate
    /// Context command. Needed because Address Device sets a provisional MPS of 8
    /// (safe for the first 8-byte descriptor read); the real value comes from
    /// bMaxPacketSize0 and must be programmed before a larger control transfer,
    /// or the device babbles. `input_ctx` is reused (only the EP0 add-flag set).
    fn set_ep0_mps(&mut self, input_ctx: u64, mps: u32) -> bool {
        let cs = self.ctx_size as u64;
        unsafe {
            core::ptr::write_volatile(input_ctx as *mut u32, 0); // drop flags
            core::ptr::write_volatile((input_ctx + 0x04) as *mut u32, 0b10); // add A1 = EP0 only
        }
        let ep0 = input_ctx + 2 * cs;
        unsafe {
            core::ptr::write_volatile((ep0 + 4) as *mut u32, (mps << 16) | (4 << 3) | (3 << 1));
            core::ptr::write_volatile((ep0 + 8) as *mut u64, dma(self.ep0_ring) | 1);
            core::ptr::write_volatile((ep0 + 16) as *mut u32, mps.min(8));
        }
        dsb();
        self.ring_command(dma(input_ctx), (TRB_EVALUATE_CONTEXT << 10) | ((self.slot as u32) << 24));
        match self.wait_for(TRB_EV_CMD_COMPLETE, 40_000) {
            Some((_p, status, _c)) => (status >> 24) & 0xFF == 1,
            None => false,
        }
    }

    /// Dump controller state for diagnosis: USBSTS (decoded), CRCR, and the raw
    /// first event-ring TRB — tells us whether the controller faulted on DMA
    /// (HSE), wrote an event we missed (EINT), or did nothing at all.
    fn dump_state(&self, tag: &[u8]) {
        let sts = self.op_rd(OP_USBSTS);
        crate::console_write(tag);
        hex(b" USBSTS=", sts as u64, 8);
        if sts & (1 << 2) != 0 {
            crate::console_write(b"    HSE set: host system error -> bad DMA address\n");
        }
        if sts & (1 << 3) != 0 {
            crate::console_write(b"    EINT set: an event WAS written (poll/cycle bug)\n");
        }
        if sts & STS_HCH != 0 {
            crate::console_write(b"    HCH set: controller halted\n");
        }
        let crcr = unsafe { read32(self.op + OP_CRCR) };
        hex(b"    CRCR.lo=", crcr as u64, 8);
        // Interrupter 0 readback — confirms the runtime offset + that the event
        // ring registers actually stuck.
        hex(b"    USBCMD=", self.op_rd(OP_USBCMD) as u64, 8);
        hex(b"    IMAN=", unsafe { read32(self.run + 0x20) } as u64, 8);
        hex(b"    ERSTSZ=", unsafe { read32(self.run + 0x28) } as u64, 8);
        hex(b"    ERSTBA.lo=", unsafe { read32(self.run + 0x30) } as u64, 8);
        hex(b"    ERDP.lo=", unsafe { read32(self.run + 0x38) } as u64, 8);
        let a = self.event_ring;
        let p = unsafe { core::ptr::read_volatile(a as *const u64) };
        let s = unsafe { core::ptr::read_volatile((a + 8) as *const u32) };
        let c = unsafe { core::ptr::read_volatile((a + 12) as *const u32) };
        hex(b"    evt[0].param=", p, 16);
        hex(b"    evt[0].status=", s as u64, 8);
        hex(b"    evt[0].control=", c as u64, 8);
    }
}

fn wait_clear(addr: usize, mask: u32, tries: u32) -> bool {
    for _ in 0..tries {
        if unsafe { read32(addr) } & mask == 0 {
            return true;
        }
        crate::arch::timer::delay_us(50);
    }
    false
}

fn wait_set(addr: usize, mask: u32, tries: u32) -> bool {
    for _ in 0..tries {
        if unsafe { read32(addr) } & mask != 0 {
            return true;
        }
        crate::arch::timer::delay_us(50);
    }
    false
}

fn hex(label: &[u8], val: u64, nibbles: usize) {
    crate::console_write(label);
    crate::console_write(b"0x");
    let mut buf = [0u8; 16];
    let n = nibbles.min(16);
    for i in 0..n {
        let nib = (val >> ((n - 1 - i) * 4)) & 0xF;
        buf[i] = if nib < 10 { b'0' + nib as u8 } else { b'a' + (nib - 10) as u8 };
    }
    crate::console_write(&buf[..n]);
    crate::console_write(b"\n");
}

fn print_hexbyte(b: u32) {
    let c = |n: u32| -> u8 {
        if n < 10 {
            b'0' + n as u8
        } else {
            b'a' + (n - 10) as u8
        }
    };
    crate::console_write(&[c((b >> 4) & 0xF), c(b & 0xF), b' ']);
}

fn report_cc(cc: u32) {
    match cc {
        1 => crate::console_write(b"success\n"),
        4 => crate::console_write(b"USB transaction error\n"),
        5 => crate::console_write(b"TRB error\n"),
        6 => crate::console_write(b"stall error\n"),
        _ => hex(b"completion code=", cc as u64, 2),
    }
}

/// Find the first root-hub port with a device connected; reset it and return
/// (port_number, psc) once enabled, or None.
fn reset_connected_port(x: &Xhci) -> Option<(u32, u32)> {
    let max_ports = (unsafe { read32(x.cap + 0x04) } >> 24) & 0xFF;
    let mut p = 1u32;
    while p <= max_ports {
        let off = x.op + OP_PORTSC0 + (p as usize - 1) * OP_PORT_STRIDE;
        let mut psc = unsafe { read32(off) };
        if psc & PORTSC_PP == 0 {
            unsafe { write32(off, (psc & !PORTSC_RW1CS) | PORTSC_PP) };
            crate::arch::timer::delay_us(20_000);
            psc = unsafe { read32(off) };
        }
        if psc & PORTSC_CCS != 0 {
            // Reset the port (USB2): write PR, keep PP, clear RW1C change bits.
            let base = (psc & !PORTSC_RW1CS) | PORTSC_PP;
            unsafe { write32(off, base | PORTSC_PR) };
            if !wait_clear(off, PORTSC_PR, 20_000) {
                crate::console_write(b"  port reset timeout\n");
                return None;
            }
            crate::arch::timer::delay_us(20_000);
            let psc2 = unsafe { read32(off) };
            crate::console_write(b"  port ");
            crate::console_write(&[b'0' + p as u8]);
            hex(b" after reset PORTSC=", psc2 as u64, 8);
            if psc2 & PORTSC_PED == 0 {
                crate::console_write(b"  port did not enable after reset\n");
                return None;
            }
            return Some((p, psc2));
        }
        p += 1;
    }
    crate::console_write(b"  no connected port found\n");
    None
}

/// Quick check (no full init) for any powered port with a device connected — so
/// usb_probe can skip the verbose enumerate on an empty controller and keep the
/// populated one's output on screen.
fn has_device(cap: usize) -> bool {
    let caplength = (unsafe { read32(cap) } & 0xFF) as usize;
    let op = cap + caplength;
    let max_ports = (unsafe { read32(cap + 0x04) } >> 24) & 0xFF;
    let mut p = 1u32;
    while p <= max_ports {
        let off = op + OP_PORTSC0 + (p as usize - 1) * OP_PORT_STRIDE;
        let mut psc = unsafe { read32(off) };
        if psc & PORTSC_PP == 0 {
            unsafe { write32(off, (psc & !PORTSC_RW1CS) | PORTSC_PP) };
            crate::arch::timer::delay_us(20_000);
            psc = unsafe { read32(off) };
        }
        if psc & PORTSC_CCS != 0 {
            return true;
        }
        p += 1;
    }
    false
}

/// Enumerate a single device: reset its port, enable a slot, address it, and
/// read its device descriptor. Prints progress at every step.
pub fn enumerate(cap_base: usize, name: &[u8]) {
    if !has_device(cap_base) {
        crate::console_write(name);
        crate::console_write(b": no device connected\n");
        return;
    }
    crate::console_write(name);
    crate::console_write(b" xHCI enumerate:\n");

    let Some(mut x) = Xhci::init(cap_base) else { return };

    let Some((port, psc)) = reset_connected_port(&x) else { return };
    let speed = (psc >> 10) & 0xF;

    let Some(slot) = x.enable_slot() else { return };
    crate::console_write(b"  slot assigned: ");
    crate::console_write(&[b'0' + (slot % 10)]);
    crate::console_write(b"\n");
    x.slot = slot;

    // --- Address Device -----------------------------------------------------
    let Some(input_ctx) = alloc_page() else { return };
    let Some(dev_ctx) = alloc_page() else { return };
    let Some(ep0_ring) = alloc_page() else { return };
    x.ep0_ring = ep0_ring;
    x.ep0_enq = 0;
    let cs = x.ctx_size as u64;

    // Input Control Context: add slot (A0) + EP0 (A1).
    unsafe { core::ptr::write_volatile((input_ctx + 0x04) as *mut u32, 0b11) };
    // Slot context (1 * ctx_size): speed, 1 context entry, root-hub port.
    let slot_ctx = input_ctx + cs;
    unsafe {
        core::ptr::write_volatile(slot_ctx as *mut u32, (1u32 << 27) | (speed << 20));
        core::ptr::write_volatile((slot_ctx + 4) as *mut u32, port << 16);
    }
    // EP0 context (2 * ctx_size): Control, MPS 8 (full-speed default), CErr 3.
    let ep0 = input_ctx + 2 * cs;
    unsafe {
        core::ptr::write_volatile((ep0 + 4) as *mut u32, (8u32 << 16) | (4 << 3) | (3 << 1));
        core::ptr::write_volatile((ep0 + 8) as *mut u64, dma(ep0_ring) | 1); // TR deq | DCS
        core::ptr::write_volatile((ep0 + 16) as *mut u32, 8); // avg TRB length
    }
    // Register the output device context in the DCBAA.
    unsafe { core::ptr::write_volatile((x.dcbaa + slot as u64 * 8) as *mut u64, dma(dev_ctx)) };
    dsb();

    x.ring_command(dma(input_ctx), (TRB_ADDRESS_DEVICE << 10) | ((slot as u32) << 24));
    let Some((_p, status, _control)) = x.wait_for(TRB_EV_CMD_COMPLETE, 40_000) else {
        crate::console_write(b"  address_device: no completion event\n");
        return;
    };
    if (status >> 24) & 0xFF != 1 {
        crate::console_write(b"  address_device: ");
        report_cc((status >> 24) & 0xFF);
        return;
    }
    crate::console_write(b"  device addressed\n");

    // --- GET_DESCRIPTOR(device): 8 bytes first to learn bMaxPacketSize0 ------
    let Some(buf) = alloc_page() else { return };
    let rd = |o: usize| -> u32 {
        (unsafe { core::ptr::read_volatile((buf + o as u64) as *const u8) }) as u32
    };
    if x.control(0x80, 6, 0x0100, 0, dma(buf), 8).is_none() {
        crate::console_write(b"  get device descriptor (8) failed\n");
        return;
    }
    // bMaxPacketSize0 is byte 7. If the control endpoint is bigger than the
    // provisional 8, re-program it (Evaluate Context) before reading more, or
    // the device babbles.
    let mps0 = rd(7);
    hex(b"  bMaxPacketSize0=", mps0 as u64, 2);
    if (mps0 == 16 || mps0 == 32 || mps0 == 64) && !x.set_ep0_mps(input_ctx, mps0) {
        crate::console_write(b"  set EP0 max-packet failed\n");
        return;
    }
    // Now read the full 18-byte descriptor at the correct max-packet size.
    if x.control(0x80, 6, 0x0100, 0, dma(buf), 18).is_none() {
        crate::console_write(b"  get device descriptor (18) failed\n");
        return;
    }
    crate::console_write(b"  device descriptor:\n");
    hex(b"    idVendor=", (rd(8) | (rd(9) << 8)) as u64, 4);
    hex(b"    idProduct=", (rd(10) | (rd(11) << 8)) as u64, 4);

    // --- GET_DESCRIPTOR(config): 9-byte header, then the full thing ---------
    if x.control(0x80, 6, 0x0200, 0, dma(buf), 9).is_none() {
        crate::console_write(b"  get config header failed\n");
        return;
    }
    let total = (rd(2) | (rd(3) << 8)).min(4096) as u16;
    let cfg_val = rd(5); // bConfigurationValue (from the 9-byte header)
    if x.control(0x80, 6, 0x0200, 0, dma(buf), total).is_none() {
        crate::console_write(b"  get full config failed\n");
        return;
    }

    // Walk the config: find the first HID interface and its interrupt-IN endpoint.
    let total = total as usize;
    let mut off = 0usize;
    let mut cur_is_hid = false;
    let mut hid_iface = 0xFFu32;
    let mut iface_proto = 0u32;
    let mut ep_addr = 0u32;
    let mut ep_mps = 0u32;
    let mut ep_interval = 0u32;
    let mut found_ep = false;
    while off + 2 <= total {
        let blen = rd(off) as usize;
        let btype = rd(off + 1);
        if blen == 0 {
            break;
        }
        if btype == 4 {
            // Interface descriptor: bInterfaceClass @ +5 (3 = HID).
            cur_is_hid = rd(off + 5) == 3;
            // Record this interface only until an endpoint locks one in, so the
            // reported interface/protocol matches the endpoint we keep (combo
            // keyboard+mouse devices have two HID interfaces).
            if cur_is_hid && !found_ep {
                hid_iface = rd(off + 2); // bInterfaceNumber
                iface_proto = rd(off + 7); // bInterfaceProtocol: 1=keyboard, 2=mouse
            }
        } else if btype == 5 && cur_is_hid && !found_ep {
            // Endpoint descriptor: want IN (addr bit7) + interrupt (attr[1:0]=3).
            let addr = rd(off + 2);
            if addr & 0x80 != 0 && rd(off + 3) & 0x3 == 3 {
                ep_addr = addr;
                ep_mps = rd(off + 4) | (rd(off + 5) << 8);
                ep_interval = rd(off + 6);
                found_ep = true;
            }
        }
        off += blen;
    }

    if hid_iface == 0xFF || !found_ep {
        crate::console_write(b"  no HID interrupt-IN endpoint found in config\n");
        return;
    }
    crate::console_write(b"  HID interface found:\n");
    hex(b"    bInterfaceNumber=", hid_iface as u64, 2);
    crate::console_write(if iface_proto == 1 {
        b"    protocol=keyboard\n"
    } else if iface_proto == 2 {
        b"    protocol=mouse\n"
    } else {
        b"    protocol=other\n"
    });
    hex(b"    int-IN endpoint addr=", ep_addr as u64, 2);
    hex(b"    wMaxPacketSize=", ep_mps as u64, 4);
    hex(b"    bInterval=", ep_interval as u64, 2);

    // --- SET_CONFIGURATION --------------------------------------------------
    if x.control(0x00, 9, cfg_val as u16, 0, 0, 0).is_none() {
        crate::console_write(b"  SET_CONFIGURATION failed\n");
        return;
    }
    crate::console_write(b"  device configured\n");

    // --- Configure the interrupt-IN endpoint --------------------------------
    let ep_num = ep_addr & 0x0F;
    let dci = (ep_num * 2 + 1) as usize; // IN direction
    let Some(int_ring) = alloc_page() else { return };
    // Full/low-speed interrupt interval field = 3 + floor(log2(bInterval)).
    let mut iv = 0u32;
    let mut bi = ep_interval.max(1);
    while bi > 1 {
        bi >>= 1;
        iv += 1;
    }
    let interval_field = (iv + 3).clamp(3, 15);

    // Input context: add slot (A0) + this endpoint (A<dci>); slot ContextEntries=dci.
    unsafe {
        core::ptr::write_volatile(input_ctx as *mut u32, 0);
        core::ptr::write_volatile((input_ctx + 0x04) as *mut u32, 1 | (1 << dci));
        let sctx = input_ctx + cs;
        core::ptr::write_volatile(sctx as *mut u32, ((dci as u32) << 27) | (speed << 20));
        core::ptr::write_volatile((sctx + 4) as *mut u32, port << 16);
        let epc = input_ctx + dci as u64 * cs;
        core::ptr::write_volatile(epc as *mut u32, interval_field << 16);
        core::ptr::write_volatile((epc + 4) as *mut u32, (ep_mps << 16) | (7 << 3) | (3 << 1)); // Int-IN, CErr 3
        core::ptr::write_volatile((epc + 8) as *mut u64, dma(int_ring) | 1); // TR deq | DCS
        core::ptr::write_volatile((epc + 16) as *mut u32, (ep_mps << 16) | ep_mps.min(8));
    }
    dsb();
    x.ring_command(dma(input_ctx), (TRB_CONFIGURE_ENDPOINT << 10) | ((slot as u32) << 24));
    match x.wait_for(TRB_EV_CMD_COMPLETE, 40_000) {
        Some((_p, s, _c)) if (s >> 24) & 0xFF == 1 => crate::console_write(b"  endpoint configured\n"),
        Some((_p, s, _c)) => {
            crate::console_write(b"  configure endpoint: ");
            report_cc((s >> 24) & 0xFF);
            return;
        },
        None => {
            crate::console_write(b"  configure endpoint: no event\n");
            return;
        },
    }

    // --- SET_PROTOCOL(boot): best effort (gaming devices may STALL) ----------
    if x.control(0x21, 0x0B, 0, hid_iface as u16, 0, 0).is_some() {
        crate::console_write(b"  boot protocol set\n");
    } else {
        crate::console_write(b"  (no boot protocol; using report protocol)\n");
    }

    // --- Poll the interrupt-IN endpoint for HID reports ---------------------
    crate::console_write(b"  polling -- MOVE THE MOUSE NOW:\n");
    let Some(rbuf) = alloc_page() else { return };
    let rd8 = |o: usize| -> u32 {
        (unsafe { core::ptr::read_volatile((rbuf + o as u64) as *const u8) }) as u32
    };
    let mut ir_enq = 0usize;
    let start = crate::arch::timer::now_ms();
    let mut n = 0u32;
    while crate::arch::timer::now_ms() - start < 8000 && n < 24 && ir_enq < RING_TRBS - 1 {
        trb_write(int_ring, ir_enq, dma(rbuf), ep_mps, (TRB_NORMAL << 10) | (1 << 5) | 1); // IOC
        ir_enq += 1;
        dsb();
        unsafe { write32(x.db + slot as usize * 4, dci as u32) }; // endpoint doorbell
        match x.wait_for(TRB_EV_TRANSFER, 40_000) {
            Some((_p, s, _c)) if matches!((s >> 24) & 0xFF, 1 | 13) => {
                crate::console_write(b"    report: ");
                for o in 0..4 {
                    print_hexbyte(rd8(o));
                }
                crate::console_write(b"\n");
                n += 1;
            },
            Some((_p, s, _c)) => {
                crate::console_write(b"    xfer ");
                report_cc((s >> 24) & 0xFF);
                break;
            },
            None => {
                crate::console_write(b"    (no report within timeout)\n");
                break;
            },
        }
    }
    crate::console_write(b"  -> HID reports read -- USB input works!\n");
}
