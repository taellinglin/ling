use std::sync::OnceLock;

// ─── NaN-boxed Value Encoding ───────────────────────────────────────────────
//
// u64 representation of Ling values using NaN-boxing:
//
//   If (val >> 48) == 0x7F: tagged special value
//     - bits 63-56 = 0x7F (NaN sentinel)
//     - bits 55-48 = discriminator (0xF5 = general tag, 0x00-0x09 = kind)
//     - bits  0-47 = payload (pointer or discriminator data)
//   Else: raw f64 number (bitcast)
//
// Tags:
pub const TAG_UNIT: u64 = 0x7F00_0000_0000_0000;
pub const TAG_FALSE: u64 = 0x7F01_0000_0000_0000;
pub const TAG_TRUE: u64 = 0x7F02_0000_0000_0000;
pub const TAG_MASK: u64 = 0xFFFF_0000_0000_0000;
pub const TAG_PATTERN: u64 = 0x7F00_0000_0000_0000;
pub const PTR_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

// The tag discriminator occupies bits 48-55, above the 48-bit pointer area.
// Bits 56-63 hold the 0x7F NaN sentinel.
pub const TAG_KIND_UNIT: u64 = 0x0000_0000_0000_0000;
pub const TAG_KIND_BOOL_FALSE: u64 = 0x0001_0000_0000_0000;
pub const TAG_KIND_BOOL_TRUE: u64 = 0x0002_0000_0000_0000;
// For heap-allocated types, the tag kind identifies the variant:
pub const TAG_KIND_STRING: u64 = 0x0003_0000_0000_0000;
pub const TAG_KIND_LIST: u64 = 0x0004_0000_0000_0000;
pub const TAG_KIND_STRUCT: u64 = 0x0005_0000_0000_0000;
pub const TAG_KIND_VARIANT: u64 = 0x0006_0000_0000_0000;
pub const TAG_KIND_CLOSURE: u64 = 0x0007_0000_0000_0000;
pub const TAG_KIND_OK: u64 = 0x0008_0000_0000_0000;
pub const TAG_KIND_ERR: u64 = 0x0009_0000_0000_0000;

// ─── Builtin Dispatch ──────────────────────────────────────────────────────

/// Signature of the builtin dispatch function provided by the host `ling` crate.
/// Receives builtin name (ptr+len), args array (ptr+len), returns encoded Value.
pub type BuiltinDispatch = unsafe extern "C" fn(
    name_ptr: *const u8,
    name_len: usize,
    args_ptr: *const u64,
    args_len: usize,
) -> u64;

static BUILTIN_DISPATCH: OnceLock<BuiltinDispatch> = OnceLock::new();

pub fn register_builtin_dispatch(f: BuiltinDispatch) {
    BUILTIN_DISPATCH.set(f).ok();
}

/// Call a builtin by name from JIT'd code.
#[inline]
pub unsafe fn call_builtin(name: &str, args: &[u64]) -> u64 {
    let dispatch = BUILTIN_DISPATCH.get().expect("BUILTIN_DISPATCH not registered");
    dispatch(name.as_ptr(), name.len(), args.as_ptr(), args.len())
}

// ─── Value Constructors / Destructors ──────────────────────────────────────

#[inline]
pub fn encode_f64(val: f64) -> u64 {
    val.to_bits()
}

#[inline]
pub fn decode_f64(val: u64) -> f64 {
    f64::from_bits(val)
}

#[inline]
pub fn is_number(val: u64) -> bool {
    (val >> 56) != 0x7F
}

#[inline]
pub fn is_tagged(val: u64) -> bool {
    (val >> 56) == 0x7F
}

#[inline]
pub fn encode_bool(val: bool) -> u64 {
    if val { TAG_TRUE } else { TAG_FALSE }
}

#[inline]
pub fn decode_bool(val: u64) -> bool {
    val == TAG_TRUE
}

#[inline]
pub const fn encode_unit() -> u64 {
    TAG_UNIT
}

/// Encode a heap-allocated value (pointer must be valid and aligned).
/// The pointer's lower 48 bits are stored, higher bits must be zero-extendable.
/// On x86-64, user-space pointers are < 2^48 so this is safe.
#[inline]
pub fn encode_heap(tag: u64, ptr: *const u8) -> u64 {
    TAG_PATTERN | tag | (ptr as u64 & PTR_MASK)
}

/// Extract the pointer from a heap-allocated tagged value.
#[inline]
pub fn decode_ptr(val: u64) -> *const u8 {
    (val & PTR_MASK) as *const u8
}

/// Extract the kind tag from a tagged value.
#[inline]
pub fn tag_kind(val: u64) -> u64 {
    val & 0x00FF_0000_0000_0000
}

// ─── Runtime Symbol Table ──────────────────────────────────────────────────
//
// Maps JIT symbol names --> extern "C" function names.
// Used to register symbols with the JIT builder.
// Each entry: (jit_symbol_name, c_function_name)
//
// The JIT compiler emits calls to `__ling_*` symbols which are resolved
// to C ABI functions either from this crate or registered externally.

pub static SYMBOL_MAP: &[(&str, &[u8])] = &[
    // Core allocation / panics
    ("__ling_alloc", b"ling_alloc\0"),
    ("__ling_free", b"ling_free\0"),
    ("__ling_panic", b"ling_panic\0"),

    // Arithmetic
    ("__ling_add", b"ling_add\0"),
    ("__ling_sub", b"ling_sub\0"),
    ("__ling_mul", b"ling_mul\0"),
    ("__ling_div", b"ling_div\0"),
    ("__ling_rem", b"ling_rem\0"),
    ("__ling_neg", b"ling_neg\0"),

    // Comparison
    ("__ling_eq", b"ling_eq\0"),
    ("__ling_ne", b"ling_ne\0"),
    ("__ling_lt", b"ling_lt\0"),
    ("__ling_le", b"ling_le\0"),
    ("__ling_gt", b"ling_gt\0"),
    ("__ling_ge", b"ling_ge\0"),

    // Logics
    ("__ling_and", b"ling_and\0"),
    ("__ling_or", b"ling_or\0"),
    ("__ling_not", b"ling_not\0"),

    // Builtin dispatch (all builtins go through this)
    ("__ling_builtin", b"ling_builtin\0"),

    // String operations
    ("__ling_str_new", b"ling_str_new\0"),
    ("__ling_str_len", b"ling_str_len\0"),
    ("__ling_str_concat", b"ling_str_concat\0"),
    ("__ling_str_eq", b"ling_str_eq\0"),

    // List operations
    ("__ling_list_new", b"ling_list_new\0"),
    ("__ling_list_push", b"ling_list_push\0"),
    ("__ling_list_get", b"ling_list_get\0"),
    ("__ling_list_len", b"ling_list_len\0"),

    // Struct operations
    ("__ling_struct_new", b"ling_struct_new\0"),
    ("__ling_struct_get", b"ling_struct_get\0"),

    // Print / output
    ("__ling_print", b"ling_print\0"),
    ("__ling_print_val", b"ling_print_val\0"),
    ("__ling_print_newline", b"ling_print_newline\0"),
    ("__ling_time_now", b"ling_time_now\0"),

    // Numeric helpers (for JIT-inlined operations)
    ("__ling_f64_add", b"ling_f64_add\0"),
    ("__ling_f64_sub", b"ling_f64_sub\0"),
    ("__ling_f64_mul", b"ling_f64_mul\0"),
    ("__ling_f64_div", b"ling_f64_div\0"),
    ("__ling_f64_rem", b"ling_f64_rem\0"),
    ("__ling_f64_neg", b"ling_f64_neg\0"),
    ("__ling_f64_eq", b"ling_f64_eq\0"),
    ("__ling_f64_lt", b"ling_f64_lt\0"),
    ("__ling_f64_gt", b"ling_f64_gt\0"),
    ("__ling_f64_le", b"ling_f64_le\0"),
    ("__ling_f64_ge", b"ling_f64_ge\0"),
    ("__ling_sin", b"ling_sin\0"),
    ("__ling_cos", b"ling_cos\0"),
    ("__ling_sqrt", b"ling_sqrt\0"),
    ("__ling_abs", b"ling_abs\0"),
    ("__ling_floor", b"ling_floor\0"),
    ("__ling_ceil", b"ling_ceil\0"),
    ("__ling_round", b"ling_round\0"),

    // Bool helpers
    ("__ling_bool_to_u64", b"ling_bool_to_u64\0"),
];

// ─── Extern C Reference Functions ──────────────────────────────────────────
//
// These are the C ABI entry points called from JIT'd code.
// They must be provided by the host crate via `register_runtime_symbols`.
//
// We declare them as extern "C" references so the JIT builder can bind them.

// Helper: wrap/unwrap f64 values as boxed Values
extern "C" {
    pub fn ling_f64_add(a: f64, b: f64) -> f64;
    pub fn ling_f64_sub(a: f64, b: f64) -> f64;
    pub fn ling_f64_mul(a: f64, b: f64) -> f64;
    pub fn ling_f64_div(a: f64, b: f64) -> f64;
    pub fn ling_f64_rem(a: f64, b: f64) -> f64;
    pub fn ling_f64_neg(a: f64) -> f64;
    pub fn ling_f64_eq(a: f64, b: f64) -> u64;
    pub fn ling_f64_lt(a: f64, b: f64) -> u64;
    pub fn ling_f64_gt(a: f64, b: f64) -> u64;
    pub fn ling_f64_le(a: f64, b: f64) -> u64;
    pub fn ling_f64_ge(a: f64, b: f64) -> u64;
    pub fn ling_sin(a: f64) -> f64;
    pub fn ling_cos(a: f64) -> f64;
    pub fn ling_sqrt(a: f64) -> f64;
    pub fn ling_abs(a: f64) -> f64;
    pub fn ling_floor(a: f64) -> f64;
    pub fn ling_ceil(a: f64) -> f64;
    pub fn ling_round(a: f64) -> f64;
    pub fn ling_bool_to_u64(b: u64) -> u64;

    pub fn ling_add(a: u64, b: u64) -> u64;
    pub fn ling_sub(a: u64, b: u64) -> u64;
    pub fn ling_mul(a: u64, b: u64) -> u64;
    pub fn ling_div(a: u64, b: u64) -> u64;
    pub fn ling_rem(a: u64, b: u64) -> u64;
    pub fn ling_neg(a: u64) -> u64;
    pub fn ling_eq(a: u64, b: u64) -> u64;
    pub fn ling_ne(a: u64, b: u64) -> u64;
    pub fn ling_lt(a: u64, b: u64) -> u64;
    pub fn ling_le(a: u64, b: u64) -> u64;
    pub fn ling_gt(a: u64, b: u64) -> u64;
    pub fn ling_ge(a: u64, b: u64) -> u64;
    pub fn ling_and(a: u64, b: u64) -> u64;
    pub fn ling_or(a: u64, b: u64) -> u64;
    pub fn ling_not(a: u64) -> u64;

    pub fn ling_alloc(size: u64) -> u64;
    pub fn ling_free(ptr: u64) -> u64;
    pub fn ling_panic(msg: u64);

    pub fn ling_str_new(ptr: *const u8, len: usize) -> u64;
    pub fn ling_str_len(val: u64) -> u64;
    pub fn ling_str_concat(a: u64, b: u64) -> u64;
    pub fn ling_str_eq(a: u64, b: u64) -> u64;

    pub fn ling_list_new() -> u64;
    pub fn ling_list_push(list: u64, val: u64) -> u64;
    pub fn ling_list_get(list: u64, idx: u64) -> u64;
    pub fn ling_list_len(list: u64) -> u64;

    pub fn ling_struct_new(name_ptr: *const u8, name_len: usize, args_ptr: *const u64, args_len: usize) -> u64;
    pub fn ling_struct_get(val: u64, field_ptr: *const u8, field_len: usize) -> u64;

    pub fn ling_print(cstr: *const u8);
    pub fn ling_print_val(val: u64);

    /// Generic builtin dispatcher – calls builtin by name.
    pub fn ling_builtin(name_ptr: *const u8, name_len: usize, args_ptr: *const u64, args_len: usize) -> u64;
}
