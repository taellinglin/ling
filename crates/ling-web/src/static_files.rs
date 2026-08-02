use axum::Router;
use std::path::Path;
use tower_http::services::ServeDir;

/// Mounts a directory of static files at `prefix` (e.g. `serve_static("/static", "./static")`).
pub fn serve_static<S>(prefix: &str, dir: impl AsRef<Path>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new().nest_service(prefix, ServeDir::new(dir.as_ref()))
}
