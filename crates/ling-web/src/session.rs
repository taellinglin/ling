use ling_http::Db;
use tower_sessions::cookie::time::Duration as CookieDuration;
use tower_sessions::cookie::SameSite;
use tower_sessions::{Expiry, SessionManagerLayer};

use crate::session_store::SqliteSessionStore;

pub type SessionStore = SqliteSessionStore;

/// Builds SQLite-backed session middleware: an HttpOnly, SameSite=Lax
/// cookie holding an opaque session id, 7-day sliding expiry, session data
/// stored server-side (so it's revocable — delete the row, the session is
/// dead everywhere, unlike client-side signed-cookie sessions).
///
/// `secure` sets the cookie's `Secure` flag — pass `true` for anything
/// served over HTTPS (required, or browsers keep the cookie but it's sent
/// over plaintext too) and `false` only for local plain-HTTP dev servers,
/// where a `Secure` cookie would silently never be stored.
pub async fn session_layer(
    db: &Db,
    secure: bool,
) -> anyhow::Result<SessionManagerLayer<SqliteSessionStore>> {
    let store = SqliteSessionStore::new(db.clone()).await?;

    let layer = SessionManagerLayer::new(store)
        .with_name("ling_session")
        .with_http_only(true)
        .with_secure(secure)
        .with_same_site(SameSite::Lax)
        .with_expiry(Expiry::OnInactivity(CookieDuration::days(7)));

    Ok(layer)
}
