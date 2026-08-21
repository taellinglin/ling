//! aarch64 architecture backend: CPU intrinsics, MMIO helpers, the
//! Raspberry Pi raw-binary boot stub (`kernel8.img` has no ELF loader, so
//! `boot::zero_bss` has to run before any Rust static is trusted, and
//! `boot::_start` drops EL2 -> EL1 before anything else does), the
//! CNTPCT_EL0-based clock, and the BCM2837 interrupt controllers (`intc`;
//! see its module doc for why this isn't a GIC driver) plus the AArch64
//! exception vector table (`vectors`) built on top of them. `paging` is the
//! aarch64 counterpart of the x86_64 backend's `paging` (4-level stage-1
//! tables under `TTBR0_EL1`); `trap` is its counterpart of `trap.rs` —
//! `SVC`/EL0 process entry, exit, and the portable `TrapFrame` accessors
//! `proc::uproc`/`abi::syscalls` use without caring which architecture they
//! run on.

pub mod cpu;
pub mod mailbox;
pub mod mmio;
pub mod timer;
pub mod intc;
pub mod paging;
pub mod vectors;
pub mod trap;

pub mod boot;
