//! CLIP byte-level BPE tokenizer (the one OpenAI's CLIP and every SD text encoder uses),
//! read from the stock `vocab.json` + `merges.txt` pair. Same algorithm as
//! `clip/simple_tokenizer.py`: lower-case, collapse whitespace, split into words /
//! single digits / punctuation runs, byte-encode, then BPE with `</w>` on the last symbol.

use std::collections::HashMap;
use std::path::Path;

pub const BOS: u32 = 49406;
pub const EOS: u32 = 49407;

pub struct ClipTokenizer {
    encoder: HashMap<String, u32>,
    ranks: HashMap<(String, String), usize>,
    byte_enc: [char; 256],
    cache: std::sync::Mutex<HashMap<String, Vec<u32>>>,
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

impl ClipTokenizer {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let vocab = std::fs::read_to_string(dir.join("vocab.json")).map_err(|e| format!("CLIP vocab.json in {}: {e}", dir.display()))?;
        let merges = std::fs::read_to_string(dir.join("merges.txt")).map_err(|e| format!("CLIP merges.txt in {}: {e}", dir.display()))?;
        let v: serde_json::Value = serde_json::from_str(&vocab).map_err(|e| format!("vocab.json: {e}"))?;
        let mut encoder = HashMap::new();
        if let Some(obj) = v.as_object() {
            for (k, id) in obj {
                if let Some(id) = id.as_u64() {
                    encoder.insert(k.clone(), id as u32);
                }
            }
        }
        if encoder.len() < 49000 {
            return Err("vocab.json does not look like the CLIP vocabulary".into());
        }
        let mut ranks = HashMap::new();
        for (i, line) in merges.lines().filter(|l| !l.starts_with("#version")).enumerate() {
            let mut it = line.split(' ');
            if let (Some(a), Some(b)) = (it.next(), it.next()) {
                ranks.insert((a.to_string(), b.to_string()), i);
            }
        }
        Ok(Self { encoder, ranks, byte_enc: bytes_to_unicode(), cache: std::sync::Mutex::new(HashMap::new()) })
    }

    fn bpe(&self, token: &str) -> Vec<u32> {
        if let Some(hit) = self.cache.lock().ok().and_then(|c| c.get(token).cloned()) {
            return hit;
        }
        let chars: Vec<char> = token.chars().collect();
        if chars.is_empty() {
            return Vec::new();
        }
        let mut word: Vec<String> = chars.iter().map(|c| c.to_string()).collect();
        let last = word.len() - 1;
        word[last].push_str("</w>");
        loop {
            if word.len() < 2 {
                break;
            }
            let mut best: Option<(usize, usize)> = None; // (rank, index)
            for i in 0..word.len() - 1 {
                if let Some(&r) = self.ranks.get(&(word[i].clone(), word[i + 1].clone())) {
                    if best.map_or(true, |(br, _)| r < br) {
                        best = Some((r, i));
                    }
                }
            }
            let Some((_, _)) = best else { break };
            let (first, second) = {
                let (_, i) = best.unwrap();
                (word[i].clone(), word[i + 1].clone())
            };
            // merge every occurrence of the best pair, left to right
            let mut merged = Vec::with_capacity(word.len());
            let mut i = 0;
            while i < word.len() {
                if i + 1 < word.len() && word[i] == first && word[i + 1] == second {
                    merged.push(format!("{first}{second}"));
                    i += 2;
                } else {
                    merged.push(word[i].clone());
                    i += 1;
                }
            }
            word = merged;
        }
        let ids: Vec<u32> = word.iter().filter_map(|w| self.encoder.get(w).copied()).collect();
        if let Ok(mut c) = self.cache.lock() {
            if c.len() < 20000 {
                c.insert(token.to_string(), ids.clone());
            }
        }
        ids
    }

    /// Token ids of `text` with no BOS / EOS / padding.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let clean: String = text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
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
            let hit = ["re", "ve", "ll"].iter().find(|p| rest.starts_with(*p)).map(|p| p.len()).or_else(|| {
                rest.chars().next().filter(|c| matches!(c, 's' | 't' | 'm' | 'd')).map(|_| 1)
            });
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
