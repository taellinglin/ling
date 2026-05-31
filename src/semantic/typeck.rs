#[derive(Clone, Debug, Default)]
pub struct TypeChecker;

impl TypeChecker {
    pub fn new() -> Self {
        Self
    }

    #[allow(dead_code)]
    pub fn check(&self, _program: &crate::parser::ast::Program) -> Result<(), crate::core::LingError> {
        Ok(())
    }
}

