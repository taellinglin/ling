use r2d2::ManageConnection;
use rusqlite::Connection;
use std::path::PathBuf;

/// A minimal `r2d2::ManageConnection` for rusqlite. Hand-written instead of
/// depending on the `r2d2_sqlite` crate: that crate pins its own `rusqlite`
/// version, which never lines up with the `rusqlite` version `ling-ai`
/// already pulls in transitively (via the burn ML framework) — and two
/// different `rusqlite`/`libsqlite3-sys` versions can't coexist in one
/// Cargo.lock (both declare `links = "sqlite3"`). `r2d2` itself has no
/// sqlite dependency, so implementing this ourselves keeps us on exactly
/// one rusqlite version, chosen by us, everywhere in the workspace.
pub enum SqliteConnectionManager {
    File(PathBuf),
    SharedMemory { uri: String },
}

impl SqliteConnectionManager {
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self::File(path.into())
    }

    /// A named, shared-cache in-memory database (`file:<name>?mode=memory&cache=shared`).
    pub fn shared_memory(uri: impl Into<String>) -> Self {
        Self::SharedMemory { uri: uri.into() }
    }
}

impl ManageConnection for SqliteConnectionManager {
    type Connection = Connection;
    type Error = rusqlite::Error;

    fn connect(&self) -> Result<Connection, rusqlite::Error> {
        let conn = match self {
            Self::File(path) => Connection::open(path)?,
            Self::SharedMemory { uri } => Connection::open_with_flags(
                uri,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                    | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
                    | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            )?,
        };
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Ok(conn)
    }

    fn is_valid(&self, conn: &mut Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch("SELECT 1;")
    }

    fn has_broken(&self, _conn: &mut Connection) -> bool {
        false
    }
}
