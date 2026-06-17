//! Token definitions for the Ling language

/// Canonical token set for the Ling language.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // ============================================================================
    // DECLARATION KEYWORDS
    // ============================================================================
    Bind, // `bind` or `令` or `灵符`
    Do,   // `do` or `执`
    Fn,   // `fn` or `函`
    Mod,  // `mod` or `核`
    Type, // `type` or `符`

    // ============================================================================
    // CONTROL FLOW KEYWORDS
    // ============================================================================
    If,       // `if` or `若`
    Else,     // `else` or `否则`
    While,    // `while` or `循`
    For,      // `for` or `历`
    In,       // `in` or `于`
    Match,    // `match` or `配`
    Return,   // `return` or `归`
    Loop,     // `loop` or `永`
    Break,    // `break` or `止`
    Continue, // `continue` or `续`

    // ============================================================================
    // OWNERSHIP KEYWORDS
    // ============================================================================
    Own,    // `own` or `拥有`
    Lend,   // `lend` or `借`
    Share,  // `share` or `共享`
    Move,   // `move` or `移动`
    Copy,   // `copy` or `复制`
    Change, // `change` or `改变`

    // ============================================================================
    // CONCURRENCY KEYWORDS
    // ============================================================================
    Async, // `async` or `异步`
    Wait,  // `wait` or `等待`
    Spawn, // `spawn` or `生成`

    // ============================================================================
    // OTHER KEYWORDS
    // ============================================================================
    Post,   // `post` or `发布`
    Give,   // `give` or `给`
    Fit,    // `fit` or `适合`
    Form,   // `form` or `形式`
    Choose, // `choose` or `选择`
    Can,    // `can` or `能`
    Stop,   // `stop` or `停止`
    Again,  // `again` or `继续`
    Try,    // `try` or `尝试`
    Sure,   // `sure` or `确定`
    Maybe,  // `maybe` or `可能`
    Pure,   // `pure` or `纯`
    Ok,     // `ok` or `好`
    Bad,    // `bad` or `坏`
    NoneKw, // `none` or `无`
    As,     // `as` or `为`
    Where,  // `where` or `条件`

    // ============================================================================
    // TYPE KEYWORDS
    // ============================================================================
    Num,    // `num` or `数`
    Text,   // `text` or `文`
    Bool,   // `bool` or `布`
    Vec,    // `vec` or `列`
    Map,    // `map` or `图`
    Tuple,  // `tuple` or `元`
    Struct, // `struct` or `形`
    Enum,   // `enum` or `枚`
    Impl,   // `impl` or `实现`
    Trait,  // `trait` or `特`
    Use,    // `use` or `用`

    // ============================================================================
    // IDENTIFIERS & LITERALS
    // ============================================================================
    Ident(String),  // variable/function names
    Number(String), // integer or float
    Str(String),    // string literal
    Char(char),     // character literal
    BoolLit(bool),  // true/false

    // ============================================================================
    // ARITHMETIC OPERATORS
    // ============================================================================
    Plus,    // `+`
    Minus,   // `-`
    Star,    // `*`
    Slash,   // `/`
    Percent, // `%`

    // ============================================================================
    // COMPARISON OPERATORS
    // ============================================================================
    Eq,    // `=`
    EqEq,  // `==`
    NotEq, // `!=`
    Lt,    // `<`
    Gt,    // `>`
    Le,    // `<=`
    Ge,    // `>=`

    // ============================================================================
    // LOGICAL OPERATORS
    // ============================================================================
    And, // `&&` or `且`
    Or,  // `||` or `或`
    Not, // `!` or `非`

    // ============================================================================
    // SPECIAL OPERATORS
    // ============================================================================
    Arrow,      // `->`
    FatArrow,   // `=>`
    Dot,        // `.`
    DotDot,     // `..`
    DotDotDot,  // `...`
    Ampersand,  // `&`
    ColonColon, // `::`
    At,         // `@`
    Underscore, // `_`
    Question,   // `?`

    // ============================================================================
    // PUNCTUATION
    // ============================================================================
    LParen,    // `(`
    RParen,    // `)`
    LBrace,    // `{`
    RBrace,    // `}`
    LBracket,  // `[`
    RBracket,  // `]`
    Comma,     // `,`
    Colon,     // `:`
    Semicolon, // `;`

    // ============================================================================
    // SPECIAL TOKENS
    // ============================================================================
    Comment(String), // comment text
    Whitespace,      // skipped
    Error(String),   // lexer error
    Eof,             // end of file
}

impl Token {
    /// Returns true if the token is a keyword
    pub fn is_keyword(&self) -> bool {
        matches!(
            self,
            Token::Bind
                | Token::Do
                | Token::Fn
                | Token::Mod
                | Token::Type
                | Token::If
                | Token::Else
                | Token::While
                | Token::For
                | Token::In
                | Token::Match
                | Token::Return
                | Token::Loop
                | Token::Break
                | Token::Continue
                | Token::Own
                | Token::Lend
                | Token::Share
                | Token::Move
                | Token::Copy
                | Token::Change
                | Token::Async
                | Token::Wait
                | Token::Spawn
                | Token::Post
                | Token::Give
                | Token::Fit
                | Token::Form
                | Token::Choose
                | Token::Can
                | Token::Stop
                | Token::Again
                | Token::Try
                | Token::Sure
                | Token::Maybe
                | Token::Pure
                | Token::Ok
                | Token::Bad
                | Token::NoneKw
                | Token::As
                | Token::Where
        )
    }

    /// Returns true if the token is a type keyword
    pub fn is_type(&self) -> bool {
        matches!(
            self,
            Token::Num
                | Token::Text
                | Token::Bool
                | Token::Vec
                | Token::Map
                | Token::Tuple
                | Token::Struct
                | Token::Enum
                | Token::Impl
                | Token::Trait
        )
    }

    /// Returns true if the token is a literal
    pub fn is_literal(&self) -> bool {
        matches!(
            self,
            Token::Number(_) | Token::Str(_) | Token::Char(_) | Token::BoolLit(_)
        )
    }

    /// Returns true if the token is an operator
    pub fn is_operator(&self) -> bool {
        matches!(
            self,
            Token::Plus
                | Token::Minus
                | Token::Star
                | Token::Slash
                | Token::Percent
                | Token::Eq
                | Token::EqEq
                | Token::NotEq
                | Token::Lt
                | Token::Gt
                | Token::Le
                | Token::Ge
                | Token::And
                | Token::Or
                | Token::Not
                | Token::Arrow
                | Token::FatArrow
                | Token::Dot
                | Token::Ampersand
        )
    }
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Keywords
            Token::Bind => write!(f, "bind"),
            Token::Do => write!(f, "do"),
            Token::Fn => write!(f, "fn"),
            Token::Mod => write!(f, "mod"),
            Token::Type => write!(f, "type"),
            Token::If => write!(f, "if"),
            Token::Else => write!(f, "else"),
            Token::While => write!(f, "while"),
            Token::For => write!(f, "for"),
            Token::In => write!(f, "in"),
            Token::Match => write!(f, "match"),
            Token::Return => write!(f, "return"),
            Token::Loop => write!(f, "loop"),
            Token::Break => write!(f, "break"),
            Token::Continue => write!(f, "continue"),
            Token::Own => write!(f, "own"),
            Token::Lend => write!(f, "lend"),
            Token::Share => write!(f, "share"),
            Token::Move => write!(f, "move"),
            Token::Copy => write!(f, "copy"),
            Token::Change => write!(f, "change"),
            Token::Async => write!(f, "async"),
            Token::Wait => write!(f, "wait"),
            Token::Spawn => write!(f, "spawn"),
            Token::Post => write!(f, "post"),
            Token::Give => write!(f, "give"),
            Token::Fit => write!(f, "fit"),
            Token::Form => write!(f, "form"),
            Token::Choose => write!(f, "choose"),
            Token::Can => write!(f, "can"),
            Token::Stop => write!(f, "stop"),
            Token::Again => write!(f, "again"),
            Token::Try => write!(f, "try"),
            Token::Sure => write!(f, "sure"),
            Token::Maybe => write!(f, "maybe"),
            Token::Pure => write!(f, "pure"),
            Token::Ok => write!(f, "ok"),
            Token::Bad => write!(f, "bad"),
            Token::NoneKw => write!(f, "none"),
            Token::As => write!(f, "as"),
            Token::Where => write!(f, "where"),

            // Types
            Token::Num => write!(f, "num"),
            Token::Text => write!(f, "text"),
            Token::Bool => write!(f, "bool"),
            Token::Vec => write!(f, "vec"),
            Token::Map => write!(f, "map"),
            Token::Tuple => write!(f, "tuple"),
            Token::Struct => write!(f, "struct"),
            Token::Enum => write!(f, "enum"),
            Token::Impl => write!(f, "impl"),
            Token::Trait => write!(f, "trait"),
            Token::Use => write!(f, "use"),

            // Literals
            Token::Ident(s) => write!(f, "{}", s),
            Token::Number(n) => write!(f, "{}", n),
            Token::Str(s) => write!(f, "\"{}\"", s),
            Token::Char(c) => write!(f, "'{}'", c),
            Token::BoolLit(b) => write!(f, "{}", b),

            // Operators
            Token::Plus => write!(f, "+"),
            Token::Minus => write!(f, "-"),
            Token::Star => write!(f, "*"),
            Token::Slash => write!(f, "/"),
            Token::Percent => write!(f, "%"),
            Token::Eq => write!(f, "="),
            Token::EqEq => write!(f, "=="),
            Token::NotEq => write!(f, "!="),
            Token::Lt => write!(f, "<"),
            Token::Gt => write!(f, ">"),
            Token::Le => write!(f, "<="),
            Token::Ge => write!(f, ">="),
            Token::And => write!(f, "&&"),
            Token::Or => write!(f, "||"),
            Token::Not => write!(f, "!"),
            Token::Arrow => write!(f, "->"),
            Token::FatArrow => write!(f, "=>"),
            Token::Dot => write!(f, "."),
            Token::DotDot => write!(f, ".."),
            Token::DotDotDot => write!(f, "..."),
            Token::Ampersand => write!(f, "&"),
            Token::ColonColon => write!(f, "::"),
            Token::At => write!(f, "@"),
            Token::Underscore => write!(f, "_"),
            Token::Question => write!(f, "?"),

            // Punctuation
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::LBrace => write!(f, "{{"),
            Token::RBrace => write!(f, "}}"),
            Token::LBracket => write!(f, "["),
            Token::RBracket => write!(f, "]"),
            Token::Comma => write!(f, ","),
            Token::Colon => write!(f, ":"),
            Token::Semicolon => write!(f, ";"),

            // Special
            Token::Comment(c) => write!(f, "//{}", c),
            Token::Whitespace => write!(f, " "),
            Token::Error(e) => write!(f, "error({})", e),
            Token::Eof => write!(f, "EOF"),
        }
    }
}
