//! Code generation for Ling MIR

pub mod llvm;
pub mod cranelift;
pub mod wasm;

pub trait CodegenBackend {
    fn new() -> Self where Self: Sized;
    fn emit_object(&mut self, mir: &MirProgram, path: &std::path::Path) -> anyhow::Result<()>;
}
