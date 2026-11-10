# 데이터베이스 (SQLite)

Ling에는 SQLite가 내장되어 있습니다. 내장 함수 세 개면 모든 일을 할 수 있습니다.
`db_open`은 데이터베이스 파일을 열고, `db_exec`는 데이터나 스키마를 바꾸고,
`db_query`는 행을 레코드로 읽어 옵니다. SQLite는 `ling`에 함께 들어 있으므로 설치할
것도, 띄울 서버도 없습니다. 데이터베이스는 프로그램 옆에 놓인 평범한 파일 하나입니다.

이 내장 함수들은 `web` 설치 프로필(그리고 기본값인 `full`)에 들어 있습니다. 보통
[웹 서버](../reference/web.md)와 함께 쓰기 때문입니다. 명령줄 도구에서도 똑같이 잘
동작합니다.

## 요약

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `db_open(path)` | — | — | — | `ฐานข้อมูลเปิด` | 데이터베이스를 열거나 만들고 현재 연결로 삼습니다. |
| `db_exec(sql, params...)` | — | — | — | `ฐานข้อมูลรัน` | 데이터나 스키마를 바꾸는 문장 하나를 실행하고, 영향받은 행 수를 반환합니다. |
| `db_query(sql, params...)` | — | — | — | `ฐานข้อมูลถาม` | 쿼리를 실행하고 `Row` 레코드의 리스트를 반환합니다. |

이 세 함수에는 한국어 별칭이 없으므로, 한국어 프로그램에서도 영어 이름으로 씁니다.

## 첫 데이터베이스

```ling
바인드 시작 = 실행 {
  db_open(":memory:")
  db_exec("CREATE TABLE 메모(번호 INTEGER PRIMARY KEY AUTOINCREMENT, 제목 TEXT NOT NULL, 별점 REAL, 비고 TEXT)")
  db_exec("INSERT INTO 메모(제목, 별점) VALUES (?1, ?2)", "첫 번째", 4.5)
  db_exec("INSERT INTO 메모(제목, 별점) VALUES (?1, ?2)", "안전해요", 3)
  바인드 행목록 = db_query("SELECT 번호, 제목, 별점, 비고 FROM 메모 ORDER BY 번호")
  출력(길이(행목록))
  출력(행목록[0])
  위해 행 안에 행목록 {
    출력(str(행.번호) + ": " + 행.제목)
  }
  출력("[" + 행목록[1].비고 + "]")
}
```

```output
2
Row { 번호: 1, 제목: 첫 번째, 별점: 4.5, 비고:  }
1: 첫 번째
2: 안전해요
[]
```

SQLite는 테이블 이름과 열 이름에 한글을 허용하므로, 레코드의 필드 이름도 한글로 쓸 수
있습니다. `":memory:"`는 프로그램이 실행되는 동안에만 존재하는 데이터베이스를 만들어서
테스트와 예제에 편리합니다. 데이터를 남기려면 `"app.db"` 같은 파일 이름을 쓰세요.

## `db_open(path)`

`path`(기본값 `"app.db"`)에 있는 SQLite 데이터베이스를 열고(파일이 없으면 만들고),
인터프리터의 현재 연결로 삼습니다. 반환값은 없습니다.

- 열 때 Ling은 외래 키(`PRAGMA foreign_keys = ON`), 미리 쓰기 로그(`journal_mode = WAL`),
  5초 바쁨 대기 시간을 켭니다. 그래서 쓰기가 다른 쓰기와 부딪혀도 "database is locked"로
  실패하지 않고 차례를 기다립니다.
- 인터프리터 하나는 연결을 **하나**만 가집니다. `db_open`을 다시 호출하면 교체됩니다.
- WAL에서는 열려 있는 동안 SQLite가 데이터베이스 옆에 보조 파일 두 개(`app.db-wal`,
  `app.db-shm`)를 둡니다. 세 파일을 함께 백업하거나, 아무 프로그램도 열지 않았을 때
  복사하세요.
- 파일을 열 수 없으면 `db_open`은 오류를 냅니다.

## `db_exec(sql, params...)`

행을 반환하지 않는 SQL 문장을 **하나** 실행합니다. `CREATE`, `INSERT`, `UPDATE`,
`DELETE`, `BEGIN`, `COMMIT` 등입니다. 바뀐 행 수를 반환합니다(스키마 문장은 `0`).

- 한 번 호출할 때 문장은 하나만 넣으세요. `;`로 구분한 두 문장은 오류입니다.
- 실패하면 SQL 본문이 들어간 오류를 냅니다. `db_open` 전에 호출해도 오류입니다.

## `db_query(sql, params...)`

쿼리를 실행하고 **모든** 행을 `Row` 레코드의 리스트로 반환합니다. 열은 점으로
읽습니다(`행.제목`). 필드 이름은 `SELECT`의 열 이름이므로, 계산한 열에는 `AS`로
이름을 붙이세요.

```ling
바인드 시작 = 실행 {
  db_open(":memory:")
  db_exec("CREATE TABLE 로그(수 INTEGER)")
  db_exec("BEGIN")
  바인드 i = 0
  동안 i < 1000 {
    db_exec("INSERT INTO 로그(수) VALUES (?1)", i)
    바인드 i = i + 1
  }
  db_exec("COMMIT")
  바인드 결과 = db_query("SELECT count(*) AS 개수, sum(수) AS 합계, avg(수) AS 평균 FROM 로그")[0]
  출력(결과.개수)
  출력(결과.합계)
  출력(결과.평균)
}
```

```output
1000
499500
499.5
```

많은 삽입을 `BEGIN`과 `COMMIT`으로 감싸면 하나의 트랜잭션이 되어, 한 행씩 커밋하는
것보다 훨씬 빠릅니다.

### 값이 돌아오는 방식

| SQLite 값 | Ling 값 |
|---|---|
| `INTEGER` | 숫자 |
| `REAL` | 숫자 |
| `TEXT` | 텍스트 |
| `NULL` | `""`(빈 텍스트) |
| `BLOB` | base64 텍스트 |

일치하는 행이 없는 쿼리는 빈 리스트를 반환하므로, `행목록[0]`을 읽기 전에
`길이(행목록)`를 확인하세요.

## 매개변수

매개변수는 SQL 뒤에 넘긴 순서대로 **위치에 따라** `?1`, `?2`, `?3` …에 바인딩됩니다.
사용자 입력은 반드시 매개변수로 넘기고, 절대로 SQL 본문에 이어 붙이지 마세요.
매개변수는 언제나 값일 뿐이므로 문장 자체를 바꿀 수 없습니다.

```ling
바인드 시작 = 실행 {
  db_open(":memory:")
  db_exec("CREATE TABLE 사용자(번호 INTEGER PRIMARY KEY, 이름 TEXT UNIQUE, 관리자 INTEGER)")
  바인드 이름 = "Robert'); DROP TABLE 사용자; --"
  db_exec("INSERT INTO 사용자(이름, 관리자) VALUES (?1, ?2)", 이름, 거짓)
  db_exec("INSERT INTO 사용자(이름, 관리자) VALUES (?1, ?2)", "에이다", 참)
  출력(db_query("SELECT count(*) AS 인원 FROM 사용자")[0].인원)
  출력(db_query("SELECT 이름 FROM 사용자 WHERE 관리자 = ?1", 참)[0].이름)
  출력(db_query("SELECT last_insert_rowid() AS 번호")[0].번호)
  출력(db_exec("UPDATE 사용자 SET 관리자 = 0 WHERE 이름 = ?1", "에이다"))
  출력(db_exec("DELETE FROM 사용자 WHERE 번호 > ?1", 100))
}
```

```output
2
에이다
2
1
0
```

Ling 값이 바인딩되는 방식:

| Ling 값 | 바인딩 형태 |
|---|---|
| 정수(`3`) | `INTEGER` |
| 소수(`4.5`) | `REAL` |
| `참` / `거짓` | `1` / `0` |
| 그 밖의 값 | `TEXT`(값을 텍스트로 바꾼 것) |

방금 삽입한 행의 id는 위처럼 같은 연결에서 `SELECT last_insert_rowid() AS 번호`를
조회해서 얻습니다.

## 오류

문장이 실패하면(구문 오류, 없는 테이블, `UNIQUE` 제약 위반 등) `db_exec:` 또는
`db_query:`로 시작하고 SQL을 보여 주는 런타임 오류가 납니다.

인터프리터(`ling run --interp`, 그리고 `http_serve`를 호출하는 프로그램은 항상
여기서 실행)에서는 오류가 프로그램을 멈추거나, `시도` 안에서는 `나쁨`으로 돌아옵니다.
`ling run`의 기본 JIT 백엔드에서는 실패한 문장이 현재 아무 메시지 없이 건너뛰어지므로,
데이터베이스 스크립트를 개발할 때는 `--interp`를 붙여 실행하세요.

```ling-module
함수 사용자추가(이름) {
  반환 db_exec("INSERT INTO 사용자(이름) VALUES (?1)", 이름)
}

함수 추가시도(이름) {
  매치 시도(| | 사용자추가(이름)) {
    좋아(n) => 반환 "추가했습니다"
    나쁨(e) => 반환 이름 + "을(를) 추가할 수 없습니다: " + e
  }
}
```

## 웹 서버와 함께 쓰기

흔한 구조는 `시작`에서 데이터베이스를 열고, 필요하면 테이블을 만들고, 라우트를
등록한 다음 서버를 띄우는 것입니다. 핸들러는 `db_query`와 `db_exec`를 바로 호출합니다.

```ling-gui
구조 응답 { status, content_type, body, set_cookie, location }

함수 메모목록(요청) {
  바인드 행목록 = db_query("SELECT 번호, 제목 FROM 메모 ORDER BY 번호 DESC LIMIT 50")
  바인드 출력문 = "<ul>"
  위해 행 안에 행목록 {
    바인드 출력문 = 출력문 + "<li>" + html_escape(행.제목) + "</li>"
  }
  반환 출력문 + "</ul>"
}

함수 메모추가(요청) {
  바인드 제목 = query_param(요청.body, "title", "")
  만약 제목 == "" {
    반환 응답(400, "text/plain; charset=utf-8", "제목이 필요합니다", "", "")
  }
  db_exec("INSERT INTO 메모(제목) VALUES (?1)", 제목)
  반환 응답(303, "text/plain", "", "", "/")
}

바인드 시작 = 실행 {
  db_open("notes.db")
  db_exec("CREATE TABLE IF NOT EXISTS 메모(번호 INTEGER PRIMARY KEY AUTOINCREMENT, 제목 TEXT NOT NULL)")
  http_route("GET", "/", 메모목록)
  http_route("POST", "/add", 메모추가)
  http_serve("127.0.0.1", 8080, 4)
}
```

응답 레코드의 필드 이름(`status`, `content_type`, `body`, `set_cookie`, `location`)과
요청의 필드 이름(`body`, `query` 등)은 영어 그대로 두어야 합니다. 서버가 그 이름으로
알아보기 때문입니다. 레코드 자체의 이름(여기서는 `응답`)은 자유롭게 정해도 됩니다.

### 워커와 연결

`http_serve(host, port, 1)`(기본값)에서는 인터프리터 하나가 연결 하나로 모든 요청에
하나씩 답합니다.

`workers`가 1보다 크면, 추가 워커는 각각 독립된 인터프리터가 되어 `db_open`에 넘긴
경로로 **자기만의 연결**을 엽니다. 읽기는 나란히 실행됩니다. 쓰기는 여전히 한 번에
하나씩이며, WAL과 바쁨 대기 시간 덕분에 실패하지 않고 줄을 섭니다. 여기서 두 가지가
따라옵니다.

- `db_open`은 `시작`에서 `http_serve`보다 **먼저** 호출하세요. 워커는 서버가 시작할 때
  현재였던 경로를 엽니다.
- 워커를 쓸 때는 `":memory:"`를 쓰지 마세요. 워커마다 따로 떨어진 빈 메모리
  데이터베이스가 생깁니다.

핸들러 안의 트랜잭션은 짧게 유지하세요. 같은 핸들러에서 `COMMIT` 없는 `BEGIN`은
쓰기 잠금을 계속 쥐고 있어서 다른 모든 쓰기를 기다리게 합니다.

## 함께 보기

- [웹 서버와 HTTP](../reference/web.md): 라우트, 요청, 응답, 워커.
- [작은 웹 서비스](../tutorials/web-service.md): 완전한 서비스를 단계별로 만듭니다.
- [ling-http와 ling-web](../crates/web.md): 같은 SQLite 계층을 Rust에서 씁니다.
