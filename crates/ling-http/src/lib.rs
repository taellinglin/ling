//! ling-http — an HTTPS/JSON/SQLite MVC framework for the Ling ecosystem.
//!
//! Roughly: axum + rustls + rusqlite, wrapped in a thin, opinionated layer
//! so Ling services (starting with the linglibs crate registry) get a
//! consistent JSON error shape, CRUD-resource routing, and HTTPS serving
//! without each service reinventing that plumbing.

pub mod app;
pub mod db;
pub mod error;
pub mod mvc;
pub mod pool;
pub mod tls;

pub use app::{serve_http, serve_tls, App};
pub use db::Db;
pub use error::{HttpError, Result};
pub use mvc::{resource, Resource};

#[cfg(feature = "dev-certs")]
pub use app::serve_dev_tls;

// Re-export the crates callers need in handler signatures so downstream
// Cargo.toml files don't have to pin matching versions themselves.
pub use axum;
pub use rusqlite;
pub use serde_json;
pub use tokio;
