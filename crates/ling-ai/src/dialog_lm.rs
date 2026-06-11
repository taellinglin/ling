//! A miniature neural language model for in-engine dialog generation.
//!
//! This is a **neural probabilistic LM** (Bengio-style): a trainable word
//! embedding table feeds a fixed-width context window into the optimized MLP from
//! [`crate::nn`], which predicts a softmax distribution over the next token.
//! Embeddings *and* MLP are trained end-to-end with cross-entropy SGD — the input
//! gradient returned by [`crate::nn::Net::backward`] flows back into the table.
//!
//! It is deliberately tiny (kilobytes, no external deps, runs on one CPU core) so
//! every NPC can carry its own personality model, trained from a `.lingdialog`
//! corpus ([`crate::dataset`]). It will not write a novel — it captures the
//! cadence and vocabulary of its training dialog and samples fresh lines from it.

use crate::dataset::Dataset;
use crate::nn::{Activation, Net, Rng};
use std::collections::HashMap;

const PAD: u32 = 0;
#[allow(dead_code)] // reserved sentinel; seeded into the vocab as "<bos>"
const BOS: u32 = 1;
const EOS: u32 = 2;
const UNK: u32 = 3;

/// Word ⇄ id mapping with the four reserved special tokens pre-seeded.
struct Vocab {
    to_id: HashMap<String, u32>,
    to_word: Vec<String>,
}

impl Vocab {
    fn new() -> Self {
        let mut v = Vocab { to_id: HashMap::new(), to_word: Vec::new() };
        for s in ["<pad>", "<bos>", "<eos>", "<unk>"] {
            v.intern(s);
        }
        v
    }
    fn len(&self) -> usize {
        self.to_word.len()
    }
    fn intern(&mut self, w: &str) -> u32 {
        if let Some(&id) = self.to_id.get(w) {
            return id;
        }
        let id = self.to_word.len() as u32;
        self.to_word.push(w.to_string());
        self.to_id.insert(w.to_string(), id);
        id
    }
    fn id_of(&self, w: &str) -> u32 {
        self.to_id.get(w).copied().unwrap_or(UNK)
    }
    fn word(&self, id: u32) -> &str {
        self.to_word.get(id as usize).map(|s| s.as_str()).unwrap_or("<unk>")
    }
}

/// Split text into lowercased word/punctuation tokens, preserving `<eos>`-style
/// special markers as single tokens.
fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for chunk in text.split_whitespace() {
        let lc = chunk.to_ascii_lowercase();
        if matches!(lc.as_str(), "<pad>" | "<bos>" | "<eos>" | "<unk>") {
            out.push(lc);
            continue;
        }
        let mut word = String::new();
        for ch in chunk.chars() {
            if ch.is_alphanumeric() || ch == '\'' {
                word.extend(ch.to_lowercase());
            } else {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
                out.push(ch.to_string()); // punctuation as its own token
            }
        }
        if !word.is_empty() {
            out.push(word);
        }
    }
    out
}

/// Configuration of a [`DialogLM`].
#[derive(Clone, Copy, Debug)]
pub struct LmConfig {
    pub ctx: usize,
    pub embed: usize,
    pub hidden: usize,
    pub seed: u64,
}

impl Default for LmConfig {
    fn default() -> Self {
        Self { ctx: 3, embed: 32, hidden: 64, seed: 1 }
    }
}

/// A trainable miniature dialog LM.
pub struct DialogLM {
    cfg: LmConfig,
    vocab: Vocab,
    emb: Vec<f32>,         // vocab.len() * embed (row PAD kept at zero)
    net: Option<Net>,      // input = ctx*embed, output = vocab.len()
    net_vocab: usize,      // vocab size the net was built for
    corpus: Vec<Vec<u32>>, // tokenized training sequences
    rng: Rng,
}

impl DialogLM {
    pub fn new(cfg: LmConfig) -> Self {
        Self {
            vocab: Vocab::new(),
            emb: Vec::new(),
            net: None,
            net_vocab: 0,
            corpus: Vec::new(),
            rng: Rng::new(cfg.seed ^ 0xD1A1_06D1),
            cfg,
        }
    }

    /// Words currently in the vocabulary (including the 4 reserved tokens).
    pub fn vocab_size(&self) -> usize {
        self.vocab.len()
    }

    /// Number of training sequences accumulated.
    pub fn corpus_len(&self) -> usize {
        self.corpus.len()
    }

    /// Add one line/utterance to the training corpus, growing the vocabulary.
    pub fn learn(&mut self, text: &str) {
        let toks = tokenize(text);
        if toks.is_empty() {
            return;
        }
        let mut seq = Vec::with_capacity(toks.len() + 1);
        for t in &toks {
            seq.push(self.vocab.intern(t));
        }
        seq.push(EOS);
        self.corpus.push(seq);
    }

    /// Load a `.lingdialog` corpus, returning the number of training lines added.
    pub fn learn_dataset(&mut self, ds: &Dataset) -> usize {
        let lines = ds.training_lines();
        for line in &lines {
            self.learn(line);
        }
        lines.len()
    }

    /// Ensure embeddings and the MLP match the current vocabulary size. Rebuilds
    /// the net (preserving embeddings) when the vocab has grown since last build.
    fn ensure_built(&mut self) {
        let v = self.vocab.len();
        let e = self.cfg.embed;
        if self.emb.len() < v * e {
            let old = self.emb.len();
            self.emb.resize(v * e, 0.0);
            // Random-init the new rows (row PAD stays zero).
            for row in (old / e)..v {
                if row as u32 == PAD {
                    continue;
                }
                for k in 0..e {
                    self.emb[row * e + k] = self.rng.uniform(0.1);
                }
            }
        }
        if self.net.is_none() || self.net_vocab != v {
            let mut net = Net::new(self.cfg.ctx * e, self.cfg.seed);
            net.dense(self.cfg.hidden, Activation::Relu);
            net.dense(v, Activation::Linear);
            self.net = Some(net);
            self.net_vocab = v;
        }
    }

    /// Concatenate the embeddings of a context window into the net input vector.
    fn gather(&self, ctx_ids: &[u32]) -> Vec<f32> {
        let e = self.cfg.embed;
        let mut x = vec![0.0f32; self.cfg.ctx * e];
        for (p, &id) in ctx_ids.iter().enumerate() {
            let src = id as usize * e;
            if src + e <= self.emb.len() {
                x[p * e..p * e + e].copy_from_slice(&self.emb[src..src + e]);
            }
        }
        x
    }

    /// Train for `epochs` passes at learning rate `lr`. Returns the final-epoch
    /// average cross-entropy loss (lower is better; 0 if there is no data).
    pub fn train(&mut self, epochs: usize, lr: f32) -> f32 {
        self.ensure_built();
        let (ctx, e) = (self.cfg.ctx, self.cfg.embed);

        // Build (context → next-token) examples with left-padding.
        let mut examples: Vec<(Vec<u32>, u32)> = Vec::new();
        for seq in &self.corpus {
            let mut window = vec![PAD; ctx];
            for &tok in seq {
                examples.push((window.clone(), tok));
                window.remove(0);
                window.push(tok);
            }
        }
        if examples.is_empty() {
            return 0.0;
        }

        let mut last_loss = 0.0;
        for _ in 0..epochs.max(1) {
            // Fisher–Yates shuffle for SGD with our seeded RNG.
            for i in (1..examples.len()).rev() {
                let j = (self.rng.next_u64() % (i as u64 + 1)) as usize;
                examples.swap(i, j);
            }

            let mut total = 0.0;
            for (ctx_ids, gold) in &examples {
                let x = self.gather(ctx_ids);
                let net = self.net.as_mut().unwrap();
                let acts = net.forward_cache(&x);
                let logits = acts.last().unwrap();
                let probs = softmax(logits);

                // Cross-entropy loss + gradient (softmax − onehot).
                let g = *gold as usize;
                total += -(probs[g].max(1e-9)).ln();
                let mut d_out = probs;
                d_out[g] -= 1.0;

                let dx = net.backward(&acts, &d_out, lr);
                // Flow the input gradient into the embedding rows (skip PAD).
                for (p, &id) in ctx_ids.iter().enumerate() {
                    if id == PAD {
                        continue;
                    }
                    let base = id as usize * e;
                    for k in 0..e {
                        self.emb[base + k] -= lr * dx[p * e + k];
                    }
                }
            }
            last_loss = total / examples.len() as f32;
        }
        last_loss
    }

    fn logits_for(&self, ctx_ids: &[u32]) -> Vec<f32> {
        let x = self.gather(ctx_ids);
        self.net.as_ref().map(|n| n.forward(&x)).unwrap_or_default()
    }

    /// Generate a reply. `prompt` seeds the context; `max_tokens` caps length;
    /// `temperature` controls randomness (≤0 = greedy/argmax). Stops at `<eos>`.
    pub fn say(&mut self, prompt: &str, max_tokens: usize, temperature: f32) -> String {
        if self.net.is_none() {
            return String::new();
        }
        let ctx = self.cfg.ctx;
        let mut window: Vec<u32> = vec![PAD; ctx];
        for t in tokenize(prompt) {
            window.remove(0);
            window.push(self.vocab.id_of(&t));
        }

        let mut out: Vec<u32> = Vec::new();
        for _ in 0..max_tokens.max(1) {
            let logits = self.logits_for(&window);
            if logits.is_empty() {
                break;
            }
            let next = self.sample(&logits, temperature);
            if next as u32 == EOS {
                break;
            }
            // Skip reserved tokens in the visible output.
            if next as u32 > UNK {
                out.push(next as u32);
            }
            window.remove(0);
            window.push(next as u32);
        }
        self.detokenize(&out)
    }

    /// Sample a token id from logits with temperature + top-k(40) + top-p(0.9).
    fn sample(&mut self, logits: &[f32], temperature: f32) -> usize {
        if temperature <= 0.0 {
            // Greedy.
            return logits
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
        let scaled: Vec<f32> = logits.iter().map(|&l| l / temperature).collect();
        let probs = softmax(&scaled);

        // Rank indices by probability descending.
        let mut order: Vec<usize> = (0..probs.len()).collect();
        order.sort_unstable_by(|&a, &b| probs[b].partial_cmp(&probs[a]).unwrap());

        // top-k then top-p (nucleus): keep the smallest leading set.
        let top_k = 40usize.min(order.len());
        let mut keep = Vec::new();
        let mut cum = 0.0;
        for &idx in order.iter().take(top_k) {
            keep.push(idx);
            cum += probs[idx];
            if cum >= 0.9 {
                break;
            }
        }
        // Zero out everything else and renormalize.
        let total: f32 = keep.iter().map(|&i| probs[i]).sum();
        if total <= 0.0 {
            return order[0];
        }
        let r = self.rng.next_f32() * total;
        let mut acc = 0.0;
        for &idx in &keep {
            acc += probs[idx];
            if r <= acc {
                return idx;
            }
        }
        *keep.last().unwrap_or(&0)
    }

    /// Join word tokens with spaces, attaching punctuation to the previous word.
    fn detokenize(&self, ids: &[u32]) -> String {
        let mut s = String::new();
        for &id in ids {
            let w = self.vocab.word(id);
            let is_punct = w.chars().all(|c| !c.is_alphanumeric());
            if s.is_empty() || is_punct {
                s.push_str(w);
            } else {
                s.push(' ');
                s.push_str(w);
            }
        }
        s
    }

    // ── Serialization ────────────────────────────────────────────────────────

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"LLM1");
        for v in [self.cfg.ctx, self.cfg.embed, self.cfg.hidden] {
            b.extend_from_slice(&(v as u32).to_le_bytes());
        }
        // Vocabulary.
        b.extend_from_slice(&(self.vocab.len() as u32).to_le_bytes());
        for w in &self.vocab.to_word {
            let bytes = w.as_bytes();
            b.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            b.extend_from_slice(bytes);
        }
        // Embeddings.
        b.extend_from_slice(&(self.emb.len() as u32).to_le_bytes());
        for &f in &self.emb {
            b.extend_from_slice(&f.to_le_bytes());
        }
        // Net (0-length block if untrained).
        let net_bytes = self.net.as_ref().map(|n| n.to_bytes()).unwrap_or_default();
        b.extend_from_slice(&(net_bytes.len() as u32).to_le_bytes());
        b.extend_from_slice(&net_bytes);
        b
    }

    pub fn from_bytes(data: &[u8]) -> Option<DialogLM> {
        let mut p = Cur { d: data, i: 0 };
        if p.take(4)? != b"LLM1" {
            return None;
        }
        let cfg = LmConfig {
            ctx: p.u32()? as usize,
            embed: p.u32()? as usize,
            hidden: p.u32()? as usize,
            seed: 1,
        };
        let mut vocab = Vocab { to_id: HashMap::new(), to_word: Vec::new() };
        let n_words = p.u32()? as usize;
        for _ in 0..n_words {
            let len = p.u32()? as usize;
            let w = std::str::from_utf8(p.take(len)?).ok()?.to_string();
            let id = vocab.to_word.len() as u32;
            vocab.to_id.insert(w.clone(), id);
            vocab.to_word.push(w);
        }
        let n_emb = p.u32()? as usize;
        let mut emb = Vec::with_capacity(n_emb);
        for _ in 0..n_emb {
            emb.push(f32::from_le_bytes(p.take(4)?.try_into().ok()?));
        }
        let net_len = p.u32()? as usize;
        let net = if net_len == 0 {
            None
        } else {
            Net::from_bytes(p.take(net_len)?)
        };
        let net_vocab = vocab.len();
        Some(DialogLM {
            cfg,
            vocab,
            emb,
            net,
            net_vocab,
            corpus: Vec::new(),
            rng: Rng::new(cfg.seed),
        })
    }
}

/// Numerically-stable softmax.
fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let mut exp: Vec<f32> = logits.iter().map(|&l| (l - max).exp()).collect();
    let sum: f32 = exp.iter().sum();
    if sum > 0.0 {
        for v in &mut exp {
            *v /= sum;
        }
    }
    exp
}

struct Cur<'a> {
    d: &'a [u8],
    i: usize,
}
impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.d.get(self.i..self.i + n)?;
        self.i += n;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_splits_punctuation_and_specials() {
        let t = tokenize("Hello, world! <eos>");
        assert_eq!(t, vec!["hello", ",", "world", "!", "<eos>"]);
    }

    #[test]
    fn learns_and_generates() {
        let mut lm = DialogLM::new(LmConfig { ctx: 2, embed: 16, hidden: 32, seed: 3 });
        // A tiny, highly-repetitive corpus the model can memorize.
        for _ in 0..6 {
            lm.learn("hello there friend");
            lm.learn("hello there hero");
        }
        let loss = lm.train(300, 0.1);
        assert!(loss < 1.5, "loss did not drop: {loss}");

        // Greedy continuation of "hello there" should be a known word.
        let reply = lm.say("hello there", 3, 0.0);
        assert!(
            reply.contains("friend") || reply.contains("hero"),
            "unexpected reply: {reply:?}"
        );
    }

    #[test]
    fn save_load_preserves_greedy_output() {
        let mut lm = DialogLM::new(LmConfig { ctx: 2, embed: 12, hidden: 24, seed: 5 });
        for _ in 0..5 {
            lm.learn("good morning sunshine");
        }
        lm.train(200, 0.1);
        let before = lm.say("good morning", 2, 0.0);

        let bytes = lm.to_bytes();
        let mut back = DialogLM::from_bytes(&bytes).expect("decode");
        let after = back.say("good morning", 2, 0.0);
        assert_eq!(before, after);
    }
}
