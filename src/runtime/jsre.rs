//! Regular expressions for a browser's page-script sandbox (Horizon's JavaScript reader).
//!
//! `re_compile(pattern, flags)` takes a JavaScript pattern and flags (i, m, s, u, g, y),
//! translates the few spellings that differ from Rust's syntax, and returns an id (-1 when it
//! does not compile; `re_error()` says why). `re_exec(id, text, start)` finds the first match at
//! or after character `start`: [] when there is none, else [start, end, whole, group1, ...] with
//! character (not byte) positions and -1 for a group that did not take part. Lookaround and
//! backreferences work (fancy-regex).

use std::cell::RefCell;

thread_local! {
    static RES: RefCell<Vec<fancy_regex::Regex>> = const { RefCell::new(Vec::new()) };
    static CACHE: RefCell<std::collections::HashMap<String, usize>> = RefCell::new(std::collections::HashMap::new());
    static ERR: RefCell<String> = const { RefCell::new(String::new()) };
}

/// JavaScript pattern text -> Rust (fancy-regex) pattern text.
fn translate(p: &str) -> String {
    let c: Vec<char> = p.chars().collect();
    let mut out = String::with_capacity(p.len() + 8);
    let mut i = 0;
    let mut in_class = false;
    while i < c.len() {
        let ch = c[i];
        if ch == '\\' && i + 1 < c.len() {
            let n = c[i + 1];
            match n {
                // \uXXXX and \u{X...}
                'u' if c.get(i + 2) == Some(&'{') => {
                    let mut j = i + 3;
                    let mut hex = String::new();
                    while j < c.len() && c[j] != '}' {
                        hex.push(c[j]);
                        j += 1;
                    }
                    out.push_str(&format!("\\x{{{hex}}}"));
                    i = j + 1;
                    continue;
                },
                'u' if i + 6 <= c.len() && c[i + 2..i + 6].iter().all(|h| h.is_ascii_hexdigit()) => {
                    let hex: String = c[i + 2..i + 6].iter().collect();
                    out.push_str(&format!("\\x{{{hex}}}"));
                    i += 6;
                    continue;
                },
                // \/ is just a slash; \- \_ \# and other identity escapes keep their character
                '/' => out.push('/'),
                'c' => out.push_str("\\c"),
                'd' | 'D' | 'w' | 'W' | 's' | 'S' | 'b' | 'B' | 'n' | 'r' | 't' | 'f' | 'v' | 'x' | 'p' | 'P' | '0'..='9' | 'k' => {
                    if n == 'v' {
                        out.push_str("\\x0B");
                    } else if n == 'k' && c.get(i + 2) == Some(&'<') {
                        // \k<name> -> (?P=name)
                        let mut j = i + 3;
                        let mut name = String::new();
                        while j < c.len() && c[j] != '>' {
                            name.push(c[j]);
                            j += 1;
                        }
                        out.push_str(&format!("(?P={name})"));
                        i = j + 1;
                        continue;
                    } else {
                        out.push('\\');
                        out.push(n);
                    }
                },
                _ if n.is_ascii_alphanumeric() => out.push(n),
                _ => {
                    out.push('\\');
                    out.push(n);
                },
            }
            i += 2;
            continue;
        }
        if in_class {
            if ch == ']' {
                in_class = false;
            }
            // a bare [ inside a class is literal in JavaScript
            if ch == '[' {
                out.push_str("\\[");
                i += 1;
                continue;
            }
            out.push(ch);
            i += 1;
            continue;
        }
        if ch == '[' {
            // [^] matches anything, [] nothing
            if c.get(i + 1) == Some(&'^') && c.get(i + 2) == Some(&']') {
                out.push_str("[\\s\\S]");
                i += 3;
                continue;
            }
            if c.get(i + 1) == Some(&']') {
                out.push_str("[^\\s\\S]");
                i += 2;
                continue;
            }
            in_class = true;
            out.push('[');
            // a ] right after [ or [^ is literal in Rust; in JavaScript it ends the class (handled above)
            i += 1;
            continue;
        }
        // (?<name> stays; fancy-regex reads it
        out.push(ch);
        i += 1;
    }
    out
}

pub fn compile(pattern: &str, flags: &str) -> f64 {
    let key = format!("{flags}\u{1}{pattern}");
    if let Some(id) = CACHE.with(|c| c.borrow().get(&key).copied()) {
        return id as f64;
    }
    let mut pre = String::new();
    for f in ['i', 'm', 's'] {
        if flags.contains(f) {
            pre.push(f);
        }
    }
    let body = translate(pattern);
    let full = if pre.is_empty() { body } else { format!("(?{pre}){body}") };
    match fancy_regex::RegexBuilder::new(&full).backtrack_limit(200_000).build() {
        Ok(re) => {
            let id = RES.with(|r| {
                let mut r = r.borrow_mut();
                r.push(re);
                r.len() - 1
            });
            CACHE.with(|c| c.borrow_mut().insert(key, id));
            id as f64
        },
        Err(e) => {
            ERR.with(|x| *x.borrow_mut() = e.to_string());
            -1.0
        },
    }
}

pub fn error() -> String {
    ERR.with(|x| x.borrow().clone())
}

/// [start, end, whole, groups...] in characters, or [] (see the module comment).
pub fn exec(id: usize, text: &str, start_chars: usize) -> Option<Vec<crate::runtime::Value>> {
    use crate::runtime::Value;
    RES.with(|r| {
        let r = r.borrow();
        let re = r.get(id)?;
        // character position -> byte position
        let start_b = text.char_indices().nth(start_chars).map(|(b, _)| b).unwrap_or(text.len());
        if start_chars > 0 && start_b >= text.len() && start_chars > text.chars().count() {
            return Some(Vec::new());
        }
        let caps = match re.captures_from_pos(text, start_b) {
            Ok(Some(c)) => c,
            _ => return Some(Vec::new()),
        };
        let m0 = caps.get(0)?;
        let to_chars = |b: usize| text[..b].chars().count() as f64;
        let mut out = vec![Value::Number(to_chars(m0.start())), Value::Number(to_chars(m0.end()))];
        for g in 0..caps.len() {
            match caps.get(g) {
                Some(m) => out.push(Value::Str(m.as_str().to_string())),
                None => out.push(Value::Number(-1.0)),
            }
        }
        Some(out)
    })
}

/// the names of a pattern's groups, in order ("" for unnamed ones)
pub fn names(id: usize) -> Vec<String> {
    RES.with(|r| {
        let r = r.borrow();
        match r.get(id) {
            Some(re) => re.capture_names().map(|n| n.unwrap_or("").to_string()).collect(),
            None => Vec::new(),
        }
    })
}
