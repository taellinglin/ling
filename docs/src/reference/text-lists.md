# Text and Lists

Text (strings) and lists are the two everyday collections in Ling. This page covers the builtins
that measure, search, cut and convert text, and the ones that build and read lists. For a gentler
introduction, see the [Lists and Text](../tutorials/lists-text.md) tutorial.

Two rules apply throughout:

- **Builtins never change their input.** They return a new value, so rebind the result:
  `bind xs = list_push(xs, 5)`.
- **Bad input rarely raises an error.** An out-of-range index or unparsable number gives a
  fallback value (`""`, `0`, `-1`) instead. Each entry says which.

## Summary

### Text

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `len(value)` | `长度` | `長さ` | `길이` | `ความยาว` | Number of characters (or list items) |
| `to_str(value)` | — | — | — | `แปลงสตริง` | Any value as text (alias `str`) |
| `to_number(text)` | `转数字` | — | — | — | Parse a number; `0` if invalid |
| `hex_to_number(text)` | `十六进制转数字` | — | — | — | Parse hex digits; `0` if invalid |
| `split(text, sep)` | — | — | — | `แยก` | Split into a list of strings |
| `trim(text)` | — | — | — | `ตัดช่องว่าง` | Remove surrounding whitespace |
| `starts_with(text, prefix)` | — | — | — | `เริ่มด้วย` | Does the text start with `prefix`? |
| `ends_with(text, suffix)` | — | — | — | `ลงท้ายด้วย` | Does the text end with `suffix`? |
| `str_contains(text, part)` | `字符串包含` | — | `문자열포함` | `มีข้อความ` | Does the text contain `part`? |
| `str_find(text, part)` | — | — | — | `หาในสตริง` | Character index of `part`, or `-1` |
| `substr(text, start, length)` | — | — | — | `ส่วนสตริง` | Cut out part of the text |
| `str_replace(text, from, to)` | — | — | — | `แทนสตริง` | Replace every occurrence |
| `str_repeat(text, count)` | — | — | — | `ทำซ้ำสตริง` | Repeat the text |
| `str_upper(text)` | — | — | — | — | Upper case |
| `str_lower(text)` | — | — | — | — | Lower case |
| `str_strip_prefix(text, prefix)` | — | — | — | `ตัดคำนำหน้า` | Remove a prefix if present |
| `chr(code)` | `字符` | — | `문자` | `อักขระ` | Character for a Unicode code point |
| `ord(text)` | `字码` | — | `문자코드` | `รหัสอักขระ` | Code point of the first character |
| `hash_str(text)` | — | — | — | `แฮช` | Stable hash in 0..1 (not cryptographic) |
| `hash_int(text, n)` | — | — | — | `แฮชจำนวน` | Stable hash in 0..n-1 |

### Lists

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `list_new()` | `新建列表` | `新規リスト` | `새목록` | `รายการใหม่` | Empty list |
| `list_push(list, value)` | `列表添加` | `リスト追加` | `목록추가` | `เพิ่มรายการ` | List with `value` added at the end |
| `list_get(list, index)` | `取元素`, `要素取得` | — | `요소가져오기` | `รับรายการ` | Item at `index`; `""` if out of range |
| `list_set(list, index, value)` | `设元素`, `要素設定` | — | `요소설정` | `ตั้งรายการ` | List with one item replaced |
| `list_join(list, sep)` | `连接` | — | `연결` | `รวมรายการ` | Join the items into text (alias `join`) |
| `list_cat(a, b)` | — | — | — | — | Two lists joined together |
| `list_iota(n)` | — | — | — | — | `[0, 1, ..., n-1]` |
| `list_max(list, default)` | `列表最大值` | — | — | — | Largest number, or `default` if empty |
| `list_min(list, default)` | `列表最小值` | — | — | — | Smallest number, or `default` if empty |
| `list_map(list, fn)` | `映射列表` | — | — | — | Apply `fn` to each item (interpreter only) |
| `list_filter(list, fn)` | `过滤列表` | — | — | — | Keep items where `fn` is `true` (interpreter only) |
| `list_find(list, fn)` | `查找列表` | — | — | — | First item where `fn` is `true` (interpreter only) |

## Text

### Joining and converting

`+` joins text, and converts a number on either side to text. `to_str` converts any value
explicitly. `to_number` ignores surrounding whitespace and returns **`0`** for anything it cannot
read, so it cannot tell `"0"` from bad input. It does not read hex; use `hex_to_number` for that.

```ling
bind start = do {
  print("score: " + 10)
  print(to_str(3) + "!")
  print(to_number(" 42 ") + 1, " ", to_number("abc"))
  print(hex_to_number("ff"))
}
```
```output
score: 10
3!
43 0
255
```

### `len(value)`

Counts the characters of a text or the items of a list; anything else gives `0`. The method form
`"text".len()` counts **bytes** instead, so it differs for non-ASCII text.

```ling
bind start = do {
  print(len("Ling"), " ", len([1, 2, 3]))
  print(len("日本"), " ", "日本".len())
}
```
```output
4 3
2 6
```

### Searching

`starts_with`, `ends_with` and `str_contains` return `true` or `false`. `str_find` returns the
0-based character index of the first match, or `-1`. The method `text.contains(part)` works too.

```ling
bind start = do {
  bind word = "hello"
  print(starts_with(word, "he"), " ", ends_with(word, "lo"))
  print(str_contains(word, "ell"), " ", word.contains("z"))
  print(str_find(word, "l"), " ", str_find(word, "z"))
}
```
```output
true true
true false
2 -1
```

### Cutting and changing

`substr(text, start, length)` counts characters from 0; leave out `length` to go to the end.
Out-of-range values are clamped, never an error. `str_replace` replaces every occurrence.

```ling
bind start = do {
  print(substr("hello", 1, 3), " ", substr("hello", 2))
  print(str_replace("a-b-c", "-", "+"))
  print(trim("   padded   ") + "|")
  print(str_upper("Ling"), " ", str_lower("LING"))
  print(str_repeat("ab", 3), " ", str_strip_prefix("v1.2", "v"))
}
```
```output
ell llo
a+b+c
padded|
LING ling
ababab 1.2
```

### `split(text, sep)`

Returns a list of strings. Without a separator (or with `""`) it splits on newlines. It never
trims, so two separators in a row give an empty string.

```ling
bind start = do {
  print(split("one two three", " "))
  print(split("a,b,,c", ","))
  print(len(split("line 1\nline 2")))
}
```
```output
[one, two, three]
[a, b, , c]
2
```

### Characters and hashes

`ord` gives the code point of the first character (`-1` for empty text), and `chr` turns a code
point back into a one-character text. `hash_str` and `hash_int` give the same value for the same
text every run, which is useful for picking a stable colour or variant from a name.

```ling
bind start = do {
  print(ord("A"), " ", chr(97))
  print(hash_str("apple") == hash_str("apple"))
  print(hash_int("apple", 10))
}
```
```output
65 a
true
5
```

## Lists

### Building and reading

Write a list literally as `[1, 2, 3]`, or start from `list_new()` and add items with
`list_push`. Read an item with `xs[i]` (0-based) or `list_get(xs, i)`. The difference is what
happens out of range: `list_get` returns `""`, while `xs[i]` is an error on the interpreter.
`list_set` returns a copy with one item replaced and ignores an out-of-range index.

```ling
bind start = do {
  bind xs = list_new()
  bind xs = list_push(xs, "a")
  bind xs = list_push(xs, "b")
  bind xs = list_set(xs, 0, "z")
  print(xs, " ", len(xs))
  print(xs[1], " ", list_get(xs, 0))
  print(list_get(xs, 9) == "")
}
```
```output
[z, b] 2
b z
true
```

The method `xs.push(value)` does the same as `list_push`. Always rebind the result: the
interpreter leaves the original list unchanged, the default backend may change it in place, and
your program should not depend on either.

### Combining

```ling
bind start = do {
  print(list_join(["a", "b", "c"], ", "))
  print(list_join([1, 2, 3], "-"))
  print(list_cat([1, 2], [3]), " ", list_iota(4))
  print(list_max([3, 9, 2], 0), " ", list_min([], 0 - 1))
}
```
```output
a, b, c
1-2-3
[1, 2, 3] [0, 1, 2, 3]
9 -1
```

### Loops instead of map and filter

`list_map`, `list_filter` and `list_find` take a function, and functions passed to builtins only
work on the interpreter (`ling run --interp`); the default backend passes `()` instead.
`list_filter` keeps an item only when the function returns exactly `true`.

```ling-module
fn doubled(xs) { list_map(xs, |x| x * 2) }
```

There are also no `sort`, `slice` or `reverse` builtins. A `while` loop does all of these and
runs on every backend. Use `while` rather than `for` when you accumulate: a `bind` inside a `for`
body does not reach the outer scope.

```ling
bind start = do {
  bind xs = [3, 1, 2, 4]
  bind total = 0
  bind rev = list_new()
  bind i = len(xs) - 1
  while i >= 0 {
    bind total = total + xs[i]
    bind rev = list_push(rev, xs[i])
    bind i = i - 1
  }
  print(total, " ", rev)
}
```
```output
10 [4, 2, 1, 3]
```
