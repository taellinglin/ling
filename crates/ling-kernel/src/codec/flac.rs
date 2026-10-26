//! FLAC decoding (the format as specified at xiph.org/flac/format.html):
//! STREAMINFO, frame headers, CONSTANT / VERBATIM / FIXED / LPC subframes
//! with Rice-coded residuals, wasted bits, and the three stereo
//! decorrelation modes. `decode` turns a whole `.flac` file into interleaved
//! 16-bit PCM. CRCs are not checked; a damaged frame ends the decode with
//! what came before it.

use alloc::vec;
use alloc::vec::Vec;

struct Bits<'a> {
    data: &'a [u8],
    pos: usize, // bit position
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8], byte: usize) -> Self {
        Bits { data, pos: byte * 8 }
    }

    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.pos >> 3)?;
        let b = (byte >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        Some(b as u32)
    }

    /// `n` (0..=32) bits, most significant first.
    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut v: u64 = 0;
        let mut left = n;
        while left > 0 {
            if self.pos & 7 == 0 && left >= 8 {
                let byte = *self.data.get(self.pos >> 3)? as u64;
                v = (v << 8) | byte;
                self.pos += 8;
                left -= 8;
            } else {
                v = (v << 1) | self.bit()? as u64;
                left -= 1;
            }
        }
        Some(v as u32)
    }

    fn signed(&mut self, n: u32) -> Option<i32> {
        if n == 0 {
            return Some(0);
        }
        let v = self.bits(n)?;
        let shift = 32 - n;
        Some(((v << shift) as i32) >> shift)
    }

    fn unary(&mut self) -> Option<u32> {
        let mut n = 0;
        while self.bit()? == 0 {
            n += 1;
            if n > 1 << 20 {
                return None;
            }
        }
        Some(n)
    }

    fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }

    fn byte_pos(&self) -> usize {
        self.pos >> 3
    }
}

struct StreamInfo {
    rate: u32,
    channels: u32,
    bps: u32,
    total: u64,
}

fn read_streaminfo(d: &[u8]) -> Option<(StreamInfo, usize)> {
    if d.len() < 8 || &d[0..4] != b"fLaC" {
        return None;
    }
    let mut off = 4;
    let mut info = None;
    loop {
        let hdr = *d.get(off)?;
        let last = hdr & 0x80 != 0;
        let kind = hdr & 0x7F;
        let len = ((*d.get(off + 1)? as usize) << 16) | ((*d.get(off + 2)? as usize) << 8) | *d.get(off + 3)? as usize;
        let body = off + 4;
        if body + len > d.len() {
            return None;
        }
        if kind == 0 && len >= 18 {
            let mut b = Bits::new(d, body);
            b.bits(16)?; // min block size
            b.bits(16)?; // max block size
            b.bits(24)?; // min frame size
            b.bits(24)?; // max frame size
            let rate = b.bits(20)?;
            let channels = b.bits(3)? + 1;
            let bps = b.bits(5)? + 1;
            let total = ((b.bits(4)? as u64) << 32) | b.bits(32)? as u64;
            info = Some(StreamInfo { rate, channels, bps, total });
        }
        off = body + len;
        if last {
            break;
        }
    }
    Some((info?, off))
}

fn residual(b: &mut Bits, order: usize, block: usize, out: &mut [i32]) -> Option<()> {
    let method = b.bits(2)?;
    let (pbits, escape) = match method {
        0 => (4, 15),
        1 => (5, 31),
        _ => return None,
    };
    let porder = b.bits(4)?;
    let parts = 1usize << porder;
    if block % parts != 0 {
        return None;
    }
    let per = block >> porder;
    let mut i = order;
    for p in 0..parts {
        let param = b.bits(pbits)?;
        let n = if p == 0 { per.checked_sub(order)? } else { per };
        if param == escape {
            let raw = b.bits(5)?;
            for _ in 0..n {
                out[i] = b.signed(raw)?;
                i += 1;
            }
        } else {
            for _ in 0..n {
                let q = b.unary()?;
                let r = if param > 0 { b.bits(param)? } else { 0 };
                let u = (q << param) | r;
                out[i] = ((u >> 1) as i32) ^ -((u & 1) as i32);
                i += 1;
            }
        }
    }
    Some(())
}

fn subframe(b: &mut Bits, bps: u32, block: usize, out: &mut [i32]) -> Option<()> {
    if b.bit()? != 0 {
        return None;
    }
    let kind = b.bits(6)?;
    let mut wasted = 0;
    if b.bit()? == 1 {
        wasted = b.unary()? + 1;
    }
    let bps = bps.checked_sub(wasted)?;
    match kind {
        0 => {
            let v = b.signed(bps)?;
            out[..block].iter_mut().for_each(|s| *s = v);
        },
        1 => {
            for s in out[..block].iter_mut() {
                *s = b.signed(bps)?;
            }
        },
        8..=12 => {
            let order = (kind - 8) as usize;
            if order > block {
                return None;
            }
            for s in out[..order].iter_mut() {
                *s = b.signed(bps)?;
            }
            residual(b, order, block, out)?;
            for i in order..block {
                let r = out[i] as i64;
                let p: i64 = match order {
                    0 => 0,
                    1 => out[i - 1] as i64,
                    2 => 2 * out[i - 1] as i64 - out[i - 2] as i64,
                    3 => 3 * out[i - 1] as i64 - 3 * out[i - 2] as i64 + out[i - 3] as i64,
                    _ => 4 * out[i - 1] as i64 - 6 * out[i - 2] as i64 + 4 * out[i - 3] as i64 - out[i - 4] as i64,
                };
                out[i] = (p + r) as i32;
            }
        },
        32..=63 => {
            let order = (kind - 31) as usize;
            if order > block {
                return None;
            }
            for s in out[..order].iter_mut() {
                *s = b.signed(bps)?;
            }
            let precision = b.bits(4)? + 1;
            if precision == 16 {
                return None; // reserved
            }
            let shift = b.signed(5)?;
            if shift < 0 {
                return None;
            }
            let mut coefs = [0i32; 32];
            for c in coefs[..order].iter_mut() {
                *c = b.signed(precision)?;
            }
            residual(b, order, block, out)?;
            for i in order..block {
                let mut sum: i64 = 0;
                for (j, &c) in coefs[..order].iter().enumerate() {
                    sum += c as i64 * out[i - 1 - j] as i64;
                }
                out[i] = ((sum >> shift) + out[i] as i64) as i32;
            }
        },
        _ => return None,
    }
    if wasted > 0 {
        out[..block].iter_mut().for_each(|s| *s <<= wasted);
    }
    Some(())
}

/// Decode a whole FLAC file: (sample rate, channels, interleaved i16).
/// More than two channels keep the first two.
pub fn decode(d: &[u8]) -> Option<(u32, u8, Vec<i16>)> {
    let (info, mut off) = read_streaminfo(d)?;
    if info.channels == 0 || info.rate == 0 {
        return None;
    }
    let out_ch = info.channels.min(2) as usize;
    let mut out: Vec<i16> = Vec::with_capacity((info.total as usize).min(1 << 28) * out_ch);
    let mut chans: Vec<Vec<i32>> = Vec::new();
    while off + 2 <= d.len() {
        // frame sync: 0xFFF8 / 0xFFF9
        if d[off] != 0xFF || d[off + 1] & 0xFE != 0xF8 {
            match d[off + 1..].windows(2).position(|w| w[0] == 0xFF && w[1] & 0xFE == 0xF8) {
                Some(p) => {
                    off += 1 + p;
                    continue;
                },
                None => break,
            }
        }
        let mut b = Bits::new(d, off + 2);
        let Some(()) = (|| -> Option<()> {
            let bs_code = b.bits(4)?;
            let sr_code = b.bits(4)?;
            let ch_code = b.bits(4)?;
            let ss_code = b.bits(3)?;
            b.bits(1)?;
            // UTF-8-style frame / sample number
            let first = b.bits(8)?;
            let extra = (first as u8).leading_ones().saturating_sub(1);
            for _ in 0..extra {
                b.bits(8)?;
            }
            let block = match bs_code {
                1 => 192,
                2..=5 => 576 << (bs_code - 2),
                6 => b.bits(8)? as usize + 1,
                7 => b.bits(16)? as usize + 1,
                8..=15 => 256 << (bs_code - 8),
                _ => return None,
            };
            match sr_code {
                12 => {
                    b.bits(8)?;
                },
                13 | 14 => {
                    b.bits(16)?;
                },
                15 => return None,
                _ => {},
            }
            b.bits(8)?; // CRC-8
            let bps = match ss_code {
                0 => info.bps,
                1 => 8,
                2 => 12,
                4 => 16,
                5 => 20,
                6 => 24,
                7 => 32,
                _ => return None,
            };
            let nch = match ch_code {
                0..=7 => ch_code as usize + 1,
                8..=10 => 2,
                _ => return None,
            };
            if block == 0 || block > 65536 {
                return None;
            }
            while chans.len() < nch {
                chans.push(Vec::new());
            }
            for c in 0..nch {
                chans[c].clear();
                chans[c].resize(block, 0);
                // the side channel carries one more bit
                let extra_bit = match (ch_code, c) {
                    (8, 1) | (9, 0) | (10, 1) => 1,
                    _ => 0,
                };
                let (lo, hi) = chans.split_at_mut(c);
                let _ = lo;
                subframe(&mut b, bps + extra_bit, block, &mut hi[0])?;
            }
            // stereo decorrelation
            match ch_code {
                8 => {
                    for i in 0..block {
                        chans[1][i] = chans[0][i] - chans[1][i];
                    }
                },
                9 => {
                    for i in 0..block {
                        chans[0][i] += chans[1][i];
                    }
                },
                10 => {
                    for i in 0..block {
                        let mid = (chans[0][i] as i64) << 1 | (chans[1][i] as i64 & 1);
                        let side = chans[1][i] as i64;
                        chans[0][i] = ((mid + side) >> 1) as i32;
                        chans[1][i] = ((mid - side) >> 1) as i32;
                    }
                },
                _ => {},
            }
            let shift_down = bps.saturating_sub(16);
            let shift_up = 16u32.saturating_sub(bps);
            let to16 = |v: i32| -> i16 {
                let v = if shift_down > 0 { v >> shift_down } else { v << shift_up };
                v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
            };
            for i in 0..block {
                out.push(to16(chans[0][i]));
                if out_ch == 2 {
                    out.push(to16(if nch > 1 { chans[1][i] } else { chans[0][i] }));
                }
            }
            b.align();
            b.bits(16)?; // CRC-16
            Some(())
        })() else {
            break;
        };
        off = b.byte_pos();
    }
    if out.is_empty() {
        return None;
    }
    let _ = vec![0u8; 0];
    Some((info.rate, out_ch as u8, out))
}
