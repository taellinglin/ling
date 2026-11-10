# Database (SQLite)

Ling has SQLite built in. Three builtins cover everything: `db_open` opens a
database file, `db_exec` changes data or schema, and `db_query` reads rows
back as records. SQLite is bundled into `ling`, so there is nothing to install
and no server to run. The database is one ordinary file next to your program.

The builtins are part of the `web` install profile (and `full`, the default),
because they are usually paired with the [web server](../reference/web.md).
They work just as well in a command-line tool.

## Summary

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `db_open(path)` | — | — | — | `ฐานข้อมูลเปิด` | Open or create a database and make it the current connection. |
| `db_exec(sql, params...)` | — | — | — | `ฐานข้อมูลรัน` | Run one statement that changes data or schema. Returns rows affected. |
| `db_query(sql, params...)` | — | — | — | `ฐานข้อมูลถาม` | Run a query. Returns a list of `Row` records. |

## A first database

```ling
bind start = do {
  db_open(":memory:")
  db_exec("CREATE TABLE notes(id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL, stars REAL, note TEXT)")
  db_exec("INSERT INTO notes(title, stars) VALUES (?1, ?2)", "first", 4.5)
  db_exec("INSERT INTO notes(title, stars) VALUES (?1, ?2)", "it's safe", 3)
  bind rows = db_query("SELECT id, title, stars, note FROM notes ORDER BY id")
  print(len(rows))
  print(rows[0])
  for row in rows {
    print(str(row.id) + ": " + row.title)
  }
  print("[" + rows[1].note + "]")
}
```

```output
2
Row { id: 1, title: first, stars: 4.5, note:  }
1: first
2: it's safe
[]
```

`":memory:"` makes a database that lives only while the program runs, which is
handy for tests and examples. Use a file name such as `"app.db"` to keep the data.

## `db_open(path)`

Opens the SQLite database at `path` (default `"app.db"`), creating the file if it
doesn't exist, and makes it the interpreter's current connection. Returns nothing.

- On opening, Ling turns on foreign keys (`PRAGMA foreign_keys = ON`), write-ahead
  logging (`journal_mode = WAL`) and a 5-second busy timeout, so a write that meets
  another write waits its turn instead of failing with "database is locked".
- Each interpreter holds **one** connection. Calling `db_open` again replaces it.
- With WAL, SQLite keeps two helper files next to the database while it's open
  (`app.db-wal` and `app.db-shm`). Back up all three, or copy the database while
  no program has it open.
- If the file can't be opened, `db_open` raises an error.

## `db_exec(sql, params...)`

Runs **one** SQL statement that doesn't return rows: `CREATE`, `INSERT`,
`UPDATE`, `DELETE`, `BEGIN`, `COMMIT`, and so on. Returns the number of rows it
changed (`0` for schema statements).

- Put one statement in each call. Two statements separated by `;` are an error.
- On failure it raises an error that includes the SQL text. Calling it before
  `db_open` is also an error.

## `db_query(sql, params...)`

Runs a query and returns **all** rows as a list of `Row` records. Read a column
with a dot: `row.title`. The field names are the column names from your `SELECT`,
so give computed columns a name with `AS`:

```ling
bind start = do {
  db_open(":memory:")
  db_exec("CREATE TABLE log(n INTEGER)")
  db_exec("BEGIN")
  bind i = 0
  while i < 1000 {
    db_exec("INSERT INTO log(n) VALUES (?1)", i)
    bind i = i + 1
  }
  db_exec("COMMIT")
  bind r = db_query("SELECT count(*) AS n, sum(n) AS total, avg(n) AS mean FROM log")[0]
  print(r.n)
  print(r.total)
  print(r.mean)
}
```

```output
1000
499500
499.5
```

Wrapping many inserts in `BEGIN` and `COMMIT` makes them one transaction, which
is much faster than committing each row separately.

### How values come back

| SQLite value | Ling value |
|---|---|
| `INTEGER` | number |
| `REAL` | number |
| `TEXT` | text |
| `NULL` | `""` (empty text) |
| `BLOB` | base64 text |

A query with no matches returns an empty list, so check `len(rows)` before
reading `rows[0]`.

## Parameters

Parameters bind **positionally** to `?1`, `?2`, `?3`, and so on, in the order you
pass them after the SQL. Always pass user input as a parameter, never by gluing
it into the SQL text. A parameter is only ever a value, so it can't change the
statement:

```ling
bind start = do {
  db_open(":memory:")
  db_exec("CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT UNIQUE, admin INTEGER)")
  bind name = "Robert'); DROP TABLE users; --"
  db_exec("INSERT INTO users(name, admin) VALUES (?1, ?2)", name, false)
  db_exec("INSERT INTO users(name, admin) VALUES (?1, ?2)", "ada", true)
  print(db_query("SELECT count(*) AS n FROM users")[0].n)
  print(db_query("SELECT name FROM users WHERE admin = ?1", true)[0].name)
  print(db_query("SELECT last_insert_rowid() AS id")[0].id)
  print(db_exec("UPDATE users SET admin = 0 WHERE name = ?1", "ada"))
  print(db_exec("DELETE FROM users WHERE id > ?1", 100))
}
```

```output
2
ada
2
1
0
```

How Ling values bind:

| Ling value | Binds as |
|---|---|
| whole number (`3`) | `INTEGER` |
| fractional number (`4.5`) | `REAL` |
| `true` / `false` | `1` / `0` |
| anything else | `TEXT` (the value as text) |

To get the id of the row you just inserted, query `SELECT last_insert_rowid() AS id`
on the same connection, as above.

## Errors

A failing statement (a syntax error, a missing table, a broken `UNIQUE`
constraint) raises a runtime error whose message starts with `db_exec:` or
`db_query:` and shows the SQL.

On the interpreter (`ling run --interp`, and always for programs that call
`http_serve`) the error stops the program, or comes back as `bad` inside `try`.
With the default JIT backend of `ling run`, a failing statement is currently
skipped without a message, so run database scripts with `--interp` while you
develop them.

```ling-module
fn add_user(name) {
  return db_exec("INSERT INTO users(name) VALUES (?1)", name)
}

fn try_add(name) {
  match try(| | add_user(name)) {
    ok(n) => return "added"
    bad(e) => return "could not add " + name + ": " + e
  }
}
```

## With the web server

The usual shape is: open the database in `start`, create the tables if needed,
register routes, then serve. Handlers call `db_query` and `db_exec` directly.

```ling-gui
form Response { status, content_type, body, set_cookie, location }

fn list_notes(req) {
  bind rows = db_query("SELECT id, title FROM notes ORDER BY id DESC LIMIT 50")
  bind out = "<ul>"
  for row in rows {
    bind out = out + "<li>" + html_escape(row.title) + "</li>"
  }
  return out + "</ul>"
}

fn add_note(req) {
  bind title = query_param(req.body, "title", "")
  if title == "" {
    return Response(400, "text/plain", "title is required", "", "")
  }
  db_exec("INSERT INTO notes(title) VALUES (?1)", title)
  return Response(303, "text/plain", "", "", "/")
}

bind start = do {
  db_open("notes.db")
  db_exec("CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL)")
  http_route("GET", "/", list_notes)
  http_route("POST", "/add", add_note)
  http_serve("127.0.0.1", 8080, 4)
}
```

### Workers and connections

With `http_serve(host, port, 1)` (the default) one interpreter answers every
request, one at a time, over its one connection.

With `workers` above 1, each extra worker is its own interpreter with **its own
connection** to the path you gave `db_open`. Reads run side by side. Writes still
happen one at a time: WAL plus the busy timeout make them queue instead of fail.
Two things follow:

- Call `db_open` in `start`, **before** `http_serve`. Workers open the path that
  was current when the server started.
- Don't use `":memory:"` with workers. Each worker would get its own separate,
  empty in-memory database.

Keep transactions short inside handlers. A `BEGIN` without a `COMMIT` in the same
handler holds the write lock and makes every other writer wait.

## See also

- [Web Server and HTTP](../reference/web.md): routes, requests, responses and workers.
- [A Small Web Service](../tutorials/web-service.md): a complete service step by step.
- [ling-http and ling-web](../crates/web.md): the same SQLite layer used from Rust.
