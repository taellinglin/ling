//! Native (non-WASM) microphone capture via cpal.

use std::sync::{Arc, Mutex};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crate::{MicConfig, MicError};

pub struct NativeMic {
    stream:  Mutex<Option<cpal::Stream>>,
    peak:    Arc<Mutex<f32>>,
    rms:     Arc<Mutex<f32>>,
    samples: Arc<Mutex<Vec<f32>>>,
}

impl NativeMic {
    pub fn open(_config: MicConfig) -> Result<Self, MicError> {
        Ok(Self {
            stream:  Mutex::new(None),
            peak:    Arc::new(Mutex::new(0.0)),
            rms:     Arc::new(Mutex::new(0.0)),
            samples: Arc::new(Mutex::new(Vec::new())),
        })
    }

    pub fn start<F>(&self, callback: F) -> Result<(), MicError>
    where
        F: Fn(&[f32]) + Send + 'static,
    {
        let host   = cpal::default_host();
        let device = host.default_input_device().ok_or(MicError::NoDevice)?;
        let config = device.default_input_config()
            .map_err(|e| MicError::StreamError(e.to_string()))?;

        let peak_w    = Arc::clone(&self.peak);
        let rms_w     = Arc::clone(&self.rms);
        let samples_w = Arc::clone(&self.samples);

        let stream = device.build_input_stream(
            &config.into(),
            move |data: &[f32], _| {
                let peak = data.iter().cloned().fold(0.0f32, f32::max);
                let rms  = (data.iter().map(|s| s * s).sum::<f32>() / data.len() as f32).sqrt();
                *peak_w.lock().unwrap() = peak;
                *rms_w.lock().unwrap()  = rms;
                // Store latest samples for FFT
                *samples_w.lock().unwrap() = data.to_vec();
                callback(data);
            },
            |e| eprintln!("[ling-mic] stream error: {e}"),
            None,
        ).map_err(|e| MicError::StreamError(e.to_string()))?;

        stream.play().map_err(|e| MicError::StreamError(e.to_string()))?;
        *self.stream.lock().unwrap() = Some(stream);
        Ok(())
    }

    pub fn stop(&self) {
        // Drop the stream to stop it.
        *self.stream.lock().unwrap() = None;
    }

    pub fn peak(&self) -> f32 { *self.peak.lock().unwrap() }
    pub fn rms(&self)  -> f32 { *self.rms.lock().unwrap()  }
    pub fn latest_samples(&self) -> Vec<f32> { self.samples.lock().unwrap().clone() }
}
