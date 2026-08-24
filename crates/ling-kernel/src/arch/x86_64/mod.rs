//! x86_64 architecture backend: CPU intrinsics, port I/O, the Multiboot2
//! long-mode bootstrap, PCI config-space access, and the boot-relative TSC
//! clock. See `boot::` (formerly `boot32.rs`) for why long-mode bring-up
//! has to be hand-written assembly before any of the rest of this crate is
//! safe to run.

pub mod bootmodule;
pub mod cpu;
pub mod io;
pub mod meminfo;
pub mod pci;
pub mod rtc;
pub mod timer;

pub mod gdt;
pub mod idt;
pub mod paging;
pub mod pic;
pub mod trap;

pub mod boot;
