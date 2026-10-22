//! The prompt and the generation loop. History arrives in the wire format the Ling apps already use (`<|user|>\n...\n<|assistant|>\n...\n`
//! blocks) and is rendered as ChatML, with the empty `<think>` block that makes Qwen3 answer directly: the same as `ling-ai`'s `llm.rs`.
use crate::error::Error;
use crate::gguf::{Gguf, Source};
use crate::model::{Model, State, MAX_BATCH};
use crate::sample::Sampler;
use crate::tok::{Detok, Tokenizer};
use alloc::string::String;
use alloc::vec::Vec;

/// splits the wire format into (role, content) turns
pub fn parse_history(history: &str) -> Vec<(&'static str, &str)> {
    const TAGS: [(&str, &str); 2] = [("<|user|>\n", "user"), ("<|assistant|>\n", "assistant")];
    let mut turns = Vec::new();
    let mut pos = 0;
    while pos < history.len() {
        let next = TAGS.iter().filter_map(|(tag, role)| history[pos..].find(tag).map(|i| (pos + i, tag.len(), *role))).min_by_key(|(i, _, _)| *i);
        let Some((tag_start, tag_len, role)) = next else { break };
        let content_start = tag_start + tag_len;
        let content_end = TAGS.iter().filter_map(|(tag, _)| history[content_start..].find(tag).map(|i| content_start + i)).min().unwrap_or(history.len());
        let content = history[content_start..content_end].trim_end_matches('\n');
        turns.push((if role == "user" { "user" } else { "assistant" }, content));
        pos = content_end;
    }
    turns
}

pub fn build_prompt(system: &str, history: &str) -> String {
    let mut out = String::new();
    if !system.is_empty() {
        out.push_str("<|im_start|>system\n");
        out.push_str(system);
        out.push_str("<|im_end|>\n");
    }
    for (role, content) in parse_history(history) {
        out.push_str("<|im_start|>");
        out.push_str(role);
        out.push('\n');
        out.push_str(content);
        out.push_str("<|im_end|>\n");
    }
    out.push_str("<|im_start|>assistant\n<think>\n\n</think>\n\n");
    out
}

/// a loaded model with its tokenizer
pub struct Llm {
    pub model: Model,
    pub tok: Tokenizer,
}

impl Llm {
    /// Parse, read the weights (`progress(done, total)` as they come) and build the tokenizer.
    pub fn load(src: &dyn Source, progress: &mut dyn FnMut(usize, usize)) -> Result<Llm, Error> {
        let g = Gguf::parse(src)?;
        let model = Model::load(src, &g, progress)?;
        let tok = Tokenizer::from_gguf(&g)?;
        Ok(Llm { model, tok })
    }

    /// Feed `ids` through the model from the start of `st` (positions 0..), in batches; the logits of the last one are left in `st.logits`.
    pub fn prefill(&self, st: &mut State, ids: &[u32], progress: &mut dyn FnMut(usize, usize)) -> Result<(), Error> {
        st.reset();
        let mut done = 0;
        while done < ids.len() {
            let n = (ids.len() - done).min(MAX_BATCH);
            self.model.forward(st, &ids[done..done + n], done + n == ids.len())?;
            done += n;
            progress(done, ids.len());
        }
        Ok(())
    }

    /// Answer a prompt. `on_piece` gets each piece of text as it is produced and returns false to stop early. Returns the number of
    /// tokens generated. The state is reused between calls (its cache starts over every time).
    pub fn generate(
        &self,
        st: &mut State,
        system: &str,
        history: &str,
        max_tokens: usize,
        temperature: f32,
        top_p: f32,
        seed: u64,
        on_piece: &mut dyn FnMut(&str) -> bool,
    ) -> Result<usize, Error> {
        let ids = self.tok.encode(&build_prompt(system, history));
        if ids.is_empty() || ids.len() >= st.max_ctx {
            return Err(Error::Format("the prompt does not fit the context"));
        }
        self.prefill(st, &ids, &mut |_, _| {})?;
        let budget = max_tokens.max(1).min(st.max_ctx - ids.len());
        let mut sampler = Sampler::new(seed, temperature, top_p);
        let mut detok = Detok::new();
        let mut produced = 0;
        for _ in 0..budget {
            let next = sampler.sample(&st.logits);
            if self.tok.eos.contains(&next) {
                break;
            }
            produced += 1;
            let piece = detok.feed(&self.tok, next);
            if !piece.is_empty() && !on_piece(&piece) {
                break;
            }
            if st.pos >= st.max_ctx {
                break;
            }
            self.model.forward(st, &[next], true)?;
        }
        Ok(produced)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_format_becomes_chatml() {
        let p = build_prompt("be brief", "<|user|>\nhi\n<|assistant|>\nhello\n<|user|>\nand now?\n");
        assert_eq!(
            p,
            "<|im_start|>system\nbe brief<|im_end|>\n<|im_start|>user\nhi<|im_end|>\n<|im_start|>assistant\nhello<|im_end|>\n<|im_start|>user\nand now?<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
        );
        assert_eq!(build_prompt("", "<|user|>\nx\n"), "<|im_start|>user\nx<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n");
    }
}
