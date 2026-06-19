//! ling-net — async networking for Ling.
//!
//! Feature flags (all off by default except `http`):
//! - `http`      — axum server + reqwest client
//! - `websocket` — tokio-tungstenite
//! - `quic`      — quinn (QUIC/HTTP-3)
//! - `grpc`      — tonic + prost

pub mod error;
pub mod types;

#[cfg(feature = "http")]
pub mod http;

pub use error::NetError;
pub use types::{HttpMethod, Request, Response};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
