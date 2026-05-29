// src/lib.rs - Public API entry point
pub mod core;
pub mod lexer;
pub mod parser;
pub mod semantic;
pub mod borrowck;
pub mod mir;
pub mod codegen;
pub mod lexicon;
pub mod polyglot;
pub mod gfx;
pub mod runtime;
pub mod utils;
pub mod visualize;

#[cfg(not(target_arch = "wasm32"))]
pub use ling_audio;

// Re-exports
pub use core::{LingCompiler, CompilerConfig, OptimizationLevel};
pub use lexicon::{CanonicalToken, Lexicon, LexiconRegistry};
pub use polyglot::{normalize_source, ScriptDetector};

// Version constant
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Run a Ling source string through the interpreter.
/// Lexes → parses → executes the `start` binding.
pub fn run(source: &str) -> Result<(), String> {
    let program = parser::parse(source)
        .map_err(|e| format!("parse error: {e}"))?;
    let mut interp = runtime::Interpreter::new();
    interp.run_program(&program)
        .map_err(|e| format!("runtime error: {e}"))
}

/// Detect the primary human language used for keywords in a Ling source file.
pub fn detect_language(source: &str) -> &'static str {
    let languages: &[(&[&str], &str)] = &[
        (&["令", "灵符", "执", "函", "核", "若", "否则", "历", "于", "配", "归", "印", "格式"], "Chinese (中文)"),
        (&["束縛", "実行", "もし", "一方", "ために", "試す", "待つ", "帰る"], "Japanese (日本語)"),
        (&["바인드", "만약", "동안", "출력", "시작"], "Korean (한국어)"),
        (&["связать", "сделать", "если", "иначе", "пока", "для", "вернуть", "вывести"], "Russian (русский)"),
        (&["ผูก", "ทำ", "ถ้า", "มิฉะนั้น", "สำหรับ", "คืน", "พิมพ์", "รูปแบบ", "เริ่ม"], "Thai (ภาษาไทย)"),
        (&["बाँधो", "करो", "अगर", "जबकि", "वापस", "सत्य"], "Hindi (हिन्दी)"),
        (&["ربط", "افعل", "إذا", "وإلا", "بينما", "أعد"], "Arabic (العربية)"),
        (&["enlazar", "hacer", "mientras", "retornar", "verdadero"], "Spanish (Español)"),
        (&["lier", "faire", "sinon", "tantque", "retourner", "vrai"], "French (Français)"),
        (&["binden", "machen", "wenn", "solange", "zurück", "wahr"], "German (Deutsch)"),
        (&["ligar", "fazer", "enquanto", "retornar", "verdadeiro"], "Portuguese (Português)"),
    ];

    let best = languages.iter()
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
