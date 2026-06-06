use super::Transform;
use crate::ir::*;

pub struct RedundantBranch;

impl Transform for RedundantBranch {
    fn run(&self, func: &mut MirFunction) -> bool {
        let mut changed = false;
        for bb in &mut func.basic_blocks {
            if let Some(term) = &mut bb.terminator {
                if let TerminatorKind::SwitchInt { targets, otherwise, .. } = &term.kind {
                    if targets.iter().all(|(_, t)| *t == *otherwise) {
                        let target = *otherwise;
                        term.kind = TerminatorKind::Goto { target };
                        changed = true;
                    }
                }
            }
        }
        changed
    }
}
