//! ling-ai — tensor math, inference scaffolding, and prompt utilities for Ling.

pub mod tensor;
pub mod model;
pub mod prompt;
pub mod tokenizer;

pub use tensor::{Tensor, Shape};
pub use model::{InferenceEngine, ModelConfig};
pub use prompt::{Message, Role, ChatHistory};
pub use tokenizer::Tokenizer;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
