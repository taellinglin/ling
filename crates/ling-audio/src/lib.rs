//! ling-audio — 4D positional audio synthesis and WAV BGM.
//!
//! Primary entry point: [`AudioEngine`].  Create one instance at startup,
//! feed it [`ToneParams`] via [`AudioEngine::set_tone`], update the listener
//! orientation each frame with [`AudioEngine::set_listener`], and optionally
//! load a WAV background track with [`AudioEngine::load_bgm`].

pub mod engine;

pub use engine::{AudioEngine, ToneParams};
