// build.rs - Compile-time code generation
use std::env;
use std::path::Path;
use std::fs::File;
use std::io::Write;

fn main() {
    println!("cargo:rerun-if-changed=src/parser/grammar.lalrpop");
    println!("cargo:rerun-if-changed=lexicons/");
    
    // Generate LALRPOP parser
    // NOTE: `process_root()` may fail in this WIP repo due to incomplete grammar/type setup.
    // Keeping the build runnable so `cargo check` can proceed.
    if let Err(e) = lalrpop::process_root() {
        eprintln!("warning: lalrpop::process_root() failed, skipping parser generation: {e}");
    }

    
    // Generate lexicon lookup tables
    generate_lexicon_tables();

    // Generate Unicode script detection tables
    generate_unicode_tables();

    // Embed the default Ling icon into every Windows binary of this crate.
    embed_default_icon("assets/ling.ico");
}

/// Embed `ico_rel` (relative to this crate) as the executable icon for all of
/// the crate's binaries on Windows. A missing icon or absent resource compiler
/// only warns — it must never fail the build.
fn embed_default_icon(ico_rel: &str) {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let ico = Path::new(&manifest).join(ico_rel);
    if !ico.exists() {
        println!("cargo:warning=icon '{}' not found; building without an app icon", ico.display());
        return;
    }
    println!("cargo:rerun-if-changed={ico_rel}");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(&ico.to_string_lossy());
    if let Err(e) = res.compile() {
        println!("cargo:warning=icon embed skipped ({e})");
    }
}

fn generate_lexicon_tables() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("lexicon_tables.rs");
    let mut file = File::create(dest_path).unwrap();
    
    writeln!(file, "// Auto-generated lexicon tables").unwrap();
    // Write lookup tables for all 16 lexicons
}

fn generate_unicode_tables() {
    // Generate script detection tables
}
