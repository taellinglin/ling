// build.rs — embed the shared Ling icon (repo-root assets/ling.ico) into every
// lingfu binary on Windows. Missing icon / no resource compiler only warns.
use std::env;
use std::path::Path;

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    // crates/ling-fu → ../../assets/ling.ico
    let ico = Path::new(&manifest).join("../../assets/ling.ico");
    if !ico.exists() {
        println!(
            "cargo:warning=icon '{}' not found; lingfu built without an app icon",
            ico.display()
        );
        return;
    }
    println!("cargo:rerun-if-changed=../../assets/ling.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(&ico.to_string_lossy());
    if let Err(e) = res.compile() {
        println!("cargo:warning=icon embed skipped ({e})");
    }
}
