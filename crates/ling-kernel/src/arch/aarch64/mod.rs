//! aarch64 architecture backend: CPU intrinsics, MMIO helpers, the
//! Raspberry Pi raw-binary boot stub (`kernel8.img` has no ELF loader, so
//! `boot::zero_bss` has to run before any Rust static is trusted, and
//! `boot::_start` drops EL2 -> EL1 before anything else does), the
//! CNTPCT_EL0-based clock, and the BCM2837 interrupt controllers (`intc`;
//! see its module doc for why this isn't a GIC driver) plus the AArch64
//! exception vector table (`vectors`) built on top of them.

pub mod cpu;
pub mod mailbox;
pub mod mmio;
pub mod timer;
pub mod intc;
pub mod vectors;

pub mod boot;
