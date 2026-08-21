//! `lingfs` (a git-style content-addressed store — see `lingfs`'s own module
//! doc) and everything built directly on it: the block-device abstraction it
//! reads/writes through, user accounts, and the `.lpkg` package format. All
//! x86_64-only today — the aarch64 storage stack (`drivers::emmc`, once it
//! exists) is on the LingOS packages roadmap, not started yet.

#[cfg(target_arch = "x86_64")]
pub mod blockdev;
#[cfg(target_arch = "x86_64")]
pub mod lingfs;
#[cfg(target_arch = "x86_64")]
pub mod packages;
#[cfg(target_arch = "x86_64")]
pub mod users;
