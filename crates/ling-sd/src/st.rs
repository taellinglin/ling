//! The safetensors container: an 8-byte little-endian header length, a JSON header
//! `{"name": {"dtype": "F16", "shape": [..], "data_offsets": [begin, end]}, "__metadata__": {..}}`, then the data.
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Random-access bytes: a file on the host, a file on a FAT volume in LingOS.
pub trait Source {
    fn len(&self) -> u64;
    /// Fill `buf` from `offset`; false on an I/O error or a range past the end.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DType {
    F16,
    BF16,
    F32,
    Other,
}

impl DType {
    pub fn size(self) -> usize {
        match self {
            DType::F16 | DType::BF16 => 2,
            DType::F32 => 4,
            DType::Other => 1,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Info {
    pub dtype: DType,
    pub shape: Vec<usize>,
    /// absolute offset in the file
    pub at: u64,
    pub bytes: u64,
}

impl Info {
    pub fn numel(&self) -> usize {
        self.shape.iter().product::<usize>().max(1)
    }
}

pub struct SafeTensors {
    pub tensors: BTreeMap<String, Info>,
}

impl SafeTensors {
    pub fn parse(src: &dyn Source) -> Result<SafeTensors, crate::Error> {
        let mut n = [0u8; 8];
        if src.len() < 16 || !src.read_at(0, &mut n) {
            return Err("not a safetensors file (too short)".to_string());
        }
        let n = u64::from_le_bytes(n);
        if n < 2 || n > 64 << 20 || 8 + n > src.len() {
            return Err("not a safetensors file (bad header length)".to_string());
        }
        let mut hdr = alloc::vec![0u8; n as usize];
        if !src.read_at(8, &mut hdr) {
            return Err("could not read the safetensors header".to_string());
        }
        let base = 8 + n;
        let mut p = Json { s: &hdr, i: 0 };
        let mut tensors = BTreeMap::new();
        p.ws();
        p.expect(b'{')?;
        loop {
            p.ws();
            if p.peek() == Some(b'}') {
                break;
            }
            let name = p.string()?;
            p.ws();
            p.expect(b':')?;
            p.ws();
            if name == "__metadata__" {
                p.skip_value()?;
            } else {
                let mut dtype = DType::Other;
                let mut shape = Vec::new();
                let mut offs = (0u64, 0u64);
                p.expect(b'{')?;
                loop {
                    p.ws();
                    if p.peek() == Some(b'}') {
                        p.i += 1;
                        break;
                    }
                    let key = p.string()?;
                    p.ws();
                    p.expect(b':')?;
                    p.ws();
                    match key.as_str() {
                        "dtype" => {
                            dtype = match p.string()?.as_str() {
                                "F16" => DType::F16,
                                "BF16" => DType::BF16,
                                "F32" => DType::F32,
                                _ => DType::Other,
                            }
                        },
                        "shape" => shape = p.numbers()?.into_iter().map(|v| v as usize).collect(),
                        "data_offsets" => {
                            let v = p.numbers()?;
                            if v.len() == 2 {
                                offs = (v[0], v[1]);
                            }
                        },
                        _ => p.skip_value()?,
                    }
                    p.ws();
                    if p.peek() == Some(b',') {
                        p.i += 1;
                    }
                }
                if offs.1 < offs.0 || base + offs.1 > src.len() {
                    return Err(alloc::format!("tensor {name} lies outside the file (is the file complete?)"));
                }
                tensors.insert(name, Info { dtype, shape, at: base + offs.0, bytes: offs.1 - offs.0 });
            }
            p.ws();
            match p.peek() {
                Some(b',') => p.i += 1,
                Some(b'}') => break,
                _ => return Err("bad safetensors header".to_string()),
            }
        }
        Ok(SafeTensors { tensors })
    }

    pub fn get(&self, name: &str) -> Option<&Info> {
        self.tensors.get(name)
    }
}

struct Json<'a> {
    s: &'a [u8],
    i: usize,
}

impl Json<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.i += 1;
        }
    }
    fn expect(&mut self, c: u8) -> Result<(), crate::Error> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(alloc::format!("bad safetensors header at byte {}", self.i))
        }
    }
    fn string(&mut self) -> Result<String, crate::Error> {
        self.expect(b'"')?;
        let mut out: Vec<u8> = Vec::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                b'"' => return Ok(String::from_utf8_lossy(&out).into_owned()),
                b'\\' => {
                    let e = self.peek().unwrap_or(b'"');
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let h = core::str::from_utf8(self.s.get(self.i..self.i + 4).unwrap_or(b"003f")).unwrap_or("003f");
                            let cp = u32::from_str_radix(h, 16).unwrap_or(0x3f);
                            self.i += 4;
                            let ch = char::from_u32(cp).unwrap_or('?');
                            let mut b = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
                        },
                        x => out.push(x),
                    }
                },
                x => out.push(x),
            }
        }
        Err("unterminated string in the safetensors header".to_string())
    }
    fn numbers(&mut self) -> Result<Vec<u64>, crate::Error> {
        self.expect(b'[')?;
        let mut v = Vec::new();
        loop {
            self.ws();
            match self.peek() {
                Some(b']') => {
                    self.i += 1;
                    return Ok(v);
                },
                Some(b',') => self.i += 1,
                Some(c) if c.is_ascii_digit() => {
                    let mut n = 0u64;
                    while let Some(d) = self.peek().filter(|d| d.is_ascii_digit()) {
                        n = n * 10 + (d - b'0') as u64;
                        self.i += 1;
                    }
                    v.push(n);
                },
                _ => return Err("bad number list in the safetensors header".to_string()),
            }
        }
    }
    fn skip_value(&mut self) -> Result<(), crate::Error> {
        self.ws();
        match self.peek() {
            Some(b'"') => {
                self.string()?;
            },
            Some(b'{') | Some(b'[') => {
                let mut depth = 0i32;
                while let Some(c) = self.peek() {
                    match c {
                        b'"' => {
                            self.string()?;
                            continue;
                        },
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                self.i += 1;
                                return Ok(());
                            }
                        },
                        _ => {},
                    }
                    self.i += 1;
                }
            },
            _ => {
                while let Some(c) = self.peek() {
                    if matches!(c, b',' | b'}' | b']') {
                        break;
                    }
                    self.i += 1;
                }
            },
        }
        Ok(())
    }
}
