// src/lexer/token.rs
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Declaration keywords
    Bind,
    Do,
    Fn,
    Mod,
    Type,
    // Module imports
    Use,

    // Control flow
    If,
    Else,
    While,
    For,
    In,
    Match,
    Return,

    // Ownership / borrow semantics
    Own,
    Lend,
    Share,
    Move,
    Copy,

    // Concurrency
    Async,
    Wait,

    // Assembly
    Asm,

    // Other keywords
    Post,
    Give,
    Fit,
    Form,
    Choose,
    Can,
    Change,
    Stop,
    Again,
    Try,
    Sure,
    Maybe,
    Pure,
    Spawn,

    // Result / Option helpers
    Ok,
    Bad,
    None,

    // Type / trait helpers
    As,
    Where,

    // Identifiers & literals
    Ident(String),
    Number(String),
    String(String),
    Char(char),
    Bool(bool),

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    EqEq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    Not,
    Arrow,
    FatArrow,
    Dot,
    DotDot,
    Ampersand,

    // Path / namespace
    ColonColon,

    // Punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Semicolon,

    // Special
    Whitespace,
    Comment(std::string::String),
    Error(std::string::String),
    Eof,
}
