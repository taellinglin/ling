use super::Transform;
use crate::ir::*;
use ling_ast::ast::BinOp;

pub struct StrengthReduction;

impl Transform for StrengthReduction {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(_, rval) = &mut stmt.kind {
                    let replacement = match &*rval {
                        Rvalue::BinaryOp(BinOp::Mul, op, Operand::Constant(Constant::I64(c)))
                        | Rvalue::BinaryOp(BinOp::Mul, Operand::Constant(Constant::I64(c)), op) => {
                            if *c > 2 && (*c as u64).is_power_of_two() {
                                let shift = (*c as u64).trailing_zeros() as i64;
                                Some(Rvalue::BinaryOp(
                                    BinOp::Shl,
                                    op.clone(),
                                    Operand::Constant(Constant::I64(shift)),
                                ))
                            } else {
                                None
                            }
                        },
                        Rvalue::BinaryOp(BinOp::Div, op, Operand::Constant(Constant::I64(c))) => {
                            if *c > 1 && (*c as u64).is_power_of_two() {
                                let shift = (*c as u64).trailing_zeros() as i64;
                                Some(Rvalue::BinaryOp(
                                    BinOp::Shr,
                                    op.clone(),
                                    Operand::Constant(Constant::I64(shift)),
                                ))
                            } else {
                                None
                            }
                        },
                        Rvalue::BinaryOp(BinOp::Rem, op, Operand::Constant(Constant::I64(c))) => {
                            if *c > 1 && (*c as u64).is_power_of_two() {
                                let mask = *c - 1;
                                Some(Rvalue::BinaryOp(
                                    BinOp::BitAnd,
                                    op.clone(),
                                    Operand::Constant(Constant::I64(mask)),
                                ))
                            } else {
                                None
                            }
                        },
                        _ => None,
                    };
                    if let Some(new_rval) = replacement {
                        *rval = new_rval;
                        changed = true;
                    }
                }
            }
        }
        changed
    }
}
