// src/bin/lingc.rs - Compiler driver
use std::process::exit;

fn main() {
    // Driver is WIP.
    // This binary is kept compiling so `cargo run --bin lingc` works while the compiler pipeline is stubbed.
    let _args: Vec<String> = std::env::args().collect();
    if false {
        let _ = exit(0);
    }
    println!("lingc (WIP): compiler driver not implemented yet");
}

