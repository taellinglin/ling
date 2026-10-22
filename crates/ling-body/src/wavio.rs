//! RIFF/WAVE writer and chunk scanner. Decoding of the audio itself is left to
//! `ling_music`, which also reads FLAC, Ogg and MP3; this module only handles the
//! container so private chunks can ride along with the samples.

pub type ChunkId = [u8; 4];

/// Serialises planar `channels` (equal length, nominal range ±1) as 16/24-bit PCM or 32-bit float.
pub fn encode(rate: u32, channels: &[Vec<f32>], bits: u32, extra: &[(ChunkId, Vec<u8>)]) -> Vec<u8> {
    let nch = channels.len().max(1);
    let frames = channels.first().map_or(0, Vec::len);
    let (tag, bits) = match bits {
        32 => (3u16, 32u16),
        24 => (1, 24),
        _ => (1, 16),
    };
    let bytes_per = (bits / 8) as usize;
    let mut data = Vec::with_capacity(frames * nch * bytes_per);
    for f in 0..frames {
        for ch in channels {
            let s = ch[f].clamp(-1.0, 1.0);
            match bits {
                32 => data.extend_from_slice(&s.to_le_bytes()),
                24 => {
                    let v = (s * 8_388_607.0).round() as i32;
                    data.extend_from_slice(&v.to_le_bytes()[..3]);
                },
                _ => data.extend_from_slice(&((s * 32767.0).round() as i16).to_le_bytes()),
            }
        }
    }

    let mut fmt = Vec::with_capacity(16);
    fmt.extend_from_slice(&tag.to_le_bytes());
    fmt.extend_from_slice(&(nch as u16).to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * nch as u32 * bytes_per as u32).to_le_bytes());
    fmt.extend_from_slice(&((nch * bytes_per) as u16).to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());

    let mut body: Vec<u8> = b"WAVE".to_vec();
    push_chunk(&mut body, *b"fmt ", &fmt);
    if tag == 3 {
        push_chunk(&mut body, *b"fact", &(frames as u32).to_le_bytes());
    }
    push_chunk(&mut body, *b"data", &data);
    for (id, payload) in extra {
        push_chunk(&mut body, *id, payload);
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn push_chunk(out: &mut Vec<u8>, id: ChunkId, payload: &[u8]) {
    out.extend_from_slice(&id);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        out.push(0);
    }
}

/// Every chunk of a RIFF/WAVE file as `(id, payload)`.
pub fn chunks(bytes: &[u8]) -> Result<Vec<(ChunkId, &[u8])>, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut out = Vec::new();
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = [bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]];
        let len = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
        let start = pos + 8;
        let end = (start + len).min(bytes.len());
        out.push((id, &bytes[start..end]));
        pos = start + len + (len & 1);
    }
    Ok(out)
}

pub fn find_chunk<'a>(bytes: &'a [u8], id: &ChunkId) -> Option<&'a [u8]> {
    chunks(bytes).ok()?.into_iter().find(|(i, _)| i == id).map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_round_trip_with_private_chunk() {
        let l: Vec<f32> = (0..500).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let r: Vec<f32> = l.iter().map(|v| -v).collect();
        for bits in [16, 24, 32] {
            let wav = encode(22050, &[l.clone(), r.clone()], bits, &[(*b"LBDY", b"odd".to_vec())]);
            assert_eq!(find_chunk(&wav, b"LBDY"), Some(&b"odd"[..]));
            let dec = ling_music::decode::from_bytes(&wav).unwrap();
            assert_eq!(dec.rate, 22050);
            assert_eq!(dec.channels, 2);
            assert_eq!(dec.mono.len(), 500);
            let tol = if bits == 16 { 1e-3 } else { 1e-5 };
            assert!((dec.stereo[2 * 100] - l[100]).abs() < tol);
            assert!((dec.stereo[2 * 100 + 1] - r[100]).abs() < tol);
        }
    }
}
