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
pub mod runtime;
pub mod utils;

// Re-exports
pub use core::{LingCompiler, CompilerConfig, OptimizationLevel};
pub use lexicon::{CanonicalToken, Lexicon, LexiconRegistry};
pub use polyglot::{normalize_source, ScriptDetector};


// Version constant
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
