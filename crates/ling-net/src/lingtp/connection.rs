use std::io;
use std::net::TcpStream;

use super::envelope::{LingtpRequest, LingtpResponse};
use super::error::LingtpError;
use super::framing::{read_frame, write_frame};
use super::record::RecordCipher;

/// An authenticated, encrypted lingtp:// connection, post-handshake. Each
/// direction has its own [`RecordCipher`] (own XChaCha20 key, own DICE-42
/// reactor) — client and server never share a send/recv key pair.
pub struct Connection {
    stream: TcpStream,
    send: RecordCipher,
    recv: RecordCipher,
    /// Hex-encoded ML-DSA-87 public key of the server, as verified during
    /// the handshake. Empty on the server side (lingtp v1 doesn't
    /// authenticate client identity at the transport layer).
    pub server_identity_hex: String,
}

impl Connection {
    pub(crate) fn new(
        stream: TcpStream,
        send: RecordCipher,
        recv: RecordCipher,
        server_identity_hex: String,
    ) -> Self {
        Self { stream, send, recv, server_identity_hex }
    }

    /// Client-side: send one request, block for its response. lingtp is
    /// keep-alive, so a `Connection` can be reused for many requests.
    pub fn request(&mut self, req: &LingtpRequest) -> Result<LingtpResponse, LingtpError> {
        let payload = serde_json::to_vec(req)?;
        let sealed = self.send.seal(&payload)?;
        write_frame(&mut self.stream, &sealed)?;

        let sealed_resp = read_frame(&mut self.stream)?;
        let plain = self.recv.open(&sealed_resp)?;
        Ok(serde_json::from_slice(&plain)?)
    }

    /// Server-side: block for the next request. `Ok(None)` means the peer
    /// closed the connection cleanly — the caller's keep-alive loop ends.
    pub(crate) fn recv_request(&mut self) -> Result<Option<LingtpRequest>, LingtpError> {
        match read_frame(&mut self.stream) {
            Ok(sealed) => {
                let plain = self.recv.open(&sealed)?;
                Ok(Some(serde_json::from_slice(&plain)?))
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::BrokenPipe
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    pub(crate) fn send_response(&mut self, resp: &LingtpResponse) -> Result<(), LingtpError> {
        let payload = serde_json::to_vec(resp)?;
        let sealed = self.send.seal(&payload)?;
        write_frame(&mut self.stream, &sealed)?;
        Ok(())
    }
}
