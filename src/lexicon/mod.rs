// src/lexicon/mod.rs
pub fn canonical_token(token: &str) -> String {
    token.to_string()
}

// Lexicon system is WIP; keep stubs for public API.
#[derive(Clone, Debug)]
pub struct Lexicon;

#[derive(Clone, Debug, Default)]
pub struct LexiconRegistry;

impl LexiconRegistry {
    pub fn global() -> &'static Self {
        static REG: once_cell_stub::OnceCell<LexiconRegistry> = once_cell_stub::OnceCell::new();
        REG.get_or_init(|| LexiconRegistry)
    }
    pub fn translate(&self, text: &str, _from: &str, _to: &str) -> String {
        text.to_string()
    }
}

// Minimal once_cell substitute to avoid adding deps.
mod once_cell_stub {
    use std::sync::OnceLock;
    pub struct OnceCell<T>(OnceLock<T>);
    impl<T> OnceCell<T> {
        pub const fn new() -> Self {
            Self(OnceLock::new())
        }
        pub fn get_or_init(&'static self, f: impl FnOnce() -> T) -> &T {
            self.0.get_or_init(f)
        }
    }
}

#[derive(Clone, Debug)]
pub struct CanonicalToken(pub String);

