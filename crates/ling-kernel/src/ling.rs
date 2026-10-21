//! The Terminal's `ling run` / `ling eval` / `ling demo`: run a program with
//! the in-kernel compiler + interpreter (`crate::lingc`), capturing its console
//! output so the Terminal can show it (the text console is invisible under the
//! desktop). This replaced the early subset interpreter that lived here.

use alloc::string::String;

pub use crate::lingc::DEMO;

/// Run a one-line program (imports resolve from the root).
pub fn run_source(src: &str) -> (String, Option<String>) {
    capture(|| crate::lingc::run_source(src))
}

/// Run the first of `candidates` that names a program (a `.ling` file, or a
/// folder with a `main.ling`), with `args` as its command line.
pub fn run_file(candidates: &[&str], args: &[&str]) -> (String, Option<String>) {
    capture(|| {
        for c in candidates {
            if crate::lingc::locate(c).is_some() {
                return crate::lingc::run_file(c, args);
            }
        }
        Err(alloc::format!("ling run: cannot find '{}'", candidates.last().copied().unwrap_or("")))
    })
}

fn capture(body: impl FnOnce() -> Result<(), String>) -> (String, Option<String>) {
    crate::console_capture_begin();
    let r = body();
    let out = String::from_utf8_lossy(crate::console_capture_end()).into_owned();
    (out, r.err())
}
