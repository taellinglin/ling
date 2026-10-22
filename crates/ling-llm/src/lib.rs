//! ling-llm: a freestanding Qwen3 inference engine.
//!
//! Everything is `no_std` + `alloc`: the model, its tokenizer and its chat template all come out of one GGUF file, read through the
//! [`gguf::Source`] trait (a file on a host, a disk image in LingOS), and the maths needs only `libm`. Weights are held in chunks of at
//! most [`quant::CHUNK_BYTES`] so a kernel whose allocator cannot hand out hundreds of contiguous megabytes still loads a 600 MB model.
//!
//! * [`gguf`]  the container: metadata, tensor table, a tensor's bytes through a `Source`
//! * [`quant`] F32 / F16 / Q8_0 / Q4_K / Q6_K tensors and their matrix-vector products
//! * [`tok`]   the byte-level BPE tokenizer built from the GGUF's own vocabulary and merges
//! * [`model`] the Qwen3 transformer: RMSNorm, per-head q/k norm, NeoX RoPE, grouped-query attention, SwiGLU, an f16 KV cache
//! * [`sample`] greedy and temperature / top-p sampling
//! * [`chat`]  the ChatML prompt and the generation loop
#![no_std]
extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod chat;
pub mod error;
pub mod f16;
pub mod gguf;
pub mod m;
pub mod model;
pub mod quant;
pub mod sample;
pub mod tok;

pub use chat::Llm;
pub use error::Error;
pub use gguf::{Gguf, Source};
pub use model::{Config, Model, State};
pub use tok::Tokenizer;
