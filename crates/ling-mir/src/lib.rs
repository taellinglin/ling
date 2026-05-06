//! Ling MIR - Robust Mid-level Intermediate Representation for code generation backends.
//!
//! This crate is intentionally self-contained so it can compile in WIP monorepos.
//! It provides:
//! - A typed, SSA-flavored MIR (phi nodes supported)
//! - Basic blocks and control-flow graph
//! - A small but extensible instruction set
//! - Verification and simple optimizations
//!
//! It does not currently implement a full frontend (parser/semantic lowering). The
//! goal is to provide a solid IR core that other crates (like codegen backends)
//! can rely on.

use std::collections::{BTreeMap, BTreeSet};

/// A simple, stable identifier for values produced/used by instructions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValueId(pub u32);

/// A simple, stable identifier for basic blocks.
#[derive(Clone, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId(pub u32);

/// A simple, stable identifier for functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FunctionId(pub u32);

/// Primitive MIR types.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    /// Machine-sized signed integer.
    I64,
    /// Machine-sized unsigned integer.
    U64,
    /// 32-bit floating point.
    F32,
    /// 64-bit floating point.
    F64,
    /// Boolean.
    Bool,
    /// Unit / void.
    Unit,
    /// Pointer-like typed value (opaque for now).
    Ptr(Box<Type>),
}

impl Type {
    pub fn is_unit(&self) -> bool {
        matches!(self, Type::Unit)
    }
}

/// A whole MIR program containing multiple functions.
#[derive(Clone, Debug, Default)]
pub struct MirProgram {
    pub functions: Vec<MirFunction>,
}

/// A MIR function (CFG + instruction list).
#[derive(Clone, Debug)]
pub struct MirFunction {
    pub name: String,
    pub ret_type: Type,
    pub args: Vec<(String, Type)>,
    pub blocks: BTreeMap<BlockId, MirBlock>,
    pub entry: BlockId,
    pub next_value: u32,
}

impl MirFunction {
    pub fn new(name: impl Into<String>, ret_type: Type, args: Vec<(String, Type)>) -> Self {
        Self {
            name: name.into(),
            ret_type,
            args,
            blocks: BTreeMap::new(),
            entry: BlockId(0),
            next_value: 0,
        }
    }

    pub fn alloc_value(&mut self) -> ValueId {
        let id = self.next_value;
        self.next_value += 1;
        ValueId(id)
    }

    pub fn add_block(&mut self, id: BlockId) {
        self.blocks
            .entry(id)
            .or_insert_with(|| MirBlock::new(id));
    }

    pub fn block(&self, id: BlockId) -> Option<&MirBlock> {
        self.blocks.get(&id)
    }

    pub fn block_mut(&mut self, id: BlockId) -> Option<&mut MirBlock> {
        self.blocks.get_mut(&id)
    }
}

/// A basic block.
#[derive(Clone, Debug)]
pub struct MirBlock {
    pub id: BlockId,
    pub insts: Vec<Instruction>,
    pub terminator: Option<Terminator>,
}

impl MirBlock {
    pub fn new(id: BlockId) -> Self {
        Self {
            id,
            insts: Vec::new(),
            terminator: None,
        }
    }
}

/// An SSA-like instruction.
#[derive(Clone, Debug)]
pub enum Instruction {
    /// v = const
    Const {
        dst: ValueId,
        ty: Type,
        value: ConstValue,
    },

    /// v = a <op> b
    BinOp {
        dst: ValueId,
        ty: Type,
        op: BinOp,
        lhs: ValueId,
        rhs: ValueId,
    },

    /// v = select cond ? a : b
    Select {
        dst: ValueId,
        ty: Type,
        cond: ValueId,
        then_val: ValueId,
        else_val: ValueId,
    },

    /// v = phi [v1, bb1], [v2, bb2], ...
    Phi {
        dst: ValueId,
        ty: Type,
        incomings: Vec<(ValueId, BlockId)>,
    },

    /// v = call f(args...)
    Call {
        dst: Option<ValueId>,
        callee: String,
        args: Vec<ValueId>,
        ret_type: Type,
    },

    /// v = load ptr
    Load {
        dst: ValueId,
        ty: Type,
        ptr: ValueId,
    },

    /// store ptr, val
    Store {
        ptr: ValueId,
        val: ValueId,
    },
}

#[derive(Clone, Debug)]
pub enum ConstValue {
    I64(i64),
    U64(u64),
    F32(f32),
    F64(f64),
    Bool(bool),
}

#[derive(Clone, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Control flow terminator.
#[derive(Clone, Debug)]
pub enum Terminator {
    /// br bb
    Jump(BlockId),
    /// br cond ? then_bb : else_bb
    Branch {
        cond: ValueId,
        then_bb: BlockId,
        else_bb: BlockId,
    },
    /// return v
    Return(Option<ValueId>),
}

/// Verification errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// A block is missing a terminator.
    MissingTerminator(BlockId),
    /// A terminator references a non-existent block.
    UnknownTarget { from: BlockId, target: BlockId },
    /// Phi node has empty incomings.
    PhiEmpty(BlockId, ValueId),
    /// Phi node incomings contain duplicated predecessor blocks.
    PhiDuplicateIncomingBlock(BlockId, ValueId, BlockId),
    /// A function entry block is not present.
    MissingEntry,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use VerifyError::*;
        match self {
            MissingTerminator(b) => write!(f, "block {b:?} missing terminator"),
            UnknownTarget { from, target } => {
                write!(f, "block {from:?} terminator targets unknown block {target:?}")
            }
            PhiEmpty(b, v) => write!(f, "phi in block {b:?} for dst {v:?} has no incomings"),
            PhiDuplicateIncomingBlock(b, v, inc) => {
                write!(f, "phi in block {b:?} for dst {v:?} has duplicate incoming from {inc:?}")
            }
            MissingEntry => write!(f, "missing entry block"),
        }
    }
}

impl std::error::Error for VerifyError {}

/// A MIR builder for a single function.
///
/// Usage:
/// - builder = MirBuilder::new(...)
/// - builder.start_block(...)
/// - builder.emit(...)
/// - builder.set_terminator(...)
/// - finish.
#[derive(Debug)]
pub struct MirBuilder {
    pub func: MirFunction,
    curr: BlockId,
}

impl MirBuilder {
    pub fn new(name: impl Into<String>, ret_type: Type, args: Vec<(String, Type)>) -> Self {
        let mut func = MirFunction::new(name, ret_type, args);
        let entry = BlockId(0);
        func.entry = entry;
        func.add_block(entry);
        Self { func, curr: entry }
    }

    pub fn start_block(&mut self, id: BlockId) {
        if !self.func.blocks.contains_key(&id) {
            self.func.add_block(id);
        }
        self.curr = id;
    }

    pub fn emit(&mut self, inst: Instruction) {
        let b = self
            .func
            .block_mut(self.curr)
            .expect("current block must exist");
        b.insts.push(inst);
    }

    pub fn set_terminator(&mut self, term: Terminator) {
        let b = self
            .func
            .block_mut(self.curr)
            .expect("current block must exist");
        b.terminator = Some(term);
    }

    pub fn finish(self) -> MirFunction {
        self.func
    }
}

impl MirFunction {
    /// Verify structural invariants (not full SSA correctness).
    pub fn verify(&self) -> Result<(), VerifyError> {
        if !self.blocks.contains_key(&self.entry) {
            return Err(VerifyError::MissingEntry);
        }

        for (bid, block) in &self.blocks {
            if block.terminator.is_none() {
                return Err(VerifyError::MissingTerminator(*bid));
            }

            // Check terminator targets
            let term = block.terminator.as_ref().unwrap();
            match term {
                Terminator::Jump(t) => {
                    if !self.blocks.contains_key(t) {
                        return Err(VerifyError::UnknownTarget {
                            from: *bid,
                            target: *t,
                        });
                    }
                }
                Terminator::Branch { then_bb, else_bb, .. } => {
                    for t in [then_bb, else_bb] {
                        if !self.blocks.contains_key(t) {
                            return Err(VerifyError::UnknownTarget {
                                from: *bid,
                                target: *t,
                            });
                        }
                    }
                }
                Terminator::Return(_) => {}
            }

            // Phi checks
            for inst in &block.insts {
                if let Instruction::Phi { dst, ty: _, incomings } = inst {
                    if incomings.is_empty() {
                        return Err(VerifyError::PhiEmpty(*bid, *dst));
                    }
                    let mut seen = BTreeSet::new();
                    for (_, pred) in incomings {
                        if !seen.insert(*pred) {
                            return Err(VerifyError::PhiDuplicateIncomingBlock(
                                *bid,
                                *dst,
                                *pred,
                            ));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Run a very small set of optimizations:
    /// - Remove trivially dead instructions with Unit type (placeholder pass)
    pub fn optimize_simple(&mut self) {
        for block in self.blocks.values_mut() {
            block.insts.retain(|inst| match inst {
                Instruction::Const { ty, .. } => !ty.is_unit(),
                Instruction::BinOp { ty, .. } => !ty.is_unit(),
                Instruction::Select { ty, .. } => !ty.is_unit(),
                Instruction::Phi { ty, .. } => !ty.is_unit(),
                Instruction::Call { dst, ret_type, .. } => {
                    // keep if it has side-effect or produces value
                    // We conservatively keep calls always if dst is Some.
                    dst.is_some() || !ret_type.is_unit()
                }
                Instruction::Load { ty, .. } => !ty.is_unit(),
                Instruction::Store { .. } => true, // side effect
            });
        }
    }

    /// Collect blocks reachable from entry.
    pub fn reachable_blocks(&self) -> BTreeSet<BlockId> {
        let mut visited = BTreeSet::new();
        let mut stack = vec![self.entry];
        while let Some(b) = stack.pop() {
            if !visited.insert(b) {
                continue;
            }
            let block = match self.blocks.get(&b) {
                Some(b) => b,
                None => continue,
            };
            let term = match &block.terminator {
                Some(t) => t,
                None => continue,
            };
            match term {
                Terminator::Jump(t) => stack.push(*t),
                Terminator::Branch { then_bb, else_bb, .. } => {
                    stack.push(*then_bb);
                    stack.push(*else_bb);
                }
                Terminator::Return(_) => {}
            }
        }
        visited
    }
}

/// A whole-program utility namespace.
pub mod program {
    use super::*;

    pub fn verify_program(p: &MirProgram) -> Result<(), VerifyError> {
        for f in &p.functions {
            f.verify()?;
        }
        Ok(())
    }
}

/// Convenience helpers for clients.
pub mod prelude {
    pub use super::{
        BinOp, BlockId, ConstValue, Instruction, MirBuilder, MirFunction, MirProgram, Terminator,
        Type, ValueId,
    };
}

