// src/lexer/mod.rs — hand-written polyglot lexer
mod cursor;
mod token;
mod unicode;

pub use cursor::Cursor;
pub use token::Token;

pub struct Lexer<'a> {
    source: &'a str,
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self { source, pos: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.pos..].chars().next()
    }

    fn peek_nth(&self, n: usize) -> Option<char> {
        self.source[self.pos..].chars().nth(n)
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.source[self.pos..].chars().next()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }

    fn rest(&self) -> &str {
        &self.source[self.pos..]
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            while matches!(
                self.peek(),
                Some(' ') | Some('\t') | Some('\n') | Some('\r') | Some('\x0C')
            ) {
                self.advance();
            }
            if self.peek() == Some('/') && self.peek_nth(1) == Some('/') {
                while self.peek().map_or(false, |c| c != '\n') {
                    self.advance();
                }
                continue;
            }
            // Shebang or # comment
            if self.peek() == Some('#') {
                while self.peek().map_or(false, |c| c != '\n') {
                    self.advance();
                }
                continue;
            }
            break;
        }
    }

    fn lex_string(&mut self) -> Token {
        self.advance(); // opening "
        let mut s = std::string::String::new();
        loop {
            match self.advance() {
                None => break,
                Some('"') => break,
                Some('\\') => match self.advance() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
                    Some('"') => s.push('"'),
                    Some('\\') => s.push('\\'),
                    Some('0') => s.push('\0'),
                    Some(c) => {
                        s.push('\\');
                        s.push(c);
                    },
                    None => break,
                },
                Some(c) => s.push(c),
            }
        }
        Token::String(s)
    }

    fn lex_number(&mut self) -> Token {
        let start = self.pos;
        let mut has_dot = false;
        loop {
            match self.peek() {
                Some('0'..='9') => {
                    self.advance();
                },
                Some('.') if !has_dot && matches!(self.peek_nth(1), Some('0'..='9')) => {
                    has_dot = true;
                    self.advance();
                },
                _ => break,
            }
        }
        Token::Number(self.source[start..self.pos].to_string())
    }

    fn lex_word(&mut self) -> Token {
        let start = self.pos;
        // Scan contiguous word characters: letters, digits, underscore, and Unicode
        // combining marks (tone marks, vowel signs, diacritics) that Rust's
        // is_alphabetic() misses because they lack the Other_Alphabetic property.
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || is_unicode_combining(c) {
                self.advance();
            } else {
                break;
            }
        }
        let word = &self.source[start..self.pos];
        Self::classify_word(word)
    }

    /// Map a word (ASCII or Unicode) to its canonical token.
    /// Chinese/Japanese/Korean/Russian keywords resolve here so there is
    /// no pre-processing that could corrupt variable names or string contents.
    fn classify_word(word: &str) -> Token {
        match word {
            // ── English keywords ────────────────────────────────────────────
            "bind" | "令" | "灵符" => Token::Bind,
            "do" | "执" => Token::Do,
            "fn" | "函" => Token::Fn,
            "mod" | "核" => Token::Mod,
            "type" | "符" => Token::Type,
            // Module imports: use / 载 (zh) / 使う (ja) / 사용 (ko) / ใช้ / นำเข้า (th)
            "use" | "载" | "引" | "사용" | "使う" | "ใช้" | "นำเข้า" | "importar" | "nutzen"
            | "utiliser" => Token::Use,
            "if" | "若" | "如" => Token::If,
            "else" | "否则" | "否" => Token::Else,
            "while" | "循" | "当" => Token::While,
            "for" | "历" => Token::For,
            "in" | "于" => Token::In,
            "match" | "配" => Token::Match,
            "return" | "归" => Token::Return,
            "own" | "拥有" | "独" => Token::Own,
            "lend" | "借" => Token::Lend,
            "share" | "共享" | "共" => Token::Share,
            "move" | "移动" | "移" => Token::Move,
            "copy" | "复制" | "复" => Token::Copy,
            "async" | "异步" | "异" => Token::Async,
            "wait" | "等待" | "待" => Token::Wait,
            "as" | "为" => Token::As,
            "where" | "条件" => Token::Where,
            "post" | "发布" | "出" => Token::Post,
            "give" | "给" | "予" => Token::Give,
            "fit" | "适合" => Token::Fit,
            // `form` — record/struct definition (EN · ZH · JA · KO · TH)
            "form" | "形式" | "形" | "構造" | "구조" | "โครงสร้าง" => {
                Token::Form
            },
            // `choose` — sum type / enum definition (EN · ZH · JA · KO · TH)
            "choose" | "选择" | "选" | "選択" | "선택" | "เลือกแบบ" => {
                Token::Choose
            },
            "can" | "能" => Token::Can,
            "change" | "改变" | "变" => Token::Change,
            "stop" | "停止" | "止" => Token::Stop,
            "again" | "继续" => Token::Again,
            "try" | "尝试" | "试" => Token::Try,
            "sure" | "确定" | "确" => Token::Sure,
            "maybe" | "可能" | "或" => Token::Maybe,
            "pure" | "纯" => Token::Pure,
            "spawn" | "生成" | "启" => Token::Spawn,
            "ok" | "好" | "可" => Token::Ok,
            "bad" | "坏" | "误" => Token::Bad,
            "none" | "无" => Token::None,
            // Japanese — full keyword set with short kanji shorthands
            "束縛" | "バ" => Token::Bind,
            "実行" | "執" => Token::Do,
            "関数" | "関" => Token::Fn,
            "モジュール" | "模" => Token::Mod,
            "もし" => Token::If,
            "他" => Token::Else,
            "間" | "一方" => Token::While,
            "繰" | "ために" => Token::For,
            "の中" => Token::In,
            "一致" => Token::Match,
            "戻る" | "帰る" => Token::Return,
            "試す" => Token::Try,
            "待つ" => Token::Wait,
            "非同期" => Token::Async,
            "起動" => Token::Spawn,
            "止まれ" | "停め" => Token::Stop,
            "継続" => Token::Again,
            // Booleans — shared across Chinese, Japanese, Korean, Thai (already in Thai section)
            "true" | "真" => Token::Bool(true),
            "false" | "假" | "偽" => Token::Bool(false),
            // Korean — full keyword set with short Hangul forms
            "바인드" | "묶" => Token::Bind,
            "실행" => Token::Do,
            "함수" => Token::Fn,
            "모듈" => Token::Mod,
            "만약" | "조건" => Token::If,
            "아니면" => Token::Else,
            "동안" | "반복" => Token::While,
            "위해" => Token::For,
            "안에" => Token::In,
            "매치" => Token::Match,
            "반환" | "귀환" => Token::Return,
            "시도" => Token::Try,
            "기다려" => Token::Wait,
            "비동기" => Token::Async,
            "생성" => Token::Spawn,
            "멈춤" => Token::Stop,
            "계속" => Token::Again,
            "참" => Token::Bool(true),
            "거짓" => Token::Bool(false),
            // Russian
            "связать" => Token::Bind,
            "сделать" => Token::Do,
            "если" => Token::If,
            "иначе" => Token::Else,
            "пока" => Token::While,
            "для" => Token::For,
            "вернуть" => Token::Return,
            // Thai
            "ผูก" => Token::Bind,
            "ทำ" => Token::Do,
            "ฟังก์ชัน" => Token::Fn,
            "โมดูล" => Token::Mod,
            "ถ้า" => Token::If,
            "มิฉะนั้น" => Token::Else,
            "ขณะที่" => Token::While,
            "สำหรับ" => Token::For,
            "ใน" => Token::In,
            "จับคู่" => Token::Match,
            "คืน" => Token::Return,
            "รอ" => Token::Wait,
            "ไม่พร้อมกัน" => Token::Async,
            "จริง" => Token::Bool(true),
            "เท็จ" => Token::Bool(false),
            // Hindi
            "बाँधो" => Token::Bind,
            "करो" => Token::Do,
            "अगर" => Token::If,
            "नहींतो" => Token::Else,
            "जबकि" => Token::While,
            "केलिए" => Token::For,
            "वापस" => Token::Return,
            "सत्य" => Token::Bool(true),
            "असत्य" => Token::Bool(false),
            // Arabic
            "ربط" => Token::Bind,
            "افعل" => Token::Do,
            "إذا" => Token::If,
            "وإلا" => Token::Else,
            "بينما" => Token::While,
            "لأجل" => Token::For,
            "في" => Token::In,
            "أعد" => Token::Return,
            "صحيح" => Token::Bool(true),
            "خطأ" => Token::Bool(false),
            // Spanish
            "enlazar" => Token::Bind,
            "hacer" => Token::Do,
            "si" => Token::If,
            "sino" => Token::Else,
            "mientras" => Token::While,
            "para" => Token::For,
            "retornar" => Token::Return,
            "verdadero" => Token::Bool(true),
            "falso" => Token::Bool(false),
            // French
            "lier" => Token::Bind,
            "faire" => Token::Do,
            "func" => Token::Fn,
            "module" => Token::Mod,
            "sinon" => Token::Else,
            "tantque" => Token::While,
            "retourner" => Token::Return,
            "vrai" => Token::Bool(true),
            "faux" => Token::Bool(false),
            // German
            "binden" => Token::Bind,
            "machen" => Token::Do,
            "wenn" => Token::If,
            "sonst" => Token::Else,
            "solange" => Token::While,
            "für" => Token::For,
            "zurück" => Token::Return,
            "wahr" => Token::Bool(true),
            "falsch" => Token::Bool(false),
            // Portuguese (deduplicated from Spanish)
            "ligar" => Token::Bind,
            "fazer" => Token::Do,
            "se" => Token::If,
            "senão" => Token::Else,
            "enquanto" => Token::While,
            "verdadeiro" => Token::Bool(true),
            // Everything else is an identifier
            other => Token::Ident(other.to_string()),
        }
    }

    pub fn next_token(&mut self) -> Option<Token> {
        self.skip_whitespace_and_comments();
        let ch = self.peek()?;

        // String literal
        if ch == '"' {
            return Some(self.lex_string());
        }

        // Number literal
        if ch.is_ascii_digit() {
            return Some(self.lex_number());
        }

        // Word (keyword or identifier) — handles ASCII and all Unicode letters/ideographs
        if ch.is_alphabetic() || ch == '_' || is_unicode_combining(ch) {
            return Some(self.lex_word());
        }

        // Multi-character punctuation / operators
        let rest = self.rest();

        // `..`
        if rest.starts_with("..") {
            self.advance();
            self.advance();
            return Some(Token::DotDot);
        }
        // `::`
        if rest.starts_with("::") {
            self.advance();
            self.advance();
            return Some(Token::ColonColon);
        }
        // `==`
        if rest.starts_with("==") {
            self.advance();
            self.advance();
            return Some(Token::EqEq);
        }
        // `!=`
        if rest.starts_with("!=") {
            self.advance();
            self.advance();
            return Some(Token::Ne);
        }
        // `<=`
        if rest.starts_with("<=") {
            self.advance();
            self.advance();
            return Some(Token::Le);
        }
        // `>=`
        if rest.starts_with(">=") {
            self.advance();
            self.advance();
            return Some(Token::Ge);
        }
        // `->`
        if rest.starts_with("->") {
            self.advance();
            self.advance();
            return Some(Token::Arrow);
        }
        // `=>`
        if rest.starts_with("=>") {
            self.advance();
            self.advance();
            return Some(Token::FatArrow);
        }
        // `&&`
        if rest.starts_with("&&") {
            self.advance();
            self.advance();
            return Some(Token::And);
        }
        // `||`
        if rest.starts_with("||") {
            self.advance();
            self.advance();
            return Some(Token::Or);
        }

        // Single-character tokens
        self.advance();
        Some(match ch {
            '=' => Token::Eq,
            '<' => Token::Lt,
            '>' => Token::Gt,
            '!' => Token::Not,
            '+' => Token::Plus,
            '-' => Token::Minus,
            '*' => Token::Star,
            '/' => Token::Slash,
            '%' => Token::Percent,
            '.' => Token::Dot,
            '&' => Token::Ampersand,
            '(' => Token::LParen,
            ')' => Token::RParen,
            '{' => Token::LBrace,
            '}' => Token::RBrace,
            '[' => Token::LBracket,
            ']' => Token::RBracket,
            ',' => Token::Comma,
            ':' => Token::Colon,
            ';' => Token::Semicolon,
            '|' => Token::Or, // single | treated as Or (for closure context)
            c => Token::Error(c.to_string()),
        })
    }
}

/// Returns true for Unicode combining marks (Mn/Mc category) that Rust's
/// `is_alphabetic()` may miss — Thai tone marks, Devanagari vowel signs,
/// Arabic diacritics, Hebrew cantillation, etc.
fn is_unicode_combining(c: char) -> bool {
    let cp = c as u32;
    matches!(cp,
        0x0300..=0x036F |   // Combining Diacritical Marks (Latin)
        0x0483..=0x0489 |   // Combining Cyrillic
        0x0591..=0x05C7 |   // Hebrew cantillation / points
        0x0610..=0x061A |   // Arabic extended
        0x064B..=0x065F |   // Arabic diacritics (harakat)
        0x0670         |    // Arabic superscript alef
        0x06D6..=0x06DC |   // Arabic small high letters
        0x0730..=0x074A |   // Syriac diacritics
        0x0816..=0x082D |   // Samaritan diacritics
        0x0900..=0x0903 |   // Devanagari (anusvara, visarga)
        0x093A..=0x094F |   // Devanagari vowel signs / halant
        0x0951..=0x0957 |   // Devanagari stress marks
        0x0962..=0x0963 |   // Devanagari vowel signs
        0x0981..=0x0983 |   // Bengali
        0x09BC         |    // Bengali nukta
        0x09BE..=0x09C4 |   // Bengali vowel signs
        0x09C7..=0x09C8 |   // Bengali vowel signs
        0x09CB..=0x09CD |   // Bengali
        0x0A01..=0x0A03 |   // Gurmukhi
        0x0A3C         |    // Gurmukhi nukta
        0x0A3E..=0x0A42 |   // Gurmukhi vowels
        0x0B01..=0x0B03 |   // Oriya
        0x0B3C..=0x0B4D |   // Oriya
        0x0C00..=0x0C03 |   // Telugu
        0x0C3E..=0x0C56 |   // Telugu vowels
        0x0D00..=0x0D03 |   // Malayalam
        0x0D3B..=0x0D4D |   // Malayalam vowels / chandrakkala
        0x0E31         |    // Thai SARA AM
        0x0E34..=0x0E3A |   // Thai vowel signs
        0x0E47..=0x0E4E |   // Thai tone marks & other signs
        0x0EB1         |    // Lao vowel sign
        0x0EB4..=0x0EBC |   // Lao vowel signs
        0x0EC8..=0x0ECD |   // Lao tone marks
        0x3099..=0x309A |   // Japanese combining dakuten
        0xFE20..=0xFE2F     // Combining Half Marks
    )
}
