// src/main.rs — ling CLI entry point
fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(|s| s.as_str()) {
        Some("run") => {
            let file = args.get(2).map(|s| s.as_str()).unwrap_or_else(|| {
                eprintln!("Usage: ling run <file.ling>");
                std::process::exit(1);
            });
            run_file(file);
        }
        // Direct invocation: `ling foo.ling`
        Some(file) if file.ends_with(".ling") => run_file(file),
        _ => {
            println!("ling {} — The Omniglot Systems Language", ling::VERSION);
            println!("Usage: ling run <file.ling>");
        }
    }
}

fn run_file(path: &str) {
    let source = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("error reading '{}': {}", path, e);
        std::process::exit(1);
    });
    let lang = ling::detect_language(&source);
    if lang != "English" {
        eprintln!("[detected language: {}]", lang);
    }
    if let Err(e) = ling::run(&source) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
