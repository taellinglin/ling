# Keywords

Every Ling keyword has an English spelling and spellings in Chinese, Japanese, Korean
and Thai. The lexer turns each spelling into the same token, so you can write a whole
program in one language or mix them. This page lists every keyword, its spellings, and
what it really does today.

Some keywords are reserved for future features and do nothing yet. They are marked
below, so you won't be surprised when `stop` doesn't leave a loop.

## Spellings

Where a cell has more than one spelling, all of them work. The first one is the usual
choice. These spellings come from the lexer (`src/lexer/mod.rs`), which is what the
runtime actually accepts. In a few places it differs from `lexicons/*.ling`, and the
lexer wins: for example Japanese `choose` is `選択`, Korean `form` is `구조`, and Thai
`form` is `โครงสร้าง`.

| English | 中文 | 日本語 | 한국어 | ไทย |
|---|---|---|---|---|
| `bind` | `令` `灵符` | `束縛` `バ` | `바인드` `묶` | `ผูก` |
| `do` | `执` | `実行` `執` | `실행` | `ทำ` |
| `fn` | `函` | `関数` `関` | `함수` | `ฟังก์ชัน` |
| `if` | `若` `如` | `もし` | `만약` `조건` | `ถ้า` |
| `else` | `否` `否则` | `他` | `아니면` | `มิฉะนั้น` |
| `while` | `循` `当` | `間` `一方` | `동안` `반복` | `ขณะที่` |
| `for` | `历` | `繰` `ために` | `위해` | `สำหรับ` |
| `in` | `于` | `の中` | `안에` | `ใน` |
| `match` | `配` | `一致` | `매치` | `จับคู่` |
| `return` | `归` | `戻る` `帰る` | `반환` `귀환` | `คืน` |
| `form` | `形` `形式` | `構造` | `구조` | `โครงสร้าง` |
| `choose` | `选` `选择` | `選択` | `선택` | `เลือกแบบ` |
| `mod` | `核` | `モジュール` `模` | `모듈` | `โมดูล` |
| `use` | `载` `引` | `使う` | `사용` | `ใช้` `นำเข้า` |
| `as` | `为` | `として` | `로서` | `เป็น` |
| `ok` | `好` `可` | `良い` | `좋아` | `ตกลง` |
| `bad` | `坏` `误` | `悪い` | `나쁨` | `ผิดพลาด` |
| `true` | `真` | `真` | `참` | `จริง` |
| `false` | `假` | `偽` | `거짓` | `เท็จ` |
| `try` | `试` `尝试` | `試す` | `시도` | `ลอง` |
| `type` | `符` | `型` | `타입` | `ชนิด` |
| `where` | `条件` | `但し` | `단` | `โดยที่` |
| `async` | `异` `异步` | `非同期` | `비동기` | `ไม่พร้อมกัน` |
| `wait` | `待` `等待` | `待つ` | `기다려` | `รอ` |
| `own` | `独` `拥有` | `所有` | `소유` | `เป็นเจ้าของ` |
| `lend` | `借` | `貸す` | `빌려` | `ให้ยืม` |
| `share` | `共` `共享` | `共有` | `공유` | `แบ่งปัน` |
| `move` | `移` `移动` | `移動` | `이동` | `ย้าย` |
| `copy` | `复` `复制` | `複製` | `복사` | `คัดลอก` |
| `stop` | `止` | `停止` `止まれ` `停め` | `멈춤` | `หยุด` |
| `again` | `继续` | `継続` | `계속` | `ทำอีก` |
| `change` | `变` `改变` | `変える` | `변경` | `เปลี่ยนแปลง` |
| `post` | `出` `发布` | `投稿` | `게시` | `ส่ง` |
| `give` | `予` `给` | `渡す` | `전달` | `ให้` |
| `fit` | `适合` | `適合` | `적합` | `เหมาะสม` |
| `can` | `能` | `できる` | `가능` | `สามารถ` |
| `sure` | `确` `确定` | `確か` | `확실` | `แน่นอน` |
| `maybe` | `或` `可能` | `多分` | `아마도` | `อาจจะ` |
| `pure` | `纯` | `純粋` | `순수` | `บริสุทธิ์` |
| `spawn` | `启` `生成` | `起動` | `생성` | `สร้าง` |
| `none` | `无` | `なし` | `없음` | `ไม่มี` |

`asm` has an English spelling only.

## What each keyword does

| Keyword | Status | What it does |
|---|---|---|
| `bind` | active | Creates or replaces a binding. Also defines globals and the entry point. |
| `do` | active | A block whose value is its last statement. |
| `fn` | active | Declares a function. |
| `if` / `else` | active | Conditional expression. |
| `while` | active | Loop while a condition holds. The body shares the outer scope. |
| `for` / `in` | active | Loop over a range or list. Each pass gets a fresh scope. |
| `match` | active | Pattern matching. |
| `return` | active | Leaves the current function with a value. |
| `form` | active | Declares a record type. |
| `choose` | active | Declares an enum type with optional payloads. |
| `mod` | active | Groups functions into a namespace (`name::f()`). |
| `use` / `as` | active | Loads another file, optionally under an alias. |
| `ok` / `bad` | active | Build a result, and match one as a pattern. |
| `true` / `false` | active | The boolean values. |
| `try` | builtin | Not syntax: the builtin `try(f, args...)`. Works on the interpreter only. |
| `type` | ignored | `type Name as T` is parsed and thrown away. It swallows what follows it, so avoid it. |
| `where` | ignored | `fn f(x) where ... { }`: everything up to `{` is skipped. |
| `async` | ignored | `async fn` runs synchronously, like a normal function. |
| `wait` | unreliable | Gives the value on the interpreter but `()` on the JIT. Don't use it. |
| `own` `lend` `share` `move` `copy` | ignored | Prefix words that return their operand unchanged. No ownership checks happen. |
| `asm` | ignored | `asm("...")` is meant for bare-metal builds. In a normal run it does nothing useful. |
| `stop` | reserved | Does **not** break out of a loop. |
| `again` | reserved | Does **not** continue a loop. |
| `change` | reserved | Does nothing. Rebind with `bind` instead. |
| `post` `give` `fit` `can` `sure` `maybe` `pure` | reserved | Do nothing. They are ordinary names to the parser. |
| `spawn` | reserved | Does nothing, and there is no `spawn` builtin. |
| `none` | reserved | Not a value. The interpreter reports `undefined: 'none'`. Use `()` for "nothing". |

**Reserved** words are ordinary names to the parser, so `bind stop = 2` is legal. On
the interpreter, a bare `stop` or `again` in a loop is an `undefined` error; on the JIT
it is silently ignored and the loop keeps going.

## Active keywords

### bind

`bind name = expression` is the only way to create or "change" a variable. Binding a
name again replaces it. See [Bindings](syntax.md#bindings-and-rebinding).

```ling
bind start = do {
  bind price = 40
  bind price = price + 2
  println(price)
}
```
```output
42
```

### do

`do { ... }` runs statements in order and gives the value of the last one. The entry
point is a `do` block bound to `start`. See [Blocks](syntax.md#blocks).

### fn and return

`fn name(params) { body }` declares a function. The last expression is the result, and
`return` leaves early.

```ling
fn describe(n) {
  if n > 100 { return "big" }
  "small"
}
bind start = do {
  println(describe(500))
  println(describe(5))
}
```
```output
big
small
```

A `return` directly inside `start` ends the program quietly on the JIT and is an error
(`unexpected top-level return`) on the interpreter.

### if, else, while, for, in

The control-flow keywords are described in [Control flow](syntax.md#control-flow).
There is no `break` or `continue`: `stop` and `again` are reserved words only. To leave
a loop early, `return` from a function:

```ling
fn find(xs, target) {
  for x in xs {
    if x == target { return true }
  }
  false
}
bind start = do {
  println(find([1, 2, 3], 2))
  println(find([1, 2, 3], 9))
}
```
```output
true
false
```

### match

`match value { pattern => expression, ... }` evaluates the first arm whose pattern
fits. Patterns are `_`, literals, a name, `ok(p)` / `bad(p)` and enum variants. See
[match](syntax.md#match).

### form and choose

`form` declares a record type and `choose` an enum. See
[Records and enums](syntax.md#records-and-enums).

```ling
choose Light { Red, Yellow, Green }
fn action(l) {
  match l {
    Red() => "stop",
    Yellow() => "slow",
    Green() => "go"
  }
}
bind start = do {
  println(action(Green()))
  println(Red().tag)
}
```
```output
go
Red
```

### mod, use, as

`mod name { ... }` groups functions, called as `name::f()`. `use "file.ling"` loads
another file, and `use "file.ling" as m` makes its functions available as `m::f()`.
See [Modules and Projects](modules.md).

### ok and bad

`ok(value)` and `bad(reason)` build results. In a `match` they are patterns. A `bad`
result prints as `Err(...)`. See [Errors and Results](errors.md).

```ling
fn half(n) {
  if n % 2 == 0 { ok(n / 2) } else { bad("odd number") }
}
bind start = do {
  println(half(10))
  println(match half(7) { ok(v) => v, bad(e) => "error: " + e })
}
```
```output
Ok(5)
error: odd number
```

### true and false

The boolean literals. They are the values `if` and `while` should be given.

## Keywords that are parsed but ignored

The ownership words `own`, `lend`, `share`, `move` and `copy` can stand in front of an
expression (or a parameter name) and are then ignored. There is no borrow checker, and
using a value after `move` is fine:

```ling
bind start = do {
  bind a = own [1, 2, 3]
  bind b = move a
  println(a)
  println(b)
}
```
```output
[1, 2, 3]
[1, 2, 3]
```

`async fn` declares an ordinary function that runs immediately when called. `where`
clauses after a function's parameters are skipped:

```ling
async fn work(n) { n * 2 }
fn next(x) where x { x + 1 }
bind start = do {
  println(work(4))
  println(next(2))
}
```
```output
8
3
```

For real parallel work, use the `thread_*` builtins.

## Words that look like keywords but aren't

- `start`, `main`, `启`, `เริ่ม` and `시작` are names, not keywords. The runtime looks
  for them to find the entry point. See [The entry point](syntax.md#the-entry-point).
- `result`, `loop`, `break`, `continue`, `struct`, `enum`, `let`, `const` and similar
  words appear in some lexicon files but are not keywords. Ling has no `loop` and no
  `let`: use `while` and `bind`.
- `number`, `text`, `bool`, `list`, `map` and `tuple` are documentation words only.
  Ling is dynamically typed. See [Types and Values](types.md).
