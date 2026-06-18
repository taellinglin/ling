// src/parser/mod.rs
pub mod ast;
pub mod grammar;

use crate::lexer::Token;
use ast::*;

pub fn parse(source: &str) -> Result<Program, String> {
    let mut lex = crate::lexer::Lexer::new(source);
    let mut tokens = Vec::new();
    while let Some(tok) = lex.next_token() {
        tokens.push(tok);
    }
    tokens.push(Token::Eof);
    parse_tokens(tokens)
}

pub fn parse_tokens(tokens: Vec<Token>) -> Result<Program, String> {
    Parser::new(tokens).parse_program()
}

// ─── Parser ──────────────────────────────────────────────────────────────────

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    #[allow(dead_code)]
    fn peek2(&self) -> &Token {
        self.tokens.get(self.pos + 1).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, expected: &Token) -> Result<(), String> {
        let tok = self.advance();
        if &tok == expected {
            Ok(())
        } else {
            Err(format!("expected {:?}, got {:?}", expected, tok))
        }
    }

    /// Parse a name that can be a keyword used as an identifier.
    fn parse_name(&mut self) -> Result<String, String> {
        match self.advance() {
            Token::Ident(s) => Ok(s),
            // Allow all keywords to serve as bind names (contextual)
            tok => Ok(token_to_name(&tok)
                .ok_or_else(|| format!("expected identifier, got {:?}", tok))?
                .to_string()),
        }
    }

    // ─── Top-level ───────────────────────────────────────────────────────────

    fn parse_program(&mut self) -> Result<Program, String> {
        let mut items = Vec::new();
        while !matches!(self.peek(), Token::Eof) {
            items.push(self.parse_item()?);
        }
        Ok(Program { items })
    }

    fn parse_item(&mut self) -> Result<Item, String> {
        let is_async = if matches!(self.peek(), Token::Async) {
            self.advance();
            true
        } else {
            false
        };

        match self.peek().clone() {
            Token::Bind => {
                self.advance();
                let name = self.parse_name()?;
                self.expect(&Token::Eq)?;
                let expr = self.parse_expr()?;
                Ok(Item::Bind(name, expr))
            },
            Token::Fn => self.parse_fn_item(is_async),
            Token::Mod => {
                self.advance();
                let name = self.parse_name()?;
                self.expect(&Token::LBrace)?;
                let mut body = Vec::new();
                while !matches!(self.peek(), Token::RBrace | Token::Eof) {
                    body.push(self.parse_item()?);
                }
                self.expect(&Token::RBrace)?;
                Ok(Item::Mod(name, body))
            },
            Token::Type => {
                self.advance();
                let name = self.parse_name()?;
                // Skip optional generic params
                self.skip_generics();
                self.expect(&Token::As)?;
                let ty = self.parse_type_str();
                Ok(Item::TypeAlias(name, ty))
            },
            Token::Form => self.parse_struct_item(),
            Token::Choose => self.parse_enum_item(),
            Token::Use => {
                self.advance();
                // Expect a string path: use "path/to/module"
                let path = match self.advance() {
                    Token::String(s) => s,
                    tok => return Err(format!("use: expected string path, got {:?}", tok)),
                };
                // Optional `as name` alias
                let alias = if matches!(self.peek(), Token::As) {
                    self.advance();
                    Some(self.parse_name()?)
                } else {
                    None
                };
                Ok(Item::Use { path, alias })
            },
            tok => Err(format!("unexpected token at top level: {:?}", tok)),
        }
    }

    /// `form Name { field (: Type)?, ... }` — record/struct definition.
    /// Types after `:` are parsed and discarded; only field names (in order) survive.
    fn parse_struct_item(&mut self) -> Result<Item, String> {
        self.advance(); // consume `form`
        let name = self.parse_name()?;
        self.skip_generics();
        self.expect(&Token::LBrace)?;
        let mut fields = Vec::new();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            fields.push(self.parse_name()?);
            if matches!(self.peek(), Token::Colon) {
                self.advance();
                self.parse_type_str(); // ignored
            }
            if matches!(self.peek(), Token::Comma | Token::Semicolon) {
                self.advance();
            }
        }
        self.expect(&Token::RBrace)?;
        Ok(Item::Struct(name, fields))
    }

    /// `choose Name { Variant, Variant(a, b), ... }` — sum type / enum.
    /// Each variant records its name and payload arity; payload field names/types
    /// are parsed and discarded.
    fn parse_enum_item(&mut self) -> Result<Item, String> {
        self.advance(); // consume `choose`
        let name = self.parse_name()?;
        self.skip_generics();
        self.expect(&Token::LBrace)?;
        let mut variants = Vec::new();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            let vname = self.parse_name()?;
            let mut arity = 0usize;
            if matches!(self.peek(), Token::LParen) {
                self.advance();
                while !matches!(self.peek(), Token::RParen | Token::Eof) {
                    // A payload slot may be `name: Type`, just `Type`, or just a name.
                    self.parse_name().ok();
                    if matches!(self.peek(), Token::Colon) {
                        self.advance();
                        self.parse_type_str();
                    }
                    arity += 1;
                    if matches!(self.peek(), Token::Comma) {
                        self.advance();
                    }
                }
                self.expect(&Token::RParen)?;
            }
            variants.push(EnumVariant { name: vname, arity });
            if matches!(self.peek(), Token::Comma | Token::Semicolon) {
                self.advance();
            }
        }
        self.expect(&Token::RBrace)?;
        Ok(Item::Enum(name, variants))
    }

    fn parse_fn_item(&mut self, is_async: bool) -> Result<Item, String> {
        self.advance(); // consume `fn`
        let name = self.parse_name()?;
        self.skip_generics(); // ignore `<T, U>`
        self.expect(&Token::LParen)?;
        let params = self.parse_params()?;
        self.expect(&Token::RParen)?;
        // Optional return type: `-> Type`
        if matches!(self.peek(), Token::Arrow) {
            self.advance();
            self.parse_type_str();
        }
        // Optional `where` clause
        if matches!(self.peek(), Token::Where) {
            self.advance();
            while !matches!(self.peek(), Token::LBrace | Token::Eof) {
                self.advance();
            }
        }
        self.expect(&Token::LBrace)?;
        let body = self.parse_block()?;
        self.expect(&Token::RBrace)?;
        Ok(Item::Fn(FnDef { name, is_async, params, body }))
    }

    fn parse_params(&mut self) -> Result<Vec<String>, String> {
        let mut params = Vec::new();
        while !matches!(self.peek(), Token::RParen | Token::Eof) {
            // Skip leading & or own/lend
            while matches!(self.peek(), Token::Ampersand | Token::Own | Token::Lend) {
                self.advance();
            }
            let name = self.parse_name()?;
            params.push(name);
            // Skip `: Type`
            if matches!(self.peek(), Token::Colon) {
                self.advance();
                self.parse_type_str();
            }
            if matches!(self.peek(), Token::Comma) {
                self.advance();
            }
        }
        Ok(params)
    }

    /// Eat a type expression (until `,`, `)`, `{`, `where`, `>`) — ignored at runtime.
    fn parse_type_str(&mut self) -> String {
        let mut depth = 0usize;
        let mut result = String::new();
        loop {
            match self.peek() {
                Token::Eof => break,
                Token::Lt => {
                    depth += 1;
                    result.push('<');
                    self.advance();
                },
                Token::Gt if depth > 0 => {
                    depth -= 1;
                    result.push('>');
                    self.advance();
                },
                Token::LBrace | Token::Where if depth == 0 => break,
                Token::RParen | Token::Comma | Token::Semicolon if depth == 0 => break,
                Token::Arrow if depth == 0 => break,
                tok => {
                    result.push_str(&format!("{:?}", tok));
                    self.advance();
                },
            }
        }
        result
    }

    fn skip_generics(&mut self) {
        if matches!(self.peek(), Token::Lt) {
            let mut depth = 0;
            loop {
                match self.advance() {
                    Token::Lt => depth += 1,
                    Token::Gt => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    },
                    Token::Eof => break,
                    _ => {},
                }
            }
        }
    }

    // ─── Blocks ──────────────────────────────────────────────────────────────

    fn parse_block(&mut self) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            stmts.push(self.parse_stmt()?);
            // Optional trailing semicolon
            if matches!(self.peek(), Token::Semicolon) {
                self.advance();
            }
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        match self.peek().clone() {
            Token::Bind => {
                self.advance();
                let name = self.parse_name()?;
                self.expect(&Token::Eq)?;
                let expr = self.parse_expr()?;
                Ok(Stmt::Bind(name, expr))
            },
            Token::Return => {
                self.advance();
                if matches!(self.peek(), Token::Semicolon | Token::RBrace) {
                    Ok(Stmt::Return(Expr::Unit))
                } else {
                    Ok(Stmt::Return(self.parse_expr()?))
                }
            },
            _ => Ok(Stmt::Expr(self.parse_expr()?)),
        }
    }

    // ─── Expressions ─────────────────────────────────────────────────────────

    fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_or_expr()
    }

    fn parse_or_expr(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and_expr()?;
        while matches!(self.peek(), Token::Or) {
            self.advance();
            let right = self.parse_and_expr()?;
            left = Expr::BinOp(BinOp::Or, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and_expr(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_cmp_expr()?;
        while matches!(self.peek(), Token::And) {
            self.advance();
            let right = self.parse_cmp_expr()?;
            left = Expr::BinOp(BinOp::And, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_cmp_expr(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_add_expr()?;
        loop {
            let op = match self.peek() {
                Token::EqEq => BinOp::Eq,
                Token::Ne => BinOp::Ne,
                Token::Lt => BinOp::Lt,
                Token::Gt => BinOp::Gt,
                Token::Le => BinOp::Le,
                Token::Ge => BinOp::Ge,
                _ => break,
            };
            self.advance();
            let right = self.parse_add_expr()?;
            left = Expr::BinOp(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_add_expr(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_mul_expr()?;
        loop {
            let op = match self.peek() {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_mul_expr()?;
            left = Expr::BinOp(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_mul_expr(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary_expr()?;
        loop {
            let op = match self.peek() {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                Token::Percent => BinOp::Rem,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary_expr()?;
            left = Expr::BinOp(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary_expr(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            Token::Ampersand => {
                self.advance();
                Ok(Expr::Ref(Box::new(self.parse_postfix_expr()?)))
            },
            Token::Not => {
                self.advance();
                Ok(Expr::BinOp(
                    BinOp::Eq,
                    Box::new(self.parse_postfix_expr()?),
                    Box::new(Expr::Bool(false)),
                ))
            },
            Token::Minus => {
                self.advance();
                Ok(Expr::BinOp(
                    BinOp::Sub,
                    Box::new(Expr::Number(0.0)),
                    Box::new(self.parse_postfix_expr()?),
                ))
            },
            Token::Wait => {
                self.advance();
                Ok(Expr::Await(Box::new(self.parse_postfix_expr()?)))
            },
            // Ownership modifiers are hints; just evaluate the inner expression
            Token::Own | Token::Lend | Token::Share | Token::Move | Token::Copy => {
                self.advance();
                self.parse_unary_expr()
            },
            _ => self.parse_postfix_expr(),
        }
    }

    /// Parse suffix operations: calls, method calls, indexing, `..`, path (::)
    fn parse_postfix_expr(&mut self) -> Result<Expr, String> {
        let mut base = self.parse_primary()?;

        loop {
            match self.peek().clone() {
                // `.method(args)` or `.field`
                Token::Dot => {
                    self.advance();
                    let method = self.parse_name()?;
                    if matches!(self.peek(), Token::LParen) {
                        self.advance();
                        let args = self.parse_call_args()?;
                        self.expect(&Token::RParen)?;
                        base = Expr::MethodCall { receiver: Box::new(base), method, args };
                    } else {
                        // field access — treat as method call with no args
                        base =
                            Expr::MethodCall { receiver: Box::new(base), method, args: Vec::new() };
                    }
                },
                // `::ident` — extend path
                Token::ColonColon => {
                    self.advance();
                    let segment = self.parse_name()?;
                    // Collect any further `::` segments
                    base = match base {
                        Expr::Path(mut segs) => {
                            segs.push(segment);
                            Expr::Path(segs)
                        },
                        Expr::Ident(s) => Expr::Path(vec![s, segment]),
                        other => Expr::Path(vec![format!("{:?}", other), segment]),
                    };
                },
                // `(args)` — call
                Token::LParen => {
                    self.advance();
                    let args = self.parse_call_args()?;
                    self.expect(&Token::RParen)?;
                    base = Expr::Call(Box::new(base), args);
                },
                // `[idx]`
                Token::LBracket => {
                    self.advance();
                    let idx = self.parse_expr()?;
                    self.expect(&Token::RBracket)?;
                    base = Expr::Index(Box::new(base), Box::new(idx));
                },
                // `..hi` — range
                Token::DotDot => {
                    self.advance();
                    let hi = self.parse_primary()?;
                    base = Expr::Range(Box::new(base), Box::new(hi));
                },
                _ => break,
            }
        }
        Ok(base)
    }

    fn parse_call_args(&mut self) -> Result<Vec<Expr>, String> {
        let mut args = Vec::new();
        while !matches!(self.peek(), Token::RParen | Token::Eof) {
            args.push(self.parse_expr()?);
            if matches!(self.peek(), Token::Comma) {
                self.advance();
            }
        }
        Ok(args)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            // Literals
            Token::String(s) => {
                self.advance();
                Ok(Expr::Str(s))
            },
            Token::Number(n) => {
                self.advance();
                Ok(Expr::Number(n.parse().unwrap_or(0.0)))
            },
            Token::Bool(b) => {
                self.advance();
                Ok(Expr::Bool(b))
            },

            // Keywords that start expressions
            Token::Do => {
                self.advance();
                self.expect(&Token::LBrace)?;
                let stmts = self.parse_block()?;
                self.expect(&Token::RBrace)?;
                Ok(Expr::Do(stmts))
            },
            Token::LBrace => {
                self.advance();
                let stmts = self.parse_block()?;
                self.expect(&Token::RBrace)?;
                Ok(Expr::Do(stmts))
            },
            Token::If => self.parse_if_expr(),
            Token::For => self.parse_for_expr(),
            Token::While => self.parse_while_expr(),
            Token::Match => self.parse_match_expr(),
            Token::Return => {
                self.advance();
                let val = if matches!(self.peek(), Token::Semicolon | Token::RBrace | Token::Eof) {
                    Expr::Unit
                } else {
                    self.parse_expr()?
                };
                Ok(Expr::Do(vec![Stmt::Return(val)]))
            },

            // Array literal
            Token::LBracket => {
                self.advance();
                let mut elems = Vec::new();
                while !matches!(self.peek(), Token::RBracket | Token::Eof) {
                    elems.push(self.parse_expr()?);
                    if matches!(self.peek(), Token::Comma) {
                        self.advance();
                    }
                }
                self.expect(&Token::RBracket)?;
                Ok(Expr::Array(elems))
            },

            // Closure `|| expr` or `|params| expr`
            Token::Or => {
                self.advance(); // first |
                let mut params = Vec::new();
                // If next is not |, parse params
                if !matches!(self.peek(), Token::Or) {
                    while !matches!(self.peek(), Token::Or | Token::Eof) {
                        params.push(self.parse_name()?);
                        if matches!(self.peek(), Token::Comma) {
                            self.advance();
                        }
                    }
                }
                self.advance(); // closing |
                let body = self.parse_expr()?;
                Ok(Expr::Closure(params, Box::new(body)))
            },

            // Grouped expression
            Token::LParen => {
                self.advance();
                if matches!(self.peek(), Token::RParen) {
                    self.advance();
                    return Ok(Expr::Unit);
                }
                let e = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                Ok(e)
            },

            // Async block
            Token::Async => {
                self.advance();
                let inner = self.parse_expr()?;
                Ok(inner) // async is a hint; we just execute synchronously
            },

            // Identifier — could start a path
            Token::Ident(name) => {
                self.advance();
                Ok(Expr::Ident(name))
            },

            // Allow keywords as expression-position identifiers (e.g. 移动/move)
            tok => {
                if let Some(name) = token_to_name(&tok) {
                    self.advance();
                    Ok(Expr::Ident(name.to_string()))
                } else {
                    Err(format!("unexpected token in expression: {:?}", tok))
                }
            },
        }
    }

    // ─── if / for / match ────────────────────────────────────────────────────

    fn parse_if_expr(&mut self) -> Result<Expr, String> {
        self.advance(); // consume `if`
        let cond = self.parse_cmp_expr()?;
        self.expect(&Token::LBrace)?;
        let then = self.parse_block()?;
        self.expect(&Token::RBrace)?;

        let mut elseifs = Vec::new();
        let mut else_body = None;

        while matches!(self.peek(), Token::Else) {
            self.advance(); // consume `else`
            if matches!(self.peek(), Token::If) {
                self.advance(); // consume `if`
                let ei_cond = self.parse_cmp_expr()?;
                self.expect(&Token::LBrace)?;
                let ei_body = self.parse_block()?;
                self.expect(&Token::RBrace)?;
                elseifs.push((ei_cond, ei_body));
            } else {
                self.expect(&Token::LBrace)?;
                else_body = Some(self.parse_block()?);
                self.expect(&Token::RBrace)?;
                break;
            }
        }

        Ok(Expr::If { cond: Box::new(cond), then, elseifs, else_body })
    }

    fn parse_while_expr(&mut self) -> Result<Expr, String> {
        self.advance(); // consume `while` / `ขณะที่`
        let cond = self.parse_expr()?;
        self.expect(&Token::LBrace)?;
        let body = self.parse_block()?;
        self.expect(&Token::RBrace)?;
        Ok(Expr::While { cond: Box::new(cond), body })
    }

    fn parse_for_expr(&mut self) -> Result<Expr, String> {
        self.advance(); // consume `for` / `历`
        let var = self.parse_name()?;
        self.expect(&Token::In)?;
        let iter = self.parse_postfix_expr()?;
        // Handle `0..n` already parsed as Range in postfix
        self.expect(&Token::LBrace)?;
        let body = self.parse_block()?;
        self.expect(&Token::RBrace)?;
        Ok(Expr::For { var, iter: Box::new(iter), body })
    }

    fn parse_match_expr(&mut self) -> Result<Expr, String> {
        self.advance(); // consume `match` / `配`
        let subject = self.parse_postfix_expr()?;
        self.expect(&Token::LBrace)?;
        let mut arms = Vec::new();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            let pattern = self.parse_pattern()?;
            self.expect(&Token::FatArrow)?;
            let body = self.parse_expr()?;
            // Optional trailing comma
            if matches!(self.peek(), Token::Comma) {
                self.advance();
            }
            arms.push(MatchArm { pattern, body });
        }
        self.expect(&Token::RBrace)?;
        Ok(Expr::Match(Box::new(subject), arms))
    }

    fn parse_pattern(&mut self) -> Result<Pattern, String> {
        match self.peek().clone() {
            Token::Ident(s) if s == "_" => {
                self.advance();
                Ok(Pattern::Wildcard)
            },
            Token::String(s) => {
                self.advance();
                Ok(Pattern::Str(s))
            },
            Token::Number(n) => {
                self.advance();
                Ok(Pattern::Number(n.parse().unwrap_or(0.0)))
            },
            Token::Bool(b) => {
                self.advance();
                Ok(Pattern::Bool(b))
            },
            // Ok(inner), Bad(inner), 好(inner), 坏(inner)
            Token::Ok | Token::Bad => {
                let ctor_tok = self.advance();
                let ctor = match ctor_tok {
                    Token::Ok => "ok".to_string(),
                    Token::Bad => "bad".to_string(),
                    _ => unreachable!(),
                };
                if matches!(self.peek(), Token::LParen) {
                    self.advance();
                    let inner = self.parse_pattern()?;
                    self.expect(&Token::RParen)?;
                    Ok(Pattern::Constructor(ctor, Some(Box::new(inner))))
                } else {
                    Ok(Pattern::Constructor(ctor, None))
                }
            },
            _ => {
                let name = self.parse_name()?;
                if matches!(self.peek(), Token::LParen) {
                    // User enum variant pattern: `Variant(p1, p2, ...)` or nullary `Variant()`.
                    self.advance();
                    let mut subs = Vec::new();
                    while !matches!(self.peek(), Token::RParen | Token::Eof) {
                        subs.push(self.parse_pattern()?);
                        if matches!(self.peek(), Token::Comma) {
                            self.advance();
                        }
                    }
                    self.expect(&Token::RParen)?;
                    Ok(Pattern::Variant(name, subs))
                } else if name == "_" {
                    Ok(Pattern::Wildcard)
                } else {
                    Ok(Pattern::Ident(name))
                }
            },
        }
    }
}

// ─── Helper ──────────────────────────────────────────────────────────────────

/// Convert a keyword token to its string name for use as an identifier.
fn token_to_name(tok: &Token) -> Option<&'static str> {
    match tok {
        Token::Bind => Some("bind"),
        Token::Do => Some("do"),
        Token::Fn => Some("fn"),
        Token::Mod => Some("mod"),
        Token::Type => Some("type"),
        Token::If => Some("if"),
        Token::Else => Some("else"),
        Token::While => Some("while"),
        Token::For => Some("for"),
        Token::In => Some("in"),
        Token::Match => Some("match"),
        Token::Return => Some("return"),
        Token::Own => Some("own"),
        Token::Lend => Some("lend"),
        Token::Share => Some("share"),
        Token::Move => Some("move"),
        Token::Copy => Some("copy"),
        Token::Async => Some("async"),
        Token::Wait => Some("wait"),
        Token::As => Some("as"),
        Token::Where => Some("where"),
        Token::Post => Some("post"),
        Token::Give => Some("give"),
        Token::Fit => Some("fit"),
        Token::Form => Some("form"),
        Token::Choose => Some("choose"),
        Token::Can => Some("can"),
        Token::Change => Some("change"),
        Token::Stop => Some("stop"),
        Token::Again => Some("again"),
        Token::Try => Some("try"),
        Token::Sure => Some("sure"),
        Token::Maybe => Some("maybe"),
        Token::Pure => Some("pure"),
        Token::Spawn => Some("spawn"),
        Token::Ok => Some("ok"),
        Token::Bad => Some("bad"),
        Token::None => Some("none"),
        Token::Use => Some("use"),
        _ => None,
    }
}
