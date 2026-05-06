//! Core types and errors for the Ling compiler

pub mod error;
pub mod types;
pub mod arena;

pub use error::LingError;
pub use types::{Type, TypeId};
pub use arena::Arena;
