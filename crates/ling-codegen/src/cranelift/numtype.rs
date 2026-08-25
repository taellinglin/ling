//! Whole-program number-type inference.
//!
//! Ling values are NaN-boxed: a `u64` is either a raw `f64` or a tagged pointer/
//! singleton. The naive code path tag-checks both operands on every arithmetic op
//! and branches to a runtime fallback. That overhead dominates numeric loops.
//!
//! This pass recovers static type information the untyped MIR throws away. A local
//! is a *number* when every value that can flow into it is provably an `f64`.
//! Because Ling discards declared parameter types, parameter and return types are
//! inferred interprocedurally from call sites with a greatest fixpoint: assume
//! everything is a number, then retract that wherever a non-number can reach.
//!
//! The JIT/AOT backends consult the result to emit raw `fadd`/`fcmp`/… with no tag
//! checks wherever both operands are known numbers, closing most of the gap to
//! native code.

use ling_ast::ast::{BinOp, UnOp};
use ling_mir::ir::*;
use std::collections::{HashMap, HashSet};

/// Per-function static types: which local indices are proven numbers (`f64`) or
/// strict booleans (a `TAG_TRUE`/`TAG_FALSE` singleton).
#[derive(Default)]
pub struct NumberTypes {
    locals: HashMap<String, HashSet<usize>>,
    bools: HashMap<String, HashSet<usize>>,
    ints: HashMap<String, HashSet<usize>>,
}

impl NumberTypes {
    /// Whether `local` in function `func` is statically known to be a number.
    pub fn local_is_num(&self, func: &str, local: usize) -> bool {
        self.locals.get(func).is_some_and(|s| s.contains(&local))
    }

    /// Whether `local` in function `func` is a *strict integer* — a number that
    /// only ever holds integral values, so it can live in a raw `i64` register
    /// instead of a NaN-boxed `f64`. A subset of `local_is_num`.
    pub fn local_is_int(&self, func: &str, local: usize) -> bool {
        self.ints.get(func).is_some_and(|s| s.contains(&local))
    }

    /// Whether `op` evaluates to a strict integer inside function `func`.
    pub fn operand_is_int(&self, func: &str, op: &Operand) -> bool {
        match op {
            Operand::Copy(l) | Operand::Move(l) => self.local_is_int(func, l.0),
            Operand::Constant(c) => constant_is_int(c),
        }
    }

    /// Whether `op` evaluates to a number inside function `func`.
    pub fn operand_is_num(&self, func: &str, op: &Operand) -> bool {
        match op {
            Operand::Copy(l) | Operand::Move(l) => self.local_is_num(func, l.0),
            Operand::Constant(c) => matches!(c, Constant::I64(_) | Constant::F64(_)),
        }
    }

    /// Whether `op` is a strict boolean inside function `func` (so a branch can
    /// test it directly against `TAG_TRUE` rather than running full truthiness).
    pub fn operand_is_bool(&self, func: &str, op: &Operand) -> bool {
        match op {
            Operand::Copy(l) | Operand::Move(l) => {
                self.bools.get(func).is_some_and(|s| s.contains(&l.0))
            },
            Operand::Constant(Constant::Bool(_)) => true,
            _ => false,
        }
    }
}

fn bool_binop(op: &BinOp) -> bool {
    matches!(
        op,
        BinOp::Eq
            | BinOp::Ne
            | BinOp::Lt
            | BinOp::Le
            | BinOp::Gt
            | BinOp::Ge
            | BinOp::And
            | BinOp::Or
    )
}

/// Returns true if a binary op always produces a number given numeric inputs.
fn arith_binop(op: &BinOp) -> bool {
    matches!(
        op,
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem
    )
}

/// Compute number-ness for every function in the program.
pub fn analyze(functions: &[MirFunction]) -> NumberTypes {
    let by_name: HashMap<&str, &MirFunction> =
        functions.iter().map(|f| (f.name.as_str(), f)).collect();

    // Call sites: callee name -> list of (caller name, args).
    let mut call_sites: HashMap<String, Vec<(String, Vec<Operand>)>> = HashMap::new();
    // Functions whose name is used as a value (not a direct callee): their
    // parameters can be invoked with unknown arguments, so stay non-number.
    let mut address_taken: HashSet<String> = HashSet::new();

    for func in functions {
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(_, rval) = &stmt.kind {
                    match rval {
                        Rvalue::Call { func: callee, args } => {
                            if let Operand::Constant(Constant::Function(name)) = callee {
                                call_sites
                                    .entry(name.clone())
                                    .or_default()
                                    .push((func.name.clone(), args.clone()));
                            }
                            // A function passed as a call argument is address-taken.
                            for a in args {
                                if let Operand::Constant(Constant::Function(n)) = a {
                                    address_taken.insert(n.clone());
                                }
                            }
                        },
                        Rvalue::Use(Operand::Constant(Constant::Function(n))) => {
                            address_taken.insert(n.clone());
                        },
                        _ => {},
                    }
                }
            }
        }
    }

    // State: per function, which locals are currently believed to be numbers.
    let mut state: HashMap<String, HashSet<usize>> = HashMap::new();
    for func in functions {
        // Optimistically assume every local is a number.
        let all: HashSet<usize> = (0..func.locals.len() + func.arg_count + 1).collect();
        state.insert(func.name.clone(), all);
    }

    let num_of = |state: &HashMap<String, HashSet<usize>>, func: &str, op: &Operand| -> bool {
        match op {
            Operand::Copy(l) | Operand::Move(l) => {
                state.get(func).is_some_and(|s| s.contains(&l.0))
            },
            Operand::Constant(c) => matches!(c, Constant::I64(_) | Constant::F64(_)),
        }
    };

    let mut changed = true;
    while changed {
        changed = false;

        // 1. Parameters: a parameter is a number iff every call site passes a
        //    number, the function is reachable from a call, and it is not invoked
        //    indirectly with unknown arguments.
        let mut param_num: HashMap<String, Vec<bool>> = HashMap::new();
        for func in functions {
            let mut pnums = vec![false; func.arg_count];
            let sites = call_sites.get(&func.name);
            let callable_directly = sites.is_some() && !address_taken.contains(&func.name);
            if callable_directly {
                for (j, pnum) in pnums.iter_mut().enumerate() {
                    *pnum = sites.unwrap().iter().all(|(caller, args)| {
                        args.get(j).is_some_and(|a| num_of(&state, caller, a))
                    });
                }
            }
            param_num.insert(func.name.clone(), pnums);
        }

        // 2. Locals: recompute from assignments. Parameters take their inferred
        //    type; temporaries and the return slot are the meet of all writers.
        for func in functions {
            let pnums = &param_num[&func.name];
            // Gather assignments per local.
            let mut writers: HashMap<usize, Vec<&Rvalue>> = HashMap::new();
            for bb in &func.basic_blocks {
                for stmt in &bb.statements {
                    if let StatementKind::Assign(l, rval) = &stmt.kind {
                        writers.entry(l.0).or_default().push(rval);
                    }
                }
            }

            let total = func.locals.len() + func.arg_count + 1;
            let mut new_set = HashSet::new();
            for idx in 0..total {
                // Parameters: Local(1..=arg_count).
                if idx >= 1 && idx <= func.arg_count {
                    if pnums[idx - 1] {
                        new_set.insert(idx);
                    }
                    continue;
                }
                let assigns = writers.get(&idx);
                let is_num = match assigns {
                    // Never written and not a parameter: treat as non-number.
                    None => false,
                    Some(rvals) => rvals
                        .iter()
                        .all(|r| rvalue_is_num(r, &state, &param_num, func, &by_name)),
                };
                if is_num {
                    new_set.insert(idx);
                }
            }

            let prev = state.get(&func.name);
            if prev != Some(&new_set) {
                changed = true;
                state.insert(func.name.clone(), new_set);
            }
        }
    }

    // Booleans are intra-procedural: a local is bool when every writer is a
    // comparison/logical op, `!`, a bool constant, or a copy of a bool. Iterate to
    // a fixpoint so copy chains converge.
    let mut bools: HashMap<String, HashSet<usize>> = HashMap::new();
    for func in functions {
        let mut writers: HashMap<usize, Vec<&Rvalue>> = HashMap::new();
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(l, rval) = &stmt.kind {
                    writers.entry(l.0).or_default().push(rval);
                }
            }
        }
        let mut set: HashSet<usize> = HashSet::new();
        let mut changed = true;
        while changed {
            changed = false;
            for (&idx, rvals) in &writers {
                if set.contains(&idx) {
                    continue;
                }
                let is_bool = rvals.iter().all(|r| match r {
                    Rvalue::BinaryOp(op, _, _) => bool_binop(op),
                    Rvalue::UnaryOp(UnOp::Not, _) => true,
                    Rvalue::Use(Operand::Constant(Constant::Bool(_))) => true,
                    Rvalue::Use(Operand::Copy(l)) | Rvalue::Use(Operand::Move(l)) => {
                        set.contains(&l.0)
                    },
                    _ => false,
                });
                if is_bool {
                    set.insert(idx);
                    changed = true;
                }
            }
        }
        bools.insert(func.name.clone(), set);
    }

    // Integers: a refinement of the numbers. A local is an integer when every
    // value that flows into it is provably integral — an integer literal, an
    // integer copy, or `+`/`-`/`*`/`%`/unary-`-` of integers. Float division
    // (`/`) is never integral. This is intra-procedural and *optimistic*
    // (assume integer, retract on a non-integer writer) so loop-carried
    // counters such as `i = i + 1` converge instead of collapsing on their
    // own back-edge.
    //
    // Parameters and the return slot stay boxed on purpose — not just "not
    // yet specialized" (an earlier version of this pass did extend across
    // direct call sites the same way the number analysis above does, and the
    // analysis itself was sound). The reason it was reverted: unlike a body
    // local, a parameter/return's *representation* is observable from
    // outside the function — the call ABI always passes/returns the boxed
    // form, so specializing one means boxing at every call site and
    // unboxing at every return, on top of the reverse at entry/return
    // inside the callee. For loop-heavy code that's amortized over many
    // arithmetic ops per call and is a clear win; for call-heavy code with
    // little work per call (the textbook case: naive recursive `fib`) it's
    // pure overhead — measured ~1.5x *slower* than leaving params/return
    // boxed. Cross-call integers would need a real cost model (or a
    // per-call-site decision, not a per-function one) to be a net win
    // across both shapes, which is a later pass, not this one.
    let mut ints: HashMap<String, HashSet<usize>> = HashMap::new();
    for func in functions {
        let total = func.locals.len() + func.arg_count + 1;
        let mut writers: HashMap<usize, Vec<&Rvalue>> = HashMap::new();
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(l, rval) = &stmt.kind {
                    writers.entry(l.0).or_default().push(rval);
                }
            }
        }
        // Optimistically assume every body local (not a parameter, not the return
        // slot 0) that's already a number is an integer, then retract.
        let nums = &state[&func.name];
        let mut set: HashSet<usize> =
            (func.arg_count + 1..total).filter(|idx| nums.contains(idx)).collect();
        let mut changed = true;
        while changed {
            let before = set.len();
            let snapshot = set.clone();
            set.retain(|&idx| match writers.get(&idx) {
                None => false,
                Some(rvals) => rvals.iter().all(|r| rvalue_is_int(r, &snapshot, func)),
            });
            changed = set.len() != before;
        }
        ints.insert(func.name.clone(), set);
    }

    NumberTypes { locals: state, bools, ints }
}

/// Whether a constant is provably integral. Ling has no integer-literal
/// syntax distinct from `number` — every source literal lowers to
/// `Constant::F64` (see `Expr::Number` in `src/mir/mod.rs`), and MIR
/// const-folding only ever combines two same-kind constants, never promotes
/// a bare F64 literal to I64. So `Constant::I64` alone is essentially
/// unreachable from ordinary source: without this, a call like
/// `modexp(base, 65537, 65521)` could never prove its literal arguments
/// integral, no matter how precise the interprocedural analysis around it is.
/// `Constant::I64` is kept as its own case because a few MIR passes (loop
/// unrolling, vectorization) synthesize genuine i64 constants directly.
pub(crate) fn constant_is_int(c: &Constant) -> bool {
    match c {
        Constant::I64(_) => true,
        Constant::F64(bits) => f64_bits_is_int(*bits),
        _ => false,
    }
}

/// The exact `i64` value of a constant operand, if it's provably integral.
/// The one place that extracts a literal's integer value — both the analysis
/// (`safe_int_divisor`, below) and codegen's `BinOp::Rem` fast path
/// (`translate.rs`) call this, so they can never independently drift apart
/// on what counts as a safe literal divisor.
pub(crate) fn int_literal_value(op: &Operand) -> Option<i64> {
    match op {
        Operand::Constant(Constant::I64(v)) => Some(*v),
        Operand::Constant(Constant::F64(bits)) if f64_bits_is_int(*bits) => {
            Some(f64::from_bits(*bits) as i64)
        },
        _ => None,
    }
}

/// Whether `op` is a literal safe to use as a `%` divisor on the native
/// `srem` path — `srem` traps on a zero divisor and on `INT_MIN % -1`, which
/// the boxed float path never does, so only a literal that provably isn't
/// either is safe to specialize.
fn safe_int_divisor(op: &Operand) -> bool {
    matches!(int_literal_value(op), Some(v) if v != 0 && v != -1)
}

/// Whether an f64 bit pattern is an exact integer safe to widen to a raw
/// `i64`. Bounded to ±2^53 (not the full i64 range): the boxing/unboxing
/// round-trip in `int_to_boxed`/`boxed_to_int` is only exact within ±2^53
/// (the largest range where every integer is itself exactly representable
/// as an f64), and ongoing arithmetic on an `int`-classified local (a loop
/// counter, an accumulator) needs to keep matching what the boxed f64 path
/// would have computed if the value ever grew past that. `-0.0` is excluded
/// separately — a raw `i64` can't carry its sign, so it must stay on the
/// boxed path or `1 / x < 0`-style sign-sensitive uses would break.
pub(crate) fn f64_bits_is_int(bits: u64) -> bool {
    let v = f64::from_bits(bits);
    v.is_finite()
        && !(v == 0.0 && v.is_sign_negative())
        && v.fract() == 0.0
        && v >= -(2u64.pow(53) as f64)
        && v <= (2u64.pow(53) as f64)
}

/// Whether an rvalue produces a strict integer, given the current
/// intra-procedural estimate for `func`'s own locals. A call's result is
/// never an integer here — see the doc comment on the `ints` pass above for
/// why parameters/returns stay boxed, which this mirrors: nothing crosses a
/// call boundary, including a self-recursive one.
fn rvalue_is_int(rval: &Rvalue, ints: &HashSet<usize>, func: &MirFunction) -> bool {
    let op_int = |op: &Operand| match op {
        Operand::Copy(l) | Operand::Move(l) => ints.contains(&l.0),
        Operand::Constant(c) => constant_is_int(c),
    };
    let _ = &func.name;
    match rval {
        Rvalue::Use(op) => op_int(op),
        Rvalue::BinaryOp(BinOp::Rem, a, b) => {
            // Codegen only takes the native-`srem` path for Rem when the
            // divisor is a safe (nonzero, non -1) *literal* — a variable
            // divisor, even one proven integral, falls back to the boxed
            // float emulation (translate_binop_rvalue's BinOp::Rem arm). If
            // this claimed "int" whenever both operands were merely int,
            // that boxed NaN result (from an actual zero/-1 divisor) would
            // get coerced through `boxed_to_int`'s saturating conversion —
            // silently turning NaN into 0 instead of staying NaN. Must match
            // the codegen's own literal-divisor check exactly.
            op_int(a) && safe_int_divisor(b)
        },
        Rvalue::BinaryOp(op, a, b) => {
            matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul) && op_int(a) && op_int(b)
        },
        Rvalue::UnaryOp(UnOp::Neg, a) => op_int(a),
        _ => false,
    }
}

/// Whether an rvalue produces a number, given the current global estimate.
fn rvalue_is_num(
    rval: &Rvalue,
    state: &HashMap<String, HashSet<usize>>,
    param_num: &HashMap<String, Vec<bool>>,
    func: &MirFunction,
    by_name: &HashMap<&str, &MirFunction>,
) -> bool {
    let op_num = |op: &Operand| -> bool {
        match op {
            Operand::Copy(l) | Operand::Move(l) => {
                state.get(&func.name).is_some_and(|s| s.contains(&l.0))
            },
            Operand::Constant(c) => matches!(c, Constant::I64(_) | Constant::F64(_)),
        }
    };
    match rval {
        Rvalue::Use(op) => op_num(op),
        Rvalue::BinaryOp(op, a, b) => arith_binop(op) && op_num(a) && op_num(b),
        Rvalue::UnaryOp(UnOp::Neg, a) => op_num(a),
        Rvalue::UnaryOp(_, _) => false,
        Rvalue::Call { func: callee, .. } => {
            // Return type of a directly-called function: number-ness of its Local 0.
            if let Operand::Constant(Constant::Function(name)) = callee {
                if by_name.contains_key(name.as_str()) {
                    // Use param_num presence to confirm it's a known function, then
                    // read its return slot from the running estimate.
                    let _ = param_num;
                    return state.get(name).is_some_and(|s| s.contains(&0));
                }
            }
            false
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ling_ast::Span;

    fn decl() -> LocalDecl {
        LocalDecl { ty: MirType::Any, name: None, span: Span::DUMMY, is_mut: false, is_owning: false }
    }
    fn stmt(kind: StatementKind) -> Statement {
        Statement { kind, span: Span::DUMMY }
    }
    fn ret() -> Terminator {
        Terminator { kind: TerminatorKind::Return, span: Span::DUMMY }
    }
    fn f64_const(v: f64) -> Operand {
        Operand::Constant(Constant::F64(v.to_bits()))
    }

    /// Regression test for a real bug: `a % b` with `a` and `b` both proven
    /// integral was classified as producing an integer result purely from
    /// `matches!(op, Add | Sub | Mul | Rem)`, even when `b` is a *variable*
    /// (not a literal) divisor. Codegen's `BinOp::Rem` fast path only takes
    /// the native `srem` path for a literal divisor it can prove is safe
    /// (nonzero, not -1); for a variable divisor it falls back to the boxed
    /// float emulation, which correctly produces NaN for a zero divisor. If
    /// the destination local were classified `int` anyway, that boxed NaN
    /// got coerced through `boxed_to_int`'s saturating conversion — silently
    /// turning `5 % 0` from NaN into 0. Caught via `ling run` vs `ling run
    /// --interp` disagreeing on `bind a = 5; bind b = 0; print(a % b)`.
    #[test]
    fn rem_with_variable_divisor_is_not_classified_as_int() {
        // fn f() { a = 5.0; b = 0.0; r = a % b; return r }
        let mut f = MirFunction::new("f", 0);
        f.locals = vec![decl(), decl(), decl()]; // Local(1)=a, Local(2)=b, Local(3)=r
        f.basic_blocks = vec![BasicBlock {
            statements: vec![
                stmt(StatementKind::Assign(Local(1), Rvalue::Use(f64_const(5.0)))),
                stmt(StatementKind::Assign(Local(2), Rvalue::Use(f64_const(0.0)))),
                stmt(StatementKind::Assign(
                    Local(3),
                    Rvalue::BinaryOp(BinOp::Rem, Operand::Copy(Local(1)), Operand::Copy(Local(2))),
                )),
                stmt(StatementKind::Assign(Local(0), Rvalue::Use(Operand::Copy(Local(3))))),
            ],
            terminator: Some(ret()),
        }];

        let nt = analyze(&[f]);
        assert!(nt.local_is_int("f", 1), "a = 5.0 (an integral literal) should be proven int");
        assert!(nt.local_is_int("f", 2), "b = 0.0 (an integral literal) should be proven int");
        assert!(
            !nt.local_is_int("f", 3),
            "a % b with a variable divisor must NOT be classified int — a real \
             zero/-1 divisor has to stay on the boxed NaN path, not get \
             silently saturated to 0"
        );
    }

    /// The safe-literal-divisor case this exists to still allow: codegen
    /// *can* prove `% 3` never traps, so this should take the fast path.
    #[test]
    fn rem_with_safe_literal_divisor_is_classified_as_int() {
        // fn f() { a = 5.0; r = a % 3.0; return r }
        let mut f = MirFunction::new("f", 0);
        f.locals = vec![decl(), decl()]; // Local(1)=a, Local(2)=r
        f.basic_blocks = vec![BasicBlock {
            statements: vec![
                stmt(StatementKind::Assign(Local(1), Rvalue::Use(f64_const(5.0)))),
                stmt(StatementKind::Assign(
                    Local(2),
                    Rvalue::BinaryOp(BinOp::Rem, Operand::Copy(Local(1)), f64_const(3.0)),
                )),
                stmt(StatementKind::Assign(Local(0), Rvalue::Use(Operand::Copy(Local(2))))),
            ],
            terminator: Some(ret()),
        }];

        let nt = analyze(&[f]);
        assert!(
            nt.local_is_int("f", 2),
            "a % 3.0 with a safe literal divisor should be classified int"
        );
    }

    /// `% 0` and `% -1` are the two divisors `srem` traps on — even as
    /// literals, these must not take the fast path.
    #[test]
    fn rem_with_unsafe_literal_divisor_is_not_classified_as_int() {
        for divisor in [0.0, -1.0] {
            let mut f = MirFunction::new("f", 0);
            f.locals = vec![decl(), decl()];
            f.basic_blocks = vec![BasicBlock {
                statements: vec![
                    stmt(StatementKind::Assign(Local(1), Rvalue::Use(f64_const(5.0)))),
                    stmt(StatementKind::Assign(
                        Local(2),
                        Rvalue::BinaryOp(BinOp::Rem, Operand::Copy(Local(1)), f64_const(divisor)),
                    )),
                    stmt(StatementKind::Assign(Local(0), Rvalue::Use(Operand::Copy(Local(2))))),
                ],
                terminator: Some(ret()),
            }];

            let nt = analyze(&[f]);
            assert!(
                !nt.local_is_int("f", 2),
                "a % {divisor} must not be classified int — srem traps on this divisor"
            );
        }
    }

    /// Deliberate design boundary, not a missing feature: even when a
    /// parameter is only ever called with an integer argument, it must stay
    /// boxed. An earlier version of this pass *did* prove parameters/returns
    /// integral across direct call sites — it was reverted after measuring
    /// it make purely-recursive, call-heavy code (naive `fib`) ~1.5x slower,
    /// since the call ABI always passes/returns the boxed form regardless,
    /// so specializing a parameter means boxing at every call site and
    /// unboxing at every entry (and the mirror image for a specialized
    /// return) — overhead that only pays for itself when there's enough
    /// per-call arithmetic to amortize it against, which a per-function
    /// (not per-call-site) analysis can't tell apart.
    #[test]
    fn parameter_stays_boxed_even_when_every_call_site_passes_an_integer() {
        // fn callee(m) { r = 7.0 % m; return r }
        let mut callee = MirFunction::new("callee", 1);
        callee.locals = vec![decl()]; // Local(2) = r
        callee.basic_blocks = vec![BasicBlock {
            statements: vec![
                stmt(StatementKind::Assign(
                    Local(2),
                    Rvalue::BinaryOp(BinOp::Rem, f64_const(7.0), Operand::Copy(Local(1))),
                )),
                stmt(StatementKind::Assign(Local(0), Rvalue::Use(Operand::Copy(Local(2))))),
            ],
            terminator: Some(ret()),
        }];

        // fn caller() { r = callee(3.0); return r }
        let mut caller = MirFunction::new("caller", 0);
        caller.locals = vec![decl()];
        caller.basic_blocks = vec![BasicBlock {
            statements: vec![
                stmt(StatementKind::Assign(
                    Local(1),
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function("callee".into())),
                        args: vec![f64_const(3.0)],
                    },
                )),
                stmt(StatementKind::Assign(Local(0), Rvalue::Use(Operand::Copy(Local(1))))),
            ],
            terminator: Some(ret()),
        }];

        let nt = analyze(&[callee, caller]);
        assert!(
            !nt.local_is_int("callee", 1),
            "parameters must stay boxed regardless of what call sites pass"
        );
        assert!(!nt.local_is_int("callee", 0), "the return slot must stay boxed too");
    }
}
