use alloc::string::String;
use core::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// the byte source failed (or ended early)
    Io,
    /// not a GGUF file, or one that does not hold together
    Format(&'static str),
    /// a model or tensor type this engine does not run
    Unsupported(String),
    /// a metadata key or tensor the model needs is not there
    Missing(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io => write!(f, "could not read the model"),
            Error::Format(s) => write!(f, "bad GGUF: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
            Error::Missing(s) => write!(f, "missing in the model: {s}"),
        }
    }
}
