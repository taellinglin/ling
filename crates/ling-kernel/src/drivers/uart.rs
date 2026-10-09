//! PL011 UART driver — the aarch64/Raspberry Pi serial console.
//!
//! - **Pi 3 (BCM2837) / Pi 4 (BCM2711)**: UART0 on GPIO 14/15, at
//!   `peripheral_base + 0x20_1000`. RX is IRQ-delivered into a ring buffer.
//! - **Pi 5 (BCM2712)**: the GPIO 14/15 UART is on the RP1 southbridge (PCIe),
//!   but the SoC also has a **dedicated debug UART (UART10, a PL011 at
//!   `0x10_7d00_1000`)** on the 3-pin debug header, which the firmware always
//!   leaves initialized at 115200 8N1. We use that directly, polled (Pi 5 has no
//!   GIC wired yet, so no RX IRQ): no GPIO/baud setup needed.
//!
//! `ling_kernel_vga_*` intrinsics route here (and, on Pi 5, also to the HDMI
//! framebuffer console).
use crate::arch::mmio::{board, peripheral_base, read32, write32, RpiBoard};

// Register offsets within the GPIO / UART blocks.
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

/// Pi 5 (BCM2712) dedicated debug UART (UART10, PL011) — the 3-pin header.
const PI5_DEBUG_UART: usize = 0x10_7d00_1000;

#[inline]
fn gpio_base() -> usize {
    peripheral_base() + 0x0020_0000
}

/// Base address of the UART this driver drives on the detected board.
#[inline]
fn uart_base() -> usize {
    if board() == RpiBoard::Pi5 {
        PI5_DEBUG_UART
    } else {
        peripheral_base() + 0x0020_1000
    }
}

/// Pi 5 uses the firmware-configured debug UART, polled — not the GPIO UART +
/// IRQ setup path the BCM283x/BCM2711 boards use.
#[inline]
fn is_pi5() -> bool {
    board() == RpiBoard::Pi5
}

const FR_TXFF: u32 = 1 << 5; // transmit FIFO full
const FR_RXFE: u32 = 1 << 4; // receive FIFO empty

const CR_UARTEN: u32 = 1 << 0;
const CR_TXE: u32 = 1 << 8;
const CR_RXE: u32 = 1 << 9;

const RXIM: u32 = 1 << 4; // RX FIFO interrupt
const RTIM: u32 = 1 << 6; // RX timeout interrupt (bytes below the FIFO trigger level)
const RXIC_RTIC: u32 = RXIM | RTIM; // same bit positions in UARTICR

/// Bound on the TX-full spin so a wrong/unresponsive UART can never hang boot.
const TX_SPIN_MAX: u32 = 2_000_000;

/// Fixed-capacity RX queue the IRQ handler (`arch::aarch64::vectors`) drains the
/// PL011 FIFO into and `read_byte`/`poll_byte` drain from (Pi 3/4 only — Pi 5
/// polls the FIFO directly).
const RING_CAPACITY: usize = 256;
static mut RING: [u8; RING_CAPACITY] = [0; RING_CAPACITY];
static mut RING_HEAD: u8 = 0;
static mut RING_TAIL: u8 = 0;

fn delay(cycles: u32) {
    for _ in 0..cycles {
        unsafe { core::arch::asm!("nop", options(nomem, nostack)) };
    }
}

/// Initialize the console UART.
pub fn init() {
    let u = uart_base();
    if is_pi5() {
        // The firmware already set up UART10 (115200 8N1, enabled). Just make
        // sure RX is on too (firmware uses it for TX-only logging), without
        // touching the baud/line config it already programmed. We poll RX, so
        // no interrupt setup.
        unsafe {
            let cr = read32(u + O_CR);
            write32(u + O_CR, cr | CR_UARTEN | CR_TXE | CR_RXE);
        }
        return;
    }

    // Pi 3 / Pi 4: bring up UART0 on GPIO 14/15 (TXD0/RXD0, ALT0) at 115200.
    let g = gpio_base();
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
                                                  // Only RX + RX-timeout unmasked: TX is polled.
        write32(u + O_IMSC, RXIC_RTIC);
        write32(u + O_CR, CR_UARTEN | CR_TXE | CR_RXE);
    }
}

fn tx_ready() -> bool {
    unsafe { read32(uart_base() + O_FR) & FR_TXFF == 0 }
}

fn rx_ready() -> bool {
    unsafe { read32(uart_base() + O_FR) & FR_RXFE == 0 }
}

pub fn write_byte(byte: u8) {
    let mut spins = 0u32;
    while !tx_ready() {
        core::hint::spin_loop();
        spins += 1;
        if spins > TX_SPIN_MAX {
            return; // don't hang if the UART never drains
        }
    }
    unsafe { write32(uart_base() + O_DR, byte as u32) };
}

pub fn write(bytes: &[u8]) {
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

/// Called from the IRQ handler (`arch::aarch64::vectors`, Pi 3/4) once `intc`
/// reports UART0's shared peripheral IRQ pending. Drains the RX FIFO into the
/// ring buffer and clears the serviced interrupt sources.
pub(crate) fn irq_drain() {
    if is_pi5() {
        return; // Pi 5 has no UART IRQ wired; it polls (see poll_byte).
    }
    while rx_ready() {
        let byte = unsafe { read32(uart_base() + O_DR) as u8 };
        let next = unsafe { RING_HEAD.wrapping_add(1) };
        if next != unsafe { RING_TAIL } {
            unsafe { RING[RING_HEAD as usize] = byte };
            unsafe { RING_HEAD = next };
        }
    }
    unsafe { write32(uart_base() + O_ICR, RXIC_RTIC) };
}

/// Non-blocking read: `None` if no byte is waiting. On Pi 5 this polls the
/// PL011 RX FIFO directly (no IRQ); on Pi 3/4 it drains the IRQ ring buffer.
pub fn poll_byte() -> Option<u8> {
    if is_pi5() {
        if rx_ready() {
            Some(unsafe { read32(uart_base() + O_DR) as u8 })
        } else {
            None
        }
    } else {
        ring_pop()
    }
}

/// Blocking single-byte read.
pub fn read_byte() -> u8 {
    loop {
        if let Some(b) = poll_byte() {
            return b;
        }
        if is_pi5() {
            core::hint::spin_loop(); // no IRQ to wake a wfi on Pi 5
        } else {
            unsafe { crate::arch::cpu::wfi() };
        }
    }
}
