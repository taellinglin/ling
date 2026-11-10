//! ling-sd: Stable Diffusion 1.5 on the CPU, freestanding.
//!
//! Everything is `no_std` + `alloc` + `libm`, so the same code runs inside the LingOS kernel and in host tests. The weights come out
//! of one AUTOMATIC1111-style single-file checkpoint (`.safetensors`, fp16) read through [`Source`] (a file on a host, a FAT volume
//! in LingOS); they stay fp16 in RAM, packed into 16-column panels, and are widened to f32 a layer at a time right before use.
//!
//! * [`st`]      the safetensors container: header, tensor table, a tensor's bytes through a `Source`
//! * [`kern`]    the hooks (cores, yielding, AVX2) and the compute kernels: a 4x16 GEMM tile (SSE / AVX2+FMA / portable), exp, f16
//! * [`act`]     activations: tokens x channels, f32, held in pieces of at most 32 MiB (the LingOS allocator's largest block is 64)
//! * [`ops`]     the engine: packed weights, conv 3x3 / linear GEMMs, group / layer norm, attention, all split over the cores
//! * [`tok`]     CLIP's byte-level BPE tokenizer (vocab.json + merges.txt)
//! * [`prompt`]  the WebUI's prompt syntax: (word:1.2), ((word)), [word], BREAK
//! * [`clip`]    the CLIP ViT-L/14 text encoder
//! * [`unet`]    the SD 1.x UNet (LDM layout)
//! * [`vae`]     the VAE decoder
//! * [`sample`]  the k-diffusion noise schedule and samplers (Euler a, Euler, DPM++ 2M, Karras / uniform)
//! * [`rng`]     rand 0.8's StdRng (ChaCha12) + the host engine's Box-Muller, so a seed gives the host engine's starting noise
//! * [`model`]   loading a checkpoint and txt2img
#![no_std]
extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod act;
pub mod clip;
pub mod f16;
pub mod kern;
pub mod model;
pub mod ops;
pub mod prompt;
pub mod rng;
pub mod sample;
pub mod st;
pub mod tok;
pub mod unet;
pub mod vae;

pub use model::{Image, Model, Params, Progress, Stage};
pub use st::Source;

/// What went wrong, in words the user can act on.
pub type Error = alloc::string::String;
