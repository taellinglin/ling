// src/semantic/mod.rs
mod resolver;
mod typeck;
mod symbol;
mod scope;

use crate::parser::ast::Program;
use crate::semantic::typeck::TypeChecker;
use crate::core::{LingError, LingResult};

// Semantic pipeline is WIP; provide a stub so the crate builds.
#[derive(Clone, Debug, Default)]
pub struct SemanticAnalyzer {
    type_checker: TypeChecker,
}

impl SemanticAnalyzer {
    pub fn new() -> Self {
        Self {
            type_checker: TypeChecker::new(),
        }
    }

    pub fn analyze(&mut self, _program: &Program) -> LingResult<()> {
        Ok(())
    }
}

