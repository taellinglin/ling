use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::Result;

/// A REST-ish resource controller: implement this once for a type and get
/// the five conventional CRUD routes (`GET /x`, `GET /x/:id`, `POST /x`,
/// `PUT /x/:id`, `DELETE /x/:id`) wired up for free via [`resource`].
///
/// `State` is the app state (typically holding a [`crate::db::Db`]); `Id` is
/// the identifier as it appears in the URL; `Item` is the JSON shape
/// returned to clients; `Create`/`Update` are the accepted JSON bodies.
#[async_trait::async_trait]
pub trait Resource: Send + Sync + 'static {
    type State: Clone + Send + Sync + 'static;
    type Id: DeserializeOwned + Send + Sync + 'static;
    type Item: Serialize + Send + Sync + 'static;
    type Create: DeserializeOwned + Send + Sync + 'static;
    type Update: DeserializeOwned + Send + Sync + 'static;

    async fn index(state: &Self::State) -> Result<Vec<Self::Item>>;
    async fn show(state: &Self::State, id: Self::Id) -> Result<Self::Item>;
    async fn create(state: &Self::State, body: Self::Create) -> Result<Self::Item>;
    async fn update(state: &Self::State, id: Self::Id, body: Self::Update) -> Result<Self::Item>;
    async fn destroy(state: &Self::State, id: Self::Id) -> Result<()>;
}

/// Mounts a [`Resource`] at `path`, wiring the five CRUD routes onto it.
///
/// ```ignore
/// let router = Router::new().merge(resource::<Notes>("/notes"));
/// ```
pub fn resource<R: Resource>(path: &str) -> Router<R::State> {
    let item_path = format!("{path}/{{id}}");
    Router::new()
        .route(path, get(index::<R>).post(create::<R>))
        .route(
            &item_path,
            get(show::<R>).put(update::<R>).delete(destroy::<R>),
        )
}

async fn index<R: Resource>(State(state): State<R::State>) -> Result<Json<Vec<R::Item>>> {
    Ok(Json(R::index(&state).await?))
}

async fn show<R: Resource>(
    State(state): State<R::State>,
    Path(id): Path<R::Id>,
) -> Result<Json<R::Item>> {
    Ok(Json(R::show(&state, id).await?))
}

async fn create<R: Resource>(
    State(state): State<R::State>,
    Json(body): Json<R::Create>,
) -> Result<Json<R::Item>> {
    Ok(Json(R::create(&state, body).await?))
}

async fn update<R: Resource>(
    State(state): State<R::State>,
    Path(id): Path<R::Id>,
    Json(body): Json<R::Update>,
) -> Result<Json<R::Item>> {
    Ok(Json(R::update(&state, id, body).await?))
}

async fn destroy<R: Resource>(
    State(state): State<R::State>,
    Path(id): Path<R::Id>,
) -> Result<axum::http::StatusCode> {
    R::destroy(&state, id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
