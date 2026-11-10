//! AUTOMATIC1111 prompt syntax (port of the host engine's `prompt.rs`, itself prompt_parser.py::parse_prompt_attention):
//!   (text) x1.1   ((text)) x1.21   [text] /1.1   (text:1.4) x1.4   \( \) \[ \] literal   BREAK starts a new 75-token chunk.
//! `<lora:...>` tags are dropped (LoRAs are not supported on LingOS yet).
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Text(String, f32),
    Break,
}

/// The prompt without `<...>` extra-network tags.
pub fn strip_networks(prompt: &str) -> String {
    let mut out = String::new();
    let mut rest = prompt;
    while let Some(st) = rest.find('<') {
        let Some(len) = rest[st..].find('>') else { break };
        let inner = &rest[st + 1..st + len];
        let kind = inner.split(':').next().unwrap_or("").trim().to_ascii_lowercase();
        if kind == "lora" || kind == "lyco" || kind == "hypernet" {
            out.push_str(&rest[..st]);
        } else {
            out.push_str(&rest[..st + len + 1]);
        }
        rest = &rest[st + len + 1..];
    }
    out.push_str(rest);
    out
}

pub fn parse_attention(text: &str) -> Vec<Piece> {
    let mut res: Vec<(String, f32)> = Vec::new();
    let mut round: Vec<usize> = Vec::new();
    let mut square: Vec<usize> = Vec::new();
    fn mul(res: &mut [(String, f32)], from: usize, m: f32) {
        for r in res.iter_mut().skip(from) {
            if r.1 >= 0.0 {
                r.1 *= m;
            }
        }
    }
    fn push_text(res: &mut Vec<(String, f32)>, t: &str) {
        let mut first = true;
        for part in split_break(t) {
            if !first {
                res.push((String::new(), -1.0));
            }
            first = false;
            if !part.is_empty() {
                res.push((part.to_string(), 1.0));
            }
        }
    }
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            if i + 1 < chars.len() && matches!(chars[i + 1], '(' | ')' | '[' | ']' | '\\') {
                res.push((chars[i + 1].to_string(), 1.0));
                i += 2;
            } else {
                res.push(("\\".to_string(), 1.0));
                i += 1;
            }
            continue;
        }
        match c {
            '(' => {
                round.push(res.len());
                i += 1;
            },
            '[' => {
                square.push(res.len());
                i += 1;
            },
            ':' => {
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                let ns = j;
                if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                    j += 1;
                }
                while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == '.') {
                    j += 1;
                }
                let num: String = chars[ns..j].iter().collect();
                let mut k = j;
                while k < chars.len() && chars[k].is_whitespace() {
                    k += 1;
                }
                let w = num.parse::<f32>().ok();
                if let (Some(w), true) = (w, k < chars.len() && chars[k] == ')') {
                    if let Some(p) = round.pop() {
                        mul(&mut res, p, w);
                    } else {
                        push_text(&mut res, &chars[i..=k].iter().collect::<String>());
                    }
                    i = k + 1;
                } else {
                    push_text(&mut res, ":");
                    i += 1;
                }
            },
            ')' => {
                if let Some(p) = round.pop() {
                    mul(&mut res, p, 1.1);
                } else {
                    push_text(&mut res, ")");
                }
                i += 1;
            },
            ']' => {
                if let Some(p) = square.pop() {
                    mul(&mut res, p, 1.0 / 1.1);
                } else {
                    push_text(&mut res, "]");
                }
                i += 1;
            },
            _ => {
                let st = i;
                while i < chars.len() && !matches!(chars[i], '\\' | '(' | ')' | '[' | ']' | ':') {
                    i += 1;
                }
                push_text(&mut res, &chars[st..i].iter().collect::<String>());
            },
        }
    }
    for p in round {
        mul(&mut res, p, 1.1);
    }
    for p in square {
        mul(&mut res, p, 1.0 / 1.1);
    }
    let mut out: Vec<Piece> = Vec::new();
    for (t, w) in res {
        if w < 0.0 {
            out.push(Piece::Break);
            continue;
        }
        if let Some(Piece::Text(pt, pw)) = out.last_mut() {
            if (*pw - w).abs() < 1e-6 {
                pt.push_str(&t);
                continue;
            }
        }
        out.push(Piece::Text(t, w));
    }
    if out.is_empty() {
        out.push(Piece::Text(String::new(), 1.0));
    }
    out
}

fn split_break(t: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut last = 0;
    let b = t.as_bytes();
    let mut i = 0;
    while let Some(off) = t[i..].find("BREAK") {
        let at = i + off;
        let before_ok = at == 0 || !(b[at - 1] as char).is_alphanumeric();
        let after = at + 5;
        let after_ok = after >= b.len() || !(b[after] as char).is_alphanumeric();
        if before_ok && after_ok {
            parts.push(t[last..at].trim_end());
            last = after;
            while last < b.len() && (b[last] as char).is_whitespace() {
                last += 1;
            }
        }
        i = at + 5;
    }
    parts.push(&t[last..]);
    parts
}
