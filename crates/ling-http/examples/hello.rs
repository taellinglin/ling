//! Minimal end-to-end proof that ling-http's networking works: a JSON API,
//! backed by SQLite, serving a health check and a CRUD resource — over
//! plain HTTP by default, or HTTPS with `--tls`.
//!
//! Run it:
//!   cargo run -p ling-http --example hello --features dev-certs -- --listen 0.0.0.0 --port 6666
//! Then, from another shell:
//!   curl -s http://127.0.0.1:6666/health
//!   curl -s http://127.0.0.1:6666/notes -X POST -H 'content-type: application/json' -d '{"title":"hi","body":"first note"}'
//!   curl -s http://127.0.0.1:6666/notes
//!
//! Add `--tls` to serve HTTPS instead (self-signed dev cert — use `curl -sk https://...`).

use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{params, Row};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

use ling_http::{App, Db, HttpError, Resource, Result as HttpResult};

const MIGRATIONS: &[(&str, &str)] = &[("0001_init", include_str!("../migrations/0001_init.sql"))];

#[derive(Clone)]
struct AppState {
    db: Db,
}

#[derive(Debug, Serialize)]
struct Note {
    id: i64,
    title: String,
    body: String,
    created_at: String,
}

fn row_to_note(row: &Row) -> rusqlite::Result<Note> {
    Ok(Note {
        id: row.get(0)?,
        title: row.get(1)?,
        body: row.get(2)?,
        created_at: row.get(3)?,
    })
}

#[derive(Debug, Deserialize)]
struct CreateNote {
    title: String,
    #[serde(default)]
    body: String,
}

#[derive(Debug, Deserialize)]
struct UpdateNote {
    title: Option<String>,
    body: Option<String>,
}

struct Notes;

#[async_trait::async_trait]
impl Resource for Notes {
    type State = AppState;
    type Id = i64;
    type Item = Note;
    type Create = CreateNote;
    type Update = UpdateNote;

    async fn index(state: &AppState) -> HttpResult<Vec<Note>> {
        state
            .db
            .with_conn(|conn| {
                let mut stmt =
                    conn.prepare("SELECT id, title, body, created_at FROM notes ORDER BY id")?;
                let rows = stmt.query_map([], row_to_note)?;
                rows.collect()
            })
            .await
    }

    async fn show(state: &AppState, id: i64) -> HttpResult<Note> {
        state
            .db
            .with_conn(move |conn| {
                conn.query_row(
                    "SELECT id, title, body, created_at FROM notes WHERE id = ?1",
                    params![id],
                    row_to_note,
                )
            })
            .await
    }

    async fn create(state: &AppState, body: CreateNote) -> HttpResult<Note> {
        state
            .db
            .with_conn(move |conn| {
                conn.query_row(
                    "INSERT INTO notes (title, body) VALUES (?1, ?2) \
                     RETURNING id, title, body, created_at",
                    params![body.title, body.body],
                    row_to_note,
                )
            })
            .await
    }

    async fn update(state: &AppState, id: i64, body: UpdateNote) -> HttpResult<Note> {
        let existing = Self::show(state, id).await?;
        let title = body.title.unwrap_or(existing.title);
        let note_body = body.body.unwrap_or(existing.body);
        state
            .db
            .with_conn(move |conn| {
                conn.query_row(
                    "UPDATE notes SET title = ?1, body = ?2 WHERE id = ?3 \
                     RETURNING id, title, body, created_at",
                    params![title, note_body, id],
                    row_to_note,
                )
            })
            .await
    }

    async fn destroy(state: &AppState, id: i64) -> HttpResult<()> {
        let deleted = state
            .db
            .with_conn(move |conn| conn.execute("DELETE FROM notes WHERE id = ?1", params![id]))
            .await?;
        if deleted == 0 {
            return Err(HttpError::NotFound);
        }
        Ok(())
    }
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "service": "ling-http/hello" }))
}

/// ling-http demo server — health check + a SQLite-backed `notes` CRUD resource.
#[derive(clap::Parser)]
#[command(name = "ling-http-hello")]
struct Cli {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1")]
    listen: String,

    /// Port to listen on.
    #[arg(long, default_value_t = 8443)]
    port: u16,

    /// Serve HTTPS with a throwaway self-signed dev cert instead of plain HTTP.
    #[arg(long)]
    tls: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let cli = <Cli as clap::Parser>::parse();

    let db = Db::connect_memory().await?;
    db.run_migrations(MIGRATIONS).await?;
    let state = AppState { db };

    let router: Router = App::new()
        .merge(Router::new().route("/health", get(health)))
        .merge(ling_http::resource::<Notes>("/notes"))
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", cli.listen, cli.port).parse()?;
    if cli.tls {
        ling_http::serve_dev_tls(router, addr).await?;
    } else {
        ling_http::serve_http(router, addr).await?;
    }
    Ok(())
}
