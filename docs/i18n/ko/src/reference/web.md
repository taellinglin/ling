# 웹 서버와 HTTP

Ling에는 작고 빠른 웹 서버가 들어 있습니다. 라우트, 정적 폴더, 가상 호스트를 등록한 뒤
`http_serve`를 호출하면, 그 자리에서 멈춰 요청에 응답합니다. `http_*` 내장 함수는 Ling
코드에서 [`ling-http`](https://crates.io/crates/ling-http)(axum과 tokio)를 구동합니다.
같은 묶음에 HTTP 클라이언트(`http_get` 등)와 쿼리 문자열, 쿠키, 이스케이프를 다루는
도우미도 있습니다.

**ling-lang.org가 바로 이것으로 돌아갑니다.** 서버는 `site.ling`이라는 Ling 파일 하나입니다.
첫 페이지, `/docs/` 아래의 이 문서, Ling 북 PDF, 예제 SVG를 제공하고, `http_vhost`로
더 이상 쓰지 않는 `docs.ling-lang.org` 서브도메인을 `/docs/`로 보냅니다.

> **설치 프로필.** 서버와 클라이언트 내장 함수는 `web` 프로필(그리고 기본값인 `full`)에
> 들어 있습니다. `--no-default-features --features core`로 빌드한 `ling`은
> `unknown function 'http_static'`에서 멈춥니다. `cargo install ling-lang`으로 다시
> 설치하거나 `--features web`을 붙이세요.

## 요청에 응답하는 순서

1. **가상 호스트**(`http_vhost`)가 먼저입니다. `Host` 헤더가 등록된 호스트를 가리키면
   그 호스트의 폴더에서 응답하고, 대상이 URL이면 리디렉션합니다.
2. 다음은 **정적 접두사**(`http_static`)입니다. `/css` 접두사 아래의 `/css/style.css`는
   디스크에서 바이트 그대로 읽습니다.
3. 마지막은 **라우트**(`http_route`)로, 메서드와 **정확히 일치하는** 경로로 고릅니다.
   핸들러는 `Request`를 받아 텍스트나 응답 레코드를 반환하는 Ling 함수입니다.
4. 그 밖의 요청은 `404`를 받고, 본문은 `not found`입니다.

1과 2는 비동기 런타임에서 돌고 인터프리터를 건드리지 않으므로, 큰 파일(동영상, PDF,
zip)도 동시에 내보낼 수 있고 HTTP 범위 요청도 지원합니다. Ling 핸들러는 `http_serve`에
`workers`를 넘기지 않는 한 인터프리터에서 하나씩 실행됩니다.

`http_serve`를 호출하는 프로그램은 평범한 `ling run`으로 실행해도 항상 인터프리터에서
돕니다. 핸들러가 인수로 넘기는 함수이기 때문입니다.

## 요약

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `http_route(method, path, handler)` | — | — | — | `เว็บเส้นทาง` | 메서드 하나와 정확히 일치하는 경로를 처리합니다. |
| `http_static(prefix, dir)` | — | — | — | `เว็บสแตติก` | URL 접두사에서 폴더를 바이트 그대로 제공합니다. |
| `http_vhost(host, target)` | — | — | — | — | 호스트 이름의 루트에서 폴더를 제공하거나, 그 호스트 이름을 리디렉션합니다. |
| `http_serve(host, port, workers)` | — | — | — | `เว็บเสิร์ฟ` | HTTP를 시작하고 요청 루프를 돕니다. 블로킹됩니다. |
| `http_serve_tls(host, port, cert, key, workers)` | — | — | — | — | PEM 인증서와 키 파일로 HTTPS를 제공합니다. |
| `http_serve_dev_tls(host, port, workers)` | — | — | — | — | 일회용 자체 서명 인증서로 HTTPS를 제공합니다. |
| `http_get(url)` | — | — | — | `เว็บรับ` | URL을 가져오고 본문을 기다립니다. |
| `http_get_status()` | — | — | — | — | 마지막 `http_get`의 상태 코드. |
| `http_get_all(urls)` | — | — | — | — | 여러 URL을 동시에 가져옵니다. |
| `http_get_async(url)` | — | — | — | — | 백그라운드에서 GET을 시작하고 작업 id를 반환합니다. |
| `http_post_async(url, body, type)` | — | — | — | `เว็บโพสต์ไม่บล็อก` | 백그라운드에서 POST를 시작하고 작업 id를 반환합니다. |
| `http_job_poll(id)` | — | — | — | `เว็บงานสำรวจ` | 작업이 도는 동안 `""`, 끝나면 결과. |
| `query_param(qs, name, default)` | — | — | — | `พารามิเตอร์` | `a=1&b=2`에서 값 하나를 디코딩해 꺼냅니다. |
| `cookie_get(header, name, default)` | — | — | — | `รับคุกกี้` | `Cookie` 헤더에서 쿠키 하나를 꺼냅니다. |
| `html_escape(text)` | — | — | — | `กันเอชทีเอ็มแอล` | HTML용으로 `& < > " '`를 이스케이프합니다. |
| `json_escape(text)` | — | — | — | `หนีเจสัน` | JSON 문자열용으로 텍스트를 이스케이프합니다. |
| `url_decode(text)` | `网址解码` | — | — | — | `%XX`와 `+`를 디코딩합니다. |
| `cli_arg(name, default)` | — | — | — | `อาร์กิวเมนต์` | 명령줄의 `--name value`를 텍스트로 반환합니다. |
| `env_get(name, default)` | — | — | — | `รับตัวแปรแวดล้อม` | 환경 변수. |

## 서버 내장 함수

### `http_route(method, path, handler)`

`method`(대소문자 구분 없음, 기본값 `"GET"`)와 `path`(기본값 `"/"`)에 `handler`를
등록합니다. 경로는 **정확히 일치**해야 하고 쿼리 문자열은 빠지므로, 경로 매개변수는
없습니다. id는 쿼리에 넣고(`/note?id=2`) `query_param(req.query, "id", "")`로 읽으세요.
모든 라우트는 `http_serve`를 호출하기 전에 등록합니다.

핸들러가 오류를 내면 본문에 `handler error: ...`가 담긴 `500`으로 응답하는데, 내부 정보가
드러날 수 있습니다. 입력을 검증해서 오류가 밖으로 새지 않게 하세요.

### `http_static(prefix, dir)`

폴더 `dir`(기본값 `"static"`)을 `prefix`(기본값 `"/static"`)에서 제공합니다.
`http_static("/css", "css")`는 `/css/a.css`를 `css/a.css`에 대응시킵니다. 폴더를 요청하면
그 안의 `index.html`을 돌려줍니다. 라우트 핸들러는 텍스트만 반환할 수 있으므로, 바이너리는
모두 이것으로 제공하세요.

### `http_vhost(host, target)`

호스트 이름 전체에 따로 동작을 정해 줍니다. `target`은 폴더이거나 URL입니다.

- **폴더**: 그 호스트 이름을 폴더의 루트에서 `http_static`처럼 바이트 그대로 제공합니다.
  파일이 없으면 `404`로 응답하고, 폴더에 `404.html`이 있으면 그것을 씁니다(mdBook과
  대부분의 정적 사이트 생성기가 만들어 줍니다). 정적 사이트를 독자적인 서브도메인에 둘 때
  씁니다.
- **`http://` 또는 `https://` URL**: 그 호스트 이름으로 오는 모든 요청에
  `301 Moved Permanently`로 응답하고, 대상 뒤에 요청의 경로와 쿼리를 붙인 곳으로
  보냅니다. 옛 링크를 깨뜨리지 않고 서브도메인을 없앨 때 씁니다.

세부 사항:

- `host`는 `Host` 헤더와 대소문자 구분 없이 비교하며, 포트는 무시합니다.
- 가상 호스트는 모든 `http_static` 접두사와 라우트보다 먼저 검사하므로, 가상 호스트로 온
  요청은 핸들러에 닿지 않습니다.
- 가상 호스트는 여러 개 등록해도 됩니다. `Host` 하나는 많아야 하나와 일치합니다.

```ling-gui
바인드 시작 = 실행 {
  http_vhost("docs.example.org", "../docs/book")              // 서브도메인 루트의 정적 사이트
  http_vhost("old.example.org", "https://example.org/docs")   // old.example.org/a.html -> https://example.org/docs/a.html
  http_static("/docs", "../docs/book")                        // 같은 사이트를 메인 호스트의 /docs/에도
  http_serve("0.0.0.0", 8080)
}
```

ling-lang.org는 `site.ling`에서 이렇게 옛 문서 서브도메인을 없앴습니다.

```ling-module
함수 문서호스트등록(포트) {
  http_vhost("docs.ling-lang.org", "https://ling-lang.org/docs")
  http_vhost("docs.localhost", "http://localhost:" + str(포트) + "/docs")
  반환 참
}
```

DNS 없이 내 컴퓨터에서 가상 호스트를 시험하려면 헤더를 직접 보내세요.

```sh
curl -i -H "Host: docs.localhost" http://127.0.0.1:8080/
```

대부분의 브라우저도 `*.localhost`를 내 컴퓨터로 해석하므로 `http://docs.localhost:8080/`도
됩니다.

### `http_serve(host, port, workers)`

서버를 시작하고 프로그램이 멈출 때까지 요청에 응답합니다. 반환하지 않습니다.

- `host`: `"127.0.0.1"`(기본값)은 이 컴퓨터에서만, `"0.0.0.0"`은 네트워크에서 오는 연결도
  받습니다.
- `port`: 기본값 `8080`.
- `workers`: 기본값 `1`. 아래 "워커" 절을 보세요.

소켓이 바인딩되면 배너가 출력됩니다. 포트가 사용 중이면 오류는 표준 오류로 나가고
프로그램은 계속 기다리므로, 아무 응답이 없으면 로그를 확인하세요.

### `http_serve_tls(host, port, cert, key, workers)`

리버스 프록시 없이 바로 HTTPS를 제공합니다. `cert`와 `key`는 PEM 파일의 **경로**로,
예를 들면 Let's Encrypt의 `fullchain.pem`과 `privkey.pem`입니다. 키 파일은 서버 사용자만
읽을 수 있게 하세요. 나머지는 `http_serve`와 같습니다.

### `http_serve_dev_tls(host, port, workers)`

시작할 때 만든 자체 서명 인증서로 HTTPS를 제공합니다. 브라우저와 `curl`은 이를
거부합니다(내 컴퓨터에서는 `curl -k`). 로컬 개발 전용이며, 인터넷에 절대 노출하지 마세요.

## 요청

핸들러는 인수 하나를 받습니다. 다음 텍스트 필드를 가진 `Request` 레코드입니다.

| 필드 | 내용 |
|---|---|
| `method` | 대문자 메서드(`GET` 등). |
| `path` | 쿼리 문자열을 뺀 경로. |
| `query` | `?`를 뺀 원시 쿼리 문자열. `query_param`으로 읽습니다. |
| `body` | UTF-8 본문. 최대 32 MiB(멀티파트 업로드는 예외, 아래 참고). |
| `cookie` | 원시 `Cookie` 헤더, 또는 `""`. `cookie_get`으로 읽습니다. |
| `authorization` | 원시 `Authorization` 헤더, 또는 `""`. |
| `client_ip` | `X-Forwarded-For`의 첫 값, 없으면 `X-Real-IP`, 없으면 `""`. |

`client_ip`는 어떤 클라이언트든 설정할 수 있는 헤더에서 옵니다. 직접 운영하는 리버스
프록시 뒤에서만 믿으세요. 직접 연결에서는 비어 있습니다.

`multipart/form-data` 업로드는 파일을 `storage/tmp/` 아래로 스트리밍해 씁니다. 핸들러는
텍스트 필드와 `artifact_path`, `artifact_size`가 담긴 폼 인코딩 본문을 받으므로,
`query_param(req.body, ...)`로 읽으세요.

## 응답

**텍스트**를 반환하면 `text/html; charset=utf-8`의 `200`이 됩니다. 레코드가 아닌 다른
값은 텍스트로 바뀌어 `text/plain`으로 나갑니다.

그 밖의 응답에는 `status`, `content_type`, `body`, `set_cookie`, `location` 중 필요한
필드를 가진 레코드를 반환하세요. 빠진 필드의 기본값은 상태 `200`, 빈 본문, `text/html`
입니다. 레코드는 위치로 만들어지고, 의미 있는 것은 필드 이름입니다. 레코드 자체의
이름은 상관없습니다. 필드 이름은 영어 그대로 두세요.

```ling-gui
구조 응답 { status, content_type, body, set_cookie, location }

함수 상태API(요청) {
  반환 응답(200, "application/json", "{\"ok\":true}", "", "")
}

함수 옛페이지(요청) {
  반환 응답(301, "text/plain", "", "", "/new")
}

함수 로그인(요청) {
  바인드 쿠키 = "sid=" + random_hex(16) + "; HttpOnly; Secure; SameSite=Lax; Path=/"
  반환 응답(303, "text/plain", "", 쿠키, "/me")
}

바인드 시작 = 실행 {
  http_route("GET", "/api/status", 상태API)
  http_route("GET", "/old", 옛페이지)
  http_route("POST", "/login", 로그인)
  http_serve("127.0.0.1", 8080)
}
```

Ling 라우트에서 나오는 응답에는 `/embed` 아래 경로를 빼고 `X-Frame-Options: DENY`와
`Content-Security-Policy: frame-ancestors 'none'`이 붙습니다. 그 밖에는 아무것도 붙지
않습니다. 쿠키의 `Secure`, `HttpOnly`, `SameSite`는 직접 설정하고, 다른 보안 헤더(HSTS,
완전한 콘텐츠 보안 정책)는 리버스 프록시에서 추가하세요.

## 워커

기본적으로 인터프리터 하나가 모든 요청에 하나씩 응답합니다. 느린 핸들러가 있으면 다른
방문자는 모두 기다립니다.

`http_serve(host, port, N)`에서 `N`이 1보다 크면, 인터프리터 `N`개가 큐 하나에서 요청을
가져갑니다. ling-lang.org는 이 수를 명령줄(`--workers 4`)에서 읽습니다. 규칙은 다음과
같습니다.

- 핸들러는 **이름이 있는 최상위 함수**여야 합니다. 인라인 클로저를 핸들러로 쓰면 시작할
  때 오류가 납니다.
- `시작` 블록은 메인 인터프리터에서 **한 번만** 실행됩니다. 워커는 프로그램의 선언만
  보므로, 설정은 `시작`이 바꾸는 최상위 바인딩이 아니라 함수에 두세요.
- 워커마다 `db_open` 데이터베이스로 자기 연결을 엽니다.
  [데이터베이스 (SQLite)](../reference/database.md)를 보세요.
- 비동기 작업은 공유됩니다. 한 워커에서 시작한 작업을 다른 워커에서 폴링할 수 있습니다.

## 요청 도우미

이 함수들은 핸들러 안에서만이 아니라 어디서나 쓸 수 있으므로, 헤드리스로 시험해 볼 수
있습니다.

```ling
바인드 시작 = 실행 {
  바인드 쿼리 = "q=%EC%95%88%EB%85%95%ED%95%98%EC%84%B8%EC%9A%94&page=2&tag=x+y"
  출력(query_param(쿼리, "q", ""))
  출력(query_param(쿼리, "page", "1"))
  출력(query_param(쿼리, "tag", ""))
  출력(query_param(쿼리, "sort", "최신"))
  출력(cookie_get("sid=abc123; theme=dark", "theme", "light"))
  출력(html_escape("<b>에이다 & '밥'</b>"))
  출력("{\"이름\":\"" + json_escape("그가 \"안녕\"이라고 했다\n") + "\"}")
  출력(url_decode("a%20b+c%41"))
  출력(cli_arg("port", "8080"))
}
```

```output
안녕하세요
2
x y
최신
dark
&lt;b&gt;에이다 &amp; &#39;밥&#39;&lt;/b&gt;
{"이름":"그가 \"안녕\"이라고 했다\n"}
a b cA
8080
```

- `query_param(qs, name, default)`: `name`의 첫 값을 `%XX`와 `+`를 디코딩해 반환합니다.
  `req.query`와 폼 본문(`req.body`) 모두에 쓸 수 있습니다.
- `cookie_get(header, name, default)`: `req.cookie`에서 쿠키 하나를 디코딩 없이 반환합니다.
- `html_escape(text)`: HTML을 만드는 자리에서 믿을 수 없는 값을 모두 이스케이프하세요.
  요소 내용과 따옴표로 감싼 속성에는 충분하지만, `<script>` 안이나 따옴표 없는 속성에는
  충분하지 않습니다.
- `json_escape(text)`: `"`, `\`, 제어 문자를 이스케이프하며 양쪽 따옴표는 붙이지 않습니다.
  Ling은 JSON을 이어 붙여 만들므로 텍스트는 반드시 이것을 거치세요.
- `url_decode(text)`: `%XX`와 `+`를 디코딩합니다.
- `cli_arg(name, default)`: 명령줄의 `--name value`를 **텍스트**로 반환합니다.
  `to_number`로 바꾸세요. `cli_arg("port", "8080") + 0`은 `"80800"`이 됩니다.
- `env_get(name, default)`: 환경 변수. 비밀 값은 소스에 쓰지 말고 이렇게 읽으세요.

## HTTP 클라이언트

### `http_get(url)`과 `http_get_status()`

`http_get`은 `http://` 또는 `https://` URL을 가져오고 본문을 기다립니다. 오류 상태여도
본문을 반환합니다. `http_get_status()`는 마지막 `http_get`의 상태 코드이며, `0`은 응답이
전혀 없었다는 뜻(DNS, 연결, TLS 실패)이고 이때 본문은 `""`입니다. 본문을 믿기 전에 상태를
확인하세요.

`http_get`은 블로킹됩니다. 도구나 시작 단계에서는 괜찮지만, 핸들러 안에서는 모든 방문자를
멈추게 하므로 거기서는 비동기 버전을 쓰세요.

### `http_get_all(urls)`

URL 리스트를 동시에 가져오고 모두 끝날 때까지 기다립니다. 본문을 같은 순서로 반환하며,
실패나 HTTP 오류 상태는 `""`입니다.

### `http_get_async(url)`, `http_post_async(url, body, type)`, `http_job_poll(id)`

요청을 백그라운드에서 시작하고 바로 작업 id를 반환합니다. `type`은 POST 본문의
`Content-Type`입니다(기본값 `"application/json"`). 나중에 `http_job_poll(id)`를 호출하면
작업이 도는 동안은 `""`, 끝나면 응답 본문을 반환합니다. GET이 실패하면 `ERROR:`로
시작하는 텍스트가, POST가 실패하면 `{"error":"..."}`가 나옵니다.

```ling-gui
바인드 시작 = 실행 {
  바인드 작업 = http_get_async("https://ling-lang.org/api/books.json")
  바인드 본문 = ""
  바인드 횟수 = 0
  동안 본문 == "" && 횟수 < 100 {
    바인드 본문 = http_job_poll(작업)
    잠(50)
    바인드 횟수 = 횟수 + 1
  }
  출력(본문)
}
```

## 예제: 작은 사이트

```ling-gui
구조 응답 { status, content_type, body, set_cookie, location }

함수 첫페이지(요청) {
  반환 read_file("index.html")
}

함수 인사API(요청) {
  바인드 이름 = query_param(요청.query, "name", "세계")
  반환 응답(200, "application/json", "{\"hello\":\"" + json_escape(이름) + "\"}", "", "")
}

바인드 시작 = 실행 {
  http_static("/css", "css")
  http_static("/images", "images")
  http_route("GET", "/", 첫페이지)
  http_route("GET", "/api/hello", 인사API)
  http_serve(cli_arg("host", "127.0.0.1"), to_number(cli_arg("port", "8080")), 4)
}
```

`ling run site.ling --port 8080`으로 실행한 뒤 `http://127.0.0.1:8080/` 또는
`http://127.0.0.1:8080/api/hello?name=Ada`를 여세요.

## 함께 보기

- [데이터베이스 (SQLite)](../reference/database.md): `db_open`, `db_exec`, `db_query`.
- [작은 웹 서비스](../tutorials/web-service.md): 완전한 서비스를 단계별로 만듭니다.
- [네트워킹](../reference/networking.md): 로비 네트워킹, SSH, P2P.
- [ling-http와 ling-web](../crates/web.md): 같은 스택을 Rust에서 바로 씁니다.
