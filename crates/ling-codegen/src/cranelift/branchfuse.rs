//! Compare-branch fusion for the Cranelift backends.
//!
//! The MIR lowering materializes every loop/if condition as a NaN-boxed
//! boolean: the comparison emits `fcmp → select(TAG_TRUE, TAG_FALSE)` into a
//! temp, and the branch re-tests that temp against `TAG_TRUE`. The serial
//! `cmp → select → store → cmp → branch` chain sits on every loop back-edge
//! where C emits a bare `cmp; jcc` — the main reason tight Ling loops run
//! several times slower than equivalent C.
//!
//! This module detects the pattern statically and fuses it: when a branch
//! discriminant traces back through single-writer/single-reader temps to
//! comparisons (possibly combined with `&&`/`||`), the comparisons are emitted
//! directly as the `brif` condition — no boxed bool, no select, no re-test —
//! and their defining statements are skipped.
//!
//! Soundness rests on per-function tables:
//! - a temp is chased only if written exactly once and read exactly once (by
//!   the next link of the chain);
//! - absorbed statements form a suffix of their block: planning walks
//!   backwards from the terminator, so relocating a comparison to the branch
//!   crosses no intervening effect;
//! - a comparison fuses only under the same static-type precondition the
//!   ordinary boxed fast path in `translate_binop` uses (both operands
//!   statically numbers; integral operands compare as raw `i64`).
//!
//! Any link that doesn't fit degrades to a `Leaf`, evaluated with the existing
//! truthiness lowering at the branch point — identical semantics to the
//! unfused path. Cyclic or over-deep chains abort the whole fusion instead of
//! silently emitting a read of a skipped definition.

use super::numtype::NumberTypes;
use super::translate::{i64_as_f64, translate_op, translate_op_int, truthy_of, TransCtx};
use cranelift::codegen::ir::BlockArg;
use cranelift::prelude::*;
use cranelift_frontend::FunctionBuilder;
use ling_ast::ast::BinOp;
use ling_mir::ir::*;
use rustc_hash::{FxHashMap, FxHashSet};

/// Per-function fusion plans keyed by basic block.
pub(crate) struct FunctionFusion {
    plans: FxHashMap<usize, PlannedBranch>,
}

/// How a fused comparison tests its operands.
#[derive(Clone, Copy)]
pub(crate) enum CmpKind {
    /// Raw integer registers — operands statically proven integral.
    Int(IntCC),
    /// NaN-boxed values bitcast to `f64`.
    Float(FloatCC),
}

/// One node of a fused condition tree.
pub(crate) enum CondNode {
    Cmp {
        kind: CmpKind,
        lhs: Operand,
        rhs: Operand,
    },
    And(Box<CondNode>, Box<CondNode>),
    Or(Box<CondNode>, Box<CondNode>),
    /// An operand that couldn't be traced to comparisons; tested with the
    /// ordinary truthiness path at its position in the branch structure.
    Leaf(Operand),
}

pub(crate) struct PlannedBranch {
    /// Indices of this block's statements absorbed into the condition.
    skip: FxHashSet<usize>,
    root: CondNode,
}

impl PlannedBranch {
    pub(crate) fn skips(&self, si: usize) -> bool {
        self.skip.contains(&si)
    }

    pub(crate) fn condition(&self) -> &CondNode {
        &self.root
    }
}

impl FunctionFusion {
    pub(crate) fn build(func: &MirFunction, nt: &NumberTypes) -> Self {
        let tables = Tables::build(func);
        let mut plans = FxHashMap::default();
        for bi in 0..func.basic_blocks.len() {
            if let Some(plan) = plan_block(func, bi, &tables, nt) {
                plans.insert(bi, plan);
            }
        }
        Self { plans }
    }

    pub(crate) fn plan_for(&self, bi: usize) -> Option<&PlannedBranch> {
        self.plans.get(&bi)
    }
}

struct Tables {
    /// Statement index of the single writer of each once-written local.
    unique_writers: FxHashMap<Local, usize>,
    reads: FxHashMap<Local, u32>,
}

impl Tables {
    fn build(func: &MirFunction) -> Self {
        let mut writer_sites: FxHashMap<Local, Vec<usize>> = FxHashMap::default();
        let mut reads: FxHashMap<Local, u32> = FxHashMap::default();

        for bb in &func.basic_blocks {
            for (si, stmt) in bb.statements.iter().enumerate() {
                match &stmt.kind {
                    StatementKind::Assign(l, rval) => {
                        writer_sites.entry(*l).or_default().push(si);
                        collect_rvalue_reads(rval, &mut reads);
                    },
                    StatementKind::SetAttr(obj, _, val) => {
                        count_read(obj, &mut reads);
                        count_read(val, &mut reads);
                    },
                    StatementKind::SetIndex(obj, idx, val)
                    | StatementKind::VectorStore(obj, idx, val) => {
                        count_read(obj, &mut reads);
                        count_read(idx, &mut reads);
                        count_read(val, &mut reads);
                    },
                    // Dropping observes the value just like reading it.
                    StatementKind::Drop(l) => *reads.entry(*l).or_insert(0) += 1,
                    StatementKind::StorageLive(_) | StatementKind::StorageDead(_) => {},
                }
            }
            if let Some(Terminator { kind: TerminatorKind::SwitchInt { discr, .. }, .. }) =
                &bb.terminator
            {
                count_read(discr, &mut reads);
            }
        }

        let unique_writers = writer_sites
            .into_iter()
            .filter_map(|(l, sites)| (sites.len() == 1).then(|| (l, sites[0])))
            .collect();

        Self { unique_writers, reads }
    }
}

fn count_read(op: &Operand, reads: &mut FxHashMap<Local, u32>) {
    if let Operand::Copy(l) | Operand::Move(l) = op {
        *reads.entry(*l).or_insert(0) += 1;
    }
}

fn collect_rvalue_reads(rval: &Rvalue, reads: &mut FxHashMap<Local, u32>) {
    match rval {
        Rvalue::Use(op) | Rvalue::VectorSplat(op, _) => count_read(op, reads),
        Rvalue::BinaryOp(_, a, b) | Rvalue::VectorLoad(a, b, _) => {
            count_read(a, reads);
            count_read(b, reads);
        },
        Rvalue::VectorFMA(a, b, c) => {
            count_read(a, reads);
            count_read(b, reads);
            count_read(c, reads);
        },
        Rvalue::UnaryOp(_, a) => count_read(a, reads),
        Rvalue::Call { func, args } => {
            count_read(func, reads);
            for a in args {
                count_read(a, reads);
            }
        },
        Rvalue::Aggregate(_, ops) => {
            for op in ops {
                count_read(op, reads);
            }
        },
        Rvalue::GetAttr(op, _) => count_read(op, reads),
        Rvalue::GetIndex(a, b) => {
            count_read(a, reads);
            count_read(b, reads);
        },
        Rvalue::Ref(l) | Rvalue::MutRef(l) => *reads.entry(*l).or_insert(0) += 1,
        Rvalue::InlineAsm(_) => {},
    }
}

fn int_cc_of(op: &BinOp) -> Option<IntCC> {
    match op {
        BinOp::Eq => Some(IntCC::Equal),
        BinOp::Ne => Some(IntCC::NotEqual),
        BinOp::Lt => Some(IntCC::SignedLessThan),
        BinOp::Le => Some(IntCC::SignedLessThanOrEqual),
        BinOp::Gt => Some(IntCC::SignedGreaterThan),
        BinOp::Ge => Some(IntCC::SignedGreaterThanOrEqual),
        _ => None,
    }
}

fn float_cc_of(op: &BinOp) -> Option<FloatCC> {
    match op {
        BinOp::Eq => Some(FloatCC::Equal),
        BinOp::Ne => Some(FloatCC::NotEqual),
        BinOp::Lt => Some(FloatCC::LessThan),
        BinOp::Le => Some(FloatCC::LessThanOrEqual),
        BinOp::Gt => Some(FloatCC::GreaterThan),
        BinOp::Ge => Some(FloatCC::GreaterThanOrEqual),
        _ => None,
    }
}

/// Static-type gate for fusing one comparison, mirroring `translate_binop`'s
/// fast paths: raw-integer compare when both sides are proven integral,
/// raw-float compare when both are proven numbers, no fusion otherwise
/// (strings, mixed kinds — those must keep the general runtime path).
fn fusable_cmp_kind(
    op: &BinOp,
    lhs: &Operand,
    rhs: &Operand,
    nt: &NumberTypes,
    fname: &str,
) -> Option<CmpKind> {
    if nt.operand_is_int(fname, lhs) && nt.operand_is_int(fname, rhs) {
        return int_cc_of(op).map(CmpKind::Int);
    }
    if nt.operand_is_num(fname, lhs) && nt.operand_is_num(fname, rhs) {
        return float_cc_of(op).map(CmpKind::Float);
    }
    None
}

/// Try to plan a fused conditional branch for block `bi`.
fn plan_block(
    func: &MirFunction,
    bi: usize,
    tables: &Tables,
    nt: &NumberTypes,
) -> Option<PlannedBranch> {
    let bb = &func.basic_blocks[bi];
    let term = bb.terminator.as_ref()?;
    let TerminatorKind::SwitchInt { discr, targets, .. } = &term.kind else {
        return None;
    };
    // Only the boolean-shaped branch the lowering always emits:
    // 1 → true (optionally 0 → false), everything else → otherwise.
    let mut has_true_arm = false;
    for (v, _) in targets {
        match *v {
            1 => has_true_arm = true,
            0 => {},
            _ => return None,
        }
    }
    if !has_true_arm || bb.statements.is_empty() {
        return None;
    }
    let root_local = match discr {
        Operand::Copy(l) | Operand::Move(l) => *l,
        _ => return None,
    };

    // Phase 1 — walk backwards from the terminator, absorbing definitions of
    // still-needed condition links. Stops at the first statement that isn't
    // one, so the absorbed set is always a contiguous suffix of the block and
    // relocating it to the branch crosses no other effect.
    let mut needed: FxHashSet<Local> = FxHashSet::from_iter([root_local]);
    let mut defs: FxHashMap<Local, Rvalue> = FxHashMap::default();
    let mut skip: FxHashSet<usize> = FxHashSet::default();
    let mut frontier = bb.statements.len();
    while frontier > 0 {
        frontier -= 1;
        let stmt = &bb.statements[frontier];
        let StatementKind::Assign(local, rval) = &stmt.kind else { break };
        if !needed.remove(local) {
            break;
        }
        if tables.reads.get(local).copied().unwrap_or(0) != 1
            || tables.unique_writers.get(local) != Some(&frontier)
        {
            break;
        }
        match rval {
            // Pass-through binding (`test = <cond>`): follow the wrapped
            // operand one link further back.
            Rvalue::Use(Operand::Copy(next)) | Rvalue::Use(Operand::Move(next)) => {
                needed.insert(*next);
            },
            // Comparison operands are evaluated in place at the branch; they
            // are never chased, only type-checked.
            Rvalue::BinaryOp(
                op @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge),
                lhs,
                rhs,
            ) => {
                fusable_cmp_kind(op, lhs, rhs, nt, &func.name)?;
            },
            // `&&`/`||` sides naming temps defined earlier join the chase;
            // sides that fail later resolve to leaves.
            Rvalue::BinaryOp(BinOp::And | BinOp::Or, lhs, rhs) => {
                for side in [lhs, rhs] {
                    if let Operand::Copy(l) | Operand::Move(l) = side {
                        needed.insert(*l);
                    }
                }
            },
            _ => break,
        }
        defs.insert(*local, rval.clone());
        skip.insert(frontier);
    }
    if skip.is_empty() {
        return None;
    }

    // Phase 2 — rebuild the condition tree from the absorbed definitions.
    // Links left unresolved by the walk (defined out of order, in another
    // block, or failing validation) come out as leaves; a tree with no real
    // comparison gains nothing, so reject it outright.
    let root = resolve_local(&root_local, &defs, nt, &func.name)?;
    match root {
        CondNode::Leaf(_) => None,
        _ => Some(PlannedBranch { skip, root }),
    }
}

/// Depth bound mirroring the optimizer's own inline-chain limits; a legit
/// condition chain is a handful of links long.
const RESOLVE_DEPTH_CAP: u32 = 16;

/// Resolve a chased local to a condition subtree. `None` aborts the whole
/// fusion (over-deep structure) rather than risking an undefined read.
fn resolve_local(
    local: &Local,
    defs: &FxHashMap<Local, Rvalue>,
    nt: &NumberTypes,
    fname: &str,
) -> Option<CondNode> {
    resolve_depth(local, defs, nt, fname, 0)
}

fn resolve_depth(
    local: &Local,
    defs: &FxHashMap<Local, Rvalue>,
    nt: &NumberTypes,
    fname: &str,
    depth: u32,
) -> Option<CondNode> {
    if depth > RESOLVE_DEPTH_CAP {
        return None;
    }
    match defs.get(local)? {
        Rvalue::Use(op) => resolve_operand_depth(op, defs, nt, fname, depth),
        Rvalue::BinaryOp(
            op @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge),
            lhs,
            rhs,
        ) => Some(CondNode::Cmp {
            kind: fusable_cmp_kind(op, lhs, rhs, nt, fname)?,
            lhs: lhs.clone(),
            rhs: rhs.clone(),
        }),
        Rvalue::BinaryOp(BinOp::And, lhs, rhs) => Some(CondNode::And(
            Box::new(resolve_operand_depth(lhs, defs, nt, fname, depth + 1)?),
            Box::new(resolve_operand_depth(rhs, defs, nt, fname, depth + 1)?),
        )),
        Rvalue::BinaryOp(BinOp::Or, lhs, rhs) => Some(CondNode::Or(
            Box::new(resolve_operand_depth(lhs, defs, nt, fname, depth + 1)?),
            Box::new(resolve_operand_depth(rhs, defs, nt, fname, depth + 1)?),
        )),
        // Absorbed but unrecognizable (can't happen today — phase 1 only
        // absorbs the shapes above) — degrade to a leaf.
        _ => Some(CondNode::Leaf(Operand::Copy(*local))),
    }
}

/// Resolve one side of an And/Or to a subtree. `None` (over-deep) propagates
/// up and aborts the whole fusion — substituting anything else here would
/// change which runtime effects the branch observes.
fn resolve_operand_depth(
    op: &Operand,
    defs: &FxHashMap<Local, Rvalue>,
    nt: &NumberTypes,
    fname: &str,
    depth: u32,
) -> Option<CondNode> {
    match op {
        Operand::Copy(l) | Operand::Move(l) => match defs.get(l) {
            Some(_) => resolve_depth(l, defs, nt, fname, depth),
            None => Some(CondNode::Leaf(op.clone())),
        },
        _ => Some(CondNode::Leaf(op.clone())),
    }
}

/// Emit a fused condition tree, returning an `i1` suitable for `brif`.
/// Comparisons run natively (no boxed bool, no select); `&&`/`||` keep real
/// short-circuit control flow; leaves fall back to the ordinary truthiness
/// lowering.
pub(crate) fn emit_condition(
    node: &CondNode,
    builder: &mut FunctionBuilder,
    ctx: &TransCtx,
) -> Value {
    match node {
        CondNode::Cmp { kind, lhs, rhs } => match *kind {
            CmpKind::Int(cc) => {
                let a = translate_op_int(lhs, builder, ctx);
                let b = translate_op_int(rhs, builder, ctx);
                builder.ins().icmp(cc, a, b)
            },
            CmpKind::Float(cc) => {
                let av = translate_op(lhs, builder, ctx);
                let bv = translate_op(rhs, builder, ctx);
                let a = i64_as_f64(builder, av);
                let b = i64_as_f64(builder, bv);
                builder.ins().fcmp(cc, a, b)
            },
        },
        CondNode::Leaf(op) => {
            let val = translate_op(op, builder, ctx);
            truthy_of(
                builder,
                val,
                ctx.nt.operand_is_bool(ctx.fname, op),
                ctx.runtime_refs,
            )
        },
        CondNode::And(lhs, rhs) => {
            let l = emit_condition(lhs, builder, ctx);
            let block_rhs = builder.create_block();
            let block_done = builder.create_block();
            builder.append_block_param(block_done, types::I8);
            let farg = BlockArg::Value(builder.ins().iconst(types::I8, 0));
            builder.ins().brif(l, block_rhs, &[], block_done, &[farg]);

            builder.switch_to_block(block_rhs);
            let r = emit_condition(rhs, builder, ctx);
            let r_i8 = builder.ins().uextend(types::I8, r);
            builder.ins().jump(block_done, &[BlockArg::Value(r_i8)]);
            builder.seal_block(block_rhs);

            builder.switch_to_block(block_done);
            builder.seal_block(block_done);
            builder.block_params(block_done)[0]
        },
        CondNode::Or(lhs, rhs) => {
            let l = emit_condition(lhs, builder, ctx);
            let block_rhs = builder.create_block();
            let block_done = builder.create_block();
            builder.append_block_param(block_done, types::I8);
            let targ = BlockArg::Value(builder.ins().iconst(types::I8, 1));
            builder.ins().brif(l, block_done, &[targ], block_rhs, &[]);

            builder.switch_to_block(block_rhs);
            let r = emit_condition(rhs, builder, ctx);
            let r_i8 = builder.ins().uextend(types::I8, r);
            builder.ins().jump(block_done, &[BlockArg::Value(r_i8)]);
            builder.seal_block(block_rhs);

            builder.switch_to_block(block_done);
            builder.seal_block(block_done);
            builder.block_params(block_done)[0]
        },
    }
}
