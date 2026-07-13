//! Live music engine: its own cpal output stream mixing decoded-track playback
//! with the polyphonic [`Synth`]. Separate from `ling-audio`'s `AudioEngine` so
//! music and sound-effects run independently.

use crate::synth::{Patch, Synth};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

struct TrackPlayer {
    stereo: Arc<Vec<f32>>, // interleaved L,R at src_rate
    src_rate: u32,
    pos: f64, // fractional stereo-pair index
    playing: bool,
    volume: f32,
}

struct MusicState {
    track: Option<TrackPlayer>,
    synth: Synth,
    master: f32,
    out_rate: u32,
}

impl MusicState {
    #[inline]
    fn next(&mut self) -> (f32, f32) {
        let (mut l, mut r) = (0.0f32, 0.0f32);

        if let Some(tp) = &mut self.track {
            if tp.playing {
                let n = tp.stereo.len() / 2;
                if n >= 2 {
                    let ratio = tp.src_rate as f64 / self.out_rate as f64;
                    let idx = tp.pos as usize;
                    if idx + 1 < n {
                        let frac = (tp.pos - idx as f64) as f32;
                        let bl = tp.stereo[idx * 2]
                            + (tp.stereo[(idx + 1) * 2] - tp.stereo[idx * 2]) * frac;
                        let br = tp.stereo[idx * 2 + 1]
                            + (tp.stereo[(idx + 1) * 2 + 1] - tp.stereo[idx * 2 + 1]) * frac;
                        l += bl * tp.volume;
                        r += br * tp.volume;
                        tp.pos += ratio;
                    } else {
                        tp.playing = false; // reached the end
                    }
                }
            }
        }

        let s = self.synth.next_sample();
        l += s;
        r += s;

        let m = self.master;
        ((l * m).tanh(), (r * m).tanh())
    }
}

/// The live music engine. Construct once; keep alive for the program duration.
pub struct MusicEngine {
    state: Arc<Mutex<MusicState>>,
    _stream: cpal::Stream,
    pub out_rate: u32,
}

impl MusicEngine {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("no output device")?;
        let supported = device.default_output_config()?;
        let channels = supported.channels() as usize;
        let out_rate = supported.sample_rate().0;
        let fmt = supported.sample_format();
        let config = supported.config();

        let state = Arc::new(Mutex::new(MusicState {
            track: None,
            synth: Synth::new(out_rate as f32),
            master: 0.8,
            out_rate,
        }));
        let stream = build_stream(&device, &config, channels, Arc::clone(&state), fmt)?;
        stream.play()?;
        Ok(Self { state, _stream: stream, out_rate })
    }

    // ── track playback ─────────────────────────────────────────────────────
    pub fn set_track(&self, stereo: Vec<f32>, src_rate: u32) {
        if let Ok(mut s) = self.state.lock() {
            s.track = Some(TrackPlayer {
                stereo: Arc::new(stereo),
                src_rate,
                pos: 0.0,
                playing: false,
                volume: 1.0,
            });
        }
    }

    pub fn play(&self) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(t) = &mut s.track {
                t.playing = true;
            }
        }
    }

    pub fn pause(&self) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(t) = &mut s.track {
                t.playing = false;
            }
        }
    }

    pub fn stop(&self) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(t) = &mut s.track {
                t.playing = false;
                t.pos = 0.0;
            }
        }
    }

    pub fn seek(&self, sec: f32) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(t) = &mut s.track {
                t.pos = (sec.max(0.0) as f64) * t.src_rate as f64;
            }
        }
    }

    /// Current playback position in seconds.
    pub fn position(&self) -> f32 {
        self.state
            .lock()
            .ok()
            .and_then(|s| s.track.as_ref().map(|t| (t.pos / t.src_rate as f64) as f32))
            .unwrap_or(0.0)
    }

    pub fn set_volume(&self, v: f32) {
        if let Ok(mut s) = self.state.lock() {
            s.master = v.clamp(0.0, 2.0);
        }
    }

    pub fn set_track_volume(&self, v: f32) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(t) = &mut s.track {
                t.volume = v.clamp(0.0, 2.0);
            }
        }
    }

    // ── synth ──────────────────────────────────────────────────────────────
    pub fn add_patch(&self, p: Patch) -> usize {
        self.state
            .lock()
            .map(|mut s| s.synth.add_patch(p))
            .unwrap_or(0)
    }

    pub fn note(&self, patch: usize, midi: i32, vel: f32, dur: f32) -> u32 {
        self.state
            .lock()
            .map(|mut s| s.synth.note(patch, midi, vel, dur))
            .unwrap_or(0)
    }

    pub fn note_on(&self, patch: usize, midi: i32, vel: f32) -> u32 {
        self.state
            .lock()
            .map(|mut s| s.synth.note_on(patch, midi, vel))
            .unwrap_or(0)
    }

    pub fn note_off(&self, patch: usize, midi: i32) {
        if let Ok(mut s) = self.state.lock() {
            s.synth.note_off_pitch(patch, midi);
        }
    }

    pub fn all_notes_off(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.synth.all_notes_off();
        }
    }
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    state: Arc<Mutex<MusicState>>,
    fmt: cpal::SampleFormat,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    let err_fn = |e: cpal::StreamError| eprintln!("ling-music stream error: {e}");
    Ok(match fmt {
        cpal::SampleFormat::F32 => {
            let st = Arc::clone(&state);
            device.build_output_stream(
                config,
                move |d: &mut [f32], _| fill_f32(d, channels, &st),
                err_fn,
                None,
            )?
        },
        _ => {
            let st = Arc::clone(&state);
            device.build_output_stream::<i16, _, _>(
                config,
                move |d: &mut [i16], _| fill_i16(d, channels, &st),
                err_fn,
                None,
            )?
        },
    })
}

fn fill_f32(data: &mut [f32], ch: usize, state: &Arc<Mutex<MusicState>>) {
    let ch = ch.max(1);
    if let Ok(mut s) = state.try_lock() {
        for frame in data.chunks_mut(ch) {
            let (l, r) = s.next();
            frame[0] = l;
            if ch > 1 {
                frame[1] = r;
            }
            for x in frame.iter_mut().skip(2) {
                *x = 0.0;
            }
        }
    } else {
        for s in data.iter_mut() {
            *s = 0.0;
        }
    }
}

fn fill_i16(data: &mut [i16], ch: usize, state: &Arc<Mutex<MusicState>>) {
    let ch = ch.max(1);
    if let Ok(mut s) = state.try_lock() {
        for frame in data.chunks_mut(ch) {
            let (l, r) = s.next();
            frame[0] = (l * 32767.0) as i16;
            if ch > 1 {
                frame[1] = (r * 32767.0) as i16;
            }
            for x in frame.iter_mut().skip(2) {
                *x = 0;
            }
        }
    } else {
        for s in data.iter_mut() {
            *s = 0;
        }
    }
}
