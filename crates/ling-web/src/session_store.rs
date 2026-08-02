use std::collections::HashMap;

use ling_http::rusqlite;
use ling_http::Db;
use tower_sessions::cookie::time::OffsetDateTime;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{Error as StoreError, Result as StoreResult};
use tower_sessions::SessionStore as SessionStoreTrait;

const MIGRATIONS: &[(&str, &str)] = &[(
    "0001_sessions",
    "CREATE TABLE IF NOT EXISTS sessions (\
       id TEXT PRIMARY KEY, \
       data TEXT NOT NULL, \
       expiry_date INTEGER NOT NULL\
     )",
)];

/// A `tower_sessions::SessionStore` backed by [`ling_http::Db`] (rusqlite).
///
/// Not `tower-sessions-sqlx-store`: that crate pins its own `sqlx`, and two
/// different `rusqlite`/`sqlx`-sqlite driver versions can't coexist in one
/// Cargo.lock (both link the native `sqlite3` library) — see the same
/// reasoning in `ling-http/src/pool.rs`. This is a small enough trait (four
/// methods) that hand-writing it against the `Db` we already have is less
/// work than reconciling driver versions.
#[derive(Clone)]
pub struct SqliteSessionStore {
    db: Db,
}

impl SqliteSessionStore {
    pub async fn new(db: Db) -> anyhow::Result<Self> {
        db.run_migrations(MIGRATIONS).await?;
        Ok(Self { db })
    }
}

impl std::fmt::Debug for SqliteSessionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteSessionStore").finish_non_exhaustive()
    }
}

fn to_store_err(e: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(e.to_string())
}

#[async_trait::async_trait]
impl SessionStoreTrait for SqliteSessionStore {
    async fn create(&self, record: &mut Record) -> StoreResult<()> {
        // Retry on the (astronomically unlikely) id collision, matching the
        // trait's documented contract for `create`.
        loop {
            let id = record.id;
            let data = serde_json::to_string(&record.data).map_err(to_store_err)?;
            let expiry = record.expiry_date.unix_timestamp();

            let exists = self
                .db
                .with_conn(move |conn| {
                    conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
                        rusqlite::params![id.to_string()],
                        |row| row.get::<_, bool>(0),
                    )
                })
                .await
                .map_err(to_store_err)?;

            if exists {
                record.id = Id::default();
                continue;
            }

            self.db
                .with_conn(move |conn| {
                    conn.execute(
                        "INSERT INTO sessions (id, data, expiry_date) VALUES (?1, ?2, ?3)",
                        rusqlite::params![id.to_string(), data, expiry],
                    )
                })
                .await
                .map_err(to_store_err)?;
            return Ok(());
        }
    }

    async fn save(&self, record: &Record) -> StoreResult<()> {
        let id = record.id;
        let data = serde_json::to_string(&record.data).map_err(to_store_err)?;
        let expiry = record.expiry_date.unix_timestamp();
        self.db
            .with_conn(move |conn| {
                conn.execute(
                    "INSERT INTO sessions (id, data, expiry_date) VALUES (?1, ?2, ?3) \
                     ON CONFLICT(id) DO UPDATE SET data = excluded.data, expiry_date = excluded.expiry_date",
                    rusqlite::params![id.to_string(), data, expiry],
                )
            })
            .await
            .map_err(to_store_err)?;
        Ok(())
    }

    async fn load(&self, session_id: &Id) -> StoreResult<Option<Record>> {
        let id = *session_id;
        let row: Option<(String, i64)> = self
            .db
            .with_conn(move |conn| {
                conn.query_row(
                    "SELECT data, expiry_date FROM sessions WHERE id = ?1",
                    rusqlite::params![id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    e => Err(e),
                })
            })
            .await
            .map_err(to_store_err)?;

        let Some((data, expiry)) = row else {
            return Ok(None);
        };

        let expiry_date =
            OffsetDateTime::from_unix_timestamp(expiry).map_err(to_store_err)?;
        if expiry_date < OffsetDateTime::now_utc() {
            let _ = self.delete(session_id).await;
            return Ok(None);
        }

        let data: HashMap<String, serde_json::Value> =
            serde_json::from_str(&data).map_err(to_store_err)?;
        Ok(Some(Record { id, data, expiry_date }))
    }

    async fn delete(&self, session_id: &Id) -> StoreResult<()> {
        let id = *session_id;
        self.db
            .with_conn(move |conn| {
                conn.execute("DELETE FROM sessions WHERE id = ?1", rusqlite::params![id.to_string()])
            })
            .await
            .map_err(to_store_err)?;
        Ok(())
    }
}
