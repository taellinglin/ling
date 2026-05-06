// src/codegen/mod.rs
// Backends are WIP; keep the crate building without heavy deps.
// Enable with cargo features later.

#[cfg(feature = "llvm")]
pub mod llvm;

#[cfg(feature = "llvm")]
pub use llvm::LlvmCodegen;

