# ling-http and ling-web

`ling-http` and `ling-web` are the Rust crates under Ling's web support. Ling's own
`http_serve` builtin runs on `ling-http`. You can also use both crates directly
from Rust when a service outgrows what Ling's builtins offer: sessions, templates,
JSON bodies of real shape, or many requests in flight at once.

- **`ling-http`** (灵网, 0.1.8): an axum toolkit with a pooled SQLite handle, a
  uniform JSON error type, CRUD routing and HTTP/HTTPS serving.
- **`ling-web`** (灵网站, 0.1.9): a Flask-style layer on top: server-side sessions,
  flash messages, templates, static files, blueprints and CSRF protection.

Ling programs can't call `ling-web` (it has no builtins), and a Rust service can't
run a `.ling` file as a handler. The two share crates and conventions, not a
runtime. To write a small API in Ling itself, see
[A Small Web Service](../tutorials/web-service.md) and
[Web Server and HTTP](../reference/web.md).

## Ling or Rust?

| Concern | Ling (`http_serve`) | Rust with `ling-http` / `ling-web` |
|---|---|---|
| Concurrency | One handler at a time on the interpreter | Async handlers on a thread pool |
| JSON bodies | Build and parse by hand | `serde` structs, `Json<T>` |
| Sessions, CSRF, flash, templates | Build them yourself from `db_*` and cookies | Provided by `ling-web` |
| Response headers and middleware | A fixed set | Any header, any `tower` layer |
| Edit cycle | Edit and `ling run` | Compile with Cargo |

A thin API in front of SQLite with a handful of routes fits Ling well. A
browser-facing application with logins and templates fits Rust better.

## Setting up

Add the crates and Tokio to your `Cargo.toml`:

```toml
[dependencies]
ling-http = "0.1"
ling-web  = "0.1"
tokio     = { version = "1", features = ["rt-multi-thread", "macros"] }
anyhow    = "1"
```

`ling-http` re-exports `axum`, `rusqlite`, `serde_json` and `tokio`, so you can write
`ling_http::axum::...` and never get a version mismatch with the crate's own `axum`.

## ling-http

| Item | What it does |
|---|---|
| `Db::connect(path)`, `Db::connect_memory()` | Open a pooled SQLite database (async). |
| `db.with_conn(\|conn\| ...)` | Run a closure on a pooled `rusqlite::Connection` on a blocking thread. Errors become `HttpError`, so you can use `?`. |
| `db.run_migrations(&[(name, sql), ...])` | Apply migrations in order, recording them in a `_migrations` table. |
| `HttpError`, `Result<T>` | Error type that turns into a JSON body `{"error": "..."}` with a matching status (`NotFound`, `BadRequest`, `Unauthorized`, `Forbidden`, `Conflict`, `Database`, `Pool`, `Internal`). |
| `Resource` trait, `resource::<R>(path)` | Implement `index`, `show`, `create`, `update`, `destroy` once and get the five CRUD routes. |
| `App::new()`, `.merge(...)`, `.with_state(...)` | A router builder with tracing, a 60 s timeout, a 25 MiB body limit and permissive CORS. |
| `serve_http(router, addr)` | Serve plain HTTP (use behind a TLS-terminating proxy). |
| `serve_tls(router, addr, cert_pem, key_pem)` | Serve HTTPS with a certificate and key from disk. |
| `serve_dev_tls(router, addr)` | HTTPS with a throwaway self-signed certificate. Local development only; needs the `dev-certs` feature. |

A minimal JSON service with a SQLite counter:

```rust
use ling_http::axum::{extract::State, routing::get, Json, Router};
use ling_http::{serde_json, Db};
use std::net::SocketAddr;

const MIGRATIONS: &[(&str, &str)] = &[(
    "0001_init",
    "CREATE TABLE visits(id INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT NOT NULL DEFAULT (datetime('now')));",
)];

#[derive(Clone)]
struct AppState {
    db: Db,
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn visit(State(state): State<AppState>) -> ling_http::Result<String> {
    let n: i64 = state
        .db
        .with_conn(|conn| {
            conn.execute("INSERT INTO visits DEFAULT VALUES", [])?;
            conn.query_row("SELECT COUNT(*) FROM visits", [], |row| row.get(0))
        })
        .await?;
    Ok(format!("visit number {n}\n"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db = Db::connect("app.db").await?;
    db.run_migrations(MIGRATIONS).await?;

    let router: Router = Router::new()
        .route("/health", get(health))
        .route("/visit", get(visit))
        .with_state(AppState { db });

    let addr: SocketAddr = "127.0.0.1:8080".parse()?;
    ling_http::serve_http(router, addr).await
}
```

> **Warning:** `App::new()` adds its middleware to an empty router. In axum, a layer
> only wraps the routes that exist when you call `.layer(...)`, so routes merged in
> afterwards don't get the timeout, body limit or CORS policy. The Ling Book's tests
> confirmed this for CORS. Add your own `tower-http` layers to the **final** router,
> after all routes are merged.

`HttpError::Database` passes the database's own message to the client (for example
`database error: Query returned no rows`). For a public API, map errors to your own
type so internal details stay on the server.

## ling-web

| Item | What it does |
|---|---|
| `session_layer(&db, secure)` | SQLite-backed sessions: an opaque `ling_session` cookie, `HttpOnly`, `SameSite=Lax`, 7-day sliding expiry. Pass `secure = true` when serving HTTPS. |
| `Session` | Extractor for handlers: `get`, `insert`, `remove`, ... |
| `csrf::csrf_token(&session)`, `csrf::verify_csrf(&session, token)` | A per-session CSRF token and a constant-time check. |
| `flash::flash(&session, category, message)`, `flash::take_flashes(&session)` | One-time messages for the next page. |
| `Templates::load(dir)`, `templates.render(name, ctx)` | minijinja (Jinja2-syntax) templates; build `ctx` with `minijinja::context!`. |
| `blueprint(prefix, router)` | Group routes under a URL prefix. |
| `serve_static(prefix, dir)` | Mount a folder of static files. |

A page that counts your visits in the session:

```rust
use ling_http::axum::{routing::get, Router};
use ling_http::Db;
use ling_web::Session;
use std::net::SocketAddr;

async fn index(session: Session) -> String {
    let visits: u32 = session.get::<u32>("visits").await.unwrap_or(None).unwrap_or(0) + 1;
    let _ = session.insert("visits", visits).await;
    format!("You have been here {visits} time(s).\n")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db = Db::connect_memory().await?;
    let sessions = ling_web::session_layer(&db, false).await?;

    let router: Router = Router::new()
        .route("/", get(index))
        .merge(ling_web::serve_static("/static", "./static"))
        .layer(sessions);

    let addr: SocketAddr = "127.0.0.1:8080".parse()?;
    ling_http::serve_http(router, addr).await
}
```

Add the session layer **after** the routes it serves; handlers that take a `Session`
fail at request time otherwise. Add your own middleware after that, so it sees every
response.

## Examples in the repository

- `crates/ling-http/examples/hello.rs`: a SQLite-backed `notes` CRUD resource with a
  health check, over HTTP or HTTPS:
  `cargo run -p ling-http --example hello --features dev-certs -- --port 6666`
- `crates/ling-web/examples/guestbook.rs`: sessions, flash messages, CSRF-protected
  forms and templates: `cargo run -p ling-web --example guestbook -- --port 7777`

The chapter "Web Apps with the Rust Crates" in *Mastering Ling* ([Ling Book](../books.md))
builds a complete application with accounts, sessions, templates and a JSON API.
