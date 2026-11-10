//! CLIP's byte-level BPE tokenizer, from the stock `vocab.json` + `merges.txt`. The same algorithm as OpenAI's
//! `simple_tokenizer.py` (and the host engine's `tokenizer.rs`): lower-case, collapse whitespace, split into words / single digits /
//! punctuation runs, byte-encode, then BPE with `</w>` on the last symbol.
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const BOS: u32 = 49406;
pub const EOS: u32 = 49407;

pub struct Tokenizer {
    encoder: BTreeMap<String, u32>,
    ranks: BTreeMap<(String, String), u32>,
    byte_enc: [char; 256],
}

fn bytes_to_unicode() -> [char; 256] {
    let mut bs: Vec<u32> = Vec::new();
    bs.extend(b'!' as u32..=b'~' as u32);
    bs.extend(0xA1u32..=0xAC);
    bs.extend(0xAEu32..=0xFF);
    let mut cs = bs.clone();
    let mut n = 0u32;
    for b in 0u32..256 {
        if !bs.contains(&b) {
            bs.push(b);
            cs.push(256 + n);
            n += 1;
        }
    }
    let mut out = ['\0'; 256];
    for (b, c) in bs.iter().zip(cs.iter()) {
        out[*b as usize] = char::from_u32(*c).unwrap_or('?');
    }
    out
}

/// `{"token": id, ...}` -- strings with JSON escapes, integer values.
fn parse_vocab(s: &[u8]) -> Result<BTreeMap<String, u32>, crate::Error> {
    let mut m = BTreeMap::new();
    let mut i = 0;
    let n = s.len();
    while i < n && s[i] != b'{' {
        i += 1;
    }
    i += 1;
    loop {
        while i < n && s[i] != b'"' && s[i] != b'}' {
            i += 1;
        }
        if i >= n || s[i] == b'}' {
            break;
        }
        i += 1;
        let mut key: Vec<u8> = Vec::new();
        while i < n && s[i] != b'"' {
            if s[i] == b'\\' && i + 1 < n {
                i += 1;
                match s[i] {
                    b'n' => key.push(b'\n'),
                    b't' => key.push(b'\t'),
                    b'r' => key.push(b'\r'),
                    b'u' if i + 4 < n => {
                        let h = core::str::from_utf8(&s[i + 1..i + 5]).unwrap_or("003f");
                        let cp = u32::from_str_radix(h, 16).unwrap_or(0x3f);
                        let ch = char::from_u32(cp).unwrap_or('?');
                        let mut b = [0u8; 4];
                        key.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
                        i += 4;
                    },
                    c => key.push(c),
                }
            } else {
                key.push(s[i]);
            }
            i += 1;
        }
        i += 1;
        while i < n && !s[i].is_ascii_digit() {
            i += 1;
        }
        let mut v = 0u32;
        while i < n && s[i].is_ascii_digit() {
            v = v * 10 + (s[i] - b'0') as u32;
            i += 1;
        }
        m.insert(String::from_utf8_lossy(&key).into_owned(), v);
    }
    if m.len() < 49000 {
        return Err("vocab.json does not look like the CLIP vocabulary".to_string());
    }
    Ok(m)
}

impl Tokenizer {
    pub fn new(vocab_json: &[u8], merges_txt: &[u8]) -> Result<Tokenizer, crate::Error> {
        let encoder = parse_vocab(vocab_json)?;
        let merges = core::str::from_utf8(merges_txt).map_err(|_| "merges.txt is not text".to_string())?;
        let mut ranks = BTreeMap::new();
        for (i, line) in merges.lines().filter(|l| !l.starts_with("#version")).enumerate() {
            let mut it = line.split(' ');
            if let (Some(a), Some(b)) = (it.next(), it.next()) {
                ranks.insert((a.to_string(), b.to_string()), i as u32);
            }
        }
        if ranks.len() < 40000 {
            return Err("merges.txt does not look like CLIP's merges".to_string());
        }
        Ok(Tokenizer { encoder, ranks, byte_enc: bytes_to_unicode() })
    }

    fn bpe(&self, token: &str) -> Vec<u32> {
        let chars: Vec<char> = token.chars().collect();
        if chars.is_empty() {
            return Vec::new();
        }
        let mut word: Vec<String> = chars.iter().map(|c| c.to_string()).collect();
        let last = word.len() - 1;
        word[last].push_str("</w>");
        let mut key = (String::new(), String::new());
        while word.len() >= 2 {
            let mut best: Option<(u32, usize)> = None;
            for i in 0..word.len() - 1 {
                key.0.clear();
                key.0.push_str(&word[i]);
                key.1.clear();
                key.1.push_str(&word[i + 1]);
                if let Some(&r) = self.ranks.get(&key) {
                    if best.map_or(true, |(br, _)| r < br) {
                        best = Some((r, i));
                    }
                }
            }
            let Some((_, bi)) = best else { break };
            let (first, second) = (word[bi].clone(), word[bi + 1].clone());
            let mut merged = Vec::with_capacity(word.len());
            let mut i = 0;
            while i < word.len() {
                if i + 1 < word.len() && word[i] == first && word[i + 1] == second {
                    let mut m = first.clone();
                    m.push_str(&second);
                    merged.push(m);
                    i += 2;
                } else {
                    merged.push(word[i].clone());
                    i += 1;
                }
            }
            word = merged;
        }
        word.iter().filter_map(|w| self.encoder.get(w).copied()).collect()
    }

    /// Token ids of `text` with no BOS / EOS / padding.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let clean = text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        let mut out = Vec::new();
        for piece in pre_tokenize(&clean) {
            let mapped: String = piece.bytes().map(|b| self.byte_enc[b as usize]).collect();
            out.extend(self.bpe(&mapped));
        }
        out
    }
}

/// The CLIP regex, by hand:  's|'t|'re|'ve|'m|'ll|'d | letters+ | one digit | [^\s letters digits]+
fn pre_tokenize(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '\'' {
            let rest: String = chars[i + 1..chars.len().min(i + 3)].iter().collect();
            let hit = ["re", "ve", "ll"]
                .iter()
                .find(|p| rest.starts_with(*p))
                .map(|p| p.len())
                .or_else(|| rest.chars().next().filter(|c| matches!(c, 's' | 't' | 'm' | 'd')).map(|_| 1));
            if let Some(n) = hit {
                out.push(chars[i..i + 1 + n].iter().collect());
                i += 1 + n;
                continue;
            }
        }
        if c.is_alphabetic() {
            let st = i;
            while i < chars.len() && chars[i].is_alphabetic() {
                i += 1;
            }
            out.push(chars[st..i].iter().collect());
        } else if c.is_numeric() {
            out.push(c.to_string());
            i += 1;
        } else {
            let st = i;
            while i < chars.len() && !chars[i].is_whitespace() && !chars[i].is_alphabetic() && !chars[i].is_numeric() {
                i += 1;
            }
            out.push(chars[st..i].iter().collect());
        }
    }
    out
}
