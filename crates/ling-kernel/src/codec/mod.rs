//! Compressed audio for the sound store (`drivers::sfx`): Ogg Vorbis and FLAC,
//! decoded whole into interleaved 16-bit PCM when a file is loaded.

pub mod flac;
pub mod vorbis;

use alloc::vec::Vec;

/// What kind of audio file the bytes are, by their magic.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 4 && &bytes[..4] == b"OggS" {
        Some("ogg")
    } else if bytes.len() >= 4 && &bytes[..4] == b"fLaC" {
        Some("flac")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        Some("wav")
    } else {
        None
    }
}

/// Decode an Ogg Vorbis or FLAC file: (rate, channels 1-2, interleaved i16).
pub fn decode(bytes: &[u8]) -> Option<(u32, u8, Vec<i16>)> {
    let (rate, ch, pcm) = match sniff(bytes)? {
        "ogg" => vorbis::decode(bytes)?,
        "flac" => flac::decode(bytes)?,
        _ => return None,
    };
    if ch == 0 || rate == 0 {
        return None;
    }
    if ch <= 2 {
        return Some((rate, ch, pcm));
    }
    // more channels: keep front left / right
    let n = ch as usize;
    let mut st = Vec::with_capacity(pcm.len() / n * 2);
    for f in pcm.chunks_exact(n) {
        st.push(f[0]);
        st.push(f[1]);
    }
    Some((rate, 2, st))
}
