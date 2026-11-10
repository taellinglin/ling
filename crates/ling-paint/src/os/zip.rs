//! Zip archives for .lanime projects in the no_std build: reads stored / deflated entries (zip64 sizes too, as the
//! desktop's `zip` crate writes them with `large_file`) and writes stored / deflated entries.

use alloc::string::String;
use alloc::vec::Vec;

use super::crc32;

fn le16(b: &[u8], i: usize) -> u64 {
    u16::from_le_bytes([b[i], b[i + 1]]) as u64
}
fn le32(b: &[u8], i: usize) -> u64 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as u64
}
fn le64(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3], b[i + 4], b[i + 5], b[i + 6], b[i + 7]])
}

struct Entry {
    name: String,
    method: u16,
    csize: u64,
    local: u64,
}

pub struct Reader<'a> {
    data: &'a [u8],
    entries: Vec<Entry>,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Result<Reader<'a>, String> {
        // the end-of-central-directory record, searched backwards (a comment may follow it)
        let n = data.len();
        if n < 22 {
            return Err("not a zip archive".into());
        }
        let lo = n.saturating_sub(22 + 65535);
        let mut eocd = None;
        let mut i = n - 22;
        loop {
            if data[i..i + 4] == [0x50, 0x4b, 0x05, 0x06] {
                eocd = Some(i);
                break;
            }
            if i == lo {
                break;
            }
            i -= 1;
        }
        let e = eocd.ok_or("not a zip archive")?;
        let mut count = le16(data, e + 10);
        let mut cd = le32(data, e + 16);
        // zip64 end of central directory, through its locator just before
        if (count == 0xFFFF || cd == 0xFFFF_FFFF) && e >= 20 && data[e - 20..e - 16] == [0x50, 0x4b, 0x06, 0x07] {
            let z = le64(data, e - 20 + 8) as usize;
            if z + 56 <= n && data[z..z + 4] == [0x50, 0x4b, 0x06, 0x06] {
                count = le64(data, z + 32);
                cd = le64(data, z + 48);
            }
        }
        let mut entries = Vec::new();
        let mut p = cd as usize;
        for _ in 0..count {
            if p + 46 > n || data[p..p + 4] != [0x50, 0x4b, 0x01, 0x02] {
                break;
            }
            let method = le16(data, p + 10) as u16;
            let mut csize = le32(data, p + 20);
            let usize_ = le32(data, p + 24);
            let nlen = le16(data, p + 28) as usize;
            let xlen = le16(data, p + 30) as usize;
            let clen = le16(data, p + 32) as usize;
            let mut local = le32(data, p + 42);
            let name = String::from_utf8_lossy(&data[p + 46..(p + 46 + nlen).min(n)]).into_owned();
            // zip64 extra: the fields that read 0xFFFFFFFF, in order: size, compressed size, local offset
            let mut x = p + 46 + nlen;
            let xend = (x + xlen).min(n);
            while x + 4 <= xend {
                let id = le16(data, x);
                let sz = le16(data, x + 2) as usize;
                if id == 1 {
                    let mut q = x + 4;
                    if usize_ == 0xFFFF_FFFF && q + 8 <= xend {
                        q += 8;
                    }
                    if csize == 0xFFFF_FFFF && q + 8 <= xend {
                        csize = le64(data, q);
                        q += 8;
                    }
                    if local == 0xFFFF_FFFF && q + 8 <= xend {
                        local = le64(data, q);
                    }
                }
                x += 4 + sz;
            }
            entries.push(Entry { name, method, csize, local });
            p += 46 + nlen + xlen + clen;
        }
        Ok(Reader { data, entries })
    }

    pub fn names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.name.as_str()).collect()
    }

    /// the bytes of `name`, None when there is no such entry
    pub fn read(&self, name: &str) -> Option<Result<Vec<u8>, String>> {
        let e = self.entries.iter().find(|e| e.name == name)?;
        let d = self.data;
        let l = e.local as usize;
        if l + 30 > d.len() || d[l..l + 4] != [0x50, 0x4b, 0x03, 0x04] {
            return Some(Err(format!("{name}: broken zip entry")));
        }
        let start = l + 30 + le16(d, l + 26) as usize + le16(d, l + 28) as usize;
        let end = start.saturating_add(e.csize as usize);
        let Some(body) = d.get(start..end) else { return Some(Err(format!("{name}: truncated zip entry"))) };
        Some(match e.method {
            0 => Ok(body.to_vec()),
            8 => miniz_oxide::inflate::decompress_to_vec(body).map_err(|_| format!("{name}: broken deflate data")),
            m => Err(format!("{name}: zip compression method {m} is not supported")),
        })
    }
}

/// an archive built in memory
pub struct Writer {
    out: Vec<u8>,
    cd: Vec<u8>,
    count: u16,
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

impl Writer {
    pub fn new() -> Writer {
        Writer { out: Vec::new(), cd: Vec::new(), count: 0 }
    }
    /// add `data` as `name`, deflated when `deflate`
    pub fn add(&mut self, name: &str, data: &[u8], deflate: bool) {
        let crc = crc32(data);
        let packed;
        let (method, body): (u16, &[u8]) = if deflate {
            packed = miniz_oxide::deflate::compress_to_vec(data, 6);
            (8, &packed)
        } else {
            (0, data)
        };
        let local = self.out.len() as u32;
        let nm = name.as_bytes();
        let mut h = Vec::with_capacity(30 + nm.len());
        h.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
        h.extend_from_slice(&20u16.to_le_bytes()); // version needed
        h.extend_from_slice(&0x0800u16.to_le_bytes()); // utf-8 names
        h.extend_from_slice(&method.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes()); // time
        h.extend_from_slice(&0x21u16.to_le_bytes()); // date: 1980-01-01
        h.extend_from_slice(&crc.to_le_bytes());
        h.extend_from_slice(&(body.len() as u32).to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(nm.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(nm);
        self.out.extend_from_slice(&h);
        self.out.extend_from_slice(body);
        let c = &mut self.cd;
        c.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
        c.extend_from_slice(&20u16.to_le_bytes()); // made by
        c.extend_from_slice(&20u16.to_le_bytes());
        c.extend_from_slice(&0x0800u16.to_le_bytes());
        c.extend_from_slice(&method.to_le_bytes());
        c.extend_from_slice(&0u16.to_le_bytes());
        c.extend_from_slice(&0x21u16.to_le_bytes());
        c.extend_from_slice(&crc.to_le_bytes());
        c.extend_from_slice(&(body.len() as u32).to_le_bytes());
        c.extend_from_slice(&(data.len() as u32).to_le_bytes());
        c.extend_from_slice(&(nm.len() as u16).to_le_bytes());
        c.extend_from_slice(&[0u8; 8]); // extra, comment, disk, internal attributes
        c.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        c.extend_from_slice(&local.to_le_bytes());
        c.extend_from_slice(nm);
        self.count += 1;
    }
    pub fn finish(mut self) -> Vec<u8> {
        let cd_at = self.out.len() as u32;
        let cd_len = self.cd.len() as u32;
        let cd = core::mem::take(&mut self.cd);
        self.out.extend_from_slice(&cd);
        let o = &mut self.out;
        o.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        o.extend_from_slice(&[0u8; 4]);
        o.extend_from_slice(&self.count.to_le_bytes());
        o.extend_from_slice(&self.count.to_le_bytes());
        o.extend_from_slice(&cd_len.to_le_bytes());
        o.extend_from_slice(&cd_at.to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        self.out
    }
}
