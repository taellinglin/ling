# Known Limits

Ling is young and moves quickly. This page collects the behaviours that surprise people
most, so you can recognise them in minutes instead of hours. Everything here was checked
against the current `ling` binary (2030.1.53) on both engines: the default JIT
(`ling run`) and the interpreter (`ling run --interp`).

> **Tip:** When a program misbehaves, run it once with `ling run --interp`. The
> interpreter reports unknown functions and runtime errors that the JIT quietly turns
> into `()`.

## JIT and interpreter at a glance

| Situation | JIT (default) | Interpreter |
|---|---|---|
| Unknown function name | returns `()` silently | `unknown function` error |
| Runtime error (missing file, bad index, ...) | returns `()` or an empty value, keeps going | stops with an error and a traceback |
| `1 / 0` | `()` | `inf` |
| `pi` as a bare name | `()` (write `pi()`) | the value |
| `""` or `[]` in a condition | true | false |
| Index past the end of a list | empty value | `index N out of bounds` error |
| One character of text, `s[i]` | empty value | the character (use `substr(s, i, 1)` on both) |
| `for c in "text"` | empty values | each character |
| Ordering text, `"a" < "b"` | `false` | error |
| `bind` inside `do { }` | visible afterwards | discarded |
| Closure stored in a variable, `f(3)` | `()` | `unknown function 'f'` error |
| Function or closure passed to a builtin (`try`, `list_map`, ...) | becomes `()` | works |
| `wait expr` | `()` | the value |
| Printing a range value `0..5` | `[0, 5]` | `[0, 1, 2, 3, 4]` |
| Constructor with the wrong number of fields | `()` | error |
| Missing function argument, when used | `()` | `undefined` error |
| `return` directly inside `start` | ends the program quietly | `unexpected top-level return` error |

A program that avoids these corners behaves the same on both engines.

## There is no assignment

`x = x + 1` is a parse error. Rebind instead: `bind x = x + 1`. See
[Bindings](syntax.md#bindings-and-rebinding).

## Loops and scope

| Construct | Does a `bind` inside update the outer variable? |
|---|---|
| `while` body | yes |
| `if` body | yes |
| `for` body | **no**: each pass gets a fresh copy of the scope |
| `do { }` block | yes on the JIT, no on the interpreter; don't rely on either |

To accumulate a total or build a list, use a `while` loop with a counter.

There is no `break` or `continue`. `stop` and `again` are reserved words that do nothing:
on the JIT the loop just keeps going, and on the interpreter they are `undefined`
errors. Put the loop in a function and `return`, or use `while` with a flag.

## Range bounds must be simple

`0..n` and `0..(n + 1)` work. `0..n + 1` is a parse error, and `0..len(xs)` silently
does the wrong thing (an empty loop on the JIT, an error on the interpreter). Wrap
anything but a plain name or number in parentheses: `0..(len(xs))`.

## && and || always evaluate both sides

Neither engine short-circuits. In `x != 0 && 10 / x > 1` the division still runs when
`x` is `0`. Use nested `if`s when the right side must not run.

## Numbers and text

- Number literals are decimal digits with at most one fractional part. There is no
  `1e3`, `0xff`, `1_000`, `.5` or `3.`. Worse, `1e3` and `0xff` aren't parse errors: they
  read as `1` or `0` followed by an unknown name.
- Text uses double quotes only and has no interpolation. Build text with `+` and `str()`.
- `print` and `println` both end the line, and both join their arguments with **no
  separator**: `println("sum", 1 + 2)` prints `sum3`.
- `!x` means `x == false`, so `!0` and `!""` are `false`. Use `!` on booleans only.

## Comparing structured values

`==` on lists, records, enum values and results is always `false`, even for identical
contents. Compare elements or fields yourself. See
[Types and Values](types.md#comparing-values).

## What the type system does and doesn't do

Ling is dynamically typed. Type annotations such as `x: number` and `-> T` are parsed
and ignored. There is no map or dictionary, no set, no tuple and no JSON parser. Use
records or parallel lists, or store data in SQLite with the `db_*` builtins.

## Keywords that do nothing

These words are reserved but have no effect:

`stop`, `again`, `change`, `post`, `give`, `fit`, `can`, `sure`, `maybe`, `pure`, `spawn`,
`none`

The ownership words `own`, `lend`, `share`, `move` and `copy` are parsed and ignored,
with no borrow or move checking. `async fn` runs synchronously. `type` declarations are
thrown away and swallow what follows them. See [Keywords](keywords.md).

## Closures and function values

A closure expression called directly, `(|x| x + 1)(5)`, works everywhere. Beyond that:

- A closure stored in a variable can't be called by name on the interpreter, and is
  unreliable on the JIT.
- The JIT can't pass a function or closure to a builtin. `list_map`, `list_filter` and
  similar builtins return `()`, and `try()` always returns
  `bad("try: first argument must be a function")`. They work on the interpreter.
- Programs that call `http_serve` run on the interpreter automatically, so closures and
  `try` work in request handlers.

Prefer named top-level `fn`s and plain loops.

## Modules

- `use "file"` loads into one shared namespace. If two files define the same function,
  the last one loaded silently wins, even for callers in the earlier file. This also
  applies to `start`, so a library must never bind `start`.
- Inside a `mod`, call sibling functions with the qualified name (`geo::area`). A bare
  call fails.
- Values bound inside a `mod` are not usable. Expose constants as functions.

Prefix your library functions with the library name, for example `geom_area`. See
[Modules and Projects](modules.md).

## Tooling

- `lingfu new` creates `src/start.ling`, but `lingfu build` looks for `main.ling` unless
  the manifest says otherwise. Add `entry = "src/start.ling"` to the `[spirit]` section
  of `Ling.toml`.
- `lingfu check`, `lingfu fmt` and `lingfu translate` call `ling` subcommands that don't
  exist. They print `ling`'s usage text and do nothing; `check` and `fmt` still report
  success. Use `ling run --interp` to check a program and
  `lingfu normalize <lang>` to translate one (it rewrites every `.ling` file in the
  current folder, so use it on a copy).
- `lingfu normalize` gets a few keywords wrong. It writes `form` as a word the lexer
  doesn't accept in Chinese, Korean and Thai (use the spelling from
  [Keywords](keywords.md#spellings)), leaves `choose` in English, and in Japanese and
  Thai writes `ok`/`bad` with builtin names that don't work as `match` patterns. Some ownership words (`copy`,
  `lend`, `share`) also come out wrong in Japanese and Thai. Run the
  result and fix those words by hand.
- `lingfu build` compiles generated Rust with Cargo, so it needs a Rust toolchain.

## Testing

`lingfu test` runs tests on the JIT, which doesn't stop on runtime errors, so it can
print `All tests passed!` for a test that failed. Make your tests print a clear `PASS`
or `FAIL` line and check it, or run them with `ling run --interp`. A helper file inside
`tests/` is treated as a test and fails with `no entry point`, so keep helpers
elsewhere.

## Parse traps

- A line that starts with `-` or `[` continues the previous line.
- `form P { x: number, y: number }` is a parse error. End annotated field lists with a
  comma: `form P { x: number, y: number, }`.
- An empty closure needs a space, `| | expr`, because `||` is "or".
- `fn main() { ... }` is not an entry point. Use `bind main = do { ... }`.
- `assert`, `panic`, `exit`, `append_file` and `delta_time` are not builtins.
  For frame timing, subtract successive `time_now()` values.
