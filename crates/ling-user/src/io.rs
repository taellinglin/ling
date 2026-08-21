use crate::strings::{bytes_of_ptr, decode_ptr, TAG_KIND_STRING};
use crate::syscall::ling_sys_write;

fn write_stdout(bytes: &[u8]) {
    unsafe { ling_sys_write(1, bytes.as_ptr() as u64, bytes.len() as u64) };
}

fn print_f64(n: f64) {
    if n.is_nan() {
        write_stdout(b"NaN");
        return;
    }
    if n.is_infinite() {
        if n < 0.0 {
            write_stdout(b"-Infinity");
        } else {
            write_stdout(b"Infinity");
        }
        return;
    }
    let mut val = n;
    if val < 0.0 {
        write_stdout(b"-");
        val = -val;
    }
    let int_part = val as u64;
    print_u64(int_part);

    let frac = val - (int_part as f64);
    if frac > 1e-9 {
        write_stdout(b".");
        let mut f = frac;
        for _ in 0..6 {
            f *= 10.0;
            let digit = f as u8;
            write_stdout(&[b'0' + digit]);
            f -= digit as f64;
            if f < 1e-6 {
                break;
            }
        }
    }
}

fn print_u64(n: u64) {
    if n == 0 {
        write_stdout(b"0");
        return;
    }
    let mut digits = [0u8; 20];
    let mut i = digits.len();
    let mut v = n;
    while v > 0 {
        i -= 1;
        digits[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    write_stdout(&digits[i..]);
}

pub fn format_and_print(val: u64) {
    if (val >> 56) != 0x7F {
        print_f64(f64::from_bits(val));
    } else {
        let tag = val & 0x00FF_0000_0000_0000;
        match tag {
            0x0000_0000_0000_0000 => write_stdout(b"()"),
            0x0001_0000_0000_0000 => write_stdout(b"false"),
            0x0002_0000_0000_0000 => write_stdout(b"true"),
            TAG_KIND_STRING => {
                let bytes = unsafe { bytes_of_ptr(decode_ptr(val)) };
                write_stdout(bytes);
            }
            0x0004_0000_0000_0000 => write_stdout(b"[list]"),
            0x0005_0000_0000_0000 => write_stdout(b"{struct}"),
            _ => write_stdout(b"<tagged>"),
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ling_print(val: u64) -> u64 {
    format_and_print(val);
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_print_val(val: u64) -> u64 {
    format_and_print(val);
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_print_newline() -> u64 {
    write_stdout(b"\n");
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_time_now() -> u64 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_builtin(
    name_ptr: u64,
    name_len: u64,
    args_ptr: u64,
    args_len: u64,
) -> u64 {
    let name_slice = core::slice::from_raw_parts(name_ptr as *const u8, name_len as usize);
    let name = core::str::from_utf8(name_slice).unwrap_or("");
    let args = core::slice::from_raw_parts(args_ptr as *const u64, args_len as usize);

    match name {
        "print" => {
            for (i, &arg) in args.iter().enumerate() {
                if i > 0 {
                    write_stdout(b" ");
                }
                format_and_print(arg);
            }
            write_stdout(b"\n");
            0x7F00_0000_0000_0000 // Unit
        }
        "exit" => {
            let code = if !args.is_empty() {
                if (args[0] >> 56) == 0x7F { 0 } else { f64::from_bits(args[0]) as u64 }
            } else {
                0
            };
            crate::syscall::ling_sys_exit(code);
            0
        }
        _ => 0x7F00_0000_0000_0000,
    }
}
