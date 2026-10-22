//! The GGUF container: a header, a table of metadata (hyper-parameters, the tokenizer's vocabulary and merges), a table of tensors, then
//! the tensor data. It is read through [`Source`], so it works on a file, a disk image, or bytes in memory.
use crate::error::Error;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// Anything the model can be read from: random access to bytes.
pub trait Source {
    fn len(&self) -> u64;
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error>;
}

/// A model held in memory.
impl Source for &[u8] {
    fn len(&self) -> u64 {
        (**self).len() as u64
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let a = offset as usize;
        let b = a.checked_add(buf.len()).ok_or(Error::Io)?;
        if b > (**self).len() {
            return Err(Error::Io);
        }
        buf.copy_from_slice(&self[a..b]);
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    U64(u64),
    I64(i64),
    F32(f32),
    F64(f64),
    Bool(bool),
    Str(String),
    Arr(Vec<Value>),
}

impl Value {
    pub fn as_u64(&self) -> Option<u64> {
        Some(match self {
            Value::U8(x) => *x as u64,
            Value::U16(x) => *x as u64,
            Value::U32(x) => *x as u64,
            Value::U64(x) => *x,
            Value::I8(x) if *x >= 0 => *x as u64,
            Value::I16(x) if *x >= 0 => *x as u64,
            Value::I32(x) if *x >= 0 => *x as u64,
            Value::I64(x) if *x >= 0 => *x as u64,
            _ => return None,
        })
    }
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Value::F32(x) => Some(*x),
            Value::F64(x) => Some(*x as f32),
            _ => self.as_u64().map(|x| x as f32),
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// The tensor element types this engine reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DType {
    F32,
    F16,
    Q8_0,
    Q4K,
    Q6K,
}

impl DType {
    pub fn from_id(id: u32) -> Result<DType, Error> {
        Ok(match id {
            0 => DType::F32,
            1 => DType::F16,
            8 => DType::Q8_0,
            12 => DType::Q4K,
            14 => DType::Q6K,
            other => return Err(Error::Unsupported(alloc::format!("tensor type {other} (this engine reads F32, F16, Q8_0, Q4_K and Q6_K)"))),
        })
    }
    /// elements per block
    pub fn block(self) -> usize {
        match self {
            DType::F32 | DType::F16 => 1,
            DType::Q8_0 => 32,
            DType::Q4K | DType::Q6K => 256,
        }
    }
    /// bytes per block
    pub fn block_bytes(self) -> usize {
        match self {
            DType::F32 => 4,
            DType::F16 => 2,
            DType::Q8_0 => 34,
            DType::Q4K => 144,
            DType::Q6K => 210,
        }
    }
    /// bytes of one row of `cols` elements
    pub fn row_bytes(self, cols: usize) -> usize {
        cols / self.block() * self.block_bytes()
    }
}

#[derive(Clone, Debug)]
pub struct TensorInfo {
    pub name: String,
    /// innermost dimension first, as in the file
    pub dims: Vec<u64>,
    /// the type this engine reads it as, `None` for one it cannot (only an error if the model needs that tensor)
    pub dtype: Option<DType>,
    /// the file's own type id
    pub type_id: u32,
    /// from the start of the data section
    pub offset: u64,
}

impl TensorInfo {
    /// the element type, or the error that says which tensor type is not supported
    pub fn dtype_or_err(&self) -> Result<DType, Error> {
        self.dtype.ok_or_else(|| Error::Unsupported(alloc::format!("tensor {} has type {} (this engine reads F32, F16, Q8_0, Q4_K and Q6_K)", self.name, self.type_id)))
    }
    /// (rows, cols) of a matrix (or (1, n) of a vector)
    pub fn shape2(&self) -> (usize, usize) {
        match self.dims.len() {
            0 => (1, 1),
            1 => (1, self.dims[0] as usize),
            _ => (self.dims[1..].iter().product::<u64>() as usize, self.dims[0] as usize),
        }
    }
}

pub struct Gguf {
    pub version: u32,
    pub kv: Vec<(String, Value)>,
    pub tensors: Vec<TensorInfo>,
    /// absolute offset of the data section
    pub data_start: u64,
}

/// a small read-ahead cache over the source: the header is read in thousands of tiny pieces
struct Rd<'a> {
    src: &'a dyn Source,
    pos: u64,
    buf: Vec<u8>,
    start: u64,
}

impl<'a> Rd<'a> {
    fn new(src: &'a dyn Source) -> Self {
        Rd { src, pos: 0, buf: Vec::new(), start: 0 }
    }
    fn fill(&mut self) -> Result<(), Error> {
        let n = (self.src.len().saturating_sub(self.pos)).min(1 << 16) as usize;
        self.buf = vec![0u8; n];
        self.start = self.pos;
        if n > 0 {
            self.src.read_at(self.pos, &mut self.buf)?;
        }
        Ok(())
    }
    fn read(&mut self, out: &mut [u8]) -> Result<(), Error> {
        let mut done = 0;
        while done < out.len() {
            let have_from = self.pos.wrapping_sub(self.start);
            if self.pos < self.start || have_from as usize >= self.buf.len() {
                self.fill()?;
                if self.buf.is_empty() {
                    return Err(Error::Io);
                }
                continue;
            }
            let off = (self.pos - self.start) as usize;
            let n = (self.buf.len() - off).min(out.len() - done);
            out[done..done + n].copy_from_slice(&self.buf[off..off + n]);
            done += n;
            self.pos += n as u64;
        }
        Ok(())
    }
    fn u8(&mut self) -> Result<u8, Error> {
        let mut b = [0u8; 1];
        self.read(&mut b)?;
        Ok(b[0])
    }
    fn u16(&mut self) -> Result<u16, Error> {
        let mut b = [0u8; 2];
        self.read(&mut b)?;
        Ok(u16::from_le_bytes(b))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        let mut b = [0u8; 4];
        self.read(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        let mut b = [0u8; 8];
        self.read(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }
    fn string(&mut self) -> Result<String, Error> {
        let n = self.u64()?;
        if n > (1 << 26) {
            return Err(Error::Format("a string is absurdly long"));
        }
        let mut v = vec![0u8; n as usize];
        self.read(&mut v)?;
        Ok(String::from_utf8_lossy(&v).to_string())
    }
    fn value(&mut self, ty: u32, depth: u32) -> Result<Value, Error> {
        Ok(match ty {
            0 => Value::U8(self.u8()?),
            1 => Value::I8(self.u8()? as i8),
            2 => Value::U16(self.u16()?),
            3 => Value::I16(self.u16()? as i16),
            4 => Value::U32(self.u32()?),
            5 => Value::I32(self.u32()? as i32),
            6 => Value::F32(f32::from_bits(self.u32()?)),
            7 => Value::Bool(self.u8()? != 0),
            8 => Value::Str(self.string()?),
            9 => {
                if depth > 2 {
                    return Err(Error::Format("arrays nested too deep"));
                }
                let et = self.u32()?;
                let n = self.u64()?;
                if n > (1 << 24) {
                    return Err(Error::Format("an array is absurdly long"));
                }
                let mut v = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    v.push(self.value(et, depth + 1)?);
                }
                Value::Arr(v)
            }
            10 => Value::U64(self.u64()?),
            11 => Value::I64(self.u64()? as i64),
            12 => Value::F64(f64::from_bits(self.u64()?)),
            _ => return Err(Error::Format("unknown metadata value type")),
        })
    }
}

impl Gguf {
    pub fn parse(src: &dyn Source) -> Result<Gguf, Error> {
        let mut r = Rd::new(src);
        let mut magic = [0u8; 4];
        r.read(&mut magic)?;
        if &magic != b"GGUF" {
            return Err(Error::Format("not a GGUF file (no GGUF magic)"));
        }
        let version = r.u32()?;
        if !(2..=3).contains(&version) {
            return Err(Error::Unsupported(alloc::format!("GGUF version {version}")));
        }
        let n_tensors = r.u64()?;
        let n_kv = r.u64()?;
        if n_tensors > 100_000 || n_kv > 100_000 {
            return Err(Error::Format("implausible tensor or metadata count"));
        }
        let mut kv = Vec::with_capacity(n_kv as usize);
        for _ in 0..n_kv {
            let key = r.string()?;
            let ty = r.u32()?;
            let v = r.value(ty, 0)?;
            kv.push((key, v));
        }
        let mut tensors = Vec::with_capacity(n_tensors as usize);
        for _ in 0..n_tensors {
            let name = r.string()?;
            let nd = r.u32()?;
            if nd > 8 {
                return Err(Error::Format("a tensor has too many dimensions"));
            }
            let mut dims = Vec::new();
            for _ in 0..nd {
                dims.push(r.u64()?);
            }
            let ty = r.u32()?;
            let offset = r.u64()?;
            // an unsupported type is only an error if the model actually asks for that tensor
            let dtype = DType::from_id(ty).ok();
            tensors.push(TensorInfo { name, dims, dtype, type_id: ty, offset });
        }
        let align = kv.iter().find(|(k, _)| k == "general.alignment").and_then(|(_, v)| v.as_u64()).unwrap_or(32).max(1);
        let data_start = r.pos.div_ceil(align) * align;
        Ok(Gguf { version, kv, tensors, data_start })
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.kv.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.get(key).and_then(|v| v.as_u64())
    }
    pub fn get_f32(&self, key: &str) -> Option<f32> {
        self.get(key).and_then(|v| v.as_f32())
    }
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(|v| v.as_str())
    }
    pub fn tensor(&self, name: &str) -> Option<&TensorInfo> {
        self.tensors.iter().find(|t| t.name == name)
    }
}
