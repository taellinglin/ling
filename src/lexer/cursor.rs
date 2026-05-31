// src/lexer/cursor.rs
#[derive(Clone)]
pub struct Cursor<'a> {
    source: &'a str,
    offset: usize,
    _line: usize,
    col: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(source: &'a str) -> Self {
        Self { source, offset: 0, _line: 0, col: 0 }
    }
    
    pub fn advance(&mut self, count: usize) {
        self.offset += count;
        self.col += count;
    }
    
    pub fn peek(&self) -> Option<char> {
        self.source[self.offset..].chars().next()
    }
}
