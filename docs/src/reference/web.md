# Web Server and HTTP

Ling ships a small, fast web server. You register routes, static folders and
virtual hosts, then call `http_serve`, which blocks and answers requests. The
`http_*` builtins drive [`ling-http`](https://crates.io/crates/ling-http)
(axum and tokio) from Ling code. The same family has an HTTP client
(`http_get` and friends) and helpers for query strings, cookies and escaping.

**ling-lang.org runs on this.** Its server is one Ling file, `site.ling`. It
serves the landing page, these docs under `/docs/`, the Ling Book PDFs and the
example SVGs, and uses `http_vhost` to send the retired `docs.ling-lang.org`
subdomain to `/docs/`.

> **Install profile.** The server and client builtins are part of the `web`
> profile (and `full`, the default). A `ling` built with
> `--no-default-features --features core` stops with
> `unknown function 'http_static'`. Reinstall with `cargo install ling-lang`
> or add `--features web`.

## How a request is answered

1. **Virtual hosts** (`http_vhost`) come first. If the `Host` header names a
   registered host, the request is answered from that host's folder, or
   redirected if the target is a URL.
2. **Static prefixes** (`http_static`) come next. `/css/style.css` under a
   `/css` prefix is read from disk byte for byte.
3. **Routes** (`http_route`) come last: method plus **exact** path. The handler
   is a Ling function that receives a `Request` and returns text or a response
   record.
4. Anything else gets `404` with the body `not found`.

Steps 1 and 2 run on the async runtime and never touch the interpreter, so big
files (videos, PDFs, zips) stream concurrently and support HTTP range requests.
Ling handlers run one at a time on the interpreter unless you pass `workers` to
`http_serve`.

Programs that call `http_serve` always run on the interpreter, even under plain
`ling run`, because handlers are functions passed as arguments.

## Summary

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `http_route(method, path, handler)` | — | — | — | `เว็บเส้นทาง` | Handle one method and exact path. |
| `http_static(prefix, dir)` | — | — | — | `เว็บสแตติก` | Serve a folder at a URL prefix, byte for byte. |
| `http_vhost(host, target)` | — | — | — | — | Serve a folder at the root of a hostname, or redirect that hostname. |
| `http_serve(host, port, workers)` | — | — | — | `เว็บเสิร์ฟ` | Start HTTP and run the request loop. Blocks. |
| `http_serve_tls(host, port, cert, key, workers)` | — | — | — | — | HTTPS with PEM certificate and key files. |
| `http_serve_dev_tls(host, port, workers)` | — | — | — | — | HTTPS with a throwaway self-signed certificate. |
| `http_get(url)` | — | — | — | `เว็บรับ` | Fetch a URL and wait for the body. |
| `http_get_status()` | — | — | — | — | Status of the last `http_get`. |
| `http_get_all(urls)` | — | — | — | — | Fetch several URLs at once. |
| `http_get_async(url)` | — | — | — | — | Start a GET in the background. Returns a job id. |
| `http_post_async(url, body, type)` | — | — | — | `เว็บโพสต์ไม่บล็อก` | Start a POST in the background. Returns a job id. |
| `http_job_poll(id)` | — | — | — | `เว็บงานสำรวจ` | `""` while a job runs, then its result. |
| `query_param(qs, name, default)` | — | — | — | `พารามิเตอร์` | One value from `a=1&b=2`, decoded. |
| `cookie_get(header, name, default)` | — | — | — | `รับคุกกี้` | One cookie from a `Cookie` header. |
| `html_escape(text)` | — | — | — | `กันเอชทีเอ็มแอล` | Escape `& < > " '` for HTML. |
| `json_escape(text)` | — | — | — | `หนีเจสัน` | Escape text for a JSON string. |
| `url_decode(text)` | `网址解码` | — | — | — | Decode `%XX` and `+`. |
| `cli_arg(name, default)` | — | — | — | `อาร์กิวเมนต์` | `--name value` from the command line, as text. |
| `env_get(name, default)` | — | — | — | `รับตัวแปรแวดล้อม` | An environment variable. |

## Server builtins

### `http_route(method, path, handler)`

Registers `handler` for `method` (case-insensitive, default `"GET"`) and `path`
(default `"/"`). Paths match **exactly** and exclude the query string, so there
are no path parameters: pass ids in the query (`/note?id=2`) and read them with
`query_param(req.query, "id", "")`. Register every route before calling
`http_serve`.

A handler that raises an error answers `500` with `handler error: ...` in the
body, which can reveal internals. Validate input so errors don't escape.

### `http_static(prefix, dir)`

Serves the folder `dir` (default `"static"`) at `prefix` (default `"/static"`):
`http_static("/css", "css")` maps `/css/a.css` to `css/a.css`. A request for a
folder serves its `index.html`. Use it for everything binary, because route
handlers can only return text.

### `http_vhost(host, target)`

Gives a whole hostname its own behaviour. `target` is either a folder or a URL.

- **A folder:** that hostname is served from the folder at its root, byte for
  byte like `http_static`. A missing file answers `404` with the folder's own
  `404.html` when it has one (mdBook and most static-site generators write one).
  Use it to put a static site on its own subdomain.
- **An `http://` or `https://` URL:** every request to that hostname answers
  `301 Moved Permanently` to the target plus the request's path and query.
  Use it to retire a subdomain without breaking old links.

Details:

- `host` is matched against the `Host` header case-insensitively, with any
  port ignored.
- Vhosts are checked before every `http_static` prefix and route, so a vhost
  request never reaches your handlers.
- Registering several vhosts is fine; each `Host` matches at most one.

```ling-gui
bind start = do {
  http_vhost("docs.example.org", "../docs/book")              // a static site at the root of the subdomain
  http_vhost("old.example.org", "https://example.org/docs")   // old.example.org/a.html -> https://example.org/docs/a.html
  http_static("/docs", "../docs/book")                        // the same site under /docs/ on the main host
  http_serve("0.0.0.0", 8080)
}
```

This is how ling-lang.org retires its old docs subdomain in `site.ling`:

```ling-module
fn register_docs_hosts(port) {
  http_vhost("docs.ling-lang.org", "https://ling-lang.org/docs")
  http_vhost("docs.localhost", "http://localhost:" + str(port) + "/docs")
  return true
}
```

To try a vhost locally without DNS, send the header yourself:

```sh
curl -i -H "Host: docs.localhost" http://127.0.0.1:8080/
```

Most browsers also resolve `*.localhost` to your own machine, so
`http://docs.localhost:8080/` works too.

### `http_serve(host, port, workers)`

Starts the server and answers requests until the program is stopped. It never
returns.

- `host`: `"127.0.0.1"` (default) for this machine only, `"0.0.0.0"` to accept
  connections from the network.
- `port`: default `8080`.
- `workers`: default `1`. See the Workers section below.

A banner prints once the socket is bound. If the port is busy, the error goes to
standard error and the program keeps waiting, so check the log when nothing
answers.

### `http_serve_tls(host, port, cert, key, workers)`

Serves HTTPS directly, no reverse proxy needed. `cert` and `key` are **paths** to
PEM files, for example Let's Encrypt's `fullchain.pem` and `privkey.pem`. Keep the
key file readable only by the server's user. Otherwise it works like
`http_serve`.

### `http_serve_dev_tls(host, port, workers)`

Serves HTTPS with a self-signed certificate generated at start-up. Browsers and
`curl` reject it (use `curl -k` locally). For local development only; never
expose it to the internet.

## The request

A handler receives one argument, a `Request` record with these text fields:

| Field | Content |
|---|---|
| `method` | Upper-case method, such as `GET`. |
| `path` | Path without the query string. |
| `query` | Raw query string, without the `?`. Read it with `query_param`. |
| `body` | Body as UTF-8. Up to 32 MiB, except multipart uploads (below). |
| `cookie` | Raw `Cookie` header, or `""`. Read it with `cookie_get`. |
| `authorization` | Raw `Authorization` header, or `""`. |
| `client_ip` | First `X-Forwarded-For` hop, else `X-Real-IP`, else `""`. |

`client_ip` comes from headers that any client can set. Trust it only behind
your own reverse proxy; it is empty for direct connections.

A `multipart/form-data` upload streams its file to disk under `storage/tmp/`. The
handler then gets a form-encoded body with the text fields plus `artifact_path`
and `artifact_size`; read them with `query_param(req.body, ...)`.

## The response

Return **text** for `200` with `text/html; charset=utf-8`. Any other value that
isn't a record is turned into text and sent as `text/plain`.

For anything else, return a record with any of the fields `status`,
`content_type`, `body`, `set_cookie` and `location`. Missing fields default to
status `200`, an empty body and `text/html`. Records are built positionally, and
the field names are what count; the record's own name doesn't matter:

```ling-gui
form Response { status, content_type, body, set_cookie, location }

fn api_status(req) {
  return Response(200, "application/json", "{\"ok\":true}", "", "")
}

fn old_page(req) {
  return Response(301, "text/plain", "", "", "/new")
}

fn login(req) {
  bind cookie = "sid=" + random_hex(16) + "; HttpOnly; Secure; SameSite=Lax; Path=/"
  return Response(303, "text/plain", "", cookie, "/me")
}

bind start = do {
  http_route("GET", "/api/status", api_status)
  http_route("GET", "/old", old_page)
  http_route("POST", "/login", login)
  http_serve("127.0.0.1", 8080)
}
```

Responses from Ling routes carry `X-Frame-Options: DENY` and
`Content-Security-Policy: frame-ancestors 'none'`, except for paths under
`/embed`. Nothing else is added: set `Secure`, `HttpOnly` and `SameSite` on
cookies yourself, and add other security headers (HSTS, a full content security
policy) at your reverse proxy.

## Workers

By default one interpreter answers every request, one at a time. A slow handler
makes every other visitor wait.

With `http_serve(host, port, N)` and `N` above 1, `N` interpreters take requests
from one queue. ling-lang.org reads the number from the command line
(`--workers 4`). Rules:

- Handlers must be **named top-level functions**. An inline closure handler is
  an error at start-up.
- The `start` block runs **once**, on the main interpreter. Workers only see the
  program's declarations, so keep configuration in functions, not in top-level
  `bind`s that `start` changes.
- Each worker opens its own connection to the `db_open` database. See
  [Database (SQLite)](../reference/database.md).
- Async jobs are shared: a job started on one worker can be polled from another.

## Request helpers

These work anywhere, not only in handlers, so you can try them headless:

```ling
bind start = do {
  bind qs = "q=a%20b&page=2&tag=x+y"
  print(query_param(qs, "q", ""))
  print(query_param(qs, "page", "1"))
  print(query_param(qs, "tag", ""))
  print(query_param(qs, "sort", "new"))
  print(cookie_get("sid=abc123; theme=dark", "theme", "light"))
  print(html_escape("<b>Ada & 'Bob'</b>"))
  print("{\"name\":\"" + json_escape("say \"hi\"\n") + "\"}")
  print(url_decode("a%20b+c%41"))
  print(cli_arg("port", "8080"))
}
```

```output
a b
2
x y
new
dark
&lt;b&gt;Ada &amp; &#39;Bob&#39;&lt;/b&gt;
{"name":"say \"hi\"\n"}
a b cA
8080
```

- `query_param(qs, name, default)`: the first value for `name`, with `%XX` and
  `+` decoded. Works on `req.query` and on form bodies (`req.body`).
- `cookie_get(header, name, default)`: one cookie from `req.cookie`, not decoded.
- `html_escape(text)`: escape every untrusted value where you build HTML. It is
  enough for element content and quoted attributes, not inside `<script>` or
  unquoted attributes.
- `json_escape(text)`: escapes `"`, `\` and control characters, without adding
  the surrounding quotes. Ling builds JSON by concatenation, so always pass text
  through it.
- `url_decode(text)`: decode `%XX` and `+`.
- `cli_arg(name, default)`: `--name value` from the command line, as **text**.
  Convert with `to_number`: `cli_arg("port", "8080") + 0` gives `"80800"`.
- `env_get(name, default)`: an environment variable. Load secrets this way, not
  from source.

## HTTP client

### `http_get(url)` and `http_get_status()`

`http_get` fetches an `http://` or `https://` URL and waits for the body. It
returns the body even for an error status. `http_get_status()` gives the status
of the last `http_get`; `0` means no response at all (DNS, connection or TLS
failure), and the body is then `""`. Check the status before trusting the body.

`http_get` blocks. That's fine in a tool or at start-up, but inside a handler it
stalls every visitor, so use the async versions there.

### `http_get_all(urls)`

Fetches a list of URLs at the same time and waits for all of them. Returns the
bodies in the same order, with `""` for a failure or an HTTP error status.

### `http_get_async(url)`, `http_post_async(url, body, type)` and `http_job_poll(id)`

Start a request in the background and return a job id at once. `type` is the
`Content-Type` of the POST body (default `"application/json"`). Call
`http_job_poll(id)` later: it returns `""` while the job runs, then the response
body. A failed GET gives text starting with `ERROR:`; a failed POST gives
`{"error":"..."}`.

```ling-gui
bind start = do {
  bind job = http_get_async("https://ling-lang.org/api/books.json")
  bind body = ""
  bind tries = 0
  while body == "" && tries < 100 {
    bind body = http_job_poll(job)
    sleep(50)
    bind tries = tries + 1
  }
  print(body)
}
```

## Worked example: a small site

```ling-gui
form Response { status, content_type, body, set_cookie, location }

fn page_index(req) {
  return read_file("index.html")
}

fn api_hello(req) {
  bind name = query_param(req.query, "name", "world")
  return Response(200, "application/json", "{\"hello\":\"" + json_escape(name) + "\"}", "", "")
}

bind start = do {
  http_static("/css", "css")
  http_static("/images", "images")
  http_route("GET", "/", page_index)
  http_route("GET", "/api/hello", api_hello)
  http_serve(cli_arg("host", "127.0.0.1"), to_number(cli_arg("port", "8080")), 4)
}
```

Run it with `ling run site.ling --port 8080`, then open `http://127.0.0.1:8080/`
or `http://127.0.0.1:8080/api/hello?name=Ada`.

## See also

- [Database (SQLite)](../reference/database.md): `db_open`, `db_exec`, `db_query`.
- [A Small Web Service](../tutorials/web-service.md): a complete service step by step.
- [Networking](../reference/networking.md): lobby networking, SSH and peer-to-peer.
- [ling-http and ling-web](../crates/web.md): the same stack used directly from Rust.
