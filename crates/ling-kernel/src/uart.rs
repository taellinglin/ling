//! PL011 UART driver (BCM2837/Pi3 register layout — see `mmio::PERIPHERAL_BASE`).
//! This is the aarch64/Raspberry Pi console: no VGA text buffer exists on
//! this hardware, so `ling_kernel_vga_*` intrinsics route here instead.
use crate::mmio::{read32, write32, PERIPHERAL_BASE};

const GPIO_BASE: usize = PERIPHERAL_BASE + 0x0020_0000;
const UART0_BASE: usize = PERIPHERAL_BASE + 0x0020_1000;

const GPFSEL1: usize = GPIO_BASE + 0x04;
const GPPUD: usize = GPIO_BASE + 0x94;
const GPPUDCLK0: usize = GPIO_BASE + 0x98;

const UART_DR: usize = UART0_BASE + 0x00;
const UART_FR: usize = UART0_BASE + 0x18;
const UART_IBRD: usize = UART0_BASE + 0x24;
const UART_FBRD: usize = UART0_BASE + 0x28;
const UART_LCRH: usize = UART0_BASE + 0x2C;
const UART_CR: usize = UART0_BASE + 0x30;
const UART_IMSC: usize = UART0_BASE + 0x38;
const UART_ICR: usize = UART0_BASE + 0x44;

const FR_TXFF: u32 = 1 << 5; // transmit FIFO full
const FR_RXFE: u32 = 1 << 4; // receive FIFO empty

fn delay(cycles: u32) {
    for _ in 0..cycles {
        unsafe { core::arch::asm!("nop", options(nomem, nostack)) };
    }
}

/// Initialize UART0 on GPIO 14/15 (TXD0/RXD0, ALT0) at 115200 baud.
pub fn init() {
    unsafe {
        write32(UART_CR, 0); // disable UART0

        // Route GPIO 14/15 to UART0 (ALT0) instead of GPIO.
        let mut sel = read32(GPFSEL1);
        sel &= !((0b111 << 12) | (0b111 << 15));
        sel |= (0b100 << 12) | (0b100 << 15);
        write32(GPFSEL1, sel);

        // Disable pull-up/down on GPIO 14/15 (BCM2837 two-step sequence).
        write32(GPPUD, 0);
        delay(150);
        write32(GPPUDCLK0, (1 << 14) | (1 << 15));
        delay(150);
        write32(GPPUDCLK0, 0);

        write32(UART_ICR, 0x7FF); // clear pending interrupts

        // 115200 baud assuming a 48MHz UART clock (default firmware config).
        write32(UART_IBRD, 26);
        write32(UART_FBRD, 3);

        write32(UART_LCRH, (1 << 4) | (3 << 5)); // FIFOs enabled, 8N1
        write32(UART_IMSC, 0x7FF); // mask all interrupts (we poll)
        write32(UART_CR, (1 << 0) | (1 << 8) | (1 << 9)); // UARTEN | TXE | RXE
    }
}

fn tx_ready() -> bool {
    unsafe { read32(UART_FR) & FR_TXFF == 0 }
}

fn rx_ready() -> bool {
    unsafe { read32(UART_FR) & FR_RXFE == 0 }
}

pub fn write_byte(byte: u8) {
    while !tx_ready() {
        core::hint::spin_loop();
    }
    unsafe { write32(UART_DR, byte as u32) };
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

/// Blocking single-byte read.
pub fn read_byte() -> u8 {
    while !rx_ready() {
        core::hint::spin_loop();
    }
    unsafe { read32(UART_DR) as u8 }
}

/// Non-blocking poll: `None` if no byte is waiting.
pub fn poll_byte() -> Option<u8> {
    if rx_ready() {
        Some(unsafe { read32(UART_DR) as u8 })
    } else {
        None
    }
}
