//! Lexer for the Ling language using Logos

use logos::Logos;
use crate::token::Token;

/// Internal logos-derived token.
/// Supports multiple human languages including Chinese, Japanese, Korean, Russian, Thai, Hindi, Arabic, Spanish, French, German.
#[allow(clippy::upper_case_acronyms)]
#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\r\n\f]+")]
#[logos(skip r"//[^\n]*")]
#[logos(skip r"#[^\n]*")]
enum RawToken {
    // ──────────────────────────────────────────────────────────────────────────
    // DECLARATION KEYWORDS
    // ──────────────────────────────────────────────────────────────────────────
    #[token("bind")]  #[token("令")]  #[token("灵符")]
    #[token("ผูก")]   #[token("बाँधो")] #[token("ربط")]
    #[token("enlazar")] #[token("lier")] #[token("binden")] Bind,
    
    #[token("do")]    #[token("执")]
    #[token("ทำ")]    #[token("करो")]  #[token("افعل")]
    #[token("hacer")] #[token("faire")] #[token("machen")] Do,
    
    #[token("fn")]    #[token("函")]  Fn,
    #[token("mod")]   #[token("核")]  Mod,
    #[token("type")]  #[token("符")]  Type,
    
    // ──────────────────────────────────────────────────────────────────────────
    // CONTROL FLOW KEYWORDS
    // ──────────────────────────────────────────────────────────────────────────
    #[token("if")]    #[token("若")]   #[token("ถ้า")]
    #[token("अगर")]   #[token("إذا")] #[token("si")]
    #[token("wenn")]  If,
    
    #[token("else")]  #[token("否则")] #[token("มิฉะนั้น")]
    #[token("नहींतो")] #[token("وإلا")] #[token("sino")]
    #[token("sinon")] #[token("sonst")] Else,
    
    #[token("while")] #[token("循")]   #[token("ขณะที่")]
    #[token("जबकि")]  #[token("بينما")] #[token("mientras")]
    #[token("tantque")] #[token("solange")] While,
    
    #[token("for")]   #[token("历")]   #[token("สำหรับ")]
    #[token("para")]  #[token("لأجل")] For,
    
    #[token("in")]    #[token("于")]   #[token("ใน")]
    #[token("في")]    In,
    
    #[token("match")] #[token("配")]   #[token("จับคู่")] Match,
    
    #[token("return")]#[token("归")]   #[token("คืน")]
    #[token("वापस")]  #[token("أعد")]  #[token("retornar")]
    #[token("retourner")] #[token("zurück")] Return,
    
    #[token("loop")]  #[token("永")]   Loop,
    #[token("break")] #[token("止")]   Break,
    #[token("continue")] #[token("续")] Continue,
    
    // ──────────────────────────────────────────────────────────────────────────
    // OWNERSHIP KEYWORDS
    // ──────────────────────────────────────────────────────────────────────────
    #[token("own")]   #[token("拥有")] Own,
    #[token("lend")]  #[token("借")]   Lend,
    #[token("share")] #[token("共享")] Share,
    #[token("move")]  #[token("移动")] Move,
    #[token("copy")]  #[token("复制")] Copy,
    #[token("change")] #[token("改变")] Change,
    
    // ──────────────────────────────────────────────────────────────────────────
    // CONCURRENCY KEYWORDS
    // ──────────────────────────────────────────────────────────────────────────
    #[token("async")] #[token("异步")] Async,
    #[token("wait")]  #[token("等待")] Wait,
    #[token("spawn")] #[token("生成")] Spawn,
    
    // ──────────────────────────────────────────────────────────────────────────
    // OTHER KEYWORDS
    // ──────────────────────────────────────────────────────────────────────────
    #[token("post")]   #[token("发布")] Post,
    #[token("give")]   #[token("给")]   Give,
    #[token("fit")]    #[token("适合")] Fit,
    #[token("form")]   #[token("形式")] Form,
    #[token("choose")] #[token("选择")] Choose,
    #[token("can")]    #[token("能")]   Can,
    #[token("stop")]   #[token("停止")] Stop,
    #[token("again")]  #[token("继续")] Again,
    #[token("try")]    #[token("尝试")] Try,
    #[token("sure")]   #[token("确定")] Sure,
    #[token("maybe")]  #[token("可能")] Maybe,
    #[token("pure")]   #[token("纯")]   Pure,
    #[token("ok")]     #[token("好")]   Ok,
    #[token("bad")]    #[token("坏")]   Bad,
    #[token("none")]   #[token("无")]   NoneKw,
    #[token("as")]     #[token("为")]   As,
    #[token("where")]  #[token("条件")] Where,
    
    // ──────────────────────────────────────────────────────────────────────────
    // TYPE KEYWORDS
    // ──────────────────────────────────────────────────────────────────────────
    #[token("num")]    #[token("数")]   Num,
    #[token("text")]   #[token("文")]   Text,
    #[token("bool")]   #[token("布")]   Bool,
    #[token("vec")]    #[token("列")]   Vec,
    #[token("map")]    #[token("图")]   Map,
    #[token("tuple")]  #[token("元")]   Tuple,
    #[token("struct")] #[token("形")]   Struct,
    #[token("enum")]   #[token("枚")]   Enum,
    #[token("impl")]   #[token("实现")] Impl,
    #[token("trait")]  #[token("特")]   Trait,
    #[token("use")]    #[token("用")]   Use,
    
    // ──────────────────────────────────────────────────────────────────────────
    // BOOLEAN LITERALS
    // ──────────────────────────────────────────────────────────────────────────
    #[token("true",      |_| true)]
    #[token("false",     |_| false)]
    #[token("真",        |_| true)]
    #[token("假",        |_| false)]
    #[token("จริง",      |_| true)]
    #[token("เท็จ",      |_| false)]
    #[token("सत्य",      |_| true)]
    #[token("असत्य",     |_| false)]
    #[token("صحيح",      |_| true)]
    #[token("خطأ",       |_| false)]
    Bool(bool),
    
    // ──────────────────────────────────────────────────────────────────────────
    // STRING LITERALS
    // ──────────────────────────────────────────────────────────────────────────
    #[regex(r#""([^"\\]|\\.)*""#, |lex| {
        let s = lex.slice();
        s[1..s.len()-1]
            .replace("\\n", "\n")
            .replace("\\t", "\t")
            .replace("\\r", "\r")
            .replace("\\\\", "\\")
            .replace("\\\"", "\"")
    })]
    Str(String),
    
    // ──────────────────────────────────────────────────────────────────────────
    // NUMBER LITERALS
    // ──────────────────────────────────────────────────────────────────────────
    #[regex(r"[0-9]+(\.[0-9]+)?", |lex| lex.slice().to_string())]
    Number(String),
    
    // ──────────────────────────────────────────────────────────────────────────
    // CHARACTER LITERALS
    // ──────────────────────────────────────────────────────────────────────────
    #[regex(r"'([^'\\]|\\.)'", |lex| {
        let s = lex.slice();
        let inner = &s[1..s.len()-1];
        match inner {
            "\\n" => '\n',
            "\\t" => '\t',
            "\\\\" => '\\',
            "\\'" => '\'',
            o => o.chars().next().unwrap_or('\0')
        }
    })]
    Char(char),
    
    // ──────────────────────────────────────────────────────────────────────────
    // IDENTIFIERS (ASCII + Unicode)
    // ──────────────────────────────────────────────────────────────────────────
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice().to_string())]
    #[regex(r"[\p{L}_][\p{L}\p{N}_]*", |lex| lex.slice().to_string())]
    Ident(String),
    
    // ──────────────────────────────────────────────────────────────────────────
    // MULTI-CHARACTER OPERATORS
    // ──────────────────────────────────────────────────────────────────────────
    #[token("...")] DotDotDot,
    #[token("..")]  DotDot,
    #[token("::")]  ColonColon,
    #[token("==")]  EqEq,
    #[token("!=")]  NotEq,
    #[token("<=")]  Le,
    #[token(">=")]  Ge,
    #[token("->")]  Arrow,
    #[token("=>")]  FatArrow,
    #[token("&&")]  And,
    #[token("||")]  Or,
    
    // ──────────────────────────────────────────────────────────────────────────
    // SINGLE-CHARACTER OPERATORS
    // ──────────────────────────────────────────────────────────────────────────
    #[token("+")] Plus,
    #[token("-")] Minus,
    #[token("*")] Star,
    #[token("/")] Slash,
    #[token("%")] Percent,
    #[token("<")] Lt,
    #[token(">")] Gt,
    #[token("=")] Eq,
    #[token("!")] Not,
    #[token(".")] Dot,
    #[token("&")] Ampersand,
    #[token("?")] Question,
    #[token("@")] At,
    #[token("_")] Underscore,
    
    // ──────────────────────────────────────────────────────────────────────────
    // PUNCTUATION
    // ──────────────────────────────────────────────────────────────────────────
    #[token("(")] LParen,
    #[token(")")] RParen,
    #[token("{")] LBrace,
    #[token("}")] RBrace,
    #[token("[")] LBracket,
    #[token("]")] RBracket,
    #[token(",")] Comma,
    #[token(":")] Colon,
    #[token(";")] Semicolon,
}

impl From<RawToken> for Token {
    fn from(r: RawToken) -> Token {
        match r {
            // Declaration
            RawToken::Bind => Token::Bind,
            RawToken::Do => Token::Do,
            RawToken::Fn => Token::Fn,
            RawToken::Mod => Token::Mod,
            RawToken::Type => Token::Type,
            
            // Control flow
            RawToken::If => Token::If,
            RawToken::Else => Token::Else,
            RawToken::While => Token::While,
            RawToken::For => Token::For,
            RawToken::In => Token::In,
            RawToken::Match => Token::Match,
            RawToken::Return => Token::Return,
            RawToken::Loop => Token::Loop,
            RawToken::Break => Token::Break,
            RawToken::Continue => Token::Continue,
            
            // Ownership
            RawToken::Own => Token::Own,
            RawToken::Lend => Token::Lend,
            RawToken::Share => Token::Share,
            RawToken::Move => Token::Move,
            RawToken::Copy => Token::Copy,
            RawToken::Change => Token::Change,
            
            // Concurrency
            RawToken::Async => Token::Async,
            RawToken::Wait => Token::Wait,
            RawToken::Spawn => Token::Spawn,
            
            // Other keywords
            RawToken::Post => Token::Post,
            RawToken::Give => Token::Give,
            RawToken::Fit => Token::Fit,
            RawToken::Form => Token::Form,
            RawToken::Choose => Token::Choose,
            RawToken::Can => Token::Can,
            RawToken::Stop => Token::Stop,
            RawToken::Again => Token::Again,
            RawToken::Try => Token::Try,
            RawToken::Sure => Token::Sure,
            RawToken::Maybe => Token::Maybe,
            RawToken::Pure => Token::Pure,
            RawToken::Ok => Token::Ok,
            RawToken::Bad => Token::Bad,
            RawToken::NoneKw => Token::NoneKw,
            RawToken::As => Token::As,
            RawToken::Where => Token::Where,
            
            // Types
            RawToken::Num => Token::Num,
            RawToken::Text => Token::Text,
            RawToken::Bool => Token::Bool,
            RawToken::Vec => Token::Vec,
            RawToken::Map => Token::Map,
            RawToken::Tuple => Token::Tuple,
            RawToken::Struct => Token::Struct,
            RawToken::Enum => Token::Enum,
            RawToken::Impl => Token::Impl,
            RawToken::Trait => Token::Trait,
            RawToken::Use => Token::Use,
            
            // Literals
            RawToken::Bool(b) => Token::BoolLit(b),
            RawToken::Str(s) => Token::Str(s),
            RawToken::Number(n) => Token::Number(n),
            RawToken::Char(c) => Token::Char(c),
            RawToken::Ident(s) => Token::Ident(s),
            
            // Operators
            RawToken::Plus => Token::Plus,
            RawToken::Minus => Token::Minus,
            RawToken::Star => Token::Star,
            RawToken::Slash => Token::Slash,
            RawToken::Percent => Token::Percent,
            RawToken::Lt => Token::Lt,
            RawToken::Gt => Token::Gt,
            RawToken::Eq => Token::Eq,
            RawToken::EqEq => Token::EqEq,
            RawToken::NotEq => Token::NotEq,
            RawToken::Le => Token::Le,
            RawToken::Ge => Token::Ge,
            RawToken::And => Token::And,
            RawToken::Or => Token::Or,
            RawToken::Not => Token::Not,
            RawToken::Arrow => Token::Arrow,
            RawToken::FatArrow => Token::FatArrow,
            RawToken::Dot => Token::Dot,
            RawToken::DotDot => Token::DotDot,
            RawToken::DotDotDot => Token::DotDotDot,
            RawToken::Ampersand => Token::Ampersand,
            RawToken::ColonColon => Token::ColonColon,
            RawToken::At => Token::At,
            RawToken::Underscore => Token::Underscore,
            RawToken::Question => Token::Question,
            
            // Punctuation
            RawToken::LParen => Token::LParen,
            RawToken::RParen => Token::RParen,
            RawToken::LBrace => Token::LBrace,
            RawToken::RBrace => Token::RBrace,
            RawToken::LBracket => Token::LBracket,
            RawToken::RBracket => Token::RBracket,
            RawToken::Comma => Token::Comma,
            RawToken::Colon => Token::Colon,
            RawToken::Semicolon => Token::Semicolon,
        }
    }
}

/// Public lexer that produces Tokens
pub struct Lexer {
    tokens: Vec<Token>,
    pos: usize,
}

impl Lexer {
    /// Create a new lexer from source code
    pub fn new(source: &str) -> Self {
        let mut raw_lex = RawToken::lexer(source);
        let mut tokens = Vec::new();
        
        while let Some(result) = raw_lex.next() {
            match result {
                Ok(raw) => tokens.push(Token::from(raw)),
                Err(_) => tokens.push(Token::Error(raw_lex.slice().to_string())),
            }
        }
        tokens.push(Token::Eof);
        
        Self { tokens, pos: 0 }
    }
    
    /// Get all tokens as a vector
    pub fn into_tokens(self) -> Vec<Token> {
        self.tokens
    }
    
    /// Peek at the current token without consuming it
    pub fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }
    
    /// Get the next token
    pub fn next_token(&mut self) -> Option<Token> {
        if self.pos >= self.tokens.len() {
            return None;
        }
        let tok = self.tokens[self.pos].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        if tok == Token::Eof {
            None
        } else {
            Some(tok)
        }
    }
    
    /// Get the current position
    pub fn pos(&self) -> usize {
        self.pos
    }
}

impl Iterator for Lexer {
    type Item = Token;
    
    fn next(&mut self) -> Option<Token> {
        self.next_token()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_basic_tokens() {
        let mut lexer = Lexer::new("bind x = 42");
        assert_eq!(lexer.next_token(), Some(Token::Bind));
        assert_eq!(lexer.next_token(), Some(Token::Ident("x".to_string())));
        assert_eq!(lexer.next_token(), Some(Token::Eq));
        assert_eq!(lexer.next_token(), Some(Token::Number("42".to_string())));
    }
    
    #[test]
    fn test_operators() {
        let mut lexer = Lexer::new("+ - * / %");
        assert_eq!(lexer.next_token(), Some(Token::Plus));
        assert_eq!(lexer.next_token(), Some(Token::Minus));
        assert_eq!(lexer.next_token(), Some(Token::Star));
        assert_eq!(lexer.next_token(), Some(Token::Slash));
        assert_eq!(lexer.next_token(), Some(Token::Percent));
    }
    
    #[test]
    fn test_comparison_operators() {
        let mut lexer = Lexer::new("== != < <= > >=");
        assert_eq!(lexer.next_token(), Some(Token::EqEq));
        assert_eq!(lexer.next_token(), Some(Token::NotEq));
        assert_eq!(lexer.next_token(), Some(Token::Lt));
        assert_eq!(lexer.next_token(), Some(Token::Le));
        assert_eq!(lexer.next_token(), Some(Token::Gt));
        assert_eq!(lexer.next_token(), Some(Token::Ge));
    }
    
    #[test]
    fn test_arrow_operators() {
        let mut lexer = Lexer::new("-> =>");
        assert_eq!(lexer.next_token(), Some(Token::Arrow));
        assert_eq!(lexer.next_token(), Some(Token::FatArrow));
    }
    
    #[test]
    fn test_dot_operators() {
        let mut lexer = Lexer::new(". .. ...");
        assert_eq!(lexer.next_token(), Some(Token::Dot));
        assert_eq!(lexer.next_token(), Some(Token::DotDot));
        assert_eq!(lexer.next_token(), Some(Token::DotDotDot));
    }
    
    #[test]
    fn test_string_literal() {
        let mut lexer = Lexer::new("\"hello world\"");
        assert_eq!(lexer.next_token(), Some(Token::Str("hello world".to_string())));
    }
    
    #[test]
    fn test_escaped_string() {
        let mut lexer = Lexer::new("\"hello\\nworld\"");
        assert_eq!(lexer.next_token(), Some(Token::Str("hello\nworld".to_string())));
    }
    
    #[test]
    fn test_number_literal() {
        let mut lexer = Lexer::new("42 3.14");
        assert_eq!(lexer.next_token(), Some(Token::Number("42".to_string())));
        assert_eq!(lexer.next_token(), Some(Token::Number("3.14".to_string())));
    }
    
    #[test]
    fn test_chinese_keywords() {
        let mut lexer = Lexer::new("令 x = 42");
        assert_eq!(lexer.next_token(), Some(Token::Bind));
        assert_eq!(lexer.next_token(), Some(Token::Ident("x".to_string())));
        assert_eq!(lexer.next_token(), Some(Token::Eq));
        assert_eq!(lexer.next_token(), Some(Token::Number("42".to_string())));
    }
    
    #[test]
    fn test_thai_keywords() {
        let mut lexer = Lexer::new("ผูก x = 42");
        assert_eq!(lexer.next_token(), Some(Token::Bind));
    }
}