use std::net::TcpStream;

use super::connection::Connection;
use super::error::LingtpError;
use super::handshake::client_handshake;

/// Connect to a lingtp:// server, perform the full PQ handshake, and return
/// a ready-to-use [`Connection`]. Verifies the server's ML-DSA-87 identity
/// against `.lingtp/known_hosts` (trust-on-first-use) — refuses to connect
/// if a previously-seen host presents a different key.
pub fn connect(host: &str, port: u16) -> Result<Connection, LingtpError> {
    let stream = TcpStream::connect((host, port))?;
    let host_port = format!("{host}:{port}");
    client_handshake(stream, &host_port)
}
