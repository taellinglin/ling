# ling-http と ling-web

`ling-http` と `ling-web` は、Ling の Web 機能の土台となる Rust クレートです。Ling 自身の組み込み関数
`http_serve` は `ling-http` の上で動いています。セッション、テンプレート、しっかりした形の JSON ボディ、
同時に多数のリクエストなど、Ling の組み込み関数では足りなくなったときは、2 つのクレートを Rust から直接使えます。

- **`ling-http`**（灵网、0.1.8）：プール付きの SQLite ハンドル、統一された JSON エラー型、CRUD ルーティング、
  HTTP/HTTPS での配信を備えた axum ツールキット。
- **`ling-web`**（灵网站、0.1.9）：その上に載る Flask 風のレイヤー。サーバー側セッション、フラッシュメッセージ、
  テンプレート、静的ファイル、ブループリント、CSRF 対策。

Ling のプログラムから `ling-web` は呼べません（組み込み関数がありません）。また Rust のサービスで `.ling`
ファイルをハンドラとして動かすこともできません。共有しているのはクレートと約束ごとで、ランタイムではありません。
Ling 自身で小さな API を書くには、[小さな Web サービス](../tutorials/web-service.md)と
[Web サーバーと HTTP](../reference/web.md)を参照してください。

## Ling か Rust か

| 観点 | Ling（`http_serve`） | Rust + `ling-http` / `ling-web` |
|---|---|---|
| 並行性 | インタプリタ上で一度に 1 つのハンドラ | スレッドプール上の非同期ハンドラ |
| JSON ボディ | 手で組み立てて解析 | `serde` の構造体、`Json<T>` |
| セッション、CSRF、フラッシュ、テンプレート | `db_*` とクッキーで自作 | `ling-web` が提供 |
| レスポンスヘッダーとミドルウェア | 決まったものだけ | どんなヘッダーも、どんな `tower` レイヤーも |
| 修正のサイクル | 編集して `ling run` | Cargo でコンパイル |

SQLite の前に立つ、ルートが数本だけの薄い API なら Ling が向いています。ログインやテンプレートのある
ブラウザー向けアプリケーションなら Rust のほうが向いています。

## 準備

`Cargo.toml` にクレートと Tokio を追加します。

```toml
[dependencies]
ling-http = "0.1"
ling-web  = "0.1"
tokio     = { version = "1", features = ["rt-multi-thread", "macros"] }
anyhow    = "1"
```

`ling-http` は `axum`、`rusqlite`、`serde_json`、`tokio` を再エクスポートしているので、
`ling_http::axum::...` と書けば、クレート自身の `axum` とバージョンが食い違うことはありません。

## ling-http

| 項目 | 内容 |
|---|---|
| `Db::connect(path)`、`Db::connect_memory()` | プール付きの SQLite データベースを開きます（非同期）。 |
| `db.with_conn(\|conn\| ...)` | プールの `rusqlite::Connection` を使ってクロージャをブロッキングスレッドで実行します。エラーは `HttpError` になるので `?` が使えます。 |
| `db.run_migrations(&[(name, sql), ...])` | マイグレーションを順に適用し、`_migrations` テーブルに記録します。 |
| `HttpError`、`Result<T>` | 対応するステータスと JSON ボディ `{"error": "..."}` に変わるエラー型（`NotFound`、`BadRequest`、`Unauthorized`、`Forbidden`、`Conflict`、`Database`、`Pool`、`Internal`）。 |
| `Resource` トレイト、`resource::<R>(path)` | `index`、`show`、`create`、`update`、`destroy` を一度実装すれば、5 つの CRUD ルートが手に入ります。 |
| `App::new()`、`.merge(...)`、`.with_state(...)` | トレース、60 秒のタイムアウト、25 MiB のボディ上限、寛容な CORS を備えたルータービルダー。 |
| `serve_http(router, addr)` | 平文の HTTP で配信します（TLS を終端するリバースプロキシの後ろで使います）。 |
| `serve_tls(router, addr, cert_pem, key_pem)` | ディスク上の証明書と鍵で HTTPS を配信します。 |
| `serve_dev_tls(router, addr)` | 使い捨ての自己署名証明書で HTTPS を配信します。ローカル開発専用で、`dev-certs` フィーチャーが必要です。 |

SQLite のカウンターを持つ最小の JSON サービス：

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

> **警告：** `App::new()` はミドルウェアを空のルーターに追加します。axum のレイヤーは `.layer(...)` を
> 呼んだ時点で存在するルートしか包まないので、あとから merge したルートにはタイムアウトもボディ上限も
> CORS ポリシーも効きません。Ling ブックのテストでは CORS についてこれを確認しています。
> すべてのルートを merge したあと、**最終的な**ルーターに自分で `tower-http` のレイヤーを追加してください。

`HttpError::Database` はデータベース自身のメッセージをそのままクライアントに渡します（たとえば
`database error: Query returned no rows`）。公開 API では、内部の詳細がサーバーの外に出ないよう、
エラーを自分の型に変換してください。

## ling-web

| 項目 | 内容 |
|---|---|
| `session_layer(&db, secure)` | SQLite に保存するセッション：中身の見えない `ling_session` クッキー、`HttpOnly`、`SameSite=Lax`、7 日間のスライディング有効期限。HTTPS で配信するときは `secure = true` を渡します。 |
| `Session` | ハンドラ用のエクストラクター：`get`、`insert`、`remove` など。 |
| `csrf::csrf_token(&session)`、`csrf::verify_csrf(&session, token)` | セッションごとの CSRF トークンと、定数時間での照合。 |
| `flash::flash(&session, category, message)`、`flash::take_flashes(&session)` | 次のページで一度だけ表示するメッセージ。 |
| `Templates::load(dir)`、`templates.render(name, ctx)` | minijinja（Jinja2 構文）のテンプレート。`ctx` は `minijinja::context!` で作ります。 |
| `blueprint(prefix, router)` | ルートを URL のプレフィックスの下にまとめます。 |
| `serve_static(prefix, dir)` | 静的ファイルのフォルダーをマウントします。 |

セッションで訪問回数を数えるページ：

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

セッションレイヤーは、それが担当するルートの**あと**に追加してください。そうしないと `Session`
を受け取るハンドラがリクエスト時に失敗します。自分のミドルウェアはさらにそのあとに追加すると、
すべてのレスポンスを見られます。

## リポジトリーの例

- `crates/ling-http/examples/hello.rs`：SQLite を使った `notes` の CRUD リソースとヘルスチェック。
  HTTP でも HTTPS でも動きます：
  `cargo run -p ling-http --example hello --features dev-certs -- --port 6666`
- `crates/ling-web/examples/guestbook.rs`：セッション、フラッシュメッセージ、CSRF で守られたフォーム、
  テンプレート：`cargo run -p ling-web --example guestbook -- --port 7777`

[Ling ブック](../books.md)の『Mastering Ling』の章「Web Apps with the Rust Crates」では、
アカウント、セッション、テンプレート、JSON API を備えた完全なアプリケーションを作ります。
