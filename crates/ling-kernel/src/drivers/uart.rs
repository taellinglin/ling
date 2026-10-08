//! PL011 UART driver (BCM2837/Pi 3, BCM2711/Pi 4 register layout — see
//! `mmio::peripheral_base`). This is the aarch64/Raspberry Pi serial console:
//! no VGA text buffer exists on this hardware, so `ling_kernel_vga_*`
//! intrinsics route here (and, on Pi 5, to the HDMI framebuffer).
//!
//! On Pi 5 (BCM2712) the GPIO 14/15 UART lives on the RP1 southbridge behind
//! PCIe — not reachable with this BCM283x GPIO/PL011 sequence — so every entry
//! point here is a no-op on Pi 5 rather than touching MMIO that would hang or
//! fault. Pi 5 output goes to the HDMI framebuffer instead.
use crate::arch::mmio::{board, peripheral_base, read32, write32, RpiBoard};

// Register offsets within the GPIO / UART0 blocks.
const O_GPFSEL1: usize = 0x04;
const O_GPPUD: usize = 0x94;
const O_GPPUDCLK0: usize = 0x98;
const O_DR: usize = 0x00;
const O_FR: usize = 0x18;
const O_IBRD: usize = 0x24;
const O_FBRD: usize = 0x28;
const O_LCRH: usize = 0x2C;
const O_CR: usize = 0x30;
const O_IMSC: usize = 0x38;
const O_ICR: usize = 0x44;

#[inline]
fn gpio_base() -> usize {
    peripheral_base() + 0x0020_0000
}
#[inline]
fn uart0_base() -> usize {
    peripheral_base() + 0x0020_1000
}

/// True on a board whose GPIO-header UART this driver can't drive directly
/// (Pi 5's is on RP1/PCIe). Every public entry point short-circuits on it.
#[inline]
fn unsupported() -> bool {
    board() == RpiBoard::Pi5
}

const FR_TXFF: u32 = 1 << 5; // transmit FIFO full
const FR_RXFE: u32 = 1 << 4; // receive FIFO empty

const RXIM: u32 = 1 << 4; // RX FIFO interrupt
const RTIM: u32 = 1 << 6; // RX timeout interrupt (bytes below the FIFO trigger level)
const RXIC_RTIC: u32 = RXIM | RTIM; // same bit positions in UARTICR

/// Fixed-capacity RX queue the IRQ handler (`arch::aarch64::vectors`) drains
/// the PL011 FIFO into and `read_byte`/`poll_byte` drain from — same single-
/// producer/single-consumer reasoning as `keyboard.rs`'s ring buffer.
const RING_CAPACITY: usize = 256;
static mut RING: [u8; RING_CAPACITY] = [0; RING_CAPACITY];
static mut RING_HEAD: u8 = 0;
static mut RING_TAIL: u8 = 0;

fn delay(cycles: u32) {
    for _ in 0..cycles {
        unsafe { core::arch::asm!("nop", options(nomem, nostack)) };
    }
}

/// Initialize UART0 on GPIO 14/15 (TXD0/RXD0, ALT0) at 115200 baud.
pub fn init() {
    if unsupported() {
        return;
    }
    let g = gpio_base();
    let u = uart0_base();
    unsafe {
        write32(u + O_CR, 0); // disable UART0

        // Route GPIO 14/15 to UART0 (ALT0) instead of GPIO.
        let mut sel = read32(g + O_GPFSEL1);
        sel &= !((0b111 << 12) | (0b111 << 15));
        sel |= (0b100 << 12) | (0b100 << 15);
        write32(g + O_GPFSEL1, sel);

        // Disable pull-up/down on GPIO 14/15 (BCM2837 two-step sequence).
        write32(g + O_GPPUD, 0);
        delay(150);
        write32(g + O_GPPUDCLK0, (1 << 14) | (1 << 15));
        delay(150);
        write32(g + O_GPPUDCLK0, 0);

        write32(u + O_ICR, 0x7FF); // clear pending interrupts

        // 115200 baud assuming a 48MHz UART clock (default firmware config).
        write32(u + O_IBRD, 26);
        write32(u + O_FBRD, 3);

        write32(u + O_LCRH, (1 << 4) | (3 << 5)); // FIFOs enabled, 8N1
                                                  // Only RX + RX-timeout unmasked: TX is still polled (`write_byte`),
                                                  // and unmasking TXIM here would fire continuously any time the TX
                                                  // FIFO isn't full — which, for a byte-at-a-time console, is nearly
                                                  // always — flooding the IRQ line with nothing to service.
        write32(u + O_IMSC, RXIC_RTIC);
        write32(u + O_CR, (1 << 0) | (1 << 8) | (1 << 9)); // UARTEN | TXE | RXE
    }
}

fn tx_ready() -> bool {
    unsafe { read32(uart0_base() + O_FR) & FR_TXFF == 0 }
}

fn rx_ready() -> bool {
    unsafe { read32(uart0_base() + O_FR) & FR_RXFE == 0 }
}

pub fn write_byte(byte: u8) {
    if unsupported() {
        return;
    }
    while !tx_ready() {
        core::hint::spin_loop();
    }
    unsafe { write32(uart0_base() + O_DR, byte as u32) };
}

pub fn write(bytes: &[u8]) {
    if unsupported() {
        return;
    }
    for &b in bytes {
        if b == b'\n' {
            write_byte(b'\r');
        }
        write_byte(b);
    }
}

pub fn write_str(s: &str) {
    write(s.as_bytes());
}

fn ring_pop() -> Option<u8> {
    unsafe {
        if RING_HEAD == RING_TAIL {
            return None;
        }
        let byte = RING[RING_TAIL as usize];
        RING_TAIL = RING_TAIL.wrapping_add(1);
        Some(byte)
    }
}

/// Called from the IRQ handler (`arch::aarch64::vectors`) once `intc`
/// reports UART0's shared peripheral IRQ pending. Drains every byte
/// currently in the RX FIFO into the ring buffer (there may be several —
/// that's exactly what the timeout interrupt, RTIM, is for: bytes sitting
/// below the FIFO's trigger level long enough to need flushing) and clears
/// both serviced interrupt sources.
pub(crate) fn irq_drain() {
    if unsupported() {
        return;
    }
    while rx_ready() {
        let byte = unsafe { read32(uart0_base() + O_DR) as u8 };
        let next = unsafe { RING_HEAD.wrapping_add(1) };
        if next != unsafe { RING_TAIL } {
            unsafe { RING[RING_HEAD as usize] = byte };
            unsafe { RING_HEAD = next };
        }
    }
    unsafe { write32(uart0_base() + O_ICR, RXIC_RTIC) };
}

/// Blocking single-byte read: parks in `wfi` between interrupts instead of
/// spinning on the FIFO — real work now that RX is IRQ-delivered.
pub fn read_byte() -> u8 {
    loop {
        if let Some(b) = ring_pop() {
            return b;
        }
        unsafe { crate::arch::cpu::wfi() };
    }
}

/// Non-blocking poll: `None` if no byte is waiting.
pub fn poll_byte() -> Option<u8> {
    ring_pop()
}
