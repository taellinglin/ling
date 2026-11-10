# ling-http와 ling-web

`ling-http`와 `ling-web`은 Ling 웹 기능의 바탕이 되는 Rust 크레이트입니다. Ling의 내장 함수
`http_serve`는 `ling-http` 위에서 동작합니다. 세션, 템플릿, 구조가 복잡한 JSON 본문, 동시에 많은
요청 처리처럼 서비스가 Ling 내장 함수의 범위를 넘어서면 Rust에서 두 크레이트를 직접 쓸 수도
있습니다.

- **`ling-http`**(灵网, 0.1.8): 커넥션 풀을 갖춘 SQLite 핸들, 통일된 JSON 오류 타입, CRUD
  라우팅, HTTP/HTTPS 서비스를 제공하는 axum 도구 모음.
- **`ling-web`**(灵网站, 0.1.9): 그 위의 Flask 스타일 계층. 서버 측 세션, 플래시 메시지, 템플릿,
  정적 파일, 블루프린트, CSRF 보호.

Ling 프로그램은 `ling-web`을 호출할 수 없고(내장 함수가 없습니다), Rust 서비스는 `.ling` 파일을
핸들러로 실행할 수 없습니다. 둘이 공유하는 것은 크레이트와 관례이지 런타임이 아닙니다. Ling으로
직접 작은 API를 만들려면 [작은 웹 서비스](../tutorials/web-service.md)와
[웹 서버와 HTTP](../reference/web.md)를 참고하십시오.

## Ling인가, Rust인가?

| 고려 사항 | Ling(`http_serve`) | Rust + `ling-http` / `ling-web` |
|---|---|---|
| 동시성 | 인터프리터에서 한 번에 핸들러 하나 | 스레드 풀 위의 비동기 핸들러 |
| JSON 본문 | 직접 만들고 파싱 | `serde` 구조체, `Json<T>` |
| 세션, CSRF, 플래시, 템플릿 | `db_*`와 쿠키로 직접 구현 | `ling-web`이 제공 |
| 응답 헤더와 미들웨어 | 정해진 몇 가지 | 어떤 헤더든, 어떤 `tower` 계층이든 |
| 수정 주기 | 편집 후 `ling run` | Cargo로 컴파일 |

SQLite 앞에 놓인 라우트 몇 개짜리 가벼운 API는 Ling에 잘 맞습니다. 로그인과 템플릿이 있는
브라우저용 애플리케이션은 Rust가 더 잘 맞습니다.

## 준비

`Cargo.toml`에 크레이트와 Tokio를 추가합니다.

```toml
[dependencies]
ling-http = "0.1"
ling-web  = "0.1"
tokio     = { version = "1", features = ["rt-multi-thread", "macros"] }
anyhow    = "1"
```

`ling-http`는 `axum`, `rusqlite`, `serde_json`, `tokio`를 다시 내보내므로
`ling_http::axum::...`처럼 쓰면 크레이트 자체의 `axum`과 버전이 어긋날 일이 없습니다.

## ling-http

| 항목 | 하는 일 |
|---|---|
| `Db::connect(path)`, `Db::connect_memory()` | 커넥션 풀이 있는 SQLite 데이터베이스를 엽니다(비동기). |
| `db.with_conn(\|conn\| ...)` | 블로킹 스레드에서 풀의 `rusqlite::Connection`으로 클로저를 실행합니다. 오류는 `HttpError`가 되므로 `?`를 쓸 수 있습니다. |
| `db.run_migrations(&[(name, sql), ...])` | 마이그레이션을 순서대로 적용하고 `_migrations` 표에 기록합니다. |
| `HttpError`, `Result<T>` | 알맞은 상태 코드와 함께 JSON 본문 `{"error": "..."}`이 되는 오류 타입(`NotFound`, `BadRequest`, `Unauthorized`, `Forbidden`, `Conflict`, `Database`, `Pool`, `Internal`). |
| `Resource` 트레이트, `resource::<R>(path)` | `index`, `show`, `create`, `update`, `destroy`를 한 번 구현하면 CRUD 라우트 다섯 개가 생깁니다. |
| `App::new()`, `.merge(...)`, `.with_state(...)` | 요청 추적, 60초 타임아웃, 25 MiB 본문 제한, 느슨한 CORS가 들어간 라우터 빌더. |
| `serve_http(router, addr)` | 일반 HTTP로 서비스합니다(TLS를 처리하는 리버스 프록시 뒤에서 사용). |
| `serve_tls(router, addr, cert_pem, key_pem)` | 디스크의 인증서와 키로 HTTPS를 서비스합니다. |
| `serve_dev_tls(router, addr)` | 일회용 자체 서명 인증서로 HTTPS를 서비스합니다. 로컬 개발 전용이며 `dev-certs` 기능이 필요합니다. |

SQLite 카운터가 있는 최소 JSON 서비스:

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

> **경고:** `App::new()`는 빈 라우터에 미들웨어를 붙입니다. axum에서 계층은 `.layer(...)`를
> 호출하는 시점에 있는 라우트만 감싸므로, 나중에 합쳐진 라우트에는 타임아웃, 본문 제한, CORS
> 정책이 적용되지 않습니다. Ling 북의 테스트에서 CORS로 이를 확인했습니다. 모든 라우트를 합친
> 뒤, **최종** 라우터에 직접 `tower-http` 계층을 추가하십시오.

`HttpError::Database`는 데이터베이스의 메시지를 그대로 클라이언트에 전달합니다(예:
`database error: Query returned no rows`). 공개 API라면 오류를 직접 만든 타입으로 바꿔 내부
정보가 서버에만 남게 하십시오.

## ling-web

| 항목 | 하는 일 |
|---|---|
| `session_layer(&db, secure)` | SQLite 기반 세션: 불투명한 `ling_session` 쿠키, `HttpOnly`, `SameSite=Lax`, 7일 슬라이딩 만료. HTTPS로 서비스할 때는 `secure = true`를 넘기십시오. |
| `Session` | 핸들러용 추출기: `get`, `insert`, `remove` 등. |
| `csrf::csrf_token(&session)`, `csrf::verify_csrf(&session, token)` | 세션별 CSRF 토큰과 상수 시간 검사. |
| `flash::flash(&session, category, message)`, `flash::take_flashes(&session)` | 다음 페이지에 한 번만 보이는 메시지. |
| `Templates::load(dir)`, `templates.render(name, ctx)` | minijinja(Jinja2 문법) 템플릿. `ctx`는 `minijinja::context!`로 만듭니다. |
| `blueprint(prefix, router)` | 라우트를 URL 접두사 아래로 묶습니다. |
| `serve_static(prefix, dir)` | 정적 파일 폴더를 연결합니다. |

세션에 방문 횟수를 세는 페이지:

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

세션 계층은 그것이 담당하는 라우트 **뒤에** 추가하십시오. 그렇지 않으면 `Session`을 받는
핸들러가 요청 시점에 실패합니다. 직접 만든 미들웨어는 그다음에 추가해야 모든 응답을 볼 수
있습니다.

## 저장소의 예제

- `crates/ling-http/examples/hello.rs`: 헬스 체크와 SQLite 기반 `notes` CRUD 리소스, HTTP
  또는 HTTPS:
  `cargo run -p ling-http --example hello --features dev-certs -- --port 6666`
- `crates/ling-web/examples/guestbook.rs`: 세션, 플래시 메시지, CSRF로 보호되는 폼, 템플릿:
  `cargo run -p ling-web --example guestbook -- --port 7777`

[Ling 북](../books.md) *Mastering Ling*의 "Web Apps with the Rust Crates" 장에서는 계정, 세션,
템플릿, JSON API를 갖춘 완전한 애플리케이션을 만듭니다.
