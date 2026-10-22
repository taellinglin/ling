//! Byte-level BPE built from the GGUF file's own vocabulary (`tokenizer.ggml.tokens`), token types and merge list, so the 11 MB
//! tokenizer.json is not needed. Text is split into pieces the way Qwen's pre-tokenizer does (the regular expression is written out as a
//! scanner: there is no regex engine in `no_std`), each piece's bytes are mapped to the GPT-2 printable characters, then merged by rank.
use crate::error::Error;
use crate::gguf::{Gguf, Value};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub struct Tokenizer {
    tokens: Vec<String>,
    types: Vec<i32>,
    ids: BTreeMap<String, u32>,
    ranks: BTreeMap<String, u32>,
    byte_char: [char; 256],
    char_byte: BTreeMap<char, u8>,
    /// control / user-defined tokens, longest first: matched in the text before anything else
    specials: Vec<(String, u32)>,
    pub eos: Vec<u32>,
}

/// GPT-2's byte to printable character map
fn byte_tables() -> ([char; 256], BTreeMap<char, u8>) {
    let mut keep = [false; 256];
    for b in (33..=126).chain(161..=172).chain(174..=255) {
        keep[b] = true;
    }
    let mut bc = ['\0'; 256];
    let mut cb = BTreeMap::new();
    let mut n = 0u32;
    for b in 0..256usize {
        let c = if keep[b] {
            char::from_u32(b as u32).unwrap()
        } else {
            n += 1;
            char::from_u32(256 + n - 1).unwrap()
        };
        bc[b] = c;
        cb.insert(c, b as u8);
    }
    (bc, cb)
}

impl Tokenizer {
    pub fn from_gguf(g: &Gguf) -> Result<Tokenizer, Error> {
        let Some(Value::Arr(toks)) = g.get("tokenizer.ggml.tokens") else { return Err(Error::Missing("tokenizer.ggml.tokens".into())) };
        let tokens: Vec<String> = toks.iter().map(|v| v.as_str().unwrap_or("").into()).collect();
        let types: Vec<i32> = match g.get("tokenizer.ggml.token_type") {
            Some(Value::Arr(a)) => a.iter().map(|v| match v {
                Value::I32(x) => *x,
                Value::U32(x) => *x as i32,
                _ => 1,
            }).collect(),
            _ => vec![1; tokens.len()],
        };
        let mut ids = BTreeMap::new();
        for (i, t) in tokens.iter().enumerate() {
            ids.entry(t.clone()).or_insert(i as u32);
        }
        let mut ranks = BTreeMap::new();
        if let Some(Value::Arr(ms)) = g.get("tokenizer.ggml.merges") {
            for (r, m) in ms.iter().enumerate() {
                if let Some(s) = m.as_str() {
                    ranks.insert(s.into(), r as u32);
                }
            }
        } else {
            return Err(Error::Missing("tokenizer.ggml.merges".into()));
        }
        let (byte_char, char_byte) = byte_tables();
        let mut specials: Vec<(String, u32)> = tokens
            .iter()
            .enumerate()
            .filter(|(i, t)| matches!(types.get(*i), Some(3) | Some(4)) && t.len() > 2 && t.starts_with('<') && t.ends_with('>'))
            .map(|(i, t)| (t.clone(), i as u32))
            .collect();
        specials.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        let mut eos = Vec::new();
        for name in ["<|im_end|>", "<|endoftext|>"] {
            if let Some(id) = ids.get(name) {
                eos.push(*id);
            }
        }
        Ok(Tokenizer { tokens, types, ids, ranks, byte_char, char_byte, specials, eos })
    }

    pub fn vocab(&self) -> usize {
        self.tokens.len()
    }
    pub fn id_of(&self, token: &str) -> Option<u32> {
        self.ids.get(token).copied()
    }
    pub fn is_special(&self, id: u32) -> bool {
        matches!(self.types.get(id as usize), Some(3) | Some(4))
    }

    /// text to token ids. Special tokens such as `<|im_start|>` written in the text become their ids.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut out = Vec::new();
        let bytes = text.as_bytes();
        let (mut i, mut seg) = (0, 0);
        while i < text.len() {
            if bytes[i] == b'<' {
                if let Some((s, id)) = self.specials.iter().find(|(s, _)| text[i..].starts_with(s.as_str())) {
                    self.encode_plain(&text[seg..i], &mut out);
                    out.push(*id);
                    i += s.len();
                    seg = i;
                    continue;
                }
            }
            i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
        }
        self.encode_plain(&text[seg..], &mut out);
        out
    }

    fn encode_plain(&self, text: &str, out: &mut Vec<u32>) {
        for (a, b) in pieces(text) {
            self.bpe(&text[a..b], out);
        }
    }

    /// one pre-tokenized piece: its bytes as printable characters, merged by rank
    fn bpe(&self, piece: &str, out: &mut Vec<u32>) {
        let mut syms: Vec<String> = piece.bytes().map(|b| String::from(self.byte_char[b as usize])).collect();
        loop {
            let mut best: Option<(u32, usize)> = None;
            let mut key = String::new();
            for k in 0..syms.len().saturating_sub(1) {
                key.clear();
                key.push_str(&syms[k]);
                key.push(' ');
                key.push_str(&syms[k + 1]);
                if let Some(r) = self.ranks.get(&key) {
                    if best.map_or(true, |(br, _)| *r < br) {
                        best = Some((*r, k));
                    }
                }
            }
            let Some((_, k)) = best else { break };
            let right = syms.remove(k + 1);
            syms[k].push_str(&right);
        }
        for s in syms {
            match self.ids.get(&s) {
                Some(id) => out.push(*id),
                // a symbol that is not a token (a vocabulary without every byte): fall back to its characters
                None => {
                    for c in s.chars() {
                        if let Some(id) = self.ids.get(&String::from(c)) {
                            out.push(*id);
                        }
                    }
                }
            }
        }
    }

    /// the bytes a token stands for
    pub fn token_bytes(&self, id: u32) -> Vec<u8> {
        let Some(t) = self.tokens.get(id as usize) else { return Vec::new() };
        if self.is_special(id) {
            return t.as_bytes().to_vec();
        }
        let mut v = Vec::new();
        for c in t.chars() {
            match self.char_byte.get(&c) {
                Some(b) => v.push(*b),
                None => {
                    let mut buf = [0u8; 4];
                    v.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
            }
        }
        v
    }

    pub fn decode(&self, ids: &[u32]) -> String {
        let mut bytes = Vec::new();
        for id in ids {
            bytes.extend(self.token_bytes(*id));
        }
        String::from_utf8_lossy(&bytes).into()
    }
}

/// Turns a stream of tokens into text without ever splitting a UTF-8 character: bytes of an unfinished character wait for the next token.
#[derive(Default)]
pub struct Detok {
    pending: Vec<u8>,
}

impl Detok {
    pub fn new() -> Detok {
        Detok { pending: Vec::new() }
    }
    pub fn feed(&mut self, tok: &Tokenizer, id: u32) -> String {
        self.pending.extend(tok.token_bytes(id));
        match core::str::from_utf8(&self.pending) {
            Ok(s) => {
                let out = String::from(s);
                self.pending.clear();
                out
            }
            Err(e) => {
                let good = e.valid_up_to();
                if e.error_len().is_some() {
                    // not merely unfinished: bytes that can never be valid
                    let out = String::from_utf8_lossy(&self.pending).into();
                    self.pending.clear();
                    out
                } else {
                    let out = String::from(core::str::from_utf8(&self.pending[..good]).unwrap_or(""));
                    self.pending.drain(..good);
                    out
                }
            }
        }
    }
}

// ── the pre-tokenizer ───────────────────────────────────────────────────────────
#[inline]
fn is_l(c: char) -> bool {
    c.is_alphabetic()
}
#[inline]
fn is_n(c: char) -> bool {
    c.is_numeric()
}
#[inline]
fn is_nl(c: char) -> bool {
    c == '\r' || c == '\n'
}

/// Qwen's split, as byte ranges: the alternatives of
/// `(?i:'s|'t|'re|'ve|'m|'ll|'d)|[^\r\n\p{L}\p{N}]?\p{L}+|\p{N}| ?[^\s\p{L}\p{N}]+[\r\n]*|\s*[\r\n]+|\s+(?!\S)|\s+`, tried in that order.
pub fn pieces(text: &str) -> Vec<(usize, usize)> {
    let ch: Vec<(usize, char)> = text.char_indices().collect();
    let n = ch.len();
    let end = |i: usize| if i < n { ch[i].0 } else { text.len() };
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let c = ch[i].1;
        // 'ss 't 're 've 'm 'll 'd, any case
        if c == '\'' && i + 1 < n {
            let d = ch[i + 1].1.to_ascii_lowercase();
            let mut len = 0;
            if matches!(d, 's' | 't' | 'm' | 'd') {
                len = 2;
            } else if i + 2 < n {
                let e = ch[i + 2].1.to_ascii_lowercase();
                if (d == 'r' && e == 'e') || (d == 'v' && e == 'e') || (d == 'l' && e == 'l') {
                    len = 3;
                }
            }
            if len > 0 {
                out.push((end(i), end(i + len)));
                i += len;
                continue;
            }
        }
        // one optional non-letter, non-number, non-newline character, then letters
        let mut j = i;
        if !is_nl(c) && !is_l(c) && !is_n(c) {
            j = i + 1;
        }
        if j < n && is_l(ch[j].1) {
            while j < n && is_l(ch[j].1) {
                j += 1;
            }
            out.push((end(i), end(j)));
            i = j;
            continue;
        }
        // a single digit
        if is_n(c) {
            out.push((end(i), end(i + 1)));
            i += 1;
            continue;
        }
        // an optional space, then symbols (not space, letter or number), then newlines
        let mut j = i;
        if c == ' ' {
            j = i + 1;
        }
        if j < n && !ch[j].1.is_whitespace() && !is_l(ch[j].1) && !is_n(ch[j].1) {
            while j < n && !ch[j].1.is_whitespace() && !is_l(ch[j].1) && !is_n(ch[j].1) {
                j += 1;
            }
            while j < n && is_nl(ch[j].1) {
                j += 1;
            }
            out.push((end(i), end(j)));
            i = j;
            continue;
        }
        if c.is_whitespace() {
            let mut k = i;
            while k < n && ch[k].1.is_whitespace() {
                k += 1;
            }
            // whitespace up to and including its last newline
            if let Some(m) = (i..k).rev().find(|m| is_nl(ch[*m].1)) {
                out.push((end(i), end(m + 1)));
                i = m + 1;
                continue;
            }
            // whitespace that is not followed by a non-space keeps all of it; before a word the last space belongs to the word
            if k >= n {
                out.push((end(i), end(k)));
                i = k;
                continue;
            }
            if k - i > 1 {
                out.push((end(i), end(k - 1)));
                i = k - 1;
                continue;
            }
            out.push((end(i), end(k)));
            i = k;
            continue;
        }
        out.push((end(i), end(i + 1)));
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(s: &str) -> Vec<&str> {
        pieces(s).into_iter().map(|(a, b)| &s[a..b]).collect()
    }

    #[test]
    fn the_pre_tokenizer_splits_like_qwen() {
        assert_eq!(split("Hello world"), ["Hello", " world"]);
        assert_eq!(split("don't stop"), ["don", "'t", " stop"]);
        assert_eq!(split("I'LL go"), ["I", "'LL", " go"]);
        assert_eq!(split("a1234b"), ["a", "1", "2", "3", "4", "b"]);
        assert_eq!(split("x = (y+1);"), ["x", " =", " (", "y", "+", "1", ");"]);
        assert_eq!(split("line one\n\nline two"), ["line", " one", "\n\n", "line", " two"]);
        assert_eq!(split("a   b"), ["a", "  ", " b"]);
        assert_eq!(split("end   "), ["end", "   "]);
        assert_eq!(split("tab\t\tx"), ["tab", "\t", "\tx"]);
        assert_eq!(split("naïve café"), ["naïve", " café"]);
        // one optional non-letter may lead a run of letters, so the comma belongs to the word after it
        assert_eq!(split("你好,世界!"), ["你好", ",世界", "!"]);
        // every byte of the text is covered exactly once
        for s in ["Hello  \n world 'x 12  ", "  ", "\n", "a\r\nb", "(((", "ß'é"] {
            let joined: String = split(s).concat();
            assert_eq!(joined, s);
        }
    }

    #[test]
    fn byte_characters_follow_gpt2() {
        let (bc, cb) = byte_tables();
        assert_eq!(bc[b'a' as usize], 'a');
        assert_eq!(bc[b' ' as usize], '\u{120}'); // the famous "G with a dot"
        assert_eq!(bc[b'\n' as usize], '\u{10a}');
        assert_eq!(cb.len(), 256);
        for b in 0..=255u8 {
            assert_eq!(cb[&bc[b as usize]], b);
        }
    }
}
