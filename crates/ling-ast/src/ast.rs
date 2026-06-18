//! Ling abstract syntax tree node types.

use crate::Span;
use serde::{Deserialize, Serialize};

/// A complete source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Program {
    pub items: Vec<Item>,
    pub span: Span,
}

/// Top-level item (function, module, type alias, binding, …).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Item {
    Fn(FnDef),
    Bind(BindDef),
    TypeAlias(TypeAlias),
    Mod(ModDef),
    Import(ImportPath),
    Expr(Expr),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FnDef {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub body: Box<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub ty: Option<TypeExpr>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindDef {
    pub name: String,
    pub ty: Option<TypeExpr>,
    pub value: Box<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeAlias {
    pub name: String,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModDef {
    pub name: String,
    pub items: Vec<Item>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPath {
    pub path: Vec<String>,
    pub span: Span,
}

/// Type expression (in annotations / return types).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TypeExpr {
    Name(String),
    Generic { base: String, args: Vec<TypeExpr> },
    Tuple(Vec<TypeExpr>),
    Fn { params: Vec<TypeExpr>, ret: Box<TypeExpr> },
}

/// Expression — the core of Ling AST.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    // Literals
    Number(f64),
    Str(String),
    Bool(bool),
    Unit,
    Nil,

    // Identifier reference
    Ident(String),

    // Compound
    Block(Vec<Expr>, Span),
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    Index {
        base: Box<Expr>,
        idx: Box<Expr>,
        span: Span,
    },
    Field {
        base: Box<Expr>,
        name: String,
        span: Span,
    },

    // Binding / assignment
    Bind {
        name: String,
        ty: Option<TypeExpr>,
        value: Box<Expr>,
        span: Span,
    },
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
        span: Span,
    },

    // Control flow
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Option<Box<Expr>>,
        span: Span,
    },
    While {
        cond: Box<Expr>,
        body: Box<Expr>,
        span: Span,
    },
    For {
        var: String,
        iter: Box<Expr>,
        body: Box<Expr>,
        span: Span,
    },
    Return(Option<Box<Expr>>, Span),
    Break(Span),
    Continue(Span),

    // Operators
    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    UnOp {
        op: UnOp,
        operand: Box<Expr>,
        span: Span,
    },

    // Closures / lambdas
    Lambda {
        params: Vec<Param>,
        body: Box<Expr>,
        span: Span,
    },

    // Collections
    List(Vec<Expr>, Span),
    Tuple(Vec<Expr>, Span),
    Map(Vec<(Expr, Expr)>, Span),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Self::Number(_)
            | Self::Str(_)
            | Self::Bool(_)
            | Self::Unit
            | Self::Nil
            | Self::Ident(_) => Span::DUMMY,
            Self::Block(_, s)
            | Self::Call { span: s, .. }
            | Self::Index { span: s, .. }
            | Self::Field { span: s, .. }
            | Self::Bind { span: s, .. }
            | Self::Assign { span: s, .. }
            | Self::If { span: s, .. }
            | Self::While { span: s, .. }
            | Self::For { span: s, .. }
            | Self::Return(_, s)
            | Self::Break(s)
            | Self::Continue(s)
            | Self::BinOp { span: s, .. }
            | Self::UnOp { span: s, .. }
            | Self::Lambda { span: s, .. }
            | Self::List(_, s)
            | Self::Tuple(_, s)
            | Self::Map(_, s) => *s,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnOp {
    Neg,
    Not,
    Ref,
    Deref,
}
