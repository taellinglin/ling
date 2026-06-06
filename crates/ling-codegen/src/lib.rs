//! Code generation backends for Ling.

pub mod bytecode;
pub mod wasm;

pub mod cranelift;

#[cfg(feature = "llvm")]
pub mod llvm;

pub struct MirProgram {
    pub mir: ling_mir::MirProgram,
    pub name: String,
}

impl MirProgram {
    pub fn new(mir: ling_mir::MirProgram, name: impl Into<String>) -> Self {
        Self { mir, name: name.into() }
    }
}

pub trait CodegenBackend {
    fn emit(&mut self, mir: &MirProgram, out: &std::path::Path) -> anyhow::Result<()>;
}

pub use bytecode::{BytecodeBackend, Vm, VmProgram, compile_mir_program};
pub use wasm::WasmBackend;
