/*
 * Placeholder module: src/parser/ast.rs
 */

#[derive(Debug, Clone)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub enum Item {
    Bind(String, Expr),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Number(i64),
    String(String),
}
