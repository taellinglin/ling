# Syntax

This page describes how a Ling program is written: its layout, literals, operators,
bindings, functions and control flow. Every example was run with the current `ling`
binary. Ling has two engines, the default JIT (`ling run`) and the interpreter
(`ling run --interp`). Where they behave differently, this page says so, and
[Known Limits](limits.md) collects all the differences in one place.

## Program structure

A Ling file is a list of top-level **items**:

| Item | Form | See |
|---|---|---|
| binding | `bind name = expression` | [Bindings](#bindings-and-rebinding) |
| function | `fn name(a, b) { ... }` | [Functions](#functions) |
| record type | `form Name { field, field }` | [Records and enums](#records-and-enums) |
| enum type | `choose Name { Variant, Variant(a) }` | [Records and enums](#records-and-enums) |
| module | `mod name { fn ... }` | [Modules and Projects](modules.md) |
| import | `use "file.ling"` | [Modules and Projects](modules.md) |

Statements such as `println(...)` are not allowed at top level. They live inside a
function or inside the entry point.

### The entry point

A program starts at a top-level binding whose value is a `do` block:

```ling
bind start = do {
  println("Hello, Ling!")
}
```
```output
Hello, Ling!
```

The runtime looks for a binding with one of these names, in this order: `start`,
`main`, `启`, `เริ่ม`, `시작`, and the same word in several other languages. If none
exists, it runs the first top-level binding whose value is a `do` block. A file with
no such binding is a library, and `ling run` stops with `no entry point`.

```ling
bind main = do {
  println("main works too")
}
```
```output
main works too
```

> `fn main() { ... }` is **not** an entry point. Write `bind main = do { ... }`.

## Comments and statements

A comment runs from `#` or `//` to the end of the line. There are no block comments.
Statements are separated by newlines. A `;` may end a statement, which lets you put
two on one line.

```ling
# a hash comment
// a slash comment
bind start = do {
  println("a") // a trailing comment
  println("b"); println("c")
}
```
```output
a
b
c
```

## Identifiers

A name starts with a letter or `_` and continues with letters, digits and `_`. Any
Unicode letter counts, so Chinese, Japanese, Korean and Thai names work. Names are
case-sensitive.

```ling
bind start = do {
  bind 名前 = "Ling"
  bind ชื่อ = 3
  println(名前)
  println(ชื่อ + 1)
}
```
```output
Ling
4
```

Keywords that do nothing yet (such as `change`, `stop` or `maybe`, see
[Keywords](keywords.md)) are ordinary names to the parser, so `bind stop = 2` works.
Don't use an active keyword such as `for` or `if` as a name.

## Literals

| Literal | Form | Notes |
|---|---|---|
| number | `42`, `3.14` | Decimal digits with at most one fractional part. All numbers are 64-bit floats. |
| text | `"hello"` | Double quotes only. May span lines. Escapes: `\n` `\t` `\r` `\"` `\\` `\0`. |
| boolean | `true`, `false` | |
| unit | `()` | The "nothing" value. |
| list | `[1, "two", 3.5]` | Items of any type, indexed from 0. |
| range | `0..5` | Half-open: 0, 1, 2, 3, 4. Used with `for`. |
| closure | `\|x\| x + 1` | See [Closures](#closures). |

There is no `1e3`, `0xff`, `1_000`, `.5` or `3.`. There is no character literal
(`'a'` is a parse error), no map literal and no tuple. Text has no interpolation:
`"{x}"` is just those three characters. Build text with `+` and `str()`. An unknown
escape such as `\q` is kept as written, backslash included.

```ling
bind start = do {
  println("line1\nline2")
  println("quote: \"q\" backslash: \\")
  println(1.5 + 1)
  println([1, "two", 3.5, true])
  println(())
}
```
```output
line1
line2
quote: "q" backslash: \
2.5
[1, two, 3.5, true]
()
```

## Operators and precedence

From tightest to loosest:

| Level | Operators | Notes |
|---|---|---|
| 1 | `.` `()` `[]` `::` `..` | field or method, call, index, module path, range |
| 2 | `-x` `!x` | prefix minus and not |
| 3 | `*` `/` `%` | `%` works on fractions: `7.5 % 2` is `1.5` |
| 4 | `+` `-` | `+` joins text; a number on either side is turned into text |
| 5 | `==` `!=` `<` `>` `<=` `>=` | |
| 6 | `&&` | |
| 7 | `\|\|` | |

All binary operators group to the left: `10 - 3 - 2` is `5`.

```ling
bind start = do {
  println(2 + 3 * 4)
  println((2 + 3) * 4)
  println(10 - 3 - 2)
  println(-3 + 5)
  println(2 * -4)
  println(1 < 2 && 2 < 3)
  println("n=" + 5)
  println(7 / 2)
}
```
```output
14
20
5
2
-8
true
n=5
3.5
```

Points to watch:

- `&&` and `||` **always evaluate both sides**, on both engines. `x != 0 && 10 / x > 1`
  does not protect the division. Use a nested `if` instead.
- Comparisons chain to the left: `1 == 1 == 1` means `(1 == 1) == 1`, which is `false`.
- Prefix minus can't be doubled: `- - x` is a parse error. `0 - x` always works.
- `==` on lists, records and enum values is always `false`. See
  [Types and Values](types.md#comparing-values).

## Bindings and rebinding

`bind name = expression` gives a value a name. There is no assignment: `x = 5` is a
parse error. To "change" a value, bind the name again. The new binding replaces the old
one from that point on.

```ling
bind start = do {
  bind x = 1
  bind x = x + 10
  bind x = x * 2
  println(x)
}
```
```output
22
```

Whether a rebinding is still visible after a block ends depends on the block:

| Inside | Rebinding after the block ends |
|---|---|
| `if` / `else` body | kept |
| `while` body | kept, so `while` loops can accumulate |
| `for` body | **lost**: each pass starts from a fresh copy |
| `match` arm | lost |
| function body | local to the call |
| `do { }` block | lost on the interpreter, kept on the JIT; don't rely on either |

```ling
bind start = do {
  bind total = 0
  for i in 1..5 { bind total = total + i }
  println(total)
  bind sum = 0
  bind i = 1
  while i < 5 {
    bind sum = sum + i
    bind i = i + 1
  }
  println(sum)
}
```
```output
0
10
```

A top-level `bind` is a global constant. Functions can read it, but a function never
sees the local bindings of its caller:

```ling
bind RATE = 3
fn scaled(n) { n * RATE }
bind start = do {
  bind RATE = 100
  println(scaled(2))
}
```
```output
6
```

## Blocks

`do { ... }` runs its statements in order, and its value is the value of the last one.
`start` is a `do` block, and you can use one anywhere an expression goes:

```ling
bind start = do {
  bind total = do {
    bind a = 20
    bind b = 22
    a + b
  }
  println(total)
}
```
```output
42
```

## Functions

`fn name(params) { body }` declares a function at top level (or inside a `mod`).
Its result is the last expression in the body. `return value` leaves early, and a bare
`return` returns `()`. Functions can be recursive and can be declared in any order.
Functions can't be nested inside other functions.

```ling
fn fib(n) {
  if n < 2 { return n }
  fib(n - 1) + fib(n - 2)
}
fn greet(name) { "Hello, " + name + "!" }
bind start = do {
  println(fib(15))
  println(greet("Ling"))
}
```
```output
610
Hello, Ling!
```

- Type annotations such as `fn f(a: number) -> text` are accepted and ignored.
- If you pass too few arguments, the missing parameters are undefined. Using one is an
  error on the interpreter and gives `()` on the JIT.
- Calling a function that doesn't exist is an error on the interpreter, but the JIT
  quietly returns `()`. When something "does nothing", run it with `ling run --interp`.

## Closures

A closure is written `|params| expression`, for example `|x| x * 2`. A closure with no
parameters needs a space: `| | 42`, because `||` is the "or" operator.

Calling a closure expression directly works everywhere:

```ling
bind start = do {
  println((|x| x + 1)(5))
}
```
```output
6
```

Closures are the least reliable part of the language today. A closure stored in a
variable can't be called by name on the interpreter, and the JIT can't pass a closure
(or a function name) to a builtin such as `list_map` or `try`. Prefer a named `fn`.
See [Known Limits](limits.md#closures-and-function-values).

## Control flow

### if / else

`if` is an expression. Braces are required and the condition needs no parentheses. With
no `else`, an `if` whose condition is false gives `()`.

```ling
bind start = do {
  bind n = 7
  bind label = if n < 0 { "negative" } else if n == 0 { "zero" } else { "positive" }
  println(label)
}
```
```output
positive
```

Use real booleans in conditions. The engines disagree about `""` and `[]`; see
[Types and Values](types.md#truthiness).

### while

`while condition { body }` repeats while the condition holds. The body shares the
enclosing scope, so it is the loop to use for counters and totals.

```ling
bind start = do {
  bind n = 27
  bind steps = 0
  while n != 1 {
    bind n = if n % 2 == 0 { n / 2 } else { 3 * n + 1 }
    bind steps = steps + 1
  }
  println(steps)
}
```
```output
111
```

### for

`for name in sequence { body }` runs the body once per item of a range or list. A range
`lo..hi` stops before `hi`.

```ling
bind start = do {
  for i in 1..4 { println(i * i) }
  for name in ["ada", "lin"] { println(str_upper(name)) }
}
```
```output
1
4
9
ADA
LIN
```

There is no `break` or `continue`. To stop early, put the loop in a function and
`return` from it, or use a `while` loop with a flag.

### match

`match value { pattern => expression, ... }` tries each arm from top to bottom and
evaluates the first one that fits. Patterns:

| Pattern | Matches |
|---|---|
| `_` | anything |
| `0`, `"yes"`, `true` | that literal value |
| `name` | anything, and binds it to `name` |
| `ok(p)`, `bad(p)` | a result (see [Errors and Results](errors.md)) |
| `Variant(p, ...)`, `Variant()` | an enum value |

```ling
fn classify(n) {
  match n {
    0 => "zero",
    1 => "one",
    other => "many (" + str(other) + ")"
  }
}
bind start = do {
  println(classify(0))
  println(classify(9))
}
```
```output
zero
many (9)
```

If no arm fits, the `match` gives `()`. There are no guards and no patterns for lists
or records.

## Records and enums

`form` declares a record type. Create a record by calling the type's name with the
fields in order, and read a field with `.`:

```ling
form Point { x, y }
bind start = do {
  bind p = Point(3, 4)
  println(p)
  println(p.x + p.y)
}
```
```output
Point { x: 3, y: 4 }
7
```

`choose` declares an enum whose variants may carry values. Build a value by calling a
variant, and take it apart with `match`. A variant without values is still written with
parentheses: `Dot()`. The variant's name is available as `.tag`.

```ling
choose Shape { Circle(r), Rect(w, h), Dot }
fn area(s) {
  match s {
    Circle(r) => 3 * r * r,
    Rect(w, h) => w * h,
    Dot() => 0
  }
}
bind start = do {
  println(area(Circle(2)))
  println(area(Rect(3, 4)))
  println(area(Dot()))
  println(Rect(1, 2).tag)
}
```
```output
12
12
0
Rect
```

Records can't be changed: build a new one instead. `Point { x: 1, y: 2 }` literal
syntax is not supported. Calling a constructor with the wrong number of fields is an
error on the interpreter and gives `()` on the JIT.

## Parse traps

- **A line starting with `-` or `[` continues the previous line.** `bind x = 5`
  followed by a line `-1` makes `x` equal to `4`, and a line starting with `[1, 2]` is
  read as indexing.
- **Range bounds.** `0..n + 1` is a parse error and `0..len(xs)` silently misbehaves.
  Put anything but a plain name or number in parentheses: `0..(n + 1)`, `0..(len(xs))`.
- **Field annotations in `form`.** `form P { x: number, y: number }` is a parse error,
  because an annotation right before `}` swallows the brace. End the list with a comma:
  `form P { x: number, y: number, }`.
- **No assignment.** `x = x + 1` is a parse error. Write `bind x = x + 1`.
- **Empty closure.** Write `| | expr`, not `|| expr`.
