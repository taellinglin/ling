//! End-to-end proof of ling-web's "Flask stuff": sessions, flash messages,
//! CSRF-protected forms, and minijinja templates.
//!
//! Run it:
//!   cargo run -p ling-web --example guestbook -- --listen 0.0.0.0 --port 7777
//! Then open http://127.0.0.1:7777/ in a browser and sign the guestbook.

use axum::extract::{Form, State};
use axum::response::{Html, Redirect};
use axum::routing::get;
use axum::Router;
use clap::Parser;
use ling_http::HttpError;
use ling_web::{Session, Templates};
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct AppState {
    templates: Templates,
    entries: Arc<Mutex<Vec<String>>>,
}

#[derive(Deserialize)]
struct SignForm {
    name: String,
    csrf_token: String,
}

async fn index(State(state): State<AppState>, session: Session) -> Result<Html<String>, HttpError> {
    let visits: u32 = session
        .get::<u32>("visits")
        .await
        .unwrap_or(None)
        .unwrap_or(0)
        + 1;
    session
        .insert("visits", visits)
        .await
        .map_err(|e| HttpError::Internal(e.into()))?;

    let token = ling_web::csrf::csrf_token(&session)
        .await
        .map_err(HttpError::Internal)?;
    let flashes = ling_web::flash::take_flashes(&session)
        .await
        .map_err(HttpError::Internal)?;
    let entries = state.entries.lock().unwrap().clone();

    state.templates.render(
        "index.html",
        minijinja::context! {
            visits,
            csrf_token => token,
            flashes,
            entries,
        },
    )
}

async fn sign(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<SignForm>,
) -> Result<Redirect, HttpError> {
    let ok = ling_web::csrf::verify_csrf(&session, &form.csrf_token)
        .await
        .map_err(HttpError::Internal)?;
    if !ok {
        ling_web::flash::flash(&session, "error", "That form expired — try again.")
            .await
            .map_err(HttpError::Internal)?;
        return Ok(Redirect::to("/"));
    }

    state.entries.lock().unwrap().push(form.name.clone());
    ling_web::flash::flash(&session, "success", format!("Thanks, {}!", form.name))
        .await
        .map_err(HttpError::Internal)?;
    Ok(Redirect::to("/"))
}

#[derive(Parser)]
#[command(name = "ling-web-guestbook")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1")]
    listen: String,
    #[arg(long, default_value_t = 7777)]
    port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let db = ling_http::Db::connect_memory().await?;
    let sessions = ling_web::session_layer(&db, false).await?;

    let templates = Templates::load(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/templates"))?;
    let state = AppState { templates, entries: Arc::new(Mutex::new(Vec::new())) };

    let router: Router = Router::new()
        .route("/", get(index).post(sign))
        .merge(ling_web::serve_static(
            "/static",
            concat!(env!("CARGO_MANIFEST_DIR"), "/examples/static"),
        ))
        .with_state(state)
        .layer(sessions);

    let addr: SocketAddr = format!("{}:{}", cli.listen, cli.port).parse()?;
    println!("ling-web guestbook listening on http://{addr}");
    ling_http::serve_http(router, addr).await?;
    Ok(())
}
