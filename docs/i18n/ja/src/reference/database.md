# データベース（SQLite）

Ling には SQLite が組み込まれています。必要なのは 3 つの組み込み関数だけです。
`db_open` でデータベースファイルを開き、`db_exec` でデータやスキーマを変更し、
`db_query` で行をレコードとして読み出します。SQLite は `ling` に同梱されているので、
インストールするものも、動かすサーバーもありません。データベースはプログラムの隣に
置かれた普通のファイル 1 つです。

これらの組み込み関数は `web` インストールプロファイル（と既定の `full`）に含まれます。
ふつうは [Web サーバー](../reference/web.md)と組み合わせて使うからです。
コマンドラインツールでも同じように使えます。

## 一覧

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `db_open(path)` | — | — | — | `ฐานข้อมูลเปิด` | データベースを開くか作成し、現在の接続にします。 |
| `db_exec(sql, params...)` | — | — | — | `ฐานข้อมูลรัน` | データやスキーマを変更する文を 1 つ実行し、影響した行数を返します。 |
| `db_query(sql, params...)` | — | — | — | `ฐานข้อมูลถาม` | クエリを実行し、`Row` レコードのリストを返します。 |

この 3 つには日本語の別名がないので、日本語のプログラムでも英語名で書きます。

## はじめてのデータベース

```ling
束縛 開始 = 実行 {
  db_open(":memory:")
  db_exec("CREATE TABLE メモ(番号 INTEGER PRIMARY KEY AUTOINCREMENT, 題名 TEXT NOT NULL, 星 REAL, 備考 TEXT)")
  db_exec("INSERT INTO メモ(題名, 星) VALUES (?1, ?2)", "最初", 4.5)
  db_exec("INSERT INTO メモ(題名, 星) VALUES (?1, ?2)", "安全です", 3)
  束縛 行一覧 = db_query("SELECT 番号, 題名, 星, 備考 FROM メモ ORDER BY 番号")
  印刷(長さ(行一覧))
  印刷(行一覧[0])
  繰 行 の中 行一覧 {
    印刷(str(行.番号) + ": " + 行.題名)
  }
  印刷("[" + 行一覧[1].備考 + "]")
}
```

```output
2
Row { 番号: 1, 題名: 最初, 星: 4.5, 備考:  }
1: 最初
2: 安全です
[]
```

SQLite ではテーブル名や列名に日本語を使えるので、レコードのフィールド名も日本語に
できます。`":memory:"` はプログラムの実行中だけ存在するデータベースを作るので、
テストや例に便利です。データを残すには `"app.db"` のようなファイル名を使います。

## `db_open(path)`

`path`（既定値 `"app.db"`）の SQLite データベースを開き（ファイルがなければ作成し）、
インタプリタの現在の接続にします。戻り値はありません。

- 開くときに Ling は外部キー（`PRAGMA foreign_keys = ON`）、ログ先行書き込み
  （`journal_mode = WAL`）、5 秒のビジータイムアウトを有効にします。そのため書き込みが
  別の書き込みとぶつかっても、"database is locked" で失敗せずに順番を待ちます。
- 1 つのインタプリタが持つ接続は **1 つ**です。もう一度 `db_open` を呼ぶと置き換わります。
- WAL では、開いている間 SQLite がデータベースの隣に補助ファイルを 2 つ置きます
  （`app.db-wal` と `app.db-shm`）。3 つまとめてバックアップするか、どのプログラムも
  開いていないときにコピーしてください。
- ファイルを開けないと `db_open` はエラーになります。

## `db_exec(sql, params...)`

行を返さない SQL 文を **1 つ**実行します。`CREATE`、`INSERT`、`UPDATE`、`DELETE`、
`BEGIN`、`COMMIT` などです。変更した行数を返します（スキーマの文では `0`）。

- 1 回の呼び出しには文を 1 つだけ入れてください。`;` で区切った 2 つの文はエラーです。
- 失敗すると、SQL の本文を含むエラーになります。`db_open` より前に呼んでもエラーです。

## `db_query(sql, params...)`

クエリを実行し、**すべての**行を `Row` レコードのリストで返します。列はドットで
読みます（`行.題名`）。フィールド名は `SELECT` の列名なので、計算した列には `AS` で
名前を付けてください。

```ling
束縛 開始 = 実行 {
  db_open(":memory:")
  db_exec("CREATE TABLE ログ(数 INTEGER)")
  db_exec("BEGIN")
  束縛 i = 0
  間 i < 1000 {
    db_exec("INSERT INTO ログ(数) VALUES (?1)", i)
    束縛 i = i + 1
  }
  db_exec("COMMIT")
  束縛 結果 = db_query("SELECT count(*) AS 件数, sum(数) AS 合計, avg(数) AS 平均 FROM ログ")[0]
  印刷(結果.件数)
  印刷(結果.合計)
  印刷(結果.平均)
}
```

```output
1000
499500
499.5
```

たくさんの挿入を `BEGIN` と `COMMIT` で囲むと 1 つのトランザクションになり、
1 行ずつコミットするよりずっと速くなります。

### 値の戻り方

| SQLite の値 | Ling の値 |
|---|---|
| `INTEGER` | 数値 |
| `REAL` | 数値 |
| `TEXT` | テキスト |
| `NULL` | `""`（空のテキスト） |
| `BLOB` | base64 のテキスト |

一致がないクエリは空のリストを返すので、`行一覧[0]` を読む前に `長さ(行一覧)` を
確かめてください。

## パラメータ

パラメータは SQL の後に渡した順に、**位置で** `?1`、`?2`、`?3` … に結び付きます。
ユーザーの入力は必ずパラメータとして渡し、SQL の本文につなげてはいけません。
パラメータは常にただの値なので、文そのものを変えることはできません。

```ling
束縛 開始 = 実行 {
  db_open(":memory:")
  db_exec("CREATE TABLE ユーザー(番号 INTEGER PRIMARY KEY, 名前 TEXT UNIQUE, 管理者 INTEGER)")
  束縛 名前 = "Robert'); DROP TABLE ユーザー; --"
  db_exec("INSERT INTO ユーザー(名前, 管理者) VALUES (?1, ?2)", 名前, 偽)
  db_exec("INSERT INTO ユーザー(名前, 管理者) VALUES (?1, ?2)", "エイダ", 真)
  印刷(db_query("SELECT count(*) AS 人数 FROM ユーザー")[0].人数)
  印刷(db_query("SELECT 名前 FROM ユーザー WHERE 管理者 = ?1", 真)[0].名前)
  印刷(db_query("SELECT last_insert_rowid() AS 番号")[0].番号)
  印刷(db_exec("UPDATE ユーザー SET 管理者 = 0 WHERE 名前 = ?1", "エイダ"))
  印刷(db_exec("DELETE FROM ユーザー WHERE 番号 > ?1", 100))
}
```

```output
2
エイダ
2
1
0
```

Ling の値の結び付き方：

| Ling の値 | 結び付く型 |
|---|---|
| 整数（`3`） | `INTEGER` |
| 小数（`4.5`） | `REAL` |
| `真` / `偽` | `1` / `0` |
| それ以外 | `TEXT`（値をテキストにしたもの） |

挿入したばかりの行の id は、上の例のように同じ接続で
`SELECT last_insert_rowid() AS 番号` を問い合わせて取得します。

## エラー

文が失敗すると（構文エラー、存在しないテーブル、`UNIQUE` 制約違反など）、
`db_exec:` または `db_query:` で始まり SQL を含むメッセージの実行時エラーになります。

インタプリタ（`ling run --interp`、および `http_serve` を呼ぶプログラムは常にこちら）では、
エラーでプログラムが止まるか、`試す` の中なら `悪い` として返ります。`ling run` の既定の
JIT バックエンドでは、失敗した文は今のところ何のメッセージもなく飛ばされます。
データベースのスクリプトを開発するときは `--interp` を付けて実行してください。

```ling-module
関数 ユーザー追加(名前) {
  戻る db_exec("INSERT INTO ユーザー(名前) VALUES (?1)", 名前)
}

関数 追加を試す(名前) {
  一致 試す(| | ユーザー追加(名前)) {
    良し(n) => 戻る "追加しました"
    悪い(e) => 戻る 名前 + " を追加できません: " + e
  }
}
```

## Web サーバーと使う

よくある形は、`開始` でデータベースを開き、必要ならテーブルを作り、ルートを登録してから
サーバーを動かす、というものです。ハンドラは `db_query` と `db_exec` を直接呼びます。

```ling-gui
形 応答 { status, content_type, body, set_cookie, location }

関数 メモ一覧(要求) {
  束縛 行一覧 = db_query("SELECT 番号, 題名 FROM メモ ORDER BY 番号 DESC LIMIT 50")
  束縛 出力 = "<ul>"
  繰 行 の中 行一覧 {
    束縛 出力 = 出力 + "<li>" + html_escape(行.題名) + "</li>"
  }
  戻る 出力 + "</ul>"
}

関数 メモ追加(要求) {
  束縛 題名 = query_param(要求.body, "title", "")
  もし 題名 == "" {
    戻る 応答(400, "text/plain; charset=utf-8", "題名が必要です", "", "")
  }
  db_exec("INSERT INTO メモ(題名) VALUES (?1)", 題名)
  戻る 応答(303, "text/plain", "", "", "/")
}

束縛 開始 = 実行 {
  db_open("notes.db")
  db_exec("CREATE TABLE IF NOT EXISTS メモ(番号 INTEGER PRIMARY KEY AUTOINCREMENT, 題名 TEXT NOT NULL)")
  http_route("GET", "/", メモ一覧)
  http_route("POST", "/add", メモ追加)
  http_serve("127.0.0.1", 8080, 4)
}
```

応答レコードのフィールド名（`status`、`content_type`、`body`、`set_cookie`、`location`）と
リクエストのフィールド名（`body`、`query` など）は英語のままにしてください。サーバーは
その名前で見分けます。レコード自体の名前（ここでは `応答`）は自由です。

### ワーカーと接続

`http_serve(host, port, 1)`（既定値）では、1 つのインタプリタが 1 本の接続で
すべてのリクエストに 1 つずつ答えます。

`workers` が 1 より大きいと、追加のワーカーはそれぞれ独立したインタプリタになり、
`db_open` に渡したパスへの**自分専用の接続**を持ちます。読み込みは並行して進みます。
書き込みは今までどおり 1 つずつで、WAL とビジータイムアウトにより失敗せずに順番待ちに
なります。ここから 2 つのことが言えます。

- `db_open` は `開始` の中で、`http_serve` より**前に**呼んでください。ワーカーは
  サーバー起動時に現在だったパスを開きます。
- ワーカーを使うときは `":memory:"` を使わないでください。ワーカーごとに別々の、
  空のメモリ内データベースができてしまいます。

ハンドラ内のトランザクションは短くしてください。同じハンドラ内で `COMMIT` のない
`BEGIN` は書き込みロックを握ったままになり、ほかの書き込みをすべて待たせます。

## 関連項目

- [Web サーバーと HTTP](../reference/web.md)：ルート、リクエスト、レスポンス、ワーカー。
- [小さな Web サービス](../tutorials/web-service.md)：完全なサービスを順を追って作ります。
- [ling-http と ling-web](../crates/web.md)：同じ SQLite 層を Rust から使います。
