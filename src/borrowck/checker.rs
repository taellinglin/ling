use ling_ast::Span;
use ling_mir::ir::*;
use ling_mir::liveness::Liveness;
use std::collections::{HashMap, HashSet, VecDeque};
use rustc_hash::FxHashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalState {
    Initialized,
    Moved,
    Dead,
}

#[derive(Debug, Clone)]
struct FlowState {
    locals: Vec<LocalState>,
    borrows: Vec<(usize, bool)>,
}

impl FlowState {
    fn new(num_locals: usize) -> Self {
        Self {
            locals: vec![LocalState::Dead; num_locals],
            borrows: vec![(0, false); num_locals],
        }
    }

    fn get(&self, local: Local) -> LocalState {
        self.locals
            .get(local.0)
            .copied()
            .unwrap_or(LocalState::Dead)
    }

    fn set(&mut self, local: Local, state: LocalState) -> Result<(), String> {
        if local.0 < self.locals.len() {
            let current_borrows = self.borrows[local.0];
            if current_borrows.0 > 0 || current_borrows.1 {
                if state != LocalState::Initialized {
                    return Err("cannot move or drop variable while it is borrowed".to_string());
                } else {
                    return Err("cannot reassign variable while it is borrowed".to_string());
                }
            }
            self.locals[local.0] = state;
            if state != LocalState::Initialized {
                self.borrows[local.0] = (0, false);
            }
        }
        Ok(())
    }

    fn join(&mut self, other: &FlowState) -> bool {
        let mut changed = false;
        let len = self.locals.len().max(other.locals.len());
        if self.locals.len() < len {
            self.locals.resize(len, LocalState::Dead);
            self.borrows.resize(len, (0, false));
        }
        for (i, other_state) in other.locals.iter().enumerate() {
            let merged = merge_states(self.locals[i], *other_state);
            if merged != self.locals[i] {
                self.locals[i] = merged;
                changed = true;
            }
        }
        for i in 0..self.borrows.len().min(other.borrows.len()) {
            let new_count = self.borrows[i].0.max(other.borrows[i].0);
            let new_mut = self.borrows[i].1 || other.borrows[i].1;
            if self.borrows[i].0 != new_count || self.borrows[i].1 != new_mut {
                self.borrows[i] = (new_count, new_mut);
                changed = true;
            }
        }
        changed
    }
}

fn merge_states(a: LocalState, b: LocalState) -> LocalState {
    match (a, b) {
        (LocalState::Moved, _) | (_, LocalState::Moved) => LocalState::Moved,
        (LocalState::Initialized, _) | (_, LocalState::Initialized) => LocalState::Initialized,
        _ => LocalState::Dead,
    }
}

pub struct BorrowChecker<'a> {
    pub func: &'a MirFunction,
    pub errors: Vec<String>,
    pub liveness: Liveness,
    pub provenance: HashMap<Local, Local>,
}

impl<'a> BorrowChecker<'a> {
    pub fn new(func: &'a MirFunction) -> Self {
        let liveness = Liveness::compute(func);
        Self {
            func,
            liveness,
            errors: Vec::new(),
            provenance: HashMap::default(),
        }
    }

    pub fn check(&mut self) {
        if self.func.basic_blocks.is_empty() {
            return;
        }

        let num_blocks = self.func.basic_blocks.len();
        // Flat local space: return slot, parameters, then temporaries. The
        // return slot and parameters have no entry in `func.locals`.
        let num_slots = self.func.next_local();

        let mut entry_states: Vec<Option<FlowState>> = vec![None; num_blocks];

        let mut init_state = FlowState::new(num_slots);
        let _ = init_state.set(Local(0), LocalState::Initialized);
        for i in 1..=self.func.arg_count {
            let _ = init_state.set(Local(i), LocalState::Initialized);
        }
        entry_states[0] = Some(init_state);

        let mut worklist: VecDeque<usize> = VecDeque::new();
        worklist.push_back(0);
        let mut in_worklist: Vec<bool> = vec![false; num_blocks];
        in_worklist[0] = true;

        while let Some(bb_idx) = worklist.pop_front() {
            in_worklist[bb_idx] = false;

            let state = match &entry_states[bb_idx] {
                Some(s) => s.clone(),
                None => continue,
            };

            let mut state = state;
            let bb = &self.func.basic_blocks[bb_idx];

            for (stmt_idx, stmt) in bb.statements.iter().enumerate() {
                self.transfer_stmt(stmt, &mut state);
                let live_after = &self.liveness.live_after[bb_idx][stmt_idx + 1];
                self.release_dead_borrows(&mut state, live_after);
            }

            let term_idx = bb.statements.len();
            if let Some(term) = &bb.terminator {
                self.check_terminator(term, &mut state);
            }

            let live_after_term = &self.liveness.live_after[bb_idx][term_idx];
            self.release_dead_borrows(&mut state, live_after_term);

            let successors = self.successors(bb);
            for succ in successors {
                let changed = match &mut entry_states[succ] {
                    None => {
                        entry_states[succ] = Some(state.clone());
                        true
                    },
                    Some(existing) => existing.join(&state),
                };
                if changed && !in_worklist[succ] {
                    worklist.push_back(succ);
                    in_worklist[succ] = true;
                }
            }
        }
    }

    fn transfer_stmt(&mut self, stmt: &Statement, state: &mut FlowState) {
        match &stmt.kind {
            StatementKind::Assign(lhs, rvalue) => {
                self.check_rvalue(rvalue, state, stmt.span);

                match rvalue {
                    Rvalue::Ref(rhs) | Rvalue::MutRef(rhs) => {
                        self.provenance.insert(*lhs, *rhs);
                    },
                    Rvalue::Use(Operand::Copy(rhs)) | Rvalue::Use(Operand::Move(rhs)) => {
                        if let Some(prov) = self.provenance.get(rhs).cloned() {
                            self.provenance.insert(*lhs, prov);
                        }
                    },
                    _ => {
                        self.provenance.remove(lhs);
                    },
                }

                if let Err(msg) = state.set(*lhs, LocalState::Initialized) {
                    let name = self.local_name(*lhs);
                    self.errors.push(format!("{} `{}`", msg, name));
                }
            },
            StatementKind::SetAttr(obj, _field, val) => {
                self.check_mutation(obj, state, stmt.span);
                self.check_operand(obj, state, stmt.span);
                self.check_operand(val, state, stmt.span);
            },
            StatementKind::SetIndex(obj, idx, val) => {
                self.check_mutation(obj, state, stmt.span);
                self.check_operand(obj, state, stmt.span);
                self.check_operand(idx, state, stmt.span);
                self.check_operand(val, state, stmt.span);
            },
            StatementKind::StorageLive(local) => {
                let _ = state.set(*local, LocalState::Initialized);
            },
            StatementKind::StorageDead(local) => {
                let _ = state.set(*local, LocalState::Dead);
            },
            StatementKind::Drop(local) => {
                if state.get(*local) == LocalState::Initialized {
                    if let Err(msg) = state.set(*local, LocalState::Moved) {
                        let name = self.local_name(*local);
                        self.errors
                            .push(format!("{} `{}` (lifetime error)", msg, name));
                    }
                }
            },
            StatementKind::VectorStore(obj, idx, val) => {
                self.check_mutation(obj, state, stmt.span);
                self.check_operand(obj, state, stmt.span);
                self.check_operand(idx, state, stmt.span);
                self.check_operand(val, state, stmt.span);
            },
        }
    }

    fn check_terminator(&mut self, term: &Terminator, state: &mut FlowState) {
        match &term.kind {
            TerminatorKind::SwitchInt { discr, .. } => {
                self.check_operand(discr, state, term.span);
            },
            TerminatorKind::Return | TerminatorKind::Goto { .. } | TerminatorKind::Unreachable => {
            },
        }
    }

    fn successors(&self, bb: &BasicBlock) -> Vec<usize> {
        match &bb.terminator {
            Some(t) => match &t.kind {
                TerminatorKind::Goto { target } => vec![target.0],
                TerminatorKind::SwitchInt { targets, otherwise, .. } => {
                    let mut succs: Vec<usize> = targets.iter().map(|(_, bb)| bb.0).collect();
                    succs.push(otherwise.0);
                    succs
                },
                TerminatorKind::Return | TerminatorKind::Unreachable => vec![],
            },
            None => vec![],
        }
    }

    fn check_rvalue(&mut self, rvalue: &Rvalue, state: &mut FlowState, span: Span) {
        match rvalue {
            Rvalue::Use(op) => self.check_operand(op, state, span),
            Rvalue::BinaryOp(_, lhs, rhs) => {
                self.check_operand(lhs, state, span);
                self.check_operand(rhs, state, span);
            },
            Rvalue::UnaryOp(_, op) => self.check_operand(op, state, span),
            Rvalue::Call { func, args } => {
                self.check_operand(func, state, span);
                for arg in args {
                    self.check_operand(arg, state, span);
                }
            },
            Rvalue::Aggregate(_, ops) => {
                for op in ops {
                    self.check_operand(op, state, span);
                }
            },
            Rvalue::GetAttr(op, _) => self.check_operand(op, state, span),
            Rvalue::GetIndex(obj, idx) => {
                self.check_operand(obj, state, span);
                self.check_operand(idx, state, span);
            },
            Rvalue::Ref(local) => {
                let s = state.get(*local);
                if s != LocalState::Initialized {
                    let name = self.local_name(*local);
                    self.errors.push(format!(
                        "cannot borrow uninitialized or moved variable `{}`",
                        name
                    ));
                }
                if local.0 < state.borrows.len() {
                    let borrow = &mut state.borrows[local.0];
                    if borrow.1 {
                        let name = self.local_name(*local);
                        self.errors.push(format!(
                            "cannot borrow `{}` as immutable because it is also borrowed as mutable",
                            name
                        ));
                    }
                    borrow.0 += 1;
                }
            },
            Rvalue::MutRef(local) => {
                let s = state.get(*local);
                if s != LocalState::Initialized {
                    let name = self.local_name(*local);
                    self.errors.push(format!(
                        "cannot mutably borrow uninitialized or moved variable `{}`",
                        name
                    ));
                }
                if let Some(decl) = self.func.local_decl(*local) {
                    if !decl.is_mut {
                        let name = self.local_name(*local);
                        self.errors.push(format!(
                            "cannot mutably borrow immutable variable `{}`",
                            name
                        ));
                    }
                }
                if local.0 < state.borrows.len() {
                    let borrow = &mut state.borrows[local.0];
                    if borrow.0 > 0 || borrow.1 {
                        let name = self.local_name(*local);
                        self.errors.push(format!(
                            "cannot borrow `{}` as mutable because it is already borrowed",
                            name
                        ));
                    }
                    borrow.1 = true;
                }
            },
            Rvalue::VectorSplat(op, _) => self.check_operand(op, state, span),
            Rvalue::VectorLoad(obj, idx, _) => {
                self.check_operand(obj, state, span);
                self.check_operand(idx, state, span);
            },
            Rvalue::VectorFMA(a, b, c) => {
                self.check_operand(a, state, span);
                self.check_operand(b, state, span);
                self.check_operand(c, state, span);
            },
            Rvalue::InlineAsm(_) => {},
        }
    }

    fn check_mutation(&mut self, op: &Operand, state: &FlowState, _span: Span) {
        if let Operand::Copy(local) | Operand::Move(local) = op {
            if local.0 < state.borrows.len() {
                let borrow = state.borrows[local.0];
                if borrow.0 > 0 || borrow.1 {
                    let name = self.local_name(*local);
                    self.errors
                        .push(format!("cannot mutate `{}` because it is borrowed", name));
                }
            }
        }
    }

    fn check_operand(&mut self, op: &Operand, state: &mut FlowState, _span: Span) {
        match op {
            Operand::Copy(local) | Operand::Move(local) => match state.get(*local) {
                LocalState::Dead => {
                    let is_unnamed = self
                        .func
                        .local_decl(*local)
                        .is_none_or(|d| d.name.is_none());
                    if is_unnamed {
                        return;
                    }
                    let name = self.local_name(*local);
                    self.errors
                        .push(format!("use of possibly uninitialized variable `{}`", name));
                },
                LocalState::Moved => {
                    let name = self.local_name(*local);
                    self.errors
                        .push(format!("use of moved variable `{}`", name));
                },
                LocalState::Initialized => {
                    if let Operand::Move(local) = op {
                        if self
                            .func
                            .local_decl(*local)
                            .is_some_and(|d| d.ty.is_move_type())
                        {
                            if let Err(msg) = state.set(*local, LocalState::Moved) {
                                let name = self.local_name(*local);
                                self.errors.push(format!("{} `{}`", msg, name));
                            }
                        }
                    }
                },
            },
            Operand::Constant(_) => {},
        }
    }

    fn release_dead_borrows(&self, state: &mut FlowState, live_locals: &FxHashSet<Local>) {
        let mut still_borrowed = HashSet::new();
        for (ref_var, pointed_var) in &self.provenance {
            if live_locals.contains(ref_var) {
                still_borrowed.insert(*pointed_var);
            }
        }

        for (ref_var, pointed_var) in &self.provenance {
            if !live_locals.contains(ref_var)
                && !still_borrowed.contains(pointed_var)
                && pointed_var.0 < state.borrows.len()
            {
                let borrow = &mut state.borrows[pointed_var.0];
                if borrow.0 > 0 {
                    borrow.0 -= 1;
                } else {
                    borrow.1 = false;
                }
            }
        }
    }

    fn local_name(&self, local: Local) -> String {
        self.func
            .local_decl(local)
            .and_then(|decl| decl.name.as_ref())
            .cloned()
            .unwrap_or_else(|| format!("_{}", local.0))
    }
}
