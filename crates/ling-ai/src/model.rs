//! Inference engine trait and configuration.

use crate::tensor::Tensor;

#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub name: String,
    pub vocab_size: usize,
    pub hidden_size: usize,
    pub num_layers: usize,
    pub num_heads: usize,
    pub max_seq_len: usize,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            name: "ling-mini".into(),
            vocab_size: 32_000,
            hidden_size: 512,
            num_layers: 6,
            num_heads: 8,
            max_seq_len: 2048,
        }
    }
}

/// Inference backend — plug in candle, burn, GGUF, etc.
pub trait InferenceBackend: Send + Sync {
    fn forward(&self, input_ids: &[u32]) -> Tensor;
    fn config(&self) -> &ModelConfig;
}

/// Top-level engine that wraps any backend.
pub struct InferenceEngine {
    backend: Box<dyn InferenceBackend>,
}

impl InferenceEngine {
    pub fn new(backend: impl InferenceBackend + 'static) -> Self {
        Self { backend: Box::new(backend) }
    }

    pub fn forward(&self, input_ids: &[u32]) -> Tensor {
        self.backend.forward(input_ids)
    }

    pub fn config(&self) -> &ModelConfig {
        self.backend.config()
    }
}

/// Stub backend for testing / CI without a real model file.
pub struct StubBackend {
    pub cfg: ModelConfig,
}

impl InferenceBackend for StubBackend {
    fn forward(&self, input_ids: &[u32]) -> Tensor {
        Tensor::zeros(vec![input_ids.len(), self.cfg.vocab_size])
    }

    fn config(&self) -> &ModelConfig {
        &self.cfg
    }
}
