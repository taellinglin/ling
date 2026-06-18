//! Error types for ling-mic.

use std::fmt;

#[derive(Debug)]
pub enum MicError {
    NoDevice,
    StreamError(String),
    Unsupported(String),
}

impl fmt::Display for MicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDevice => write!(f, "no microphone device found"),
            Self::StreamError(msg) => write!(f, "mic stream error: {msg}"),
            Self::Unsupported(msg) => write!(f, "unsupported: {msg}"),
        }
    }
}

impl std::error::Error for MicError {}
