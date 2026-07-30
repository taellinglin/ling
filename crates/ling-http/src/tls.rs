use axum_server::tls_rustls::RustlsConfig;
#[cfg(feature = "dev-certs")]
use std::net::SocketAddr;
use std::path::Path;

/// rustls 0.23 requires a process-wide crypto provider to be installed
/// before any `ServerConfig` is built. We use `ring` (not `aws-lc-rs`,
/// axum-server's default) since it builds without needing cmake/NASM.
/// Safe to call more than once — a second install just fails silently.
fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Loads a TLS server config from a PEM-encoded certificate + private key
/// pair on disk. This is the path for real deployments.
pub async fn load_rustls_config(
    cert_pem: impl AsRef<Path>,
    key_pem: impl AsRef<Path>,
) -> anyhow::Result<RustlsConfig> {
    ensure_crypto_provider();
    RustlsConfig::from_pem_file(cert_pem, key_pem)
        .await
        .map_err(|e| anyhow::anyhow!("loading TLS cert/key: {e}"))
}

#[cfg(feature = "dev-certs")]
pub struct TlsMaterial {
    pub config: RustlsConfig,
    /// PEM-encoded self-signed cert, in case a caller wants to print it or
    /// write it to disk so a browser/curl can be told to trust it.
    pub cert_pem: String,
}

/// Generates a throwaway self-signed certificate covering `localhost` and
/// the given address's IP. **Local development only** — nothing trusts this
/// cert by default.
#[cfg(feature = "dev-certs")]
pub async fn generate_dev_cert(addr: SocketAddr) -> anyhow::Result<TlsMaterial> {
    ensure_crypto_provider();
    let names = vec!["localhost".to_string(), addr.ip().to_string()];
    let rcgen::CertifiedKey { cert, key_pair } = rcgen::generate_simple_self_signed(names)?;
    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();

    let config = RustlsConfig::from_pem(cert_pem.clone().into_bytes(), key_pem.into_bytes())
        .await
        .map_err(|e| anyhow::anyhow!("building dev TLS config: {e}"))?;

    Ok(TlsMaterial { config, cert_pem })
}
