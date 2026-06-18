//! ling-polyglot — multi-language keyword and builtin lexicon system.
//!
//! Supports 16+ human languages. Each language maps Ling keywords,
//! types, and builtin names to their native-script equivalents.

pub mod detect;
pub mod lexicon;
pub mod translate;

pub use detect::detect_language;
pub use lexicon::{LanguageCode, Lexicon, LexiconEntry};
pub use translate::translate_keyword;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
