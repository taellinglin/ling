fn main() {
    const SOURCE: &str = include_str!("../Lounge.ling");
    let lang = ling::detect_language(SOURCE);
    if lang != "English" {
        eprintln!("[language: {}]", lang);
    }
    if let Err(e) = ling::run(SOURCE) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
