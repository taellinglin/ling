use core::ptr;

pub const TAG_PATTERN: u64 = 0x7F00_0000_0000_0000;
pub const TAG_KIND_LIST: u64 = 0x0004_0000_0000_0000;
pub const TAG_KIND_STRUCT: u64 = 0x0005_0000_0000_0000;
pub const TAG_UNIT: u64 = 0x7F00_0000_0000_0000;
pub const PTR_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

#[repr(C)]
struct ListHeader {
    cap: u32,
    len: u32,
}

#[inline(always)]
fn encode_list(ptr: *const u8) -> u64 {
    TAG_PATTERN | TAG_KIND_LIST | (ptr as u64 & PTR_MASK)
}

#[inline(always)]
fn encode_struct(ptr: *const u8) -> u64 {
    TAG_PATTERN | TAG_KIND_STRUCT | (ptr as u64 & PTR_MASK)
}

#[inline(always)]
fn decode_ptr(val: u64) -> *mut u8 {
    (val & PTR_MASK) as *mut u8
}

#[no_mangle]
pub unsafe extern "C" fn ling_list_new() -> u64 {
    let initial_cap = 8;
    let size = core::mem::size_of::<ListHeader>() + initial_cap * 8;
    let p = crate::alloc::alloc(size);
    if p.is_null() {
        crate::alloc::ling_panic(0);
    }
    let hdr = p as *mut ListHeader;
    (*hdr).cap = initial_cap as u32;
    (*hdr).len = 0;
    encode_list(p)
}

#[no_mangle]
pub unsafe extern "C" fn ling_list_push(list_val: u64, elem: u64) -> u64 {
    let p = decode_ptr(list_val);
    if p.is_null() {
        return list_val;
    }
    let hdr = p as *mut ListHeader;
    let len = (*hdr).len as usize;
    let cap = (*hdr).cap as usize;

    let target_ptr = if len >= cap {
        let new_cap = cap * 2;
        let new_size = core::mem::size_of::<ListHeader>() + new_cap * 8;
        let new_p = crate::alloc::alloc(new_size);
        if new_p.is_null() {
            crate::alloc::ling_panic(0);
        }
        let new_hdr = new_p as *mut ListHeader;
        (*new_hdr).cap = new_cap as u32;
        (*new_hdr).len = (len + 1) as u32;
        let old_elems = p.add(core::mem::size_of::<ListHeader>()) as *const u64;
        let new_elems = new_p.add(core::mem::size_of::<ListHeader>()) as *mut u64;
        ptr::copy_nonoverlapping(old_elems, new_elems, len);
        *new_elems.add(len) = elem;
        crate::alloc::free(p);
        new_p
    } else {
        let elems = p.add(core::mem::size_of::<ListHeader>()) as *mut u64;
        *elems.add(len) = elem;
        (*hdr).len = (len + 1) as u32;
        p
    };

    encode_list(target_ptr)
}

#[no_mangle]
pub unsafe extern "C" fn ling_list_get(list_val: u64, idx_val: u64) -> u64 {
    let p = decode_ptr(list_val);
    if p.is_null() {
        return TAG_UNIT;
    }
    let idx = if (idx_val >> 56) == 0x7F {
        0
    } else {
        f64::from_bits(idx_val) as usize
    };
    let hdr = p as *mut ListHeader;
    if idx >= (*hdr).len as usize {
        return TAG_UNIT;
    }
    let elems = p.add(core::mem::size_of::<ListHeader>()) as *const u64;
    *elems.add(idx)
}

#[no_mangle]
pub unsafe extern "C" fn ling_list_len(list_val: u64) -> u64 {
    let p = decode_ptr(list_val);
    if p.is_null() {
        return 0;
    }
    let hdr = p as *mut ListHeader;
    ((*hdr).len as f64).to_bits()
}

#[repr(C)]
struct StructHeader {
    n_fields: u32,
}

#[no_mangle]
pub unsafe extern "C" fn ling_struct_new(
    _name: u64,
    n_fields: u64,
    keys: u64,
    vals: u64,
) -> u64 {
    let n = n_fields as usize;
    let size = core::mem::size_of::<StructHeader>() + n * 16;
    let p = crate::alloc::alloc(size);
    if p.is_null() {
        crate::alloc::ling_panic(0);
    }
    let hdr = p as *mut StructHeader;
    (*hdr).n_fields = n as u32;

    let k_ptr = keys as *const u64;
    let v_ptr = vals as *const u64;
    let dst = p.add(core::mem::size_of::<StructHeader>()) as *mut (u64, u64);
    for i in 0..n {
        let k = if k_ptr.is_null() { 0 } else { *k_ptr.add(i) };
        let v = if v_ptr.is_null() { 0 } else { *v_ptr.add(i) };
        *dst.add(i) = (k, v);
    }
    encode_struct(p)
}

#[no_mangle]
pub unsafe extern "C" fn ling_struct_get(st_val: u64, key: u64, default_val: u64) -> u64 {
    let p = decode_ptr(st_val);
    if p.is_null() {
        return default_val;
    }
    let hdr = p as *mut StructHeader;
    let n = (*hdr).n_fields as usize;
    let pairs = p.add(core::mem::size_of::<StructHeader>()) as *const (u64, u64);
    for i in 0..n {
        let (k, v) = *pairs.add(i);
        if k == key {
            return v;
        }
    }
    default_val
}
