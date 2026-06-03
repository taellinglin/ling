//! WASM microphone stub — Web Audio API capture is handled in JS.

use crate::{MicConfig, MicError};

pub struct WebMic;

impl WebMic {
    pub fn open(_config: MicConfig) -> Result<Self, MicError> {
        Err(MicError::Unsupported("microphone capture on WASM requires JS interop".into()))
    }
    pub fn start<F>(&self, _cb: F) -> Result<(), MicError>
    where F: Fn(&[f32]) + Send + 'static { Ok(()) }
    pub fn stop(&self) {}
    pub fn peak(&self) -> f32 { 0.0 }
    pub fn rms(&self)  -> f32 { 0.0 }
    pub fn latest_samples(&self) -> Vec<f32> { Vec::new() }
}
