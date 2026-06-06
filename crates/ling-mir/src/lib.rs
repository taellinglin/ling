//! Ling MIR - Mid-level Intermediate Representation for code generation backends.
//!
//! Provides:
//! - A statement-based MIR (non-SSA, Local-indexed, BasicBlock-based)
//! - Full optimization pipeline (17 passes with fixpoint iteration)
//! - Liveness analysis
//! - Loop detection (dominator-based)
//! - MIR builder for constructing IR
//!
//! Ported and adapted from the Olive compiler's optimization infrastructure.

pub mod ir;
pub mod liveness;
pub mod loop_utils;
pub mod optimizations;
pub mod optimizer;

pub use ir::*;
pub use liveness::Liveness;
pub use loop_utils::find_loops;
pub use optimizations::Transform;
pub use optimizer::OptLevel;
pub use optimizer::Optimizer;
