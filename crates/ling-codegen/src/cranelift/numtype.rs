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
            Operand::Constant(Constant::I64(_)) => true,
            _ => false,
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
    // Escape hatch: treat every local as fully boxed (no num/int/bool fast path).
    // Used to bisect whether the numeric type inference is miscompiling a program.
    if std::env::var_os("LING_JIT_NO_NUMOPT").is_some() {
        return NumberTypes {
            locals: HashMap::new(),
            bools: HashMap::new(),
            ints: HashMap::new(),
        };
    }
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
    // (`/`) is never integral. This is intra-procedural and *optimistic* (assume
    // integer, retract on a non-integer writer) so loop-carried counters such as
    // `i = i + 1` converge instead of collapsing on their own back-edge.
    //
    // Parameters and the return slot stay boxed: the call ABI is unchanged, so
    // only function-local integers are specialized. That already covers the tight
    // counter/accumulator loops; cross-call integers are left for a later pass.
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
        // slot 0) is an integer, then retract.
        let mut set: HashSet<usize> = (func.arg_count + 1..total).collect();
        let int_op = |set: &HashSet<usize>, op: &Operand| -> bool {
            match op {
                Operand::Copy(l) | Operand::Move(l) => set.contains(&l.0),
                Operand::Constant(Constant::I64(_)) => true,
                _ => false,
            }
        };
        let mut changed = true;
        while changed {
            let before = set.len();
            let snapshot = set.clone();
            set.retain(|&idx| match writers.get(&idx) {
                None => false,
                Some(rvals) => rvals.iter().all(|r| match r {
                    Rvalue::Use(op) => int_op(&snapshot, op),
                    Rvalue::BinaryOp(op, a, b) => {
                        matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Rem)
                            && int_op(&snapshot, a)
                            && int_op(&snapshot, b)
                    },
                    Rvalue::UnaryOp(UnOp::Neg, a) => int_op(&snapshot, a),
                    _ => false,
                }),
            });
            changed = set.len() != before;
        }
        ints.insert(func.name.clone(), set);
    }

    NumberTypes { locals: state, bools, ints }
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
