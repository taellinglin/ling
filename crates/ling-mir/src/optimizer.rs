use crate::ir::*;
use crate::optimizations::{
    algebraic::AlgebraicSimplification, const_fold::ConstantFolding,
    const_prop::ConstantPropagation, copy_prop::CopyPropagation,
    cse::CommonSubexpressionElimination, dce::DeadCodeElimination, gvn::GlobalValueNumbering,
    inliner::Inliner, licm::Licm, loop_unroll::LoopUnroll, move_elision::MoveElision,
    peephole::PeepholeOptimize, redundant_branch::RedundantBranch, scalarize::ScalarizeStructs,
    simplify_cfg::SimplifyCfg, sink_store::SinkStore, strength_reduction::StrengthReduction,
    tail_call::TailCallOpt, vectorize::LoopVectorizer, Transform,
};
use rustc_hash::FxHashMap as HashMap;

#[derive(Clone, Copy, Debug, Default)]
pub enum OptLevel {
    #[default]
    None,
    O1,
    O2,
    O3,
}

pub struct Optimizer {
    scalar_passes: Vec<Box<dyn Transform>>,
    late_passes: Vec<Box<dyn Transform>>,
    inliner: Option<Inliner>,
    opt_level: OptLevel,
}

impl Optimizer {
    pub fn new(opt_level: OptLevel) -> Self {
        let scalar_passes: Vec<Box<dyn Transform>> = vec![
            Box::new(CopyPropagation),
            Box::new(ConstantPropagation),
            Box::new(ConstantFolding),
            Box::new(AlgebraicSimplification),
            Box::new(StrengthReduction),
            Box::new(CommonSubexpressionElimination),
            Box::new(PeepholeOptimize),
            Box::new(GlobalValueNumbering),
            Box::new(SimplifyCfg),
            Box::new(DeadCodeElimination),
            Box::new(MoveElision),
            Box::new(SinkStore),
            Box::new(RedundantBranch),
        ];

        let late_passes: Vec<Box<dyn Transform>> = match opt_level {
            OptLevel::None | OptLevel::O1 => vec![],
            OptLevel::O2 => vec![
                Box::new(TailCallOpt),
                Box::new(Licm),
                Box::new(SimplifyCfg),
                Box::new(DeadCodeElimination),
                Box::new(CopyPropagation),
                Box::new(ConstantPropagation),
                Box::new(ConstantFolding),
                Box::new(StrengthReduction),
                Box::new(PeepholeOptimize),
                Box::new(DeadCodeElimination),
            ],
            OptLevel::O3 => vec![
                Box::new(TailCallOpt),
                Box::new(Licm),
                Box::new(LoopVectorizer),
                Box::new(LoopUnroll),
                Box::new(SimplifyCfg),
                Box::new(DeadCodeElimination),
                Box::new(CopyPropagation),
                Box::new(ConstantPropagation),
                Box::new(ConstantFolding),
                Box::new(StrengthReduction),
                Box::new(PeepholeOptimize),
                Box::new(DeadCodeElimination),
            ],
        };

        let inliner = match opt_level {
            OptLevel::None | OptLevel::O1 => None,
            OptLevel::O2 | OptLevel::O3 => Some(Inliner::new()),
        };

        Self { scalar_passes, late_passes, inliner, opt_level }
    }

    pub fn run(&self, functions: &mut [MirFunction]) {
        let fn_map: HashMap<String, MirFunction> = functions
            .iter()
            .map(|f| (f.name.clone(), f.clone()))
            .collect();

        for func in functions.iter_mut() {
            let is_trivial = func.basic_blocks.len() <= 2
                && func.basic_blocks.iter().all(|bb| {
                    bb.statements.iter().all(|s| {
                        matches!(
                            &s.kind,
                            StatementKind::Assign(_, Rvalue::Call { .. })
                                | StatementKind::Assign(_, Rvalue::Use(_))
                                | StatementKind::StorageLive(_)
                                | StatementKind::StorageDead(_)
                                | StatementKind::Drop(_)
                        )
                    })
                });

            if is_trivial && func.name == "__main__" {
                SimplifyCfg.run(func);
                DeadCodeElimination.run(func);
                continue;
            }

            if let Some(ref inliner) = self.inliner {
                inliner.inline_function(func, &fn_map, 12);
            }

            let max_iter = match self.opt_level {
                OptLevel::O3 => 15,
                _ => 10,
            };
            let mut changed = true;
            let mut iterations = 0;
            while changed {
                iterations += 1;
                if iterations > max_iter {
                    break;
                }
                changed = false;
                for pass in &self.scalar_passes {
                    if pass.run(func) {
                        changed = true;
                    }
                }
            }

            let scalarize_did_work = ScalarizeStructs.run(func);

            if !scalarize_did_work || matches!(self.opt_level, OptLevel::O2 | OptLevel::O3) {
                for pass in &self.late_passes {
                    pass.run(func);
                }
            }
        }
    }
}

impl Default for Optimizer {
    fn default() -> Self {
        Self::new(OptLevel::O2)
    }
}
