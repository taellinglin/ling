use rand::RngCore;
use subtle::ConstantTimeEq;
use tower_sessions::Session;

const CSRF_KEY: &str = "_csrf";

/// Returns this session's CSRF token, generating and storing one on first
/// use. Put it in a hidden form field (or a header for JSON APIs) and
/// check it with [`verify_csrf`] before acting on the request.
pub async fn csrf_token(session: &Session) -> anyhow::Result<String> {
    if let Some(token) = session.get::<String>(CSRF_KEY).await? {
        return Ok(token);
    }
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token = hex_encode(&bytes);
    session.insert(CSRF_KEY, token.clone()).await?;
    Ok(token)
}

/// Verifies a submitted CSRF token against the one stored in the session,
/// in constant time. Returns `false` (not an error) for a missing session
/// token or a mismatch — both just mean "reject the request."
pub async fn verify_csrf(session: &Session, submitted: &str) -> anyhow::Result<bool> {
    let Some(expected) = session.get::<String>(CSRF_KEY).await? else {
        return Ok(false);
    };
    if expected.len() != submitted.len() {
        return Ok(false);
    }
    Ok(bool::from(expected.as_bytes().ct_eq(submitted.as_bytes())))
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
