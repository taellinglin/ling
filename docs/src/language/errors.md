# Errors and Results

Ling has no exceptions. A function that can fail returns a **result**: `ok(value)` when
it worked, or `bad(reason)` when it didn't. The caller opens the result with `match`.
This page covers results, the `try` builtin, runtime errors and tracebacks, and the
"soft failure" convention most builtins follow.

| Tool | Use it for |
|---|---|
| `ok(value)` / `bad(reason)` | Returning success or failure from your own functions |
| `match r { ok(v) => ..., bad(e) => ... }` | Handling both cases |
| `try(f, args...)` | Turning a runtime error into a `bad` result (interpreter only) |
| `ling run --interp` | Seeing runtime errors with a traceback |

## Results: ok and bad

`ok(value)` wraps a success value and `bad(reason)` wraps a failure. The reason is
usually text, but it can be any value. A result prints as `Ok(...)` or `Err(...)`.
(`err` is another name for `bad` when you build a result, but patterns must use `bad`.)

```ling
fn safe_div(a, b) {
  if b == 0 { bad("divide by zero") } else { ok(a / b) }
}
bind start = do {
  println(safe_div(6, 3))
  println(safe_div(1, 0))
}
```
```output
Ok(2)
Err(divide by zero)
```

## Handling a result with match

`ok(v)` and `bad(e)` are also patterns. The name inside the parentheses is bound to the
wrapped value for that arm:

```ling
fn safe_div(a, b) {
  if b == 0 { bad("divide by zero") } else { ok(a / b) }
}
fn show(r) {
  match r {
    ok(v) => "value " + str(v),
    bad(why) => "failed: " + why
  }
}
bind start = do {
  println(show(safe_div(8, 2)))
  println(show(safe_div(8, 0)))
}
```
```output
value 4
failed: divide by zero
```

`match` is the only way to look inside a result, so small helpers pay off:

```ling
fn unwrap_or(r, fallback) {
  match r {
    ok(v) => v,
    _ => fallback
  }
}
fn safe_sqrt(x) {
  if x < 0 { bad("negative") } else { ok(sqrt(x)) }
}
bind start = do {
  println(unwrap_or(safe_sqrt(16), 0))
  println(unwrap_or(safe_sqrt(-16), 0))
}
```
```output
4
0
```

> Calling a method on a result returns the wrapped value (`ok(5).anything` is `5`).
> That is rarely what you want. Use `match`.

## Validating input

Check each rule in turn and give each failure its own reason:

```ling
fn parse_age(text) {
  bind t = trim(text)
  if len(t) == 0 {
    bad("empty input")
  } else if to_number(t) == 0 && t != "0" {
    bad("not a number: " + t)
  } else if to_number(t) > 130 {
    bad("too large: " + t)
  } else {
    ok(to_number(t))
  }
}
fn show(r) {
  match r {
    ok(v) => "age " + str(v),
    bad(why) => "rejected (" + why + ")"
  }
}
bind start = do {
  for s in ["42", " 7 ", "", "forty", "200"] {
    println(show(parse_age(s)))
  }
}
```
```output
age 42
age 7
rejected (empty input)
rejected (not a number: forty)
rejected (too large: 200)
```

When one step depends on another, nest the matches and add context to each `bad`, so
the final message says where things went wrong:

```ling
fn parse_num(text) {
  bind n = to_number(text)
  if n == 0 && text != "0" { bad("not a number: " + text) } else { ok(n) }
}
fn area(wtext, htext) {
  match parse_num(wtext) {
    bad(e) => bad("width: " + e),
    ok(w) => match parse_num(htext) {
      bad(e) => bad("height: " + e),
      ok(h) => ok(w * h)
    }
  }
}
bind start = do {
  println(area("4", "2.5"))
  println(area("4", "x"))
}
```
```output
Ok(10)
Err(height: not a number: x)
```

## Runtime errors and tracebacks

Some problems stop a program with a **runtime error**: calling an unknown function,
reading a missing file, indexing past the end of a list, a constructor with the wrong
number of fields, and so on. The interpreter prints the error, the file, and a
traceback with the deepest call last:

```ling-module
fn load(p) { read_file(p) }
fn go() { load("no_such_file.txt") }
bind start = do {
  println(go())
}
```

```sh
$ ling run --interp broken.ling
error[runtime]: read_file 'no_such_file.txt': The system cannot find the file specified. (os error 2)
 --> broken.ling
  traceback (deepest call last):
    0: start
    1: go
    2: load
```

The program stops with a non-zero exit code. (The OS message after the colon depends on
your system.)

> **The JIT doesn't stop.** On the default engine most of these problems quietly give
> `()` and the program carries on: the program above prints `()` under `ling run`.
> When a program misbehaves, run it once with `ling run --interp` to see the real error.

Ling has no `assert`, `panic` or `exit` builtin. To signal failure, return a `bad`
result and print a clear message.

## try: catching a runtime error

`try(f, args...)` calls `f` with the given arguments. It returns `ok(result)` if the call
worked and `bad(message)` if it raised a runtime error. Pass the function itself (its
bare name, no parentheses), followed by its arguments. A closure works too:
`try(| | 6 * 7)`.

**`try` works only on the interpreter.** The JIT can't pass a function to a builtin, so
there `try` always returns `bad("try: first argument must be a function")`:

```ling
fn plus1(x) { x + 1 }
fn broken(x) { missing_function(x) }
bind start = do {
  println(try(plus1, 4))
  println(try(broken, 4))
}
```
```output
Err(try: first argument must be a function)
Err(try: first argument must be a function)
```

The same program on the interpreter:

```sh
$ ling run --interp try_demo.ling
Ok(5)
Err(unknown function 'missing_function')
```

Programs that call `http_serve` always run on the interpreter, so `try` works inside web
request handlers. Everywhere else, treat `try` as a development aid and write functions
that return `ok` and `bad` yourself.

## Soft failure in builtins

Most builtins don't return results. On failure they return a harmless neutral value and
keep going:

| Builtin | On failure |
|---|---|
| `to_number("x")` | `0` |
| `list_get(xs, 99)` | `""` (empty text) |
| `str_find("abc", "z")` | `-1` |
| `file_exists(path)` | `false` |
| `read_file("missing.txt")` | `()` on the JIT, a runtime error on the interpreter |

```ling
bind start = do {
  println(to_number("banana"))
  println("[" + list_get([1, 2, 3], 99) + "]")
  println(str_find("hello", "z"))
  println(file_exists("no-such-file.txt"))
}
```
```output
0
[]
-1
false
```

A failure can look like a real value: `to_number("0")` and `to_number("zero")` are both
`0`. Ask first: call `file_exists` before `read_file`, compare an index with `len(xs)`,
and validate text before trusting `to_number`. A small wrapper turns a soft failure into
a real result:

```ling
fn load_text(path) {
  if file_exists(path) { ok(read_file(path)) } else { bad("no such file: " + path) }
}
bind start = do {
  println(load_text("missing.cfg"))
}
```
```output
Err(no such file: missing.cfg)
```
