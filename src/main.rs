// src/main.rs — ling CLI entry point
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(|s| s.as_str()) {
        Some("visualize") => {
            let file = args.get(2).map(|s| s.as_str()).unwrap_or_else(|| {
                eprintln!("Usage: ling visualize <file.ling>");
                std::process::exit(1);
            });
            let source = std::fs::read_to_string(file).unwrap_or_else(|e| {
                eprintln!("error reading '{}': {}", file, e);
                std::process::exit(1);
            });
            let program = ling::parser::parse(&source).unwrap_or_else(|e| {
                eprintln!("parse error: {}", e);
                std::process::exit(1);
            });
            print!("{}", ling::visualize::render(file, &program));
        }
        Some("run") => {
            let file = args.get(2).map(|s| s.as_str()).unwrap_or_else(|| {
                eprintln!("Usage: ling run <file.ling>");
                std::process::exit(1);
            });
            run_file(file);
        }
        Some("build") => {
            let target = args.get(2).map(|s| s.as_str()).unwrap_or(".");
            let out     = flag_value(&args, "--out").unwrap_or_else(|| "dist".into());
            let platforms = collect_platforms(&args);
            run_build(target, &out, &platforms);
        }
        Some(file) if file.ends_with(".ling") => run_file(file),
        _ => {
            println!("ling {} — The Omniglot Systems Language", ling::VERSION);
            println!("Usage:");
            println!("  ling run <file.ling>");
            println!("  ling visualize <file.ling>          emit SVG AST to stdout");
            println!("  ling build <file.ling|dir> [opts]   compile to distributable");
            println!("    --out <dir>                       output folder (default: dist)");
            println!("    --platform <targets>              web win lin mac all (comma-sep)");
        }
    }
}

fn run_file(path: &str) {
    eprintln!("[ling] reading source file: {path}");
    let resolved = std::path::Path::new(path);
    if !resolved.exists() {
        eprintln!("[ling] error: file does not exist: {}", resolved.display());
        std::process::exit(1);
    }

    let source = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("error reading '{}': {}", path, e);
        std::process::exit(1);
    });

    let lang = ling::detect_language(&source);
    if lang != "English" {
        eprintln!("[detected language: {}]", lang);
    }
    let src_dir = resolved.parent().map(|p| p.to_path_buf());
    if let Err(e) = ling::run_file(&source, src_dir) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

// ── arg helpers ───────────────────────────────────────────────────────────────

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
}

fn collect_platforms(args: &[String]) -> Vec<String> {
    let mut platforms: Vec<String> = args.windows(2)
        .filter(|w| w[0] == "--platform")
        .flat_map(|w| w[1].split(',').map(|s| s.trim().to_lowercase()).collect::<Vec<_>>())
        .collect();
    // Expand "all"
    if platforms.iter().any(|p| p == "all") {
        return vec!["win".into(), "lin".into(), "mac".into(), "web".into()];
    }
    if platforms.is_empty() {
        // Default: current native platform + web
        platforms.push(native_platform().into());
        platforms.push("web".into());
    }
    platforms
}

fn native_platform() -> &'static str {
    if cfg!(target_os = "windows") { "win" }
    else if cfg!(target_os = "macos") { "mac" }
    else { "lin" }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Project discovery
// ═══════════════════════════════════════════════════════════════════════════════

/// Whether this is a graphical/windowed program (not headless).
#[derive(Debug, Clone, PartialEq)]
enum ProjKind { Bin, Web, Game, Ui, Ai, Crypto, Lib, Polyglot }

impl ProjKind {
    fn default_platforms(&self) -> Vec<&'static str> {
        match self {
            ProjKind::Web => vec!["web"],
            _ => vec![native_platform(), "web"],
        }
    }
}

struct LingProject {
    name:       String,
    version:    String,
    kind:       ProjKind,
    entry:      PathBuf,     // absolute path to the entry .ling file
    source_dir: PathBuf,     // directory that contains the .ling sources
    build_dir:  PathBuf,     // where the temp Rust build project lives
}

fn discover_project(target: &str) -> LingProject {
    let raw = Path::new(target);
    let path = raw.canonicalize().unwrap_or_else(|_| raw.to_path_buf());

    if path.is_file() {
        // Single .ling file
        let name = sanitise_name(&path.file_stem().unwrap_or_default()
            .to_string_lossy());
        let source_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let build_dir  = source_dir.join(".ling-build").join(&name);
        LingProject { name, version: "0.1.0".into(), kind: ProjKind::Bin,
                      entry: path, source_dir, build_dir }
    } else if path.is_dir() {
        // ling-fu project: 灵符.toml
        let lf = path.join("灵符.toml");
        if lf.exists() { return parse_lingfu_toml(&lf, &path); }

        // Simple project: Ling.toml
        let lt = path.join("Ling.toml");
        if lt.exists() { return parse_ling_toml(&lt, &path); }

        // Bare directory: auto-detect entry
        let entry = auto_entry(&path).unwrap_or_else(|| {
            eprintln!("error: no .ling file found in '{}'", path.display());
            std::process::exit(1);
        });
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "app".into());
        let build_dir = path.join(".ling-build").join(&name);
        LingProject { name, version: "0.1.0".into(), kind: ProjKind::Bin,
                      entry, source_dir: path, build_dir }
    } else {
        eprintln!("error: '{}' is not a .ling file or directory", target);
        std::process::exit(1);
    }
}

/// Parse a ling-fu `灵符.toml` manifest.
/// Expected shape:
///   [灵符]
///   名 = "my-app"
///   型 = "bin"
///   版 = "0.1.0"
fn parse_lingfu_toml(toml: &Path, base: &Path) -> LingProject {
    let text = std::fs::read_to_string(toml)
        .unwrap_or_else(|e| { eprintln!("read 灵符.toml: {e}"); std::process::exit(1); });

    let mut name    = base.file_name().map(|n| n.to_string_lossy().into_owned())
                          .unwrap_or_else(|| "app".into());
    let mut version = "0.1.0".into();
    let mut kind    = ProjKind::Bin;

    for line in text.lines() {
        let line = line.trim();
        // 名 = "value"  or  名 = value
        if let Some(v) = toml_kv(line, "名")  { name    = sanitise_name(&v); }
        if let Some(v) = toml_kv(line, "版")  { version = v; }
        if let Some(v) = toml_kv(line, "型")  { kind    = parse_kind(&v); }
    }

    // Entry point: 灵源/启.灵  (ling-fu convention)
    let entry = ["灵源/启.灵", "灵源/main.ling", "main.ling"]
        .iter()
        .map(|p| base.join(p))
        .find(|p| p.exists())
        .unwrap_or_else(|| {
            auto_entry(base).unwrap_or_else(|| {
                eprintln!("error: cannot find entry point for project '{}'", name);
                std::process::exit(1);
            })
        });

    let source_dir = base.to_path_buf();
    // ling-fu uses 灵碑/ for build artifacts
    let build_dir = base.join("灵碑").join(&name);
    LingProject { name, version, kind, entry, source_dir, build_dir }
}

/// Parse a simple English-key `Ling.toml`.
/// [project]
/// name = "my-app"
/// version = "0.1.0"
/// entry = "main.ling"
/// kind = "bin"
fn parse_ling_toml(toml: &Path, base: &Path) -> LingProject {
    let text = std::fs::read_to_string(toml)
        .unwrap_or_else(|e| { eprintln!("read Ling.toml: {e}"); std::process::exit(1); });

    let mut name       = base.file_name().map(|n| n.to_string_lossy().into_owned())
                             .unwrap_or_else(|| "app".into());
    let mut version    = "0.1.0".to_string();
    let mut entry_name = "main.ling".to_string();
    let mut kind       = ProjKind::Bin;

    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = toml_kv(line, "name")    { name       = sanitise_name(&v); }
        if let Some(v) = toml_kv(line, "version") { version    = v; }
        if let Some(v) = toml_kv(line, "entry")   { entry_name = v; }
        if let Some(v) = toml_kv(line, "kind")    { kind       = parse_kind(&v); }
    }

    let entry = base.join(&entry_name);
    if !entry.exists() {
        eprintln!("error: entry '{}' not found", entry.display());
        std::process::exit(1);
    }
    let build_dir = base.join(".ling-build").join(&name);
    LingProject { name, version, kind, entry, source_dir: base.to_path_buf(), build_dir }
}

fn auto_entry(dir: &Path) -> Option<PathBuf> {
    for name in &["main.ling", "start.ling", "启.灵"] {
        let p = dir.join(name);
        if p.exists() { return Some(p); }
    }
    // Try source sub-directory (ling-fu: 灵源/)
    for subdir in &["灵源", "src"] {
        let sub = dir.join(subdir);
        if let Some(e) = auto_entry(&sub) { return Some(e); }
    }
    // Fall back to any .ling file
    std::fs::read_dir(dir).ok()?.flatten()
        .find(|e| e.path().extension().map_or(false, |x| x == "ling"))
        .map(|e| e.path())
}

fn parse_kind(s: &str) -> ProjKind {
    match s.to_lowercase().as_str() {
        "web" | "网灵" => ProjKind::Web,
        "game" | "游灵" => ProjKind::Game,
        "ui" | "显灵" => ProjKind::Ui,
        "ai" | "智灵" => ProjKind::Ai,
        "crypto" | "密灵" => ProjKind::Crypto,
        "lib" | "共修" => ProjKind::Lib,
        "polyglot" | "万言" => ProjKind::Polyglot,
        _ => ProjKind::Bin,
    }
}

fn toml_kv(line: &str, key: &str) -> Option<String> {
    // key = "value"  or  key = value  (key may be unicode)
    let pat = format!("{key} =");
    if !line.starts_with(&pat) { return None; }
    let v = line[pat.len()..].trim().trim_matches('"').to_string();
    if v.is_empty() { None } else { Some(v) }
}

fn sanitise_name(s: &str) -> String {
    // Cargo package names: alphanumeric + hyphen/underscore, must not start with a digit
    let out: String = s.chars().map(|c| {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }
    }).collect();
    let out = out.trim_matches('-').to_string();
    let out = if out.is_empty() { "app".into() } else { out };
    if out.chars().next().map_or(false, |c| c.is_ascii_digit()) {
        format!("r{}", out)
    } else {
        out
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Build orchestration
// ═══════════════════════════════════════════════════════════════════════════════

fn run_build(target: &str, out: &str, platforms: &[String]) {
    let project = discover_project(target);
    println!("Building '{}' v{} ({:?})", project.name, project.version, project.kind);

    std::fs::create_dir_all(out).unwrap_or_else(|e| {
        eprintln!("create dist dir '{}': {e}", out); std::process::exit(1);
    });

    for platform in platforms {
        match platform.as_str() {
            "web"                     => build_web(&project, out),
            "win" | "windows"         => build_native(&project, out, NativePlatform::Windows),
            "lin" | "linux"           => build_native(&project, out, NativePlatform::Linux),
            "mac" | "macos" | "darwin" => build_native(&project, out, NativePlatform::Mac),
            other => {
                eprintln!("unknown platform '{}' — use web|win|lin|mac|all", other);
                std::process::exit(1);
            }
        }
    }

    println!("\nOutputs written to: {out}/");
}

// ── Web build ─────────────────────────────────────────────────────────────────

fn build_web(project: &LingProject, out: &str) {
    println!("  [web] building WebGL bundle…");

    // Delegate to `lingc webgl` — it lives next to this binary.
    let lingc = sibling_binary("lingc");
    let web_out = Path::new(out).join("web");

    // lingc webgl <entry.ling> --out <dist/web>
    let status = Command::new(&lingc)
        .arg("webgl")
        .arg(&project.entry)
        .arg("--out")
        .arg(&web_out)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("  lingc not found ({e}). Build lingc first: cargo build --bin lingc");
            std::process::exit(1);
        });

    if !status.success() {
        eprintln!("  [web] build failed");
        std::process::exit(1);
    }
    println!("  [web] → {}/web/", out);
}

// ── Native build ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum NativePlatform { Windows, Linux, Mac }

impl NativePlatform {
    fn dir_name(self) -> &'static str {
        match self { Self::Windows => "windows", Self::Linux => "linux", Self::Mac => "macos" }
    }

    /// Best triple for the platform, given where we're compiling FROM.
    fn triple(self) -> &'static str {
        match self {
            Self::Windows => {
                if cfg!(target_os = "windows") { "x86_64-pc-windows-msvc" }
                else { "x86_64-pc-windows-gnu" }
            }
            Self::Linux => {
                if cfg!(target_os = "linux") { "x86_64-unknown-linux-gnu" }
                else { "x86_64-unknown-linux-musl" }
            }
            Self::Mac => {
                // On Apple Silicon build arm64; everywhere else (including cross) use x86_64
                if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
                    "aarch64-apple-darwin"
                } else {
                    "x86_64-apple-darwin"
                }
            }
        }
    }

    fn exe_suffix(self) -> &'static str {
        match self { Self::Windows => ".exe", _ => "" }
    }

    fn is_current_host(self) -> bool {
        match self {
            Self::Windows => cfg!(target_os = "windows"),
            Self::Linux   => cfg!(target_os = "linux"),
            Self::Mac     => cfg!(target_os = "macos"),
        }
    }
}

fn build_native(project: &LingProject, out: &str, platform: NativePlatform) {
    let triple = platform.triple();
    println!("  [{}] building {} ({triple})…", platform.dir_name(), platform.dir_name());

    let ling_root = find_ling_root().unwrap_or_else(|| {
        eprintln!(
            "  cannot find ling-lang source.\n  \
             Run from the repository or set LING_HOME=<path-to-ling-repo>."
        );
        std::process::exit(1);
    });

    // ── 1. Set up build directory ────────────────────────────────────────────
    let build_dir = &project.build_dir;
    std::fs::create_dir_all(build_dir.join("src")).unwrap_or_else(|e| {
        eprintln!("  create build dir: {e}"); std::process::exit(1);
    });

    // Copy all .ling files from source_dir (recurse one level for ling-fu 灵源/)
    copy_ling_sources(&project.source_dir, build_dir);

    // ── 2. Write generated Cargo.toml + src/main.rs ──────────────────────────
    let entry_filename = project.entry.file_name()
        .unwrap_or_default().to_string_lossy().into_owned();

    std::fs::write(
        build_dir.join("Cargo.toml"),
        gen_cargo_toml(&project.name, &project.version, &ling_root),
    ).expect("write Cargo.toml");

    std::fs::write(
        build_dir.join("src/main.rs"),
        gen_main_rs(&entry_filename),
    ).expect("write src/main.rs");

    // ── 3. Ensure rustup target is installed ─────────────────────────────────
    ensure_rustup_target(triple);

    // ── 4. Build ─────────────────────────────────────────────────────────────
    let build_cmd = choose_build_tool(platform);
    let status = Command::new(build_cmd)
        .args(["build", "--release", "--target", triple])
        .current_dir(build_dir)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("  {build_cmd}: {e}");
            if build_cmd == "cross" {
                eprintln!("  install cross: cargo install cross  (requires Docker)");
            }
            std::process::exit(1);
        });

    if !status.success() {
        eprintln!("  [{}] build failed.", platform.dir_name());
        if !platform.is_current_host() && build_cmd == "cargo" && !has_cross() {
            eprintln!("  Tip: install `cross` for cross-compilation (needs Docker):");
            eprintln!("       cargo install cross");
        }
        std::process::exit(1);
    }

    // ── 5. Copy binary to dist/<platform>/ ──────────────────────────────────
    let exe = format!("{}{}", project.name, platform.exe_suffix());
    let src_bin = build_dir.join("target").join(triple).join("release").join(&exe);
    let platform_dir = Path::new(out).join(platform.dir_name());
    std::fs::create_dir_all(&platform_dir).expect("create platform dir");
    let dst = platform_dir.join(&exe);
    std::fs::copy(&src_bin, &dst).unwrap_or_else(|e| {
        eprintln!("  copy binary {}: {e}", src_bin.display());
        std::process::exit(1);
    });
    println!("  [{}] → {}", platform.dir_name(), dst.display());
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Recursively collect all .ling files from `src` and copy them flat into `dst`.
/// Also descends into 灵源/ and src/ sub-directories (ling-fu convention).
fn copy_ling_sources(src: &Path, dst: &Path) {
    let Ok(entries) = std::fs::read_dir(src) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let dname = path.file_name().unwrap_or_default().to_string_lossy();
            if dname == "灵源" || dname == "src" {
                copy_ling_sources(&path, dst);
            }
        } else if path.extension().map_or(false, |e| e == "ling") {
            if let Some(name) = path.file_name() {
                let _ = std::fs::copy(&path, dst.join(name));
            }
        }
    }
}

fn gen_cargo_toml(name: &str, version: &str, ling_root: &Path) -> String {
    // On Windows canonicalize gives UNC paths; forward-slashes work fine in TOML paths.
    let root_str = ling_root.display().to_string().replace('\\', "/");
    format!(
r#"[package]
name = "{name}"
version = "{version}"
edition = "2021"

[[bin]]
name = "{name}"
path = "src/main.rs"

[dependencies]
ling-lang = {{ path = "{root_str}" }}

[profile.release]
lto = "fat"
codegen-units = 1
opt-level = 3
panic = "abort"
"#)
}

fn gen_main_rs(entry_file: &str) -> String {
    format!(
r#"// Built by ling build — no console window on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {{
    const SOURCE: &str = include_str!("../{entry_file}");
    let lang = ling::detect_language(SOURCE);
    if lang != "English" {{
        eprintln!("[language: {{}}]", lang);
    }}
    if let Err(e) = ling::run(SOURCE) {{
        eprintln!("{{e}}");
        std::process::exit(1);
    }}
}}
"#)
}

fn ensure_rustup_target(triple: &str) {
    let Ok(out) = Command::new("rustup").args(["target", "list", "--installed"]).output()
    else { return };
    let installed = String::from_utf8_lossy(&out.stdout);
    if !installed.contains(triple) {
        println!("    installing target {triple}…");
        let _ = Command::new("rustup").args(["target", "add", triple]).status();
    }
}

/// Use `cross` for cross-compilation when available; otherwise fall back to `cargo`.
fn choose_build_tool(platform: NativePlatform) -> &'static str {
    if !platform.is_current_host() && has_cross() { "cross" } else { "cargo" }
}

fn has_cross() -> bool {
    Command::new("cross").arg("--version").output().is_ok()
}

/// Return the path to a sibling binary in the same directory as this executable.
fn sibling_binary(name: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let candidate = exe.parent().unwrap_or(Path::new(".")).join(
            if cfg!(target_os = "windows") { format!("{name}.exe") } else { name.to_string() }
        );
        if candidate.exists() { return candidate; }
    }
    PathBuf::from(name)
}

/// Locate the ling-lang repository root (the directory containing the root Cargo.toml).
fn find_ling_root() -> Option<PathBuf> {
    // 1. Explicit env var
    if let Ok(home) = std::env::var("LING_HOME") {
        let p = PathBuf::from(home);
        if p.join("Cargo.toml").exists() { return Some(p); }
    }
    // 2. Relative to this binary: target/{debug,release}/ling → 3 levels up
    if let Ok(exe) = std::env::current_exe() {
        if let Some(repo) = exe.parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            if repo.join("Cargo.toml").exists() { return Some(repo.to_path_buf()); }
        }
    }
    // 3. Current working directory
    let cwd = std::env::current_dir().ok()?;
    if cwd.join("Cargo.toml").exists() { return Some(cwd); }
    None
}
