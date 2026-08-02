use axum::Router;

/// Groups routes under a URL prefix — Flask's `Blueprint`, minus the
/// registration ceremony: it's a named wrapper over `Router::nest`.
pub fn blueprint<S>(prefix: &str, routes: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new().nest(prefix, routes)
}
