//! Device drivers: console (VGA text mode + PL011 UART), input (PS/2
//! keyboard/mouse), storage (ATA PIO, AHCI), and the Multiboot2 linear
//! framebuffer. Each module is gated to the one architecture that actually
//! has it today — see `arch/mod.rs`'s doc for why a missing one is a
//! compile error at the call site rather than a stub.

#[cfg(target_arch = "x86_64")]
pub mod ac97;
#[cfg(target_arch = "x86_64")]
pub mod ahci;
#[cfg(target_arch = "x86_64")]
pub mod ata;
#[cfg(target_arch = "x86_64")]
pub mod browser;
#[cfg(target_arch = "x86_64")]
pub mod daw;
#[cfg(target_arch = "x86_64")]
pub mod clipboard;
#[cfg(target_arch = "x86_64")]
pub mod display;
#[cfg(target_arch = "x86_64")]
pub mod editor;
#[cfg(target_arch = "x86_64")]
pub mod horizon;
#[cfg(target_arch = "x86_64")]
pub mod llm;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub mod messenger;
/// USB gamepad report layouts and decoding (used by the Pi's xHCI driver).
pub mod hidpad;
#[cfg(target_arch = "x86_64")]
pub mod mixer;
#[cfg(target_arch = "x86_64")]
pub mod wallpaper;
// font8x8 + framebuffer are shared: x86_64 (Multiboot2/DISPI) and aarch64
// (Raspberry Pi VideoCore-mailbox framebuffer, incl. the Pi boot banner).
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub mod font8x8;
#[cfg(target_arch = "x86_64")]
pub mod flags;
#[cfg(target_arch = "x86_64")]
pub mod font_unicode;
#[cfg(target_arch = "x86_64")]
pub mod gallery;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub mod framebuffer;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub mod cursor;
#[cfg(target_arch = "x86_64")]
pub mod kbdlayout;
#[cfg(target_arch = "x86_64")]
pub mod keyboard;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub mod lingfu;
#[cfg(target_arch = "x86_64")]
pub mod locale;
// PNG/SVG decoding: the x86 desktop's Gallery/browsers and the Pi desktop's Files.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub mod image;
#[cfg(target_arch = "x86_64")]
pub mod media;
#[cfg(target_arch = "x86_64")]
pub mod pkgman;
#[cfg(target_arch = "x86_64")]
pub mod mouse;
#[cfg(target_arch = "x86_64")]
pub mod net_e1000;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub mod netstack;
#[cfg(target_arch = "x86_64")]
pub mod serial;
#[cfg(target_arch = "x86_64")]
pub mod sshd;
#[cfg(target_arch = "x86_64")]
pub mod term;
#[cfg(target_arch = "x86_64")]
pub mod terminal;
#[cfg(target_arch = "x86_64")]
pub mod theme;
#[cfg(target_arch = "x86_64")]
pub mod ui_scale;
#[cfg(target_arch = "x86_64")]
pub mod vga;
#[cfg(target_arch = "x86_64")]
pub mod wm;
#[cfg(target_arch = "x86_64")]
pub mod wm_liquid;

#[cfg(target_arch = "aarch64")]
pub mod fbcon;
#[cfg(target_arch = "aarch64")]
pub mod uart;
// USB flash drives (bulk-only mass storage over xHCI).
#[cfg(target_arch = "aarch64")]
pub mod usbmsd;
