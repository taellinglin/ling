// src/core/mod.rs
mod config;
mod error;
mod result;

pub use config::{CompilerConfig, OptimizationLevel};
pub use error::LingError;
pub use result::LingResult;

use crate::mir;
use ling_codegen::CodegenBackend;
use std::path::Path;

pub struct LingCompiler {
    config: CompilerConfig,
}

impl LingCompiler {
    pub fn new(config: CompilerConfig) -> Self {
        Self { config }
    }

    pub fn compile<P: AsRef<Path>>(&self, input: P, output: P) -> LingResult<()> {
        let source =
            std::fs::read_to_string(input.as_ref()).map_err(|e| LingError::Io(e.to_string()))?;
        let mir = mir::compile_and_optimize(&source, self.config.optimization)?;
        println!(
            "compiled {} functions, optimized at {:?}",
            mir.functions.len(),
            self.config.optimization
        );

        let mir_prog = ling_codegen::MirProgram::new(mir, input.as_ref().to_string_lossy());
        let mut backend = ling_codegen::BytecodeBackend;
        backend
            .emit(&mir_prog, output.as_ref())
            .map_err(|e| LingError::Codegen(e.to_string()))?;
        println!("bytecode emitted to {}", output.as_ref().display());
        Ok(())
    }

    pub fn compile_and_run<P: AsRef<Path>>(&self, input: P) -> LingResult<()> {
        let source =
            std::fs::read_to_string(input.as_ref()).map_err(|e| LingError::Io(e.to_string()))?;
        let mir = mir::compile_and_optimize(&source, self.config.optimization)?;
        println!(
            "compiled {} functions, optimized at {:?}",
            mir.functions.len(),
            self.config.optimization
        );

        let vm_prog = ling_codegen::compile_mir_program(&mir);
        let mut vm = ling_codegen::Vm::new();
        vm.load(vm_prog);
        vm.run_main()
            .map_err(|e| LingError::Codegen(e.to_string()))?;
        Ok(())
    }

    pub fn dump_mir<P: AsRef<Path>>(&self, input: P) -> LingResult<()> {
        let source =
            std::fs::read_to_string(input.as_ref()).map_err(|e| LingError::Io(e.to_string()))?;
        let mir = mir::compile_and_optimize(&source, self.config.optimization)?;
        for func in &mir.functions {
            println!("=== function: {} ===", func.name);
            println!("  args: {}, locals: {}", func.arg_count, func.locals.len());
            for (i, decl) in func.locals.iter().enumerate() {
                let name = decl.name.as_deref().unwrap_or("_");
                println!(
                    "  local {}: {} {:?} mut={} owning={}",
                    i, name, decl.ty, decl.is_mut, decl.is_owning
                );
            }
            for (bi, bb) in func.basic_blocks.iter().enumerate() {
                println!("  bb{}:", bi);
                for stmt in &bb.statements {
                    println!("    {:?}", stmt.kind);
                }
                if let Some(term) = &bb.terminator {
                    println!("    term: {:?}", term.kind);
                }
            }
        }
        Ok(())
    }
}
