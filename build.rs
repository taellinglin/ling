// build.rs - Compile-time code generation
use std::env;
use std::fs::File;
use std::io::Write;
use std::path::Path;

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
    // `assets/` is excluded from the published crate (it holds ~1 GB of audio),
    // so the published package ships a copy of just the icon at the repo root
    // (`ling.ico`). Try the dev path first, then the packaged fallback.
    println!("cargo:rerun-if-changed=assets/ling.ico");
    println!("cargo:rerun-if-changed=ling.ico");
    embed_default_icon(&["assets/ling.ico", "ling.ico"]);
}

/// Embed the first existing icon in `candidates` (relative to this crate) as the
/// executable icon for all of the crate's binaries on Windows. A missing icon or
/// absent resource compiler only warns — it must never fail the build.
fn embed_default_icon(candidates: &[&str]) {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let ico = candidates
        .iter()
        .map(|rel| Path::new(&manifest).join(rel))
        .find(|p| p.exists());
    let Some(ico) = ico else {
        println!(
            "cargo:warning=app icon not found ({}); building without one",
            candidates.join(" or ")
        );
        return;
    };
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
