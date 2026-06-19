//! HTTP server (axum) and client (reqwest) for ling-net.

use crate::error::NetError;
use crate::types::{HttpMethod, Request, Response};

/// Make an outgoing HTTP request.
pub async fn send(req: Request) -> Result<Response, NetError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let client = reqwest::Client::new();
        let method = match req.method {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Put => reqwest::Method::PUT,
            HttpMethod::Delete => reqwest::Method::DELETE,
            HttpMethod::Patch => reqwest::Method::PATCH,
            HttpMethod::Head => reqwest::Method::HEAD,
            HttpMethod::Options => reqwest::Method::OPTIONS,
        };
        let mut builder = client.request(method, &req.url);
        for (k, v) in &req.headers {
            builder = builder.header(k, v);
        }
        if let Some(body) = req.body {
            builder = builder.body(body);
        }
        let resp = builder
            .send()
            .await
            .map_err(|e| NetError::Http(e.to_string()))?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let body = resp
            .bytes()
            .await
            .map_err(|e| NetError::Http(e.to_string()))?
            .to_vec();
        Ok(Response { status, headers, body })
    }
    #[cfg(target_arch = "wasm32")]
    {
        Err(NetError::Unsupported(
            "HTTP not available on WASM via reqwest".into(),
        ))
    }
}

/// Simple in-process HTTP GET helper.
pub async fn get(url: &str) -> Result<Vec<u8>, NetError> {
    send(Request {
        method: HttpMethod::Get,
        url: url.to_string(),
        headers: Default::default(),
        body: None,
    })
    .await
    .map(|r| r.body)
}

/// Simple in-process HTTP POST helper.
pub async fn post(url: &str, body: Vec<u8>) -> Result<Vec<u8>, NetError> {
    send(Request {
        method: HttpMethod::Post,
        url: url.to_string(),
        headers: Default::default(),
        body: Some(body),
    })
    .await
    .map(|r| r.body)
}
