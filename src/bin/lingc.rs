// src/bin/lingc.rs — Ling compiler driver
fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(|s| s.as_str()) {
        Some("run") => {
            let file = args.get(2).map(|s| s.as_str()).unwrap_or_else(|| {
                eprintln!("Usage: lingc run <file.ling>");
                std::process::exit(1);
            });
            run_file(file);
        }
        Some(file) if file.ends_with(".ling") => run_file(file),
        _ => {
            println!("lingc {} — Ling Compiler", ling::VERSION);
            println!("Usage:");
            println!("  lingc run <file.ling>   interpret and run");
            println!("  lingc <file.ling>        same as above");
        }
    }
}

fn run_file(path: &str) {
    let source = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("error reading '{}': {}", path, e);
        std::process::exit(1);
    });
    if let Err(e) = ling::run(&source) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
