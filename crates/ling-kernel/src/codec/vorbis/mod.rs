//! Ogg Vorbis decoding: lewton 0.10.2's decoder core (vendored, no_std) and
//! a small Ogg page reader. `decode` turns a whole `.ogg` file into
//! interleaved 16-bit PCM.
#![allow(dead_code, clippy::all)]

use alloc::vec::Vec;

macro_rules! record_residue_pre_inverse { ($e:expr) => {}; }
macro_rules! record_residue_post_inverse { ($e:expr) => {}; }
macro_rules! record_pre_mdct { ($e:expr) => {}; }
macro_rules! record_post_mdct { ($e:expr) => {}; }

pub mod audio;
pub mod bitpacking;
pub mod header;
pub mod header_cached;
pub mod huffman_tree;
pub mod imdct;
pub mod samples;

/// The float functions `std` would provide, from libm (no_std).
pub(crate) trait F32Ext {
    fn sqrt(self) -> f32;
    fn exp(self) -> f32;
    fn exp2(self) -> f32;
    fn ln(self) -> f32;
    fn cos(self) -> f32;
    fn sin(self) -> f32;
    fn floor(self) -> f32;
    fn ceil(self) -> f32;
    fn round(self) -> f32;
    fn powf(self, e: f32) -> f32;
    fn powi(self, e: i32) -> f32;
    fn atan(self) -> f32;
}

impl F32Ext for f32 {
    fn sqrt(self) -> f32 { libm::sqrtf(self) }
    fn exp(self) -> f32 { libm::expf(self) }
    fn exp2(self) -> f32 { libm::exp2f(self) }
    fn ln(self) -> f32 { libm::logf(self) }
    fn cos(self) -> f32 { libm::cosf(self) }
    fn sin(self) -> f32 { libm::sinf(self) }
    fn floor(self) -> f32 { libm::floorf(self) }
    fn ceil(self) -> f32 { libm::ceilf(self) }
    fn round(self) -> f32 { libm::roundf(self) }
    fn powf(self, e: f32) -> f32 { libm::powf(self, e) }
    fn powi(self, e: i32) -> f32 { libm::powf(self, e as f32) }
    fn atan(self) -> f32 { libm::atanf(self) }
}

#[derive(Debug)]
pub enum VorbisError {
    BadAudio(audio::AudioReadError),
    BadHeader(header::HeaderReadError),
}

impl From<audio::AudioReadError> for VorbisError {
    fn from(err: audio::AudioReadError) -> VorbisError {
        VorbisError::BadAudio(err)
    }
}

impl From<header::HeaderReadError> for VorbisError {
    fn from(err: header::HeaderReadError) -> VorbisError {
        VorbisError::BadHeader(err)
    }
}

fn ilog(val :u64) -> u8 {
	64 - val.leading_zeros() as u8
}

fn bit_reverse(n :u32) -> u32 {
	// From the stb_vorbis implementation
	let mut nn = n;
	nn = ((nn & 0xAAAAAAAA) >> 1) | ((nn & 0x55555555) << 1);
	nn = ((nn & 0xCCCCCCCC) >> 2) | ((nn & 0x33333333) << 2);
	nn = ((nn & 0xF0F0F0F0) >> 4) | ((nn & 0x0F0F0F0F) << 4);
	nn = ((nn & 0xFF00FF00) >> 8) | ((nn & 0x00FF00FF) << 8);
	return (nn >> 16) | (nn << 16);
}


/// The packets of the first logical stream in an Ogg file, in order.
fn ogg_packets(data: &[u8]) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut serial: Option<u32> = None;
    let mut off = 0usize;
    while off + 27 <= data.len() {
        if &data[off..off + 4] != b"OggS" {
            // resync on the next capture pattern
            match data[off + 1..].windows(4).position(|w| w == b"OggS") {
                Some(p) => {
                    off += 1 + p;
                    continue;
                },
                None => break,
            }
        }
        let sn = u32::from_le_bytes([data[off + 14], data[off + 15], data[off + 16], data[off + 17]]);
        let nseg = data[off + 26] as usize;
        let table = off + 27;
        if table + nseg > data.len() {
            break;
        }
        let mut body = table + nseg;
        let this_stream = *serial.get_or_insert(sn) == sn;
        for k in 0..nseg {
            let l = data[table + k] as usize;
            if body + l > data.len() {
                return packets;
            }
            if this_stream {
                cur.extend_from_slice(&data[body..body + l]);
                if l < 255 {
                    packets.push(core::mem::take(&mut cur));
                }
            }
            body += l;
        }
        off = body;
    }
    packets
}

/// Decode a whole Ogg Vorbis file: (sample rate, channels, interleaved i16).
pub fn decode(data: &[u8]) -> Option<(u32, u8, Vec<i16>)> {
    let packets = ogg_packets(data);
    if packets.len() < 3 {
        return None;
    }
    let ident = header::read_header_ident(&packets[0]).ok()?;
    let setup = header::read_header_setup(&packets[2], ident.audio_channels, (ident.blocksize_0, ident.blocksize_1)).ok()?;
    let ch = ident.audio_channels;
    let mut pwr = audio::PreviousWindowRight::new();
    let mut out: Vec<i16> = Vec::new();
    for p in &packets[3..] {
        match audio::read_audio_packet_generic::<samples::InterleavedSamples<i16>>(&ident, &setup, p, &mut pwr) {
            Ok(s) => out.extend_from_slice(&s.samples),
            Err(_) => continue, // a damaged packet: skip it, keep the rest
        }
    }
    Some((ident.audio_sample_rate, ch, out))
}
