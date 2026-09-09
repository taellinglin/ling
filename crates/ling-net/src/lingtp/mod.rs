//! lingtp:// — a raw-TCP, post-quantum secure transport.
//!
//! Handshake: `ClientHello` → `ServerHello` (X25519+ML-KEM-768 hybrid KEM
//! ephemeral public key, ML-DSA-87 identity public key, an ML-DSA-87
//! signature over the transcript) → `ClientKeyExchange` (KEM ciphertext +
//! a finished proof) → `ServerFinished`. Session keys and the DICE-42
//! (Möbius-Helix Reactor, see [`ling_crypto::mobius_helix`]) seeds are all
//! derived via HKDF-SHA3 from the hybrid shared secret, one independent key
//! per purpose. Every record is sealed with XChaCha20-Poly1305 first, then
//! DICE-42 — an independently-keyed cascade layer, defense-in-depth on top
//! of the vetted AEAD, never a replacement for it. Server identity is
//! checked against a trust-on-first-use `.lingtp/known_hosts` store, the
//! same model this workspace's SSH client uses.
//!
//! ```no_run
//! # fn handler(_req: &ling_net::lingtp::LingtpRequest) -> ling_net::lingtp::LingtpResponse {
//! #     ling_net::lingtp::LingtpResponse::text(200, "ok")
//! # }
//! // Server:
//! let identity = ling_net::lingtp::server::load_or_generate_identity(".lingtp/host_mldsa87")?;
//! ling_net::lingtp::server::serve("0.0.0.0", 7780, identity, handler)?;
//!
//! // Client:
//! let mut conn = ling_net::lingtp::client::connect("127.0.0.1", 7780)?;
//! let resp = conn.request(&ling_net::lingtp::LingtpRequest::get("/"))?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod connection;
mod error;
mod framing;
mod handshake;
mod record;
mod session;
mod util;

pub mod client;
pub mod envelope;
pub mod known_hosts;
pub mod server;

pub use connection::Connection;
pub use envelope::{LingtpRequest, LingtpResponse};
pub use error::LingtpError;
pub use handshake::PROTOCOL_VERSION;
