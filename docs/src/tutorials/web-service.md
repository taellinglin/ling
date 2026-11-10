# A Small Web Service

Ling has a web server built in. On this page you serve a page, read what the browser
sent, answer with JSON, keep notes in a SQLite database, and run the result as a small
notes API. No framework and no extra installs are needed.

Code that starts a server can't be checked in the terminal, so those blocks have no
output. The helpers and handlers are ordinary functions, and you can test them without
a server, as you will see below.

## Hello, web

```ling-gui
fn hello(req) {
    return "<h1>Hello from Ling!</h1>"
}

bind start = do {
    http_route("GET", "/", hello)
    http_serve("127.0.0.1", 8080)
}
```

Save it as `hello.ling`, run it, and open <http://127.0.0.1:8080/> in a browser:

```sh
ling run hello.ling
```

- `http_route(method, path, handler)` registers a **handler** for one method and one exact
  path. Register every route before you start the server.
- `http_serve(host, port)` starts the server and keeps answering requests until you press
  Ctrl+C. Use `"127.0.0.1"` for your own machine only, `"0.0.0.0"` to accept connections
  from the network.
- A handler takes the request and returns a string. A string answers `200` with an HTML page.
- Any path you didn't register answers `404 not found`.

To serve files such as CSS, images or a whole folder, add `http_static(prefix, dir)`:
`http_static("/css", "css")` maps `/css/site.css` to the file `css/site.css`. Static files
are sent byte for byte and never pass through your handlers.

## What the browser sent

The handler's argument is a `Request` with these text fields:

| Field | Content |
|---|---|
| `method` | `"GET"`, `"POST"`, ... in upper case |
| `path` | the path without the query string |
| `query` | the raw query string after `?`, such as `name=Ada&page=2` |
| `body` | the request body as text |
| `cookie` | the raw `Cookie` header |
| `authorization` | the raw `Authorization` header |
| `client_ip` | the client address from proxy headers, or `""` |

Routes match the path exactly, so there are no path parameters like `/note/2`. Pass values
in the query string instead (`/note?id=2`) and read them with
`query_param(text, name, default)`. It percent-decodes the value, and it works on form
bodies too, since they use the same `a=1&b=2` format.

```ling
bind start = do {
    bind qs = "name=Ada%20Lovelace&lang=en"
    print(query_param(qs, "name", "nobody"))
    print(query_param(qs, "page", "1"))
}
```

```output
Ada Lovelace
1
```

Never put user text straight into HTML. `html_escape` turns `& < > " '` into entities:

```ling
bind start = do {
    print(html_escape("<b>Tom & Jerry</b>"))
}
```

```output
&lt;b&gt;Tom &amp; Jerry&lt;/b&gt;
```

So a greeting page looks like this:

```ling-module
fn greet(req) {
    bind name = html_escape(query_param(req.query, "name", "world"))
    return "<h1>Hello, " + name + "!</h1>"
}
```

## Answering with JSON

To set a status code or a content type, return a record instead of a string. Declare a
`form` whose field names are any of `status`, `content_type`, `body`, `set_cookie` and
`location`. Records are built by position, and the server reads the fields by name:

```ling-module
form Response { status, content_type, body }

fn json(status, body) {
    return Response(status, "application/json", body)
}
```

Ling builds JSON by joining strings. Pass every piece of text through `json_escape`, which
escapes quotes, backslashes and control characters:

```ling
bind start = do {
    bind title = "Say \"hi\""
    print("{\"title\":\"" + json_escape(title) + "\"}")
}
```

```output
{"title":"Say \"hi\""}
```

For a redirect, return status `302` with a `location`, for example
`Response(302, "text/plain", "", "", "/new")` with a form that declares all five fields.

## A notes API with SQLite

Now store notes in a database. Three builtins cover SQLite:

| Builtin | Purpose |
|---|---|
| `db_open(path)` | open (or create) a database file, or `":memory:"` |
| `db_exec(sql, params...)` | run a statement that changes data; returns the number of rows changed |
| `db_query(sql, params...)` | run a query; returns a list of rows, read as `row.column` |

Parameters bind by position as `?1`, `?2` and so on. Always pass user input as a parameter,
never by joining it into the SQL text.

Handlers are just functions, so you can try them without a server by building a request
yourself. Here `Request` is a small `form` with the fields the handlers use:

```ling
form Response { status, content_type, body }
form Request { method, path, query, body }

fn json(status, body) {
    return Response(status, "application/json", body)
}

fn note_json(row) {
    return "{\"id\":" + row.id + ",\"text\":\"" + json_escape(row.text) + "\"}"
}

fn list_notes(req) {
    bind rows = db_query("SELECT id, text FROM notes ORDER BY id")
    bind out = "["
    bind i = 0
    while i < len(rows) {
        if i > 0 {
            bind out = out + ","
        }
        bind out = out + note_json(rows[i])
        bind i = i + 1
    }
    return json(200, out + "]")
}

fn add_note(req) {
    bind text = query_param(req.body, "text", "")
    if text == "" {
        return json(400, "{\"error\":\"text is required\"}")
    }
    db_exec("INSERT INTO notes(text) VALUES (?1)", text)
    bind rows = db_query("SELECT id, text FROM notes ORDER BY id DESC LIMIT 1")
    return json(201, note_json(rows[0]))
}

fn setup() {
    db_open(":memory:")
    db_exec("CREATE TABLE notes(id INTEGER PRIMARY KEY AUTOINCREMENT, text TEXT NOT NULL)")
}

bind start = do {
    setup()
    bind r1 = add_note(Request("POST", "/notes", "", "text=Buy%20milk"))
    print(r1.status, " ", r1.body)
    bind r2 = add_note(Request("POST", "/notes", "", "text=Say+%22hi%22"))
    print(r2.status, " ", r2.body)
    bind r3 = add_note(Request("POST", "/notes", "", ""))
    print(r3.status, " ", r3.body)
    bind r4 = list_notes(Request("GET", "/notes", "", ""))
    print(r4.status, " ", r4.body)
}
```

```output
201 {"id":1,"text":"Buy milk"}
201 {"id":2,"text":"Say \"hi\""}
400 {"error":"text is required"}
200 [{"id":1,"text":"Buy milk"},{"id":2,"text":"Say \"hi\""}]
```

The list is built with a `while` loop and an index, because a `for` body can't update a
binding outside the loop.

### Serve it

Keep the handlers and `setup` exactly as above, drop the test `Request` form, and replace
the `start` block with routes:

```ling-gui
form Response { status, content_type, body }

fn json(status, body) {
    return Response(status, "application/json", body)
}

fn note_json(row) {
    return "{\"id\":" + row.id + ",\"text\":\"" + json_escape(row.text) + "\"}"
}

fn list_notes(req) {
    bind rows = db_query("SELECT id, text FROM notes ORDER BY id")
    bind out = "["
    bind i = 0
    while i < len(rows) {
        if i > 0 {
            bind out = out + ","
        }
        bind out = out + note_json(rows[i])
        bind i = i + 1
    }
    return json(200, out + "]")
}

fn add_note(req) {
    bind text = query_param(req.body, "text", "")
    if text == "" {
        return json(400, "{\"error\":\"text is required\"}")
    }
    db_exec("INSERT INTO notes(text) VALUES (?1)", text)
    bind rows = db_query("SELECT id, text FROM notes ORDER BY id DESC LIMIT 1")
    return json(201, note_json(rows[0]))
}

fn setup() {
    db_open("notes.db")
    db_exec("CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY AUTOINCREMENT, text TEXT NOT NULL)")
}

fn greet(req) {
    bind name = html_escape(query_param(req.query, "name", "world"))
    return "<h1>Hello, " + name + "!</h1>"
}

bind start = do {
    setup()
    http_route("GET", "/", greet)
    http_route("GET", "/notes", list_notes)
    http_route("POST", "/notes", add_note)
    http_serve("127.0.0.1", to_number(cli_arg("port", "8080")))
}
```

This version opens `notes.db`, a file next to the program, so the notes survive a restart.
`cli_arg("port", "8080")` reads `--port` from the command line as text, and `to_number`
turns it into a number.

```sh
ling run notes.ling --port 8080
```

Try it from another terminal:

```text
$ curl -X POST -d "text=Buy milk" http://127.0.0.1:8080/notes
{"id":1,"text":"Buy milk"}
$ curl http://127.0.0.1:8080/notes
[{"id":1,"text":"Buy milk"}]
$ curl "http://127.0.0.1:8080/?name=%3Cb%3EAda"
<h1>Hello, &lt;b&gt;Ada!</h1>
```

## More visitors at once: workers

By default one interpreter answers requests one at a time, so a slow handler makes the
others wait. Static files from `http_static` don't count: they are served separately and
stream in parallel. To run handlers in parallel, pass a number of workers as the third
argument to `http_serve`. Here it comes from the command line:

```ling-gui
fn hello(req) {
    return "<h1>Hello from Ling!</h1>"
}

fn workers() {
    return to_number(cli_arg("workers", "1"))
}

bind start = do {
    http_route("GET", "/", hello)
    http_serve("127.0.0.1", 8080, workers())
}
```

```sh
ling run hello.ling --workers 4
```

With more than one worker, each extra interpreter loads your program fresh, and the `start`
block runs only once, on the main one. So:

- Handlers must be **named top-level functions**, like every handler on this page.
- Keep settings in functions (like `workers()` above), not in top-level `bind`s.
- Each worker reopens the database you opened with `db_open`. Use a file, not
  `":memory:"`, or every worker gets its own empty database. SQLite makes a write wait its
  turn when two workers write at the same moment.

## ling-lang.org runs on this

The website ling-lang.org is served by one Ling program, `site.ling`, built from the same
builtins. It registers static folders for CSS, images, videos, the Ling Book PDFs and these
docs with `http_static`, answers the landing page and a few JSON endpoints with
`http_route`, and starts with workers:

```sh
ling run site.ling --host 0.0.0.0 --port 80 --workers 4
```

It also uses `http_vhost(host, dir)`, which serves a folder at the root of another host
name, matched against the request's `Host` header. When `dir` is an `http://` or
`https://` address instead of a folder, that host answers with a redirect to it. That is
how the old `docs.ling-lang.org` sends visitors to `ling-lang.org/docs`:

```ling-module
fn register_docs_hosts() {
    http_vhost("docs.ling-lang.org", "https://ling-lang.org/docs")
    return true
}
```

## Try it

1. Add `GET /note?id=2` that returns one note, or status `404` with
   `{"error":"not found"}` when `db_query` returns an empty list.
2. Add `POST /delete?id=2` with `db_exec("DELETE FROM notes WHERE id = ?1", id)`. Use the
   number `db_exec` returns to answer `404` when nothing was deleted.
3. Serve a folder `public/` with an `index.html` at `/app` using `http_static`.
4. Make the greeting page count visits in a `visits` table and show the total.

For every server builtin, see [Web Server and HTTP](../reference/web.md) and
[Database (SQLite)](../reference/database.md).
