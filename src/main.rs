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
        },
        Some("run") => {
            // `--wasm` builds the file to a WebAssembly bundle, serves it
            // locally, and opens it in the browser to play.
            // `--jit` uses the Cranelift JIT backend instead of the tree-walker.
            let wasm = args.iter().any(|a| a == "--wasm" || a == "--web");
            let use_jit = args.iter().any(|a| a == "--jit");
            let file = args[2..]
                .iter()
                .map(|s| s.as_str())
                .find(|a| a.ends_with(".ling"))
                .unwrap_or_else(|| {
                    eprintln!("Usage: ling run [--wasm|--jit] <file.ling>");
                    std::process::exit(1);
                });
            if wasm {
                run_wasm(file);
            } else if use_jit {
                run_file_jit(file);
            } else {
                run_file(file);
            }
        },

        Some("convert") => {
            // ling convert <asset> [-o out.ling] [--no-compression]
            std::process::exit(ling::convert::run(&args[1..]));
        },

        Some("ast") => {
            // ling ast [path] [--technical] [--artwork] [--ling] [--all] [--out <dir>]
            std::process::exit(run_ast(&args[2..]));
        },

        Some("build") => {
            let target = args.get(2).map(|s| s.as_str()).unwrap_or(".");
            let out = flag_value(&args, "--out").unwrap_or_else(|| "dist".into());
            let platforms = collect_platforms(&args);
            let icon = flag_value(&args, "--icon").map(PathBuf::from);
            let pack = args.iter().any(|a| a == "--pack");
            run_build(target, &out, &platforms, icon, pack);
        },
        Some(file) if file.ends_with(".ling") => run_file(file),
        _ => {
            println!("ling {} — The Omniglot Systems Language", ling::VERSION);
            println!("Usage:");
            println!("  ling run <file.ling>");
            println!("  ling run --wasm <file.ling>         build to WebAssembly, serve, open in browser");
            println!("  ling run --jit <file.ling>          run using Cranelift JIT backend");
            println!("  ling visualize <file.ling>          emit SVG AST to stdout");
            println!("  ling build <file.ling|dir> [opts]   compile to distributable");
            println!("    --out <dir>                       output folder (default: dist)");
            println!("    --platform <targets>              web win lin mac all (comma-sep)");
            println!("    --icon <file.svg|png|ico>         app icon (overrides manifest icon)");
            println!(
                "    --pack                            embed [includes] resources into the exe"
            );
            println!("  ling ast [path] [--technical|--artwork|--ling|--all]");
            println!(
                "                                      project-wide AST → SVG in ./AST/ (300 dpi)"
            );
            println!("  ling convert <asset> [opts]         transcode an asset → importable .ling");
            println!("    -o <out.ling>                     output path (default: <asset>.ling)");
            println!("    --no-compression                  emit plain arrays instead of blobs");
            println!("    (.gltf .glb .wav .ogg .flac .mid .svg .blend)");
        },
    }
}

fn run_file(path: &str) {
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
    if let Err(e) = ling::run_named(&source, src_dir, Some(path)) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run_file_jit(path: &str) {
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

    use ling::CompilerConfig;
    let config = CompilerConfig::default();
    let compiler = ling::LingCompiler::new(config);
    if let Err(e) = compiler.compile_and_run_jit(path) {
        eprintln!("JIT error: {:?}", e);
        std::process::exit(1);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// AST visualisation — `ling ast`
// ═══════════════════════════════════════════════════════════════════════════════

/// `ling ast [path] [--technical|--artwork|--ling|--all] [--out <dir>]`
///
/// Treats the whole project (a directory of `.ling` files, or a single file) as
/// one program while preserving file scope, then writes the requested SVG
/// style(s) to `<out>/` (default `./AST/`). Returns a process exit code.
fn run_ast(args: &[String]) -> i32 {
    use ling::astviz::AstStyle;

    let out_dir = flag_value(&Vec::from(args), "--out").unwrap_or_else(|| "AST".into());
    let mut styles: Vec<AstStyle> = Vec::new();
    let all = args.iter().any(|a| a == "--all");
    if all || args.iter().any(|a| a == "--technical") {
        styles.push(AstStyle::Technical);
    }
    if all || args.iter().any(|a| a == "--artwork") {
        styles.push(AstStyle::Artwork);
    }
    if all || args.iter().any(|a| a == "--ling") {
        styles.push(AstStyle::Ling);
    }
    if styles.is_empty() {
        // No style flag → produce all three.
        styles = vec![AstStyle::Technical, AstStyle::Artwork, AstStyle::Ling];
    }

    // First non-flag argument is the path (skipping the --out value); default ".".
    let path = pick_ast_path(args).unwrap_or_else(|| ".".into());

    let (proj_name, files) = gather_project(&path);
    if files.is_empty() {
        eprintln!("ling ast: no parseable .ling files found in '{path}'");
        return 1;
    }

    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("ling ast: cannot create '{out_dir}': {e}");
        return 1;
    }

    let n_fns: usize = files
        .iter()
        .map(|(_, p)| {
            p.items
                .iter()
                .filter(|i| matches!(i, ling::parser::ast::Item::Fn(_)))
                .count()
        })
        .sum();
    println!(
        "ling ast: '{proj_name}' — {} file(s), {n_fns} fn(s)",
        files.len()
    );

    for style in &styles {
        let svg = ling::astviz::render(*style, &proj_name, &files);
        let dst = std::path::Path::new(&out_dir).join(format!("{proj_name}.{}.svg", style.slug()));
        match std::fs::write(&dst, svg.as_bytes()) {
            Ok(()) => println!("  ✓ {}", dst.display()),
            Err(e) => {
                eprintln!("  ✗ {}: {e}", dst.display());
                return 1;
            },
        }
    }
    0
}

/// First non-flag argument that is not the value of `--out`.
fn pick_ast_path(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--out" {
            i += 2;
            continue;
        }
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        return Some(a.clone());
    }
    None
}

/// Gather the project as `(project_name, [(file_label, Program)])`. A directory is
/// walked recursively (skipping build/output trees); a single file yields one entry.
/// Files that fail to parse are skipped with a warning.
fn gather_project(path: &str) -> (String, Vec<(String, ling::parser::ast::Program)>) {
    let p = Path::new(path);
    let mut files = Vec::new();

    if p.is_file() {
        let name = p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "program".into());
        if let Some(prog) = parse_one(p) {
            files.push((
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                prog,
            ));
        }
        return (sanitise_name(&name), files);
    }

    // Directory: recurse for .ling files.
    let mut paths = Vec::new();
    collect_ling_files(p, &mut paths);
    paths.sort();
    let base = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    for fp in &paths {
        if let Some(prog) = parse_one(fp) {
            let label = fp
                .strip_prefix(&base)
                .unwrap_or(fp)
                .to_string_lossy()
                .into_owned();
            files.push((label, prog));
        }
    }
    let proj = p
        .canonicalize()
        .ok()
        .and_then(|c| c.file_name().map(|n| n.to_string_lossy().into_owned()))
        .or_else(|| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "project".into());
    (sanitise_name(&proj), files)
}

fn parse_one(path: &Path) -> Option<ling::parser::ast::Program> {
    let src = std::fs::read_to_string(path).ok()?;
    match ling::parser::parse(&src) {
        Ok(prog) => Some(prog),
        Err(e) => {
            eprintln!("  [skip] {}: parse error: {e}", path.display());
            None
        },
    }
}

fn collect_ling_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if matches!(
                name.as_ref(),
                ".ling-build" | "灵碑" | "target" | "dist" | ".git" | "node_modules" | "AST"
            ) {
                continue;
            }
            collect_ling_files(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("ling" | "灵" | "霊" | "령" | "ลิง")
        ) {
            out.push(path);
        }
    }
}

// ── arg helpers ───────────────────────────────────────────────────────────────

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
}

fn collect_platforms(args: &[String]) -> Vec<String> {
    let mut platforms: Vec<String> = args
        .windows(2)
        .filter(|w| w[0] == "--platform")
        .flat_map(|w| {
            w[1].split(',')
                .map(|s| s.trim().to_lowercase())
                .collect::<Vec<_>>()
        })
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
    if cfg!(target_os = "windows") {
        "win"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        "lin"
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Project discovery
// ═══════════════════════════════════════════════════════════════════════════════

/// Whether this is a graphical/windowed program (not headless).
#[derive(Debug, Clone, PartialEq)]
enum ProjKind {
    Bin,
    Web,
    Game,
    Ui,
    Ai,
    Crypto,
    Lib,
    Polyglot,
}

impl ProjKind {
    #[allow(dead_code)]
    fn default_platforms(&self) -> Vec<&'static str> {
        match self {
            ProjKind::Web => vec!["web"],
            _ => vec![native_platform(), "web"],
        }
    }
}

struct LingProject {
    name: String,
    version: String,
    kind: ProjKind,
    entry: PathBuf,        // absolute path to the entry .ling file
    source_dir: PathBuf,   // directory that contains the .ling sources
    build_dir: PathBuf,    // where the temp Rust build project lives
    icon: Option<PathBuf>, // app icon source from the manifest (svg/png/ico)
    includes: Vec<String>, // resource globs from the manifest [includes] block
}

fn discover_project(target: &str) -> LingProject {
    let raw = Path::new(target);
    let path = raw.canonicalize().unwrap_or_else(|_| raw.to_path_buf());

    if path.is_file() {
        // Single .ling file
        let name = sanitise_name(&path.file_stem().unwrap_or_default().to_string_lossy());
        let source_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let build_dir = source_dir.join(".ling-build").join(&name);
        LingProject {
            name,
            version: "0.1.0".into(),
            kind: ProjKind::Bin,
            entry: path,
            source_dir,
            build_dir,
            icon: None,
            includes: Vec::new(),
        }
    } else if path.is_dir() {
        // ling-fu project: 灵符.toml
        let lf = path.join("灵符.toml");
        if lf.exists() {
            return parse_lingfu_toml(&lf, &path);
        }

        // Simple project: Ling.toml
        let lt = path.join("Ling.toml");
        if lt.exists() {
            return parse_ling_toml(&lt, &path);
        }

        // Bare directory: auto-detect entry
        let entry = auto_entry(&path).unwrap_or_else(|| {
            eprintln!("error: no .ling file found in '{}'", path.display());
            std::process::exit(1);
        });
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "app".into());
        let build_dir = path.join(".ling-build").join(&name);
        LingProject {
            name,
            version: "0.1.0".into(),
            kind: ProjKind::Bin,
            entry,
            source_dir: path,
            build_dir,
            icon: None,
            includes: Vec::new(),
        }
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
    let text = std::fs::read_to_string(toml).unwrap_or_else(|e| {
        eprintln!("read 灵符.toml: {e}");
        std::process::exit(1);
    });

    let mut name = base
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "app".into());
    let mut version = "0.1.0".into();
    let mut kind = ProjKind::Bin;
    let mut icon: Option<PathBuf> = None;

    for line in text.lines() {
        let line = line.trim();
        // 名 = "value"  or  名 = value
        if let Some(v) = toml_kv(line, "名") {
            name = sanitise_name(&v);
        }
        if let Some(v) = toml_kv(line, "版") {
            version = v;
        }
        if let Some(v) = toml_kv(line, "型") {
            kind = parse_kind(&v);
        }
        // 图标 = "logo.svg"  (also accept the English `icon`), relative to project root.
        if let Some(v) = toml_kv(line, "图标").or_else(|| toml_kv(line, "icon")) {
            icon = Some(base.join(v));
        }
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
    let includes = parse_includes(&text);
    LingProject {
        name,
        version,
        kind,
        entry,
        source_dir,
        build_dir,
        icon,
        includes,
    }
}

/// Parse a simple English-key `Ling.toml`.
/// [project]
/// name = "my-app"
/// version = "0.1.0"
/// entry = "main.ling"
/// kind = "bin"
fn parse_ling_toml(toml: &Path, base: &Path) -> LingProject {
    let text = std::fs::read_to_string(toml).unwrap_or_else(|e| {
        eprintln!("read Ling.toml: {e}");
        std::process::exit(1);
    });

    let mut name = base
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "app".into());
    let mut version = "0.1.0".to_string();
    let mut entry_name = "main.ling".to_string();
    let mut kind = ProjKind::Bin;
    let mut icon: Option<PathBuf> = None;

    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = toml_kv(line, "name") {
            name = sanitise_name(&v);
        }
        if let Some(v) = toml_kv(line, "version") {
            version = v;
        }
        if let Some(v) = toml_kv(line, "entry") {
            entry_name = v;
        }
        if let Some(v) = toml_kv(line, "kind") {
            kind = parse_kind(&v);
        }
        // icon = "images/logo.svg"  (svg/png/ico), relative to the project root.
        if let Some(v) = toml_kv(line, "icon") {
            icon = Some(base.join(v));
        }
    }

    let entry = base.join(&entry_name);
    if !entry.exists() {
        eprintln!("error: entry '{}' not found", entry.display());
        std::process::exit(1);
    }
    let build_dir = base.join(".ling-build").join(&name);
    let includes = parse_includes(&text);
    LingProject {
        name,
        version,
        kind,
        entry,
        source_dir: base.to_path_buf(),
        build_dir,
        icon,
        includes,
    }
}

fn auto_entry(dir: &Path) -> Option<PathBuf> {
    for name in &["main.ling", "start.ling", "启.灵"] {
        let p = dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    // Try source sub-directory (ling-fu: 灵源/)
    for subdir in &["灵源", "src"] {
        let sub = dir.join(subdir);
        if let Some(e) = auto_entry(&sub) {
            return Some(e);
        }
    }
    // Fall back to any .ling file
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
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
    if !line.starts_with(&pat) {
        return None;
    }
    let v = line[pat.len()..].trim().trim_matches('"').to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

fn sanitise_name(s: &str) -> String {
    // Cargo package names: alphanumeric + hyphen/underscore, must not start with a digit
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
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

fn run_build(
    target: &str,
    out: &str,
    platforms: &[String],
    icon_override: Option<PathBuf>,
    pack: bool,
) {
    let project = discover_project(target);
    println!(
        "Building '{}' v{} ({:?})",
        project.name, project.version, project.kind
    );

    std::fs::create_dir_all(out).unwrap_or_else(|e| {
        eprintln!("create dist dir '{}': {e}", out);
        std::process::exit(1);
    });

    let icon = icon_override.as_deref();
    for platform in platforms {
        match platform.as_str() {
            "web" => build_web(&project, out),
            "win" | "windows" => build_native(&project, out, NativePlatform::Windows, icon, pack),
            "lin" | "linux" => build_native(&project, out, NativePlatform::Linux, icon, pack),
            "mac" | "macos" | "darwin" => {
                build_native(&project, out, NativePlatform::Mac, icon, pack)
            },
            other => {
                eprintln!("unknown platform '{}' — use web|win|lin|mac|all", other);
                std::process::exit(1);
            },
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

// ── `ling run --wasm` — build to WebAssembly, serve, and open in the browser ──

fn run_wasm(file: &str) {
    if !Path::new(file).exists() {
        eprintln!("[ling] error: file does not exist: {file}");
        std::process::exit(1);
    }

    // 1. Build the WebGL/WASM bundle next to the source (reuses `lingc webgl`).
    let out = std::env::temp_dir().join(format!("ling-wasm-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&out);
    let lingc = sibling_binary("lingc");
    println!("[ling] building WebAssembly bundle (first run compiles the runtime, ~1 min)…");
    let status = Command::new(&lingc)
        .arg("webgl")
        .arg(file)
        .arg("--out")
        .arg(&out)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("[ling] lingc not found ({e}); build it first: cargo build --bin lingc");
            std::process::exit(1);
        });
    if !status.success() {
        eprintln!("[ling] wasm build failed");
        std::process::exit(1);
    }

    // 2. Serve the folder locally and open the browser.
    serve_and_open(&out);
}

/// Minimal static file server (no deps) — serves `root` and opens the browser.
/// Blocks until Ctrl+C. WASM is served as `application/wasm` so the browser can
/// stream-compile it.
fn serve_and_open(root: &Path) {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:8080")
        .or_else(|_| TcpListener::bind("127.0.0.1:0"))
        .unwrap_or_else(|e| {
            eprintln!("[ling] could not bind a local port: {e}");
            std::process::exit(1);
        });
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(8080);
    let url = format!("http://localhost:{port}/index.html");

    println!("[ling] serving {} ", root.display());
    println!("[ling] ▶ {url}");
    println!("[ling] press Ctrl+C to stop.");
    open_browser(&url);

    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let raw = req
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("/");
            let path = raw.split('?').next().unwrap_or("/");
            let rel = if path == "/" {
                "index.html"
            } else {
                path.trim_start_matches('/')
            };

            // Resolve under root and reject path traversal.
            let target = root.join(rel);
            let safe = target.canonicalize().ok().filter(|p| {
                root.canonicalize()
                    .map(|r| p.starts_with(r))
                    .unwrap_or(false)
            });

            let (status_line, body, ctype) = match safe.and_then(|p| std::fs::read(p).ok()) {
                Some(bytes) => ("200 OK", bytes, mime_for(rel)),
                None => ("404 Not Found", b"404 not found".to_vec(), "text/plain"),
            };
            let header = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&body);
        });
    }
}

/// Content-Type for a served path (WASM must be `application/wasm`).
fn mime_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "json" => "application/json",
        "css" => "text/css; charset=utf-8",
        "ling" => "text/plain; charset=utf-8",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

/// Open `url` in the default browser (best-effort, per platform).
fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let _ = Command::new("cmd").args(["/C", "start", "", url]).status();
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(url).status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("xdg-open").arg(url).status();
}

// ── Native build ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum NativePlatform {
    Windows,
    Linux,
    Mac,
}

impl NativePlatform {
    fn dir_name(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Mac => "macos",
        }
    }

    /// Best triple for the platform, given where we're compiling FROM.
    fn triple(self) -> &'static str {
        match self {
            Self::Windows => {
                if cfg!(target_os = "windows") {
                    "x86_64-pc-windows-msvc"
                } else {
                    "x86_64-pc-windows-gnu"
                }
            },
            Self::Linux => {
                if cfg!(target_os = "linux") {
                    "x86_64-unknown-linux-gnu"
                } else {
                    "x86_64-unknown-linux-musl"
                }
            },
            Self::Mac => {
                // On Apple Silicon build arm64; everywhere else (including cross) use x86_64
                if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
                    "aarch64-apple-darwin"
                } else {
                    "x86_64-apple-darwin"
                }
            },
        }
    }

    fn exe_suffix(self) -> &'static str {
        match self {
            Self::Windows => ".exe",
            _ => "",
        }
    }

    fn is_current_host(self) -> bool {
        match self {
            Self::Windows => cfg!(target_os = "windows"),
            Self::Linux => cfg!(target_os = "linux"),
            Self::Mac => cfg!(target_os = "macos"),
        }
    }
}

fn build_native(
    project: &LingProject,
    out: &str,
    platform: NativePlatform,
    icon: Option<&Path>,
    pack: bool,
) {
    let triple = platform.triple();
    println!(
        "  [{}] building {} ({triple})…",
        platform.dir_name(),
        platform.dir_name()
    );

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
        eprintln!("  create build dir: {e}");
        std::process::exit(1);
    });

    // Copy all .ling files from source_dir (recurse one level for ling-fu 灵源/)
    copy_ling_sources(&project.source_dir, build_dir);

    // ── 2. Write generated Cargo.toml + src/main.rs ──────────────────────────
    let entry_filename = project
        .entry
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    std::fs::write(
        build_dir.join("Cargo.toml"),
        gen_cargo_toml(&project.name, &project.version, &ling_root),
    )
    .expect("write Cargo.toml");

    // Resources declared in the manifest [includes] block (glob-expanded).
    let resources = expand_includes(project);
    let do_pack = pack && !resources.is_empty();

    std::fs::write(
        build_dir.join("src/main.rs"),
        gen_main_rs(&entry_filename, do_pack),
    )
    .expect("write src/main.rs");

    // ── 2a. Pack resources into the executable (when --pack) ─────────────────
    if do_pack {
        write_packed_resources(build_dir, &resources);
        println!(
            "  packing {} resource(s) into the executable",
            resources.len()
        );
    } else if pack {
        println!("  --pack: no [includes] resources to pack");
    }

    // ── 2b. Build script + app icon ──────────────────────────────────────────
    // The generated build.rs embeds app.ico on Windows; we rasterize the icon
    // source (manifest / --icon / default logo) into app.ico beside it.
    std::fs::write(build_dir.join("build.rs"), gen_build_rs()).expect("write build.rs");
    if matches!(platform, NativePlatform::Windows) {
        match resolve_icon(icon, project, &ling_root) {
            Some(src) => {
                let out_ico = build_dir.join("app.ico");
                match ling_icon::write_ico(&src, &out_ico, ling_icon::DEFAULT_SIZES) {
                    Ok(()) => println!("  [windows] icon: {}", src.display()),
                    Err(e) => eprintln!("  [windows] icon skipped: {e}"),
                }
            },
            None => println!("  [windows] no icon found; building without one"),
        }
    }

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
    let src_bin = build_dir
        .join("target")
        .join(triple)
        .join("release")
        .join(&exe);
    let platform_dir = Path::new(out).join(platform.dir_name());
    std::fs::create_dir_all(&platform_dir).expect("create platform dir");
    let dst = platform_dir.join(&exe);
    std::fs::copy(&src_bin, &dst).unwrap_or_else(|e| {
        eprintln!("  copy binary {}: {e}", src_bin.display());
        std::process::exit(1);
    });
    println!("  [{}] → {}", platform.dir_name(), dst.display());

    // ── 6. Copy included resources next to the exe (unless packed inside it) ──
    if !do_pack && !resources.is_empty() {
        for (rel, abs) in &resources {
            let dst = platform_dir.join(rel);
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::copy(abs, &dst) {
                eprintln!("  resource copy {}: {e}", rel);
            }
        }
        println!(
            "  [{}] bundled {} resource(s)",
            platform.dir_name(),
            resources.len()
        );
    }
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
build = "build.rs"

[[bin]]
name = "{name}"
path = "src/main.rs"

[dependencies]
ling-lang = {{ path = "{root_str}" }}

[build-dependencies]
winresource = "0.1"

[profile.release]
lto = "fat"
codegen-units = 1
opt-level = 3
panic = "abort"
"#
    )
}

/// The build script written into each generated app crate. It embeds `app.ico`
/// (produced by `ling build`) into the Windows executable; on other targets, or
/// if no icon was generated, it does nothing.
fn gen_build_rs() -> String {
    r#"// Generated by `ling build` — embeds the app icon on Windows.
use std::path::Path;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if !Path::new("app.ico").exists() {
        return;
    }
    println!("cargo:rerun-if-changed=app.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("app.ico");
    if let Err(e) = res.compile() {
        println!("cargo:warning=icon embed skipped ({e})");
    }
}
"#
    .to_string()
}

/// Pick the icon source: explicit `--icon` flag, else the manifest field, else
/// the bundled default logo from the ling-lang repo. `None` if nothing is found.
fn resolve_icon(cli: Option<&Path>, project: &LingProject, ling_root: &Path) -> Option<PathBuf> {
    // Explicit --icon / manifest icon wins, but if the file is missing we warn
    // and fall through to the bundled default rather than shipping no icon.
    for candidate in [cli.map(Path::to_path_buf), project.icon.clone()]
        .into_iter()
        .flatten()
    {
        if candidate.exists() {
            return Some(candidate);
        }
        eprintln!(
            "  [icon] '{}' not found; using default",
            candidate.display()
        );
    }
    let default = ling_root.join("ling-lang.org/images/logo.svg");
    default.exists().then_some(default)
}

fn gen_main_rs(entry_file: &str, packed: bool) -> String {
    // When packing, pull in the generated resource table and self-extract it
    // before running so every path-based asset loader finds its files on disk.
    let res_mod = if packed { "mod resources;\n" } else { "" };
    let unpack = if packed {
        "    ling::unpack_resources(env!(\"CARGO_PKG_NAME\"), resources::RESOURCES);\n"
    } else {
        ""
    };
    format!(
        r#"// Built by ling build — no console window on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
{res_mod}
fn main() {{
    const SOURCE: &str = include_str!("../{entry_file}");
{unpack}    let lang = ling::detect_language(SOURCE);
    if lang != "English" {{
        eprintln!("[language: {{}}]", lang);
    }}
    if let Err(e) = ling::run(SOURCE) {{
        eprintln!("{{e}}");
        std::process::exit(1);
    }}
}}
"#
    )
}

// ── [includes] resource handling ───────────────────────────────────────────────

/// Parse the manifest `[includes]` block (or an inline `includes = [...]`).
/// Accepts the section form the user writes:
///   [includes]
///   "/music/*.wav",
///   "/font/*.otf",
/// Patterns are returned verbatim (leading `/`/`./` stripped later).
fn parse_includes(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_section = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            // [includes] (English) or [包含] (Chinese)
            in_section = line == "[includes]" || line == "[包含]";
            continue;
        }
        // Inline array form: includes = ["a", "b"]  /  包含 = [...]
        if let Some(rest) = line
            .strip_prefix("includes")
            .or_else(|| line.strip_prefix("包含"))
            .and_then(|r| r.trim_start().strip_prefix('='))
        {
            collect_quoted(rest, &mut out);
            continue;
        }
        if in_section {
            collect_quoted(line, &mut out);
        }
    }
    out
}

/// Push every `"..."`-quoted substring of `s` into `out`.
fn collect_quoted(s: &str, out: &mut Vec<String>) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            if let Some(end) = s[i + 1..].find('"') {
                let val = &s[i + 1..i + 1 + end];
                if !val.is_empty() {
                    out.push(val.to_string());
                }
                i += end + 2;
                continue;
            }
        }
        i += 1;
    }
}

/// Expand the manifest's include patterns against the project root into a list of
/// `(relative-path-with-forward-slashes, absolute-path)` files.
fn expand_includes(project: &LingProject) -> Vec<(String, PathBuf)> {
    let root = &project.source_dir;
    let mut all: Vec<(String, PathBuf)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let files = walk_files(root); // (rel, abs) for the whole tree, once

    for pat in &project.includes {
        let pat = pat.trim().trim_start_matches("./").trim_start_matches('/');
        if pat.is_empty() {
            continue;
        }
        let has_glob = pat.contains('*') || pat.contains('?');
        if !has_glob {
            let abs = root.join(pat);
            if abs.is_dir() {
                // A bare directory means "everything under it".
                let prefix = format!("{}/", pat.replace('\\', "/"));
                for (rel, abs) in &files {
                    if rel.starts_with(&prefix) && seen.insert(rel.clone()) {
                        all.push((rel.clone(), abs.clone()));
                    }
                }
            } else if abs.is_file() {
                let rel = pat.replace('\\', "/");
                if seen.insert(rel.clone()) {
                    all.push((rel, abs));
                }
            } else {
                eprintln!("  [includes] no match for '{pat}'");
            }
            continue;
        }
        let mut matched = false;
        for (rel, abs) in &files {
            if glob_match(pat, rel) && seen.insert(rel.clone()) {
                all.push((rel.clone(), abs.clone()));
                matched = true;
            }
        }
        if !matched {
            eprintln!("  [includes] no match for '{pat}'");
        }
    }
    all
}

/// Recursively list every file under `root` as `(relative-forward-slash, absolute)`,
/// skipping build/output directories.
fn walk_files(root: &Path) -> Vec<(String, PathBuf)> {
    fn rec(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                // Don't descend into generated/output trees.
                if matches!(
                    name.as_ref(),
                    ".ling-build" | "灵碑" | "target" | "dist" | ".git"
                ) {
                    continue;
                }
                rec(base, &path, out);
            } else if let Ok(rel) = path.strip_prefix(base) {
                out.push((rel.to_string_lossy().replace('\\', "/"), path.clone()));
            }
        }
    }
    let mut out = Vec::new();
    rec(root, root, &mut out);
    out
}

/// Glob match with `/`-aware segments: `*`/`?` stay within a path segment,
/// while a `**` segment matches across any number of segments.
fn glob_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let seg: Vec<&str> = path.split('/').collect();
    seg_match(&pat, &seg)
}

fn seg_match(pat: &[&str], seg: &[&str]) -> bool {
    match pat.first() {
        None => seg.is_empty(),
        Some(&"**") => {
            // Match zero or more path segments.
            (0..=seg.len()).any(|i| seg_match(&pat[1..], &seg[i..]))
        },
        Some(p) => match seg.first() {
            Some(s) if wildcard(p.as_bytes(), s.as_bytes()) => seg_match(&pat[1..], &seg[1..]),
            _ => false,
        },
    }
}

/// `*` (any run) and `?` (one char) matching within a single segment.
fn wildcard(pat: &[u8], s: &[u8]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let (mut star, mut mark) = (usize::MAX, 0);
    while si < s.len() {
        if pi < pat.len() && (pat[pi] == b'?' || pat[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < pat.len() && pat[pi] == b'*' {
            star = pi;
            mark = si;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len()
}

/// Copy each resource into `<build_dir>/res/<rel>` and generate
/// `src/resources.rs` mapping every relative path to its `include_bytes!`.
fn write_packed_resources(build_dir: &Path, resources: &[(String, PathBuf)]) {
    let res_root = build_dir.join("res");
    let mut entries = String::new();
    for (rel, abs) in resources {
        let dst = res_root.join(rel);
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::copy(abs, &dst) {
            eprintln!("  pack copy {rel}: {e}");
            continue;
        }
        // include_bytes! path is relative to src/resources.rs → ../res/<rel>.
        entries.push_str(&format!(
            "    ({rel:?}, include_bytes!(\"../res/{rel}\")),\n"
        ));
    }
    let module = format!(
        "// Generated by `ling build --pack`. Embedded resource table.\n\
         pub static RESOURCES: &[(&str, &[u8])] = &[\n{entries}];\n"
    );
    let _ = std::fs::write(build_dir.join("src/resources.rs"), module);
}

fn ensure_rustup_target(triple: &str) {
    let Ok(out) = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
    else {
        return;
    };
    let installed = String::from_utf8_lossy(&out.stdout);
    if !installed.contains(triple) {
        println!("    installing target {triple}…");
        let _ = Command::new("rustup")
            .args(["target", "add", triple])
            .status();
    }
}

/// Use `cross` for cross-compilation when available; otherwise fall back to `cargo`.
fn choose_build_tool(platform: NativePlatform) -> &'static str {
    if !platform.is_current_host() && has_cross() {
        "cross"
    } else {
        "cargo"
    }
}

fn has_cross() -> bool {
    Command::new("cross").arg("--version").output().is_ok()
}

/// Return the path to a sibling binary in the same directory as this executable.
fn sibling_binary(name: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let candidate =
            exe.parent()
                .unwrap_or(Path::new("."))
                .join(if cfg!(target_os = "windows") {
                    format!("{name}.exe")
                } else {
                    name.to_string()
                });
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from(name)
}

/// Locate the ling-lang repository root (the directory containing the root Cargo.toml).
fn find_ling_root() -> Option<PathBuf> {
    // 1. Explicit env var
    if let Ok(home) = std::env::var("LING_HOME") {
        let p = PathBuf::from(home);
        if p.join("Cargo.toml").exists() {
            return Some(p);
        }
    }
    // 2. Relative to this binary: target/{debug,release}/ling → 3 levels up
    if let Ok(exe) = std::env::current_exe() {
        if let Some(repo) = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            if repo.join("Cargo.toml").exists() {
                return Some(repo.to_path_buf());
            }
        }
    }
    // 3. Current working directory
    let cwd = std::env::current_dir().ok()?;
    if cwd.join("Cargo.toml").exists() {
        return Some(cwd);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_includes_section_form() {
        let toml = "\
[project]
name = \"game\"

[includes]
\"/music/*.wav\",
\"/font/*.otf\",
\"data.bin\"
";
        let inc = parse_includes(toml);
        assert_eq!(inc, vec!["/music/*.wav", "/font/*.otf", "data.bin"]);
    }

    #[test]
    fn parses_includes_inline_array() {
        let inc = parse_includes("includes = [\"a/*.png\", \"b.txt\"]\n");
        assert_eq!(inc, vec!["a/*.png", "b.txt"]);
    }

    #[test]
    fn includes_section_stops_at_next_header() {
        let toml = "[includes]\n\"a.txt\"\n[other]\n\"b.txt\"\n";
        assert_eq!(parse_includes(toml), vec!["a.txt"]);
    }

    #[test]
    fn glob_matches_within_and_across_segments() {
        assert!(glob_match("music/*.wav", "music/song.wav"));
        assert!(!glob_match("music/*.wav", "music/sub/song.wav")); // * doesn't cross '/'
        assert!(glob_match("music/**/*.wav", "music/a/b/song.wav")); // ** crosses
        assert!(glob_match("**/*.otf", "font/deep/x.otf"));
        assert!(glob_match("data?.bin", "data7.bin"));
        assert!(!glob_match("*.wav", "song.mp3"));
        assert!(glob_match("font/x.otf", "font/x.otf")); // exact
    }
}
