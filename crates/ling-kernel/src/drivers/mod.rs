//! Device drivers: console (VGA text mode + PL011 UART), input (PS/2
//! keyboard/mouse), storage (ATA PIO, AHCI), and the Multiboot2 linear
//! framebuffer. Each module is gated to the one architecture that actually
//! has it today — see `arch/mod.rs`'s doc for why a missing one is a
//! compile error at the call site rather than a stub.

#[cfg(target_arch = "x86_64")]
pub mod ahci;
#[cfg(target_arch = "x86_64")]
pub mod ata;
#[cfg(target_arch = "x86_64")]
pub mod font8x8;
#[cfg(target_arch = "x86_64")]
pub mod framebuffer;
#[cfg(target_arch = "x86_64")]
pub mod keyboard;
#[cfg(target_arch = "x86_64")]
pub mod mouse;
#[cfg(target_arch = "x86_64")]
pub mod serial;
#[cfg(target_arch = "x86_64")]
pub mod term;
#[cfg(target_arch = "x86_64")]
pub mod vga;
#[cfg(target_arch = "x86_64")]
pub mod wm_liquid;

#[cfg(target_arch = "aarch64")]
pub mod uart;
