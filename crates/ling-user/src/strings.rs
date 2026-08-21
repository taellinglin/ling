use core::ptr;

pub const TAG_PATTERN: u64 = 0x7F00_0000_0000_0000;
pub const TAG_KIND_STRING: u64 = 0x0003_0000_0000_0000;
pub const TAG_TRUE: u64 = 0x7F02_0000_0000_0000;
pub const TAG_FALSE: u64 = 0x7F01_0000_0000_0000;
pub const PTR_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

#[inline(always)]
pub fn encode_str(ptr: *const u8) -> u64 {
    TAG_PATTERN | TAG_KIND_STRING | (ptr as u64 & PTR_MASK)
}

#[inline(always)]
pub fn decode_ptr(val: u64) -> *mut u8 {
    (val & PTR_MASK) as *mut u8
}

#[inline(always)]
pub unsafe fn header_len(ptr: *const u8) -> usize {
    u32::from_le_bytes([*ptr, *ptr.add(1), *ptr.add(2), *ptr.add(3)]) as usize
}

#[inline(always)]
pub unsafe fn data_ptr(ptr: *mut u8) -> *mut u8 {
    ptr.add(4)
}

pub unsafe fn bytes_of(val: u64) -> &'static [u8] {
    bytes_of_ptr(decode_ptr(val))
}

pub unsafe fn bytes_of_ptr(p: *mut u8) -> &'static [u8] {
    if p.is_null() {
        return &[];
    }
    let len = header_len(p);
    core::slice::from_raw_parts(data_ptr(p), len)
}

pub fn alloc_string(len: usize) -> *mut u8 {
    let buf = crate::alloc::alloc(4 + len);
    if buf.is_null() {
        unsafe { crate::alloc::ling_panic(0) };
    }
    let len_bytes = (len as u32).to_le_bytes();
    unsafe { ptr::copy_nonoverlapping(len_bytes.as_ptr(), buf, 4) };
    buf
}

#[no_mangle]
pub unsafe extern "C" fn ling_str_new(bytes: *const u8, len: usize) -> u64 {
    let buf = alloc_string(len);
    if len > 0 && !bytes.is_null() {
        ptr::copy_nonoverlapping(bytes, data_ptr(buf), len);
    }
    encode_str(buf)
}

#[no_mangle]
pub unsafe extern "C" fn ling_str_len(val: u64) -> u64 {
    let p = decode_ptr(val);
    let len = if p.is_null() { 0 } else { header_len(p) };
    (len as f64).to_bits()
}

#[no_mangle]
pub unsafe extern "C" fn ling_str_concat(a: u64, b: u64) -> u64 {
    let (pa, pb) = (decode_ptr(a), decode_ptr(b));
    let (la, lb) = (header_len(pa), header_len(pb));
    let buf = alloc_string(la + lb);
    ptr::copy_nonoverlapping(data_ptr(pa), data_ptr(buf), la);
    ptr::copy_nonoverlapping(data_ptr(pb), data_ptr(buf).add(la), lb);
    encode_str(buf)
}

#[no_mangle]
pub unsafe extern "C" fn ling_str_eq(a: u64, b: u64) -> u64 {
    let eq = bytes_of(a) == bytes_of(b);
    if eq { TAG_TRUE } else { TAG_FALSE }
}
