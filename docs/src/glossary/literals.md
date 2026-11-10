# Literals and Operators

Literals and operators are written the same way in every language, except `true` and
`false`, which have their own spellings.

## Literals

| Literal | Meaning |
|---|---|
| `42`, `3.14` | Number. Every number is a 64-bit float. There is no hex or exponent form. |
| `"hello"` | Text. Double quotes only. Escapes: `\n` `\t` `\r` `\"` `\\` `\0`. No interpolation. |
| `true`, `false` | Boolean |
| `()` | Unit, the "nothing" value |
| `[1, "two", 3.0]` | List; mixed types, indexed from 0 |
| `0..5` | Range, half-open: 0, 1, 2, 3, 4. Used with `for`. |
| `\|x\| x + 1` | Function value (closure) |
| `ok(v)`, `bad(e)` | Result values |

There are no character, map or tuple literals.

## Booleans in each language

| English | 中文 | 日本語 | 한국어 | ไทย |
|---|---|---|---|---|
| `true` | `真` | `真` | `참` | `จริง` |
| `false` | `假` | `偽` | `거짓` | `เท็จ` |

## Operators

From the tightest binding to the loosest:

| Operator | Meaning |
|---|---|
| `.` `()` `[]` `::` `..` | Field or method, call, index, path, range |
| `-x` `!x` | Negate, logical not |
| `*` `/` `%` | Multiply, divide, remainder |
| `+` `-` | Add, subtract. `+` also joins text. |
| `==` `!=` `<` `>` `<=` `>=` | Compare |
| `&&` | Logical and |
| `\|\|` | Logical or |

Other symbols: `=` in `bind name = value`, `=>` between a pattern and its result in `match`,
and `->` before a return type (parsed and ignored). In the interpreter, `&&` and `||` always
evaluate both sides. See [Known Limits](../language/limits.md).
