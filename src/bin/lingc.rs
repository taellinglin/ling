// src/bin/lingc.rs — Ling compiler driver
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
        }
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
        }
        Some(file) if file.ends_with(".ling") => run_file(file),
        _ => {
            println!("lingc {} — Ling Compiler", ling::VERSION);
            println!("Usage:");
            println!("  lingc run <file.ling>              interpret and run");
            println!("  lingc webgl <file.ling> [--out d]  build WebGL folder");
            println!("  lingc <file.ling>                  same as run");
        }
    }
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

// ── WebGL build ───────────────────────────────────────────────────────────────

fn build_webgl(ling_file: &str, out_dir: &str) {
    let source = std::fs::read_to_string(ling_file).unwrap_or_else(|e| {
        eprintln!("error reading '{}': {e}", ling_file);
        std::process::exit(1);
    });

    // Create output directory
    std::fs::create_dir_all(out_dir).unwrap_or_else(|e| {
        eprintln!("cannot create output dir '{}': {e}", out_dir);
        std::process::exit(1);
    });

    // Locate the ling-wasm crate relative to this binary
    let wasm_crate = find_ling_wasm_crate().unwrap_or_else(|| {
        eprintln!(
            "cannot find crates/ling-wasm/. \
             Run from the Ling repository root or set LING_HOME."
        );
        std::process::exit(1);
    });

    // Check wasm-pack is available
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
        .unwrap_or_else(|e| { eprintln!("wasm-pack failed: {e}"); std::process::exit(1); });

    if !status.success() {
        eprintln!("wasm-pack build failed (exit {:?})", status.code());
        std::process::exit(1);
    }

    // Copy pkg outputs to out_dir
    let pkg_dir = wasm_crate.join("pkg");
    for filename in &["ling_wasm.js", "ling_wasm_bg.wasm"] {
        let src = pkg_dir.join(filename);
        let dst = Path::new(out_dir).join(filename);
        std::fs::copy(&src, &dst).unwrap_or_else(|e| {
            eprintln!("copy {} → {}: {e}", src.display(), dst.display());
            std::process::exit(1);
        });
    }

    // Write program.ling
    std::fs::write(Path::new(out_dir).join("program.ling"), &source).unwrap_or_else(|e| {
        eprintln!("write program.ling: {e}");
        std::process::exit(1);
    });

    // Write worker.js
    std::fs::write(Path::new(out_dir).join("worker.js"), WORKER_JS).unwrap_or_else(|e| {
        eprintln!("write worker.js: {e}");
        std::process::exit(1);
    });

    // Write index.html
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

fn find_ling_wasm_crate() -> Option<PathBuf> {
    // 1. LING_HOME env var
    if let Ok(home) = std::env::var("LING_HOME") {
        let p = PathBuf::from(home).join("crates/ling-wasm");
        if p.exists() { return Some(p); }
    }
    // 2. Relative to executable (covers `cargo run --bin lingc` from repo root)
    if let Ok(exe) = std::env::current_exe() {
        // target/debug/lingc  →  ../../..  == repo root
        if let Some(repo) = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
            let p = repo.join("crates/ling-wasm");
            if p.exists() { return Some(p); }
        }
    }
    // 3. Current working directory
    let p = PathBuf::from("crates/ling-wasm");
    if p.exists() { return Some(p); }
    None
}

// ── Embedded templates ────────────────────────────────────────────────────────

const WORKER_JS: &str = r#"// worker.js — runs the Ling interpreter inside a Web Worker
// The main thread transfers an OffscreenCanvas; the WASM runtime renders to it via WebGL2.

importScripts('./ling_wasm.js');

let wasmReady = false;
let pendingCanvas = null;
let pendingSource = null;

// Queue both canvas and source until wasm is instantiated — calling exported
// functions before the wasm_bindgen() promise resolves fails with "wasm is undefined".
wasm_bindgen('./ling_wasm_bg.wasm').then(() => {
    wasmReady = true;
    if (pendingCanvas !== null) {
        wasm_bindgen.init_canvas(pendingCanvas);
        pendingCanvas = null;
    }
    if (pendingSource !== null) {
        wasm_bindgen.run_program(pendingSource);
        pendingSource = null;
    }
});

self.onmessage = function(e) {
    const { type } = e.data;
    if (type === 'init') {
        if (wasmReady) {
            wasm_bindgen.init_canvas(e.data.canvas);
        } else {
            pendingCanvas = e.data.canvas;
        }
    } else if (type === 'run') {
        if (wasmReady) {
            wasm_bindgen.run_program(e.data.source);
        } else {
            pendingSource = e.data.source;
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
    #canvas {
      display: block;
      position: absolute;
      top: 50%; left: 50%;
      transform: translate(-50%, -50%);
      /* Scale canvas to fit viewport while preserving aspect ratio */
      max-width: 100vw;
      max-height: 100vh;
      width: auto;
      height: auto;
    }
    #status {
      position: fixed; bottom: 8px; right: 12px;
      color: #888; font: 12px/1 monospace;
    }
  </style>
</head>
<body>
  <!-- Canvas starts at window size; Ling's open_window/open_fullscreen may resize it -->
  <canvas id="canvas"></canvas>
  <div id="status">loading…</div>

  <script>
    const canvas = document.getElementById('canvas');
    const status = document.getElementById('status');

    // Size the canvas backing store to the window so the Ling program's
    // open_fullscreen() reads the correct dimensions from canvas_size().
    canvas.width  = window.innerWidth;
    canvas.height = window.innerHeight;

    const worker = new Worker('worker.js');

    // Transfer canvas control to the worker (enables OffscreenCanvas + WebGL in worker)
    const offscreen = canvas.transferControlToOffscreen();
    worker.postMessage({ type: 'init', canvas: offscreen }, [offscreen]);

    fetch('program.ling')
      .then(r => { if (!r.ok) throw new Error(r.statusText); return r.text(); })
      .then(source => {
        status.textContent = 'running';
        worker.postMessage({ type: 'run', source });
      })
      .catch(err => {
        status.textContent = 'error: ' + err.message;
        console.error(err);
      });

    worker.onerror = (e) => {
      status.textContent = 'worker error: ' + e.message;
      console.error('worker error', e);
    };
  </script>
</body>
</html>
"#;
