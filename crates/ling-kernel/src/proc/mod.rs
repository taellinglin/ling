//! Process/task scheduling: [`sched`] (cooperative, ring-0-only, fixed
//! 4-slot — still what `.ling` kernel-target intrinsics use) alongside
//! [`uproc`] (preemptive, ring-3, real isolated processes with their own
//! address space — see its module doc for how the two stay separate rather
//! than unified) and [`elf`], the loader that turns an ELF64 image into
//! something `uproc::spawn` can run. All x86_64-only for now: aarch64's
//! `svc`/exception-level counterpart doesn't exist yet.

#[cfg(target_arch = "x86_64")]
pub mod sched;
#[cfg(target_arch = "x86_64")]
pub mod elf;
#[cfg(target_arch = "x86_64")]
pub mod uproc;
