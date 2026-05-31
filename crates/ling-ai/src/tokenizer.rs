//! Byte-pair encoding tokenizer (minimal, no external deps).

use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct Tokenizer {
    vocab:       HashMap<String, u32>,
    id_to_token: Vec<String>,
}

impl Tokenizer {
    pub fn new() -> Self { Self::default() }

    /// Build from an iterator of `(token, id)` pairs.
    pub fn from_vocab(pairs: impl IntoIterator<Item = (String, u32)>) -> Self {
        let mut t = Self::new();
        for (token, id) in pairs {
            let id = id as usize;
            if id >= t.id_to_token.len() {
                t.id_to_token.resize(id + 1, String::new());
            }
            t.id_to_token[id] = token.clone();
            t.vocab.insert(token, id as u32);
        }
        t
    }

    pub fn vocab_size(&self) -> usize { self.id_to_token.len() }

    pub fn token_to_id(&self, token: &str) -> Option<u32> {
        self.vocab.get(token).copied()
    }

    pub fn id_to_token(&self, id: u32) -> Option<&str> {
        self.id_to_token.get(id as usize).map(|s| s.as_str())
    }

    /// Naive character-level encoding (production code uses BPE merge rules).
    pub fn encode(&self, text: &str) -> Vec<u32> {
        text.chars()
            .map(|c| {
                let s = c.to_string();
                self.vocab.get(&s).copied().unwrap_or(0)
            })
            .collect()
    }

    pub fn decode(&self, ids: &[u32]) -> String {
        ids.iter()
            .filter_map(|&id| self.id_to_token(id))
            .collect()
    }
}
