# 작은 웹 서비스

Ling에는 웹 서버가 내장되어 있습니다. 이 페이지에서는 페이지를 보내고, 브라우저가 보낸 내용을 읽고,
JSON으로 답하고, 메모를 SQLite 데이터베이스에 저장한 다음, 이것들을 합쳐 작은 메모 API를 만듭니다.
프레임워크도 추가 설치도 필요 없습니다.

서버를 시작하는 코드는 터미널에서 검증할 수 없으므로 그런 코드 블록에는 출력이 없습니다. 도우미 함수와
핸들러는 평범한 함수라서, 아래에서 보듯 서버 없이도 테스트할 수 있습니다.

웹 서버와 데이터베이스 내장 함수(`http_*`, `db_*`, `query_param` 등)에는 한국어 별칭이 없어서 한국어
코드에서도 영어 이름을 씁니다. 요청과 응답의 필드 이름(`query`, `body`, `status` 등)도 영어여야 합니다.
서버가 이름으로 읽기 때문입니다.

## 안녕, 웹

```ling-gui
함수 안녕(요청) {
    반환 "<h1>Ling에서 보내는 인사!</h1>"
}

바인드 시작 = 실행 {
    http_route("GET", "/", 안녕)
    http_serve("127.0.0.1", 8080)
}
```

`hello.ling`으로 저장해 실행하고 브라우저에서 <http://127.0.0.1:8080/>을 여세요.

```sh
ling run hello.ling
```

- `http_route(method, path, handler)`는 메서드 하나와 정확한 경로 하나에 **핸들러**를 등록합니다. 서버를
  시작하기 전에 모든 라우트를 등록하세요.
- `http_serve(host, port)`는 서버를 시작하고 Ctrl+C를 누를 때까지 요청에 계속 답합니다. `"127.0.0.1"`은
  내 컴퓨터만, `"0.0.0.0"`은 네트워크의 연결을 받습니다.
- 핸들러는 요청을 받아 문자열을 돌려줍니다. 문자열은 `200` HTML 페이지로 응답됩니다.
- 등록하지 않은 경로는 `404 not found`로 응답됩니다.

CSS, 이미지, 폴더 전체 같은 파일을 제공하려면 `http_static(prefix, dir)`을 추가합니다.
`http_static("/css", "css")`는 `/css/site.css`를 파일 `css/site.css`에 연결합니다. 정적 파일은 바이트
그대로 보내지며 핸들러를 거치지 않습니다.

## 브라우저가 보낸 것

핸들러의 인수는 `Request`이며 다음 텍스트 필드가 있습니다.

| 필드 | 내용 |
|---|---|
| `method` | 대문자 `"GET"`, `"POST"` 등 |
| `path` | 쿼리 문자열을 뺀 경로 |
| `query` | `?` 뒤의 원본 쿼리 문자열. 예: `name=Ada&page=2` |
| `body` | 텍스트로 된 요청 본문 |
| `cookie` | 원본 `Cookie` 헤더 |
| `authorization` | 원본 `Authorization` 헤더 |
| `client_ip` | 프록시 헤더에서 얻은 클라이언트 주소, 또는 `""` |

라우트는 경로와 정확히 일치해야 하므로 `/note/2` 같은 경로 매개변수는 없습니다. 대신 쿼리 문자열로 값을
넘기고(`/note?id=2`) `query_param(text, name, default)`으로 읽습니다. 값은 퍼센트 디코딩되며, 폼 본문도
같은 `a=1&b=2` 형식이라 그대로 쓸 수 있습니다.

```ling
바인드 시작 = 실행 {
    바인드 쿼리 = "name=%EA%B9%80%EB%AF%BC%EC%A4%80&lang=ko"
    출력(query_param(쿼리, "name", "이름없음"))
    출력(query_param(쿼리, "page", "1"))
}
```

```output
김민준
1
```

사용자가 입력한 텍스트를 HTML에 그대로 넣으면 안 됩니다. `html_escape`는 `& < > " '`를 엔티티로 바꿉니다.

```ling
바인드 시작 = 실행 {
    출력(html_escape("<b>톰 & 제리</b>"))
}
```

```output
&lt;b&gt;톰 &amp; 제리&lt;/b&gt;
```

그래서 인사 페이지는 이렇게 됩니다.

```ling-module
함수 인사(요청) {
    바인드 이름 = html_escape(query_param(요청.query, "name", "세계"))
    반환 "<h1>안녕하세요, " + 이름 + "님!</h1>"
}
```

## JSON으로 답하기

상태 코드나 콘텐츠 유형을 정하려면 문자열 대신 레코드를 돌려줍니다. `status`, `content_type`, `body`,
`set_cookie`, `location` 중에서 필드 이름을 골라 `form`(한국어로는 `구조`)을 선언합니다. 레코드는 위치로
만들어지고, 서버는 필드를 이름으로 읽습니다.

```ling-module
구조 Response { status, content_type, body }

함수 JSON응답(상태, 내용) {
    반환 Response(상태, "application/json", 내용)
}
```

Ling은 문자열을 이어 붙여 JSON을 만듭니다. 모든 텍스트를 `json_escape`에 통과시키세요. 따옴표, 역슬래시,
제어 문자를 이스케이프합니다.

```ling
바인드 시작 = 실행 {
    바인드 제목 = "\"안녕\"이라고 말하기"
    출력("{\"title\":\"" + json_escape(제목) + "\"}")
}
```

```output
{"title":"\"안녕\"이라고 말하기"}
```

리디렉션하려면 상태 `302`와 `location`을 돌려줍니다. 예를 들어 다섯 필드를 모두 선언한 레코드로
`Response(302, "text/plain", "", "", "/new")`를 돌려줍니다.

## SQLite를 쓰는 메모 API

이제 메모를 데이터베이스에 저장합니다. SQLite는 내장 함수 세 개로 다룹니다.

| 내장 함수 | 용도 |
|---|---|
| `db_open(path)` | 데이터베이스 파일을 열거나 만듦. `":memory:"`도 가능 |
| `db_exec(sql, params...)` | 데이터를 바꾸는 문을 실행하고 바뀐 행 수를 돌려줌 |
| `db_query(sql, params...)` | 쿼리를 실행하고 행 리스트를 돌려줌. `row.column`으로 읽음 |

매개변수는 `?1`, `?2`처럼 위치로 묶입니다. 사용자 입력은 항상 매개변수로 넘기고, SQL 텍스트에 이어 붙이면
안 됩니다.

핸들러는 그냥 함수이므로 요청을 직접 만들면 서버 없이 시험해 볼 수 있습니다. 여기서 `Request`는 핸들러가
쓰는 필드만 담은 작은 `구조`입니다.

```ling
구조 Response { status, content_type, body }
구조 Request { method, path, query, body }

함수 JSON응답(상태, 내용) {
    반환 Response(상태, "application/json", 내용)
}

함수 메모JSON(행) {
    반환 "{\"id\":" + 행.id + ",\"text\":\"" + json_escape(행.text) + "\"}"
}

함수 메모목록(요청) {
    바인드 행들 = db_query("SELECT id, text FROM notes ORDER BY id")
    바인드 결과 = "["
    바인드 i = 0
    동안 i < 길이(행들) {
        만약 i > 0 {
            바인드 결과 = 결과 + ","
        }
        바인드 결과 = 결과 + 메모JSON(행들[i])
        바인드 i = i + 1
    }
    반환 JSON응답(200, 결과 + "]")
}

함수 메모추가(요청) {
    바인드 본문 = query_param(요청.body, "text", "")
    만약 본문 == "" {
        반환 JSON응답(400, "{\"error\":\"text가 필요합니다\"}")
    }
    db_exec("INSERT INTO notes(text) VALUES (?1)", 본문)
    바인드 행들 = db_query("SELECT id, text FROM notes ORDER BY id DESC LIMIT 1")
    반환 JSON응답(201, 메모JSON(행들[0]))
}

함수 준비() {
    db_open(":memory:")
    db_exec("CREATE TABLE notes(id INTEGER PRIMARY KEY AUTOINCREMENT, text TEXT NOT NULL)")
}

바인드 시작 = 실행 {
    준비()
    바인드 r1 = 메모추가(Request("POST", "/notes", "", "text=우유+사기"))
    출력(r1.status, " ", r1.body)
    바인드 r2 = 메모추가(Request("POST", "/notes", "", "text=%22안녕%22이라고+말하기"))
    출력(r2.status, " ", r2.body)
    바인드 r3 = 메모추가(Request("POST", "/notes", "", ""))
    출력(r3.status, " ", r3.body)
    바인드 r4 = 메모목록(Request("GET", "/notes", "", ""))
    출력(r4.status, " ", r4.body)
}
```

```output
201 {"id":1,"text":"우유 사기"}
201 {"id":2,"text":"\"안녕\"이라고 말하기"}
400 {"error":"text가 필요합니다"}
200 [{"id":1,"text":"우유 사기"},{"id":2,"text":"\"안녕\"이라고 말하기"}]
```

리스트는 `동안` 루프와 인덱스로 만듭니다. `위해` 본문에서는 루프 밖의 바인딩을 바꿀 수 없기 때문입니다.

### 서비스하기

핸들러와 `준비`는 위와 똑같이 두고, 테스트용 `Request` 레코드를 빼고, `시작` 블록을 라우트로 바꿉니다.

```ling-gui
구조 Response { status, content_type, body }

함수 JSON응답(상태, 내용) {
    반환 Response(상태, "application/json", 내용)
}

함수 메모JSON(행) {
    반환 "{\"id\":" + 행.id + ",\"text\":\"" + json_escape(행.text) + "\"}"
}

함수 메모목록(요청) {
    바인드 행들 = db_query("SELECT id, text FROM notes ORDER BY id")
    바인드 결과 = "["
    바인드 i = 0
    동안 i < 길이(행들) {
        만약 i > 0 {
            바인드 결과 = 결과 + ","
        }
        바인드 결과 = 결과 + 메모JSON(행들[i])
        바인드 i = i + 1
    }
    반환 JSON응답(200, 결과 + "]")
}

함수 메모추가(요청) {
    바인드 본문 = query_param(요청.body, "text", "")
    만약 본문 == "" {
        반환 JSON응답(400, "{\"error\":\"text가 필요합니다\"}")
    }
    db_exec("INSERT INTO notes(text) VALUES (?1)", 본문)
    바인드 행들 = db_query("SELECT id, text FROM notes ORDER BY id DESC LIMIT 1")
    반환 JSON응답(201, 메모JSON(행들[0]))
}

함수 준비() {
    db_open("notes.db")
    db_exec("CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY AUTOINCREMENT, text TEXT NOT NULL)")
}

함수 인사(요청) {
    바인드 이름 = html_escape(query_param(요청.query, "name", "세계"))
    반환 "<h1>안녕하세요, " + 이름 + "님!</h1>"
}

바인드 시작 = 실행 {
    준비()
    http_route("GET", "/", 인사)
    http_route("GET", "/notes", 메모목록)
    http_route("POST", "/notes", 메모추가)
    http_serve("127.0.0.1", to_number(cli_arg("port", "8080")))
}
```

이 버전은 프로그램 옆의 파일 `notes.db`를 열기 때문에 다시 시작해도 메모가 남습니다.
`cli_arg("port", "8080")`은 명령줄의 `--port`를 텍스트로 읽고, `to_number`가 숫자로 바꿉니다.

```sh
ling run notes.ling --port 8080
```

다른 터미널에서 시험해 보세요.

```text
$ curl -X POST -d "text=Buy milk" http://127.0.0.1:8080/notes
{"id":1,"text":"Buy milk"}
$ curl http://127.0.0.1:8080/notes
[{"id":1,"text":"Buy milk"}]
$ curl "http://127.0.0.1:8080/?name=%3Cb%3EAda"
<h1>안녕하세요, &lt;b&gt;Ada님!</h1>
```

## 더 많은 방문자를 동시에: 워커

기본적으로 인터프리터 하나가 요청을 하나씩 처리하므로, 느린 핸들러가 있으면 다른 요청이 기다립니다.
`http_static`의 정적 파일은 따로 제공되고 병렬로 전송되므로 영향을 받지 않습니다. 핸들러를 병렬로
실행하려면 워커 수를 `http_serve`의 세 번째 인수로 넘깁니다. 여기서는 명령줄에서 읽습니다.

```ling-gui
함수 안녕(요청) {
    반환 "<h1>Ling에서 보내는 인사!</h1>"
}

함수 워커수() {
    반환 to_number(cli_arg("workers", "1"))
}

바인드 시작 = 실행 {
    http_route("GET", "/", 안녕)
    http_serve("127.0.0.1", 8080, 워커수())
}
```

```sh
ling run hello.ling --workers 4
```

워커가 둘 이상이면 추가 인터프리터마다 프로그램을 새로 불러오고, `시작` 블록은 주 인터프리터에서 한 번만
실행됩니다. 그래서:

- 핸들러는 **이름이 있는 최상위 함수**여야 합니다. 이 페이지의 핸들러는 모두 그렇습니다.
- 설정은 (위의 `워커수()`처럼) 함수 안에 두고, 최상위 `바인드`에 두지 마세요.
- 각 워커는 `db_open`으로 연 데이터베이스를 다시 엽니다. `":memory:"`가 아니라 파일을 쓰세요. 그렇지
  않으면 워커마다 빈 데이터베이스를 따로 갖게 됩니다. 두 워커가 동시에 쓰면 SQLite가 쓰기를 차례로
  기다리게 합니다.

## ling-lang.org도 이걸로 돌아갑니다

ling-lang.org 웹사이트는 같은 내장 함수로 만든 Ling 프로그램 하나, `site.ling`이 제공합니다. `http_static`
으로 CSS, 이미지, 동영상, Ling 북 PDF, 이 문서의 정적 폴더를 등록하고, `http_route`로 첫 페이지와 JSON
엔드포인트 몇 개에 답하며, 워커와 함께 시작합니다.

```sh
ling run site.ling --host 0.0.0.0 --port 80 --workers 4
```

`http_vhost(host, dir)`도 씁니다. 이 함수는 다른 호스트 이름의 루트에서 폴더를 제공하며, 요청의 `Host`
헤더로 맞춰 봅니다. `dir`이 폴더가 아니라 `http://`나 `https://` 주소이면 그 호스트는 그곳으로
리디렉션합니다. 예전 `docs.ling-lang.org`는 이렇게 방문자를 `ling-lang.org/docs`로 보냅니다.

```ling-module
함수 문서호스트등록() {
    http_vhost("docs.ling-lang.org", "https://ling-lang.org/docs")
    반환 참
}
```

## 직접 해 보기

1. 메모 하나를 돌려주는 `GET /note?id=2`를 추가하세요. `db_query`가 빈 리스트를 돌려주면 상태 `404`와
   `{"error":"not found"}`로 답합니다.
2. `db_exec("DELETE FROM notes WHERE id = ?1", id)`를 쓰는 `POST /delete?id=2`를 추가하세요. `db_exec`가
   돌려주는 수를 보고 아무것도 지워지지 않았으면 `404`로 답합니다.
3. `http_static`으로 `index.html`이 있는 `public/` 폴더를 `/app`에서 제공하세요.
4. 인사 페이지가 `visits` 테이블에 방문 횟수를 세고 합계를 보여 주게 하세요.

서버 내장 함수 전체는 [웹 서버와 HTTP](../reference/web.md)와 [데이터베이스 (SQLite)](../reference/database.md)를
참고하세요.
