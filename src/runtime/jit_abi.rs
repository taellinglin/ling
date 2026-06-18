//! C ABI runtime functions for the JIT backend.
//!
//! These functions bridge between NaN-boxed u64 values (used by JIT'd code)
//! and the Interpreter's `Value` type.
//!
//! They are registered as symbols with the JIT backend before compilation,
//! so JIT'd code can call them directly.

use crate::runtime::Interpreter;
use crate::runtime::Value;
use std::sync::{Mutex, OnceLock};

// ─── Global Interpreter Access ──────────────────────────────────────────────

static JIT_INTERP: OnceLock<Mutex<usize>> = OnceLock::new();

/// Initialize the JIT runtime with an interpreter.
/// Called before JIT compilation and execution.
pub fn init(interp: Interpreter) {
    let ptr = Box::into_raw(Box::new(interp)) as usize;
    JIT_INTERP.set(Mutex::new(ptr)).ok();
}

/// Run a closure with access to the interpreter.
fn with_interp<F, R>(f: F) -> R
where
    F: FnOnce(&mut Interpreter) -> R,
{
    let guard = JIT_INTERP
        .get()
        .expect("JIT interpreter not initialized")
        .lock()
        .expect("JIT interpreter lock");
    let interp = unsafe { &mut *(*guard as *mut Interpreter) };
    f(interp)
}

// ─── Value Encoding/Decoding ────────────────────────────────────────────────

pub const TAG_UNIT: u64 = 0x7F00_0000_0000_0000;
pub const TAG_FALSE: u64 = 0x7F01_0000_0000_0000;
pub const TAG_TRUE: u64 = 0x7F02_0000_0000_0000;
pub const TAG_PATTERN: u64 = 0x7F00_0000_0000_0000;
pub const PTR_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

const TAG_KIND_UNIT: u64 = 0x0000_0000_0000_0000;
const TAG_KIND_BOOL_FALSE: u64 = 0x0001_0000_0000_0000;
const TAG_KIND_BOOL_TRUE: u64 = 0x0002_0000_0000_0000;
const TAG_KIND_STRING: u64 = 0x0003_0000_0000_0000;
const TAG_KIND_LIST: u64 = 0x0004_0000_0000_0000;
const TAG_KIND_OK: u64 = 0x0008_0000_0000_0000;
const TAG_KIND_ERR: u64 = 0x0009_0000_0000_0000;

#[inline]
fn is_number(val: u64) -> bool {
    (val >> 56) != 0x7F
}

#[inline]
fn tag_kind(val: u64) -> u64 {
    val & 0x00FF_0000_0000_0000
}

#[inline]
fn decode_ptr(val: u64) -> *const u8 {
    (val & PTR_MASK) as *const u8
}

/// Convert an encoded u64 value to a `Value`.
pub fn decode_value(val: u64) -> Value {
    if is_number(val) {
        Value::Number(f64::from_bits(val))
    } else {
        match tag_kind(val) {
            TAG_KIND_UNIT => Value::Unit,
            TAG_KIND_BOOL_FALSE => Value::Bool(false),
            TAG_KIND_BOOL_TRUE => Value::Bool(true),
            TAG_KIND_STRING => {
                let ptr = decode_ptr(val);
                if ptr.is_null() {
                    Value::Str(String::new())
                } else {
                    let s = unsafe { &*(ptr as *const String) };
                    Value::Str(s.clone())
                }
            },
            TAG_KIND_LIST => {
                let ptr = decode_ptr(val);
                if ptr.is_null() {
                    Value::List(Vec::new())
                } else {
                    let list = unsafe { &*(ptr as *const Vec<Value>) };
                    Value::List(list.clone())
                }
            },
            TAG_KIND_OK => {
                let ptr = decode_ptr(val);
                if ptr.is_null() {
                    Value::Ok(Box::new(Value::Unit))
                } else {
                    let inner = unsafe { &*(ptr as *const Value) };
                    Value::Ok(Box::new(inner.clone()))
                }
            },
            TAG_KIND_ERR => {
                let ptr = decode_ptr(val);
                if ptr.is_null() {
                    Value::Err(Box::new(Value::Unit))
                } else {
                    let inner = unsafe { &*(ptr as *const Value) };
                    Value::Err(Box::new(inner.clone()))
                }
            },
            _ => Value::Unit,
        }
    }
}

/// Convert a `Value` to an encoded u64.
pub fn encode_value(val: &Value) -> u64 {
    match val {
        Value::Number(n) => n.to_bits(),
        Value::Bool(b) => {
            if *b {
                TAG_TRUE
            } else {
                TAG_FALSE
            }
        },
        Value::Unit => TAG_UNIT,
        Value::Str(s) => {
            let ptr = Box::into_raw(Box::new(s.clone())) as *const u8;
            TAG_PATTERN | TAG_KIND_STRING | (ptr as u64 & PTR_MASK)
        },
        Value::List(list) => {
            let ptr = Box::into_raw(Box::new(list.clone())) as *const u8;
            TAG_PATTERN | TAG_KIND_LIST | (ptr as u64 & PTR_MASK)
        },
        Value::Ok(inner) => {
            let ptr = Box::into_raw(Box::new((**inner).clone())) as *const u8;
            TAG_PATTERN | TAG_KIND_OK | (ptr as u64 & PTR_MASK)
        },
        Value::Err(inner) => {
            let ptr = Box::into_raw(Box::new((**inner).clone())) as *const u8;
            TAG_PATTERN | TAG_KIND_ERR | (ptr as u64 & PTR_MASK)
        },
        Value::Fn(_, _, _) => TAG_UNIT,
        Value::Struct { name: _, fields: _ } => TAG_UNIT,
        Value::Variant { .. } => TAG_UNIT,
    }
}

// ─── C ABI Functions ────────────────────────────────────────────────────────

/// Generic builtin dispatcher.
/// Receives name (ptr+len), args array (ptr+len), returns encoded Value.
#[no_mangle]
pub unsafe extern "C" fn ling_builtin(
    name_ptr: *const u8,
    name_len: usize,
    args_ptr: *const u64,
    args_len: usize,
) -> u64 {
    let name = std::str::from_utf8(std::slice::from_raw_parts(name_ptr, name_len)).unwrap_or("");
    let args_slice = std::slice::from_raw_parts(args_ptr, args_len);

    with_interp(|interp| {
        let args: Vec<Value> = args_slice.iter().map(|&a| decode_value(a)).collect();
        let env = std::collections::HashMap::new();
        match interp.call_named(name, args, &env) {
            Ok(val) => encode_value(&val),
            Err(_) => TAG_UNIT,
        }
    })
}

// Core arithmetic (boxed value ops - handles mixed types like string + string, etc.)

#[no_mangle]
pub unsafe extern "C" fn ling_add(a: u64, b: u64) -> u64 {
    with_interp(|interp| {
        let av = decode_value(a);
        let bv = decode_value(b);
        let result = interp.apply_binop(&crate::parser::ast::BinOp::Add, av, bv);
        match result {
            Ok(val) => encode_value(&val),
            Err(_) => TAG_UNIT,
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn ling_sub(a: u64, b: u64) -> u64 {
    with_interp(|interp| {
        let av = decode_value(a);
        let bv = decode_value(b);
        let result = interp.apply_binop(&crate::parser::ast::BinOp::Sub, av, bv);
        match result {
            Ok(val) => encode_value(&val),
            Err(_) => TAG_UNIT,
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn ling_mul(a: u64, b: u64) -> u64 {
    with_interp(|interp| {
        let av = decode_value(a);
        let bv = decode_value(b);
        let result = interp.apply_binop(&crate::parser::ast::BinOp::Mul, av, bv);
        match result {
            Ok(val) => encode_value(&val),
            Err(_) => TAG_UNIT,
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn ling_div(a: u64, b: u64) -> u64 {
    with_interp(|interp| {
        let av = decode_value(a);
        let bv = decode_value(b);
        let result = interp.apply_binop(&crate::parser::ast::BinOp::Div, av, bv);
        match result {
            Ok(val) => encode_value(&val),
            Err(_) => TAG_UNIT,
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn ling_rem(a: u64, b: u64) -> u64 {
    with_interp(|interp| {
        let av = decode_value(a);
        let bv = decode_value(b);
        let result = interp.apply_binop(&crate::parser::ast::BinOp::Rem, av, bv);
        match result {
            Ok(val) => encode_value(&val),
            Err(_) => TAG_UNIT,
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn ling_neg(a: u64, _b: u64) -> u64 {
    with_interp(|interp| {
        let av = decode_value(a);
        let result = interp.apply_binop(&crate::parser::ast::BinOp::Sub, Value::Number(0.0), av);
        match result {
            Ok(val) => encode_value(&val),
            Err(_) => TAG_UNIT,
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn ling_eq(a: u64, b: u64) -> u64 {
    let av = decode_value(a);
    let bv = decode_value(b);
    if super::values_equal(&av, &bv) {
        TAG_TRUE
    } else {
        TAG_FALSE
    }
}

#[no_mangle]
pub unsafe extern "C" fn ling_ne(a: u64, b: u64) -> u64 {
    let av = decode_value(a);
    let bv = decode_value(b);
    if super::values_equal(&av, &bv) {
        TAG_FALSE
    } else {
        TAG_TRUE
    }
}

#[no_mangle]
pub unsafe extern "C" fn ling_lt(a: u64, b: u64) -> u64 {
    let av = decode_value(a);
    let bv = decode_value(b);
    match (&av, &bv) {
        (Value::Number(a), Value::Number(b)) if a < b => TAG_TRUE,
        _ => TAG_FALSE,
    }
}

#[no_mangle]
pub unsafe extern "C" fn ling_le(a: u64, b: u64) -> u64 {
    let av = decode_value(a);
    let bv = decode_value(b);
    match (&av, &bv) {
        (Value::Number(a), Value::Number(b)) if a <= b => TAG_TRUE,
        _ => TAG_FALSE,
    }
}

#[no_mangle]
pub unsafe extern "C" fn ling_gt(a: u64, b: u64) -> u64 {
    let av = decode_value(a);
    let bv = decode_value(b);
    match (&av, &bv) {
        (Value::Number(a), Value::Number(b)) if a > b => TAG_TRUE,
        _ => TAG_FALSE,
    }
}

#[no_mangle]
pub unsafe extern "C" fn ling_ge(a: u64, b: u64) -> u64 {
    let av = decode_value(a);
    let bv = decode_value(b);
    match (&av, &bv) {
        (Value::Number(a), Value::Number(b)) if a >= b => TAG_TRUE,
        _ => TAG_FALSE,
    }
}

#[no_mangle]
pub unsafe extern "C" fn ling_and(a: u64, b: u64) -> u64 {
    let av = decode_value(a);
    let av_truthy = with_interp(|interp| interp.is_truthy(&av));
    if av_truthy {
        let bv = decode_value(b);
        let bv_truthy = with_interp(|interp| interp.is_truthy(&bv));
        if bv_truthy {
            TAG_TRUE
        } else {
            TAG_FALSE
        }
    } else {
        TAG_FALSE
    }
}

#[no_mangle]
pub unsafe extern "C" fn ling_or(a: u64, b: u64) -> u64 {
    let av = decode_value(a);
    let av_truthy = with_interp(|interp| interp.is_truthy(&av));
    if av_truthy {
        TAG_TRUE
    } else {
        let bv = decode_value(b);
        let bv_truthy = with_interp(|interp| interp.is_truthy(&bv));
        if bv_truthy {
            TAG_TRUE
        } else {
            TAG_FALSE
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ling_not(a: u64) -> u64 {
    let av = decode_value(a);
    let truthy = with_interp(|interp| interp.is_truthy(&av));
    if truthy {
        TAG_FALSE
    } else {
        TAG_TRUE
    }
}

#[no_mangle]
pub unsafe extern "C" fn ling_print_val(val: u64) -> u64 {
    let v = decode_value(val);
    print!("{}", v);
    TAG_UNIT
}

#[no_mangle]
pub unsafe extern "C" fn ling_print_newline() -> u64 {
    println!();
    TAG_UNIT
}

#[no_mangle]
pub unsafe extern "C" fn ling_time_now() -> u64 {
    with_interp(|interp| {
        let elapsed = interp.start_time.elapsed().as_secs_f64();
        encode_value(&Value::Number(elapsed))
    })
}

#[no_mangle]
pub unsafe extern "C" fn ling_print(cstr: *const u8) {
    if !cstr.is_null() {
        let s = unsafe { std::ffi::CStr::from_ptr(cstr as *const i8) };
        if let Ok(s) = s.to_str() {
            print!("{}", s);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ling_alloc(size: u64) -> u64 {
    let layout = std::alloc::Layout::from_size_align(size as usize, 8).unwrap();
    let ptr = std::alloc::alloc(layout);
    ptr as u64
}

#[no_mangle]
pub unsafe extern "C" fn ling_free(ptr: u64) -> u64 {
    if ptr != 0 {
        let _ = Box::from_raw(ptr as *mut Value);
    }
    TAG_UNIT
}

#[no_mangle]
pub unsafe extern "C" fn ling_panic(msg: u64) {
    let v = decode_value(msg);
    panic!("JIT panic: {}", v);
}

// String operations

#[no_mangle]
pub unsafe extern "C" fn ling_str_new(ptr: *const u8, len: usize) -> u64 {
    let s = std::str::from_utf8(std::slice::from_raw_parts(ptr, len)).unwrap_or("");
    let string_val = Value::Str(s.to_string());
    encode_value(&string_val)
}

#[no_mangle]
pub unsafe extern "C" fn ling_str_len(val: u64) -> u64 {
    let v = decode_value(val);
    match &v {
        Value::Str(s) => (s.chars().count() as f64).to_bits(),
        Value::List(list) => (list.len() as f64).to_bits(),
        _ => 0.0f64.to_bits(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn ling_str_concat(a: u64, b: u64) -> u64 {
    let av = decode_value(a);
    let bv = decode_value(b);
    match (&av, &bv) {
        (Value::Str(sa), Value::Str(sb)) => {
            let result = Value::Str(format!("{}{}", sa, sb));
            encode_value(&result)
        },
        (_, _) => {
            let result = Value::Str(format!("{}{}", av, bv));
            encode_value(&result)
        },
    }
}

#[no_mangle]
pub unsafe extern "C" fn ling_str_eq(a: u64, b: u64) -> u64 {
    let av = decode_value(a);
    let bv = decode_value(b);
    match (&av, &bv) {
        (Value::Str(sa), Value::Str(sb)) => {
            if sa == sb {
                TAG_TRUE
            } else {
                TAG_FALSE
            }
        },
        _ => TAG_FALSE,
    }
}

// List operations

#[no_mangle]
pub unsafe extern "C" fn ling_list_new() -> u64 {
    encode_value(&Value::List(Vec::new()))
}

#[no_mangle]
pub unsafe extern "C" fn ling_list_push(list: u64, val: u64) -> u64 {
    let mut list_v = decode_value(list);
    let item_v = decode_value(val);
    if let Value::List(ref mut items) = list_v {
        items.push(item_v);
    }
    encode_value(&list_v)
}

#[no_mangle]
pub unsafe extern "C" fn ling_list_get(list: u64, idx: u64) -> u64 {
    let list_v = decode_value(list);
    let idx_v = decode_value(idx);
    match (&list_v, &idx_v) {
        (Value::List(items), Value::Number(n)) => {
            let i = *n as usize;
            if i < items.len() {
                encode_value(&items[i])
            } else {
                TAG_UNIT
            }
        },
        _ => TAG_UNIT,
    }
}

#[no_mangle]
pub unsafe extern "C" fn ling_list_len(list: u64) -> u64 {
    let list_v = decode_value(list);
    match &list_v {
        Value::List(items) => items.len() as f64,
        _ => 0.0f64,
    }
    .to_bits()
}

// Struct operations

#[no_mangle]
pub unsafe extern "C" fn ling_struct_new(
    name_ptr: *const u8,
    name_len: usize,
    args_ptr: *const u64,
    args_len: usize,
) -> u64 {
    let name = std::str::from_utf8(unsafe { std::slice::from_raw_parts(name_ptr, name_len) })
        .unwrap_or("")
        .to_string();
    let args_slice = unsafe { std::slice::from_raw_parts(args_ptr, args_len) };
    let args: Vec<Value> = args_slice.iter().map(|&a| decode_value(a)).collect();

    with_interp(|interp| {
        if let Some(field_names) = interp.structs.get(&name) {
            let fields: Vec<(String, Value)> = field_names
                .iter()
                .zip(args.into_iter())
                .map(|(n, v)| (n.clone(), v))
                .collect();
            let val = Value::Struct { name, fields };
            encode_value(&val)
        } else {
            TAG_UNIT
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn ling_struct_get(val: u64, field_ptr: *const u8, field_len: usize) -> u64 {
    let field_name =
        std::str::from_utf8(unsafe { std::slice::from_raw_parts(field_ptr, field_len) })
            .unwrap_or("");
    let v = decode_value(val);
    match &v {
        Value::Struct { fields, .. } => {
            for (name, fval) in fields {
                if name == field_name {
                    return encode_value(fval);
                }
            }
            TAG_UNIT
        },
        _ => TAG_UNIT,
    }
}

// Unboxed f64 math helpers (for inlined JIT numeric operations)

#[no_mangle]
pub unsafe extern "C" fn ling_f64_add(a: f64, b: f64) -> f64 {
    a + b
}
#[no_mangle]
pub unsafe extern "C" fn ling_f64_sub(a: f64, b: f64) -> f64 {
    a - b
}
#[no_mangle]
pub unsafe extern "C" fn ling_f64_mul(a: f64, b: f64) -> f64 {
    a * b
}
#[no_mangle]
pub unsafe extern "C" fn ling_f64_div(a: f64, b: f64) -> f64 {
    a / b
}
#[no_mangle]
pub unsafe extern "C" fn ling_f64_rem(a: f64, b: f64) -> f64 {
    a % b
}
#[no_mangle]
pub unsafe extern "C" fn ling_f64_neg(a: f64) -> f64 {
    -a
}
#[no_mangle]
pub unsafe extern "C" fn ling_f64_eq(a: f64, b: f64) -> u64 {
    if a == b {
        TAG_TRUE
    } else {
        TAG_FALSE
    }
}
#[no_mangle]
pub unsafe extern "C" fn ling_f64_lt(a: f64, b: f64) -> u64 {
    if a < b {
        TAG_TRUE
    } else {
        TAG_FALSE
    }
}
#[no_mangle]
pub unsafe extern "C" fn ling_f64_gt(a: f64, b: f64) -> u64 {
    if a > b {
        TAG_TRUE
    } else {
        TAG_FALSE
    }
}
#[no_mangle]
pub unsafe extern "C" fn ling_f64_le(a: f64, b: f64) -> u64 {
    if a <= b {
        TAG_TRUE
    } else {
        TAG_FALSE
    }
}
#[no_mangle]
pub unsafe extern "C" fn ling_f64_ge(a: f64, b: f64) -> u64 {
    if a >= b {
        TAG_TRUE
    } else {
        TAG_FALSE
    }
}
#[no_mangle]
pub unsafe extern "C" fn ling_sin(a: f64) -> f64 {
    a.sin()
}
#[no_mangle]
pub unsafe extern "C" fn ling_cos(a: f64) -> f64 {
    a.cos()
}
#[no_mangle]
pub unsafe extern "C" fn ling_sqrt(a: f64) -> f64 {
    a.sqrt()
}
#[no_mangle]
pub unsafe extern "C" fn ling_abs(a: f64) -> f64 {
    a.abs()
}
#[no_mangle]
pub unsafe extern "C" fn ling_floor(a: f64) -> f64 {
    a.floor()
}
#[no_mangle]
pub unsafe extern "C" fn ling_ceil(a: f64) -> f64 {
    a.ceil()
}
#[no_mangle]
pub unsafe extern "C" fn ling_round(a: f64) -> f64 {
    a.round()
}

#[no_mangle]
pub unsafe extern "C" fn ling_bool_to_u64(b: u64) -> u64 {
    if b != 0 {
        TAG_TRUE
    } else {
        TAG_FALSE
    }
}
