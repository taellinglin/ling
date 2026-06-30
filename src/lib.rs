// src/lib.rs - Public API entry point
// Logo + favicon for the docs.rs page (the crate's public API docs).
#![doc(
    html_logo_url = "https://ling-lang.org/images/logo.svg",
    html_favicon_url = "https://ling-lang.org/images/logo.svg"
)]

pub mod astviz;
pub mod borrowck;
pub mod codegen;
#[cfg(not(target_arch = "wasm32"))]
pub mod convert;
pub mod core;
pub mod diag;
pub mod entry;
pub mod gfx;
pub mod lexer;
pub mod lexicon;
pub mod mir;
pub mod parser;
pub mod polyglot;
pub mod runtime;
pub mod semantic;
pub mod utils;
pub mod visualize;

#[cfg(not(target_arch = "wasm32"))]
pub use ling_audio;

// Re-exports
pub use core::{CompilerConfig, LingCompiler, OptimizationLevel};
pub use lexicon::{CanonicalToken, Lexicon, LexiconRegistry};
pub use polyglot::{normalize_source, ScriptDetector};

// Version constant
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Run a Ling source string through the interpreter.
/// Lexes → parses → executes the `start` binding.
pub fn run(source: &str) -> Result<(), String> {
    run_named(source, None, None)
}

/// Self-extract resources packed into the executable by `ling build --pack`.
///
/// Each `(relative_path, bytes)` is written under a per-app temp directory, then
/// the process's current directory is switched there so the app's relative asset
/// paths (e.g. `music/song.wav`) resolve against the extracted files. A no-op for
/// an empty table.
pub fn unpack_resources(app: &str, resources: &[(&str, &[u8])]) {
    if resources.is_empty() {
        return;
    }
    let base = std::env::temp_dir().join(format!("ling-pack-{app}"));
    for (rel, bytes) in resources {
        let dst = base.join(rel);
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&dst, bytes);
    }
    let _ = std::env::set_current_dir(&base);
}

/// Run with an optional source directory for relative `use` imports.
pub fn run_file(source: &str, source_dir: Option<std::path::PathBuf>) -> Result<(), String> {
    run_named(source, source_dir, None)
}

/// Run with an optional source directory and the source file name (used to label
/// diagnostics). Parse and runtime errors are returned as fully-rendered,
/// colored, localized diagnostics (see [`diag`]).
pub fn run_named(
    source: &str,
    source_dir: Option<std::path::PathBuf>,
    file: Option<&str>,
) -> Result<(), String> {
    let lang = diag::OutputLang::from_env();
    let program = parser::parse(source).map_err(|e| diag::render_parse(&e, source, file, lang))?;
    let mut interp = runtime::Interpreter::new();
    interp.source_dir = source_dir;
    match interp.run_program(&program) {
        Ok(()) => Ok(()),
        Err(msg) => {
            let trace = interp.take_error_trace();
            Err(diag::render_runtime(&msg, source, file, &trace, lang))
        },
    }
}

/// Run concatenated `source` on the Cranelift JIT backend, resolving `use`
/// imports against `source_dir`. This is the interpreter-free entry point for
/// embedders (e.g. the game launcher). Cranelift-skipped oversized functions
/// still execute through the primed fallback interpreter, so behaviour matches
/// the tree-walker. A program with no entry, a parse error, or a pre-execution
/// codegen failure falls back to [`run_named`]; a mid-run failure is returned as
/// a rendered diagnostic without retrying (output may already be on screen).
#[cfg(not(target_arch = "wasm32"))]
pub fn run_jit(
    source: &str,
    source_dir: Option<std::path::PathBuf>,
    file: Option<&str>,
) -> Result<(), String> {
    let lang = diag::OutputLang::from_env();
    let has_entry = parser::parse(source)
        .map(|p| entry::entry_name(&p.items).is_some())
        .unwrap_or(true);
    if !has_entry {
        return run_named(source, source_dir, file);
    }
    let compiler = LingCompiler::new(CompilerConfig::default());
    match compiler.compile_and_run_jit_source(source, source_dir.clone()) {
        Ok(()) => Ok(()),
        Err(core::LingError::Parse(m)) => Err(diag::render_parse(&m, source, file, lang)),
        Err(core::LingError::Mir(m)) => Err(m),
        Err(_) => run_named(source, source_dir, file),
    }
}

/// Detect the primary human language used for keywords in a Ling source file.
pub fn detect_language(source: &str) -> &'static str {
    let languages: &[(&[&str], &str)] = &[
        (
            &[
                "令", "灵符", "执", "函", "核", "若", "否则", "历", "于", "配", "归", "印", "格式",
            ],
            "Chinese (中文)",
        ),
        (
            &[
                "束縛",
                "実行",
                "もし",
                "一方",
                "ために",
                "試す",
                "待つ",
                "帰る",
            ],
            "Japanese (日本語)",
        ),
        (
            &["바인드", "만약", "동안", "출력", "시작"],
            "Korean (한국어)",
        ),
        (
            &[
                "связать",
                "сделать",
                "если",
                "иначе",
                "пока",
                "для",
                "вернуть",
                "вывести",
            ],
            "Russian (русский)",
        ),
        (
            &[
                "ผูก",
                "ทำ",
                "ถ้า",
                "มิฉะนั้น",
                "สำหรับ",
                "คืน",
                "พิมพ์",
                "รูปแบบ",
                "เริ่ม",
            ],
            "Thai (ภาษาไทย)",
        ),
        (
            &["बाँधो", "करो", "अगर", "जबकि", "वापस", "सत्य"],
            "Hindi (हिन्दी)",
        ),
        (
            &["ربط", "افعل", "إذا", "وإلا", "بينما", "أعد"],
            "Arabic (العربية)",
        ),
        (
            &["enlazar", "hacer", "mientras", "retornar", "verdadero"],
            "Spanish (Español)",
        ),
        (
            &["lier", "faire", "sinon", "tantque", "retourner", "vrai"],
            "French (Français)",
        ),
        (
            &["binden", "machen", "wenn", "solange", "zurück", "wahr"],
            "German (Deutsch)",
        ),
        (
            &["ligar", "fazer", "enquanto", "retornar", "verdadeiro"],
            "Portuguese (Português)",
        ),
    ];

    let best = languages
        .iter()
        .map(|(keywords, lang)| {
            let count = keywords.iter().filter(|&&k| source.contains(k)).count();
            (count, *lang)
        })
        .max_by_key(|&(count, _)| count);

    match best {
        Some((count, lang)) if count > 0 => lang,
        _ => "English",
    }
}
