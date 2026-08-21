//! Process/task scheduling. Currently just `sched` — cooperative,
//! ring-0-only, fixed 4-slot (see its module doc for why, and for the
//! preemptive/ring-3 replacement this is heading toward). x86_64-only:
//! there is nothing to schedule on aarch64 yet, since it has no shell.

#[cfg(target_arch = "x86_64")]
pub mod sched;
