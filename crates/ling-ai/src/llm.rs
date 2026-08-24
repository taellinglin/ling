//! In-process chat-completion model (Milestone 2 of chat.ling-lang.org).
//!
//! `LoadedModel` loads a GGUF checkpoint via candle-transformers' quantized
//! model implementations (CUDA-capable) and a HF `tokenizers` vocab, then
//! runs autoregressive generation synchronously via `generate()` — same
//! shape as [`crate::nn::Net`]/[`crate::dialog_lm::DialogLM`]: a plain type
//! with instance methods, no global state, no threading. The `ling` binary's
//! `src/runtime/llm.rs` owns the handle table and background-thread/polling
//! glue, exactly mirroring how `src/runtime/ai.rs` wraps `Net`/`DialogLM`.
//!
//! `generate()`'s `on_token` callback fires once per generated piece so the
//! caller can publish incremental progress (e.g. into a poll buffer) without
//! this crate knowing anything about jobs, threads, or HTTP.

use candle_core::quantized::gguf_file;
use candle_core::{DType, Device, Tensor};
use candle_transformers::generation::{LogitsProcessor, Sampling};
use candle_transformers::models::quantized_glm4::ModelWeights;
use std::sync::Mutex;
use tokenizers::Tokenizer;

pub struct LoadedModel {
    weights: Mutex<ModelWeights>,
    tokenizer: Tokenizer,
    device: Device,
    eos_tokens: Vec<u32>,
}

/// Streaming token-to-text decoder. Decoding one isolated token at a time
/// (`tokenizer.decode(&[tok], ..)`) silently produces "" for most sub-word
/// pieces — sub-word/byte-fallback tokens only resolve to visible text once
/// merged with their neighbors. The fix (same one candle's own examples use,
/// `candle-examples/src/token_output_stream.rs`) is to re-decode a growing
/// token window and diff against the previous decode, emitting only the new
/// suffix once it stabilizes on a full character/word boundary.
struct TokenOutputStream<'a> {
    tokenizer: &'a Tokenizer,
    tokens: Vec<u32>,
    prev_index: usize,
    current_index: usize,
}

impl<'a> TokenOutputStream<'a> {
    fn new(tokenizer: &'a Tokenizer) -> Self {
        Self {
            tokenizer,
            tokens: Vec::new(),
            prev_index: 0,
            current_index: 0,
        }
    }

    fn decode(&self, tokens: &[u32]) -> Option<String> {
        self.tokenizer.decode(tokens, true).ok()
    }

    /// Feed one newly sampled token; returns the newly-revealed text, if any.
    fn next_token(&mut self, token: u32) -> Option<String> {
        let prev_text = if self.tokens.is_empty() {
            String::new()
        } else {
            self.decode(&self.tokens[self.prev_index..self.current_index])?
        };
        self.tokens.push(token);
        let text = self.decode(&self.tokens[self.prev_index..])?;
        if text.len() > prev_text.len() && text.chars().last()?.is_alphanumeric() {
            self.prev_index = self.current_index;
            self.current_index = self.tokens.len();
            Some(text.split_at(prev_text.len()).1.to_string())
        } else {
            None
        }
    }

    /// Flushes whatever trailing text `next_token` held back (e.g. the last
    /// word if generation stopped right after it, before a following token
    /// would have confirmed the word boundary). Call once after the loop.
    fn decode_rest(&self) -> Option<String> {
        let prev_text = if self.tokens.is_empty() {
            String::new()
        } else {
            self.decode(&self.tokens[self.prev_index..self.current_index])?
        };
        let text = self.decode(&self.tokens[self.prev_index..])?;
        if text.len() > prev_text.len() {
            Some(text.split_at(prev_text.len()).1.to_string())
        } else {
            None
        }
    }
}

impl LoadedModel {
    /// `device_index < 0` selects CPU; otherwise CUDA device `device_index`.
    pub fn load(gguf_path: &str, tokenizer_path: &str, device_index: i64) -> Result<Self, String> {
        let device = if device_index < 0 {
            Device::Cpu
        } else {
            Device::new_cuda(device_index as usize).map_err(|e| e.to_string())?
        };

        let mut file = std::fs::File::open(gguf_path).map_err(|e| e.to_string())?;
        let content = gguf_file::Content::read(&mut file).map_err(|e| e.to_string())?;
        let weights = ModelWeights::from_gguf(content, &mut file, &device, DType::F32)
            .map_err(|e| e.to_string())?;
        let tokenizer = Tokenizer::from_file(tokenizer_path).map_err(|e| e.to_string())?;

        // GLM-4-0414's chat rounds end on a fresh <|user|> turn or
        // <|endoftext|>; whichever the vocab actually defines become the
        // stop set.
        let eos_tokens: Vec<u32> = ["<|user|>", "<|endoftext|>", "<|observation|>"]
            .iter()
            .filter_map(|name| tokenizer.token_to_id(name))
            .collect();

        Ok(Self { weights: Mutex::new(weights), tokenizer, device, eos_tokens })
    }

    /// `history` is pre-formatted by the caller as alternating
    /// `<|user|>\n...\n<|assistant|>\n...\n` turns — this just adds the
    /// GLM-4 BOS/system preamble and leaves the final assistant turn open.
    fn build_prompt(system_prompt: &str, history: &str) -> String {
        let mut out = String::from("[gMASK]<sop>");
        if !system_prompt.is_empty() {
            out.push_str("<|system|>\n");
            out.push_str(system_prompt);
            out.push('\n');
        }
        out.push_str(history);
        out.push_str("<|assistant|>\n");
        out
    }

    /// Runs the full autoregressive decode loop synchronously, calling
    /// `on_token` with each generated piece as it's produced. Intended to be
    /// called from a background thread by the caller — this method blocks
    /// for as long as generation takes. Holds this model's weights lock for
    /// the duration, so only one generation runs at a time per handle.
    pub fn generate(
        &self,
        system_prompt: &str,
        history: &str,
        max_tokens: usize,
        temperature: f64,
        top_p: f64,
        seed: u64,
        mut on_token: impl FnMut(&str),
    ) {
        let prompt = Self::build_prompt(system_prompt, history);
        let encoding = match self.tokenizer.encode(prompt, true) {
            Ok(e) => e,
            Err(_) => return,
        };
        let mut tokens: Vec<u32> = encoding.get_ids().to_vec();
        if tokens.is_empty() {
            return;
        }

        let sampling = if temperature <= 0.0 {
            Sampling::ArgMax
        } else if top_p > 0.0 && top_p < 1.0 {
            Sampling::TopP { p: top_p, temperature }
        } else {
            Sampling::All { temperature }
        };
        let mut logits_processor = LogitsProcessor::from_sampling(seed, sampling);

        let mut weights = match self.weights.lock() {
            Ok(w) => w,
            Err(_) => return,
        };
        let mut stream = TokenOutputStream::new(&self.tokenizer);

        for index in 0..max_tokens.max(1) {
            let context_size = if index == 0 { tokens.len() } else { 1 };
            let start_pos = tokens.len() - context_size;
            let ctxt = &tokens[start_pos..];

            let input = match Tensor::new(ctxt, &self.device).and_then(|t| t.unsqueeze(0)) {
                Ok(t) => t,
                Err(_) => break,
            };
            let logits = match weights.forward(&input, start_pos) {
                Ok(l) => l,
                Err(_) => break,
            };
            let logits = match logits.squeeze(0).and_then(|l| l.to_dtype(DType::F32)) {
                Ok(l) => l,
                Err(_) => break,
            };
            let next_token = match logits_processor.sample(&logits) {
                Ok(t) => t,
                Err(_) => break,
            };
            tokens.push(next_token);
            if self.eos_tokens.contains(&next_token) {
                break;
            }

            if let Some(piece) = stream.next_token(next_token) {
                on_token(&piece);
            }
        }
        if let Some(rest) = stream.decode_rest() {
            on_token(&rest);
        }
    }
}
