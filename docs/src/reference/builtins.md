# Core Builtins Reference

All builtins support multilingual names — see [Builtin Aliases](../multilingual/builtins.md)
for the full international lookup table.

## Output

| English | Thai | Chinese | Description |
|---------|------|---------|-------------|
| `print(v)` | `พิมพ์(v)` | `印(v)` | Print value to stdout |

## Math

| Function | Description |
|----------|-------------|
| `sin(x)` `cos(x)` `tan(x)` | Trigonometry |
| `asin(x)` `acos(x)` `atan2(y,x)` | Inverse trig |
| `sqrt(x)` | Square root |
| `abs(x)` | Absolute value |
| `floor(x)` `ceil(x)` `round(x)` | Rounding |
| `log(x)` `exp(x)` | Natural log / exp |
| `pow(x,y)` | x raised to y |
| `min(a,b)` `max(a,b)` | Min/max |
| `clamp(v,lo,hi)` | Clamp to range |

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
