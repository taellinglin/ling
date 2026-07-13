use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(|s| s.as_str()) {
        Some("run") => {
            let file = args.get(2).map(|s| s.as_str()).unwrap_or_else(|| {
                eprintln!("Usage: lingc run <file.ling>");
                std::process::exit(1);
            });
            run_file(file);
        },
        Some("compile") => {
            let file = args.get(2).map(|s| s.as_str()).unwrap_or_else(|| {
                eprintln!("Usage: lingc compile <file.ling> [-O 0|1|2|3] [--run]");
                std::process::exit(1);
            });
            let opt = parse_opt(&args);
            let run_after = args.contains(&"--run".to_string());
            compile_file(file, opt, run_after);
        },
        Some("mir") => {
            let file = args.get(2).map(|s| s.as_str()).unwrap_or_else(|| {
                eprintln!("Usage: lingc mir <file.ling> [-O 0|1|2|3]");
                std::process::exit(1);
            });
            let opt = parse_opt(&args);
            dump_mir(file, opt);
        },
        Some("webgl") => {
            let file = args.get(2).map(|s| s.as_str()).unwrap_or_else(|| {
                eprintln!("Usage: lingc webgl <file.ling> [--out <dir>]");
                std::process::exit(1);
            });
            let out_dir = parse_out_flag(&args).unwrap_or_else(|| {
                Path::new(file)
                    .file_stem()
                    .map(|s| format!("{}-web", s.to_string_lossy()))
                    .unwrap_or_else(|| "webgl-out".into())
            });
            build_webgl(file, &out_dir);
        },
        Some(file) if file.ends_with(".ling") => run_file(file),
        _ => {
            println!("lingc {} — Ling Compiler", ling::VERSION);
            println!("Usage:");
            println!("  lingc run <file.ling>                  interpret and run");
            println!(
                "  lingc compile <file.ling> [-O N] [--run]   compile (and optionally run via VM)"
            );
            println!("  lingc mir <file.ling> [-O N]           dump MIR");
            println!("  lingc webgl <file.ling> [--out d]      build WebGL folder");
            println!("  lingc <file.ling>                      same as run");
        },
    }
}

fn parse_opt(args: &[String]) -> u8 {
    args.windows(2)
        .find(|w| w[0] == "-O")
        .and_then(|w| w.get(1))
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(1)
}

fn parse_out_flag(args: &[String]) -> Option<String> {
    args.windows(2)
        .find(|w| w[0] == "--out")
        .map(|w| w[1].clone())
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

fn compile_file(path: &str, opt_level: u8, run_after: bool) {
    let config = ling::core::CompilerConfig {
        optimization: match opt_level {
            0 => ling::core::OptimizationLevel::None,
            1 => ling::core::OptimizationLevel::O1,
            2 => ling::core::OptimizationLevel::O2,
            _ => ling::core::OptimizationLevel::O3,
        },
    };
    let compiler = ling::core::LingCompiler::new(config);

    if run_after {
        match compiler.compile_and_run(path) {
            Ok(()) => {},
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            },
        }
    } else {
        let out = Path::new(path).with_extension("lingbc");
        let out_str = out.to_string_lossy().into_owned();
        match compiler.compile(path, &*out_str) {
            Ok(()) => {},
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            },
        }
    }
}

fn dump_mir(path: &str, opt_level: u8) {
    let config = ling::core::CompilerConfig {
        optimization: match opt_level {
            0 => ling::core::OptimizationLevel::None,
            1 => ling::core::OptimizationLevel::O1,
            2 => ling::core::OptimizationLevel::O2,
            _ => ling::core::OptimizationLevel::O3,
        },
    };
    let compiler = ling::core::LingCompiler::new(config);
    match compiler.dump_mir(path) {
        Ok(()) => {},
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        },
    }
}

// ── WebGL build ───────────────────────────────────────────────────────────────

fn build_webgl(ling_file: &str, out_dir: &str) {
    let source = std::fs::read_to_string(ling_file).unwrap_or_else(|e| {
        eprintln!("error reading '{}': {e}", ling_file);
        std::process::exit(1);
    });

    std::fs::create_dir_all(out_dir).unwrap_or_else(|e| {
        eprintln!("cannot create output dir '{}': {e}", out_dir);
        std::process::exit(1);
    });

    let wasm_crate = find_ling_wasm_crate().unwrap_or_else(|| {
        eprintln!(
            "cannot find crates/ling-wasm/. \
             Run from the Ling repository root or set LING_HOME."
        );
        std::process::exit(1);
    });

    if !has_wasm_pack() {
        eprintln!(
            "wasm-pack not found. Install it with:\n  cargo install wasm-pack\n  \
             or visit https://rustwasm.github.io/wasm-pack/installer/"
        );
        std::process::exit(1);
    }

    println!("Building WASM (this may take a minute on first run)...");
    let status = Command::new("wasm-pack")
        .args(["build", "--target", "no-modules", "--release"])
        .arg(&wasm_crate)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("wasm-pack failed: {e}");
            std::process::exit(1);
        });

    if !status.success() {
        eprintln!("wasm-pack build failed (exit {:?})", status.code());
        std::process::exit(1);
    }

    let pkg_dir = wasm_crate.join("pkg");
    for filename in &["ling_wasm.js", "ling_wasm_bg.wasm"] {
        let src = pkg_dir.join(filename);
        let dst = Path::new(out_dir).join(filename);
        std::fs::copy(&src, &dst).unwrap_or_else(|e| {
            eprintln!("copy {} → {}: {e}", src.display(), dst.display());
            std::process::exit(1);
        });
    }

    std::fs::write(Path::new(out_dir).join("program.ling"), &source).unwrap_or_else(|e| {
        eprintln!("write program.ling: {e}");
        std::process::exit(1);
    });

    // Collect all transitive `use` dependencies and bundle as modules.json
    let entry_dir = Path::new(ling_file)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let modules = collect_ling_modules(&source, &entry_dir);
    let modules_json = serde_json_modules(&modules);
    std::fs::write(Path::new(out_dir).join("modules.json"), modules_json).unwrap_or_else(|e| {
        eprintln!("write modules.json: {e}");
        std::process::exit(1);
    });

    std::fs::write(Path::new(out_dir).join("worker.js"), WORKER_JS).unwrap_or_else(|e| {
        eprintln!("write worker.js: {e}");
        std::process::exit(1);
    });

    let title = Path::new(ling_file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Ling".into());
    let html = INDEX_HTML.replace("{{TITLE}}", &title);
    std::fs::write(Path::new(out_dir).join("index.html"), html).unwrap_or_else(|e| {
        eprintln!("write index.html: {e}");
        std::process::exit(1);
    });

    println!("Done! Output folder: {out_dir}/");
    println!("  Upload the folder to any static web server and open index.html.");
    println!("  Local preview: python -m http.server --directory {out_dir}");
}

fn has_wasm_pack() -> bool {
    Command::new("wasm-pack").arg("--version").output().is_ok()
}

/// Scan a Ling source string for bare `use "path"` items (any keyword variant)
/// and return the quoted path strings.
fn scan_use_paths(source: &str) -> Vec<String> {
    // Use keywords across all supported human languages (mirrors the parser).
    let use_kws = [
        "use",
        "ใช้",
        "载",
        "引",
        "使う",
        "사용",
        "використати",
        "использовать",
        "استخدم",
        "用",
        "להשתמש",
        "उपयोग",
    ];
    let mut paths = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        for kw in &use_kws {
            if trimmed.starts_with(kw) {
                let rest = trimmed[kw.len()..].trim_start();
                if rest.starts_with('"') {
                    if let Some(end) = rest[1..].find('"') {
                        paths.push(rest[1..end + 1].to_string());
                    }
                }
                break;
            }
        }
    }
    paths
}

/// Recursively collect all `.ling` modules reachable from `source`.
/// Returns a map of module-path-key → source-string.
fn collect_ling_modules(
    source: &str,
    base_dir: &Path,
) -> std::collections::HashMap<String, String> {
    let mut modules: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut queue: Vec<(String, PathBuf)> = Vec::new();
    for path in scan_use_paths(source) {
        queue.push((path, base_dir.to_path_buf()));
    }
    while let Some((path, dir)) = queue.pop() {
        if modules.contains_key(&path) {
            continue;
        }
        let extensions = ["ling", "灵", "령", "霊", "ลิง"];
        let candidates: Vec<PathBuf> = extensions
            .iter()
            .map(|ext| dir.join(format!("{path}.{ext}")))
            .chain(std::iter::once(dir.join(&path)))
            .collect();
        let file_path = match candidates.into_iter().find(|p| p.exists()) {
            Some(p) => p,
            None => {
                eprintln!("[lingc] warning: cannot find module '{path}' (skipped)");
                continue;
            },
        };
        let content = match std::fs::read_to_string(&file_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[lingc] warning: cannot read module '{path}': {e} (skipped)");
                continue;
            },
        };
        let sub_dir = file_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| dir.clone());
        for sub_path in scan_use_paths(&content) {
            if !modules.contains_key(&sub_path) {
                queue.push((sub_path, sub_dir.clone()));
            }
        }
        modules.insert(path, content);
    }
    modules
}

/// Serialize a module map to JSON without an external dep.
fn serde_json_modules(modules: &std::collections::HashMap<String, String>) -> String {
    let mut out = String::from("{\n");
    let entries: Vec<_> = modules.iter().collect();
    for (i, (path, src)) in entries.iter().enumerate() {
        let comma = if i + 1 < entries.len() { "," } else { "" };
        out.push_str(&format!(
            "  {}: {}{}\n",
            json_str(path),
            json_str(src),
            comma
        ));
    }
    out.push('}');
    out
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn find_ling_wasm_crate() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("LING_HOME") {
        let p = PathBuf::from(home).join("crates/ling-wasm");
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(repo) = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            let p = repo.join("crates/ling-wasm");
            if p.exists() {
                return Some(p);
            }
        }
    }
    let p = PathBuf::from("crates/ling-wasm");
    if p.exists() {
        return Some(p);
    }
    None
}

const WORKER_JS: &str = r#"// worker.js — runs the Ling interpreter inside a Web Worker
importScripts('./ling_wasm.js');

// Forward console logs and errors to the main thread for debugging
const _log = console.log;
console.log = function(...args) {
    self.postMessage({ type: 'log', text: args.join(' ') });
    _log.apply(console, args);
};
const _error = console.error;
console.error = function(...args) {
    self.postMessage({ type: 'error', text: args.join(' ') });
    _error.apply(console, args);
};

let wasmReady = false;
let pendingCanvas = null;
let pendingRun = null;
wasm_bindgen('./ling_wasm_bg.wasm').then(() => {
    wasmReady = true;
    if (pendingCanvas !== null) {
        wasm_bindgen.init_canvas(pendingCanvas);
        pendingCanvas = null;
    }
    if (pendingRun !== null) {
        startRun(pendingRun.source, pendingRun.modules);
        pendingRun = null;
    }
}).catch(err => {
    console.error('wasm-bindgen init failed:', err);
});
function startRun(source, modules) {
    if (modules && typeof modules === 'object') {
        for (const [path, src] of Object.entries(modules)) {
            wasm_bindgen.register_module(path, src);
        }
    }
    wasm_bindgen.run_program(source);
}
self.onmessage = function(e) {
    const { type } = e.data;
    if (type === 'init') {
        const width = e.data.width || 800;
        const height = e.data.height || 600;
        const offscreen = new OffscreenCanvas(width, height);
        if (wasmReady) {
            wasm_bindgen.init_canvas(offscreen);
        } else {
            pendingCanvas = offscreen;
        }
    } else if (type === 'run') {
        if (wasmReady) {
            startRun(e.data.source, e.data.modules);
        } else {
            pendingRun = { source: e.data.source, modules: e.data.modules };
        }
    }
};
"#;

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{{TITLE}}</title>
  <style>
    *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
    html, body { width: 100%; height: 100%; background: #000; overflow: hidden; }
    #canvas { display: block; position: absolute; top: 50%; left: 50%; transform: translate(-50%, -50%); max-width: 100vw; max-height: 100vh; width: auto; height: auto; }
    #status { position: fixed; bottom: 8px; right: 12px; color: #888; font: 12px/1 monospace; z-index: 10; }
    #log-overlay { position: absolute; top: 10px; left: 10px; right: 10px; max-height: 40%; overflow-y: auto; font: 11px/1.4 monospace; color: #0f0; background: rgba(0, 0, 0, 0.6); pointer-events: none; padding: 8px; border-radius: 4px; z-index: 20; }
    .log-err { color: #f55; }
  </style>
</head>
<body>
  <canvas id="canvas"></canvas>
  <div id="status">loading…</div>
  <div id="log-overlay"></div>
  <script>
    const canvas = document.getElementById('canvas');
    const status = document.getElementById('status');
    const overlay = document.getElementById('log-overlay');
    
    function logToOverlay(text, isErr = false) {
      const line = document.createElement('div');
      line.textContent = text;
      if (isErr) line.className = 'log-err';
      overlay.appendChild(line);
      overlay.scrollTop = overlay.scrollHeight;
    }
    
    canvas.width  = window.innerWidth;
    canvas.height = window.innerHeight;
    const worker = new Worker('worker.js');
    const ctx = canvas.getContext('bitmaprenderer');
    
    worker.onmessage = function(e) {
      if (e.data.type === 'log') {
        logToOverlay(e.data.text);
      } else if (e.data.type === 'error') {
        logToOverlay(e.data.text, true);
      } else if (e.data.type === 'frame') {
        ctx.transferFromImageBitmap(e.data.bitmap);
      }
    };
    
    worker.postMessage({ type: 'init', width: canvas.width, height: canvas.height });
    fetch('program.ling')
      .then(r => { if (!r.ok) throw new Error(r.statusText); return r.text(); })
      .then(source => {
        status.textContent = 'running';
        return fetch('modules.json')
          .then(r => r.ok ? r.json() : {})
          .catch(() => ({}))
          .then(modules => {
            worker.postMessage({ type: 'run', source, modules });
          });
      })
      .catch(err => {
        status.textContent = 'error: ' + err.message;
        logToOverlay('Fetch error: ' + err.message, true);
        console.error(err);
      });
    worker.onerror = (e) => {
      status.textContent = 'worker error: ' + e.message;
      logToOverlay('Worker error: ' + e.message, true);
      console.error('worker error', e);
    };
  </script>
</body>
</html>
"#;
