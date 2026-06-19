use super::numtype;
use super::translate::{build_function_body, max_local_index, TransCtx};
use crate::CodegenBackend;
use crate::MirProgram;
use anyhow::Result;
use cranelift::codegen::ir::{FuncRef, GlobalValue};
use cranelift::prelude::*;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::DataId;
use cranelift_module::{DataDescription, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use ling_mir::ir::*;
use std::collections::HashMap;

// ─── AOT Backend ───────────────────────────────────────────────────────────

pub struct CraneliftBackend {
    module: Option<ObjectModule>,
    builder_ctx: FunctionBuilderContext,
}

// ─── Runtime Function Declarations ─────────────────────────────────────────

struct RuntimeDecl {
    id: FuncId,
}

fn declare_runtime_functions(module: &mut ObjectModule) -> HashMap<String, RuntimeDecl> {
    let mut decls = HashMap::new();

    let runtime_fns: &[(&str, &[types::Type], types::Type)] = &[
        ("ling_f64_add", &[types::F64, types::F64], types::F64),
        ("ling_f64_sub", &[types::F64, types::F64], types::F64),
        ("ling_f64_mul", &[types::F64, types::F64], types::F64),
        ("ling_f64_div", &[types::F64, types::F64], types::F64),
        ("ling_f64_rem", &[types::F64, types::F64], types::F64),
        ("ling_f64_neg", &[types::F64], types::F64),
        ("ling_f64_eq", &[types::F64, types::F64], types::I64),
        ("ling_f64_lt", &[types::F64, types::F64], types::I64),
        ("ling_f64_gt", &[types::F64, types::F64], types::I64),
        ("ling_f64_le", &[types::F64, types::F64], types::I64),
        ("ling_f64_ge", &[types::F64, types::F64], types::I64),
        ("ling_sin", &[types::F64], types::F64),
        ("ling_cos", &[types::F64], types::F64),
        ("ling_sqrt", &[types::F64], types::F64),
        ("ling_abs", &[types::F64], types::F64),
        ("ling_floor", &[types::F64], types::F64),
        ("ling_ceil", &[types::F64], types::F64),
        ("ling_round", &[types::F64], types::F64),
        ("ling_add", &[types::I64, types::I64], types::I64),
        ("ling_sub", &[types::I64, types::I64], types::I64),
        ("ling_mul", &[types::I64, types::I64], types::I64),
        ("ling_div", &[types::I64, types::I64], types::I64),
        ("ling_rem", &[types::I64, types::I64], types::I64),
        ("ling_neg", &[types::I64, types::I64], types::I64),
        ("ling_eq", &[types::I64, types::I64], types::I64),
        ("ling_ne", &[types::I64, types::I64], types::I64),
        ("ling_lt", &[types::I64, types::I64], types::I64),
        ("ling_le", &[types::I64, types::I64], types::I64),
        ("ling_gt", &[types::I64, types::I64], types::I64),
        ("ling_ge", &[types::I64, types::I64], types::I64),
        ("ling_and", &[types::I64, types::I64], types::I64),
        ("ling_or", &[types::I64, types::I64], types::I64),
        ("ling_not", &[types::I64], types::I64),
        ("ling_bool_to_u64", &[types::I64], types::I64),
        ("ling_alloc", &[types::I64], types::I64),
        ("ling_free", &[types::I64], types::I64),
        ("ling_panic", &[types::I64], types::I64),
        ("ling_str_new", &[types::I64, types::I64], types::I64),
        ("ling_str_len", &[types::I64], types::I64),
        ("ling_str_concat", &[types::I64, types::I64], types::I64),
        ("ling_str_eq", &[types::I64, types::I64], types::I64),
        ("ling_list_new", &[], types::I64),
        ("ling_list_push", &[types::I64, types::I64], types::I64),
        ("ling_list_get", &[types::I64, types::I64], types::I64),
        ("ling_list_len", &[types::I64], types::I64),
        ("ling_struct_new", &[types::I64, types::I64, types::I64, types::I64], types::I64),
        ("ling_struct_get", &[types::I64, types::I64, types::I64], types::I64),
        ("ling_print", &[types::I64], types::I64),
        ("ling_print_val", &[types::I64], types::I64),
        ("ling_print_newline", &[], types::I64),
        ("ling_time_now", &[], types::I64),
        ("ling_builtin", &[types::I64, types::I64, types::I64, types::I64], types::I64),
    ];

    for &(name, params, ret) in runtime_fns {
        let mut sig = module.make_signature();
        for &pt in params {
            sig.params.push(AbiParam::new(pt));
        }
        sig.returns.push(AbiParam::new(ret));
        let id = module.declare_function(name, Linkage::Import, &sig).unwrap();
        decls.insert(name.to_string(), RuntimeDecl { id });
    }

    decls
}

// ─── String/Builtin name collection ─────────────────────────────────────────

fn collect_string_constants(
    functions: &[MirFunction],
    module: &mut ObjectModule,
) -> (HashMap<String, DataId>, HashMap<String, DataId>) {
    let mut string_ids: HashMap<String, DataId> = HashMap::new();
    let mut builtin_ids: HashMap<String, DataId> = HashMap::new();
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
    module: &mut ObjectModule,
    string_ids: &mut HashMap<String, DataId>,
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
    module: &mut ObjectModule,
    builtin_ids: &mut HashMap<String, DataId>,
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
    module: &mut ObjectModule,
    string_ids: &mut HashMap<String, DataId>,
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
    module: &mut ObjectModule,
    string_ids: &mut HashMap<String, DataId>,
) {
    if let TerminatorKind::SwitchInt { discr, .. } = &term.kind {
        visit_operand_strings(discr, module, string_ids);
    }
}

// ─── Main AOT compilation ───────────────────────────────────────────────────

impl CraneliftBackend {
    pub fn new() -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("is_pic", "true").unwrap();
        flag_builder.set("opt_level", "speed").unwrap();
        flag_builder.set("enable_alias_analysis", "true").unwrap();
        flag_builder.set("enable_verifier", "false").unwrap();
        let isa_builder = cranelift::native::builder()
            .unwrap_or_else(|_| isa::lookup_by_name("aarch64").unwrap());
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap();
        let obj_builder = ObjectBuilder::new(
            isa,
            "ling_program",
            cranelift_module::default_libcall_names(),
        )
        .expect("ObjectBuilder");
        let module = ObjectModule::new(obj_builder);
        Self {
            module: Some(module),
            builder_ctx: FunctionBuilderContext::new(),
        }
    }
}

impl CodegenBackend for CraneliftBackend {
    fn emit(&mut self, program: &MirProgram, out: &std::path::Path) -> Result<()> {
        let module: &mut ObjectModule = self.module.as_mut().unwrap();

        let num_types = numtype::analyze(&program.mir.functions);

        // Phase 0: declare all runtime functions as imports
        let runtime_decls = declare_runtime_functions(module);

        // Phase 1: collect and declare string/builtin data objects
        let (string_ids, builtin_ids) = collect_string_constants(&program.mir.functions, module);

        // Phase 2: declare all user functions as exports
        let mut func_ids: HashMap<String, FuncId> = HashMap::new();
        for func in &program.mir.functions {
            let mut sig = module.make_signature();
            for _ in 0..func.arg_count {
                sig.params.push(AbiParam::new(types::I64));
            }
            sig.returns.push(AbiParam::new(types::I64));
            let id = module.declare_function(&func.name, Linkage::Export, &sig).unwrap();
            func_ids.insert(func.name.clone(), id);
        }

        // Phase 3: translate each function body
        for func in &program.mir.functions {
            let &fid = func_ids.get(&func.name).unwrap();

            let mut ctx = module.make_context();
            let mut sig = module.make_signature();
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

            // Build string GlobalValue map for this function
            let mut string_gvs: HashMap<String, GlobalValue> = HashMap::new();
            for (s, &data_id) in &string_ids {
                let gv = module.declare_data_in_func(data_id, builder.func);
                string_gvs.insert(s.clone(), gv);
            }

            // Build builtin name GlobalValue map for this function
            let mut builtin_gvs: HashMap<String, GlobalValue> = HashMap::new();
            for (s, &data_id) in &builtin_ids {
                let gv = module.declare_data_in_func(data_id, builder.func);
                builtin_gvs.insert(s.clone(), gv);
            }

            // Build runtime FuncRef map for this function. The shared translator
            // looks up runtime helpers by their JIT symbol name (`__ling_*`); the
            // object backend declares them without that prefix, so re-key here.
            let mut runtime_refs: HashMap<String, FuncRef> = HashMap::new();
            for (name, decl) in &runtime_decls {
                let fr = module.declare_func_in_func(decl.id, builder.func);
                runtime_refs.insert(format!("__{name}"), fr);
            }

            // FuncRefs are function-local, so user-function references must be
            // declared into this builder (not shared across functions).
            let mut func_refs: HashMap<String, FuncRef> = HashMap::new();
            for (name, &id) in &func_ids {
                let fr = module.declare_func_in_func(id, builder.func);
                func_refs.insert(name.clone(), fr);
            }

            let tctx = TransCtx {
                vars: &vars,
                string_gvs: &string_gvs,
                builtin_gvs: &builtin_gvs,
                runtime_refs: &runtime_refs,
                func_refs: &func_refs,
                nt: &num_types,
                fname: &func.name,
            };
            build_function_body(&mut builder, func, &blocks, &tctx);
            builder.finalize();
            module.define_function(fid, &mut ctx).unwrap();
        }

        // Phase 4: finalize and emit .o file
        let obj = self.module.take().unwrap().finish();
        let bytes = obj.emit().map_err(|e| anyhow::anyhow!("{:?}", e))?;
        std::fs::write(out, bytes)?;
        Ok(())
    }
}

impl Default for CraneliftBackend {
    fn default() -> Self {
        Self::new()
    }
}
