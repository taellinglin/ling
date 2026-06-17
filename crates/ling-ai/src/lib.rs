//! ling-ai — tensor math, neural networks, behavior trees, and an in-engine
//! dialog language model for Ling.
//!
//! # 2030 game-AI toolkit
//! - [`nn`] — an optimized, dependency-free feed-forward neural net with SGD
//!   training. Cache-friendly flat buffers; built for real-time inference.
//! - [`bt`] — behavior trees authored as a tiny text DSL and ticked against a
//!   numeric blackboard. The decision layer for NPCs.
//! - [`dialog_lm`] — a miniature neural language model that learns a character's
//!   voice from a [`dataset`] corpus and samples fresh dialog lines.
//! - [`dataset`] — the **Ling Dialog Standard** (`.lingdialog`): a tiny,
//!   human-writable corpus format for training dialog models.

pub mod model;
pub mod prompt;
pub mod tensor;
pub mod tokenizer;

pub mod bt;
pub mod dataset;
pub mod dialog_lm;
pub mod nn;

pub use model::{InferenceEngine, ModelConfig};
pub use prompt::{ChatHistory, Message, Role};
pub use tensor::{Shape, Tensor};
pub use tokenizer::Tokenizer;

pub use bt::{Status, Tree};
pub use dataset::{Conversation, Dataset, Turn};
pub use dialog_lm::{DialogLM, LmConfig};
pub use nn::{Activation, Dense, Net, Rng};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
