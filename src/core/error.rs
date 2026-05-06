// Simplified error type so the crate builds without external WIP deps.

#[derive(Debug, Clone)]
pub enum LingError {
    Lex(String),
    Parse(String),
    Type(String),
    Borrow(String),
    Codegen(String),
}

impl std::fmt::Display for LingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lex(s) => write!(f, "Lexical error: {s}"),
            Self::Parse(s) => write!(f, "Parse error: {s}"),
            Self::Type(s) => write!(f, "Type error: {s}"),
            Self::Borrow(s) => write!(f, "Borrow error: {s}"),
            Self::Codegen(s) => write!(f, "Codegen error: {s}"),
        }
    }
}

impl std::error::Error for LingError {}

