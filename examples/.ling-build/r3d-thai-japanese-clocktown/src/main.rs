fn main() {
    const SOURCE: &str = include_str!("../3d-thai-japanese-clocktown.ling");
    let lang = ling::detect_language(SOURCE);
    if lang != "English" {
        eprintln!("[language: {}]", lang);
    }
    if let Err(e) = ling::run(SOURCE) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
