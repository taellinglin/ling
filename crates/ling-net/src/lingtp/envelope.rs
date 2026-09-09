//! A small HTTP-like request/response envelope carried inside each sealed
//! lingtp record. Body is UTF-8 text — enough for the JSON APIs and HTML
//! views this protocol carries today; a binary payload would need its own
//! text-safe encoding (e.g. base64) layered on top of `body`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LingtpRequest {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LingtpResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: String,
}

impl LingtpRequest {
    pub fn get(path: impl Into<String>) -> Self {
        Self { method: "GET".into(), path: path.into(), headers: BTreeMap::new(), body: String::new() }
    }

    pub fn post(path: impl Into<String>, body: impl Into<String>) -> Self {
        Self { method: "POST".into(), path: path.into(), headers: BTreeMap::new(), body: body.into() }
    }

    pub fn put(path: impl Into<String>, body: impl Into<String>) -> Self {
        Self { method: "PUT".into(), path: path.into(), headers: BTreeMap::new(), body: body.into() }
    }

    pub fn delete(path: impl Into<String>) -> Self {
        Self { method: "DELETE".into(), path: path.into(), headers: BTreeMap::new(), body: String::new() }
    }

    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    pub fn with_json_body<T: Serialize>(mut self, value: &T) -> serde_json::Result<Self> {
        self.body = serde_json::to_string(value)?;
        self.headers.insert("content-type".into(), "application/json".into());
        Ok(self)
    }

    /// Parse `body` as JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_str(&self.body)
    }
}

impl LingtpResponse {
    pub fn json<T: Serialize>(status: u16, value: &T) -> Self {
        let body = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
        let mut headers = BTreeMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        Self { status, headers, body }
    }

    pub fn html(status: u16, html: impl Into<String>) -> Self {
        let mut headers = BTreeMap::new();
        headers.insert("content-type".to_string(), "text/html; charset=utf-8".to_string());
        Self { status, headers, body: html.into() }
    }

    pub fn text(status: u16, text: impl Into<String>) -> Self {
        let mut headers = BTreeMap::new();
        headers.insert("content-type".to_string(), "text/plain; charset=utf-8".to_string());
        Self { status, headers, body: text.into() }
    }

    pub fn error(status: u16, message: impl Into<String>) -> Self {
        Self::json(status, &serde_json::json!({ "error": message.into() }))
    }

    /// Parse `body` as JSON.
    pub fn json_body<T: serde::de::DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_str(&self.body)
    }
}
