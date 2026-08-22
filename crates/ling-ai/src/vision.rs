//! Image understanding for chat.ling-lang.org's attach-an-image flow
//! (UI screenshots, 3D/game screenshots — OCR/text-detection is handled
//! separately, see this crate's future `ocr` module).
//!
//! `VisionModel` loads a quantized Moondream2 checkpoint (candle-transformers'
//! `quantized_moondream`, CUDA-capable) — same plain-type, no-global-state
//! shape as [`crate::llm::LoadedModel`]: the `ling` binary's
//! `src/runtime/vision.rs` owns the handle table and background-thread/
//! polling glue, exactly mirroring how `src/runtime/llm.rs` wraps this.
//!
//! `analyze()`'s `on_token` callback fires once per generated piece, same
//! streaming-decode shape as `LoadedModel::generate()`.

use candle_core::quantized::gguf_file;
use candle_core::{DType, Device, Tensor};
use candle_transformers::generation::LogitsProcessor;
use candle_transformers::models::moondream::Config;
use candle_transformers::models::quantized_moondream::Model;
use image::imageops::FilterType;
use std::sync::Mutex;
use tokenizers::Tokenizer;

pub struct VisionModel {
    model: Mutex<Model>,
    tokenizer: Tokenizer,
    device: Device,
    eos_token: u32,
}

impl VisionModel {
    /// `device_index < 0` selects CPU; otherwise CUDA device `device_index`.
    pub fn load(gguf_path: &str, tokenizer_path: &str, device_index: i64) -> Result<Self, String> {
        let device = if device_index < 0 {
            Device::Cpu
        } else {
            Device::new_cuda(device_index as usize).map_err(|e| e.to_string())?
        };

        let mut file = std::fs::File::open(gguf_path).map_err(|e| e.to_string())?;
        let content = gguf_file::Content::read(&mut file).map_err(|e| e.to_string())?;
        let config = Config::v2();
        let vb =
            candle_transformers::quantized_var_builder::VarBuilder::from_gguf(gguf_path, &device)
                .map_err(|e| e.to_string())?;
        let _ = content; // content re-parsed internally by from_gguf(path, ..); kept only to fail fast above on a bad file.
        let model = Model::new(&config, vb).map_err(|e| e.to_string())?;

        let tokenizer = Tokenizer::from_file(tokenizer_path).map_err(|e| e.to_string())?;
        let eos_token = tokenizer
            .token_to_id("<|endoftext|>")
            .ok_or("moondream tokenizer has no <|endoftext|> token")?;

        Ok(Self { model: Mutex::new(model), tokenizer, device, eos_token })
    }

    /// Decodes `image_bytes` (PNG/JPEG), resizes/normalizes to what
    /// Moondream's vision encoder expects (378x378, mean/std 0.5), and
    /// returns the (3, 378, 378) input tensor.
    fn preprocess_image(&self, image_bytes: &[u8]) -> Result<Tensor, String> {
        let img = image::load_from_memory(image_bytes)
            .map_err(|e| e.to_string())?
            .resize_to_fill(378, 378, FilterType::Triangle)
            .to_rgb8();
        let data = img.into_raw();
        let data = Tensor::from_vec(data, (378, 378, 3), &Device::Cpu)
            .and_then(|t| t.permute((2, 0, 1)))
            .map_err(|e| e.to_string())?;
        let mean = Tensor::new(&[0.5f32, 0.5, 0.5], &Device::Cpu)
            .and_then(|t| t.reshape((3, 1, 1)))
            .map_err(|e| e.to_string())?;
        let std = Tensor::new(&[0.5f32, 0.5, 0.5], &Device::Cpu)
            .and_then(|t| t.reshape((3, 1, 1)))
            .map_err(|e| e.to_string())?;
        (data.to_dtype(DType::F32).map_err(|e| e.to_string())? / 255.)
            .and_then(|t| t.broadcast_sub(&mean))
            .and_then(|t| t.broadcast_div(&std))
            .and_then(|t| t.to_device(&self.device))
            .map_err(|e| e.to_string())
    }

    /// Runs vision-encode + autoregressive decode synchronously, calling
    /// `on_token` with each generated piece. Intended to be called from a
    /// background thread by the caller (same contract as
    /// `LoadedModel::generate`) — this blocks for as long as it takes.
    pub fn analyze(
        &self,
        image_bytes: &[u8],
        question: &str,
        max_tokens: usize,
        temperature: f64,
        top_p: f64,
        seed: u64,
        mut on_token: impl FnMut(&str),
    ) -> Result<(), String> {
        let image = self
            .preprocess_image(image_bytes)?
            .unsqueeze(0)
            .map_err(|e| e.to_string())?;

        // Moondream's expected prompting convention (not a system/user/
        // assistant chat template like the text model — a plain Q&A frame).
        let prompt = format!("\n\nQuestion: {question}\n\nAnswer:");
        let encoding = self
            .tokenizer
            .encode(prompt, true)
            .map_err(|e| e.to_string())?;
        let mut tokens: Vec<u32> = encoding.get_ids().to_vec();
        if tokens.is_empty() {
            return Err("empty prompt encoding".to_string());
        }

        let top_p_opt = if top_p > 0.0 && top_p < 1.0 {
            Some(top_p)
        } else {
            None
        };
        let mut logits_processor = LogitsProcessor::new(seed, Some(temperature), top_p_opt);

        let mut model = self.model.lock().map_err(|e| e.to_string())?;
        let image_embeds = image
            .apply(model.vision_encoder())
            .map_err(|e| e.to_string())?;

        for index in 0..max_tokens.max(1) {
            let context_size = if index > 0 { 1 } else { tokens.len() };
            let ctxt = &tokens[tokens.len().saturating_sub(context_size)..];
            let input = Tensor::new(ctxt, &self.device)
                .and_then(|t| t.unsqueeze(0))
                .map_err(|e| e.to_string())?;

            let logits = if index == 0 {
                let bos = Tensor::new(&[self.eos_token], &self.device)
                    .and_then(|t| t.unsqueeze(0))
                    .map_err(|e| e.to_string())?;
                model
                    .text_model
                    .forward_with_img(&bos, &input, &image_embeds)
                    .map_err(|e| e.to_string())?
            } else {
                model
                    .text_model
                    .forward(&input)
                    .map_err(|e| e.to_string())?
            };
            let logits = logits
                .squeeze(0)
                .and_then(|l| l.to_dtype(DType::F32))
                .map_err(|e| e.to_string())?;
            let next_token = logits_processor
                .sample(&logits)
                .map_err(|e| e.to_string())?;
            tokens.push(next_token);
            if next_token == self.eos_token {
                break;
            }

            if let Ok(piece) = self.tokenizer.decode(&[next_token], true) {
                if !piece.is_empty() {
                    on_token(&piece);
                }
            }
        }
        Ok(())
    }
}
