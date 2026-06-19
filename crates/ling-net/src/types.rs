use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Request {
    pub method: HttpMethod,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<Vec<u8>>,
}

impl Request {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Get,
            url: url.into(),
            headers: HashMap::new(),
            body: None,
        }
    }

    pub fn post(url: impl Into<String>, body: impl Into<Vec<u8>>) -> Self {
        Self {
            method: HttpMethod::Post,
            url: url.into(),
            headers: HashMap::new(),
            body: Some(body.into()),
        }
    }

    pub fn header(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.headers.insert(key.into(), val.into());
        self
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self { status: 200, headers: HashMap::new(), body: body.into() }
    }

    pub fn text(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap_or("")
    }

    pub fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }
}
