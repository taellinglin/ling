//! Wire framing: 4-byte big-endian length-prefixed frames, plus the
//! `u16`-prefixed field encoding used inside handshake messages.

use std::io::{self, Read, Write};

pub const MAX_FRAME_LEN: u32 = 64 * 1024 * 1024;

pub fn write_frame<W: Write>(w: &mut W, payload: &[u8]) -> io::Result<()> {
    if payload.len() as u64 > MAX_FRAME_LEN as u64 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "lingtp frame too large"));
    }
    w.write_all(&(payload.len() as u32).to_be_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

pub fn read_frame<R: Read>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_LEN {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "lingtp frame too large"));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

pub fn write_u16_prefixed(buf: &mut Vec<u8>, data: &[u8]) {
    buf.extend_from_slice(&(data.len() as u16).to_be_bytes());
    buf.extend_from_slice(data);
}

pub fn take_fixed(buf: &[u8], pos: &mut usize, n: usize) -> io::Result<Vec<u8>> {
    if *pos + n > buf.len() {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "lingtp: buffer too short"));
    }
    let out = buf[*pos..*pos + n].to_vec();
    *pos += n;
    Ok(out)
}

pub fn take_u16_prefixed(buf: &[u8], pos: &mut usize) -> io::Result<Vec<u8>> {
    let len_bytes = take_fixed(buf, pos, 2)?;
    let len = u16::from_be_bytes([len_bytes[0], len_bytes[1]]) as usize;
    take_fixed(buf, pos, len)
}
