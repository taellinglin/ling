use super::Transform;
use crate::ir::*;
use ling_ast::ast::{BinOp, UnOp};

pub struct ConstantFolding;

impl Transform for ConstantFolding {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(_, rval) = &mut stmt.kind {
                    if let Rvalue::BinaryOp(
                        op,
                        Operand::Constant(Constant::I64(a)),
                        Operand::Constant(Constant::I64(b)),
                    ) = rval
                    {
                        let res = match op {
                            BinOp::Add => Some(Constant::I64((*a).wrapping_add(*b))),
                            BinOp::Sub => Some(Constant::I64((*a).wrapping_sub(*b))),
                            BinOp::Mul => Some(Constant::I64((*a).wrapping_mul(*b))),
                            BinOp::Div => {
                                if *b != 0 {
                                    Some(Constant::I64(*a / *b))
                                } else {
                                    None
                                }
                            },
                            BinOp::Rem => {
                                if *b != 0 {
                                    Some(Constant::I64(*a % *b))
                                } else {
                                    None
                                }
                            },
                            BinOp::Eq => Some(Constant::Bool(*a == *b)),
                            BinOp::Ne => Some(Constant::Bool(*a != *b)),
                            BinOp::Lt => Some(Constant::Bool(*a < *b)),
                            BinOp::Le => Some(Constant::Bool(*a <= *b)),
                            BinOp::Gt => Some(Constant::Bool(*a > *b)),
                            BinOp::Ge => Some(Constant::Bool(*a >= *b)),
                            BinOp::Shl => Some(Constant::I64((*a).wrapping_shl(*b as u32))),
                            BinOp::Shr => Some(Constant::I64((*a).wrapping_shr(*b as u32))),
                            _ => None,
                        };
                        if let Some(val) = res {
                            *rval = Rvalue::Use(Operand::Constant(val));
                            changed = true;
                        }
                    } else if let Rvalue::BinaryOp(
                        op,
                        Operand::Constant(Constant::F64(a_bits)),
                        Operand::Constant(Constant::F64(b_bits)),
                    ) = rval
                    {
                        let a = f64::from_bits(*a_bits);
                        let b = f64::from_bits(*b_bits);
                        let res = match op {
                            BinOp::Add => Some(Constant::F64((a + b).to_bits())),
                            BinOp::Sub => Some(Constant::F64((a - b).to_bits())),
                            BinOp::Mul => Some(Constant::F64((a * b).to_bits())),
                            BinOp::Div => Some(Constant::F64((a / b).to_bits())),
                            BinOp::Eq => Some(Constant::Bool(a == b)),
                            BinOp::Ne => Some(Constant::Bool(a != b)),
                            BinOp::Lt => Some(Constant::Bool(a < b)),
                            BinOp::Le => Some(Constant::Bool(a <= b)),
                            BinOp::Gt => Some(Constant::Bool(a > b)),
                            BinOp::Ge => Some(Constant::Bool(a >= b)),
                            _ => None,
                        };
                        if let Some(val) = res {
                            *rval = Rvalue::Use(Operand::Constant(val));
                            changed = true;
                        }
                    } else if let Rvalue::BinaryOp(
                        op,
                        Operand::Constant(Constant::Bool(a)),
                        Operand::Constant(Constant::Bool(b)),
                    ) = rval
                    {
                        let res = match op {
                            BinOp::Eq => Some(Constant::Bool(*a == *b)),
                            BinOp::Ne => Some(Constant::Bool(*a != *b)),
                            BinOp::And => Some(Constant::Bool(*a && *b)),
                            BinOp::Or => Some(Constant::Bool(*a || *b)),
                            _ => None,
                        };
                        if let Some(val) = res {
                            *rval = Rvalue::Use(Operand::Constant(val));
                            changed = true;
                        }
                    } else if let Rvalue::UnaryOp(op, Operand::Constant(c)) = rval {
                        let res = match (op, c.clone()) {
                            (UnOp::Neg, Constant::I64(a)) => Some(Constant::I64(-a)),
                            (UnOp::Neg, Constant::F64(a)) => {
                                Some(Constant::F64((-f64::from_bits(a)).to_bits()))
                            },
                            (UnOp::Not, Constant::Bool(a)) => Some(Constant::Bool(!a)),
                            (UnOp::Not, Constant::I64(a)) => Some(Constant::Bool(a == 0)),
                            _ => None,
                        };
                        if let Some(val) = res {
                            *rval = Rvalue::Use(Operand::Constant(val));
                            changed = true;
                        }
                    }
                }
            }
        }
        changed
    }
}
