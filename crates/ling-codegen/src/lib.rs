//! Code generation backends for Ling.
//!
//! The default build compiles to Ling bytecode only.
//! Enable the `llvm` feature for native code generation via LLVM 18.

pub mod bytecode;
pub mod wasm;

#[cfg(feature = "llvm")]
pub mod llvm;

/// Opaque MIR program handle passed to backends.
pub struct MirProgram {
    pub name: String,
}

/// All backends implement this trait.
pub trait CodegenBackend {
    fn emit(&mut self, mir: &MirProgram, out: &std::path::Path) -> anyhow::Result<()>;
}

pub use bytecode::BytecodeBackend;
pub use wasm::WasmBackend;
