//! Decode WAV/FLAC/OGG-Vorbis/MP3/AAC → PCM via `symphonia`.

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Decoded audio: interleaved stereo (for playback) + a mono downmix (for
/// analysis), plus the source sample rate and duration in seconds.
pub struct DecodedAudio {
    pub stereo: Vec<f32>,
    pub mono: Vec<f32>,
    pub rate: u32,
    pub channels: usize,
    pub duration: f32,
}

/// Load and fully decode an audio file to PCM.
pub fn load(path: &str) -> Result<DecodedAudio, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str());
    decode_stream(mss, ext)
}

/// Decode fully from an in-memory byte buffer.
pub fn from_bytes(data: &[u8]) -> Result<DecodedAudio, String> {
    let cursor = std::io::Cursor::new(data.to_vec());
    let mss = MediaSourceStream::new(Box::new(cursor), Default::default());
    decode_stream(mss, None)
}

fn decode_stream(mss: MediaSourceStream, ext: Option<&str>) -> Result<DecodedAudio, String> {
    let mut hint = Hint::new();
    if let Some(ext) = ext {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions { enable_gapless: true, ..Default::default() },
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("probe failed: {e}"))?;
    let mut format = probed.format;

    let track = format.default_track().ok_or("no audio track")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("decoder: {e}"))?;

    let mut rate = track.codec_params.sample_rate.unwrap_or(44100);
    let mut channels = track
        .codec_params
        .channels
        .map(|c| c.count())
        .unwrap_or(2)
        .max(1);
    let mut inter: Vec<f32> = Vec::new();
    let mut sbuf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(_)) => break, // end of stream
            Err(e) => return Err(format!("read: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                rate = spec.rate;
                channels = spec.channels.count().max(1);
                if sbuf.is_none() {
                    sbuf = Some(SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
                }
                if let Some(sb) = sbuf.as_mut() {
                    sb.copy_interleaved_ref(decoded);
                    inter.extend_from_slice(sb.samples());
                }
            },
            Err(SymError::DecodeError(_)) => continue, // skip a bad frame
            Err(_) => break,
        }
    }

    if inter.is_empty() {
        return Err("decoded zero samples".into());
    }

    // Build interleaved stereo + a mono downmix.
    let frames = inter.len() / channels;
    let mut stereo = Vec::with_capacity(frames * 2);
    let mut mono = Vec::with_capacity(frames);
    for f in 0..frames {
        let base = f * channels;
        let l = inter[base];
        let r = if channels > 1 { inter[base + 1] } else { l };
        stereo.push(l);
        stereo.push(r);
        let mut sum = 0.0;
        for c in 0..channels {
            sum += inter[base + c];
        }
        mono.push(sum / channels as f32);
    }

    let duration = mono.len() as f32 / rate.max(1) as f32;
    Ok(DecodedAudio { stereo, mono, rate, channels, duration })
}
