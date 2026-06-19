use super::numtype::{self, NumberTypes};
use super::runtime;
use super::translate::{build_function_body, max_local_index, TransCtx};
use crate::MirProgram;
use anyhow::Result;
use cranelift::codegen::ir::{FuncRef, GlobalValue, Signature};
use cranelift::prelude::*;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, FuncId, Linkage, Module};
use ling_mir::ir::*;
use std::collections::HashMap;

// ─── JIT Backend ────────────────────────────────────────────────────────────

pub struct JitBackend {
    module: JITModule,
    builder_ctx: FunctionBuilderContext,
    func_ids: HashMap<String, FuncId>,
    runtime_sigs: HashMap<String, (FuncId, Signature)>,
    string_data_ids: HashMap<String, cranelift_module::DataId>,
    builtin_data_ids: HashMap<String, cranelift_module::DataId>,
    functions: Vec<MirFunction>,
    compiled_names: Vec<String>,
}

// ─── Runtime function declarations ──────────────────────────────────────────

fn declare_runtime_functions(module: &mut JITModule) -> HashMap<String, (FuncId, Signature)> {
    use cranelift::codegen::ir::AbiParam;

    let mut sigs = HashMap::new();
    let runtime_names: &[(&str, &[types::Type], types::Type)] = &[
        ("__ling_f64_add", &[types::F64, types::F64], types::F64),
        ("__ling_f64_sub", &[types::F64, types::F64], types::F64),
        ("__ling_f64_mul", &[types::F64, types::F64], types::F64),
        ("__ling_f64_div", &[types::F64, types::F64], types::F64),
        ("__ling_f64_rem", &[types::F64, types::F64], types::F64),
        ("__ling_f64_neg", &[types::F64], types::F64),
        ("__ling_f64_eq", &[types::F64, types::F64], types::I64),
        ("__ling_f64_lt", &[types::F64, types::F64], types::I64),
        ("__ling_f64_gt", &[types::F64, types::F64], types::I64),
        ("__ling_f64_le", &[types::F64, types::F64], types::I64),
        ("__ling_f64_ge", &[types::F64, types::F64], types::I64),
        ("__ling_sin", &[types::F64], types::F64),
        ("__ling_cos", &[types::F64], types::F64),
        ("__ling_sqrt", &[types::F64], types::F64),
        ("__ling_abs", &[types::F64], types::F64),
        ("__ling_floor", &[types::F64], types::F64),
        ("__ling_ceil", &[types::F64], types::F64),
        ("__ling_round", &[types::F64], types::F64),
        ("__ling_add", &[types::I64, types::I64], types::I64),
        ("__ling_sub", &[types::I64, types::I64], types::I64),
        ("__ling_mul", &[types::I64, types::I64], types::I64),
        ("__ling_div", &[types::I64, types::I64], types::I64),
        ("__ling_rem", &[types::I64, types::I64], types::I64),
        ("__ling_neg", &[types::I64, types::I64], types::I64),
        ("__ling_eq", &[types::I64, types::I64], types::I64),
        ("__ling_ne", &[types::I64, types::I64], types::I64),
        ("__ling_lt", &[types::I64, types::I64], types::I64),
        ("__ling_le", &[types::I64, types::I64], types::I64),
        ("__ling_gt", &[types::I64, types::I64], types::I64),
        ("__ling_ge", &[types::I64, types::I64], types::I64),
        ("__ling_and", &[types::I64, types::I64], types::I64),
        ("__ling_or", &[types::I64, types::I64], types::I64),
        ("__ling_not", &[types::I64], types::I64),
        ("__ling_bool_to_u64", &[types::I64], types::I64),
        ("__ling_alloc", &[types::I64], types::I64),
        ("__ling_free", &[types::I64], types::I64),
        ("__ling_panic", &[types::I64], types::I64),
        ("__ling_str_new", &[types::I64, types::I64], types::I64),
        ("__ling_str_len", &[types::I64], types::I64),
        ("__ling_str_concat", &[types::I64, types::I64], types::I64),
        ("__ling_str_eq", &[types::I64, types::I64], types::I64),
        ("__ling_list_new", &[], types::I64),
        ("__ling_list_push", &[types::I64, types::I64], types::I64),
        ("__ling_list_get", &[types::I64, types::I64], types::I64),
        ("__ling_list_len", &[types::I64], types::I64),
        ("__ling_struct_new", &[types::I64, types::I64, types::I64, types::I64], types::I64),
        ("__ling_struct_get", &[types::I64, types::I64, types::I64], types::I64),
        ("__ling_print", &[types::I64], types::I64),
        ("__ling_print_val", &[types::I64], types::I64),
        ("__ling_print_newline", &[], types::I64),
        ("__ling_time_now", &[], types::I64),
        ("__ling_builtin", &[types::I64, types::I64, types::I64, types::I64], types::I64),
    ];
    for &(name, params, ret) in runtime_names {
        let mut sig = module.make_signature();
        for &pt in params {
            sig.params.push(AbiParam::new(pt));
        }
        sig.returns.push(AbiParam::new(ret));
        let id = module.declare_function(name, Linkage::Import, &sig).unwrap();
        sigs.insert(name.to_string(), (id, sig));
    }
    sigs
}

// ─── String constant collection ──────────────────────────────────────────

fn collect_strings(
    functions: &[MirFunction],
    module: &mut JITModule,
) -> (HashMap<String, cranelift_module::DataId>, HashMap<String, cranelift_module::DataId>) {
    let mut string_ids: HashMap<String, cranelift_module::DataId> = HashMap::new();
    let mut builtin_ids: HashMap<String, cranelift_module::DataId> = HashMap::new();
    for func in functions {
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(_, rval) = &stmt.kind {
                    visit_rvalue_strings(rval, module, &mut string_ids);
                    visit_rvalue_builtin_names(rval, module, &mut builtin_ids);
                }
            }
            if let Some(term) = &bb.terminator {
                visit_term_strings(term, module, &mut string_ids);
            }
        }
    }
    (string_ids, builtin_ids)
}

fn visit_operand_strings(
    op: &Operand,
    module: &mut JITModule,
    string_ids: &mut HashMap<String, cranelift_module::DataId>,
) {
    if let Operand::Constant(Constant::Str(s)) = op {
        if !string_ids.contains_key(s) {
            let name = format!("__str_{}", string_ids.len());
            let data_id = module.declare_data(&name, Linkage::Local, true, false).unwrap();
            let mut desc = DataDescription::new();
            desc.define(s.as_bytes().to_vec().into_boxed_slice());
            desc.set_align(1);
            module.define_data(data_id, &desc).unwrap();
            string_ids.insert(s.clone(), data_id);
        }
    }
}

fn visit_rvalue_builtin_names(
    rval: &Rvalue,
    module: &mut JITModule,
    builtin_ids: &mut HashMap<String, cranelift_module::DataId>,
) {
    if let Rvalue::Call { func: Operand::Constant(Constant::Function(n)), .. } = rval {
        if !builtin_ids.contains_key(n) {
            let name = format!("__builtin_{}", builtin_ids.len());
            let data_id = module.declare_data(&name, Linkage::Local, true, false).unwrap();
            let mut desc = DataDescription::new();
            let mut bytes = n.as_bytes().to_vec();
            bytes.push(0);
            desc.define(bytes.into_boxed_slice());
            desc.set_align(1);
            module.define_data(data_id, &desc).unwrap();
            builtin_ids.insert(n.clone(), data_id);
        }
    }
}

fn visit_rvalue_strings(
    rval: &Rvalue,
    module: &mut JITModule,
    string_ids: &mut HashMap<String, cranelift_module::DataId>,
) {
    match rval {
        Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => visit_operand_strings(op, module, string_ids),
        Rvalue::BinaryOp(_, lhs, rhs) => {
            visit_operand_strings(lhs, module, string_ids);
            visit_operand_strings(rhs, module, string_ids);
        }
        Rvalue::Call { args, .. } => {
            for arg in args { visit_operand_strings(arg, module, string_ids); }
        }
        Rvalue::Aggregate(_, ops) => {
            for op in ops { visit_operand_strings(op, module, string_ids); }
        }
        _ => {}
    }
}

fn visit_term_strings(
    term: &Terminator,
    module: &mut JITModule,
    string_ids: &mut HashMap<String, cranelift_module::DataId>,
) {
    if let TerminatorKind::SwitchInt { discr, .. } = &term.kind {
        visit_operand_strings(discr, module, string_ids);
    }
}

impl JitBackend {
    /// Create a new JIT backend. Symbols can be registered via `register_symbols_fn`.
    pub fn new<F>(register_symbols_fn: F) -> Self
    where
        F: FnOnce(&mut JITBuilder),
    {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "false").unwrap();
        flag_builder.set("opt_level", "speed").unwrap();
        flag_builder.set("enable_alias_analysis", "true").unwrap();
        flag_builder.set("enable_verifier", "false").unwrap();

        let isa_builder = cranelift_native::builder().unwrap_or_else(|msg| {
            panic!("host architecture not supported: {msg}")
        });
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap_or_else(|msg| panic!("host architecture not supported: {msg}"));

        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        register_symbols_fn(&mut builder);
        let module = JITModule::new(builder);

        Self {
            module,
            builder_ctx: FunctionBuilderContext::new(),
            func_ids: HashMap::new(),
            runtime_sigs: HashMap::new(),
            string_data_ids: HashMap::new(),
            builtin_data_ids: HashMap::new(),
            functions: Vec::new(),
            compiled_names: Vec::new(),
        }
    }

    /// Compile all functions in the MIR program into JIT memory.
    pub fn compile(&mut self, program: &MirProgram) -> Result<()> {
        let num_types = numtype::analyze(&program.mir.functions);
        self.runtime_sigs = declare_runtime_functions(&mut self.module);

        let (string_ids, builtin_ids) = collect_strings(&program.mir.functions, &mut self.module);
        self.string_data_ids = string_ids;
        self.builtin_data_ids = builtin_ids;

        for func in &program.mir.functions {
            let mut sig = self.module.make_signature();
            for _ in 0..func.arg_count {
                sig.params.push(AbiParam::new(types::I64));
            }
            sig.returns.push(AbiParam::new(types::I64));
            let id = self.module.declare_function(&func.name, Linkage::Export, &sig).unwrap();
            self.func_ids.insert(func.name.clone(), id);
        }

        for func in &program.mir.functions {
            self.translate_function(func, &num_types);
        }

        self.module.finalize_definitions().unwrap();

        self.functions = program.mir.functions.clone();
        for func in &program.mir.functions {
            self.compiled_names.push(func.name.clone());
        }

        Ok(())
    }

    fn translate_function(&mut self, func: &MirFunction, nt: &NumberTypes) {
        let &fid = self.func_ids.get(&func.name).unwrap();
        let mut ctx = self.module.make_context();
        let mut sig = self.module.make_signature();
        for _ in 0..func.arg_count {
            sig.params.push(AbiParam::new(types::I64));
        }
        sig.returns.push(AbiParam::new(types::I64));
        ctx.func.signature = sig;

        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut self.builder_ctx);
        let blocks: Vec<Block> = func.basic_blocks.iter().map(|_| builder.create_block()).collect();
        let max_local = max_local_index(func);
        let mut vars: HashMap<Local, Variable> = HashMap::new();
        for i in 0..=max_local {
            vars.insert(Local(i), builder.declare_var(types::I64));
        }

        let mut string_gvs: HashMap<String, GlobalValue> = HashMap::new();
        for (s, &data_id) in &self.string_data_ids {
            let gv = self.module.declare_data_in_func(data_id, builder.func);
            string_gvs.insert(s.clone(), gv);
        }
        let mut builtin_gvs: HashMap<String, GlobalValue> = HashMap::new();
        for (s, &data_id) in &self.builtin_data_ids {
            let gv = self.module.declare_data_in_func(data_id, builder.func);
            builtin_gvs.insert(s.clone(), gv);
        }

        let mut runtime_refs: HashMap<String, FuncRef> = HashMap::new();
        for (name, (id, _sig)) in &self.runtime_sigs {
            let fr = self.module.declare_func_in_func(*id, builder.func);
            runtime_refs.insert(name.clone(), fr);
        }
        let mut func_refs: HashMap<String, FuncRef> = HashMap::new();
        for (name, &id) in &self.func_ids {
            let fr = self.module.declare_func_in_func(id, builder.func);
            func_refs.insert(name.clone(), fr);
        }

        let tctx = TransCtx {
            vars: &vars,
            string_gvs: &string_gvs,
            builtin_gvs: &builtin_gvs,
            runtime_refs: &runtime_refs,
            func_refs: &func_refs,
            nt,
            fname: &func.name,
        };
        build_function_body(&mut builder, func, &blocks, &tctx);
        builder.finalize();
        self.module.define_function(fid, &mut ctx).unwrap();
    }

    pub fn get_function(&mut self, name: &str) -> Option<*const u8> {
        let func_id = self.func_ids.get(name)?;
        Some(self.module.get_finalized_function(*func_id))
    }

    pub fn run_main(&mut self) -> Result<u64> {
        let main_name = self.compiled_names.iter()
            .find(|n| n.as_str() == "__main__" || n.as_str() == "main" || n.as_str() == "start" || n.as_str() == "เริ่ม")
            .cloned()
            .unwrap_or_else(|| self.compiled_names.first().cloned().unwrap_or_default());
        if main_name.is_empty() { return Ok(runtime::TAG_UNIT); }
        match self.get_function(&main_name) {
            Some(ptr) => {
                let func: unsafe extern "C" fn() -> u64 = unsafe { std::mem::transmute(ptr) };
                Ok(unsafe { func() })
            }
            None => Ok(runtime::TAG_UNIT),
        }
    }

    pub fn run_function(&mut self, name: &str, args: &[u64]) -> Result<u64> {
        let fn_ptr = match self.get_function(name) { Some(p) => p, None => return Ok(runtime::TAG_UNIT) };
        unsafe {
            match args.len() {
                0 => { let f: unsafe extern "C" fn() -> u64 = std::mem::transmute(fn_ptr); Ok(f()) }
                1 => { let f: unsafe extern "C" fn(u64) -> u64 = std::mem::transmute(fn_ptr); Ok(f(args[0])) }
                2 => { let f: unsafe extern "C" fn(u64, u64) -> u64 = std::mem::transmute(fn_ptr); Ok(f(args[0], args[1])) }
                3 => { let f: unsafe extern "C" fn(u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr); Ok(f(args[0], args[1], args[2])) }
                n => { let f: unsafe extern "C" fn(*const u64, usize) -> u64 = std::mem::transmute(fn_ptr); Ok(f(args.as_ptr(), n)) }
            }
        }
    }
}
