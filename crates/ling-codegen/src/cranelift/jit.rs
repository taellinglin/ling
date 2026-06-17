use super::runtime;
use crate::MirProgram;
use anyhow::Result;
use cranelift::codegen::ir::{FuncRef, GlobalValue, InstBuilder, Signature, StackSlotData, StackSlotKind};
use cranelift::prelude::*;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, FuncId, Linkage, Module};
use ling_ast::ast::{BinOp, UnOp};
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

fn int_zero(builder: &mut FunctionBuilder) -> Value {
    builder.ins().iconst(types::I64, 0)
}

fn int_one(builder: &mut FunctionBuilder) -> Value {
    builder.ins().iconst(types::I64, 1)
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
    if let Rvalue::Call { func: callee, .. } = rval {
        if let Operand::Constant(Constant::Function(n)) = callee {
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
            self.translate_function(func);
        }

        self.module.finalize_definitions().unwrap();

        self.functions = program.mir.functions.clone();
        for func in &program.mir.functions {
            self.compiled_names.push(func.name.clone());
        }

        Ok(())
    }

    fn translate_function(&mut self, func: &MirFunction) {
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

        let pred_count = count_predecessors(func);
        let mut sealed = vec![false; func.basic_blocks.len()];
        let mut filled_pred = vec![0u32; func.basic_blocks.len()];

        for bi in 0..func.basic_blocks.len() {
            builder.switch_to_block(blocks[bi]);
            if bi == 0 && !sealed[bi] {
                builder.seal_block(blocks[bi]);
                sealed[bi] = true;
            }
            if bi == 0 {
                builder.append_block_params_for_function_params(blocks[bi]);
                let params: Vec<Value> = builder.block_params(blocks[bi]).to_vec();
                for (j, val) in params.iter().enumerate() {
                    if let Some(&var) = vars.get(&Local(j + 1)) {
                        builder.def_var(var, *val);
                    }
                }
            }

            for stmt in &func.basic_blocks[bi].statements {
                translate_stmt(stmt, &mut builder, &vars, &string_gvs, &builtin_gvs, &runtime_refs, &func_refs);
            }
            if let Some(term) = &func.basic_blocks[bi].terminator {
                translate_terminator(term, &mut builder, &blocks, &vars, &string_gvs, &runtime_refs, &func_refs);
                match &term.kind {
                    TerminatorKind::Goto { target } => {
                        filled_pred[target.0] += 1;
                        if filled_pred[target.0] == pred_count[target.0] && !sealed[target.0] {
                            builder.seal_block(blocks[target.0]);
                            sealed[target.0] = true;
                        }
                    }
                    TerminatorKind::SwitchInt { targets, otherwise, .. } => {
                        for (_, t) in targets {
                            filled_pred[t.0] += 1;
                            if filled_pred[t.0] == pred_count[t.0] && !sealed[t.0] {
                                builder.seal_block(blocks[t.0]);
                                sealed[t.0] = true;
                            }
                        }
                        filled_pred[otherwise.0] += 1;
                        if filled_pred[otherwise.0] == pred_count[otherwise.0] && !sealed[otherwise.0] {
                            builder.seal_block(blocks[otherwise.0]);
                            sealed[otherwise.0] = true;
                        }
                    }
                    _ => {}
                }
            }
        }

        for (i, block) in blocks.iter().enumerate() {
            if !sealed[i] { builder.seal_block(*block); }
        }
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

// ─── Standalone translation functions ─────────────────────────────────────

fn count_predecessors(func: &MirFunction) -> Vec<u32> {
    let mut pred_count = vec![0u32; func.basic_blocks.len()];
    for bb in &func.basic_blocks {
        if let Some(term) = &bb.terminator {
            match &term.kind {
                TerminatorKind::Goto { target } => pred_count[target.0] += 1,
                TerminatorKind::SwitchInt { targets, otherwise, .. } => {
                    for (_, t) in targets { pred_count[t.0] += 1; }
                    pred_count[otherwise.0] += 1;
                }
                _ => {}
            }
        }
    }
    pred_count
}

fn max_local_index(func: &MirFunction) -> usize {
    let mut max = 0usize;
    // Check all locals in statements
    for bb in &func.basic_blocks {
        for stmt in &bb.statements {
            if let StatementKind::Assign(local, rval) = &stmt.kind {
                max = max.max(local.0);
                collect_local_from_rvalue(rval, &mut max);
            }
            if let StatementKind::SetAttr(obj, _, val) = &stmt.kind {
                if let Operand::Copy(l) | Operand::Move(l) = obj { max = max.max(l.0); }
                if let Operand::Copy(l) | Operand::Move(l) = val { max = max.max(l.0); }
            }
            if let StatementKind::SetIndex(obj, idx, val) = &stmt.kind {
                if let Operand::Copy(l) | Operand::Move(l) = obj { max = max.max(l.0); }
                if let Operand::Copy(l) | Operand::Move(l) = idx { max = max.max(l.0); }
                if let Operand::Copy(l) | Operand::Move(l) = val { max = max.max(l.0); }
            }
            if let StatementKind::StorageLive(l) | StatementKind::StorageDead(l) | StatementKind::Drop(l) = &stmt.kind {
                max = max.max(l.0);
            }
        }
        if let Some(term) = &bb.terminator {
            match &term.kind {
                TerminatorKind::SwitchInt { discr, .. } => {
                    if let Operand::Copy(l) | Operand::Move(l) = discr { max = max.max(l.0); }
                }
                _ => {}
            }
        }
    }
    // Ensure at least arg_count + 1 locals (for params + return)
    max = max.max(func.arg_count);
    max
}

fn collect_local_from_rvalue(rval: &Rvalue, max: &mut usize) {
    match rval {
        Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => {
            if let Operand::Copy(l) | Operand::Move(l) = op { *max = (*max).max(l.0); }
        }
        Rvalue::BinaryOp(_, lhs, rhs) => {
            if let Operand::Copy(l) | Operand::Move(l) = lhs { *max = (*max).max(l.0); }
            if let Operand::Copy(l) | Operand::Move(l) = rhs { *max = (*max).max(l.0); }
        }
        Rvalue::Call { args, .. } => {
            for arg in args {
                if let Operand::Copy(l) | Operand::Move(l) = arg { *max = (*max).max(l.0); }
            }
        }
        Rvalue::Aggregate(_, ops) => {
            for op in ops {
                if let Operand::Copy(l) | Operand::Move(l) = op { *max = (*max).max(l.0); }
            }
        }
        Rvalue::GetAttr(op, _) | Rvalue::GetIndex(op, _) => {
            if let Operand::Copy(l) | Operand::Move(l) = op { *max = (*max).max(l.0); }
        }
        Rvalue::Ref(l) | Rvalue::MutRef(l) => {
            *max = (*max).max(l.0);
        }
        _ => {}
    }
}

fn translate_stmt(
    stmt: &Statement,
    builder: &mut FunctionBuilder,
    vars: &HashMap<Local, Variable>,
    string_gvs: &HashMap<String, GlobalValue>,
    builtin_gvs: &HashMap<String, GlobalValue>,
    _runtime_refs: &HashMap<String, FuncRef>,
    func_refs: &HashMap<String, FuncRef>,
) {
    if let StatementKind::Assign(local, rvalue) = &stmt.kind {
        let val = translate_rvalue(rvalue, builder, vars, string_gvs, builtin_gvs, _runtime_refs, func_refs);
        if let Some(&var) = vars.get(local) {
            builder.def_var(var, val);
        }
    }
}

fn translate_rvalue(
    rvalue: &Rvalue,
    builder: &mut FunctionBuilder,
    vars: &HashMap<Local, Variable>,
    string_gvs: &HashMap<String, GlobalValue>,
    builtin_gvs: &HashMap<String, GlobalValue>,
    runtime_refs: &HashMap<String, FuncRef>,
    func_refs: &HashMap<String, FuncRef>,
) -> Value {
    match rvalue {
        Rvalue::Use(op) => translate_op(op, builder, vars, string_gvs, runtime_refs),
        Rvalue::BinaryOp(op, lhs, rhs) => {
            let lv = translate_op(lhs, builder, vars, string_gvs, runtime_refs);
            let rv = translate_op(rhs, builder, vars, string_gvs, runtime_refs);
            translate_binop(op, builder, lv, rv, runtime_refs)
        }
        Rvalue::UnaryOp(op, operand) => {
            let v = translate_op(operand, builder, vars, string_gvs, runtime_refs);
            translate_unop(op, builder, v, runtime_refs)
        }
        Rvalue::Call { func: callee, args } => {
            let callee_name = match callee {
                Operand::Constant(Constant::Function(n)) => n.clone(),
                _ => String::new(),
            };
            let mut cal_args = Vec::new();
            for arg in args {
                cal_args.push(translate_op(arg, builder, vars, string_gvs, runtime_refs));
            }
            if func_refs.contains_key(&callee_name) {
                let &fr = func_refs.get(&callee_name).unwrap();
                let inst = builder.ins().call(fr, &cal_args);
                builder.inst_results(inst)[0]
            } else {
                emit_builtin_call(callee_name, cal_args, builder, builtin_gvs, runtime_refs)
            }
        }
        Rvalue::Aggregate(_, ops) => {
            if ops.is_empty() { int_zero(builder) } else { translate_op(&ops[0], builder, vars, string_gvs, runtime_refs) }
        }
        Rvalue::GetIndex(obj, idx) => {
            let obj_val = translate_op(obj, builder, vars, string_gvs, runtime_refs);
            let idx_val = translate_op(idx, builder, vars, string_gvs, runtime_refs);
            emit_runtime_call2(builder, "__ling_list_get", obj_val, idx_val, runtime_refs)
        }
        _ => int_zero(builder),
    }
}

fn translate_op(
    op: &Operand,
    builder: &mut FunctionBuilder,
    vars: &HashMap<Local, Variable>,
    string_gvs: &HashMap<String, GlobalValue>,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    match op {
        Operand::Copy(l) | Operand::Move(l) => builder.use_var(vars[l]),
        Operand::Constant(c) => match c {
            Constant::I64(v) => {
                let bits = (*v as f64).to_bits();
                builder.ins().iconst(types::I64, bits as i64)
            }
            Constant::F64(v) => {
                builder.ins().iconst(types::I64, *v as i64)
            }
            Constant::Bool(b) => builder.ins().iconst(types::I64, if *b { runtime::TAG_TRUE as i64 } else { runtime::TAG_FALSE as i64 }),
            Constant::Str(s) => {
                if let Some(&gv) = string_gvs.get(s.as_str()) {
                    let ptr = builder.ins().symbol_value(types::I64, gv);
                    let len = builder.ins().iconst(types::I64, s.len() as i64);
                    let fr = *runtime_refs.get("__ling_str_new").unwrap();
                    let inst = builder.ins().call(fr, &[ptr, len]);
                    builder.inst_results(inst)[0]
                } else {
                    int_zero(builder)
                }
            }
            Constant::Function(_) | Constant::GlobalData(_) | Constant::None => {
                builder.ins().iconst(types::I64, runtime::TAG_UNIT as i64)
            }
        },
    }
}

fn translate_binop(
    op: &BinOp,
    builder: &mut FunctionBuilder,
    lv: Value,
    rv: Value,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    match op {
        BinOp::Add => emit_f64_or_runtime(builder, lv, rv, "__ling_add", |b, a, v| b.ins().fadd(a, v), runtime_refs),
        BinOp::Sub => emit_f64_or_runtime(builder, lv, rv, "__ling_sub", |b, a, v| b.ins().fsub(a, v), runtime_refs),
        BinOp::Mul => emit_f64_or_runtime(builder, lv, rv, "__ling_mul", |b, a, v| b.ins().fmul(a, v), runtime_refs),
        BinOp::Div => emit_f64_or_runtime(builder, lv, rv, "__ling_div", |b, a, v| b.ins().fdiv(a, v), runtime_refs),
        BinOp::Rem => emit_f64_or_runtime(builder, lv, rv, "__ling_rem", |b, a, v| {
            let div = b.ins().fdiv(a, v);
            let trunc = b.ins().trunc(div);
            let prod = b.ins().fmul(trunc, v);
            b.ins().fsub(a, prod)
        }, runtime_refs),
        BinOp::Eq => emit_f64_cmp_or_runtime(builder, lv, rv, "__ling_eq", FloatCC::Equal, runtime_refs),
        BinOp::Ne => emit_f64_cmp_or_runtime(builder, lv, rv, "__ling_ne", FloatCC::NotEqual, runtime_refs),
        BinOp::Lt => emit_f64_cmp_or_runtime(builder, lv, rv, "__ling_lt", FloatCC::LessThan, runtime_refs),
        BinOp::Le => emit_f64_cmp_or_runtime(builder, lv, rv, "__ling_le", FloatCC::LessThanOrEqual, runtime_refs),
        BinOp::Gt => emit_f64_cmp_or_runtime(builder, lv, rv, "__ling_gt", FloatCC::GreaterThan, runtime_refs),
        BinOp::Ge => emit_f64_cmp_or_runtime(builder, lv, rv, "__ling_ge", FloatCC::GreaterThanOrEqual, runtime_refs),
        BinOp::And => emit_short_circuit_and(builder, lv, rv, runtime_refs),
        BinOp::Or => emit_short_circuit_or(builder, lv, rv, runtime_refs),
        _ => emit_runtime_call2(builder, "__ling_add", lv, rv, runtime_refs),
    }
}

fn translate_unop(
    op: &UnOp,
    builder: &mut FunctionBuilder,
    v: Value,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    match op {
        UnOp::Ref | UnOp::Deref => v,
        UnOp::Neg => emit_f64_or_runtime(builder, v, v, "__ling_neg", |b, a, _| {
            b.ins().fneg(a)
        }, runtime_refs),
        UnOp::Not => {
            let is_num = emit_is_number(builder, v);
            let block_num = builder.create_block();
            let block_tag = builder.create_block();
            let block_merge = builder.create_block();
            let res_var = builder.declare_var(types::I64);

            builder.ins().brif(is_num, block_num, &[], block_tag, &[]);

            builder.switch_to_block(block_num);
            let f = i64_as_f64(builder, v);
            let zero_f = builder.ins().f64const(0.0);
            let eq_zero = builder.ins().fcmp(FloatCC::Equal, f, zero_f);
            let one = int_one(builder);
            let zero = int_zero(builder);
            let sel = builder.ins().select(eq_zero, one, zero);
            builder.def_var(res_var, sel);
            builder.ins().jump(block_merge, &[]);
            builder.seal_block(block_num);

            builder.switch_to_block(block_tag);
            let rt_ret = emit_runtime_call1(builder, "__ling_not", v, runtime_refs);
            builder.def_var(res_var, rt_ret);
            builder.ins().jump(block_merge, &[]);
            builder.seal_block(block_tag);

            builder.switch_to_block(block_merge);
            builder.seal_block(block_merge);
            builder.use_var(res_var)
        }
    }
}

fn i64_as_f64(builder: &mut FunctionBuilder, v: Value) -> Value {
    builder.ins().bitcast(types::F64, MemFlags::new(), v)
}

fn f64_as_i64(builder: &mut FunctionBuilder, v: Value) -> Value {
    builder.ins().bitcast(types::I64, MemFlags::new(), v)
}

fn emit_f64_or_runtime(
    builder: &mut FunctionBuilder,
    a: Value,
    b: Value,
    runtime_fn: &str,
    f64_op: impl FnOnce(&mut FunctionBuilder, Value, Value) -> Value,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    let is_a_num = emit_is_number(builder, a);
    let is_b_num = emit_is_number(builder, b);
    let both_num = builder.ins().band(is_a_num, is_b_num);
    let block_fast = builder.create_block();
    let block_rt = builder.create_block();
    let block_merge = builder.create_block();
    let res_var = builder.declare_var(types::I64);

    builder.ins().brif(both_num, block_fast, &[], block_rt, &[]);

    builder.switch_to_block(block_fast);
    let fa = i64_as_f64(builder, a);
    let fb = i64_as_f64(builder, b);
    let fres = f64_op(builder, fa, fb);
    let if64 = f64_as_i64(builder, fres);
    builder.def_var(res_var, if64);
    builder.ins().jump(block_merge, &[]);
    builder.seal_block(block_fast);

    builder.switch_to_block(block_rt);
    let rt_ret = emit_runtime_call2(builder, runtime_fn, a, b, runtime_refs);
    builder.def_var(res_var, rt_ret);
    builder.ins().jump(block_merge, &[]);
    builder.seal_block(block_rt);

    builder.switch_to_block(block_merge);
    builder.seal_block(block_merge);
    builder.use_var(res_var)
}

fn emit_f64_cmp_or_runtime(
    builder: &mut FunctionBuilder,
    a: Value,
    b: Value,
    runtime_fn: &str,
    cc: FloatCC,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    let is_a_num = emit_is_number(builder, a);
    let is_b_num = emit_is_number(builder, b);
    let both_num = builder.ins().band(is_a_num, is_b_num);
    let block_fast = builder.create_block();
    let block_rt = builder.create_block();
    let block_merge = builder.create_block();
    let res_var = builder.declare_var(types::I64);

    builder.ins().brif(both_num, block_fast, &[], block_rt, &[]);

    builder.switch_to_block(block_fast);
    let fa = i64_as_f64(builder, a);
    let fb = i64_as_f64(builder, b);
    let cmp = builder.ins().fcmp(cc, fa, fb);
    let t = builder.ins().iconst(types::I64, runtime::TAG_TRUE as i64);
    let f = builder.ins().iconst(types::I64, runtime::TAG_FALSE as i64);
    let sel = builder.ins().select(cmp, t, f);
    builder.def_var(res_var, sel);
    builder.ins().jump(block_merge, &[]);
    builder.seal_block(block_fast);

    builder.switch_to_block(block_rt);
    let rt_ret = emit_runtime_call2(builder, runtime_fn, a, b, runtime_refs);
    builder.def_var(res_var, rt_ret);
    builder.ins().jump(block_merge, &[]);
    builder.seal_block(block_rt);

    builder.switch_to_block(block_merge);
    builder.seal_block(block_merge);
    builder.use_var(res_var)
}

fn emit_short_circuit_and(
    builder: &mut FunctionBuilder,
    lv: Value,
    rv: Value,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    let l_is_truthy = emit_is_truthy(builder, lv, runtime_refs);
    let block_false = builder.create_block();
    let block_true = builder.create_block();
    let block_merge = builder.create_block();
    let res_var = builder.declare_var(types::I64);

    builder.ins().brif(l_is_truthy, block_true, &[], block_false, &[]);

    builder.switch_to_block(block_false);
    let f = builder.ins().iconst(types::I64, runtime::TAG_FALSE as i64);
    builder.def_var(res_var, f);
    builder.ins().jump(block_merge, &[]);
    builder.seal_block(block_false);

    builder.switch_to_block(block_true);
    let r_is_truthy = emit_is_truthy(builder, rv, runtime_refs);
    let t = builder.ins().iconst(types::I64, runtime::TAG_TRUE as i64);
    let f2 = builder.ins().iconst(types::I64, runtime::TAG_FALSE as i64);
    let sel = builder.ins().select(r_is_truthy, t, f2);
    builder.def_var(res_var, sel);
    builder.ins().jump(block_merge, &[]);
    builder.seal_block(block_true);

    builder.switch_to_block(block_merge);
    builder.seal_block(block_merge);
    builder.use_var(res_var)
}

fn emit_short_circuit_or(
    builder: &mut FunctionBuilder,
    lv: Value,
    rv: Value,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    let l_is_truthy = emit_is_truthy(builder, lv, runtime_refs);
    let block_true = builder.create_block();
    let block_false = builder.create_block();
    let block_merge = builder.create_block();
    let res_var = builder.declare_var(types::I64);

    builder.ins().brif(l_is_truthy, block_true, &[], block_false, &[]);

    builder.switch_to_block(block_true);
    let t = builder.ins().iconst(types::I64, runtime::TAG_TRUE as i64);
    builder.def_var(res_var, t);
    builder.ins().jump(block_merge, &[]);
    builder.seal_block(block_true);

    builder.switch_to_block(block_false);
    let r_is_truthy = emit_is_truthy(builder, rv, runtime_refs);
    let t2 = builder.ins().iconst(types::I64, runtime::TAG_TRUE as i64);
    let f = builder.ins().iconst(types::I64, runtime::TAG_FALSE as i64);
    let sel = builder.ins().select(r_is_truthy, t2, f);
    builder.def_var(res_var, sel);
    builder.ins().jump(block_merge, &[]);
    builder.seal_block(block_false);

    builder.switch_to_block(block_merge);
    builder.seal_block(block_merge);
    builder.use_var(res_var)
}

fn emit_is_number(builder: &mut FunctionBuilder, val: Value) -> Value {
    let shifted = builder.ins().ushr_imm(val, 56);
    let tag = builder.ins().iconst(types::I64, 0x7F);
    builder.ins().icmp(IntCC::NotEqual, shifted, tag)
}

fn emit_is_truthy(
    builder: &mut FunctionBuilder,
    val: Value,
    _runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    let is_num = emit_is_number(builder, val);
    let block_num = builder.create_block();
    let block_tag = builder.create_block();
    let block_merge = builder.create_block();
    let res_var = builder.declare_var(types::I64);

    builder.ins().brif(is_num, block_num, &[], block_tag, &[]);

    builder.switch_to_block(block_num);
    let f = i64_as_f64(builder, val);
    let zero = builder.ins().f64const(0.0);
    let is_nonzero = builder.ins().fcmp(FloatCC::NotEqual, f, zero);
    let one = int_one(builder);
    let zero2 = int_zero(builder);
    let sel = builder.ins().select(is_nonzero, one, zero2);
    builder.def_var(res_var, sel);
    builder.ins().jump(block_merge, &[]);
    builder.seal_block(block_num);

    builder.switch_to_block(block_tag);
    let is_true = builder.ins().icmp_imm(IntCC::Equal, val, runtime::TAG_TRUE as i64);
    let is_false = builder.ins().icmp_imm(IntCC::Equal, val, runtime::TAG_FALSE as i64);
    let is_unit = builder.ins().icmp_imm(IntCC::Equal, val, runtime::TAG_UNIT as i64);
    // Truthy = is_true || (!is_false && !is_unit)
    let is_false_or_unit = builder.ins().bor(is_false, is_unit);
    let one_i64 = int_one(builder);
    let zero_i64 = int_zero(builder);
    let is_non_nil = builder.ins().select(is_false_or_unit, zero_i64, one_i64);
    let is_true_i64 = builder.ins().select(is_true, one_i64, zero_i64);
    let result_i64 = builder.ins().bor(is_true_i64, is_non_nil);
    builder.def_var(res_var, result_i64);
    builder.ins().jump(block_merge, &[]);
    builder.seal_block(block_tag);

    builder.switch_to_block(block_merge);
    builder.seal_block(block_merge);
    builder.use_var(res_var)
}

fn emit_runtime_call0(
    builder: &mut FunctionBuilder,
    name: &str,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    let fr = *runtime_refs.get(name).unwrap_or_else(|| panic!("runtime fn not found: {}", name));
    let inst = builder.ins().call(fr, &[]);
    builder.inst_results(inst)[0]
}

fn emit_runtime_call1(
    builder: &mut FunctionBuilder,
    name: &str,
    a: Value,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    let fr = *runtime_refs.get(name).unwrap_or_else(|| panic!("runtime fn not found: {}", name));
    let inst = builder.ins().call(fr, &[a]);
    builder.inst_results(inst)[0]
}

fn emit_runtime_call2(
    builder: &mut FunctionBuilder,
    name: &str,
    a: Value,
    b: Value,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    let fr = *runtime_refs.get(name).unwrap_or_else(|| panic!("runtime fn not found: {}", name));
    let inst = builder.ins().call(fr, &[a, b]);
    builder.inst_results(inst)[0]
}

fn emit_runtime_call4(
    builder: &mut FunctionBuilder,
    name: &str,
    a: Value,
    b: Value,
    c: Value,
    d: Value,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    let fr = *runtime_refs.get(name).unwrap_or_else(|| panic!("runtime fn not found: {}", name));
    let inst = builder.ins().call(fr, &[a, b, c, d]);
    builder.inst_results(inst)[0]
}

fn emit_builtin_call(
    name: String,
    args: Vec<Value>,
    builder: &mut FunctionBuilder,
    builtin_gvs: &HashMap<String, GlobalValue>,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    // Fast-path for commonly-used builtins with direct JIT implementations
    match name.as_str() {
        "print" | "println" | "พิมพ์" | "印" | "打印" | "印刷" => {
            if !args.is_empty() {
                for arg in &args[..args.len()-1] {
                    emit_runtime_call1(builder, "__ling_print_val", *arg, runtime_refs);
                }
                emit_runtime_call1(builder, "__ling_print_val", args[args.len()-1], runtime_refs);
            }
            return emit_runtime_call0(builder, "__ling_print_newline", runtime_refs);
        }
        "sin" => return unbox_f64_or_call(builder, args, "__ling_sin", runtime_refs),
        "cos" => return unbox_f64_or_call(builder, args, "__ling_cos", runtime_refs),
        "sqrt" => return unbox_f64_or_call(builder, args, "__ling_sqrt", runtime_refs),
        "abs" => return unbox_f64_or_call(builder, args, "__ling_abs", runtime_refs),
        "floor" => return unbox_f64_or_call(builder, args, "__ling_floor", runtime_refs),
        "ceil" => return unbox_f64_or_call(builder, args, "__ling_ceil", runtime_refs),
        "round" => return unbox_f64_or_call(builder, args, "__ling_round", runtime_refs),
        "time_now" | "เวลาปัจจุบัน" | "当前时间" | "経過時間" | "현재시간" => {
            return emit_runtime_call0(builder, "__ling_time_now", runtime_refs);
        }
        "len" | "str_len" | "ความยาว" | "长度" | "長さ" | "길이" => {
            if !args.is_empty() { return emit_runtime_call1(builder, "__ling_str_len", args[0], runtime_refs); }
            else { return builder.ins().iconst(types::I64, runtime::TAG_UNIT as i64); }
        }
        _ => {}
    }
    // Fallback: dispatch through __ling_builtin for any builtin not handled above
    if let Some(&name_gv) = builtin_gvs.get(&name) {
        let name_ptr = builder.ins().symbol_value(types::I64, name_gv);
        let name_len = builder.ins().iconst(types::I64, name.len() as i64);
        let num_args = args.len();
        // Always allocate a stack slot (even for 0 args) so args_ptr is valid
        let slot_size = std::cmp::max(num_args * 8, 8) as u32;
        let args_slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, slot_size, 8));
        let args_ptr = builder.ins().stack_addr(types::I64, args_slot, 0);
        for (i, arg) in args.iter().enumerate() {
            let off = builder.ins().iconst(types::I64, (i * 8) as i64);
            let elem_ptr = builder.ins().iadd(args_ptr, off);
            builder.ins().store(MemFlags::new(), *arg, elem_ptr, 0);
        }
        let args_len = builder.ins().iconst(types::I64, num_args as i64);
        emit_runtime_call4(builder, "__ling_builtin", name_ptr, name_len, args_ptr, args_len, runtime_refs)
    } else {
        builder.ins().iconst(types::I64, runtime::TAG_UNIT as i64)
    }
}

fn unbox_f64_or_call(
    builder: &mut FunctionBuilder,
    args: Vec<Value>,
    runtime_fn: &str,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    if args.is_empty() {
        return builder.ins().iconst(types::I64, runtime::TAG_UNIT as i64);
    }
    let val = args[0];
    let is_num = emit_is_number(builder, val);
    let block_fast = builder.create_block();
    let block_slow = builder.create_block();
    let block_merge = builder.create_block();
    let res_var = builder.declare_var(types::I64);

    builder.ins().brif(is_num, block_fast, &[], block_slow, &[]);

    builder.switch_to_block(block_fast);
    let f = i64_as_f64(builder, val);
    let fr = *runtime_refs.get(runtime_fn).unwrap_or_else(|| panic!("runtime fn not found: {}", runtime_fn));
    let inst = builder.ins().call(fr, &[f]);
    let f_result = builder.inst_results(inst)[0];
    let if64 = f64_as_i64(builder, f_result);
    builder.def_var(res_var, if64);
    builder.ins().jump(block_merge, &[]);
    builder.seal_block(block_fast);

    builder.switch_to_block(block_slow);
    builder.def_var(res_var, val);
    builder.ins().jump(block_merge, &[]);
    builder.seal_block(block_slow);

    builder.switch_to_block(block_merge);
    builder.seal_block(block_merge);
    builder.use_var(res_var)
}

fn translate_terminator(
    term: &Terminator,
    builder: &mut FunctionBuilder,
    blocks: &[Block],
    vars: &HashMap<Local, Variable>,
    string_gvs: &HashMap<String, GlobalValue>,
    runtime_refs: &HashMap<String, FuncRef>,
    _func_refs: &HashMap<String, FuncRef>,
) {
    match &term.kind {
        TerminatorKind::Goto { target } => { builder.ins().jump(blocks[target.0], &[]); }
        TerminatorKind::SwitchInt { discr, targets, otherwise } => {
            let val = translate_op(discr, builder, vars, string_gvs, runtime_refs);
            let is_truthy = emit_is_truthy(builder, val, runtime_refs);
            let mut true_target = otherwise.0;
            let mut false_target = otherwise.0;
            for (const_val, target_block) in targets {
                let cv = *const_val as i64;
                if cv == 1 { true_target = target_block.0; }
                else if cv == 0 { false_target = target_block.0; }
            }
            if true_target != otherwise.0 && false_target != otherwise.0 {
                builder.ins().brif(is_truthy, blocks[true_target], &[], blocks[false_target], &[]);
            } else if true_target != otherwise.0 {
                builder.ins().brif(is_truthy, blocks[true_target], &[], blocks[otherwise.0], &[]);
            } else {
                builder.ins().jump(blocks[otherwise.0], &[]);
            }
        }
        TerminatorKind::Return => {
            let ret = builder.use_var(vars[&Local(0)]);
            builder.ins().return_(&[ret]);
        }
        TerminatorKind::Unreachable => { builder.ins().trap(TrapCode::INTEGER_OVERFLOW); }
    }
}
