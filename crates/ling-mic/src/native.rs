//! Native (non-WASM) microphone capture via cpal.

use crate::{MicConfig, MicError};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SizedSample};
use std::sync::{Arc, Mutex};

/// How many recent mono samples to keep for analysis (~0.17 s @ 48 kHz) — long
/// enough for low-note pitch detection and a 2048-pt FFT.
const RING: usize = 8192;

/// Names of every input device the host reports.
pub fn input_devices() -> Vec<String> {
    cpal::default_host()
        .input_devices()
        .map(|d| d.filter_map(|x| x.name().ok()).collect())
        .unwrap_or_default()
}

pub struct NativeMic {
    stream: Mutex<Option<cpal::Stream>>,
    preferred: Option<String>,
    peak: Arc<Mutex<f32>>,
    rms: Arc<Mutex<f32>>,
    samples: Arc<Mutex<Vec<f32>>>,
    rate: Arc<Mutex<u32>>,
    channels: Arc<Mutex<u16>>,
    device: Arc<Mutex<String>>,
}

impl NativeMic {
    pub fn open(config: MicConfig) -> Result<Self, MicError> {
        Ok(Self {
            stream: Mutex::new(None),
            preferred: config.device.filter(|d| !d.is_empty()),
            peak: Arc::new(Mutex::new(0.0)),
            rms: Arc::new(Mutex::new(0.0)),
            samples: Arc::new(Mutex::new(Vec::with_capacity(RING))),
            rate: Arc::new(Mutex::new(44_100)),
            channels: Arc::new(Mutex::new(1)),
            device: Arc::new(Mutex::new(String::new())),
        })
    }

    pub fn start<F>(&self, callback: F) -> Result<(), MicError>
    where
        F: Fn(&[f32], usize) + Send + Sync + 'static,
    {
        let host = cpal::default_host();
        let callback = Arc::new(callback);

        // The requested device first (when one was named), then the default input, then every
        // other input. A device can enumerate and report a config fine but still fail at
        // build/play time — e.g. an interface already opened exclusively by another app — so
        // falling back keeps the mic usable instead of silently going dead.
        let default_name = host.default_input_device().and_then(|d| d.name().ok());
        let named = |d: &cpal::Device, n: &str| d.name().map(|x| x == n).unwrap_or(false);
        let all: Vec<cpal::Device> = host.input_devices().map(|d| d.collect()).unwrap_or_default();
        let mut candidates: Vec<cpal::Device> = Vec::new();
        if let Some(p) = &self.preferred {
            candidates.extend(all.iter().filter(|d| named(d, p)).cloned());
        }
        if let Some(d) = host.default_input_device() {
            candidates.push(d);
        }
        for d in all {
            let is_default = default_name.as_deref().is_some_and(|n| named(&d, n));
            let is_pref = self.preferred.as_deref().is_some_and(|p| named(&d, p));
            if !is_default && !is_pref {
                candidates.push(d);
            }
        }
        if candidates.is_empty() {
            return Err(MicError::NoDevice);
        }

        let mut last_err = MicError::NoDevice;
        for device in candidates {
            match self.try_start_on(&device, Arc::clone(&callback)) {
                Ok(stream) => {
                    *self.device.lock().unwrap() = device.name().unwrap_or_default();
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
        F: Fn(&[f32], usize) + Send + Sync + 'static,
    {
        let config = device.default_input_config().map_err(|e| MicError::StreamError(e.to_string()))?;
        let channels = config.channels() as usize;
        *self.rate.lock().unwrap() = config.sample_rate().0;
        *self.channels.lock().unwrap() = config.channels();
        let cfg = config.config();
        let (p, r, s) = (Arc::clone(&self.peak), Arc::clone(&self.rms), Arc::clone(&self.samples));
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => build::<f32, F>(device, &cfg, channels, callback, p, r, s),
            cpal::SampleFormat::I16 => build::<i16, F>(device, &cfg, channels, callback, p, r, s),
            cpal::SampleFormat::I32 => build::<i32, F>(device, &cfg, channels, callback, p, r, s),
            cpal::SampleFormat::I8 => build::<i8, F>(device, &cfg, channels, callback, p, r, s),
            cpal::SampleFormat::U16 => build::<u16, F>(device, &cfg, channels, callback, p, r, s),
            cpal::SampleFormat::U8 => build::<u8, F>(device, &cfg, channels, callback, p, r, s),
            f => return Err(MicError::Unsupported(format!("input format {f:?}"))),
        }
        .map_err(|e| MicError::StreamError(e.to_string()))?;
        stream.play().map_err(|e| MicError::StreamError(e.to_string()))?;
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

    pub fn channels(&self) -> u16 {
        *self.channels.lock().unwrap()
    }

    pub fn device_name(&self) -> String {
        self.device.lock().unwrap().clone()
    }
}

/// Build an input stream for sample type `T`; the user callback always receives interleaved f32.
fn build<T, F>(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    channels: usize,
    callback: Arc<F>,
    peak_w: Arc<Mutex<f32>>,
    rms_w: Arc<Mutex<f32>>,
    samples_w: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample + 'static,
    f32: FromSample<T>,
    F: Fn(&[f32], usize) + Send + Sync + 'static,
{
    let mut conv: Vec<f32> = Vec::new();
    device.build_input_stream(
        cfg,
        move |data: &[T], _| {
            conv.clear();
            conv.extend(data.iter().map(|s| s.to_sample::<f32>()));
            let peak = conv.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let rms = (conv.iter().map(|s| s * s).sum::<f32>() / conv.len().max(1) as f32).sqrt();
            *peak_w.lock().unwrap() = peak;
            *rms_w.lock().unwrap() = rms;
            // Downmix interleaved → mono and append to a rolling ring so analysis (pitch/FFT)
            // sees a stable, long-enough window.
            let ch = channels.max(1);
            if let Ok(mut ring) = samples_w.lock() {
                for frame in conv.chunks(ch) {
                    ring.push(frame.iter().sum::<f32>() / ch as f32);
                }
                if ring.len() > RING {
                    let drop = ring.len() - RING;
                    ring.drain(0..drop);
                }
            }
            callback(&conv, ch);
        },
        |e| eprintln!("[ling-mic] stream error: {e}"),
        None,
    )
}
