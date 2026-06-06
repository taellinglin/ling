use crate::CodegenBackend;
use crate::MirProgram;
use anyhow::Result;
use cranelift::codegen::ir::{FuncRef, GlobalValue};
use cranelift::prelude::*;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{DataDescription, Linkage, Module};
use cranelift_module::DataId;
use cranelift_object::{ObjectBuilder, ObjectModule};
use ling_ast::ast::BinOp;
use ling_ast::ast::UnOp;
use ling_mir::ir::*;
use std::collections::HashMap;

pub struct CraneliftBackend {
    module: Option<ObjectModule>,
    builder_ctx: FunctionBuilderContext,
    string_data: HashMap<String, DataId>,
}

fn f64_to_i64(builder: &mut FunctionBuilder, v: f64) -> Value {
    builder.ins().iconst(types::I64, v.to_bits() as i64)
}

fn i64_to_f64(builder: &mut FunctionBuilder, v: Value) -> Value {
    builder.ins().bitcast(types::F64, MemFlags::new(), v)
}

fn int_zero(builder: &mut FunctionBuilder) -> Value {
    builder.ins().iconst(types::I64, 0)
}

fn int_one(builder: &mut FunctionBuilder) -> Value {
    builder.ins().iconst(types::I64, 1)
}

fn translate_op(
    op: &Operand,
    builder: &mut FunctionBuilder,
    vars: &HashMap<Local, Variable>,
    string_gvs: &HashMap<String, GlobalValue>,
) -> Value {
    match op {
        Operand::Copy(l) | Operand::Move(l) => builder.use_var(vars[l]),
        Operand::Constant(c) => match c {
            Constant::I64(v) => builder.ins().iconst(types::I64, *v),
            Constant::F64(v) => f64_to_i64(builder, f64::from_bits(*v)),
            Constant::Bool(b) => {
                if *b { int_one(builder) } else { int_zero(builder) }
            }
            Constant::Str(s) => {
                if let Some(&gv) = string_gvs.get(s.as_str()) {
                    builder.ins().symbol_value(types::I64, gv)
                } else {
                    int_zero(builder)
                }
            }
            Constant::Function(_) | Constant::GlobalData(_) | Constant::None => int_zero(builder),
        },
    }
}

fn int_to_bool(builder: &mut FunctionBuilder, v: Value) -> Value {
    let zero = int_zero(builder);
    builder.ins().icmp(IntCC::NotEqual, v, zero)
}

fn bool_to_int(builder: &mut FunctionBuilder, cond: Value) -> Value {
    let one = int_one(builder);
    let zero = int_zero(builder);
    builder.ins().select(cond, one, zero)
}

fn translate_rvalue(
    rvalue: &Rvalue,
    builder: &mut FunctionBuilder,
    vars: &HashMap<Local, Variable>,
    func_refs: &HashMap<String, FuncRef>,
    string_gvs: &HashMap<String, GlobalValue>,
) -> Value {
    match rvalue {
        Rvalue::Use(op) => translate_op(op, builder, vars, string_gvs),
        Rvalue::BinaryOp(op, lhs, rhs) => {
            let lv = translate_op(lhs, builder, vars, string_gvs);
            let rv = translate_op(rhs, builder, vars, string_gvs);
            match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                    let fl = i64_to_f64(builder, lv);
                    let fr = i64_to_f64(builder, rv);
                    let fr2 = match op {
                        BinOp::Add => builder.ins().fadd(fl, fr),
                        BinOp::Sub => builder.ins().fsub(fl, fr),
                        BinOp::Mul => builder.ins().fmul(fl, fr),
                        BinOp::Div => builder.ins().fdiv(fl, fr),
                        _ => fl,
                    };
                    i64_to_f64(builder, fr2)
                }
                BinOp::Eq => {
                    let fl = i64_to_f64(builder, lv);
                    let fr = i64_to_f64(builder, rv);
                    let c = builder.ins().fcmp(FloatCC::Equal, fl, fr);
                    bool_to_int(builder, c)
                }
                BinOp::Ne => {
                    let fl = i64_to_f64(builder, lv);
                    let fr = i64_to_f64(builder, rv);
                    let c = builder.ins().fcmp(FloatCC::NotEqual, fl, fr);
                    bool_to_int(builder, c)
                }
                BinOp::Lt => {
                    let fl = i64_to_f64(builder, lv);
                    let fr = i64_to_f64(builder, rv);
                    let c = builder.ins().fcmp(FloatCC::LessThan, fl, fr);
                    bool_to_int(builder, c)
                }
                BinOp::Le => {
                    let fl = i64_to_f64(builder, lv);
                    let fr = i64_to_f64(builder, rv);
                    let c = builder.ins().fcmp(FloatCC::LessThanOrEqual, fl, fr);
                    bool_to_int(builder, c)
                }
                BinOp::Gt => {
                    let fl = i64_to_f64(builder, lv);
                    let fr = i64_to_f64(builder, rv);
                    let c = builder.ins().fcmp(FloatCC::GreaterThan, fl, fr);
                    bool_to_int(builder, c)
                }
                BinOp::Ge => {
                    let fl = i64_to_f64(builder, lv);
                    let fr = i64_to_f64(builder, rv);
                    let c = builder.ins().fcmp(FloatCC::GreaterThanOrEqual, fl, fr);
                    bool_to_int(builder, c)
                }
                BinOp::And => {
                    let lb = int_to_bool(builder, lv);
                    let rb = int_to_bool(builder, rv);
                    let b = builder.ins().band(lb, rb);
                    bool_to_int(builder, b)
                }
                BinOp::Or => {
                    let lb = int_to_bool(builder, lv);
                    let rb = int_to_bool(builder, rv);
                    let b = builder.ins().bor(lb, rb);
                    bool_to_int(builder, b)
                }
                _ => int_zero(builder),
            }
        }
        Rvalue::UnaryOp(op, operand) => {
            let v = translate_op(operand, builder, vars, string_gvs);
            match op {
                UnOp::Neg => {
                    let fv = i64_to_f64(builder, v);
                    let fnv = builder.ins().fneg(fv);
                    i64_to_f64(builder, fnv)
                }
                UnOp::Not => {
                    let b = int_to_bool(builder, v);
                    let nb = builder.ins().bnot(b);
                    bool_to_int(builder, nb)
                }
                _ => v,
            }
        }
        Rvalue::Call { func: callee, args } => {
            let callee_name = match callee {
                Operand::Constant(Constant::Function(n)) => n.clone(),
                _ => String::new(),
            };
            let mut cal_args = Vec::new();
            for arg in args {
                cal_args.push(translate_op(arg, builder, vars, string_gvs));
            }
            if let Some(&local_id) = func_refs.get(&callee_name) {
                let call_inst = builder.ins().call(local_id, &cal_args);
                builder.inst_results(call_inst)[0]
            } else {
                int_zero(builder)
            }
        }
        Rvalue::Aggregate(_, ops) => {
            if ops.is_empty() {
                int_zero(builder)
            } else {
                translate_op(&ops[0], builder, vars, string_gvs)
            }
        }
        _ => int_zero(builder),
    }
}

fn translate_terminator(
    term: &Terminator,
    builder: &mut FunctionBuilder,
    blocks: &[Block],
    vars: &HashMap<Local, Variable>,
    _func_refs: &HashMap<String, FuncRef>,
    string_gvs: &HashMap<String, GlobalValue>,
) {
    match &term.kind {
        TerminatorKind::Goto { target } => {
            builder.ins().jump(blocks[target.0], &[]);
        }
        TerminatorKind::SwitchInt { discr, targets, otherwise } => {
            let val = translate_op(discr, builder, vars, string_gvs);
            let zero = int_zero(builder);
            let not_zero = builder.ins().icmp(IntCC::NotEqual, val, zero);

            let mut true_target = otherwise.0;
            let mut false_target = otherwise.0;
            for (const_val, target_block) in targets {
                let cv = *const_val as i64;
                if cv == 1 {
                    true_target = target_block.0;
                } else if cv == 0 {
                    false_target = target_block.0;
                }
            }

            if true_target != otherwise.0 && false_target != otherwise.0 {
                builder.ins().brif(not_zero, blocks[true_target], &[], blocks[false_target], &[]);
            } else if true_target != otherwise.0 {
                builder.ins().brif(not_zero, blocks[true_target], &[], blocks[otherwise.0], &[]);
            } else {
                builder.ins().jump(blocks[otherwise.0], &[]);
            }
        }
        TerminatorKind::Return => {
            let ret = builder.use_var(vars[&Local(0)]);
            builder.ins().return_(&[ret]);
        }
        TerminatorKind::Unreachable => {}
    }
}

fn collect_strings_from_operand(op: &Operand, string_ids: &mut HashMap<String, DataId>, module: &mut ObjectModule, string_data: &mut HashMap<String, DataId>) {
    if let Operand::Constant(Constant::Str(s)) = op {
        if !string_ids.contains_key(s) {
            let name = format!("__str_{}", string_data.len());
            let data_id = module.declare_data(&name, Linkage::Local, true, false).unwrap();
            let mut desc = DataDescription::new();
            desc.define(s.as_bytes().to_vec().into_boxed_slice());
            desc.set_align(1);
            module.define_data(data_id, &desc).unwrap();
            string_data.insert(s.clone(), data_id);
            string_ids.insert(s.clone(), data_id);
        }
    }
}

fn collect_strings_from_rvalue(rval: &Rvalue, string_ids: &mut HashMap<String, DataId>, module: &mut ObjectModule, string_data: &mut HashMap<String, DataId>) {
    match rval {
        Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => collect_strings_from_operand(op, string_ids, module, string_data),
        Rvalue::BinaryOp(_, lhs, rhs) => {
            collect_strings_from_operand(lhs, string_ids, module, string_data);
            collect_strings_from_operand(rhs, string_ids, module, string_data);
        }
        Rvalue::Call { args, .. } => {
            for arg in args { collect_strings_from_operand(arg, string_ids, module, string_data); }
        }
        Rvalue::Aggregate(_, ops) => {
            for op in ops { collect_strings_from_operand(op, string_ids, module, string_data); }
        }
        _ => {}
    }
}

fn collect_strings_from_terminator(term: &Terminator, string_ids: &mut HashMap<String, DataId>, module: &mut ObjectModule, string_data: &mut HashMap<String, DataId>) {
    if let TerminatorKind::SwitchInt { discr, .. } = &term.kind {
        collect_strings_from_operand(discr, string_ids, module, string_data);
    }
}

impl CraneliftBackend {
    pub fn new() -> Self {
        let flag_builder = settings::builder();
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
            string_data: HashMap::new(),
        }
    }

}

impl CodegenBackend for CraneliftBackend {
    fn emit(&mut self, program: &MirProgram, out: &std::path::Path) -> Result<()> {
        let module = self.module.as_mut().unwrap();
        let builder_ctx = &mut self.builder_ctx;
        let string_data = &mut self.string_data;

        // Phase 1: declare all functions, build func_refs
        let mut func_ids: HashMap<String, cranelift_module::FuncId> = HashMap::new();
        let mut func_refs: HashMap<String, FuncRef> = HashMap::new();

        for func in &program.mir.functions {
            let mut sig = module.make_signature();
            for _ in 0..func.arg_count {
                sig.params.push(AbiParam::new(types::I64));
            }
            sig.returns.push(AbiParam::new(types::I64));
            let id = module.declare_function(&func.name, Linkage::Export, &sig).unwrap();
            func_ids.insert(func.name.clone(), id);
            let mut dummy_ctx = module.make_context();
            dummy_ctx.func.signature = sig;
            let fr = module.declare_func_in_func(id, &mut dummy_ctx.func);
            func_refs.insert(func.name.clone(), fr);
        }

        // Phase 1b: scan all functions for string constants, declare data objects
        let mut string_ids: HashMap<String, DataId> = HashMap::new();
        for func in &program.mir.functions {
            for bb in &func.basic_blocks {
                for stmt in &bb.statements {
                    if let StatementKind::Assign(_, rval) = &stmt.kind {
                        collect_strings_from_rvalue(rval, &mut string_ids, module, string_data);
                    }
                }
                if let Some(term) = &bb.terminator {
                    collect_strings_from_terminator(term, &mut string_ids, module, string_data);
                }
            }
        }

        // Phase 2: define each function body
        for func in &program.mir.functions {
            let &fid = func_ids.get(&func.name).unwrap();

            let mut ctx = module.make_context();
            let mut sig = module.make_signature();
            for _ in 0..func.arg_count {
                sig.params.push(AbiParam::new(types::I64));
            }
            sig.returns.push(AbiParam::new(types::I64));
            ctx.func.signature = sig;

            let mut builder = FunctionBuilder::new(&mut ctx.func, builder_ctx);

            let blocks: Vec<Block> = func
                .basic_blocks
                .iter()
                .map(|_| builder.create_block())
                .collect();

            let mut vars: HashMap<Local, Variable> = HashMap::new();
            for (i, _) in func.locals.iter().enumerate() {
                vars.insert(Local(i), builder.declare_var(types::I64));
            }

            // Build string GlobalValue map for this function
            let mut string_gvs: HashMap<String, GlobalValue> = HashMap::new();
            for (s, &data_id) in &string_ids {
                let gv = module.declare_data_in_func(data_id, builder.func);
                string_gvs.insert(s.clone(), gv);
            }

            let mut pred_count = vec![0u32; func.basic_blocks.len()];
            for (_, bb) in func.basic_blocks.iter().enumerate() {
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

            for bi in 0..func.basic_blocks.len() {
                if bi == 0 {
                    builder.switch_to_block(blocks[bi]);
                    builder.seal_block(blocks[bi]);
                } else {
                    builder.switch_to_block(blocks[bi]);
                    if pred_count[bi] > 0 {
                        builder.seal_block(blocks[bi]);
                    }
                }

                for stmt in &func.basic_blocks[bi].statements {
                    if let StatementKind::Assign(local, rvalue) = &stmt.kind {
                        let val = translate_rvalue(
                            rvalue,
                            &mut builder,
                            &vars,
                            &func_refs,
                            &string_gvs,
                        );
                        builder.def_var(vars[local], val);
                    }
                }
                if let Some(term) = &func.basic_blocks[bi].terminator {
                    translate_terminator(
                        term,
                        &mut builder,
                        &blocks,
                        &vars,
                        &func_refs,
                        &string_gvs,
                    );
                }
            }

            drop(builder);
            module.define_function(fid, &mut ctx).unwrap();
        }

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
