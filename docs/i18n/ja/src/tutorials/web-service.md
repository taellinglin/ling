# 小さな Web サービス

Ling には Web サーバーが組み込まれています。このページでは、ページを返し、ブラウザが送ってきた内容を
読み、JSON で答え、メモを SQLite データベースに保存し、それらを組み合わせて小さなメモ API を作ります。
フレームワークも追加のインストールも要りません。

サーバーを起動するコードは端末では検証できないので、それらのコードブロックに出力はありません。
補助関数やハンドラは普通の関数なので、以下で見るようにサーバーなしでテストできます。

Web サーバーとデータベースの組み込み関数（`http_*`、`db_*`、`query_param` など）には日本語の別名が
ないので、日本語のコードでも英語の名前を使います。リクエストとレスポンスのフィールド名（`query`、`body`、
`status` など）も英語でなければなりません。サーバーが名前で読み取るからです。

## こんにちは、Web

```ling-gui
関数 こんにちは(要求) {
    戻る "<h1>Ling からこんにちは！</h1>"
}

束縛 開始 = 実行 {
    http_route("GET", "/", こんにちは)
    http_serve("127.0.0.1", 8080)
}
```

`hello.ling` として保存して実行し、ブラウザで <http://127.0.0.1:8080/> を開きます。

```sh
ling run hello.ling
```

- `http_route(method, path, handler)` は、1 つのメソッドと 1 つの完全なパスに**ハンドラ**を登録します。
  サーバーを起動する前にすべてのルートを登録してください。
- `http_serve(host, port)` はサーバーを起動し、Ctrl+C を押すまでリクエストに答え続けます。
  `"127.0.0.1"` は自分のマシンだけ、`"0.0.0.0"` はネットワークからの接続を受け付けます。
- ハンドラはリクエストを受け取って文字列を返します。文字列は `200` の HTML ページとして返されます。
- 登録していないパスには `404 not found` が返ります。

CSS、画像、フォルダ全体のようなファイルを配信するには `http_static(prefix, dir)` を加えます。
`http_static("/css", "css")` は `/css/site.css` をファイル `css/site.css` に対応させます。静的ファイルは
バイト単位でそのまま送られ、ハンドラを通りません。

## ブラウザが送ってきたもの

ハンドラの引数は `Request` で、次のテキストフィールドがあります。

| フィールド | 内容 |
|---|---|
| `method` | 大文字の `"GET"`、`"POST"` など |
| `path` | クエリ文字列を除いたパス |
| `query` | `?` の後ろの生のクエリ文字列。例：`name=Ada&page=2` |
| `body` | テキストとしてのリクエスト本文 |
| `cookie` | 生の `Cookie` ヘッダ |
| `authorization` | 生の `Authorization` ヘッダ |
| `client_ip` | プロキシのヘッダから得たクライアントのアドレス、または `""` |

ルートはパスに完全一致するので、`/note/2` のようなパスパラメータはありません。代わりにクエリ文字列で
値を渡し（`/note?id=2`）、`query_param(text, name, default)` で読みます。値はパーセントデコードされます。
フォームの本文も同じ `a=1&b=2` 形式なので、そのまま使えます。

```ling
束縛 開始 = 実行 {
    束縛 クエリ = "name=%E5%B1%B1%E7%94%B0&lang=ja"
    印刷(query_param(クエリ, "name", "名無し"))
    印刷(query_param(クエリ, "page", "1"))
}
```

```output
山田
1
```

ユーザーのテキストを HTML にそのまま入れてはいけません。`html_escape` は `& < > " '` を実体参照に
変えます。

```ling
束縛 開始 = 実行 {
    印刷(html_escape("<b>トム & ジェリー</b>"))
}
```

```output
&lt;b&gt;トム &amp; ジェリー&lt;/b&gt;
```

あいさつのページはこうなります。

```ling-module
関数 あいさつ(要求) {
    束縛 名前 = html_escape(query_param(要求.query, "name", "世界"))
    戻る "<h1>こんにちは、" + 名前 + "さん！</h1>"
}
```

## JSON で答える

ステータスコードやコンテンツタイプを指定するには、文字列の代わりにレコードを返します。`status`、
`content_type`、`body`、`set_cookie`、`location` の中からフィールド名を選んで `form`（日本語では `形`）を
宣言します。レコードは位置で作られ、サーバーはフィールドを名前で読みます。

```ling-module
形 Response { status, content_type, body }

関数 JSON応答(状態, 内容) {
    戻る Response(状態, "application/json", 内容)
}
```

Ling は文字列をつなげて JSON を作ります。テキストはすべて `json_escape` に通してください。引用符、
バックスラッシュ、制御文字をエスケープします。

```ling
束縛 開始 = 実行 {
    束縛 題名 = "「こんにちは」と \"言う\""
    印刷("{\"title\":\"" + json_escape(題名) + "\"}")
}
```

```output
{"title":"「こんにちは」と \"言う\""}
```

リダイレクトするには、ステータス `302` と `location` を返します。たとえば 5 つのフィールドすべてを
宣言したレコードで `Response(302, "text/plain", "", "", "/new")` とします。

## SQLite を使ったメモ API

次はメモをデータベースに保存します。SQLite は 3 つの組み込み関数で扱えます。

| 組み込み関数 | 用途 |
|---|---|
| `db_open(path)` | データベースファイルを開く（なければ作る）。`":memory:"` も可 |
| `db_exec(sql, params...)` | データを変える文を実行し、変わった行数を返す |
| `db_query(sql, params...)` | 問い合わせを実行し、行のリストを返す。`row.column` で読む |

パラメータは `?1`、`?2` のように位置で結び付けます。ユーザーの入力は必ずパラメータとして渡し、SQL の
テキストにつなげてはいけません。

ハンドラはただの関数なので、リクエストを自分で作れば、サーバーなしで試せます。ここでの `Request` は、
ハンドラが使うフィールドだけを持つ小さな `形` です。

```ling
形 Response { status, content_type, body }
形 Request { method, path, query, body }

関数 JSON応答(状態, 内容) {
    戻る Response(状態, "application/json", 内容)
}

関数 メモJSON(行) {
    戻る "{\"id\":" + 行.id + ",\"text\":\"" + json_escape(行.text) + "\"}"
}

関数 メモ一覧(要求) {
    束縛 行たち = db_query("SELECT id, text FROM notes ORDER BY id")
    束縛 結果 = "["
    束縛 i = 0
    間 i < 長さ(行たち) {
        もし i > 0 {
            束縛 結果 = 結果 + ","
        }
        束縛 結果 = 結果 + メモJSON(行たち[i])
        束縛 i = i + 1
    }
    戻る JSON応答(200, 結果 + "]")
}

関数 メモ追加(要求) {
    束縛 本文 = query_param(要求.body, "text", "")
    もし 本文 == "" {
        戻る JSON応答(400, "{\"error\":\"text が必要です\"}")
    }
    db_exec("INSERT INTO notes(text) VALUES (?1)", 本文)
    束縛 行たち = db_query("SELECT id, text FROM notes ORDER BY id DESC LIMIT 1")
    戻る JSON応答(201, メモJSON(行たち[0]))
}

関数 準備() {
    db_open(":memory:")
    db_exec("CREATE TABLE notes(id INTEGER PRIMARY KEY AUTOINCREMENT, text TEXT NOT NULL)")
}

束縛 開始 = 実行 {
    準備()
    束縛 r1 = メモ追加(Request("POST", "/notes", "", "text=牛乳を買う"))
    印刷(r1.status, " ", r1.body)
    束縛 r2 = メモ追加(Request("POST", "/notes", "", "text=%22やあ%22と言う"))
    印刷(r2.status, " ", r2.body)
    束縛 r3 = メモ追加(Request("POST", "/notes", "", ""))
    印刷(r3.status, " ", r3.body)
    束縛 r4 = メモ一覧(Request("GET", "/notes", "", ""))
    印刷(r4.status, " ", r4.body)
}
```

```output
201 {"id":1,"text":"牛乳を買う"}
201 {"id":2,"text":"\"やあ\"と言う"}
400 {"error":"text が必要です"}
200 [{"id":1,"text":"牛乳を買う"},{"id":2,"text":"\"やあ\"と言う"}]
```

リストは `間` ループと添字で作っています。`繰` の本体からはループの外の束縛を更新できないからです。

### 配信する

ハンドラと `準備` は上とまったく同じままにして、テスト用の `Request` レコードを外し、`開始` ブロックを
ルートに置き換えます。

```ling-gui
形 Response { status, content_type, body }

関数 JSON応答(状態, 内容) {
    戻る Response(状態, "application/json", 内容)
}

関数 メモJSON(行) {
    戻る "{\"id\":" + 行.id + ",\"text\":\"" + json_escape(行.text) + "\"}"
}

関数 メモ一覧(要求) {
    束縛 行たち = db_query("SELECT id, text FROM notes ORDER BY id")
    束縛 結果 = "["
    束縛 i = 0
    間 i < 長さ(行たち) {
        もし i > 0 {
            束縛 結果 = 結果 + ","
        }
        束縛 結果 = 結果 + メモJSON(行たち[i])
        束縛 i = i + 1
    }
    戻る JSON応答(200, 結果 + "]")
}

関数 メモ追加(要求) {
    束縛 本文 = query_param(要求.body, "text", "")
    もし 本文 == "" {
        戻る JSON応答(400, "{\"error\":\"text が必要です\"}")
    }
    db_exec("INSERT INTO notes(text) VALUES (?1)", 本文)
    束縛 行たち = db_query("SELECT id, text FROM notes ORDER BY id DESC LIMIT 1")
    戻る JSON応答(201, メモJSON(行たち[0]))
}

関数 準備() {
    db_open("notes.db")
    db_exec("CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY AUTOINCREMENT, text TEXT NOT NULL)")
}

関数 あいさつ(要求) {
    束縛 名前 = html_escape(query_param(要求.query, "name", "世界"))
    戻る "<h1>こんにちは、" + 名前 + "さん！</h1>"
}

束縛 開始 = 実行 {
    準備()
    http_route("GET", "/", あいさつ)
    http_route("GET", "/notes", メモ一覧)
    http_route("POST", "/notes", メモ追加)
    http_serve("127.0.0.1", to_number(cli_arg("port", "8080")))
}
```

この版はプログラムの隣にあるファイル `notes.db` を開くので、再起動してもメモが残ります。
`cli_arg("port", "8080")` はコマンドラインの `--port` をテキストとして読み、`to_number` が数値に変えます。

```sh
ling run notes.ling --port 8080
```

別の端末から試してみましょう。

```text
$ curl -X POST -d "text=Buy milk" http://127.0.0.1:8080/notes
{"id":1,"text":"Buy milk"}
$ curl http://127.0.0.1:8080/notes
[{"id":1,"text":"Buy milk"}]
$ curl "http://127.0.0.1:8080/?name=%3Cb%3EAda"
<h1>こんにちは、&lt;b&gt;Adaさん！</h1>
```

## もっと多くの訪問者を同時に：ワーカー

既定では 1 つのインタプリタがリクエストを 1 つずつ処理するので、遅いハンドラがあるとほかのリクエストが
待たされます。`http_static` の静的ファイルは別に配信され、並行して送られるので影響しません。ハンドラを
並行して動かすには、ワーカーの数を `http_serve` の 3 番目の引数に渡します。ここではコマンドラインから
読みます。

```ling-gui
関数 こんにちは(要求) {
    戻る "<h1>Ling からこんにちは！</h1>"
}

関数 ワーカー数() {
    戻る to_number(cli_arg("workers", "1"))
}

束縛 開始 = 実行 {
    http_route("GET", "/", こんにちは)
    http_serve("127.0.0.1", 8080, ワーカー数())
}
```

```sh
ling run hello.ling --workers 4
```

ワーカーが 2 つ以上あると、追加のインタプリタはそれぞれプログラムを読み込み直し、`開始` ブロックは
メインのインタプリタで 1 回だけ実行されます。そのため：

- ハンドラは**名前の付いたトップレベルの関数**でなければなりません。このページのハンドラはすべてそうです。
- 設定は（上の `ワーカー数()` のように）関数の中に置き、トップレベルの `束縛` には置かないでください。
- 各ワーカーは `db_open` で開いたデータベースを開き直します。`":memory:"` ではなくファイルを使って
  ください。そうしないと各ワーカーが別々の空のデータベースを持つことになります。2 つのワーカーが同時に
  書き込むと、SQLite は書き込みを順番待ちさせます。

## ling-lang.org もこれで動いている

ling-lang.org のサイトは、同じ組み込み関数で書かれた 1 つの Ling プログラム `site.ling` が配信しています。
`http_static` で CSS、画像、動画、Ling ブックの PDF、このドキュメントの静的フォルダを登録し、`http_route`
でトップページといくつかの JSON エンドポイントに答え、ワーカー付きで起動します。

```sh
ling run site.ling --host 0.0.0.0 --port 80 --workers 4
```

`http_vhost(host, dir)` も使っています。これは別のホスト名のルートでフォルダを配信する関数で、
リクエストの `Host` ヘッダで照合します。`dir` がフォルダではなく `http://` や `https://` のアドレスなら、
そのホストはそこへリダイレクトします。古い `docs.ling-lang.org` は、こうして訪問者を
`ling-lang.org/docs` へ送っています。

```ling-module
関数 ドキュメントホスト登録() {
    http_vhost("docs.ling-lang.org", "https://ling-lang.org/docs")
    戻る 真
}
```

## やってみよう

1. メモを 1 件返す `GET /note?id=2` を加えましょう。`db_query` が空のリストを返したら、ステータス `404` と
   `{"error":"not found"}` を返します。
2. `db_exec("DELETE FROM notes WHERE id = ?1", id)` を使う `POST /delete?id=2` を加えましょう。`db_exec` が
   返す数を見て、何も削除されなかったら `404` を返します。
3. `http_static` で、`index.html` のある `public/` フォルダを `/app` に配信しましょう。
4. あいさつのページで訪問回数を `visits` テーブルに数え、合計を表示しましょう。

サーバーの組み込み関数については [Web サーバーと HTTP](../reference/web.md)と
[データベース（SQLite）](../reference/database.md)を参照してください。
