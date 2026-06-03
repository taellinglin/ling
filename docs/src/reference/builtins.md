# Core Builtins Reference

All builtins support multilingual names — see [Builtin Aliases](../multilingual/builtins.md)
for the full international lookup table.

Every alias listed in the **5-language** tables below is accepted by the
interpreter, the lexer/parser, **and** the `lingfu normalize` command, so an
English program normalized to any of these languages runs unchanged. The
interpreter accepts the union of every alias, so you can mix languages in one
file.

## Output · Core

| EN | 🇨🇳 ZH | 🇯🇵 JA | 🇰🇷 KO | 🇹🇭 TH | Description |
|----|--------|--------|--------|--------|-------------|
| `print` / `println` | `印` / `打印` | `印刷` | `출력` | `พิมพ์` | Print value(s) to stdout |
| `format` | `格式` | `フォーマット` | `서식` | `รูปแบบ` | Format string with args |
| `ok` | `好` | `良し` | `좋아` | `โอเค` | Wrap value as `Ok` |
| `bad` / `err` | `坏` | `悪い` | `나쁨` | `ผิด` | Wrap value as `Err` |
| `sleep` | `睡眠` | `眠る` / `スリープ` | `잠자기` / `잠` | `หยุด` / `นอน` | Sleep for N ms |

## Math

All math args and results are `f64`. Angles are in radians.

| EN | 🇨🇳 ZH | 🇯🇵 JA | 🇰🇷 KO | 🇹🇭 TH | Description |
|----|--------|--------|--------|--------|-------------|
| `sin` | `正弦` | `サイン` | `사인` | `ไซน์` | Sine |
| `cos` | `余弦` | `コサイン` | `코사인` | `โคไซน์` | Cosine |
| `tan` | `正切` | `タンジェント` | `탄젠트` | `แทนเจนต์` | Tangent |
| `asin` | `反正弦` | `アークサイン` | `아크사인` | `อาร์กไซน์` | Arcsine |
| `acos` | `反余弦` | `アークコサイン` | `아크코사인` | `อาร์กโคไซน์` | Arccosine |
| `atan` | `反正切` | `アークタンジェント` | `아크탄젠트` | `อาร์กแทนเจนต์` | Arctangent |
| `atan2` | `反正切2` | `アークタンジェント2` | `아크탄젠트2` | — | Two-arg arctangent |
| `tanh` | `双曲正切` | `双曲線正接` | `쌍곡탄젠트` | — | Hyperbolic tangent |
| `sqrt` | `平方根` / `根` | `平方根` | `제곱근` | `รากที่สอง` | Square root |
| `cbrt` | `立方根` | `立方根` | `세제곱근` | `รากที่สาม` | Cube root |
| `pow` | `幂` | `べき乗` | `거듭제곱` | `ยกกำลัง` | x raised to y |
| `exp` | `指数` | `指数関数` | `지수` | — | e^x |
| `hypot` | `斜边` | `斜辺` | `빗변` | — | √(x²+y²) |
| `ln` / `log` | `对数` | `対数` | `로그` | `ลอการิทึม` | Natural log |
| `log2` / `log10` | `对数2` / `对数10` | `対数2` / `対数10` | `로그2` / `로그10` | — | Base-2 / base-10 log |
| `abs` | `绝对值` / `绝对` | `絶対値` | `절댓값` / `절대값` | `ค่าสัมบูรณ์` | Absolute value |
| `floor` | `向下取整` / `下整` | `床関数` | `내림` | `ปัดลง` | Round down |
| `ceil` | `向上取整` / `上整` | `天井関数` | `올림` | `ปัดขึ้น` | Round up |
| `round` | `四舍五入` / `四舍` | `四捨五入` | `반올림` | `ปัดเศษ` | Round to nearest |
| `trunc` / `int` | `取整` / `整数` | `整数化` / `切り捨て` | `정수화` / `정수` | `ตัดทศนิยม` | Truncate toward zero |
| `fract` | `小数部分` | `小数部` | `소수부` | — | Fractional part |
| `min` | `最小` | `最小` | `최솟값` | `ต่ำสุด` | Minimum of two |
| `max` | `最大` | `最大` | `최댓값` | `สูงสุด` | Maximum of two |
| `clamp` | `截取` | `範囲制限` | `범위제한` | `จำกัด` | Clamp to `[lo, hi]` |
| `pi` | `圆周率` | `円周率` | `파이` | `พาย` | π constant |
| `tau` | `双周率` | `タウ` | `타우` | `ทาว` | τ constant (2π) |

> Cells marked `—` have no native alias in that language yet (English still
> works everywhere). Filling these is tracked in [`TODO.md`](https://github.com/taellinglin/ling/blob/master/TODO.md).

## Lists

| Function | Description |
|----------|-------------|
| `list_new()` | Create empty list |
| `list_push(lst, v)` | Append value, return new list |
| `list_get(lst, i)` | Get element at index |
| `list_len(lst)` | Number of elements |
| `list_pop(lst)` | Remove last element |
| `list_set(lst, i, v)` | Set element at index |

## Strings

| Function | Description |
|----------|-------------|
| `str_len(s)` | Length in characters |
| `str_concat(a, b)` | Concatenate strings |
| `str_slice(s, start, end)` | Substring |
| `str_to_num(s)` | Parse as number |
| `num_to_str(n)` | Number to string |
| `str_contains(s, sub)` | Substring test |

## Type checks

| Function | Returns |
|----------|---------|
| `is_num(v)` | true if number |
| `is_str(v)` | true if string |
| `is_list(v)` | true if list |
| `is_bool(v)` | true if boolean |

## Window & display

See [Window & Camera reference](window.md).

## Vector geometry

See [vtex reference](vtex.md).

## Pixel textures

See [tex reference](tex.md).

## Audio

See [Audio reference](audio.md).

## FFT (native only, no-op on WASM)

| Function | Description |
|----------|-------------|
| `fft_push(samples)` | Feed list of f32 samples to analyzer |
| `fft_bands(n)` | Return n log-spaced magnitude bands (0..1) |
| `fft_beat()` | true if current frame energy exceeds average |
| `fft_beat_ratio()` | energy ratio (1.0 = threshold) |
| `fft_rms()` | RMS level of current window |
| `fft_dominant_freq()` | Peak frequency in Hz |
