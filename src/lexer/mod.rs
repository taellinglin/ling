// src/lexer/mod.rs
mod token;
mod cursor;
mod unicode;

pub use token::Token;
pub use cursor::Cursor;

pub struct Lexer<'a> {
    cursor: Cursor<'a>,
    // Script detection is WIP.
    script: crate::polyglot::Script,
}




impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            cursor: Cursor::new(source),
            script: crate::polyglot::ScriptDetector::detect(source),

        }
    }
    
pub fn next_token(&mut self) -> Option<Token> {
        None
    }

}
