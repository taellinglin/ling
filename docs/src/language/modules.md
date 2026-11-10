# Modules and Projects

When a program outgrows one file, move functions into library files and load them with
`use`. This page covers `use`, `mod` blocks, what happens when names collide, and how to
lay out a project with `lingfu`.

Ling's module system is small: `use` loads a file into one shared namespace, and nothing
is private. A few naming habits, described below, keep that comfortable.

| Form | What it does |
|---|---|
| `use "lib/file.ling"` | Loads the file; its functions, types and top-level bindings become visible by plain name |
| `use "lib/file.ling" as m` | Loads the file; call its functions as `m::name()` |
| `mod name { fn f() { ... } }` | Groups functions in one file; call them as `name::f()` |

## use: loading another file

`use "path"` loads another `.ling` file. The path is text, and it is resolved
**relative to the file that contains the `use`**, not to the folder you ran `ling` from.
The `.ling` extension may be left off.

Everything in the loaded file becomes available: functions, `form` and `choose` types,
and top-level `bind` values. A loaded file can itself `use` other files, with paths
relative to its own location.

**`lib/textkit.ling`**

```ling-module
// textkit: small text helpers
fn tk_pad_left(text, width) {
  bind gap = width - len(text)
  if gap > 0 { str_repeat(" ", gap) + text } else { text }
}
fn tk_rule(width) { str_repeat("-", width) }
```

**`lib/report.ling`** loads `textkit.ling` from its own folder:

```ling-module
use "textkit.ling"
form Item { name, qty, }
fn rp_line(item) { item.name + tk_pad_left(str(item.qty), 6) }
```

**`main.ling`**

```ling-module
use "lib/report.ling"
bind start = do {
  println(rp_line(Item("tea", 3)))
  println(rp_line(Item("jam", 12)))
  println(tk_rule(9))
}
```

```sh
$ ling run main.ling
tea     3
jam    12
---------
```

`main.ling` never loads `textkit.ling` directly, but `tk_rule` is still visible: every
loaded file shares one namespace.

### use ... as

With `as`, the loaded file's functions are called through the alias. They are then
**not** available by plain name:

```ling-module
use "lib/textkit.ling" as tk
bind start = do {
  println(tk::tk_rule(5))
}
```

```sh
$ ling run alias.ling
-----
```

## Name collisions

All loaded names share one namespace, and there is no error or warning when two
definitions collide:

- If two loaded files define the same function, the one loaded **last** wins, even for
  callers inside the earlier file.
- A definition in the main file beats anything it loads.

With `lib/first.ling` containing `fn who() { "first" }` and `lib/second.ling` containing
`fn who() { "second" }`:

```ling-module
use "lib/first.ling"
use "lib/second.ling"
bind start = do {
  println(who())
}
```

```sh
$ ling run clash.ling
second
```

Add `fn who() { "main file" }` to the main file and it prints `main file` instead.

The same rule applies to the entry point: if a library binds `start` and is loaded after
the main file's `start`, the library's `start` runs. So:

- **Give each library a short prefix** (`tk_`, `rp_`) and use it on every function.
- **Never put a `start` binding in a library.** Only the main program has one.
- **Pass data in as parameters.** A library function sees top-level bindings, but never
  the local bindings of its caller.

## mod: a namespace inside a file

`mod name { ... }` groups functions under a name that you reach with `::`. It is useful
when several groups would otherwise need the same function names:

```ling
mod vec {
  fn add(a, b) { [a[0] + b[0], a[1] + b[1]] }
  fn dot(a, b) { a[0] * b[0] + a[1] * b[1] }
  fn len2(a) { vec::dot(a, a) }
}
mod money {
  fn add(a, b) { round((a + b) * 100) / 100 }
}
bind start = do {
  println(vec::add([1, 2], [3, 4]))
  println(vec::len2([3, 4]))
  println(money::add(0.1, 0.2))
}
```
```output
[4, 6]
25
0.3
```

Rules for `mod`:

- **Call sibling functions with the qualified name** (`vec::dot`), as `len2` does. A
  bare `dot(a, a)` fails on the interpreter and gives `()` on the JIT.
- Functions in a `mod` can call the file's top-level functions without a prefix.
- **Don't bind values inside a `mod`.** `vec::ORIGIN` gives `()` on the JIT and the text
  `vec::ORIGIN` on the interpreter, and even the
  module's own functions can't see it. Expose a constant as a function instead:
  `fn origin() { [0, 0] }`, called as `vec::origin()`.

## Projects with lingfu

`lingfu new <name>` creates a project folder:

```sh
$ lingfu new shop
~ Ling Manifest — Spirit Token Descends ~
Creating project: shop

✓ Spirit Realm created!
  cd shop
  lingfu build
  lingfu run
```

```text
shop/
  Ling.toml          the manifest
  README.md
  ling.ignore
  lexicons/en.ling   language data (leave it alone)
  src/start.ling     the entry point
  tests/test.ling    a starter test
```

Put your libraries under `src/`, for example `src/lib/textkit.ling`, and load them from
`src/start.ling` with `use "lib/textkit.ling"`.

| Command | What it does |
|---|---|
| `lingfu run` | Runs `src/start.ling` |
| `lingfu test` | Runs every `.ling` file in `tests/` |
| `lingfu build` | Builds a distributable (needs the `entry` line below) |

### Entry points in a project

- The **program** is `src/start.ling`, with a `bind start = do { ... }`.
- A **test** in `tests/` is a program too. It binds any name to a `do` block (the starter
  test uses `bind test = do { ... }`) and loads the code it tests with a path relative to
  itself, such as `use "../src/lib/textkit.ling"`. Every file in `tests/` must have an
  entry point: a helper file without one fails with `no entry point`.
- A **library** has no entry binding at all.

### The manifest

`Ling.toml` is created for you. `lingfu run` and `lingfu test` work without changes, but
`lingfu build` looks for `main.ling` unless the manifest names the entry file. Add an
`entry` line to the `[spirit]` section:

```toml
[spirit]
name = "shop"
kind = "bin"
version = "2030.0.0"
entry = "src/start.ling"
```

See [lingfu](../tools/lingfu.md) for all commands and [Known Limits](limits.md#tooling)
for the current tooling quirks.
