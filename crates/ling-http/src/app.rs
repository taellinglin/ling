use axum::Router;
use std::net::SocketAddr;
use std::time::Duration;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Prints a startup banner to stdout — plain `println!`, not `tracing`, so
/// it's visible even in a binary that never wired up a tracing subscriber
/// (the common case for a small `.ling`/`lingfu` app). Raw ANSI codes rather
/// than a colour crate dependency; modern Windows Terminal/PowerShell and
/// every Unix terminal render these natively.
fn print_listening_banner(scheme: &str, addr: SocketAddr) {
    let url = format!("{scheme}://{addr}");
    println!();
    println!("  \x1b[1;36m⚡ ling-http\x1b[0m");
    println!("  \x1b[32m➜\x1b[0m  Running on \x1b[1;32m{url}\x1b[0m");
    println!("  \x1b[32m➜\x1b[0m  Press \x1b[1mCtrl+C\x1b[0m to quit");
    println!();
}

#[cfg(feature = "dev-certs")]
use crate::tls::TlsMaterial;

/// A ling-http application: an axum `Router` pre-wired with the middleware
/// every service on this framework wants (tracing, timeouts, a body-size
/// cap, CORS), plus HTTPS serving helpers.
///
/// `S` is your application state (typically a struct holding a [`crate::db::Db`]
/// and any config), shared across handlers via axum's `State` extractor.
pub struct App<S> {
    router: Router<S>,
}

impl<S> App<S>
where
    S: Clone + Send + Sync + 'static,
{
    /// Starts from an empty router with sane default middleware:
    /// request tracing, a 60s timeout, and a 25 MiB request body cap
    /// (override with `.max_body_bytes` if a route needs to accept larger
    /// uploads, e.g. crate tarballs).
    pub fn new() -> Self {
        let router = Router::new()
            .layer(TraceLayer::new_for_http())
            .layer(TimeoutLayer::with_status_code(
                axum::http::StatusCode::REQUEST_TIMEOUT,
                Duration::from_secs(60),
            ))
            .layer(RequestBodyLimitLayer::new(25 * 1024 * 1024))
            .layer(CorsLayer::permissive());
        Self { router }
    }

    /// Raises the request body size cap. Call this before merging in routes
    /// that need to accept large uploads.
    pub fn max_body_bytes(mut self, bytes: usize) -> Self {
        self.router = self.router.layer(RequestBodyLimitLayer::new(bytes));
        self
    }

    /// Merges routes (from `.route(...)` calls or [`crate::mvc::resource`])
    /// into the app.
    pub fn merge(mut self, routes: Router<S>) -> Self {
        self.router = self.router.merge(routes);
        self
    }

    /// Attaches the shared application state, finishing the router.
    pub fn with_state(self, state: S) -> Router {
        self.router.with_state(state)
    }
}

impl<S> Default for App<S>
where
    S: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

/// Serves `router` over plain HTTP. Use this only behind a reverse proxy
/// that terminates TLS (matches how the other linglin.art sites are
/// deployed); for a service that must terminate TLS itself, use
/// [`serve_tls`] or [`serve_dev_tls`] instead.
pub async fn serve_http(router: Router, addr: SocketAddr) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr().unwrap_or(addr);
    tracing::info!(%bound_addr, "ling-http listening (plain HTTP)");
    print_listening_banner("http", bound_addr);
    axum::serve(listener, router.into_make_service()).await?;
    Ok(())
}

/// Serves `router` over HTTPS using a certificate/key pair loaded from disk
/// (PEM format). This is real TLS termination — use it when there's no
/// reverse proxy in front of the service.
pub async fn serve_tls(
    router: Router,
    addr: SocketAddr,
    cert_pem: impl AsRef<std::path::Path>,
    key_pem: impl AsRef<std::path::Path>,
) -> anyhow::Result<()> {
    let config = crate::tls::load_rustls_config(cert_pem, key_pem).await?;
    tracing::info!(%addr, "ling-http listening (HTTPS)");
    print_listening_banner("https", addr);
    axum_server::bind_rustls(addr, config)
        .serve(router.into_make_service())
        .await?;
    Ok(())
}

/// Serves `router` over HTTPS using a throwaway self-signed certificate
/// generated at startup. **Local development only** — browsers and `cargo`/
/// `curl` will reject the cert as untrusted unless told to ignore it
/// (`curl -k`). Requires the `dev-certs` feature.
#[cfg(feature = "dev-certs")]
pub async fn serve_dev_tls(router: Router, addr: SocketAddr) -> anyhow::Result<()> {
    let TlsMaterial { config, .. } = crate::tls::generate_dev_cert(addr).await?;
    tracing::warn!(%addr, "ling-http listening (HTTPS, SELF-SIGNED DEV CERT — do not use in production)");
    print_listening_banner("https", addr);
    println!("  \x1b[33m⚠\x1b[0m  self-signed dev cert — browsers/curl will reject it as untrusted\n");
    axum_server::bind_rustls(addr, config)
        .serve(router.into_make_service())
        .await?;
    Ok(())
}
