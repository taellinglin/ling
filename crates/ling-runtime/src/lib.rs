//! Ling runtime library

pub mod gc;
pub mod alloc;
pub mod std;

pub use gc::Gc;
pub use alloc::LingBox;
