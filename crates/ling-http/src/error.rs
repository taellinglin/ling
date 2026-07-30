use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Uniform error type for ling-http handlers. Converts to a JSON body of the
/// shape `{"error": "..."}` with a matching HTTP status code, so every API
/// built on this framework returns errors in one consistent shape.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("not found")]
    NotFound,

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("database pool error: {0}")]
    Pool(#[from] r2d2::Error),

    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl HttpError {
    fn status(&self) -> StatusCode {
        match self {
            HttpError::NotFound => StatusCode::NOT_FOUND,
            HttpError::BadRequest(_) => StatusCode::BAD_REQUEST,
            HttpError::Unauthorized => StatusCode::UNAUTHORIZED,
            HttpError::Forbidden => StatusCode::FORBIDDEN,
            HttpError::Conflict(_) => StatusCode::CONFLICT,
            HttpError::Database(rusqlite::Error::QueryReturnedNoRows) => StatusCode::NOT_FOUND,
            HttpError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
            HttpError::Pool(_) => StatusCode::INTERNAL_SERVER_ERROR,
            HttpError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = %self, "internal error");
        }
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}

pub type Result<T> = std::result::Result<T, HttpError>;
