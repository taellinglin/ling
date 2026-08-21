//! One `pub use` surface over the two architecture backends this kernel
//! supports (`x86_64`, `aarch64`) — portable code calls `crate::arch::cpu`,
//! `crate::arch::timer`, etc. and gets whichever backend matches the build
//! target, without repeating `#[cfg(target_arch = ...)]` at every call
//! site. Each backend only declares the submodules it actually has today
//! (`aarch64` has no `timer`/`pci`/`bootmodule` yet — see `packages/README.md`
//! in the LingOS repo for the parity build order); a caller reaching for one
//! that doesn't exist on the current target gets a compile error at the
//! call site, which is the correct outcome, not something to paper over
//! with an empty stub.

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use x86_64::*;

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use aarch64::*;
