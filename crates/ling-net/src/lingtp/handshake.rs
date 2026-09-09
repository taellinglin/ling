//! The PQ handshake: `ClientHello` → `ServerHello` → `ClientKeyExchange` →
//! `ServerFinished`. See the [`super`] module docs for the message shapes.

use std::net::TcpStream;

use ling_crypto::hybrid::{self, HybridKeypair};
use ling_crypto::MlDsa87Keypair;

use super::connection::Connection;
use super::error::LingtpError;
use super::framing::{read_frame, take_fixed, take_u16_prefixed, write_frame, write_u16_prefixed};
use super::known_hosts::{check_known_host, learn_known_host};
use super::record::RecordCipher;
use super::session::derive_session_keys;
use super::util::{ct_eq32, random_32, to_hex};

pub const PROTOCOL_VERSION: u8 = 1;

pub fn client_handshake(mut stream: TcpStream, host_port: &str) -> Result<Connection, LingtpError> {
    let client_random = random_32();

    let mut hello = Vec::with_capacity(33);
    hello.push(PROTOCOL_VERSION);
    hello.extend_from_slice(&client_random);
    write_frame(&mut stream, &hello)?;

    let server_hello = read_frame(&mut stream)?;
    let mut pos = 0usize;
    let version = take_fixed(&server_hello, &mut pos, 1)?[0];
    if version != PROTOCOL_VERSION {
        return Err(LingtpError::Protocol(format!("unsupported protocol version {version}")));
    }
    let server_random: [u8; 32] = take_fixed(&server_hello, &mut pos, 32)?
        .try_into()
        .expect("take_fixed(32) returns 32 bytes");
    let hybrid_pubkey = take_u16_prefixed(&server_hello, &mut pos)?;
    let mldsa_pubkey = take_u16_prefixed(&server_hello, &mut pos)?;
    let signature = take_u16_prefixed(&server_hello, &mut pos)?;

    // Transcript covers the version byte too, so a MITM can't downgrade the
    // connection to strip a future cascade layer without invalidating the
    // ML-DSA-87 signature.
    let mut transcript = Vec::new();
    transcript.push(version);
    transcript.extend_from_slice(&client_random);
    transcript.extend_from_slice(&server_random);
    transcript.extend_from_slice(&hybrid_pubkey);
    transcript.extend_from_slice(&mldsa_pubkey);
    MlDsa87Keypair::verify(&mldsa_pubkey, &transcript, &signature)
        .map_err(|_| LingtpError::Protocol("ServerHello signature invalid".into()))?;

    let pubkey_hex = to_hex(&mldsa_pubkey);
    if check_known_host(host_port, &pubkey_hex).is_err() {
        return Err(LingtpError::HostKeyMismatch(host_port.to_string()));
    }
    learn_known_host(host_port, &pubkey_hex);

    let (kem_ct, shared_secret) =
        hybrid::encapsulate(&hybrid_pubkey).map_err(LingtpError::Crypto)?;
    let keys = derive_session_keys(&shared_secret, &client_random, &server_random);

    let mut cke = Vec::new();
    write_u16_prefixed(&mut cke, &kem_ct);
    cke.extend_from_slice(&keys.client_finished_key);
    write_frame(&mut stream, &cke)?;

    let sf = read_frame(&mut stream)?;
    let mut pos2 = 0usize;
    let server_finished: [u8; 32] = take_fixed(&sf, &mut pos2, 32)?
        .try_into()
        .expect("take_fixed(32) returns 32 bytes");
    if !ct_eq32(&server_finished, &keys.server_finished_key) {
        return Err(LingtpError::Protocol("ServerFinished mismatch".into()));
    }

    Ok(Connection::new(
        stream,
        RecordCipher::new(keys.c2s_key, keys.c2s_reactor_seed),
        RecordCipher::new(keys.s2c_key, keys.s2c_reactor_seed),
        pubkey_hex,
    ))
}

pub fn server_handshake(
    mut stream: TcpStream,
    identity: &MlDsa87Keypair,
) -> Result<Connection, LingtpError> {
    let ch = read_frame(&mut stream)?;
    let mut pos = 0usize;
    let version = take_fixed(&ch, &mut pos, 1)?[0];
    if version != PROTOCOL_VERSION {
        return Err(LingtpError::Protocol(format!("unsupported protocol version {version}")));
    }
    let client_random: [u8; 32] = take_fixed(&ch, &mut pos, 32)?
        .try_into()
        .expect("take_fixed(32) returns 32 bytes");

    let server_random = random_32();
    let ephemeral = HybridKeypair::generate();
    let hybrid_pubkey = ephemeral.public_key();
    let mldsa_pubkey = identity.public_key();

    let mut transcript = Vec::new();
    transcript.push(PROTOCOL_VERSION);
    transcript.extend_from_slice(&client_random);
    transcript.extend_from_slice(&server_random);
    transcript.extend_from_slice(&hybrid_pubkey);
    transcript.extend_from_slice(&mldsa_pubkey);
    let signature = identity.sign(&transcript);

    let mut hello = Vec::new();
    hello.push(PROTOCOL_VERSION);
    hello.extend_from_slice(&server_random);
    write_u16_prefixed(&mut hello, &hybrid_pubkey);
    write_u16_prefixed(&mut hello, &mldsa_pubkey);
    write_u16_prefixed(&mut hello, &signature);
    write_frame(&mut stream, &hello)?;

    let cke = read_frame(&mut stream)?;
    let mut pos2 = 0usize;
    let kem_ct = take_u16_prefixed(&cke, &mut pos2)?;
    let client_finished: [u8; 32] = take_fixed(&cke, &mut pos2, 32)?
        .try_into()
        .expect("take_fixed(32) returns 32 bytes");

    let shared_secret = ephemeral.decapsulate(&kem_ct).map_err(LingtpError::Crypto)?;
    let keys = derive_session_keys(&shared_secret, &client_random, &server_random);

    // Mandatory: ML-KEM's implicit-rejection failure mode means a bad/
    // tampered ciphertext silently yields a *different* shared secret
    // rather than an error, so this is what actually catches it.
    if !ct_eq32(&client_finished, &keys.client_finished_key) {
        return Err(LingtpError::Protocol("ClientKeyExchange finished mismatch".into()));
    }

    let mut sf = Vec::with_capacity(32);
    sf.extend_from_slice(&keys.server_finished_key);
    write_frame(&mut stream, &sf)?;

    Ok(Connection::new(
        stream,
        RecordCipher::new(keys.s2c_key, keys.s2c_reactor_seed),
        RecordCipher::new(keys.c2s_key, keys.c2s_reactor_seed),
        String::new(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn full_handshake_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let identity = MlDsa87Keypair::generate();
        let identity_pubkey = identity.public_key();

        let server_thread = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            server_handshake(stream, &identity).expect("server handshake should succeed")
        });

        let stream = TcpStream::connect(addr).unwrap();
        let host_port = format!("lingtp-handshake-test-{}:{}", std::process::id(), addr.port());
        let client_conn =
            client_handshake(stream, &host_port).expect("client handshake should succeed");
        assert_eq!(client_conn.server_identity_hex, to_hex(&identity_pubkey));

        server_thread.join().unwrap();
    }

    #[test]
    fn forged_server_signature_is_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _client_hello = read_frame(&mut stream).unwrap();

            // Craft a ServerHello whose signature doesn't match its own
            // transcript at all — a forger with an unrelated keypair.
            let server_random = random_32();
            let ephemeral = HybridKeypair::generate();
            let hybrid_pubkey = ephemeral.public_key();
            let forger = MlDsa87Keypair::generate();
            let mldsa_pubkey = forger.public_key();
            let bogus_signature = forger.sign(b"not the real transcript at all");

            let mut hello = Vec::new();
            hello.push(PROTOCOL_VERSION);
            hello.extend_from_slice(&server_random);
            write_u16_prefixed(&mut hello, &hybrid_pubkey);
            write_u16_prefixed(&mut hello, &mldsa_pubkey);
            write_u16_prefixed(&mut hello, &bogus_signature);
            let _ = write_frame(&mut stream, &hello);
        });

        let stream = TcpStream::connect(addr).unwrap();
        let host_port = format!("lingtp-forged-sig-test-{}:{}", std::process::id(), addr.port());
        let result = client_handshake(stream, &host_port);
        assert!(result.is_err(), "a forged ServerHello signature must be rejected");
    }

    #[test]
    fn tampered_kem_ciphertext_is_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let identity = MlDsa87Keypair::generate();

        let (result_tx, result_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let result = server_handshake(stream, &identity);
            let _ = result_tx.send(result.is_err());
        });

        // Act as a client that completes ClientHello/ServerHello normally
        // but sends a corrupted KEM ciphertext (and correspondingly bogus
        // client_finished) in ClientKeyExchange.
        let mut stream = TcpStream::connect(addr).unwrap();
        let client_random = random_32();
        let mut hello = Vec::new();
        hello.push(PROTOCOL_VERSION);
        hello.extend_from_slice(&client_random);
        write_frame(&mut stream, &hello).unwrap();

        let server_hello = read_frame(&mut stream).unwrap();
        let mut pos = 0usize;
        let _version = take_fixed(&server_hello, &mut pos, 1).unwrap();
        let _server_random = take_fixed(&server_hello, &mut pos, 32).unwrap();
        let hybrid_pubkey = take_u16_prefixed(&server_hello, &mut pos).unwrap();
        let _mldsa_pubkey = take_u16_prefixed(&server_hello, &mut pos).unwrap();
        let _sig = take_u16_prefixed(&server_hello, &mut pos).unwrap();

        let (mut kem_ct, _real_secret) = hybrid::encapsulate(&hybrid_pubkey).unwrap();
        kem_ct[0] ^= 0xFF; // corrupt the ciphertext

        let mut cke = Vec::new();
        write_u16_prefixed(&mut cke, &kem_ct);
        cke.extend_from_slice(&[0u8; 32]); // bogus client_finished
        write_frame(&mut stream, &cke).unwrap();

        let server_rejected = result_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(server_rejected, "a tampered KEM ciphertext must be rejected by the server");
    }
}
