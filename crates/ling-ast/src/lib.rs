//! Ling Abstract Syntax Tree

pub mod ast;
pub mod span;

pub use ast::{Expr, Item, Program};
pub use span::Span;
