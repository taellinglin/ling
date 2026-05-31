// Built by ling build — no console window on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    const SOURCE: &str = include_str!("../PO2.ling");
    let lang = ling::detect_language(SOURCE);
    if lang != "English" {
        eprintln!("[language: {}]", lang);
    }
    if let Err(e) = ling::run(SOURCE) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
