# Types and Values

Ling is dynamically typed. You never declare a type: every value carries its own, and
operators and builtins check it when they run. This page lists the kinds of values,
how they print, how they behave in conditions and comparisons, and how to convert
between them.

## The kinds of values

| Type | Examples | Created by | Prints as |
|---|---|---|---|
| number | `3`, `2.5`, `-7` | number literals, arithmetic | `3`, `2.5` |
| text | `"abc"` | text literals, `+`, text builtins | `abc` |
| bool | `true`, `false` | literals, comparisons | `true` |
| unit | `()` | `()`, functions with no result, failed lookups | `()` |
| list | `[1, "a", true]` | list literals, `list_new`, list builtins | `[1, a, true]` |
| record | `Point(3, 4)` | calling a `form` name | `Point { x: 3, y: 4 }` |
| enum value | `Circle(2)`, `Dot()` | calling a `choose` variant | `Circle(2)` |
| result | `ok(5)`, `bad("e")` | `ok` and `bad` | `Ok(5)`, `Err(e)` |
| function | `\|x\| x + 1` | closures | |

There is no map or dictionary, no set, no tuple, no character type and no `null`.
Use lists, records, or parallel lists for key-value data, or store it in SQLite with
the `db_*` builtins. Type annotations such as `x: number` and `-> text` are accepted by
the parser and ignored.

```ling
form Point { x, y }
bind start = do {
  println(42)
  println("text")
  println(true)
  println(())
  println([1, "a", true])
  println(Point(3, 4))
  println(ok(5))
  println(bad("no"))
}
```
```output
42
text
true
()
[1, a, true]
Point { x: 3, y: 4 }
Ok(5)
Err(no)
```

### Numbers

Every number is a 64-bit float. Whole numbers print without a decimal point, whole
numbers up to 2^53 are exact, and division always gives the exact quotient.

```ling
bind start = do {
  println(7 / 2)
  println(10 / 5)
  println(7 % 3)
  println(0.1 + 0.2)
  println(1000000 * 1000000)
}
```
```output
3.5
2
1
0.30000000000000004
1000000000000
```

Dividing by zero is not an error. On the interpreter `1 / 0` is `inf`; on the JIT it is
`()`. On both, `0 / 0` is `NaN`.

### Text

Text is a sequence of Unicode characters. `len` counts characters, not bytes, and `+`
joins text. If one side of `+` is text, the other side is turned into text.

```ling
bind start = do {
  println(len("héllo"))
  println("a" + "b")
  println("n=" + 5)
  println(5 + "!")
}
```
```output
5
ab
n=5
5!
```

Reading one character with `s[i]` works on the interpreter only. Use
`substr(s, i, 1)` to get a character on both engines.

### Lists

A list holds values of any type, indexed from 0. List builtins return a new list, so
always bind the result: `bind xs = list_push(xs, v)`. Reading past the end gives an
empty value on the JIT and stops the interpreter with `index N out of bounds`. See
[Text and Lists](../reference/text-lists.md) for the list builtins.

### Unit

`()` means "nothing". A function whose last expression gives no value returns `()`,
as does an `if` without `else` whose condition is false, and a `match` where no arm
fits.

### Records, enum values and results

Records come from `form`, enum values from `choose`, and results from `ok` and `bad`.
Read a record field with `.name`, the variant name of an enum value with `.tag`, and
open a result with `match`. See [Records and enums](syntax.md#records-and-enums) and
[Errors and Results](errors.md).

## Truthiness

`if` and `while` should be given real booleans. Other values work, but the two engines
don't fully agree:

| Value in a condition | JIT (default) | Interpreter |
|---|---|---|
| `true` / `false` | true / false | true / false |
| `0` | false | false |
| `()` | false | false |
| other numbers, non-empty text, non-empty lists | true | true |
| `""` (empty text) | **true** | false |
| `[]` (empty list) | **true** | false |

Test emptiness explicitly with `len(xs) == 0` or `s == ""`. Also note that `!x` means
`x == false`, so `!0` and `!""` are both `false`. Use `!` on booleans only.

## Comparing values

`==` and `!=` compare numbers, text, booleans and `()` by value. Values of different
types are never equal: `1 == "1"` is `false`.

**Lists, records, enum values and results are never equal**, even with identical
contents. Compare their elements or fields yourself.

```ling
form P { x }
bind start = do {
  println("abc" == "abc")
  println(1 == "1")
  println([1, 2] == [1, 2])
  println(P(1) == P(1))
  println(P(1).x == P(1).x)
}
```
```output
true
false
false
false
true
```

`<`, `>`, `<=` and `>=` are for numbers. Ordering text (`"a" < "b"`) gives `false` on
the JIT and is an error on the interpreter.

## Conversions

| Builtin | Converts | Notes |
|---|---|---|
| `str(v)` | any value to text | Same text that `println` shows. Also called `to_str`. |
| `to_number(s)` | text to number | Ignores surrounding spaces. Returns `0` if the text isn't a number. |
| `hex_to_number(s)` | hex text to number | No `0x` prefix: `hex_to_number("ff")` is `255`. |
| `int(x)` | number to whole number | Cuts off the fraction, toward zero. Also called `trunc`. |
| `floor(x)` / `ceil(x)` | number to whole number | Round down / up. |
| `round(x)` | number to whole number | Halves round away from zero. |

```ling
bind start = do {
  println(str(3) + "!")
  println(to_number(" 4 ") + 1)
  println(to_number("abc"))
  println(int(-3.7))
  println(floor(-2.5))
  println(round(2.5))
  println(hex_to_number("ff"))
}
```
```output
3!
5
0
-3
-3
3
255
```

There is no builtin that tells you a value's type, and no builtin that turns a value
into a boolean. Because `to_number` returns `0` for bad input, check the text first if
`0` is a valid answer. [Errors and Results](errors.md#validating-input) shows how.
