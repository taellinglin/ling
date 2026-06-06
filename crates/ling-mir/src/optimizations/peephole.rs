use super::Transform;
use crate::ir::*;
use ling_ast::ast::BinOp;

pub struct PeepholeOptimize;

impl Transform for PeepholeOptimize {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(_, rval) = &mut stmt.kind {
                    match rval {
                        Rvalue::BinaryOp(BinOp::Add, op, Operand::Constant(Constant::I64(0)))
                        | Rvalue::BinaryOp(BinOp::Add, Operand::Constant(Constant::I64(0)), op)
                        | Rvalue::BinaryOp(BinOp::Sub, op, Operand::Constant(Constant::I64(0)))
                        | Rvalue::BinaryOp(BinOp::Mul, op, Operand::Constant(Constant::I64(1)))
                        | Rvalue::BinaryOp(BinOp::Mul, Operand::Constant(Constant::I64(1)), op)
                        | Rvalue::BinaryOp(BinOp::Div, op, Operand::Constant(Constant::I64(1))) => {
                            *rval = Rvalue::Use(op.clone());
                            changed = true;
                        },
                        Rvalue::BinaryOp(
                            BinOp::Mul,
                            _,
                            op @ Operand::Constant(Constant::I64(0)),
                        )
                        | Rvalue::BinaryOp(
                            BinOp::Mul,
                            op @ Operand::Constant(Constant::I64(0)),
                            _,
                        ) => {
                            *rval = Rvalue::Use(op.clone());
                            changed = true;
                        },
                        Rvalue::BinaryOp(BinOp::Div, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::I64(1)));
                            changed = true;
                        },
                        Rvalue::BinaryOp(BinOp::Mul, op, Operand::Constant(Constant::I64(2)))
                        | Rvalue::BinaryOp(BinOp::Mul, Operand::Constant(Constant::I64(2)), op) => {
                            *rval = Rvalue::BinaryOp(BinOp::Add, op.clone(), op.clone());
                            changed = true;
                        },
                        Rvalue::BinaryOp(BinOp::Eq, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(true)));
                            changed = true;
                        },
                        Rvalue::BinaryOp(BinOp::Ne, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(false)));
                            changed = true;
                        },
                        Rvalue::BinaryOp(BinOp::Lt, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(false)));
                            changed = true;
                        },
                        Rvalue::BinaryOp(BinOp::Gt, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(false)));
                            changed = true;
                        },
                        Rvalue::BinaryOp(BinOp::Le, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(true)));
                            changed = true;
                        },
                        Rvalue::BinaryOp(BinOp::Ge, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::Bool(true)));
                            changed = true;
                        },
                        Rvalue::BinaryOp(BinOp::Sub, l, r) if l == r => {
                            *rval = Rvalue::Use(Operand::Constant(Constant::I64(0)));
                            changed = true;
                        },
                        Rvalue::BinaryOp(BinOp::Shl, op, Operand::Constant(Constant::I64(0)))
                        | Rvalue::BinaryOp(BinOp::Shr, op, Operand::Constant(Constant::I64(0))) => {
                            *rval = Rvalue::Use(op.clone());
                            changed = true;
                        },
                        Rvalue::BinaryOp(BinOp::BitAnd, _, Operand::Constant(Constant::I64(0)))
                        | Rvalue::BinaryOp(BinOp::BitAnd, Operand::Constant(Constant::I64(0)), _) =>
                        {
                            *rval = Rvalue::Use(Operand::Constant(Constant::I64(0)));
                            changed = true;
                        },
                        Rvalue::BinaryOp(BinOp::BitOr, op, Operand::Constant(Constant::I64(0)))
                        | Rvalue::BinaryOp(BinOp::BitOr, Operand::Constant(Constant::I64(0)), op) =>
                        {
                            *rval = Rvalue::Use(op.clone());
                            changed = true;
                        },
                        Rvalue::BinaryOp(BinOp::BitAnd, l, r) if l == r => {
                            *rval = Rvalue::Use(l.clone());
                            changed = true;
                        },
                        Rvalue::BinaryOp(BinOp::BitOr, l, r) if l == r => {
                            *rval = Rvalue::Use(l.clone());
                            changed = true;
                        },
                        _ => {},
                    }
                }
            }
        }
        changed
    }
}
