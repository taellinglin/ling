//! A small PNG codec for the no_std build: reads 1 / 2 / 4 / 8 / 16-bit grey, grey + alpha, RGB, RGBA and
//! palette pictures (not interlaced) into straight RGBA16, and writes 8 / 16-bit RGBA, RGB and grey.

use alloc::string::String;
use alloc::vec::Vec;

use super::crc32_update;

const SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

/// a decoded picture: straight RGBA, 16 bits a channel
pub struct Rgba16 {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u16>,
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

pub fn is_png(data: &[u8]) -> bool {
    data.len() >= 8 && data[..8] == SIG
}

/// width and height from the header, without decoding
pub fn dimensions(data: &[u8]) -> Option<(usize, usize)> {
    if !is_png(data) || data.len() < 24 || &data[12..16] != b"IHDR" {
        return None;
    }
    Some((be32(&data[16..20]) as usize, be32(&data[20..24]) as usize))
}

pub fn decode(data: &[u8]) -> Result<Rgba16, String> {
    if !is_png(data) {
        return Err("not a PNG picture".into());
    }
    let mut i = 8;
    let (mut w, mut h, mut depth, mut ctype, mut interlace) = (0usize, 0usize, 0u8, 0u8, 0u8);
    let mut pal: Vec<[u8; 3]> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut idat: Vec<u8> = Vec::new();
    while i + 8 <= data.len() {
        let len = be32(&data[i..]) as usize;
        let kind = &data[i + 4..i + 8];
        let body = data.get(i + 8..i + 8 + len).ok_or("truncated PNG")?;
        match kind {
            b"IHDR" if len >= 13 => {
                w = be32(body) as usize;
                h = be32(&body[4..]) as usize;
                depth = body[8];
                ctype = body[9];
                interlace = body[12];
            }
            b"PLTE" => pal = body.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
            b"tRNS" => trns = body.to_vec(),
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        i += 12 + len;
    }
    if w == 0 || h == 0 || w > 32768 || h > 32768 {
        return Err("bad PNG size".into());
    }
    if interlace != 0 {
        return Err("interlaced PNGs are not supported on LingOS".into());
    }
    let chans = match ctype {
        0 => 1,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        _ => return Err("unknown PNG colour type".into()),
    };
    let bits = depth as usize * chans;
    let stride = (w * bits).div_ceil(8);
    let bpp = bits.div_ceil(8).max(1);
    let mut raw = miniz_oxide::inflate::decompress_to_vec_zlib(&idat).map_err(|_| String::from("broken PNG data"))?;
    if raw.len() < h * (stride + 1) {
        return Err("truncated PNG data".into());
    }
    // unfilter in place: row y occupies raw[y*(stride+1)+1 ..]
    for y in 0..h {
        let off = y * (stride + 1);
        let ft = raw[off];
        for x in 0..stride {
            let a = if x >= bpp { raw[off + 1 + x - bpp] } else { 0 };
            let b = if y > 0 { raw[off - stride - 1 + 1 + x] } else { 0 };
            let c = if y > 0 && x >= bpp { raw[off - stride - 1 + 1 + x - bpp] } else { 0 };
            let v = raw[off + 1 + x];
            raw[off + 1 + x] = match ft {
                1 => v.wrapping_add(a),
                2 => v.wrapping_add(b),
                3 => v.wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => {
                    let p = a as i16 + b as i16 - c as i16;
                    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
                    v.wrapping_add(if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    })
                }
                _ => v,
            };
        }
    }
    let maxv = ((1u32 << depth) - 1) as u32;
    let scale = |v: u32| -> u16 { (v * 65535 / maxv.max(1)) as u16 };
    // sample k (channel-major within a pixel) of row y
    let sample = |row: &[u8], k: usize| -> u32 {
        match depth {
            16 => ((row[k * 2] as u32) << 8) | row[k * 2 + 1] as u32,
            8 => row[k] as u32,
            d => {
                let d = d as usize;
                let bit = k * d;
                ((row[bit / 8] >> (8 - d - bit % 8)) as u32) & ((1 << d) - 1)
            }
        }
    };
    let trns_key = |k: usize| -> Option<u32> {
        if trns.len() >= 2 * (k + 1) {
            Some(((trns[2 * k] as u32) << 8) | trns[2 * k + 1] as u32)
        } else {
            None
        }
    };
    let mut px = vec![0u16; w * h * 4];
    for y in 0..h {
        let row = &raw[y * (stride + 1) + 1..y * (stride + 1) + 1 + stride];
        for x in 0..w {
            let o = (y * w + x) * 4;
            let rgba: [u16; 4] = match ctype {
                0 => {
                    let g = sample(row, x);
                    let a = if trns_key(0) == Some(g) { 0 } else { 65535 };
                    let g = scale(g);
                    [g, g, g, a]
                }
                2 => {
                    let (r, g, b) = (sample(row, x * 3), sample(row, x * 3 + 1), sample(row, x * 3 + 2));
                    let a = if trns_key(0) == Some(r) && trns_key(1) == Some(g) && trns_key(2) == Some(b) { 0 } else { 65535 };
                    [scale(r), scale(g), scale(b), a]
                }
                3 => {
                    let idx = sample(row, x) as usize;
                    let c = pal.get(idx).copied().unwrap_or([0, 0, 0]);
                    let a = trns.get(idx).copied().unwrap_or(255);
                    [c[0] as u16 * 257, c[1] as u16 * 257, c[2] as u16 * 257, a as u16 * 257]
                }
                4 => {
                    let g = scale(sample(row, x * 2));
                    [g, g, g, scale(sample(row, x * 2 + 1))]
                }
                _ => [scale(sample(row, x * 4)), scale(sample(row, x * 4 + 1)), scale(sample(row, x * 4 + 2)), scale(sample(row, x * 4 + 3))],
            };
            px[o..o + 4].copy_from_slice(&rgba);
        }
    }
    Ok(Rgba16 { w, h, px })
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let crc = crc32_update(crc32_update(0, kind), body);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// PNG bytes of `bytes` (rows of w pixels, `chans` channels of `depth` 8 or 16 bits, big-endian for 16)
pub fn encode_raw(w: usize, h: usize, chans: usize, depth: u8, bytes: &[u8]) -> Vec<u8> {
    let ctype = match chans {
        1 => 0u8,
        2 => 4,
        3 => 2,
        _ => 6,
    };
    let bpp = chans * depth as usize / 8;
    let stride = w * bpp;
    // the Sub filter: cheap and good on flat colour and soft gradients
    let mut f = Vec::with_capacity(h * (stride + 1));
    for y in 0..h {
        let row = &bytes[y * stride..(y + 1) * stride];
        f.push(1u8);
        for x in 0..stride {
            let a = if x >= bpp { row[x - bpp] } else { 0 };
            f.push(row[x].wrapping_sub(a));
        }
    }
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&f, 3);
    drop(f);
    let mut out = Vec::with_capacity(z.len() + 64);
    out.extend_from_slice(&SIG);
    let mut ihdr = [0u8; 13];
    ihdr[..4].copy_from_slice(&(w as u32).to_be_bytes());
    ihdr[4..8].copy_from_slice(&(h as u32).to_be_bytes());
    ihdr[8] = depth;
    ihdr[9] = ctype;
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// PNG of straight RGBA8 pixels
pub fn encode_rgba8(w: usize, h: usize, px: &[u8]) -> Vec<u8> {
    encode_raw(w, h, 4, 8, px)
}

/// 16-bit PNG of straight RGBA16 pixels
pub fn encode_rgba16(w: usize, h: usize, px: &[u16]) -> Vec<u8> {
    let mut b = Vec::with_capacity(px.len() * 2);
    for v in px {
        b.extend_from_slice(&v.to_be_bytes());
    }
    encode_raw(w, h, 4, 16, &b)
}
