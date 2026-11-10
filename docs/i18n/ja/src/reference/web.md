# Web サーバーと HTTP

Ling には小さくて速い Web サーバーが付属しています。ルート、静的フォルダ、仮想ホストを
登録してから `http_serve` を呼ぶと、そこでブロックしてリクエストに応答し続けます。
`http_*` 組み込み関数は、Ling のコードから [`ling-http`](https://crates.io/crates/ling-http)
（axum と tokio）を動かします。同じ仲間に HTTP クライアント（`http_get` など）と、
クエリ文字列、Cookie、エスケープを扱うヘルパーもあります。

**ling-lang.org はこれで動いています。** サーバーは `site.ling` という Ling ファイル 1 つです。
トップページ、`/docs/` 以下のこのドキュメント、Ling ブックの PDF、サンプルの SVG を配信し、
`http_vhost` を使って役目を終えた `docs.ling-lang.org` サブドメインを `/docs/` へ転送しています。

> **インストールプロファイル。** サーバーとクライアントの組み込み関数は `web` プロファイル
> （と既定の `full`）に含まれます。`--no-default-features --features core` でビルドした
> `ling` は `unknown function 'http_static'` で止まります。`cargo install ling-lang` で
> 入れ直すか、`--features web` を付けてください。

## リクエストへの応答の順序

1. **仮想ホスト**（`http_vhost`）が最初です。`Host` ヘッダーが登録済みのホストを指していれば、
   そのホストのフォルダから応答します。ターゲットが URL ならリダイレクトします。
2. 次が**静的プレフィックス**（`http_static`）です。`/css` プレフィックスの下の
   `/css/style.css` は、ディスクからそのままのバイト列で読み出されます。
3. 最後が**ルート**（`http_route`）で、メソッドと**完全に一致する**パスで選ばれます。
   ハンドラは `Request` を受け取り、テキストかレスポンスのレコードを返す Ling 関数です。
4. それ以外は `404` で、本文は `not found` です。

1 と 2 は非同期ランタイムで動き、インタプリタには触れません。そのため大きなファイル
（動画、PDF、zip）も並行して流せ、HTTP の範囲リクエストにも対応します。Ling のハンドラは、
`http_serve` に `workers` を渡さない限り、インタプリタ上で 1 つずつ実行されます。

`http_serve` を呼ぶプログラムは、普通の `ling run` でも常にインタプリタで動きます。
ハンドラは引数として渡される関数だからです。

## 一覧

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `http_route(method, path, handler)` | — | — | — | `เว็บเส้นทาง` | 1 つのメソッドと完全一致のパスを処理します。 |
| `http_static(prefix, dir)` | — | — | — | `เว็บสแตติก` | URL プレフィックスでフォルダをそのまま配信します。 |
| `http_vhost(host, target)` | — | — | — | — | ホスト名のルートでフォルダを配信するか、そのホスト名をリダイレクトします。 |
| `http_serve(host, port, workers)` | — | — | — | `เว็บเสิร์ฟ` | HTTP を開始してリクエストループを回します。ブロックします。 |
| `http_serve_tls(host, port, cert, key, workers)` | — | — | — | — | PEM の証明書と鍵ファイルで HTTPS を提供します。 |
| `http_serve_dev_tls(host, port, workers)` | — | — | — | — | 使い捨ての自己署名証明書で HTTPS を提供します。 |
| `http_get(url)` | — | — | — | `เว็บรับ` | URL を取得して本文を待ちます。 |
| `http_get_status()` | — | — | — | — | 直前の `http_get` のステータス。 |
| `http_get_all(urls)` | — | — | — | — | 複数の URL を同時に取得します。 |
| `http_get_async(url)` | — | — | — | — | GET をバックグラウンドで開始し、ジョブ id を返します。 |
| `http_post_async(url, body, type)` | — | — | — | `เว็บโพสต์ไม่บล็อก` | POST をバックグラウンドで開始し、ジョブ id を返します。 |
| `http_job_poll(id)` | — | — | — | `เว็บงานสำรวจ` | ジョブの実行中は `""`、その後は結果。 |
| `query_param(qs, name, default)` | — | — | — | `พารามิเตอร์` | `a=1&b=2` から値を 1 つデコードして取り出します。 |
| `cookie_get(header, name, default)` | — | — | — | `รับคุกกี้` | `Cookie` ヘッダーから Cookie を 1 つ取り出します。 |
| `html_escape(text)` | — | — | — | `กันเอชทีเอ็มแอล` | HTML 用に `& < > " '` をエスケープします。 |
| `json_escape(text)` | — | — | — | `หนีเจสัน` | JSON 文字列用にテキストをエスケープします。 |
| `url_decode(text)` | `网址解码` | — | — | — | `%XX` と `+` をデコードします。 |
| `cli_arg(name, default)` | — | — | — | `อาร์กิวเมนต์` | コマンドラインの `--name value` をテキストで返します。 |
| `env_get(name, default)` | — | — | — | `รับตัวแปรแวดล้อม` | 環境変数。 |

## サーバーの組み込み関数

### `http_route(method, path, handler)`

`method`（大文字小文字を区別しない、既定値 `"GET"`）と `path`（既定値 `"/"`）に
`handler` を登録します。パスは**完全一致**で、クエリ文字列は含みません。そのため
パスパラメータはありません。id はクエリに入れ（`/note?id=2`）、
`query_param(req.query, "id", "")` で読んでください。ルートはすべて `http_serve` を
呼ぶ前に登録します。

ハンドラがエラーを出すと、本文に `handler error: ...` を入れた `500` を返します。
内部情報が見えてしまうことがあるので、入力を検証してエラーを外に出さないでください。

### `http_static(prefix, dir)`

フォルダ `dir`（既定値 `"static"`）を `prefix`（既定値 `"/static"`）で配信します。
`http_static("/css", "css")` は `/css/a.css` を `css/a.css` に対応させます。フォルダへの
リクエストにはその中の `index.html` を返します。ルートのハンドラはテキストしか返せない
ので、バイナリはすべてこれで配信してください。

### `http_vhost(host, target)`

ホスト名全体に専用の振る舞いを与えます。`target` はフォルダか URL です。

- **フォルダ**：そのホスト名を、フォルダのルートから `http_static` と同じくバイト列の
  まま配信します。ファイルがなければ `404` を返し、フォルダに `404.html` があればそれを
  使います（mdBook や多くの静的サイトジェネレーターが出力します）。静的サイトを独自の
  サブドメインに置くのに使います。
- **`http://` か `https://` の URL**：そのホスト名へのすべてのリクエストに
  `301 Moved Permanently` を返し、ターゲットにリクエストのパスとクエリを付けた場所へ
  転送します。古いリンクを壊さずにサブドメインを廃止するのに使います。

詳細：

- `host` は `Host` ヘッダーと大文字小文字を区別せずに比べ、ポートは無視します。
- 仮想ホストはすべての `http_static` プレフィックスとルートより先に調べられるので、
  仮想ホストへのリクエストがハンドラに届くことはありません。
- 仮想ホストは複数登録できます。1 つの `Host` が一致するのは多くても 1 つです。

```ling-gui
束縛 開始 = 実行 {
  http_vhost("docs.example.org", "../docs/book")              // サブドメインのルートに静的サイト
  http_vhost("old.example.org", "https://example.org/docs")   // old.example.org/a.html -> https://example.org/docs/a.html
  http_static("/docs", "../docs/book")                        // 同じサイトをメインホストの /docs/ にも
  http_serve("0.0.0.0", 8080)
}
```

ling-lang.org は `site.ling` の中で、このようにして古いドキュメント用サブドメインを
廃止しています。

```ling-module
関数 ドキュメントホスト登録(ポート) {
  http_vhost("docs.ling-lang.org", "https://ling-lang.org/docs")
  http_vhost("docs.localhost", "http://localhost:" + str(ポート) + "/docs")
  戻る 真
}
```

DNS を設定せずに手元で仮想ホストを試すには、ヘッダーを自分で送ります。

```sh
curl -i -H "Host: docs.localhost" http://127.0.0.1:8080/
```

たいていのブラウザは `*.localhost` を自分のマシンに解決するので、
`http://docs.localhost:8080/` でも試せます。

### `http_serve(host, port, workers)`

サーバーを起動し、プログラムが止められるまでリクエストに応答します。戻ることはありません。

- `host`：`"127.0.0.1"`（既定値）はこのマシンからのみ、`"0.0.0.0"` はネットワークからの
  接続も受け付けます。
- `port`：既定値は `8080`。
- `workers`：既定値は `1`。後の「ワーカー」の節を見てください。

ソケットのバインドに成功するとバナーが表示されます。ポートが使用中ならエラーは標準
エラー出力に出て、プログラムは待ち続けます。何も応答しないときはログを確認してください。

### `http_serve_tls(host, port, cert, key, workers)`

リバースプロキシなしで直接 HTTPS を提供します。`cert` と `key` は PEM ファイルの
**パス**で、たとえば Let's Encrypt の `fullchain.pem` と `privkey.pem` です。鍵ファイルは
サーバーのユーザーだけが読めるようにしてください。それ以外は `http_serve` と同じです。

### `http_serve_dev_tls(host, port, workers)`

起動時に作った自己署名証明書で HTTPS を提供します。ブラウザや `curl` はこれを拒否します
（手元では `curl -k` を使います）。ローカル開発専用です。インターネットには絶対に
公開しないでください。

## リクエスト

ハンドラは引数を 1 つ受け取ります。次のテキストフィールドを持つ `Request` レコードです。

| フィールド | 内容 |
|---|---|
| `method` | 大文字のメソッド（`GET` など）。 |
| `path` | クエリ文字列を除いたパス。 |
| `query` | `?` を除いた生のクエリ文字列。`query_param` で読みます。 |
| `body` | UTF-8 の本文。最大 32 MiB（マルチパートのアップロードは別、後述）。 |
| `cookie` | 生の `Cookie` ヘッダー、または `""`。`cookie_get` で読みます。 |
| `authorization` | 生の `Authorization` ヘッダー、または `""`。 |
| `client_ip` | `X-Forwarded-For` の最初の値、なければ `X-Real-IP`、なければ `""`。 |

`client_ip` はどのクライアントでも設定できるヘッダーから来ます。自分のリバースプロキシの
後ろでだけ信用してください。直接の接続では空です。

`multipart/form-data` のアップロードは、ファイルを `storage/tmp/` 以下へストリームで
書き込みます。ハンドラにはテキストフィールドと `artifact_path`、`artifact_size` を含む
フォーム形式の本文が渡されるので、`query_param(req.body, ...)` で読みます。

## レスポンス

**テキスト**を返すと、`text/html; charset=utf-8` の `200` になります。レコードでない
それ以外の値はテキストに変換され、`text/plain` で送られます。

それ以外の応答には、`status`、`content_type`、`body`、`set_cookie`、`location` のうち
必要なフィールドを持つレコードを返します。ないフィールドの既定値は、ステータス `200`、
空の本文、`text/html` です。レコードは位置で組み立てられ、意味を持つのはフィールド名です。
レコード自体の名前は関係ありません。フィールド名は英語のままにしてください。

```ling-gui
形 応答 { status, content_type, body, set_cookie, location }

関数 状態API(要求) {
  戻る 応答(200, "application/json", "{\"ok\":true}", "", "")
}

関数 古いページ(要求) {
  戻る 応答(301, "text/plain", "", "", "/new")
}

関数 ログイン(要求) {
  束縛 クッキー = "sid=" + random_hex(16) + "; HttpOnly; Secure; SameSite=Lax; Path=/"
  戻る 応答(303, "text/plain", "", クッキー, "/me")
}

束縛 開始 = 実行 {
  http_route("GET", "/api/status", 状態API)
  http_route("GET", "/old", 古いページ)
  http_route("POST", "/login", ログイン)
  http_serve("127.0.0.1", 8080)
}
```

Ling のルートからのレスポンスには、`/embed` 以下のパスを除いて `X-Frame-Options: DENY` と
`Content-Security-Policy: frame-ancestors 'none'` が付きます。それ以外は何も付けません。
Cookie の `Secure`、`HttpOnly`、`SameSite` は自分で設定し、ほかのセキュリティヘッダー
（HSTS、完全なコンテンツセキュリティポリシー）はリバースプロキシで追加してください。

## ワーカー

既定では 1 つのインタプリタがすべてのリクエストに 1 つずつ応答します。遅いハンドラが
あると、ほかの訪問者はみな待たされます。

`http_serve(host, port, N)` で `N` が 1 より大きいと、`N` 個のインタプリタが 1 つの
キューからリクエストを取ります。ling-lang.org はこの数をコマンドライン（`--workers 4`）
から読んでいます。決まりは次のとおりです。

- ハンドラは**名前の付いたトップレベルの関数**でなければなりません。インラインの
  クロージャをハンドラにすると、起動時にエラーになります。
- `開始` ブロックはメインのインタプリタで **1 回だけ**実行されます。ワーカーが見るのは
  プログラムの宣言だけなので、設定は `開始` が変更するトップレベルの束縛ではなく、
  関数に入れてください。
- 各ワーカーは `db_open` のデータベースへの自分の接続を開きます。
  [データベース（SQLite）](../reference/database.md)を参照してください。
- 非同期ジョブは共有されます。あるワーカーで始めたジョブを別のワーカーでポーリングできます。

## リクエスト用ヘルパー

これらはハンドラの中に限らずどこでも使えるので、ヘッドレスで試せます。

```ling
束縛 開始 = 実行 {
  束縛 クエリ = "q=%E3%81%93%E3%82%93%E3%81%AB%E3%81%A1%E3%81%AF&page=2&tag=x+y"
  印刷(query_param(クエリ, "q", ""))
  印刷(query_param(クエリ, "page", "1"))
  印刷(query_param(クエリ, "tag", ""))
  印刷(query_param(クエリ, "sort", "新着"))
  印刷(cookie_get("sid=abc123; theme=dark", "theme", "light"))
  印刷(html_escape("<b>エイダ & 'ボブ'</b>"))
  印刷("{\"名前\":\"" + json_escape("彼は\"やあ\"と言った\n") + "\"}")
  印刷(url_decode("a%20b+c%41"))
  印刷(cli_arg("port", "8080"))
}
```

```output
こんにちは
2
x y
新着
dark
&lt;b&gt;エイダ &amp; &#39;ボブ&#39;&lt;/b&gt;
{"名前":"彼は\"やあ\"と言った\n"}
a b cA
8080
```

- `query_param(qs, name, default)`：`name` の最初の値を、`%XX` と `+` をデコードして返します。
  `req.query` にもフォームの本文（`req.body`）にも使えます。
- `cookie_get(header, name, default)`：`req.cookie` から Cookie を 1 つ、デコードせずに返します。
- `html_escape(text)`：HTML を組み立てる場所で、信用できない値をすべてエスケープします。
  要素の中身と引用符で囲んだ属性には十分ですが、`<script>` の中や引用符のない属性では
  足りません。
- `json_escape(text)`：`"`、`\`、制御文字をエスケープします。前後の引用符は付けません。
  Ling は JSON を連結で組み立てるので、テキストは必ずこれを通してください。
- `url_decode(text)`：`%XX` と `+` をデコードします。
- `cli_arg(name, default)`：コマンドラインの `--name value` を**テキスト**で返します。
  `to_number` で変換してください。`cli_arg("port", "8080") + 0` は `"80800"` になります。
- `env_get(name, default)`：環境変数。秘密の値はソースに書かず、こうして読み込みます。

## HTTP クライアント

### `http_get(url)` と `http_get_status()`

`http_get` は `http://` か `https://` の URL を取得し、本文を待ちます。エラーのステータスでも
本文を返します。`http_get_status()` は直前の `http_get` のステータスで、`0` はまったく応答が
なかったこと（DNS、接続、TLS の失敗）を表し、そのとき本文は `""` です。本文を信用する前に
ステータスを確認してください。

`http_get` はブロックします。ツールや起動時なら問題ありませんが、ハンドラの中では
すべての訪問者を止めてしまうので、そこでは非同期版を使ってください。

### `http_get_all(urls)`

URL のリストを同時に取得し、すべて終わるのを待ちます。本文を同じ順番で返し、失敗や
HTTP エラーのステータスは `""` になります。

### `http_get_async(url)`、`http_post_async(url, body, type)`、`http_job_poll(id)`

リクエストをバックグラウンドで開始し、すぐにジョブ id を返します。`type` は POST の本文の
`Content-Type` です（既定値 `"application/json"`）。あとで `http_job_poll(id)` を呼ぶと、
実行中は `""`、終わればレスポンスの本文を返します。GET が失敗すると `ERROR:` で始まる
テキストに、POST が失敗すると `{"error":"..."}` になります。

```ling-gui
束縛 開始 = 実行 {
  束縛 ジョブ = http_get_async("https://ling-lang.org/api/books.json")
  束縛 本文 = ""
  束縛 回数 = 0
  間 本文 == "" && 回数 < 100 {
    束縛 本文 = http_job_poll(ジョブ)
    眠る(50)
    束縛 回数 = 回数 + 1
  }
  印刷(本文)
}
```

## 実例：小さなサイト

```ling-gui
形 応答 { status, content_type, body, set_cookie, location }

関数 トップページ(要求) {
  戻る read_file("index.html")
}

関数 挨拶API(要求) {
  束縛 名前 = query_param(要求.query, "name", "世界")
  戻る 応答(200, "application/json", "{\"hello\":\"" + json_escape(名前) + "\"}", "", "")
}

束縛 開始 = 実行 {
  http_static("/css", "css")
  http_static("/images", "images")
  http_route("GET", "/", トップページ)
  http_route("GET", "/api/hello", 挨拶API)
  http_serve(cli_arg("host", "127.0.0.1"), to_number(cli_arg("port", "8080")), 4)
}
```

`ling run site.ling --port 8080` で実行し、`http://127.0.0.1:8080/` か
`http://127.0.0.1:8080/api/hello?name=Ada` を開いてください。

## 関連項目

- [データベース（SQLite）](../reference/database.md)：`db_open`、`db_exec`、`db_query`。
- [小さな Web サービス](../tutorials/web-service.md)：完全なサービスを順を追って作ります。
- [ネットワーク](../reference/networking.md)：ロビー型ネットワーク、SSH、ピアツーピア。
- [ling-http と ling-web](../crates/web.md)：同じスタックを Rust から直接使います。
