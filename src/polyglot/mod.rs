// src/polyglot/mod.rs
// Polyglot support is WIP.
// Provide minimal stubs so the compiler front-end can build.

#[derive(Clone, Debug)]
pub struct ScriptDetector;

impl ScriptDetector {
    pub fn detect(_source: &str) -> Script {
        Script::Unknown
    }
}

#[derive(Clone, Debug)]
pub enum Script {
    Unknown,
}

pub fn normalize_source(source: &str) -> String {
    source.to_string()
}
