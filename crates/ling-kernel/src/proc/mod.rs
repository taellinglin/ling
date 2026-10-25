//! Process/task scheduling: [`sched`] (cooperative, kernel-mode only, fixed
//! 4-slot — still what `.ling` kernel-target intrinsics use; on both
//! architectures, the Pi's background sshd / Messenger tasks run on it) and,
//! portable across both architectures, [`uproc`] (preemptive, EL0/ring-3,
//! real isolated processes with their own address space — see its module
//! doc for how the two stay separate rather than unified) and [`elf`], the
//! loader that turns an ELF64 image into something `uproc::spawn` can run.

pub mod sched;
pub mod elf;
pub mod uproc;
