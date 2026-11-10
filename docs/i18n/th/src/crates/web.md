# ling-http และ ling-web

`ling-http` และ `ling-web` คือ crate ของ Rust ที่อยู่เบื้องหลังความสามารถด้านเว็บของ Ling ฟังก์ชันในตัว `http_serve`
ของ Ling ทำงานบน `ling-http` คุณยังใช้ทั้งสอง crate จาก Rust ได้โดยตรง เมื่อเซอร์วิสต้องการมากกว่าที่ฟังก์ชันในตัวของ Ling ให้ได้
เช่น เซสชัน เทมเพลต เนื้อหา JSON ที่มีโครงสร้างจริงจัง หรือการรับคำขอจำนวนมากพร้อมกัน

- **`ling-http`** (灵网, 0.1.8): ชุดเครื่องมือ axum ที่มีตัวจัดการ SQLite แบบพูล ชนิดข้อผิดพลาด JSON แบบเดียวกันทั้งระบบ
  การกำหนดเส้นทางแบบ CRUD และการให้บริการ HTTP/HTTPS
- **`ling-web`** (灵网站, 0.1.9): ชั้นแบบ Flask ที่อยู่ด้านบน: เซสชันฝั่งเซิร์ฟเวอร์ ข้อความแฟลช เทมเพลต
  ไฟล์สแตติก บลูพรินต์ และการป้องกัน CSRF

โปรแกรม Ling เรียก `ling-web` ไม่ได้ (มันไม่มีฟังก์ชันในตัว) และเซอร์วิส Rust ก็รันไฟล์ `.ling` เป็นตัวจัดการไม่ได้
ทั้งสองใช้ crate และแนวปฏิบัติร่วมกัน แต่ไม่ได้ใช้รันไทม์เดียวกัน ถ้าจะเขียน API เล็ก ๆ ด้วย Ling เอง
ดู[เว็บเซอร์วิสขนาดเล็ก](../tutorials/web-service.md) และ[เว็บเซิร์ฟเวอร์และ HTTP](../reference/web.md)

## Ling หรือ Rust?

| ประเด็น | Ling (`http_serve`) | Rust กับ `ling-http` / `ling-web` |
|---|---|---|
| การทำงานพร้อมกัน | ตัวจัดการทีละตัวบนอินเทอร์พรีเตอร์ | ตัวจัดการแบบ async บนพูลเธรด |
| เนื้อหา JSON | สร้างและแยกเองด้วยมือ | struct ของ `serde`, `Json<T>` |
| เซสชัน CSRF ข้อความแฟลช เทมเพลต | สร้างเองจาก `db_*` และคุกกี้ | `ling-web` มีให้ |
| เฮดเดอร์ตอบกลับและมิดเดิลแวร์ | ชุดตายตัว | เฮดเดอร์ใดก็ได้ ชั้น `tower` ใดก็ได้ |
| รอบการแก้ไข | แก้แล้ว `ling run` | คอมไพล์ด้วย Cargo |

API บาง ๆ ที่อยู่หน้า SQLite และมีเส้นทางไม่กี่เส้นเหมาะกับ Ling ส่วนแอปที่ผู้ใช้เปิดผ่านเบราว์เซอร์ มีการล็อกอินและเทมเพลต
เหมาะกับ Rust มากกว่า

## การเตรียมโปรเจกต์

เพิ่ม crate และ Tokio ลงใน `Cargo.toml`:

```toml
[dependencies]
ling-http = "0.1"
ling-web  = "0.1"
tokio     = { version = "1", features = ["rt-multi-thread", "macros"] }
anyhow    = "1"
```

`ling-http` ส่งออก `axum`, `rusqlite`, `serde_json` และ `tokio` ซ้ำ คุณจึงเขียน `ling_http::axum::...`
ได้โดยไม่มีปัญหาเวอร์ชันไม่ตรงกับ `axum` ของ crate

## ling-http

| รายการ | หน้าที่ |
|---|---|
| `Db::connect(path)`, `Db::connect_memory()` | เปิดฐานข้อมูล SQLite แบบพูล (async) |
| `db.with_conn(\|conn\| ...)` | รันโคลเชอร์กับ `rusqlite::Connection` จากพูลบนเธรดที่บล็อกได้ ข้อผิดพลาดจะกลายเป็น `HttpError` จึงใช้ `?` ได้ |
| `db.run_migrations(&[(name, sql), ...])` | ใช้ migration ตามลำดับ และบันทึกไว้ในตาราง `_migrations` |
| `HttpError`, `Result<T>` | ชนิดข้อผิดพลาดที่กลายเป็นเนื้อหา JSON `{"error": "..."}` พร้อมรหัสสถานะที่ตรงกัน (`NotFound`, `BadRequest`, `Unauthorized`, `Forbidden`, `Conflict`, `Database`, `Pool`, `Internal`) |
| trait `Resource`, `resource::<R>(path)` | เขียน `index`, `show`, `create`, `update`, `destroy` ครั้งเดียว แล้วได้เส้นทาง CRUD ทั้งห้าเส้น |
| `App::new()`, `.merge(...)`, `.with_state(...)` | ตัวสร้างเราเตอร์ที่มีการติดตามคำขอ หมดเวลา 60 วินาที จำกัดขนาดเนื้อหา 25 MiB และ CORS แบบเปิดกว้าง |
| `serve_http(router, addr)` | ให้บริการ HTTP ธรรมดา (ใช้หลังพร็อกซีที่จัดการ TLS) |
| `serve_tls(router, addr, cert_pem, key_pem)` | ให้บริการ HTTPS ด้วยใบรับรองและคีย์จากดิสก์ |
| `serve_dev_tls(router, addr)` | HTTPS ด้วยใบรับรองแบบลงนามเองที่สร้างชั่วคราว ใช้พัฒนาในเครื่องเท่านั้น ต้องเปิดฟีเจอร์ `dev-certs` |

เซอร์วิส JSON ขั้นต่ำที่มีตัวนับใน SQLite:

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

> **คำเตือน:** `App::new()` ใส่มิดเดิลแวร์ให้เราเตอร์ที่ยังว่างอยู่ ใน axum ชั้นหนึ่ง ๆ จะครอบเฉพาะเส้นทางที่มีอยู่ตอนเรียก `.layer(...)`
> เส้นทางที่รวมเข้ามาทีหลังจึงไม่ได้การหมดเวลา การจำกัดขนาดเนื้อหา หรือนโยบาย CORS การทดสอบในหนังสือ Ling ยืนยันเรื่องนี้กับ CORS แล้ว
> ให้เพิ่มชั้น `tower-http` ของคุณเองที่เราเตอร์**สุดท้าย** หลังรวมทุกเส้นทางแล้ว

`HttpError::Database` ส่งข้อความของฐานข้อมูลถึงไคลเอนต์ตรง ๆ (เช่น `database error: Query returned no rows`)
สำหรับ API สาธารณะ ให้แปลงข้อผิดพลาดเป็นชนิดของคุณเอง เพื่อให้รายละเอียดภายในอยู่แค่บนเซิร์ฟเวอร์

## ling-web

| รายการ | หน้าที่ |
|---|---|
| `session_layer(&db, secure)` | เซสชันที่เก็บใน SQLite: คุกกี้ `ling_session` แบบทึบ, `HttpOnly`, `SameSite=Lax` หมดอายุแบบเลื่อนได้ 7 วัน ส่ง `secure = true` เมื่อให้บริการผ่าน HTTPS |
| `Session` | ตัวดึงข้อมูลสำหรับตัวจัดการ: `get`, `insert`, `remove` ฯลฯ |
| `csrf::csrf_token(&session)`, `csrf::verify_csrf(&session, token)` | โทเค็น CSRF ต่อเซสชัน และการตรวจสอบแบบเวลาคงที่ |
| `flash::flash(&session, category, message)`, `flash::take_flashes(&session)` | ข้อความที่แสดงครั้งเดียวในหน้าถัดไป |
| `Templates::load(dir)`, `templates.render(name, ctx)` | เทมเพลต minijinja (ไวยากรณ์ Jinja2) สร้าง `ctx` ด้วย `minijinja::context!` |
| `blueprint(prefix, router)` | จัดกลุ่มเส้นทางไว้ใต้คำนำหน้า URL |
| `serve_static(prefix, dir)` | เมานต์โฟลเดอร์ไฟล์สแตติก |

หน้าที่นับจำนวนครั้งที่คุณเข้าชมไว้ในเซสชัน:

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

ให้เพิ่มชั้นเซสชัน**หลัง**เส้นทางที่มันให้บริการ ไม่เช่นนั้นตัวจัดการที่รับ `Session` จะล้มเหลวตอนรับคำขอ
แล้วจึงเพิ่มมิดเดิลแวร์ของคุณเองต่อจากนั้น เพื่อให้มันเห็นทุกการตอบกลับ

## ตัวอย่างใน repository

- `crates/ling-http/examples/hello.rs`: ทรัพยากร CRUD `notes` ที่เก็บใน SQLite พร้อมการตรวจสุขภาพ
  ผ่าน HTTP หรือ HTTPS:
  `cargo run -p ling-http --example hello --features dev-certs -- --port 6666`
- `crates/ling-web/examples/guestbook.rs`: เซสชัน ข้อความแฟลช ฟอร์มที่ป้องกัน CSRF และเทมเพลต:
  `cargo run -p ling-web --example guestbook -- --port 7777`

บท "Web Apps with the Rust Crates" ใน *Mastering Ling* ([หนังสือ Ling](../books.md))
สร้างแอปเต็มรูปแบบที่มีบัญชีผู้ใช้ เซสชัน เทมเพลต และ JSON API
