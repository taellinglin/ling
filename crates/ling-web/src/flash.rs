use serde::{Deserialize, Serialize};
use tower_sessions::Session;

const FLASH_KEY: &str = "_flashes";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flash {
    pub category: String,
    pub message: String,
}

/// Queues a one-time message to show on the next page render — Flask's
/// `flash()`. `category` is a free-form tag ("error", "success", ...) for
/// styling in the template.
pub async fn flash(
    session: &Session,
    category: impl Into<String>,
    message: impl Into<String>,
) -> anyhow::Result<()> {
    let mut flashes: Vec<Flash> = session.get(FLASH_KEY).await?.unwrap_or_default();
    flashes.push(Flash { category: category.into(), message: message.into() });
    session.insert(FLASH_KEY, flashes).await?;
    Ok(())
}

/// Pops and returns all queued flash messages — Flask's
/// `get_flashed_messages()`. Call this once per page render; the messages
/// are gone from the session afterwards.
pub async fn take_flashes(session: &Session) -> anyhow::Result<Vec<Flash>> {
    let flashes: Vec<Flash> = session.get(FLASH_KEY).await?.unwrap_or_default();
    session.remove::<Vec<Flash>>(FLASH_KEY).await?;
    Ok(flashes)
}
