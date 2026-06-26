//! Code generation backends for Ling.

pub mod bytecode;
pub mod wasm;

pub mod cranelift;

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

pub use bytecode::{compile_mir_program, BytecodeBackend, Vm, VmProgram};
pub use cranelift::aot::CraneliftBackend;
#[cfg(not(target_arch = "wasm32"))]
pub use cranelift::jit::JitBackend;
pub use cranelift::runtime;
pub use wasm::WasmBackend;
