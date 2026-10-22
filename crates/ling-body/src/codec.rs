//! Body ⇄ WAV codec.
//!
//! The audio of an exported file is the stereo pressure recorded by two virtual
//! microphones beside the head while the body rings, so it can be played like any
//! other sound. Two private RIFF chunks carry the recipe:
//!   `LBDY`  JSON with every parameter and acoustic source
//!   `LBMS`  the extracted mesh, quantised to 16 bits per axis, delta + varint coded, deflated
//! Players and converters that drop unknown chunks leave plain audio, which
//! `analysis` can still turn into approximate parameters.

use crate::mesh::Mesh;
use crate::params::Params;
use crate::wavio;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::{Read, Write};

pub const PARAM_CHUNK: [u8; 4] = *b"LBDY";
pub const MESH_CHUNK: [u8; 4] = *b"LBMS";
const MESH_VERSION: u32 = 1;

fn put_varint(out: &mut Vec<u8>, v: i32) {
    let mut z = ((v << 1) ^ (v >> 31)) as u32;
    while z >= 0x80 {
        out.push((z as u8 & 0x7F) | 0x80);
        z >>= 7;
    }
    out.push(z as u8);
}

fn get_varint(b: &[u8], pos: &mut usize) -> Result<i32, String> {
    let mut z = 0u32;
    let mut shift = 0;
    loop {
        let byte = *b.get(*pos).ok_or("truncated mesh chunk")?;
        *pos += 1;
        z |= ((byte & 0x7F) as u32) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 28 {
            return Err("bad varint".into());
        }
    }
    Ok(((z >> 1) as i32) ^ -((z & 1) as i32))
}

pub fn pack_mesh(m: &Mesh) -> Vec<u8> {
    let (lo, hi) = m.bounds().unwrap_or(([0.0; 3], [1.0; 3]));
    let mut raw = Vec::new();
    let mut prev = [0i32; 3];
    for p in m.pos.as_chunks::<3>().0 {
        for k in 0..3 {
            let span = (hi[k] - lo[k]).max(1e-9);
            let q = (((p[k] - lo[k]) / span) * 65535.0).round() as i32;
            put_varint(&mut raw, q - prev[k]);
            prev[k] = q;
        }
    }
    let mut last = 0i32;
    for &i in &m.idx {
        put_varint(&mut raw, i as i32 - last);
        last = i as i32;
    }
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
    enc.write_all(&raw).expect("in-memory write");
    let packed = enc.finish().expect("in-memory finish");

    let mut out = Vec::with_capacity(36 + packed.len());
    out.extend_from_slice(&MESH_VERSION.to_le_bytes());
    out.extend_from_slice(&(m.vert_count() as u32).to_le_bytes());
    out.extend_from_slice(&(m.tri_count() as u32).to_le_bytes());
    for v in lo.iter().chain(hi.iter()) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&packed);
    out
}

pub fn unpack_mesh(b: &[u8]) -> Result<Mesh, String> {
    if b.len() < 36 {
        return Err("mesh chunk too short".into());
    }
    let u = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    if u(0) != MESH_VERSION {
        return Err(format!("unsupported mesh chunk version {}", u(0)));
    }
    let (nv, nt) = (u(4) as usize, u(8) as usize);
    let lo = [f(12), f(16), f(20)];
    let hi = [f(24), f(28), f(32)];
    let mut raw = Vec::new();
    ZlibDecoder::new(&b[36..]).read_to_end(&mut raw).map_err(|e| format!("mesh inflate: {e}"))?;
    let mut pos = 0usize;
    let mut m = Mesh::default();
    let mut prev = [0i32; 3];
    for _ in 0..nv {
        for k in 0..3 {
            prev[k] += get_varint(&raw, &mut pos)?;
            m.pos.push(lo[k] + prev[k] as f32 / 65535.0 * (hi[k] - lo[k]));
        }
    }
    let mut last = 0i32;
    for _ in 0..nt * 3 {
        last += get_varint(&raw, &mut pos)?;
        if last < 0 || last as usize >= nv {
            return Err("mesh index out of range".into());
        }
        m.idx.push(last as u32);
    }
    m.compute_normals();
    m.compute_uvs();
    Ok(m)
}

pub struct Decoded {
    pub rate: u32,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub params: Option<Params>,
    pub mesh: Option<Mesh>,
}

/// Builds the WAV bytes for a simulated body. `left`/`right` are scaled so the
/// louder channel peaks at `gain`.
pub fn encode_body(
    params: &Params,
    mesh: Option<&Mesh>,
    left: &[f32],
    right: &[f32],
    rate: u32,
    bits: u32,
    gain: f32,
) -> Vec<u8> {
    let peak = left.iter().chain(right).fold(0.0f32, |m, v| m.max(v.abs())).max(1e-12);
    let k = gain / peak;
    let l: Vec<f32> = left.iter().map(|v| v * k).collect();
    let r: Vec<f32> = right.iter().map(|v| v * k).collect();
    let mut extra = vec![(PARAM_CHUNK, params.to_json().to_string().into_bytes())];
    if let Some(m) = mesh {
        extra.push((MESH_CHUNK, pack_mesh(m)));
    }
    wavio::encode(rate, &[l, r], bits, &extra)
}

pub fn decode_body(bytes: &[u8]) -> Result<Decoded, String> {
    let audio = ling_music::decode::from_bytes(bytes)?;
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for f in audio.stereo.as_chunks::<2>().0 {
        left.push(f[0]);
        right.push(f[1]);
    }
    let mut params = None;
    let mut mesh = None;
    if let Ok(chunks) = wavio::chunks(bytes) {
        for (id, payload) in chunks {
            if id == PARAM_CHUNK {
                let v: serde_json::Value = serde_json::from_slice(payload).map_err(|e| format!("LBDY: {e}"))?;
                params = Some(Params::from_json(&v)?);
            } else if id == MESH_CHUNK {
                mesh = Some(unpack_mesh(payload)?);
            }
        }
    }
    Ok(Decoded { rate: audio.rate, left, right, params, mesh })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{Source, Waveform};

    fn tetra() -> Mesh {
        Mesh {
            pos: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            nrm: vec![],
            uv: vec![],
            idx: vec![0, 2, 1, 0, 1, 3, 1, 2, 3, 0, 3, 2],
        }
    }

    #[test]
    fn mesh_chunk_round_trip() {
        let m = tetra();
        let back = unpack_mesh(&pack_mesh(&m)).unwrap();
        assert_eq!(back.idx, m.idx);
        for (a, b) in back.pos.iter().zip(&m.pos) {
            assert!((a - b).abs() < 2e-5);
        }
    }

    #[test]
    fn wav_carries_params_and_mesh() {
        let mut p = Params::default();
        p.set("tension", 1.7);
        p.sources.push(Source::new("head", [0.0, 1.6, 0.1], Waveform::Impulse, 440.0));
        let l: Vec<f32> = (0..2000).map(|i| (i as f32 * 0.03).sin() * 0.01).collect();
        let r: Vec<f32> = l.iter().map(|v| v * 0.5).collect();
        let wav = encode_body(&p, Some(&tetra()), &l, &r, 22050, 24, 0.9);
        let d = decode_body(&wav).unwrap();
        assert_eq!(d.rate, 22050);
        assert_eq!(d.left.len(), 2000);
        assert_eq!(d.params.unwrap().get("tension"), 1.7);
        assert_eq!(d.mesh.unwrap().tri_count(), 4);
        let peak = d.left.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.9).abs() < 1e-3);
    }
}
