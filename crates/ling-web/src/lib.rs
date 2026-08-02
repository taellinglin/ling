//! ling-web — the Flask-flavored layer on top of ling-http: SQLite-backed
//! sessions, flash messages, minijinja templates, static file serving,
//! blueprints, and CSRF protection.

pub mod blueprint;
pub mod csrf;
pub mod flash;
pub mod session;
pub mod session_store;
pub mod static_files;
pub mod templates;

pub use blueprint::blueprint;
pub use session::session_layer;
pub use static_files::serve_static;
pub use templates::Templates;

pub use ling_http;
pub use tower_sessions::Session;
