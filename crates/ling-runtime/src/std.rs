//! Ling standard library — built-in functions exposed to the runtime.

/// Print a value to stdout with a trailing newline.
pub fn ling_print(s: &str) {
    println!("{s}");
}

/// Print a debug representation.
pub fn ling_debug(s: &str) {
    eprintln!("[debug] {s}");
}

/// Abort with a message.
pub fn ling_panic(msg: &str) -> ! {
    panic!("{msg}");
}

/// Sleep for `ms` milliseconds (useful for simple animations / demos).
pub fn ling_sleep_ms(ms: u64) {
    std::thread::sleep(std::time::Duration::from_millis(ms));
}

/// Current Unix timestamp in seconds.
pub fn ling_time() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
