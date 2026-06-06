use crate::ir::*;
use crate::loop_utils;
use crate::optimizations::Transform;
use ling_ast::ast::BinOp;
use ling_ast::Span;

pub struct LoopUnroll;

impl Transform for LoopUnroll {
    fn run(&self, func: &mut MirFunction) -> bool {
        let loops = loop_utils::find_loops(func);
        for lp in loops {
            if lp.header.0 == 0 {
                continue;
            }
            if self.try_unroll(func, &lp) {
                return true;
            }
        }
        false
    }
}

impl LoopUnroll {
    fn try_unroll(&self, func: &mut MirFunction, lp: &loop_utils::Loop) -> bool {
        if lp.body.len() > 6 {
            return false;
        }

        if lp.exits.len() > 1 {
            return false;
        }

        let mut induction = None;
        for &latch_id in &lp.latches {
            let latch = &func.basic_blocks[latch_id.0];
            for stmt in &latch.statements {
                if let StatementKind::Assign(
                    local,
                    Rvalue::BinaryOp(
                        BinOp::Add,
                        Operand::Copy(src),
                        Operand::Constant(Constant::I64(1)),
                    ),
                ) = &stmt.kind
                {
                    if *src == *local {
                        if induction.is_some() {
                            return false;
                        }
                        induction = Some(*local);
                    }
                }
            }
        }
        let iv = match induction {
            Some(i) => i,
            None => return false,
        };

        let mut body_stmt_count = 0;
        for &bb_id in &lp.body {
            body_stmt_count += func.basic_blocks[bb_id.0].statements.len();
        }
        if body_stmt_count > 30 {
            return false;
        }

        for &bb_id in &lp.body {
            for stmt in &func.basic_blocks[bb_id.0].statements {
                if let StatementKind::Assign(_, Rvalue::Call { .. }) = &stmt.kind {
                    return false;
                }
            }
        }

        let header = lp.header;
        let latch_id = match lp.latches.first() {
            Some(&l) => l,
            None => return false,
        };

        let mut body_blocks: Vec<BasicBlockId> = lp
            .body
            .iter()
            .filter(|&&b| b != header && b != latch_id)
            .copied()
            .collect();
        body_blocks.sort_by_key(|b| b.0);

        if body_blocks.is_empty() && header == latch_id {
            return false;
        }

        let unroll_factor = 4;

        let latch = &mut func.basic_blocks[latch_id.0];
        for stmt in &mut latch.statements {
            if let StatementKind::Assign(
                local,
                Rvalue::BinaryOp(
                    BinOp::Add,
                    Operand::Copy(src),
                    Operand::Constant(Constant::I64(step)),
                ),
            ) = &mut stmt.kind
            {
                if *local == iv && *src == iv && *step == 1 {
                    *step = unroll_factor as i64;
                    break;
                }
            }
        }

        let mut body_stmts = Vec::new();
        for &bb_id in &body_blocks {
            for stmt in &func.basic_blocks[bb_id.0].statements {
                body_stmts.push(stmt.clone());
            }
        }

        if body_stmts.is_empty() {
            let header_bb = &func.basic_blocks[header.0];
            let stmts = &header_bb.statements;
            if stmts.len() > 1 {
                for stmt in &stmts[..stmts.len() - 1] {
                    body_stmts.push(stmt.clone());
                }
            }
        }

        if body_stmts.is_empty() {
            return false;
        }

        let latch = &mut func.basic_blocks[latch_id.0];
        let existing = std::mem::take(&mut latch.statements);
        let mut new_stmts = Vec::new();

        for _ in 0..(unroll_factor - 1) {
            new_stmts.push(Statement {
                kind: StatementKind::Assign(
                    iv,
                    Rvalue::BinaryOp(
                        BinOp::Add,
                        Operand::Copy(iv),
                        Operand::Constant(Constant::I64(1)),
                    ),
                ),
                span: Span::DUMMY,
            });
            for stmt in &body_stmts {
                new_stmts.push(stmt.clone());
            }
        }

        new_stmts.extend(existing);
        latch.statements = new_stmts;

        true
    }
}
