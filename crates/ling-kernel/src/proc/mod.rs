//! Process/task scheduling: [`sched`] (cooperative, ring-0-only, fixed
//! 4-slot — still what `.ling` kernel-target intrinsics use; x86_64-only,
//! since aarch64 has no equivalent cooperative kernel-task scheduler) and,
//! portable across both architectures, [`uproc`] (preemptive, EL0/ring-3,
//! real isolated processes with their own address space — see its module
//! doc for how the two stay separate rather than unified) and [`elf`], the
//! loader that turns an ELF64 image into something `uproc::spawn` can run.

#[cfg(target_arch = "x86_64")]
pub mod sched;
pub mod elf;
pub mod uproc;
