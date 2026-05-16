use logos::Logos;
use crate::token::Token;

/// Internal logos-derived token.
/// Chinese/Japanese/Korean/Russian keywords are listed as explicit `#[token]`
/// alternatives so normalization never corrupts variable names or string contents.
#[allow(clippy::upper_case_acronyms)]
#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\r\n\f]+")]
#[logos(skip r"//[^\n]*")]
#[logos(skip r"#[^\n]*")]
enum RawToken {
    // ── Declaration ──────────────────────────────────────────────────────────
    #[token("bind")]  #[token("令")]  #[token("灵符")]
    #[token("ผูก")]   #[token("बाँधो")] #[token("ربط")]
    #[token("enlazar")] #[token("lier")] #[token("binden")] #[token("ligar")] Bind,
    #[token("do")]    #[token("执")]
    #[token("ทำ")]    #[token("करो")]  #[token("افعل")]
    #[token("hacer")] #[token("faire")] #[token("machen")] #[token("fazer")] Do,
    #[token("fn")]    #[token("函")]  #[token("ฟังก์ชัน")] Fn,
    #[token("mod")]   #[token("核")]  #[token("โมดูล")]    Mod,
    #[token("type")]  #[token("符")]  Type,
    // ── Control flow ─────────────────────────────────────────────────────────
    #[token("if")]    #[token("若")]   #[token("ถ้า")]
    #[token("अगर")]   #[token("إذا")] #[token("si")]
    #[token("wenn")]  If,
    #[token("else")]  #[token("否则")] #[token("มิฉะนั้น")]
    #[token("नहींतो")] #[token("وإلا")] #[token("sino")]
    #[token("sinon")] #[token("sonst")] Else,
    #[token("while")] #[token("循")]   #[token("ขณะที่")]
    #[token("जबकि")]  #[token("بينما")] #[token("mientras")]
    #[token("tantque")] #[token("solange")] #[token("enquanto")] While,
    #[token("for")]   #[token("历")]   #[token("สำหรับ")]
    #[token("para")]  #[token("لأجل")]  For,
    #[token("in")]    #[token("于")]   #[token("ใน")]
    #[token("في")]    In,
    #[token("match")] #[token("配")]   #[token("จับคู่")]  Match,
    #[token("return")]#[token("归")]   #[token("คืน")]
    #[token("वापस")]  #[token("أعد")]  #[token("retornar")]
    #[token("retourner")] #[token("zurück")] Return,
    // ── Ownership ────────────────────────────────────────────────────────────
    #[token("own")]   #[token("拥有")] Own,
    #[token("lend")]  #[token("借")]   Lend,
    #[token("share")] #[token("共享")] Share,
    #[token("move")]  #[token("移动")] Move,
    #[token("copy")]  #[token("复制")] Copy,
    // ── Concurrency ──────────────────────────────────────────────────────────
    #[token("async")] #[token("异步")] #[token("ไม่พร้อมกัน")] Async,
    #[token("wait")]  #[token("等待")] #[token("รอ")]           Wait,
    // ── Misc keywords ────────────────────────────────────────────────────────
    #[token("post")]   #[token("发布")] Post,
    #[token("give")]   #[token("给")]   Give,
    #[token("fit")]    #[token("适合")] Fit,
    #[token("form")]   #[token("形式")] Form,
    #[token("choose")] #[token("选择")] Choose,
    #[token("can")]    #[token("能")]   Can,
    #[token("change")] #[token("改变")] Change,
    #[token("stop")]   #[token("停止")] Stop,
    #[token("again")]  #[token("继续")] Again,
    #[token("try")]    #[token("尝试")] Try,
    #[token("sure")]   #[token("确定")] Sure,
    #[token("maybe")]  #[token("可能")] Maybe,
    #[token("pure")]   #[token("纯")]   Pure,
    #[token("spawn")]  #[token("生成")] Spawn,
    #[token("ok")]     #[token("好")]   Ok,
    #[token("bad")]    #[token("坏")]   Bad,
    #[token("none")]   #[token("无")]   NoneKw,
    #[token("as")]     #[token("为")]   As,
    #[token("where")]  #[token("条件")] Where,

    // ── Literals ─────────────────────────────────────────────────────────────
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
    #[token("verdadero", |_| true)]
    #[token("falso",     |_| false)]
    #[token("vrai",      |_| true)]
    #[token("faux",      |_| false)]
    #[token("wahr",      |_| true)]
    #[token("falsch",    |_| false)]
    Bool(bool),

    #[regex(r#""([^"\\]|\\.)*""#, |lex| {
        let s = lex.slice();
        s[1..s.len()-1]
            .replace("\\n", "\n").replace("\\t", "\t")
            .replace("\\r", "\r").replace("\\\\", "\\")
            .replace("\\\"", "\"")
    })]
    Str(String),

    #[regex(r"[0-9]+(\.[0-9]+)?", |lex| lex.slice().to_string())]
    Number(String),

    #[regex(r"'([^'\\]|\\.)'", |lex| {
        let s = lex.slice();
        let inner = &s[1..s.len()-1];
        match inner { "\\n"=>'\n', "\\t"=>'\t', "\\\\"=>'\\', "\\'"=>'\'', o=>o.chars().next().unwrap_or('\0') }
    })]
    Char(char),

    // ASCII identifier (keywords resolved above via higher priority)
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice().to_string())]
    Ident(String),

    // ── Multi-char operators ──────────────────────────────────────────────────
    #[token("..")] DotDot,
    #[token("::")] ColonColon,
    #[token("==")] EqEq,
    #[token("!=")] Ne,
    #[token("<=")] Le,
    #[token(">=")] Ge,
    #[token("->")] Arrow,
    #[token("=>")] FatArrow,
    #[token("&&")] And,
    #[token("||")] Or,

    // ── Single-char operators ─────────────────────────────────────────────────
    #[token("+")] Plus,   #[token("-")] Minus,
    #[token("*")] Star,   #[token("/")] Slash,
    #[token("%")] Percent,#[token("<")] Lt,
    #[token(">")] Gt,     #[token("=")] Eq,
    #[token("!")] Not,    #[token(".")] Dot,
    #[token("&")] Ampersand,

    // ── Punctuation ───────────────────────────────────────────────────────────
    #[token("(")] LParen,   #[token(")")] RParen,
    #[token("{")] LBrace,   #[token("}")] RBrace,
    #[token("[")] LBracket, #[token("]")] RBracket,
    #[token(",")] Comma,    #[token(":")] Colon,
    #[token(";")] Semicolon,
}

impl From<RawToken> for Token {
    fn from(r: RawToken) -> Token {
        match r {
            RawToken::Bind=>Token::Bind, RawToken::Do=>Token::Do,
            RawToken::Fn=>Token::Fn,     RawToken::Mod=>Token::Mod,
            RawToken::Type=>Token::Type, RawToken::If=>Token::If,
            RawToken::Else=>Token::Else, RawToken::While=>Token::While,
            RawToken::For=>Token::For,   RawToken::In=>Token::In,
            RawToken::Match=>Token::Match, RawToken::Return=>Token::Return,
            RawToken::Own=>Token::Own,   RawToken::Lend=>Token::Lend,
            RawToken::Share=>Token::Share, RawToken::Move=>Token::Move,
            RawToken::Copy=>Token::Copy, RawToken::Async=>Token::Async,
            RawToken::Wait=>Token::Wait, RawToken::Post=>Token::Post,
            RawToken::Give=>Token::Give, RawToken::Fit=>Token::Fit,
            RawToken::Form=>Token::Form, RawToken::Choose=>Token::Choose,
            RawToken::Can=>Token::Can,   RawToken::Change=>Token::Change,
            RawToken::Stop=>Token::Stop, RawToken::Again=>Token::Again,
            RawToken::Try=>Token::Try,   RawToken::Sure=>Token::Sure,
            RawToken::Maybe=>Token::Maybe, RawToken::Pure=>Token::Pure,
            RawToken::Spawn=>Token::Spawn, RawToken::Ok=>Token::Ok,
            RawToken::Bad=>Token::Bad,   RawToken::NoneKw=>Token::NoneKw,
            RawToken::As=>Token::As,     RawToken::Where=>Token::Where,
            RawToken::Bool(b)=>Token::Bool(b), RawToken::Str(s)=>Token::Str(s),
            RawToken::Number(n)=>Token::Number(n), RawToken::Char(c)=>Token::Char(c),
            RawToken::Ident(s)=>Token::Ident(s),
            RawToken::DotDot=>Token::DotDot, RawToken::ColonColon=>Token::ColonColon,
            RawToken::EqEq=>Token::EqEq, RawToken::Ne=>Token::Ne,
            RawToken::Le=>Token::Le,     RawToken::Ge=>Token::Ge,
            RawToken::Arrow=>Token::Arrow, RawToken::FatArrow=>Token::FatArrow,
            RawToken::And=>Token::And,   RawToken::Or=>Token::Or,
            RawToken::Plus=>Token::Plus, RawToken::Minus=>Token::Minus,
            RawToken::Star=>Token::Star, RawToken::Slash=>Token::Slash,
            RawToken::Percent=>Token::Percent, RawToken::Lt=>Token::Lt,
            RawToken::Gt=>Token::Gt,     RawToken::Eq=>Token::Eq,
            RawToken::Not=>Token::Not,   RawToken::Dot=>Token::Dot,
            RawToken::Ampersand=>Token::Ampersand,
            RawToken::LParen=>Token::LParen, RawToken::RParen=>Token::RParen,
            RawToken::LBrace=>Token::LBrace, RawToken::RBrace=>Token::RBrace,
            RawToken::LBracket=>Token::LBracket, RawToken::RBracket=>Token::RBracket,
            RawToken::Comma=>Token::Comma, RawToken::Colon=>Token::Colon,
            RawToken::Semicolon=>Token::Semicolon,
        }
    }
}

pub struct Lexer {
    tokens: Vec<Token>,
    pos: usize,
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        let mut raw_lex = RawToken::lexer(source);
        let mut tokens = Vec::new();
        while let Some(result) = raw_lex.next() {
            match result {
                Ok(raw) => tokens.push(Token::from(raw)),
                Err(_)  => tokens.push(Token::Error(raw_lex.slice().to_string())),
            }
        }
        tokens.push(Token::Eof);
        Self { tokens, pos: 0 }
    }

    pub fn into_tokens(self) -> Vec<Token> { self.tokens }

    pub fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    pub fn next_token(&mut self) -> Option<Token> {
        if self.pos >= self.tokens.len() { return None; }
        let tok = self.tokens[self.pos].clone();
        if self.pos < self.tokens.len() - 1 { self.pos += 1; }
        if tok == Token::Eof { None } else { Some(tok) }
    }
}

impl Iterator for Lexer {
    type Item = Token;
    fn next(&mut self) -> Option<Token> { self.next_token() }
}
