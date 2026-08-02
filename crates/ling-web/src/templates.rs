use axum::response::Html;
use minijinja::Environment;
use std::path::Path;
use std::sync::Arc;

/// A minijinja (Jinja2-syntax) template environment loaded from a
/// directory — Flask's `render_template`.
#[derive(Clone)]
pub struct Templates {
    env: Arc<Environment<'static>>,
}

impl Templates {
    /// Loads templates lazily from every file under `dir` (matched by
    /// name when rendered). Call once at startup and clone the handle
    /// into your app state — cloning is cheap (it's an `Arc`).
    pub fn load(dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let mut env = Environment::new();
        env.set_loader(minijinja::path_loader(dir.as_ref()));
        Ok(Self { env: Arc::new(env) })
    }

    /// Renders `name` (a path relative to the templates directory, e.g.
    /// `"login.html"`) with the given context — build one with
    /// `minijinja::context!{ key => value, ... }`.
    pub fn render(
        &self,
        name: &str,
        ctx: impl Into<minijinja::Value>,
    ) -> Result<Html<String>, ling_http::HttpError> {
        let tmpl = self.env.get_template(name).map_err(|e| {
            ling_http::HttpError::Internal(anyhow::anyhow!("template `{name}` not found: {e}"))
        })?;
        let rendered = tmpl.render(ctx.into()).map_err(|e| {
            ling_http::HttpError::Internal(anyhow::anyhow!("rendering `{name}`: {e}"))
        })?;
        Ok(Html(rendered))
    }
}
