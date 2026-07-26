use super::Transform;
use crate::ir::*;

pub struct SinkStore;

impl Transform for SinkStore {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            let mut i = 0;
            while i + 1 < bb.statements.len() {
                if let StatementKind::Assign(l1, _) = &bb.statements[i].kind {
                    if let StatementKind::Assign(l2, rv2) = &bb.statements[i + 1].kind {
                        if l1 == l2 && !rv_uses_local(rv2, l1) {
                            bb.statements.remove(i);
                            changed = true;
                            continue;
                        }
                    }
                }
                i += 1;
            }
        }
        changed
    }
}

fn rv_uses_local(rv: &Rvalue, local: &Local) -> bool {
    match rv {
        Rvalue::Use(op) | Rvalue::UnaryOp(_, op) => op_uses_local(op, local),
        Rvalue::BinaryOp(_, l, r) => op_uses_local(l, local) || op_uses_local(r, local),
        Rvalue::Call { func, args } => {
            op_uses_local(func, local) || args.iter().any(|a| op_uses_local(a, local))
        },
        Rvalue::Aggregate(_, ops) => ops.iter().any(|o| op_uses_local(o, local)),
        Rvalue::GetAttr(o, _) => op_uses_local(o, local),
        Rvalue::GetIndex(o, i) => op_uses_local(o, local) || op_uses_local(i, local),
        Rvalue::Ref(l) | Rvalue::MutRef(l) => l == local,
        Rvalue::VectorSplat(op, _) => op_uses_local(op, local),
        Rvalue::VectorLoad(obj, idx, _) => op_uses_local(obj, local) || op_uses_local(idx, local),
        Rvalue::VectorFMA(a, b, c) => {
            op_uses_local(a, local) || op_uses_local(b, local) || op_uses_local(c, local)
        },
        Rvalue::InlineAsm(_) => false,
    }
}

fn op_uses_local(op: &Operand, local: &Local) -> bool {
    match op {
        Operand::Copy(l) | Operand::Move(l) => l == local,
        Operand::Constant(_) => false,
    }
}
