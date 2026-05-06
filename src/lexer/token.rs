// src/lexer/token.rs
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Keywords
    Bind, Start, Post, Give, Fit, Form, Choose, Can, Lend, Share, Own, Change, Move, Copy,
    If, Else, While, For, Stop, Again, Try, Sure, Maybe, Result, Ok, Bad, None, Pure, Do, Async, Wait, Spawn,
    
    // Identifiers & literals
    Ident(String), Number(String), String(String), Char(char), Bool(bool),
    
    // Operators
    Plus, Minus, Star, Slash, Percent, Eq, EqEq, Ne, Lt, Gt, Le, Ge, And, Or, Not, Arrow, FatArrow, Dot,
    
    // Punctuation
    LParen, RParen, LBrace, RBrace, LBracket, RBracket, Comma, Colon, Semicolon,
    
    // Special
    Whitespace, Comment(String), Error(String), Eof,
}
