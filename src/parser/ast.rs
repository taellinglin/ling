// src/parser/ast.rs

#[derive(Debug, Clone)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub enum Item {
    Bind(String, Expr),
    Fn(FnDef),
    Mod(String, Vec<Item>),
    TypeAlias(String, String), // type Name = RawType
    /// `use "path/to/module"` or `use "path" as ns`
    Use { path: String, alias: Option<String> },
}

#[derive(Debug, Clone)]
pub struct FnDef {
    pub name: String,
    pub is_async: bool,
    pub params: Vec<String>,   // just names; types are parsed but ignored
    pub body: Vec<Stmt>,
}

// ─── Expressions ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Expr {
    Str(String),
    Number(f64),
    Bool(bool),
    Unit,
    Ident(String),
    /// `do { stmts }` or anonymous block `{ stmts }`
    Do(Vec<Stmt>),
    /// `if cond { then } (else if cond { elif })* (else { else_body })?`
    If {
        cond: Box<Expr>,
        then: Vec<Stmt>,
        elseifs: Vec<(Expr, Vec<Stmt>)>,
        else_body: Option<Vec<Stmt>>,
    },
    /// `for name in iterable { body }`
    For {
        var: String,
        iter: Box<Expr>,
        body: Vec<Stmt>,
    },
    /// `while cond { body }` / `ขณะที่ cond { body }`
    While {
        cond: Box<Expr>,
        body: Vec<Stmt>,
    },
    /// `match expr { arms }`
    Match(Box<Expr>, Vec<MatchArm>),
    /// Normal call: `expr(args)`
    Call(Box<Expr>, Vec<Expr>),
    /// Method call: `receiver.method(args)`
    MethodCall {
        receiver: Box<Expr>,
        method: String,
        args: Vec<Expr>,
    },
    /// Path like `Mod::fn` or just chained idents; resolved at runtime
    Path(Vec<String>),
    /// `lo..hi`
    Range(Box<Expr>, Box<Expr>),
    /// `&expr`
    Ref(Box<Expr>),
    /// `await expr`
    Await(Box<Expr>),
    /// Binary operation
    BinOp(BinOp, Box<Expr>, Box<Expr>),
    /// Array/Vec literal `[a, b, c]`
    Array(Vec<Expr>),
    /// `expr[idx]`
    Index(Box<Expr>, Box<Expr>),
    /// Closure `|| expr` or `|args| expr`
    Closure(Vec<String>, Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Rem,
    Eq, Ne, Lt, Gt, Le, Ge,
    And, Or,
}

// ─── Statements ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Stmt {
    Bind(String, Expr),
    Expr(Expr),
    Return(Expr),
}

// ─── Match arms ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard,
    Str(String),
    Number(f64),
    Bool(bool),
    Ident(String),
    /// Ok(inner), Bad(inner), 好(inner), 坏(inner)
    Constructor(String, Option<Box<Pattern>>),
}
