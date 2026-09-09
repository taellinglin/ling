//! Native (non-WASM) microphone capture via cpal.

use crate::{MicConfig, MicError};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

/// How many recent mono samples to keep for analysis (~0.17 s @ 48 kHz) — long
/// enough for low-note pitch detection and a 2048-pt FFT.
const RING: usize = 8192;

pub struct NativeMic {
    stream: Mutex<Option<cpal::Stream>>,
    peak: Arc<Mutex<f32>>,
    rms: Arc<Mutex<f32>>,
    samples: Arc<Mutex<Vec<f32>>>,
    rate: Arc<Mutex<u32>>,
}

impl NativeMic {
    pub fn open(_config: MicConfig) -> Result<Self, MicError> {
        Ok(Self {
            stream: Mutex::new(None),
            peak: Arc::new(Mutex::new(0.0)),
            rms: Arc::new(Mutex::new(0.0)),
            samples: Arc::new(Mutex::new(Vec::with_capacity(RING))),
            rate: Arc::new(Mutex::new(44_100)),
        })
    }

    pub fn start<F>(&self, callback: F) -> Result<(), MicError>
    where
        F: Fn(&[f32]) + Send + Sync + 'static,
    {
        let host = cpal::default_host();
        let callback = Arc::new(callback);

        // Try the default input device first, then fall back through every
        // other enumerated input device. A device can enumerate and report a
        // config fine but still fail at build/play time — e.g. an audio
        // interface already opened exclusively by another app (or another
        // Ling process) — so falling back keeps the mic usable instead of
        // silently going dead just because the default happens to be busy.
        let default_name = host.default_input_device().and_then(|d| d.name().ok());
        let mut candidates: Vec<cpal::Device> = Vec::new();
        if let Some(d) = host.default_input_device() {
            candidates.push(d);
        }
        if let Ok(devices) = host.input_devices() {
            for d in devices {
                let is_default = d
                    .name()
                    .ok()
                    .is_some_and(|n| default_name.as_deref() == Some(n.as_str()));
                if !is_default {
                    candidates.push(d);
                }
            }
        }
        if candidates.is_empty() {
            return Err(MicError::NoDevice);
        }

        let mut last_err = MicError::NoDevice;
        for device in candidates {
            match self.try_start_on(&device, Arc::clone(&callback)) {
                Ok(stream) => {
                    *self.stream.lock().unwrap() = Some(stream);
                    return Ok(());
                },
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    fn try_start_on<F>(&self, device: &cpal::Device, callback: Arc<F>) -> Result<cpal::Stream, MicError>
    where
        F: Fn(&[f32]) + Send + Sync + 'static,
    {
        let config = device
            .default_input_config()
            .map_err(|e| MicError::StreamError(e.to_string()))?;

        let channels = config.channels() as usize;
        *self.rate.lock().unwrap() = config.sample_rate().0;

        let peak_w = Arc::clone(&self.peak);
        let rms_w = Arc::clone(&self.rms);
        let samples_w = Arc::clone(&self.samples);

        let stream = device
            .build_input_stream(
                &config.into(),
                move |data: &[f32], _| {
                    let peak = data.iter().cloned().fold(0.0f32, f32::max);
                    let rms =
                        (data.iter().map(|s| s * s).sum::<f32>() / data.len().max(1) as f32).sqrt();
                    *peak_w.lock().unwrap() = peak;
                    *rms_w.lock().unwrap() = rms;
                    // Downmix interleaved → mono and append to a rolling ring so
                    // analysis (pitch/FFT) sees a stable, long-enough window.
                    let ch = channels.max(1);
                    if let Ok(mut ring) = samples_w.lock() {
                        for frame in data.chunks(ch) {
                            let m = frame.iter().sum::<f32>() / ch as f32;
                            ring.push(m);
                        }
                        if ring.len() > RING {
                            let drop = ring.len() - RING;
                            ring.drain(0..drop);
                        }
                    }
                    callback(data);
                },
                |e| eprintln!("[ling-mic] stream error: {e}"),
                None,
            )
            .map_err(|e| MicError::StreamError(e.to_string()))?;

        stream
            .play()
            .map_err(|e| MicError::StreamError(e.to_string()))?;
        Ok(stream)
    }

    pub fn stop(&self) {
        // Drop the stream to stop it.
        *self.stream.lock().unwrap() = None;
    }

    pub fn peak(&self) -> f32 {
        *self.peak.lock().unwrap()
    }

    pub fn rms(&self) -> f32 {
        *self.rms.lock().unwrap()
    }

    pub fn latest_samples(&self) -> Vec<f32> {
        self.samples.lock().unwrap().clone()
    }

    pub fn sample_rate(&self) -> u32 {
        *self.rate.lock().unwrap()
    }
}
