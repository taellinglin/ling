//! `lingfs` (a git-style content-addressed store — see `lingfs`'s own module
//! doc) and everything built directly on it: the block-device abstraction it
//! reads/writes through, user accounts, and the `.lpkg` package format. All
//! x86_64-only today — the aarch64 storage stack (`drivers::emmc`, once it
//! exists) is on the LingOS packages roadmap, not started yet.

pub mod blockdev;
// x86: every whole disk (SATA, NVMe, USB), partition tables, the installer's GPT.
#[cfg(target_arch = "x86_64")]
pub mod disks;
// FAT16/32 volumes (USB flash drives, SD cards) and their mounts under media/.
pub mod fat;
// exFAT volumes (files over 4 GiB: big data partitions, SDXC cards).
pub mod exfat;
pub mod media;
pub mod lingfs;
pub mod packages;
pub mod users;
// Optional two-factor sign-in (TOTP + recovery codes) for accounts.
pub mod twofa;
