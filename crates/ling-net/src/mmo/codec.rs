//! Compact little-endian binary codec for MMO wire messages.
//!
//! The MMO layer avoids serde on the hot path: every packet is hand-encoded
//! through [`ByteWriter`] / [`ByteReader`] so the wire format is deterministic,
//! allocation-light, and identical across platforms (always little-endian).
//!
//! Determinism matters because [`crate::mmo::verify`] hashes the encoded bytes
//! of a snapshot to produce a cryptographic commitment — two peers must encode
//! the same logical state to the same bytes.

use glam::{Quat, Vec3};

/// Hard cap on any length-prefixed blob, to bound attacker-controlled
/// allocations from a malformed datagram.
pub const MAX_BLOB: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// Reader ran past the end of the buffer.
    UnexpectedEof,
    /// An enum tag byte did not match any known variant.
    BadTag(u8),
    /// A length-prefixed blob exceeded [`MAX_BLOB`].
    TooLarge(usize),
    /// A string field was not valid UTF-8.
    BadUtf8,
    /// Trailing bytes remained after a message that should have been fully consumed.
    TrailingBytes(usize),
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedEof    => write!(f, "unexpected end of buffer"),
            Self::BadTag(t)        => write!(f, "unknown tag byte {t}"),
            Self::TooLarge(n)      => write!(f, "blob length {n} exceeds MAX_BLOB"),
            Self::BadUtf8          => write!(f, "invalid utf-8 in string field"),
            Self::TrailingBytes(n) => write!(f, "{n} trailing bytes after message"),
        }
    }
}

impl std::error::Error for CodecError {}

// ─── Writer ─────────────────────────────────────────────────────────────────

/// Append-only little-endian byte writer.
#[derive(Default, Clone)]
pub struct ByteWriter {
    buf: Vec<u8>,
}

macro_rules! write_num {
    ($name:ident, $ty:ty) => {
        #[inline]
        pub fn $name(&mut self, v: $ty) -> &mut Self {
            self.buf.extend_from_slice(&v.to_le_bytes());
            self
        }
    };
}

impl ByteWriter {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self { buf: Vec::with_capacity(cap) }
    }

    write_num!(u8, u8);
    write_num!(i8, i8);
    write_num!(u16, u16);
    write_num!(i16, i16);
    write_num!(u32, u32);
    write_num!(i32, i32);
    write_num!(u64, u64);
    write_num!(i64, i64);
    write_num!(f32, f32);
    write_num!(f64, f64);

    #[inline]
    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(v as u8)
    }

    /// Raw bytes, no length prefix.
    #[inline]
    pub fn raw(&mut self, b: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self
    }

    /// Fixed-size byte array, no length prefix (e.g. 32-byte key, 64-byte sig).
    #[inline]
    pub fn fixed<const N: usize>(&mut self, b: &[u8; N]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self
    }

    /// Length-prefixed (u32) blob.
    #[inline]
    pub fn blob(&mut self, b: &[u8]) -> &mut Self {
        self.u32(b.len() as u32).raw(b)
    }

    /// Length-prefixed UTF-8 string.
    #[inline]
    pub fn str(&mut self, s: &str) -> &mut Self {
        self.blob(s.as_bytes())
    }

    #[inline]
    pub fn vec3(&mut self, v: Vec3) -> &mut Self {
        self.f32(v.x).f32(v.y).f32(v.z)
    }

    #[inline]
    pub fn quat(&mut self, q: Quat) -> &mut Self {
        self.f32(q.x).f32(q.y).f32(q.z).f32(q.w)
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

// ─── Reader ─────────────────────────────────────────────────────────────────

/// Cursor over a borrowed byte slice.
pub struct ByteReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

macro_rules! read_num {
    ($name:ident, $ty:ty, $n:literal) => {
        #[inline]
        pub fn $name(&mut self) -> Result<$ty, CodecError> {
            let b = self.take($n)?;
            let mut arr = [0u8; $n];
            arr.copy_from_slice(b);
            Ok(<$ty>::from_le_bytes(arr))
        }
    };
}

impl<'a> ByteReader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    #[inline]
    fn take(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        let end = self.pos.checked_add(n).ok_or(CodecError::UnexpectedEof)?;
        if end > self.buf.len() {
            return Err(CodecError::UnexpectedEof);
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    read_num!(u8, u8, 1);
    read_num!(i8, i8, 1);
    read_num!(u16, u16, 2);
    read_num!(i16, i16, 2);
    read_num!(u32, u32, 4);
    read_num!(i32, i32, 4);
    read_num!(u64, u64, 8);
    read_num!(i64, i64, 8);
    read_num!(f32, f32, 4);
    read_num!(f64, f64, 8);

    #[inline]
    pub fn bool(&mut self) -> Result<bool, CodecError> {
        Ok(self.u8()? != 0)
    }

    #[inline]
    pub fn fixed<const N: usize>(&mut self) -> Result<[u8; N], CodecError> {
        let s = self.take(N)?;
        let mut arr = [0u8; N];
        arr.copy_from_slice(s);
        Ok(arr)
    }

    #[inline]
    pub fn blob(&mut self) -> Result<&'a [u8], CodecError> {
        let len = self.u32()? as usize;
        if len > MAX_BLOB {
            return Err(CodecError::TooLarge(len));
        }
        self.take(len)
    }

    #[inline]
    pub fn str(&mut self) -> Result<String, CodecError> {
        let b = self.blob()?;
        std::str::from_utf8(b)
            .map(str::to_string)
            .map_err(|_| CodecError::BadUtf8)
    }

    #[inline]
    pub fn vec3(&mut self) -> Result<Vec3, CodecError> {
        Ok(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }

    #[inline]
    pub fn quat(&mut self) -> Result<Quat, CodecError> {
        Ok(Quat::from_xyzw(self.f32()?, self.f32()?, self.f32()?, self.f32()?))
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Error unless the buffer is fully consumed — catches truncation/overrun bugs.
    pub fn expect_end(&self) -> Result<(), CodecError> {
        match self.remaining() {
            0 => Ok(()),
            n => Err(CodecError::TrailingBytes(n)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_primitives() {
        let mut w = ByteWriter::new();
        w.u8(0xAB).i16(-1234).u32(0xDEAD_BEEF).u64(1 << 40).f32(1.5).f64(-2.5).bool(true);
        w.str("héllo").vec3(Vec3::new(1.0, 2.0, 3.0)).quat(Quat::IDENTITY);
        let bytes = w.finish();

        let mut r = ByteReader::new(&bytes);
        assert_eq!(r.u8().unwrap(), 0xAB);
        assert_eq!(r.i16().unwrap(), -1234);
        assert_eq!(r.u32().unwrap(), 0xDEAD_BEEF);
        assert_eq!(r.u64().unwrap(), 1 << 40);
        assert_eq!(r.f32().unwrap(), 1.5);
        assert_eq!(r.f64().unwrap(), -2.5);
        assert!(r.bool().unwrap());
        assert_eq!(r.str().unwrap(), "héllo");
        assert_eq!(r.vec3().unwrap(), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(r.quat().unwrap(), Quat::IDENTITY);
        assert!(r.expect_end().is_ok());
    }

    #[test]
    fn eof_is_an_error_not_a_panic() {
        let mut r = ByteReader::new(&[0x01, 0x02]);
        assert_eq!(r.u32(), Err(CodecError::UnexpectedEof));
    }

    #[test]
    fn oversized_blob_rejected() {
        let mut w = ByteWriter::new();
        w.u32((MAX_BLOB + 1) as u32);
        let bytes = w.finish();
        let mut r = ByteReader::new(&bytes);
        assert!(matches!(r.blob(), Err(CodecError::TooLarge(_))));
    }
}
