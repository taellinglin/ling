#[derive(Debug, Clone)]
pub enum LingError {
    Lex(String),
    Parse(String),
    Type(String),
    Borrow(String),
    Codegen(String),
    Mir(String),
    Io(String),
}

impl std::fmt::Display for LingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lex(s) => write!(f, "Lexical error: {s}"),
            Self::Parse(s) => write!(f, "Parse error: {s}"),
            Self::Type(s) => write!(f, "Type error: {s}"),
            Self::Borrow(s) => write!(f, "Borrow error: {s}"),
            Self::Codegen(s) => write!(f, "Codegen error: {s}"),
            Self::Mir(s) => write!(f, "MIR error: {s}"),
            Self::Io(s) => write!(f, "I/O error: {s}"),
        }
    }
}

impl std::error::Error for LingError {}

impl From<String> for LingError {
    fn from(s: String) -> Self {
        LingError::Mir(s)
    }
}

impl From<std::io::Error> for LingError {
    fn from(e: std::io::Error) -> Self {
        LingError::Io(e.to_string())
    }
}
