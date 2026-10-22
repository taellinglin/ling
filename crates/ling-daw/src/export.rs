//! Audio file export: WAV (16/24/32f), FLAC (16/24) and Ogg Vorbis.
use std::io::Write;

/// `data` is interleaved stereo f32 in [-1, 1].
/// `fmt`: "wav" | "flac" | "ogg". `bits` applies to wav/flac; `quality` (0..1) to ogg.
pub fn write_audio(path: &str, data: &[f32], sr: u32, fmt: &str, bits: u16, quality: f32) -> Result<(), String> {
    match fmt {
        "flac" => write_flac(path, data, sr, if bits >= 24 { 24 } else { 16 }),
        "ogg" => write_ogg(path, data, sr, quality),
        _ => write_wav(path, data, sr, bits),
    }
}

fn write_wav(path: &str, data: &[f32], sr: u32, bits: u16) -> Result<(), String> {
    let (fmt, bps) = if bits == 32 { (hound::SampleFormat::Float, 32) } else { (hound::SampleFormat::Int, if bits == 24 { 24 } else { 16 }) };
    let spec = hound::WavSpec { channels: 2, sample_rate: sr, bits_per_sample: bps, sample_format: fmt };
    let mut w = hound::WavWriter::create(path, spec).map_err(|x| x.to_string())?;
    for s in data {
        match bps {
            32 => w.write_sample(*s).map_err(|x| x.to_string())?,
            24 => w.write_sample((s.clamp(-1.0, 1.0) * 8_388_607.0) as i32).map_err(|x| x.to_string())?,
            _ => w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16).map_err(|x| x.to_string())?,
        }
    }
    w.finalize().map_err(|x| x.to_string())
}

fn write_flac(path: &str, data: &[f32], sr: u32, bits: usize) -> Result<(), String> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let scale = if bits == 24 { 8_388_607.0 } else { 32767.0 };
    let samples: Vec<i32> = data.iter().map(|s| (s.clamp(-1.0, 1.0) * scale) as i32).collect();
    let config = flacenc::config::Encoder::default().into_verified().map_err(|e| format!("flac config: {e:?}"))?;
    let source = flacenc::source::MemSource::from_samples(&samples, 2, bits, sr as usize);
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size).map_err(|e| format!("flac encode: {e:?}"))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|e| format!("flac write: {e:?}"))?;
    std::fs::write(path, sink.as_slice()).map_err(|e| e.to_string())
}

fn write_ogg(path: &str, data: &[f32], sr: u32, quality: f32) -> Result<(), String> {
    let mut enc = vorbis_encoder::Encoder::new(2, sr as u64, quality.clamp(-0.1, 1.0)).map_err(|e| format!("vorbis init error {e}"))?;
    let mut file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let pcm: Vec<i16> = data.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
    for chunk in pcm.chunks(2 * 16384) {
        let bytes = enc.encode(&chunk.to_vec()).map_err(|e| format!("vorbis encode error {e}"))?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    let tail = enc.flush().map_err(|e| format!("vorbis flush error {e}"))?;
    file.write_all(&tail).map_err(|e| e.to_string())
}
