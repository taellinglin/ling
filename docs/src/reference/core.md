# Output, Math and Logic

These builtins print to the console, build text, do arithmetic and pause the program.
They need no window, so every example on this page runs in a plain terminal with `ling run`.

All numbers in Ling are 64-bit floating point. Whole values print without a decimal point
(`3`, not `3.0`). Angles are in radians. A missing trailing numeric argument usually takes a
default instead of raising an error.

## Summary

### Output and results

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `print(value, ...)` | `印`, `打印` | `印刷` | `출력` | `พิมพ์` | Print the arguments, joined with no separator, then a newline |
| `println(value, ...)` | `印`, `打印` | `印刷` | `출력` | `พิมพ์` | Same builtin as `print` |
| `print_file(text)` | — | — | — | `พิมพ์ไฟล์` | Print text without a newline |
| `print_color(color, value, ...)` | — | — | — | `พิมพ์สี` | Print a line in a bright ANSI colour |
| `format(template, value, ...)` | `格式` | `フォーマット` | `서식` | `รูปแบบ` | Fill `{}` placeholders in a template |
| `format::join(list)` | `格式::拼接` | — | — | — | Concatenate list items as text |
| `ok(value)` | `好` | `良し` | `좋아` | `โอเค` | Wrap a success result |
| `bad(reason)` | `坏` | `悪い` | `나쁨` | `ผิด` | Wrap a failure result (alias `err`) |
| `try(function, arg, ...)` | — | — | — | — | Call a function, turning a runtime error into `bad` |
| `sleep(ms)` | `睡眠` | `眠る`, `スリープ` | `잠자기`, `잠` | `นอน` | Pause for a number of milliseconds |

### Math

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `abs(x)` | `绝对值`, `绝对` | `絶対値` | `절댓값`, `절대값` | `ค่าสัมบูรณ์` | Absolute value |
| `floor(x)` | `向下取整`, `下整` | `床関数` | `내림` | `ปัดลง` | Round down |
| `ceil(x)` | `向上取整`, `上整` | `天井関数` | `올림` | `ปัดขึ้น` | Round up |
| `round(x)` | `四舍五入`, `四舍` | `四捨五入` | `반올림` | `ปัดเศษ` | Round to nearest; halves go away from zero |
| `trunc(x)` | `取整`, `截整` | `整数化`, `切り捨て` | `정수화`, `버림` | `ตัดทศนิยม` | Drop the fraction (alias `int`) |
| `fract(x)` | `小数部分` | — | `소수부` | — | Fractional part |
| `min(a, b)` | `最小` | `最小` | `최솟값` | `ต่ำสุด` | Smaller of two numbers |
| `max(a, b)` | `最大` | `最大` | `최댓값` | `สูงสุด` | Larger of two numbers |
| `clamp(x, low, high)` | `截取` | `範囲制限` | `범위제한` | `จำกัด` | Limit `x` to `low..high` |
| `sign(x)` | `符号` | `符号関数` | `부호` | `เครื่องหมาย` | `-1` for negative, otherwise `1` |
| `sqrt(x)` | `根` | `平方根` | `제곱근` | `รากที่สอง` | Square root |
| `cbrt(x)` | `立方根` | — | `세제곱근` | `รากที่สาม` | Cube root |
| `pow(base, exp)` | `幂` | `べき乗` | `거듭제곱` | `ยกกำลัง` | Power |
| `exp(x)` | `指数` | — | `지수` | — | e to the power `x` |
| `ln(x)` | `对数` | `対数` | `로그` | `ลอการิทึม` | Natural logarithm (alias `log`) |
| `log2(x)`, `log10(x)` | `对数2`, `对数10` | — | `로그2`, `로그10` | — | Base-2 and base-10 logarithms |
| `hypot(x, y)` | `斜边` | — | `빗변` | — | `sqrt(x*x + y*y)` |
| `sin(x)`, `cos(x)`, `tan(x)` | `正弦`, `余弦`, `正切` | `サイン`, `コサイン`, `タンジェント` | `사인`, `코사인`, `탄젠트` | `ไซน์`, `โคไซน์`, `แทนเจนต์` | Trigonometry (radians) |
| `asin(x)`, `acos(x)`, `atan(x)` | `反正弦`, `反余弦`, `反正切` | `アークサイン`, `アークコサイン`, `アークタンジェント` | `아크사인`, `아크코사인`, `아크탄젠트` | `อาร์กไซน์`, `อาร์กโคไซน์`, `อาร์กแทนเจนต์` | Inverse trigonometry |
| `atan2(y, x)` | `反正切2` | `アークタンジェント2` | `아크탄젠트2` | — | Angle of the point `(x, y)` |
| `tanh(x)` | `双曲正切` | — | `쌍곡탄젠트` | — | Hyperbolic tangent |
| `pi()` | `圆周率` | `円周率` | `파이` | `พาย` | 3.14159... |
| `tau()` | `双周率` | `タウ` | `타우` | `ทาว` | 2 × pi |
| `lerp(a, b, t)` | `线性插值` | `線形補間` | `선형보간` | `ค่าระหว่าง` | `a + (b - a) * t` |
| `smoothstep(low, high, x)` | `平滑步进` | `スムーズステップ` | `스무스스텝` | `เปลี่ยนแบบนุ่ม` | Smooth 0-to-1 S-curve |
| `vnoise(x, y, seed)` | `柏林噪声2D` | `バリューノイズ2D` | `값노이즈2D` | `นอยส์2ดี` | Smooth 2D value noise, 0..1 |
| `fbm(x, y, octaves, seed)` | `分形噪声` | `フラクタルノイズ` | `프랙탈노이즈` | `นอยส์ออร์แกนิก` | Layered (fractal) noise |
| `perlin(x, y, z)` | `柏林噪声3D` | `パーリンノイズ3D` | `펄린노이즈3D` | `เพอร์ลิน3ดี` | 3D Perlin noise, about -1..1 |
| `rand()` | `随机` | `乱数` | `난수` | `สุ่ม` | Random number in 0 up to 1 |

## Output

### `print(value, ...)` and `println(value, ...)`

`print` and `println` are the same builtin. Each argument is converted to text, the pieces are
joined **with no separator**, and a newline is added. Lists print as `[1, 2, 3]`, results as
`Ok(v)` and `Err(e)`.

```ling
bind start = do {
  print("Hello, Ling!")
  print(42, " ", 3.5, " ", true)
  print([1, 2, 3])
  print("sum", 1 + 2)
  print("sum ", 1 + 2)
}
```
```output
Hello, Ling!
42 3.5 true
[1, 2, 3]
sum3
sum 3
```

Put spaces inside the strings yourself, or build the line with `format`.

### `print_file(text)`

Prints its first argument **without** a newline. Despite the name, it does not touch the file
system. It is the only way to keep printing on the same line.

```ling
bind start = do {
  for i in 1..4 {
    print_file(i)
    print_file(" ")
  }
  print("go!")
}
```
```output
1 2 3 go!
```

### `print_color(color, value, ...)`

Prints a line in a bold, bright ANSI colour. `color` is 0 to 7: black, red, green, yellow, blue,
magenta, cyan, white. The remaining arguments are joined like `print`. The escape codes are
written as-is, so they show up as control characters if you redirect the output to a file.

```ling-gui
bind start = do {
  print_color(2, "ok: ", "all tests passed")
  print_color(1, "error!")
}
```

### `format(template, value, ...)`

Returns text with each `{}` replaced by the next argument. `{:.N}` formats a number with `N`
decimal places. A `{}` with no matching argument becomes empty; extra arguments are ignored.
Only `{}` and `{:.N}` are supported (no widths, no `{0}` or `{name}`, no escaped braces).

```ling
bind start = do {
  print(format("{} + {} = {}", 2, 3, 2 + 3))
  print(format("pi is about {:.3}", pi()))
  print(format("{:.1}%", 45.678))
  print(format("{} and {}", "only one"))
}
```
```output
2 + 3 = 5
pi is about 3.142
45.7%
only one and 
```

`format::join(list)` concatenates every item as text with no separator. To put something between
the items, use `list_join` (see [Text and Lists](text-lists.md)).

## Results

### `ok(value)` and `bad(reason)`

Ling has no exceptions. A function that can fail returns `ok(value)` or `bad(reason)`, and the
caller unpacks it with `match`. They print as `Ok(...)` and `Err(...)`; `err` is an English alias
of `bad`. See [Errors and Results](../language/errors.md) for the full story.

```ling
fn half(n) {
  if n % 2 == 0 { ok(n / 2) } else { bad("odd number") }
}
bind start = do {
  print(half(10))
  print(half(7))
  print(match half(7) { ok(v) => v, bad(e) => "failed: " + e })
}
```
```output
Ok(5)
Err(odd number)
failed: odd number
```

### `try(function, arg, ...)`

Calls `function` with the remaining arguments. Returns `ok(result)`, or `bad(message)` if the call
raised a runtime error.

> **Interpreter only.** The default backend passes functions and closures given as arguments as
> `()`, so `try` returns `Err(try: first argument must be a function)`. Run programs that use
> `try` with `ling run --interp`. A closure with no parameters is written `| | expr`, with a space.

```ling-module
fn safe_read(path) { try(| | read_file(path)) }
```

## Math

### Rounding

`floor` rounds down, `ceil` rounds up, `round` rounds to the nearest whole number (halves go away
from zero), `trunc` drops the fraction, and `fract` keeps only the fraction. `int` is an English
alias of `trunc`: it does not convert text to a number (use `to_number` for that).

```ling
bind start = do {
  print(floor(2.7), " ", floor(0 - 2.1))
  print(ceil(2.1), " ", ceil(0 - 2.9))
  print(round(2.5), " ", round(2.4), " ", round(0 - 2.5))
  print(trunc(2.9), " ", trunc(0 - 2.9), " ", int(7.99))
  print(fract(2.25))
}
```
```output
2 -3
3 -2
3 2 -3
2 -2 7
0.25
```

### `min`, `max`, `clamp`, `sign`, `abs`

`min` and `max` take exactly two numbers; for a list use `list_min` and `list_max`. `clamp`
defaults to the range 0..1 and swaps the bounds if `low > high`. `sign(0)` is `1`, not `0`.

```ling
bind start = do {
  print(min(3, 1), " ", max(3, 1))
  print(clamp(15, 0, 10), " ", clamp(0 - 3, 0, 10))
  print(sign(0 - 4), " ", sign(0), " ", abs(0 - 7))
}
```
```output
1 3
10 0
-1 1 7
```

### Powers, roots and logarithms

`sqrt` of a negative number is `NaN`. `log` is the **natural** logarithm (same as `ln`); use
`log10` for base 10.

```ling
bind start = do {
  print(sqrt(16), " ", pow(2, 10), " ", pow(9, 0.5))
  print(cbrt(27), " ", hypot(3, 4), " ", exp(1))
  print(ln(exp(2)), " ", log(1), " ", log2(8), " ", log10(1000))
  print(sqrt(0 - 1))
}
```
```output
4 1024 3
3 5 2.718281828459045
2 0 3 3
NaN
```

### Trigonometry and constants

Angles are in radians. Always call `pi()` and `tau()` with parentheses: a bare `pi` is `()` under
the default backend.

```ling
bind start = do {
  print(sin(pi() / 2), " ", cos(pi()))
  print(atan2(1, 1) * 4)
  print(tau())
}
```
```output
1 -1
3.141592653589793
6.283185307179586
```

### Interpolation and noise

`lerp(a, b, t)` does not clamp `t`. `smoothstep(low, high, x)` returns 0 below `low`, 1 above
`high`, and a smooth curve between. The noise functions are deterministic: the same arguments
always give the same value, which makes them handy for terrain and textures.

```ling
bind start = do {
  print(lerp(0, 10, 0.25), " ", smoothstep(0, 1, 0.5), " ", smoothstep(0, 1, 2))
  print(vnoise(1.5, 2.5, 0) == vnoise(1.5, 2.5, 0))
  print(fbm(0.3, 0.7, 4, 1) >= 0)
}
```
```output
2.5 0.5 1
true
true
```

### `rand()`

Returns a pseudo-random number from 0 up to (not including) 1. There is no seed and no integer
form, so scale and round it yourself. It is not for security: use `random_hex` (see
[Files, Time and System](files.md)) for unpredictable values.

```ling
bind start = do {
  bind die = floor(rand() * 6) + 1
  print(die >= 1 && die <= 6)
}
```
```output
true
```

## Logic and comparison

Logic and comparison are operators, not builtins: `==`, `!=`, `<`, `<=`, `>`, `>=`, `&&`, `||`
and `!`. Numbers, text and booleans compare by value. Two gotchas: lists are never equal under
`==` (compare their items instead), and ordering text with `<` is not reliable.

```ling
bind start = do {
  print(1 < 2 && !(3 == 4))
  print("a" == "a", " ", 2 != 3)
  print([1, 2] == [1, 2])
}
```
```output
true
true true
false
```

## Pausing

### `sleep(ms)`

Pauses the program for the given number of milliseconds (`sleep_ms` is an English alias). To
measure time, see `time_now` in [Files, Time and System](files.md).

```ling
bind start = do {
  sleep(50)
  print("done")
}
```
```output
done
```
