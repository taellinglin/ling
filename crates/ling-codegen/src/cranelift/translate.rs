//! Backend-agnostic MIR → Cranelift translation shared by the JIT and AOT
//! object emitters. All helpers operate purely on a `FunctionBuilder` plus the
//! per-function symbol maps, so both backends build identical code from the same
//! source of truth.

use super::branchfuse;
use super::numtype::NumberTypes;
use super::runtime;
use cranelift::codegen::ir::{FuncRef, GlobalValue, InstBuilder, StackSlotData, StackSlotKind};
use cranelift::prelude::*;
use cranelift_frontend::FunctionBuilder;
use ling_ast::ast::{BinOp, UnOp};
use ling_mir::ir::*;
use std::collections::HashMap;

pub(crate) fn int_zero(builder: &mut FunctionBuilder) -> Value {
    builder.ins().iconst(types::I64, 0)
}

pub(crate) fn int_one(builder: &mut FunctionBuilder) -> Value {
    builder.ins().iconst(types::I64, 1)
}

/// Translate every basic block of `func` into `builder`, handling Cranelift's
/// deferred block sealing so loops (back-edges) build correctly. `blocks` must be
/// pre-created, one per MIR basic block, and `ctx` fully populated.
pub(crate) fn build_function_body(
    builder: &mut FunctionBuilder,
    func: &MirFunction,
    blocks: &[Block],
    ctx: &TransCtx,
) {
    let vars = ctx.vars;
    let pred_count = count_predecessors(func);
    let mut sealed = vec![false; func.basic_blocks.len()];
    let mut filled_pred = vec![0u32; func.basic_blocks.len()];
    let fusion = branchfuse::FunctionFusion::build(func, ctx.nt);

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

        // A planned branch fusion absorbs its condition's defining statements
        // (a suffix of this block); they are emitted inside the terminator.
        let plan = fusion.plan_for(bi);
        for (si, stmt) in func.basic_blocks[bi].statements.iter().enumerate() {
            let skipped = plan.is_some_and(|p| p.skips(si));
            if !skipped {
                translate_stmt(stmt, builder, ctx);
            }
        }
        if let Some(term) = &func.basic_blocks[bi].terminator {
            translate_terminator(term, builder, blocks, ctx, plan);
            match &term.kind {
                TerminatorKind::Goto { target } => {
                    filled_pred[target.0] += 1;
                    if filled_pred[target.0] == pred_count[target.0] && !sealed[target.0] {
                        builder.seal_block(blocks[target.0]);
                        sealed[target.0] = true;
                    }
                },
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
                },
                _ => {},
            }
        }
    }

    for (i, block) in blocks.iter().enumerate() {
        if !sealed[i] {
            builder.seal_block(*block);
        }
    }
}

pub(crate) fn count_predecessors(func: &MirFunction) -> Vec<u32> {
    let mut pred_count = vec![0u32; func.basic_blocks.len()];
    for bb in &func.basic_blocks {
        if let Some(term) = &bb.terminator {
            match &term.kind {
                TerminatorKind::Goto { target } => pred_count[target.0] += 1,
                TerminatorKind::SwitchInt { targets, otherwise, .. } => {
                    for (_, t) in targets {
                        pred_count[t.0] += 1;
                    }
                    pred_count[otherwise.0] += 1;
                },
                _ => {},
            }
        }
    }
    pred_count
}

pub(crate) fn max_local_index(func: &MirFunction) -> usize {
    let mut max = 0usize;
    // Check all locals in statements
    for bb in &func.basic_blocks {
        for stmt in &bb.statements {
            if let StatementKind::Assign(local, rval) = &stmt.kind {
                max = max.max(local.0);
                collect_local_from_rvalue(rval, &mut max);
            }
            if let StatementKind::SetAttr(obj, _, val) = &stmt.kind {
                if let Operand::Copy(l) | Operand::Move(l) = obj {
                    max = max.max(l.0);
                }
                if let Operand::Copy(l) | Operand::Move(l) = val {
                    max = max.max(l.0);
                }
            }
            if let StatementKind::SetIndex(obj, idx, val) = &stmt.kind {
                if let Operand::Copy(l) | Operand::Move(l) = obj {
                    max = max.max(l.0);
                }
                if let Operand::Copy(l) | Operand::Move(l) = idx {
                    max = max.max(l.0);
                }
                if let Operand::Copy(l) | Operand::Move(l) = val {
                    max = max.max(l.0);
                }
            }
            if let StatementKind::StorageLive(l)
            | StatementKind::StorageDead(l)
            | StatementKind::Drop(l) = &stmt.kind
            {
                max = max.max(l.0);
            }
        }
        if let Some(term) = &bb.terminator {
            if let TerminatorKind::SwitchInt {
                discr: Operand::Copy(l) | Operand::Move(l), ..
            } = &term.kind
            {
                max = max.max(l.0);
            }
        }
    }
    // Ensure at least arg_count + 1 locals (for params + return)
    max = max.max(func.arg_count);
    max
}

pub(crate) fn collect_local_from_rvalue(rval: &Rvalue, max: &mut usize) {
    match rval {
        Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => {
            if let Operand::Copy(l) | Operand::Move(l) = op {
                *max = (*max).max(l.0);
            }
        },
        Rvalue::BinaryOp(_, lhs, rhs) => {
            if let Operand::Copy(l) | Operand::Move(l) = lhs {
                *max = (*max).max(l.0);
            }
            if let Operand::Copy(l) | Operand::Move(l) = rhs {
                *max = (*max).max(l.0);
            }
        },
        Rvalue::Call { args, .. } => {
            for arg in args {
                if let Operand::Copy(l) | Operand::Move(l) = arg {
                    *max = (*max).max(l.0);
                }
            }
        },
        Rvalue::Aggregate(_, ops) => {
            for op in ops {
                if let Operand::Copy(l) | Operand::Move(l) = op {
                    *max = (*max).max(l.0);
                }
            }
        },
        Rvalue::GetAttr(op, _) | Rvalue::GetIndex(op, _) => {
            if let Operand::Copy(l) | Operand::Move(l) = op {
                *max = (*max).max(l.0);
            }
        },
        Rvalue::Ref(l) | Rvalue::MutRef(l) => {
            *max = (*max).max(l.0);
        },
        _ => {},
    }
}

/// All per-function symbol tables plus static type info, bundled so the
/// translation routines take one context instead of a long parameter list.
pub(crate) struct TransCtx<'a> {
    pub vars: &'a HashMap<Local, Variable>,
    pub string_gvs: &'a HashMap<String, GlobalValue>,
    pub builtin_gvs: &'a HashMap<String, GlobalValue>,
    pub runtime_refs: &'a HashMap<String, FuncRef>,
    pub func_refs: &'a HashMap<String, FuncRef>,
    pub func_arities: &'a HashMap<String, usize>,
    pub nt: &'a NumberTypes,
    pub fname: &'a str,
}

/// The static register representation of a value the translator produces.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Repr {
    /// NaN-boxed: a raw `f64` bit pattern or a tagged singleton. The default and
    /// the only representation crossing call/print/return boundaries.
    Boxed,
    /// A raw native `i64` integer — only for locals/temps `numtype` proved
    /// integral, kept unboxed so loops avoid the NaN-box round-trip.
    Int,
}

pub(crate) fn translate_stmt(stmt: &Statement, builder: &mut FunctionBuilder, ctx: &TransCtx) {
    if let StatementKind::Assign(local, rvalue) = &stmt.kind {
        let (val, repr) = translate_rvalue(rvalue, builder, ctx);
        if let Some(&var) = ctx.vars.get(local) {
            // Coerce the produced value into the destination local's static
            // representation. The analysis guarantees an `Int` local only has
            // integer writers, so (Boxed, dest=Int) is the rare divisor-fallback
            // case where an integer rides the float path — a truncating round-trip
            // recovers it exactly.
            let dest_int = ctx.nt.local_is_int(ctx.fname, local.0);
            let stored = match (repr, dest_int) {
                (Repr::Int, false) => int_to_boxed(builder, val),
                (Repr::Boxed, true) => boxed_to_int(builder, val),
                _ => val,
            };
            builder.def_var(var, stored);
        }
    }
}

pub(crate) fn translate_rvalue(
    rvalue: &Rvalue,
    builder: &mut FunctionBuilder,
    ctx: &TransCtx,
) -> (Value, Repr) {
    match rvalue {
        Rvalue::Use(op) => {
            if ctx.nt.operand_is_int(ctx.fname, op) {
                (translate_op_int(op, builder, ctx), Repr::Int)
            } else {
                (translate_op(op, builder, ctx), Repr::Boxed)
            }
        },
        Rvalue::BinaryOp(op, lhs, rhs) => translate_binop_rvalue(op, lhs, rhs, builder, ctx),
        Rvalue::UnaryOp(UnOp::Neg, operand) if ctx.nt.operand_is_int(ctx.fname, operand) => {
            let v = translate_op_int(operand, builder, ctx);
            (builder.ins().ineg(v), Repr::Int)
        },
        Rvalue::UnaryOp(op, operand) => {
            let v = translate_op(operand, builder, ctx);
            (
                translate_unop(op, builder, v, ctx.runtime_refs),
                Repr::Boxed,
            )
        },
        Rvalue::Call { func: callee, args } => {
            let callee_name = match callee {
                Operand::Constant(Constant::Function(n)) => n.clone(),
                _ => String::new(),
            };
            let is_sys_or_kernel =
                callee_name.starts_with("ling_kernel_") || callee_name.starts_with("ling_sys_");
            // Only these two intrinsics scan their string argument for a NUL
            // terminator (`ling_kernel_vga_write_str`/`ling_kernel_panic` in
            // ling-kernel's lib.rs); every other kernel intrinsic reads the
            // length-prefixed `ling_str_new` layout via `strings::bytes_of_ptr`.
            // A `Constant::Str` argument must therefore be boxed through the
            // normal `ling_str_new` path for those, exactly like any computed
            // string already is — otherwise MIR constant-propagation folding a
            // `bind`-then-call back into a direct literal call (which it does
            // for any non-trivial `__main__`, since copy/const-prop aren't
            // skipped there) silently reinterprets raw literal bytes as a
            // length header.
            let raw_cstr_kernel_fn = matches!(
                callee_name.as_str(),
                "ling_kernel_vga_write_str" | "ling_kernel_panic"
            );
            let mut cal_args = Vec::new();
            if is_sys_or_kernel {
                for arg in args {
                    if !raw_cstr_kernel_fn {
                        if let Operand::Constant(Constant::Str(_)) = arg {
                            let boxed = translate_op(arg, builder, ctx);
                            cal_args.push(dynamic_to_raw(builder, boxed));
                            continue;
                        }
                    }
                    cal_args.push(translate_op_int(arg, builder, ctx));
                }
            } else {
                for arg in args {
                    cal_args.push(translate_op(arg, builder, ctx));
                }
            }
            let v: (Value, Repr) = if let Some(&fr) = ctx.func_refs.get(&callee_name) {
                if let Some(&arity) = ctx.func_arities.get(&callee_name) {
                    if cal_args.len() < arity {
                        let unit = if is_sys_or_kernel {
                            builder.ins().iconst(types::I64, 0)
                        } else {
                            builder.ins().iconst(types::I64, runtime::TAG_UNIT as i64)
                        };
                        cal_args.resize(arity, unit);
                    } else {
                        cal_args.truncate(arity);
                    }
                }
                let inst = builder.ins().call(fr, &cal_args);
                let raw = builder.inst_results(inst)[0];
                if is_sys_or_kernel {
                    (dynamic_from_raw(builder, raw), Repr::Boxed)
                } else {
                    (raw, Repr::Boxed)
                }
            } else {
                (
                    emit_builtin_call(
                        callee_name,
                        cal_args,
                        builder,
                        ctx.builtin_gvs,
                        ctx.runtime_refs,
                    ),
                    Repr::Boxed,
                )
            };
            v
        },
        Rvalue::Aggregate(_, ops) => {
            let mut list = emit_runtime_call0(builder, "__ling_list_new", ctx.runtime_refs);
            for op in ops {
                let v = translate_op(op, builder, ctx);
                list = emit_runtime_call2(builder, "__ling_list_push", list, v, ctx.runtime_refs);
            }
            (list, Repr::Boxed)
        },
        Rvalue::GetIndex(obj, idx) => {
            let obj_val = translate_op(obj, builder, ctx);
            let idx_val = translate_op(idx, builder, ctx);
            let v = emit_runtime_call2(
                builder,
                "__ling_list_get",
                obj_val,
                idx_val,
                ctx.runtime_refs,
            );
            (v, Repr::Boxed)
        },
        Rvalue::InlineAsm(template) => {
            if let Some(&gv) = ctx.string_gvs.get(template.as_str()) {
                let ptr = builder.ins().symbol_value(types::I64, gv);
                let len = builder.ins().iconst(types::I64, template.len() as i64);
                if let Some(&fr) = ctx.func_refs.get("ling_kernel_asm_exec") {
                    let inst = builder.ins().call(fr, &[ptr, len]);
                    let raw = builder.inst_results(inst)[0];
                    (int_to_boxed(builder, raw), Repr::Boxed)
                } else {
                    (int_zero(builder), Repr::Boxed)
                }
            } else {
                (int_zero(builder), Repr::Boxed)
            }
        },
        _ => (int_zero(builder), Repr::Boxed),
    }
}

/// Lower a binary op, preferring native `i64` arithmetic when both operands are
/// statically integral (no NaN-box bitcasts, integer compares, strength-reducible
/// `%`). Falls back to the boxed `f64`/runtime path otherwise.
fn translate_binop_rvalue(
    op: &BinOp,
    lhs: &Operand,
    rhs: &Operand,
    builder: &mut FunctionBuilder,
    ctx: &TransCtx,
) -> (Value, Repr) {
    let both_int = ctx.nt.operand_is_int(ctx.fname, lhs) && ctx.nt.operand_is_int(ctx.fname, rhs);
    if both_int {
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                let a = translate_op_int(lhs, builder, ctx);
                let b = translate_op_int(rhs, builder, ctx);
                let r = match op {
                    BinOp::Add => builder.ins().iadd(a, b),
                    BinOp::Sub => builder.ins().isub(a, b),
                    _ => builder.ins().imul(a, b),
                };
                return (r, Repr::Int);
            },
            // `srem` traps on a zero divisor and on `INT_MIN % -1`, which the f64
            // path never does. Only take the integer path for a safe constant
            // divisor; variable divisors fall through to the boxed float remainder.
            BinOp::Rem => {
                if let Operand::Constant(Constant::I64(c)) = rhs {
                    if *c != 0 && *c != -1 {
                        let a = translate_op_int(lhs, builder, ctx);
                        let b = builder.ins().iconst(types::I64, *c);
                        return (builder.ins().srem(a, b), Repr::Int);
                    }
                }
            },
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let a = translate_op_int(lhs, builder, ctx);
                let b = translate_op_int(rhs, builder, ctx);
                let cc = match op {
                    BinOp::Eq => IntCC::Equal,
                    BinOp::Ne => IntCC::NotEqual,
                    BinOp::Lt => IntCC::SignedLessThan,
                    BinOp::Le => IntCC::SignedLessThanOrEqual,
                    BinOp::Gt => IntCC::SignedGreaterThan,
                    _ => IntCC::SignedGreaterThanOrEqual,
                };
                let cmp = builder.ins().icmp(cc, a, b);
                let t = builder.ins().iconst(types::I64, runtime::TAG_TRUE as i64);
                let f = builder.ins().iconst(types::I64, runtime::TAG_FALSE as i64);
                return (builder.ins().select(cmp, t, f), Repr::Boxed);
            },
            _ => {},
        }
    }
    let tys = OperandTypes {
        both_num: ctx.nt.operand_is_num(ctx.fname, lhs) && ctx.nt.operand_is_num(ctx.fname, rhs),
        l_bool: ctx.nt.operand_is_bool(ctx.fname, lhs),
        r_bool: ctx.nt.operand_is_bool(ctx.fname, rhs),
    };
    let lv = translate_op(lhs, builder, ctx);
    let rv = translate_op(rhs, builder, ctx);
    (
        translate_binop(op, builder, lv, rv, tys, ctx.runtime_refs),
        Repr::Boxed,
    )
}

/// Evaluate an operand as a NaN-boxed value (the universal representation).
/// Integer-typed locals are stored raw, so they are boxed back here on demand.
pub(crate) fn translate_op(op: &Operand, builder: &mut FunctionBuilder, ctx: &TransCtx) -> Value {
    let TransCtx { vars, string_gvs, runtime_refs, .. } = ctx;
    match op {
        Operand::Copy(l) | Operand::Move(l) => {
            let raw = builder.use_var(vars[l]);
            if ctx.nt.local_is_int(ctx.fname, l.0) {
                int_to_boxed(builder, raw)
            } else {
                raw
            }
        },
        Operand::Constant(c) => match c {
            Constant::I64(v) => {
                let bits = (*v as f64).to_bits();
                builder.ins().iconst(types::I64, bits as i64)
            },
            Constant::F64(v) => builder.ins().iconst(types::I64, *v as i64),
            Constant::Bool(b) => builder.ins().iconst(
                types::I64,
                if *b {
                    runtime::TAG_TRUE as i64
                } else {
                    runtime::TAG_FALSE as i64
                },
            ),
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
            },
            Constant::Function(_) | Constant::GlobalData(_) | Constant::None => {
                builder.ins().iconst(types::I64, runtime::TAG_UNIT as i64)
            },
        },
    }
}

/// Evaluate an operand as a raw native `i64`. Called only on operands the type
/// analysis proved integral (an `Int` local or an `I64` literal); any other input
/// is converted defensively so a misclassification degrades to correct-but-slower
/// rather than to garbage.
pub(crate) fn translate_op_int(
    op: &Operand,
    builder: &mut FunctionBuilder,
    ctx: &TransCtx,
) -> Value {
    match op {
        Operand::Constant(Constant::I64(v)) => builder.ins().iconst(types::I64, *v),
        // Integral F64 literals (`1`, `7`, loop bounds — the MIR lowering
        // encodes all numeric literals as F64 bit patterns) materialize as a
        // raw integer so int-typed arithmetic stays on the native path. The
        // integrality test is the same one the type analysis uses, so this
        // arm fires exactly when `operand_is_int` classified the constant.
        Operand::Constant(bits @ Constant::F64(_)) if ctx.nt.constant_is_int(bits) => {
            let v = match bits {
                Constant::F64(b) => f64::from_bits(*b) as i64,
                _ => unreachable!(),
            };
            builder.ins().iconst(types::I64, v)
        },
        Operand::Copy(l) | Operand::Move(l) if ctx.nt.local_is_int(ctx.fname, l.0) => {
            builder.use_var(ctx.vars[l])
        },
        // Raw pointer to the (NUL-terminated) string data — not boxed via
        // `ling_str_new`. Kernel intrinsics (`ling_kernel_vga_write_str` and
        // friends) are the only raw-int callers that ever see a `Str`
        // constant, and they all take a bare pointer, so this avoids
        // depending on the boxed/managed-string runtime (`ling_str_new`,
        // `ling_alloc`) that a `#![no_std]` kernel build doesn't provide.
        Operand::Constant(Constant::Str(s)) => {
            if let Some(&gv) = ctx.string_gvs.get(s.as_str()) {
                builder.ins().symbol_value(types::I64, gv)
            } else {
                builder.ins().iconst(types::I64, 0)
            }
        },
        _ => {
            let boxed = translate_op(op, builder, ctx);
            dynamic_to_raw(builder, boxed)
        },
    }
}

/// Static operand types feeding a binary op, used to pick the fastest lowering.
#[derive(Clone, Copy)]
pub(crate) struct OperandTypes {
    pub both_num: bool,
    pub l_bool: bool,
    pub r_bool: bool,
}

pub(crate) fn translate_binop(
    op: &BinOp,
    builder: &mut FunctionBuilder,
    lv: Value,
    rv: Value,
    tys: OperandTypes,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    let OperandTypes { both_num, l_bool, r_bool } = tys;
    // When both operands are statically known numbers, emit the raw f64 op with no
    // tag check or runtime fallback — the hot path for numeric code.
    if both_num {
        match op {
            BinOp::Add => return raw_f64_binop(builder, lv, rv, |b, a, v| b.ins().fadd(a, v)),
            BinOp::Sub => return raw_f64_binop(builder, lv, rv, |b, a, v| b.ins().fsub(a, v)),
            BinOp::Mul => return raw_f64_binop(builder, lv, rv, |b, a, v| b.ins().fmul(a, v)),
            BinOp::Div => return raw_f64_binop(builder, lv, rv, |b, a, v| b.ins().fdiv(a, v)),
            BinOp::Rem => {
                return raw_f64_binop(builder, lv, rv, |b, a, v| {
                    let div = b.ins().fdiv(a, v);
                    let trunc = b.ins().trunc(div);
                    let prod = b.ins().fmul(trunc, v);
                    b.ins().fsub(a, prod)
                })
            },
            BinOp::Eq => return raw_f64_cmp(builder, lv, rv, FloatCC::Equal),
            BinOp::Ne => return raw_f64_cmp(builder, lv, rv, FloatCC::NotEqual),
            BinOp::Lt => return raw_f64_cmp(builder, lv, rv, FloatCC::LessThan),
            BinOp::Le => return raw_f64_cmp(builder, lv, rv, FloatCC::LessThanOrEqual),
            BinOp::Gt => return raw_f64_cmp(builder, lv, rv, FloatCC::GreaterThan),
            BinOp::Ge => return raw_f64_cmp(builder, lv, rv, FloatCC::GreaterThanOrEqual),
            _ => {},
        }
    }
    match op {
        BinOp::Add => emit_f64_or_runtime(
            builder,
            lv,
            rv,
            "__ling_add",
            |b, a, v| b.ins().fadd(a, v),
            runtime_refs,
        ),
        BinOp::Sub => emit_f64_or_runtime(
            builder,
            lv,
            rv,
            "__ling_sub",
            |b, a, v| b.ins().fsub(a, v),
            runtime_refs,
        ),
        BinOp::Mul => emit_f64_or_runtime(
            builder,
            lv,
            rv,
            "__ling_mul",
            |b, a, v| b.ins().fmul(a, v),
            runtime_refs,
        ),
        BinOp::Div => emit_f64_or_runtime(
            builder,
            lv,
            rv,
            "__ling_div",
            |b, a, v| b.ins().fdiv(a, v),
            runtime_refs,
        ),
        BinOp::Rem => emit_f64_or_runtime(
            builder,
            lv,
            rv,
            "__ling_rem",
            |b, a, v| {
                let div = b.ins().fdiv(a, v);
                let trunc = b.ins().trunc(div);
                let prod = b.ins().fmul(trunc, v);
                b.ins().fsub(a, prod)
            },
            runtime_refs,
        ),
        BinOp::Eq => {
            emit_f64_cmp_or_runtime(builder, lv, rv, "__ling_eq", FloatCC::Equal, runtime_refs)
        },
        BinOp::Ne => emit_f64_cmp_or_runtime(
            builder,
            lv,
            rv,
            "__ling_ne",
            FloatCC::NotEqual,
            runtime_refs,
        ),
        BinOp::Lt => emit_f64_cmp_or_runtime(
            builder,
            lv,
            rv,
            "__ling_lt",
            FloatCC::LessThan,
            runtime_refs,
        ),
        BinOp::Le => emit_f64_cmp_or_runtime(
            builder,
            lv,
            rv,
            "__ling_le",
            FloatCC::LessThanOrEqual,
            runtime_refs,
        ),
        BinOp::Gt => emit_f64_cmp_or_runtime(
            builder,
            lv,
            rv,
            "__ling_gt",
            FloatCC::GreaterThan,
            runtime_refs,
        ),
        BinOp::Ge => emit_f64_cmp_or_runtime(
            builder,
            lv,
            rv,
            "__ling_ge",
            FloatCC::GreaterThanOrEqual,
            runtime_refs,
        ),
        BinOp::And => emit_short_circuit_and(builder, lv, rv, l_bool, r_bool, runtime_refs),
        BinOp::Or => emit_short_circuit_or(builder, lv, rv, l_bool, r_bool, runtime_refs),
        _ => emit_runtime_call2(builder, "__ling_add", lv, rv, runtime_refs),
    }
}

/// Raw f64 arithmetic on NaN-boxed operands: bitcast in, compute, bitcast out.
pub(crate) fn raw_f64_binop(
    builder: &mut FunctionBuilder,
    a: Value,
    b: Value,
    f64_op: impl FnOnce(&mut FunctionBuilder, Value, Value) -> Value,
) -> Value {
    let fa = i64_as_f64(builder, a);
    let fb = i64_as_f64(builder, b);
    let r = f64_op(builder, fa, fb);
    f64_as_i64(builder, r)
}

/// Raw f64 comparison producing a NaN-boxed boolean.
pub(crate) fn raw_f64_cmp(builder: &mut FunctionBuilder, a: Value, b: Value, cc: FloatCC) -> Value {
    let fa = i64_as_f64(builder, a);
    let fb = i64_as_f64(builder, b);
    let cmp = builder.ins().fcmp(cc, fa, fb);
    let t = builder.ins().iconst(types::I64, runtime::TAG_TRUE as i64);
    let f = builder.ins().iconst(types::I64, runtime::TAG_FALSE as i64);
    builder.ins().select(cmp, t, f)
}

pub(crate) fn translate_unop(
    op: &UnOp,
    builder: &mut FunctionBuilder,
    v: Value,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    match op {
        UnOp::Ref | UnOp::Deref => v,
        UnOp::Neg => emit_f64_or_runtime(
            builder,
            v,
            v,
            "__ling_neg",
            |b, a, _| b.ins().fneg(a),
            runtime_refs,
        ),
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
        },
    }
}

pub(crate) fn i64_as_f64(builder: &mut FunctionBuilder, v: Value) -> Value {
    builder.ins().bitcast(types::F64, MemFlags::new(), v)
}

pub(crate) fn f64_as_i64(builder: &mut FunctionBuilder, v: Value) -> Value {
    builder.ins().bitcast(types::I64, MemFlags::new(), v)
}

/// NaN-box a raw integer: widen to `f64`, then reinterpret its bits. Exact for
/// any value within ±2^53 (Ling numbers are `f64`, so larger integers were never
/// representable losslessly anyway).
pub(crate) fn int_to_boxed(builder: &mut FunctionBuilder, v: Value) -> Value {
    let f = builder.ins().fcvt_from_sint(types::F64, v);
    f64_as_i64(builder, f)
}

/// Unbox a NaN-boxed number to a raw integer, truncating toward zero. Saturating
/// (`fcvt_to_sint_sat`) so a stray non-integer or NaN can never trap.
pub(crate) fn boxed_to_int(builder: &mut FunctionBuilder, v: Value) -> Value {
    let f = i64_as_f64(builder, v);
    builder.ins().fcvt_to_sint_sat(types::I64, f)
}

/// Unbox a NaN-boxed *value of unknown kind* to a raw `u64` — used for
/// `ling_kernel_*` call arguments, which always want the raw payload (a
/// pointer for a heap value, a truncated integer for a number), never the
/// tagged bit pattern itself. `boxed_to_int` alone is wrong here: it always
/// treats its input as a boxed float, so a tagged heap value (e.g. a real
/// Ling string built via `ling_str_new`/concatenation) would have its tag
/// bits reinterpreted as float bits and saturate to 0 — silently handing
/// the kernel intrinsic a null pointer instead of the string's address.
pub(crate) fn dynamic_to_raw(builder: &mut FunctionBuilder, v: Value) -> Value {
    let is_num = emit_is_number(builder, v);
    let f = i64_as_f64(builder, v);
    let as_int = builder.ins().fcvt_to_sint_sat(types::I64, f);
    let ptr_mask = builder
        .ins()
        .iconst(types::I64, 0x0000_FFFF_FFFF_FFFFu64 as i64);
    let as_ptr = builder.ins().band(v, ptr_mask);
    builder.ins().select(is_num, as_int, as_ptr)
}

/// Box a `ling_kernel_*` call's raw `u64` return value — the mirror of
/// `dynamic_to_raw` for arguments, needed for the same reason. Most kernel
/// intrinsics return a plain small integer (a byte, a 0/1 flag) that wants
/// `int_to_boxed`'s float-NaN-boxing, but a few (`ling_kernel_read_line`)
/// return an *already-tagged* heap value built kernel-side via
/// `ling_str_new` — reboxing that as if it were an integer would
/// reinterpret its pointer bits as a signed integer and convert *that* to
/// a float, corrupting it. Distinguished the same way `dynamic_to_raw`
/// distinguishes its input: top byte `0x7F` marks an already-tagged value,
/// and no kernel-returned plain integer is remotely large enough
/// (~9.2 * 10^18) to collide with that by accident.
pub(crate) fn dynamic_from_raw(builder: &mut FunctionBuilder, raw: Value) -> Value {
    let shifted = builder.ins().ushr_imm(raw, 56);
    let already_tagged = builder.ins().icmp_imm(IntCC::Equal, shifted, 0x7F);
    let boxed_as_int = int_to_boxed(builder, raw);
    builder.ins().select(already_tagged, raw, boxed_as_int)
}

pub(crate) fn emit_f64_or_runtime(
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

pub(crate) fn emit_f64_cmp_or_runtime(
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

/// Truthiness of a value: a direct `== TAG_TRUE` test when the value is a known
/// boolean, otherwise the full runtime truthiness path.
pub(crate) fn truthy_of(
    builder: &mut FunctionBuilder,
    val: Value,
    is_bool: bool,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    if is_bool {
        builder
            .ins()
            .icmp_imm(IntCC::Equal, val, runtime::TAG_TRUE as i64)
    } else {
        emit_is_truthy(builder, val, runtime_refs)
    }
}

pub(crate) fn emit_short_circuit_and(
    builder: &mut FunctionBuilder,
    lv: Value,
    rv: Value,
    l_bool: bool,
    r_bool: bool,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    let l_is_truthy = truthy_of(builder, lv, l_bool, runtime_refs);
    let block_false = builder.create_block();
    let block_true = builder.create_block();
    let block_merge = builder.create_block();
    let res_var = builder.declare_var(types::I64);

    builder
        .ins()
        .brif(l_is_truthy, block_true, &[], block_false, &[]);

    builder.switch_to_block(block_false);
    let f = builder.ins().iconst(types::I64, runtime::TAG_FALSE as i64);
    builder.def_var(res_var, f);
    builder.ins().jump(block_merge, &[]);
    builder.seal_block(block_false);

    builder.switch_to_block(block_true);
    let r_is_truthy = truthy_of(builder, rv, r_bool, runtime_refs);
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

pub(crate) fn emit_short_circuit_or(
    builder: &mut FunctionBuilder,
    lv: Value,
    rv: Value,
    l_bool: bool,
    r_bool: bool,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    let l_is_truthy = truthy_of(builder, lv, l_bool, runtime_refs);
    let block_true = builder.create_block();
    let block_false = builder.create_block();
    let block_merge = builder.create_block();
    let res_var = builder.declare_var(types::I64);

    builder
        .ins()
        .brif(l_is_truthy, block_true, &[], block_false, &[]);

    builder.switch_to_block(block_true);
    let t = builder.ins().iconst(types::I64, runtime::TAG_TRUE as i64);
    builder.def_var(res_var, t);
    builder.ins().jump(block_merge, &[]);
    builder.seal_block(block_true);

    builder.switch_to_block(block_false);
    let r_is_truthy = truthy_of(builder, rv, r_bool, runtime_refs);
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

pub(crate) fn emit_is_number(builder: &mut FunctionBuilder, val: Value) -> Value {
    let shifted = builder.ins().ushr_imm(val, 56);
    let tag = builder.ins().iconst(types::I64, 0x7F);
    builder.ins().icmp(IntCC::NotEqual, shifted, tag)
}

pub(crate) fn emit_is_truthy(
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
    let is_true = builder
        .ins()
        .icmp_imm(IntCC::Equal, val, runtime::TAG_TRUE as i64);
    let is_false = builder
        .ins()
        .icmp_imm(IntCC::Equal, val, runtime::TAG_FALSE as i64);
    let is_unit = builder
        .ins()
        .icmp_imm(IntCC::Equal, val, runtime::TAG_UNIT as i64);
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

pub(crate) fn emit_runtime_call0(
    builder: &mut FunctionBuilder,
    name: &str,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    let fr = *runtime_refs
        .get(name)
        .unwrap_or_else(|| panic!("runtime fn not found: {}", name));
    let inst = builder.ins().call(fr, &[]);
    builder.inst_results(inst)[0]
}

pub(crate) fn emit_runtime_call1(
    builder: &mut FunctionBuilder,
    name: &str,
    a: Value,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    let fr = *runtime_refs
        .get(name)
        .unwrap_or_else(|| panic!("runtime fn not found: {}", name));
    let inst = builder.ins().call(fr, &[a]);
    builder.inst_results(inst)[0]
}

pub(crate) fn emit_runtime_call2(
    builder: &mut FunctionBuilder,
    name: &str,
    a: Value,
    b: Value,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    let fr = *runtime_refs
        .get(name)
        .unwrap_or_else(|| panic!("runtime fn not found: {}", name));
    let inst = builder.ins().call(fr, &[a, b]);
    builder.inst_results(inst)[0]
}

pub(crate) fn emit_runtime_call4(
    builder: &mut FunctionBuilder,
    name: &str,
    a: Value,
    b: Value,
    c: Value,
    d: Value,
    runtime_refs: &HashMap<String, FuncRef>,
) -> Value {
    let fr = *runtime_refs
        .get(name)
        .unwrap_or_else(|| panic!("runtime fn not found: {}", name));
    let inst = builder.ins().call(fr, &[a, b, c, d]);
    builder.inst_results(inst)[0]
}

pub(crate) fn emit_builtin_call(
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
                for arg in &args[..args.len() - 1] {
                    emit_runtime_call1(builder, "__ling_print_val", *arg, runtime_refs);
                }
                emit_runtime_call1(
                    builder,
                    "__ling_print_val",
                    args[args.len() - 1],
                    runtime_refs,
                );
            }
            return emit_runtime_call0(builder, "__ling_print_newline", runtime_refs);
        },
        "sin" => return unbox_f64_or_call(builder, args, "__ling_sin", runtime_refs),
        "cos" => return unbox_f64_or_call(builder, args, "__ling_cos", runtime_refs),
        "sqrt" => return unbox_f64_or_call(builder, args, "__ling_sqrt", runtime_refs),
        "abs" => return unbox_f64_or_call(builder, args, "__ling_abs", runtime_refs),
        "floor" => return unbox_f64_or_call(builder, args, "__ling_floor", runtime_refs),
        "ceil" => return unbox_f64_or_call(builder, args, "__ling_ceil", runtime_refs),
        "round" => return unbox_f64_or_call(builder, args, "__ling_round", runtime_refs),
        "time_now" | "เวลาปัจจุบัน" | "当前时间" | "経過時間" | "현재시간" =>
        {
            return emit_runtime_call0(builder, "__ling_time_now", runtime_refs);
        },
        "len" | "str_len" | "ความยาว" | "长度" | "長さ" | "길이" => {
            if !args.is_empty() {
                return emit_runtime_call1(builder, "__ling_str_len", args[0], runtime_refs);
            } else {
                return builder.ins().iconst(types::I64, runtime::TAG_UNIT as i64);
            }
        },
        // List ops dispatch straight to their O(1) ABI symbols, bypassing the
        // generic `__ling_builtin` → interpreter path (which deep-copies the Vec
        // on every push/get). Names mirror the interpreter's alias sets.
        "list_new" | "รายการใหม่" | "新建列表" | "新規リスト" | "새목록" =>
        {
            return emit_runtime_call0(builder, "__ling_list_new", runtime_refs);
        },
        "list_push" | "เพิ่มรายการ" | "列表添加" | "リスト追加" | "목록추가" if args.len() >= 2 =>
        {
            return emit_runtime_call2(builder, "__ling_list_push", args[0], args[1], runtime_refs);
        },
        "list_get" | "รับรายการ" | "取元素" | "要素取得" | "요소가져오기" if args.len() >= 2 =>
        {
            return emit_runtime_call2(builder, "__ling_list_get", args[0], args[1], runtime_refs);
        },
        _ => {},
    }
    // Fallback: dispatch through __ling_builtin for any builtin not handled above
    if let Some(&name_gv) = builtin_gvs.get(&name) {
        let name_ptr = builder.ins().symbol_value(types::I64, name_gv);
        let name_len = builder.ins().iconst(types::I64, name.len() as i64);
        let num_args = args.len();
        // Always allocate a stack slot (even for 0 args) so args_ptr is valid
        let slot_size = std::cmp::max(num_args * 8, 8) as u32;
        let args_slot = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            slot_size,
            8,
        ));
        let args_ptr = builder.ins().stack_addr(types::I64, args_slot, 0);
        for (i, arg) in args.iter().enumerate() {
            let off = builder.ins().iconst(types::I64, (i * 8) as i64);
            let elem_ptr = builder.ins().iadd(args_ptr, off);
            builder.ins().store(MemFlags::new(), *arg, elem_ptr, 0);
        }
        let args_len = builder.ins().iconst(types::I64, num_args as i64);
        emit_runtime_call4(
            builder,
            "__ling_builtin",
            name_ptr,
            name_len,
            args_ptr,
            args_len,
            runtime_refs,
        )
    } else {
        builder.ins().iconst(types::I64, runtime::TAG_UNIT as i64)
    }
}

pub(crate) fn unbox_f64_or_call(
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
    let fr = *runtime_refs
        .get(runtime_fn)
        .unwrap_or_else(|| panic!("runtime fn not found: {}", runtime_fn));
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

pub(crate) fn translate_terminator(
    term: &Terminator,
    builder: &mut FunctionBuilder,
    blocks: &[Block],
    ctx: &TransCtx,
    fusion_plan: Option<&branchfuse::PlannedBranch>,
) {
    match &term.kind {
        TerminatorKind::Goto { target } => {
            builder.ins().jump(blocks[target.0], &[]);
        },
        TerminatorKind::SwitchInt { discr, targets, otherwise } => {
            let is_truthy = if let Some(plan) = fusion_plan {
                // Fused path: the condition's comparisons were absorbed from
                // the statement list; emit them as the branch test directly.
                branchfuse::emit_condition(plan.condition(), builder, ctx)
            } else {
                let val = translate_op(discr, builder, ctx);
                // A strict-bool discriminant (the common loop/if condition) only needs a
                // direct compare against TAG_TRUE, skipping the general truthiness path.
                if ctx.nt.operand_is_bool(ctx.fname, discr) {
                    builder
                        .ins()
                        .icmp_imm(IntCC::Equal, val, runtime::TAG_TRUE as i64)
                } else {
                    emit_is_truthy(builder, val, ctx.runtime_refs)
                }
            };
            let mut true_target = otherwise.0;
            let mut false_target = otherwise.0;
            for (const_val, target_block) in targets {
                if *const_val == 1 {
                    true_target = target_block.0;
                } else if *const_val == 0 {
                    false_target = target_block.0;
                }
            }
            if true_target != otherwise.0 && false_target != otherwise.0 {
                builder.ins().brif(
                    is_truthy,
                    blocks[true_target],
                    &[],
                    blocks[false_target],
                    &[],
                );
            } else if true_target != otherwise.0 {
                builder.ins().brif(
                    is_truthy,
                    blocks[true_target],
                    &[],
                    blocks[otherwise.0],
                    &[],
                );
            } else {
                builder.ins().jump(blocks[otherwise.0], &[]);
            }
        },
        TerminatorKind::Return => {
            let ret = builder.use_var(ctx.vars[&Local(0)]);
            builder.ins().return_(&[ret]);
        },
        TerminatorKind::Unreachable => {
            builder.ins().trap(TrapCode::INTEGER_OVERFLOW);
        },
    }
}
