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
//! Supported architectures (selected by the GGUF's `general.architecture`
//! metadata, so `llm_load` needs no extra argument): `glm4` and `qwen3`.
//! Callers always pass history in the same GLM-tag wire format
//! (`<|user|>\n...\n<|assistant|>\n...\n` blocks — see `history_prompt()` in
//! each site's controllers); for Qwen3 models `build_prompt` re-renders that
//! history into ChatML, so no `.ling` app code changes when models swap.
//!
//! `generate()`'s `on_token` callback fires once per generated piece so the
//! caller can publish incremental progress (e.g. into a poll buffer) without
//! this crate knowing anything about jobs, threads, or HTTP.

use candle_core::quantized::gguf_file;
use candle_core::{DType, Device, Tensor};
use candle_transformers::generation::{LogitsProcessor, Sampling};
use candle_transformers::models::quantized_glm4;
use candle_transformers::models::quantized_qwen3;
use std::sync::Mutex;
use tokenizers::Tokenizer;

#[derive(Clone, Copy, PartialEq)]
enum Arch {
    Glm4,
    Qwen3,
}

enum ArchWeights {
    Glm4(quantized_glm4::ModelWeights),
    Qwen3(quantized_qwen3::ModelWeights),
}

impl ArchWeights {
    fn forward(&mut self, input: &Tensor, offset: usize) -> candle_core::Result<Tensor> {
        match self {
            ArchWeights::Glm4(w) => w.forward(input, offset),
            ArchWeights::Qwen3(w) => w.forward(input, offset),
        }
    }

    /// Must be called before each generation. glm4 resets its own KV cache
    /// whenever it sees offset 0, but qwen3 leaves that to the caller — a
    /// second generation on the same handle otherwise appends onto the
    /// previous conversation's cache at wrong positions and instantly
    /// produces EOS/garbage (empty replies from request #2 onward).
    fn reset_state(&mut self) {
        match self {
            ArchWeights::Glm4(_) => {}
            ArchWeights::Qwen3(w) => w.clear_kv_cache(),
        }
    }
}

pub struct LoadedModel {
    weights: Mutex<ArchWeights>,
    arch: Arch,
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
        let arch = match content.metadata.get("general.architecture") {
            Some(v) => v.to_string().map(|s| s.to_string()).unwrap_or_else(|_| "glm4".to_string()),
            None => "glm4".to_string(),
        };
        let (arch, weights) = match arch.as_str() {
            "qwen3" => {
                let w = quantized_qwen3::ModelWeights::from_gguf(content, &mut file, &device)
                    .map_err(|e| e.to_string())?;
                (Arch::Qwen3, ArchWeights::Qwen3(w))
            }
            "glm4" => {
                let w = quantized_glm4::ModelWeights::from_gguf(content, &mut file, &device, DType::F32)
                    .map_err(|e| e.to_string())?;
                (Arch::Glm4, ArchWeights::Glm4(w))
            }
            other => {
                return Err(format!(
                    "unsupported GGUF architecture '{other}' (supported: glm4, qwen3)"
                ))
            }
        };
        let tokenizer = Tokenizer::from_file(tokenizer_path).map_err(|e| e.to_string())?;

        // Stop set per family: GLM-4-0414 rounds end on a fresh <|user|> turn
        // or <|endoftext|>; Qwen3 (ChatML) ends on <|im_end|>. Whichever the
        // vocab actually defines become the stop set.
        let eos_names: &[&str] = match arch {
            Arch::Glm4 => &["<|user|>", "<|endoftext|>", "<|observation|>"],
            Arch::Qwen3 => &["<|im_end|>", "<|endoftext|>"],
        };
        let eos_tokens: Vec<u32> = eos_names
            .iter()
            .filter_map(|name| tokenizer.token_to_id(name))
            .collect();

        Ok(Self { weights: Mutex::new(weights), arch, tokenizer, device, eos_tokens })
    }

    /// `history` is pre-formatted by the caller as alternating
    /// `<|user|>\n...\n<|assistant|>\n...\n` turns — this just adds the
    /// GLM-4 BOS/system preamble and leaves the final assistant turn open.
    fn build_prompt_glm4(system_prompt: &str, history: &str) -> String {
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

    /// Splits the GLM-tag wire format back into (role, content) turns. Content
    /// runs from one tag to the next; the trailing newline the formatter adds
    /// after each turn is stripped. A literal tag inside user text would split
    /// a turn early — the same injection that already exists for GLM serving,
    /// accepted rather than escaped so the wire format stays dead simple.
    fn parse_history(history: &str) -> Vec<(&'static str, &str)> {
        const TAGS: [(&str, &str); 2] = [("<|user|>\n", "user"), ("<|assistant|>\n", "assistant")];
        let mut turns = Vec::new();
        let mut pos = 0;
        while pos < history.len() {
            let next = TAGS
                .iter()
                .filter_map(|(tag, role)| {
                    history[pos..].find(tag).map(|i| (pos + i, tag.len(), *role))
                })
                .min_by_key(|(i, _, _)| *i);
            let Some((tag_start, tag_len, role)) = next else { break };
            let content_start = tag_start + tag_len;
            let content_end = TAGS
                .iter()
                .filter_map(|(tag, _)| history[content_start..].find(tag).map(|i| content_start + i))
                .min()
                .unwrap_or(history.len());
            let content = history[content_start..content_end].trim_end_matches('\n');
            match role {
                "user" => turns.push(("user", content)),
                _ => turns.push(("assistant", content)),
            }
            pos = content_end;
        }
        turns
    }

    /// Same wire format in, ChatML out. The empty `<think>` block pins Qwen3
    /// to non-thinking mode (the official soft switch) so replies start
    /// immediately instead of streaming a reasoning preamble into the chat.
    fn build_prompt_qwen3(system_prompt: &str, history: &str) -> String {
        let mut out = String::new();
        if !system_prompt.is_empty() {
            out.push_str("<|im_start|>system\n");
            out.push_str(system_prompt);
            out.push_str("<|im_end|>\n");
        }
        for (role, content) in Self::parse_history(history) {
            out.push_str("<|im_start|>");
            out.push_str(role);
            out.push('\n');
            out.push_str(content);
            out.push_str("<|im_end|>\n");
        }
        out.push_str("<|im_start|>assistant\n<think>\n\n</think>\n\n");
        out
    }

    fn build_prompt(&self, system_prompt: &str, history: &str) -> String {
        match self.arch {
            Arch::Glm4 => Self::build_prompt_glm4(system_prompt, history),
            Arch::Qwen3 => Self::build_prompt_qwen3(system_prompt, history),
        }
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
        let prompt = self.build_prompt(system_prompt, history);
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
        weights.reset_state();
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
