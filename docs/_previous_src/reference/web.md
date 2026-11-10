# Builtins — Web Server & HTTP

Ling ships a small, fast web server. The `http_*` builtins drive
[`ling-http`](https://crates.io/crates/ling-http) (axum + tokio) from Ling code:
you register routes and static folders, then call `http_serve`, which blocks
and answers requests.

> **Build profile.** The server builtins are part of the `web` profile (and
> `full`, the default). A `ling` built with `--no-default-features --features core`
> stops with `unknown function 'http_static'`. Reinstall with
> `cargo install ling-lang` or `--features web`.

**ling-lang.org itself runs on this.** Its server is one file, `site.ling`,
that serves the landing page, these docs (on `docs.ling-lang.org` through
`http_vhost`), the Ling Book PDFs and the example SVGs.

## How a request is answered

1. **Virtual hosts** (`http_vhost`) come first: if the `Host` header names a
   registered host, the file comes straight from that host's folder.
2. **Static prefixes** (`http_static`) come next: `/css/style.css` under a
   `/css` prefix is read from disk byte for byte.
3. **Routes** (`http_route`) come last: method plus **exact** path. The
   handler is a Ling function that receives a `Request` and returns a string
   or a response struct.
4. Anything else gets `404 not found`.

Steps 1 and 2 run on the async runtime and never touch the interpreter, so
large files (videos, PDFs, zips) stream concurrently and support HTTP range
requests. Ling handlers run one at a time on the interpreter thread unless you
pass `workers` to `http_serve`.

## Server builtins

| Builtin | Purpose |
|---|---|
| `http_route(method, path, handler)` | Handle one method and exact path (query string excluded). |
| `http_static(prefix, dir)` | Serve a folder at a URL prefix, byte for byte. |
| `http_vhost(host, dir)` | Serve a folder at the root of another hostname. |
| `http_serve(host, port, workers?)` | Start HTTP and run the request loop. Blocks. |
| `http_serve_tls(host, port, cert, key, workers?)` | HTTPS from PEM files on disk (e.g. Let's Encrypt `fullchain.pem` and `privkey.pem`). |
| `http_serve_dev_tls(host, port, workers?)` | HTTPS with a throwaway self-signed certificate. Local development only. |

Thai aliases: `เว็บเส้นทาง` (`http_route`), `เว็บสแตติก` (`http_static`),
`เว็บเสิร์ฟ` (`http_serve`).

### `http_route(method, path, handler)`

`method` is case-insensitive and defaults to `"GET"`. Paths match exactly, so
there are no path parameters. Pass ids in the query string (`/note?id=2`) and
read them with `query_param(req.query, "id", "")`. Register every route before
calling `http_serve`.

### `http_static(prefix, dir)`

Serves `dir` at `prefix`: `http_static("/css", "css")` maps `/css/a.css` to
`css/a.css`. A request for a folder serves its `index.html`. Use it for
everything binary, since route handlers can only return text.

### `http_vhost(host, dir)`

Serves `dir` at the root of a hostname: a whole static site (an mdBook build,
for example) on its own subdomain.

- `host` is matched against the `Host` header, case-insensitively, with the
  port ignored.
- A vhost request never reaches your static prefixes or routes.
- A missing file answers `404` with the folder's own `404.html` when it has one.

```ling-gui
bind start = do {
  http_vhost("docs.example.org", "../docs/book")   // docs at the root of the subdomain
  http_static("/docs", "../docs/book")             // and under /docs/ on the main host
  http_serve("0.0.0.0", 8080)
}
```

To try a vhost locally without DNS, use `curl -H "Host: docs.localhost" http://127.0.0.1:8080/`.
Most browsers also resolve `*.localhost` to your own machine.

### `http_serve(host, port, workers)`

- `host`: `"127.0.0.1"` (default) for this machine only, `"0.0.0.0"` for the network.
- `port`: default `8080`.
- `workers`: default `1`. With `N > 1`, `N` interpreters take requests from
  one queue, so a slow handler no longer stalls other visitors. Handlers must
  then be **named top-level functions**, and the `start` block runs only once,
  on the main interpreter. Keep configuration in functions, not top-level `bind`s.

A banner prints once the socket is bound. If the port is busy, the error goes
to standard error and the program keeps waiting.

## The request

A handler receives a `Request` with these text fields:

| Field | Content |
|---|---|
| `method` | Upper-case method. |
| `path` | Path without the query. |
| `query` | Raw query string, without the `?`. |
| `body` | Body as UTF-8. Multipart uploads stream to `storage/tmp/`, and the body then lists `artifact_path` and `artifact_size`. |
| `cookie` | Raw `Cookie` header. Read it with `cookie_get`. |
| `authorization` | Raw `Authorization` header. |
| `client_ip` | First `X-Forwarded-For` hop, else `X-Real-IP`, else `""`. Trust it only behind your own proxy. |

## The response

Return a **string** for `200 text/html`. For anything else, return a struct
with any of the fields `status`, `content_type`, `body`, `set_cookie` and
`location`. Structs are built positionally, and the field names are what count:

```ling-gui
form Response { status, content_type, body, set_cookie, location }

fn page_json(req) {
  return Response(200, "application/json", "{\"ok\":true}", "", "")
}

fn page_old(req) {
  return Response(302, "text/plain", "", "", "/new")
}
```

Every response except paths under `/embed` carries `X-Frame-Options: DENY`
and `Content-Security-Policy: frame-ancestors 'none'`.

## Request helpers

| Builtin | Purpose |
|---|---|
| `query_param(qs, name, default)` | One value from `a=1&b=2`, percent-decoded. Also works on form bodies. |
| `cookie_get(header, name, default)` | One cookie from `req.cookie`. |
| `html_escape(text)` | Escape `& < > " '` before putting user text in HTML. |
| `json_escape(text)` | Escape text for a JSON string literal (Ling builds JSON by concatenation). |
| `url_decode(text)` | Decode `%XX` and `+`. |
| `cli_arg(name, default)` | `--name value` from the command line, as text (convert with `to_number`). |
| `env_get(name, default)` | An environment variable. |

## HTTP client

| Builtin | Purpose |
|---|---|
| `http_get(url)` | Fetch a URL and wait for the body (`http://` or `https://`). |
| `http_get_status()` | Status of the last `http_get`. `0` means no response at all. |
| `http_get_all(urls)` | Fetch several URLs at once and wait for all of them. |
| `http_get_async(url)` / `http_post_async(url, body, type)` | Start a request in the background and return a job id at once. |
| `http_job_poll(id)` | `""` while running, then the response body. |

`http_get` blocks. Inside a server handler that stalls every visitor, so use
the async versions there.

## Worked example: a small site

```ling-gui
form Response { status, content_type, body, set_cookie, location }

fn page_index(req) {
  return read_file("index.html")
}

fn api_hello(req) {
  bind name = html_escape(query_param(req.query, "name", "world"))
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

Run it with `ling run site.ling --port 8080`. Programs that call `http_serve`
always run on the interpreter, where closures and `try` work fully.

## From Rust

The same stack can be used from Rust directly:

- [`ling-http`](https://crates.io/crates/ling-http): the axum/TLS/SQLite core (`serve_http`, `serve_tls`, `serve_dev_tls`, `Db`).
- [`ling-web`](https://crates.io/crates/ling-web): a Flask-style layer on top, with sessions, flash messages, templates (minijinja), static files, blueprints and CSRF.

The *Network Services in Ling* and *Web Apps with the Rust Crates* chapters
of *Mastering Ling* cover both in depth. See [The Ling Book](../books.md).
