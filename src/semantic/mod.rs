mod resolver;
mod scope;
mod symbol;
mod typeck;
mod builtins;

use crate::core::LingResult;
use crate::parser::ast::Program;
use crate::semantic::typeck::TypeChecker;

#[derive(Clone, Debug, Default)]
pub struct SemanticAnalyzer {
    type_checker: TypeChecker,
}

impl SemanticAnalyzer {
    pub fn new() -> Self {
        Self { type_checker: TypeChecker::new() }
    }

    pub fn analyze(&mut self, program: &Program) -> LingResult<()> {
        self.type_checker.check(program)
    }
}
