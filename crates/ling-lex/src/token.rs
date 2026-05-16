/// Canonical token set for the Ling language.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Declaration
    Bind, Do, Fn, Mod, Type,
    // Control flow
    If, Else, While, For, In, Match, Return,
    // Ownership
    Own, Lend, Share, Move, Copy,
    // Concurrency
    Async, Wait,
    // Other keywords
    Post, Give, Fit, Form, Choose, Can, Change,
    Stop, Again, Try, Sure, Maybe, Pure, Spawn,
    Ok, Bad, NoneKw,
    As, Where,

    // Identifiers & literals
    Ident(String),
    Number(String),
    Str(String),
    Char(char),
    Bool(bool),

    // Operators
    Plus, Minus, Star, Slash, Percent,
    Eq, EqEq, Ne, Lt, Gt, Le, Ge,
    And, Or, Not,
    Arrow, FatArrow, Dot, DotDot,
    Ampersand, ColonColon,

    // Punctuation
    LParen, RParen, LBrace, RBrace, LBracket, RBracket,
    Comma, Colon, Semicolon,

    // Special
    Error(String),
    Eof,
}
