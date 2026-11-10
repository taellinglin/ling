//! AUTOMATIC1111 prompt syntax:
//!   (text)  x1.1     ((text))  x1.21     [text]  /1.1     (text:1.4)  x1.4
//!   \( \) \[ \]  literal brackets        BREAK  starts a new 75-token chunk
//!   <lora:name:0.8>  extra-network tag (pulled out of the text, applied to the model)
//! Port of modules/prompt_parser.py::parse_prompt_attention, so prompts written for the
//! WebUI weigh the same here.

#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Text(String, f32),
    Break,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoraTag {
    pub name: String,
    pub weight: f32,
}

/// Removes `<lora:name[:w]>` / `<lyco:...>` tags, returning the cleaned prompt and the tags.
/// Other `<...>` extra-network tags (hypernet etc.) are dropped with a note.
pub fn extract_networks(prompt: &str) -> (String, Vec<LoraTag>, Vec<String>) {
    let mut out = String::new();
    let mut loras = Vec::new();
    let mut ignored = Vec::new();
    let mut rest = prompt;
    while let Some(st) = rest.find('<') {
        let Some(len) = rest[st..].find('>') else { break };
        let inner = &rest[st + 1..st + len];
        let mut parts = inner.split(':');
        let kind = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        if kind == "lora" || kind == "lyco" {
            let name = parts.next().unwrap_or("").trim().to_string();
            let weight = parts.next().and_then(|w| w.trim().parse::<f32>().ok()).unwrap_or(1.0);
            if !name.is_empty() {
                loras.push(LoraTag { name, weight });
            }
            out.push_str(&rest[..st]);
        } else if kind == "hypernet" {
            ignored.push(inner.to_string());
            out.push_str(&rest[..st]);
        } else {
            out.push_str(&rest[..st + len + 1]);
        }
        rest = &rest[st + len + 1..];
    }
    out.push_str(rest);
    (out, loras, ignored)
}

pub fn parse_attention(text: &str) -> Vec<Piece> {
    // (text, weight) with weight < 0 meaning BREAK
    let mut res: Vec<(String, f32)> = Vec::new();
    let mut round: Vec<usize> = Vec::new();
    let mut square: Vec<usize> = Vec::new();
    let mul = |res: &mut Vec<(String, f32)>, from: usize, m: f32| {
        for r in res.iter_mut().skip(from) {
            if r.1 >= 0.0 {
                r.1 *= m;
            }
        }
    };
    let push_text = |res: &mut Vec<(String, f32)>, t: &str| {
        // BREAK splits on its own word
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
    };
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            if i + 1 < chars.len() && matches!(chars[i + 1], '(' | ')' | '[' | ']' | '\\') {
                res.push((chars[i + 1].to_string(), 1.0));
                i += 2;
            } else {
                res.push(("\\".into(), 1.0));
                i += 1;
            }
            continue;
        }
        match c {
            '(' => {
                round.push(res.len());
                i += 1;
            }
            '[' => {
                square.push(res.len());
                i += 1;
            }
            ':' => {
                // ":\s*([+-]?[.\d]+)\s*\)"
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
            }
            ')' => {
                if let Some(p) = round.pop() {
                    mul(&mut res, p, 1.1);
                } else {
                    push_text(&mut res, ")");
                }
                i += 1;
            }
            ']' => {
                if let Some(p) = square.pop() {
                    mul(&mut res, p, 1.0 / 1.1);
                } else {
                    push_text(&mut res, "]");
                }
                i += 1;
            }
            _ => {
                let st = i;
                while i < chars.len() && !matches!(chars[i], '\\' | '(' | ')' | '[' | ']' | ':') {
                    i += 1;
                }
                push_text(&mut res, &chars[st..i].iter().collect::<String>());
            }
        }
    }
    for p in round {
        mul(&mut res, p, 1.1);
    }
    for p in square {
        mul(&mut res, p, 1.0 / 1.1);
    }
    // merge runs of equal weight
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
    // "BREAK" as a whole word (A1111: re.split(r"\s*\bBREAK\b\s*", text))
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn weights() {
        let p = parse_attention("a (b:1.5) [c] ((d)) \\(e\\)");
        assert_eq!(p[0], Piece::Text("a ".into(), 1.0));
        assert_eq!(p[1], Piece::Text("b".into(), 1.5));
        match &p[3] {
            Piece::Text(t, w) => {
                assert_eq!(t, "c");
                assert!((w - 1.0 / 1.1).abs() < 1e-5)
            }
            _ => panic!(),
        }
        let p = parse_attention("x BREAK y");
        assert!(p.contains(&Piece::Break));
        let (s, l, _) = extract_networks("girl <lora:abc:0.7>, smile");
        assert_eq!(s, "girl , smile");
        assert_eq!(l[0], LoraTag { name: "abc".into(), weight: 0.7 });
    }
}
