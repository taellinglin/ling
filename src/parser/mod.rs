// src/parser/mod.rs
pub mod ast;
pub mod grammar;

// The lalrpop-generated parser is WIP; keep a minimal stub.
pub fn parse(_source: &str) -> Result<crate::parser::ast::Program, String> {
    Ok(crate::parser::ast::Program { items: Vec::new() })
}

