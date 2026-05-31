//! ling-polyglot — multi-language keyword and builtin lexicon system.
//!
//! Supports 16+ human languages. Each language maps Ling keywords,
//! types, and builtin names to their native-script equivalents.

pub mod lexicon;
pub mod detect;
pub mod translate;

pub use lexicon::{Lexicon, LexiconEntry, LanguageCode};
pub use detect::detect_language;
pub use translate::translate_keyword;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
