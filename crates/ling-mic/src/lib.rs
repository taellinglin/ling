//! ling-mic — real-time microphone capture for Ling.
//!
//! # Quick start
//! ```no_run
//! use ling_mic::{MicInput, MicConfig};
//! let mic = MicInput::open(MicConfig::default()).unwrap();
//! mic.start(|samples| {
//!     let rms: f32 = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
//!     println!("RMS level: {rms:.4}");
//! }).unwrap();
//! std::thread::sleep(std::time::Duration::from_secs(3));
//! mic.stop();
//! ```

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web;

pub use error::MicError;
pub mod error;

/// Configuration for the microphone input stream.
#[derive(Debug, Clone)]
pub struct MicConfig {
    /// Desired sample rate in Hz (e.g. 44100 or 48000).
    pub sample_rate: u32,
    /// Number of input channels (1 = mono, 2 = stereo).
    pub channels: u16,
    /// Preferred capture buffer size in frames.
    pub buffer_frames: u32,
}

impl Default for MicConfig {
    fn default() -> Self {
        Self { sample_rate: 44_100, channels: 1, buffer_frames: 512 }
    }
}

/// A live microphone input stream.
pub struct MicInput {
    #[cfg(not(target_arch = "wasm32"))]
    inner: native::NativeMic,
    #[cfg(target_arch = "wasm32")]
    inner: web::WebMic,
}

impl MicInput {
    /// Open the default input device with the given config.
    pub fn open(config: MicConfig) -> Result<Self, MicError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Ok(Self { inner: native::NativeMic::open(config)? })
        }
        #[cfg(target_arch = "wasm32")]
        {
            Ok(Self { inner: web::WebMic::open(config)? })
        }
    }

    /// Begin streaming; `callback` is called with each buffer of f32 PCM samples.
    pub fn start<F>(&self, callback: F) -> Result<(), MicError>
    where
        F: Fn(&[f32]) + Send + 'static,
    {
        self.inner.start(callback)
    }

    /// Stop the stream and release the device.
    pub fn stop(&self) {
        self.inner.stop();
    }

    /// Peak amplitude (0.0–1.0) of the most recent buffer.
    pub fn peak(&self) -> f32 {
        self.inner.peak()
    }

    /// RMS level (0.0–1.0) of the most recent buffer.
    pub fn rms(&self) -> f32 {
        self.inner.rms()
    }

    /// Get the most recent mono PCM samples (rolling window) for FFT/pitch analysis.
    pub fn latest_samples(&self) -> Vec<f32> {
        self.inner.latest_samples()
    }

    /// Actual capture sample rate of the open device (Hz).
    pub fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
}
