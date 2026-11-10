# ling-http 与 ling-web

`ling-http` 和 `ling-web` 是 Ling Web 功能底层的 Rust crate。Ling 自己的内置函数 `http_serve`
就运行在 `ling-http` 上。当一个服务的需求超出了 Ling 内置函数的能力，比如需要会话、模板、
结构复杂的 JSON 请求体，或者要同时处理很多请求时，你也可以直接在 Rust 中使用这两个 crate。

- **`ling-http`**（灵网，0.1.8）：一套 axum 工具，包含带连接池的 SQLite 句柄、统一的 JSON 错误类型、
  CRUD 路由，以及 HTTP/HTTPS 服务。
- **`ling-web`**（灵网站，0.1.9）：建立在它之上的 Flask 风格层：服务端会话、闪现消息、模板、
  静态文件、蓝图和 CSRF 防护。

Ling 程序无法调用 `ling-web`（它没有内置函数），Rust 服务也不能把 `.ling` 文件当作处理函数运行。
两者共享的是 crate 和约定，而不是运行时。要直接用 Ling 写一个小型 API，请参见
[一个小型 Web 服务](../tutorials/web-service.md)和 [Web 服务器与 HTTP](../reference/web.md)。

## 用 Ling 还是 Rust？

| 关注点 | Ling（`http_serve`） | Rust + `ling-http` / `ling-web` |
|---|---|---|
| 并发 | 解释器上一次只处理一个请求 | 在线程池上运行的异步处理函数 |
| JSON 请求体 | 手动构造和解析 | `serde` 结构体、`Json<T>` |
| 会话、CSRF、闪现消息、模板 | 用 `db_*` 和 cookie 自己实现 | 由 `ling-web` 提供 |
| 响应头和中间件 | 固定的几种 | 任意响应头、任意 `tower` 层 |
| 修改周期 | 编辑后 `ling run` | 用 Cargo 编译 |

挡在 SQLite 前面、只有少数几个路由的轻量 API 很适合用 Ling。带登录和模板、面向浏览器的应用
更适合用 Rust。

## 准备工作

在 `Cargo.toml` 中加入这些 crate 和 Tokio：

```toml
[dependencies]
ling-http = "0.1"
ling-web  = "0.1"
tokio     = { version = "1", features = ["rt-multi-thread", "macros"] }
anyhow    = "1"
```

`ling-http` 重新导出了 `axum`、`rusqlite`、`serde_json` 和 `tokio`，所以你可以写
`ling_http::axum::...`，永远不会和它自带的 `axum` 版本冲突。

## ling-http

| 项 | 作用 |
|---|---|
| `Db::connect(path)`、`Db::connect_memory()` | 打开带连接池的 SQLite 数据库（异步）。 |
| `db.with_conn(\|conn\| ...)` | 在阻塞线程上，用池中的 `rusqlite::Connection` 运行一个闭包。错误会变成 `HttpError`，所以可以直接用 `?`。 |
| `db.run_migrations(&[(name, sql), ...])` | 按顺序执行迁移，并记录在 `_migrations` 表中。 |
| `HttpError`、`Result<T>` | 会变成 JSON 响应体 `{"error": "..."}` 并带有对应状态码的错误类型（`NotFound`、`BadRequest`、`Unauthorized`、`Forbidden`、`Conflict`、`Database`、`Pool`、`Internal`）。 |
| `Resource` trait、`resource::<R>(path)` | 实现一次 `index`、`show`、`create`、`update`、`destroy`，就得到五个 CRUD 路由。 |
| `App::new()`、`.merge(...)`、`.with_state(...)` | 路由构建器，带有请求追踪、60 秒超时、25 MiB 请求体上限和宽松的 CORS。 |
| `serve_http(router, addr)` | 以普通 HTTP 提供服务（放在负责 TLS 的反向代理后面使用）。 |
| `serve_tls(router, addr, cert_pem, key_pem)` | 用磁盘上的证书和私钥提供 HTTPS 服务。 |
| `serve_dev_tls(router, addr)` | 用临时生成的自签名证书提供 HTTPS。仅限本地开发；需要 `dev-certs` 特性。 |

一个带 SQLite 计数器的最小 JSON 服务：

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

> **警告：** `App::new()` 把中间件加在一个空路由上。在 axum 中，层只会包裹调用 `.layer(...)`
> 时已经存在的路由，所以之后再合并进来的路由得不到超时、请求体上限和 CORS 策略。
> Ling 之书的测试已经在 CORS 上证实了这一点。请在所有路由合并完之后，把你自己的 `tower-http`
> 层加到**最终**的路由上。

`HttpError::Database` 会把数据库自己的消息原样传给客户端（例如
`database error: Query returned no rows`）。对于公开的 API，请把错误映射到你自己的类型，
让内部细节只留在服务器上。

## ling-web

| 项 | 作用 |
|---|---|
| `session_layer(&db, secure)` | 基于 SQLite 的会话：不透明的 `ling_session` cookie，`HttpOnly`、`SameSite=Lax`，7 天滑动过期。通过 HTTPS 提供服务时传入 `secure = true`。 |
| `Session` | 处理函数用的提取器：`get`、`insert`、`remove` 等。 |
| `csrf::csrf_token(&session)`、`csrf::verify_csrf(&session, token)` | 每个会话一个 CSRF 令牌，以及常数时间的校验。 |
| `flash::flash(&session, category, message)`、`flash::take_flashes(&session)` | 在下一个页面显示一次的消息。 |
| `Templates::load(dir)`、`templates.render(name, ctx)` | minijinja（Jinja2 语法）模板；用 `minijinja::context!` 构造 `ctx`。 |
| `blueprint(prefix, router)` | 把路由归到一个 URL 前缀下。 |
| `serve_static(prefix, dir)` | 挂载一个静态文件夹。 |

一个在会话中记录访问次数的页面：

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

会话层要加在它所服务的路由**之后**，否则接收 `Session` 的处理函数会在请求时失败。
你自己的中间件再加在它之后，这样才能看到每一个响应。

## 仓库中的示例

- `crates/ling-http/examples/hello.rs`：基于 SQLite 的 `notes` CRUD 资源和健康检查，
  可用 HTTP 或 HTTPS：
  `cargo run -p ling-http --example hello --features dev-certs -- --port 6666`
- `crates/ling-web/examples/guestbook.rs`：会话、闪现消息、带 CSRF 防护的表单和模板：
  `cargo run -p ling-web --example guestbook -- --port 7777`

[Ling 之书](../books.md)中《Mastering Ling》的“Web Apps with the Rust Crates”一章，
构建了一个带账户、会话、模板和 JSON API 的完整应用。
