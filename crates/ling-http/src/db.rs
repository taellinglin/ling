use r2d2::Pool;
use std::path::Path;

use crate::error::{HttpError, Result};
use crate::pool::SqliteConnectionManager;

pub type SqlitePool = Pool<SqliteConnectionManager>;

/// A pooled SQLite connection. Cheap to clone (shares the pool handle).
/// Queries run on rusqlite (blocking) under `spawn_blocking` via
/// [`Db::with_conn`] — a deliberate choice, not a stopgap: builtins called
/// from the (synchronous) Ling interpreter dispatch this way too, so one
/// blocking DB layer serves both the async HTTP side and the interpreter
/// bridge without two separate drivers.
#[derive(Clone)]
pub struct Db(pub SqlitePool);

impl Db {
    /// Opens (creating if needed) a SQLite database file and returns a pool.
    pub async fn connect(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let pool = tokio::task::spawn_blocking(move || -> anyhow::Result<SqlitePool> {
            let manager = SqliteConnectionManager::file(path);
            let pool = Pool::builder().max_size(8).build(manager)?;
            pool.get()?.execute_batch("PRAGMA journal_mode = WAL;")?;
            Ok(pool)
        })
        .await??;
        Ok(Self(pool))
    }

    /// A private, shared-cache in-memory database — useful for tests and
    /// examples. All connections in the pool see the same data.
    pub async fn connect_memory() -> anyhow::Result<Self> {
        let manager =
            SqliteConnectionManager::shared_memory("file:ling_http_mem?mode=memory&cache=shared");
        let pool = tokio::task::spawn_blocking(move || -> anyhow::Result<SqlitePool> {
            // A single connection for the shared in-memory DB: SQLite drops
            // shared-cache memory data once its last connection closes, so
            // keeping exactly one alive in the pool for the process
            // lifetime is what makes data survive across pool checkouts.
            Ok(Pool::builder().max_size(1).min_idle(Some(1)).build(manager)?)
        })
        .await??;
        Ok(Self(pool))
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.0
    }

    /// Runs `f` against a pooled connection on a blocking thread, mapping
    /// pool/query errors into [`HttpError`] (so handlers can just `?` it).
    pub async fn with_conn<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&rusqlite::Connection) -> rusqlite::Result<T> + Send + 'static,
    {
        let pool = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(HttpError::Pool)?;
            f(&conn).map_err(HttpError::Database)
        })
        .await
        .map_err(|e| HttpError::Internal(anyhow::anyhow!("db task panicked: {e}")))?
    }

    /// Applies `(name, sql)` migrations in order, skipping ones already
    /// recorded as applied in a `_migrations` bookkeeping table.
    pub async fn run_migrations(&self, migrations: &'static [(&'static str, &'static str)]) -> anyhow::Result<()> {
        let pool = self.0.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let mut conn = pool.get()?;
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS _migrations (\
                   name TEXT PRIMARY KEY, \
                   applied_at TEXT NOT NULL DEFAULT (datetime('now'))\
                 )",
            )?;
            let tx = conn.transaction()?;
            for (name, sql) in migrations {
                let already: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = ?1)",
                    [name],
                    |row| row.get(0),
                )?;
                if already {
                    continue;
                }
                tx.execute_batch(sql)?;
                tx.execute("INSERT INTO _migrations (name) VALUES (?1)", [name])?;
            }
            tx.commit()?;
            Ok(())
        })
        .await?
    }
}
