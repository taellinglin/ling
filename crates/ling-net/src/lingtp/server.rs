use std::io;
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::thread;

use ling_crypto::MlDsa87Keypair;

use super::connection::Connection;
use super::envelope::{LingtpRequest, LingtpResponse};
use super::handshake::server_handshake;
use super::util::to_hex;

pub type Handler = dyn Fn(&LingtpRequest) -> LingtpResponse + Send + Sync;

/// Load a persisted ML-DSA-87 identity keypair from `path` (a 32-byte seed
/// file), or generate a fresh one and persist it there if it doesn't exist
/// yet. Conventionally `.lingtp/host_mldsa87`.
pub fn load_or_generate_identity(path: impl AsRef<Path>) -> io::Result<MlDsa87Keypair> {
    let path = path.as_ref();
    if let Ok(bytes) = std::fs::read(path) {
        if bytes.len() == 32 {
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&bytes);
            return Ok(MlDsa87Keypair::from_seed(seed));
        }
    }
    let kp = MlDsa87Keypair::generate();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &*kp.to_bytes())?;
    Ok(kp)
}

/// Serve lingtp:// on `host:port`, calling `handler` for every request on
/// every connection (thread-per-connection; lingtp is keep-alive, so many
/// requests may share one handshake). Blocks the calling thread forever.
pub fn serve(
    host: &str,
    port: u16,
    identity: MlDsa87Keypair,
    handler: impl Fn(&LingtpRequest) -> LingtpResponse + Send + Sync + 'static,
) -> io::Result<()> {
    let listener = TcpListener::bind((host, port))?;
    let identity_hex = to_hex(&identity.public_key());
    eprintln!(
        "lingtp://{host}:{port}/ listening — identity {}...",
        &identity_hex[..16.min(identity_hex.len())]
    );
    serve_on(listener, identity, handler)
}

/// Same as [`serve`], but on an already-bound listener — useful for binding
/// to an OS-assigned ephemeral port (`"127.0.0.1:0"`) in tests, or for
/// socket-activation setups where the listener is inherited from elsewhere.
pub fn serve_on(
    listener: TcpListener,
    identity: MlDsa87Keypair,
    handler: impl Fn(&LingtpRequest) -> LingtpResponse + Send + Sync + 'static,
) -> io::Result<()> {
    let identity = Arc::new(identity);
    let handler: Arc<Handler> = Arc::new(handler);

    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(s) => s,
            Err(_) => continue,
        };
        let identity = Arc::clone(&identity);
        let handler = Arc::clone(&handler);
        thread::spawn(move || {
            if let Ok(conn) = server_handshake(stream, &identity) {
                handle_connection(conn, handler);
            }
        });
    }
    Ok(())
}

fn handle_connection(mut conn: Connection, handler: Arc<Handler>) {
    loop {
        match conn.recv_request() {
            Ok(Some(req)) => {
                let resp = handler(&req);
                if conn.send_response(&resp).is_err() {
                    break;
                }
            }
            _ => break,
        }
    }
}
