use super::Transform;
use crate::ir::*;
use ling_ast::ast::BinOp;
use std::collections::HashMap;

pub struct AlgebraicSimplification;

impl Transform for AlgebraicSimplification {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut assign_counts: HashMap<Local, usize> = HashMap::new();
        let mut def_map: HashMap<Local, Rvalue> = HashMap::new();

        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(dest, rval) = &stmt.kind {
                    *assign_counts.entry(*dest).or_insert(0) += 1;
                    def_map.insert(*dest, rval.clone());
                }
            }
        }

        let single_def: HashMap<Local, Rvalue> = def_map
            .into_iter()
            .filter(|(l, _)| assign_counts.get(l) == Some(&1))
            .collect();

        let mut changed = false;

        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(_, rval) = &mut stmt.kind {
                    if let Rvalue::BinaryOp(
                        BinOp::Div,
                        Operand::Copy(src),
                        Operand::Constant(Constant::I64(b)),
                    ) = rval
                    {
                        if *b == 0 {
                            continue;
                        }
                        if let Some(Rvalue::BinaryOp(
                            BinOp::Mul,
                            mul_lhs,
                            Operand::Constant(Constant::I64(a)),
                        )) = single_def.get(src)
                        {
                            if *a % *b == 0 {
                                let factor = *a / *b;
                                *rval = Rvalue::BinaryOp(
                                    BinOp::Mul,
                                    mul_lhs.clone(),
                                    Operand::Constant(Constant::I64(factor)),
                                );
                                changed = true;
                            }
                        } else if let Some(Rvalue::BinaryOp(
                            BinOp::Mul,
                            Operand::Constant(Constant::I64(a)),
                            mul_rhs,
                        )) = single_def.get(src)
                        {
                            if *a % *b == 0 {
                                let factor = *a / *b;
                                *rval = Rvalue::BinaryOp(
                                    BinOp::Mul,
                                    mul_rhs.clone(),
                                    Operand::Constant(Constant::I64(factor)),
                                );
                                changed = true;
                            }
                        }
                    }
                }
            }
        }

        changed
    }
}
