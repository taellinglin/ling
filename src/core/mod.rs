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

    /// Compile and run using the Cranelift JIT backend.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn compile_and_run_jit<P: AsRef<Path>>(&self, input: P) -> LingResult<()> {
        let flat = mir::flatten_path(input.as_ref())?;
        let mir = mir::lower_and_optimize(&flat, self.config.optimization);
        let mir_prog = ling_codegen::MirProgram::new(mir, input.as_ref().to_string_lossy());
        let source_dir = input.as_ref().parent().map(|p| p.to_path_buf());
        self.run_jit_mir(mir_prog, flat, source_dir)
    }

    /// Compile and run concatenated `source` (resolving its `use` imports against
    /// `source_dir`) on the Cranelift JIT backend. The host launcher pastes its
    /// `.ling` files together and calls this; duplicate definitions are resolved
    /// in [`mir::flatten_source`].
    #[cfg(not(target_arch = "wasm32"))]
    pub fn compile_and_run_jit_source(
        &self,
        source: &str,
        source_dir: Option<std::path::PathBuf>,
    ) -> LingResult<()> {
        let dir = source_dir
            .clone()
            .unwrap_or_else(|| Path::new(".").to_path_buf());
        let flat = mir::flatten_source(source, &dir)?;
        let mir = mir::lower_and_optimize(&flat, self.config.optimization);
        let mir_prog = ling_codegen::MirProgram::new(mir, "__main__".to_string());
        self.run_jit_mir(mir_prog, flat, source_dir)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn run_jit_mir(
        &self,
        mir_prog: ling_codegen::MirProgram,
        fallback_ast: crate::parser::ast::Program,
        source_dir: Option<std::path::PathBuf>,
    ) -> LingResult<()> {
        use crate::runtime::jit_abi::{
            ling_abs, ling_add, ling_alloc, ling_and, ling_bool_to_u64, ling_builtin, ling_ceil,
            ling_cos, ling_div, ling_eq, ling_f64_add, ling_f64_div, ling_f64_eq, ling_f64_ge,
            ling_f64_gt, ling_f64_le, ling_f64_lt, ling_f64_mul, ling_f64_neg, ling_f64_rem,
            ling_f64_sub, ling_floor, ling_free, ling_ge, ling_gt, ling_le, ling_list_get,
            ling_list_len, ling_list_new, ling_list_push, ling_list_set, ling_lt, ling_mul, ling_ne, ling_neg,
            ling_not, ling_or, ling_panic, ling_print, ling_print_newline, ling_print_val,
            ling_rem, ling_round, ling_sin, ling_sqrt, ling_str_concat, ling_str_eq, ling_str_len,
            ling_str_new, ling_struct_get, ling_struct_new, ling_sub, ling_time_now,
        };
        use crate::runtime::Interpreter;

        let mut jit = ling_codegen::JitBackend::new(|builder| {
            macro_rules! reg {
                ($rust:ident, $jit:literal) => {
                    builder.symbol(concat!("__ling_", $jit), $rust as *const u8);
                };
                ($name:ident) => {
                    reg!($name, stringify!($name));
                };
            }
            reg!(ling_f64_add, "f64_add");
            reg!(ling_f64_sub, "f64_sub");
            reg!(ling_f64_mul, "f64_mul");
            reg!(ling_f64_div, "f64_div");
            reg!(ling_f64_rem, "f64_rem");
            reg!(ling_f64_neg, "f64_neg");
            reg!(ling_f64_eq, "f64_eq");
            reg!(ling_f64_lt, "f64_lt");
            reg!(ling_f64_gt, "f64_gt");
            reg!(ling_f64_le, "f64_le");
            reg!(ling_f64_ge, "f64_ge");
            reg!(ling_sin, "sin");
            reg!(ling_cos, "cos");
            reg!(ling_sqrt, "sqrt");
            reg!(ling_abs, "abs");
            reg!(ling_floor, "floor");
            reg!(ling_ceil, "ceil");
            reg!(ling_round, "round");
            reg!(ling_add, "add");
            reg!(ling_sub, "sub");
            reg!(ling_mul, "mul");
            reg!(ling_div, "div");
            reg!(ling_rem, "rem");
            reg!(ling_neg, "neg");
            reg!(ling_eq, "eq");
            reg!(ling_ne, "ne");
            reg!(ling_lt, "lt");
            reg!(ling_le, "le");
            reg!(ling_gt, "gt");
            reg!(ling_ge, "ge");
            reg!(ling_and, "and");
            reg!(ling_or, "or");
            reg!(ling_not, "not");
            reg!(ling_bool_to_u64, "bool_to_u64");
            reg!(ling_alloc, "alloc");
            reg!(ling_free, "free");
            reg!(ling_panic, "panic");
            reg!(ling_str_new, "str_new");
            reg!(ling_str_len, "str_len");
            reg!(ling_str_concat, "str_concat");
            reg!(ling_str_eq, "str_eq");
            reg!(ling_list_new, "list_new");
            reg!(ling_list_push, "list_push");
            reg!(ling_list_get, "list_get");
            reg!(ling_list_set, "list_set");
            reg!(ling_list_len, "list_len");
            reg!(ling_struct_new, "struct_new");
            reg!(ling_struct_get, "struct_get");
            reg!(ling_print, "print");
            reg!(ling_print_val, "print_val");
            reg!(ling_print_newline, "print_newline");
            reg!(ling_time_now, "time_now");
            reg!(ling_builtin, "builtin");
        });

        // Prime the fallback interpreter with the SAME flattened program: every
        // function + global is registered (but the entry is not run), so any
        // cranelift-skipped oversized function still executes, with full access
        // to globals and its peer functions.
        let mut fallback = Interpreter::new();
        fallback.source_dir = source_dir;
        fallback
            .register_program(&fallback_ast)
            .map_err(LingError::Mir)?;
        crate::runtime::jit_abi::init(fallback);

        // Compile all functions. A failure here is pre-execution (Codegen),
        // so callers may safely fall back to the interpreter.
        jit.compile(&mir_prog)
            .map_err(|e| LingError::Codegen(e.to_string()))?;

        // Execute main. A failure here happens mid-run (Mir): output may already
        // have been emitted, so callers must not retry on another backend.
        jit.run_main().map_err(|e| LingError::Mir(e.to_string()))?;
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
