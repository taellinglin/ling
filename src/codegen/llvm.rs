// src/codegen/llvm.rs
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::values::FunctionValue;

#[cfg(feature = "llvm")]
pub struct LlvmCodegen<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: inkwell::builder::Builder<'ctx>,
}


impl<'ctx> LlvmCodegen<'ctx> {
    pub fn new(context: &'ctx Context, name: &str) -> Self {
        let module = context.create_module(name);
        let builder = context.create_builder();
        Self { context, module, builder }
    }
    
    pub fn compile(&mut self, mir: &MIR) -> LingResult<()> {
        for func in &mir.functions {
            self.compile_function(func)?;
        }
        Ok(())
    }
    
    fn compile_function(&mut self, func: &MirFunction) -> LingResult<FunctionValue> {
        // LLVM function generation
        unimplemented!()
    }
    
    pub fn emit_object(&self, path: &std::path::Path) -> LingResult<()> {
        self.module.print_to_file(path)?;
        Ok(())
    }
}
