# Syntax Reference

Ling is an expression-oriented language with a clean, minimal syntax. Every
keyword below has equivalents in all supported lexicons — see
[Keyword Aliases](../multilingual/keywords.md) for the full table.

## Binding (variable declaration)

```ling
令 x = 42
令 name = "Ling"
令 flag = true
```

Re-binding shadows the previous binding in the same scope:

```ling
令 i = 0
令 i = i + 1   # i is now 1
```

## Functions

```ling
函 add(a, b) {
    返 a + b
}
```

Functions are values and can be stored in bindings:

```ling
令 square = 函 (n) { 返 n * n }
令 result = square(7)   # 49
```

## Do blocks (`执`)

A `执` block evaluates all statements and returns the value of the last one.
The entry point of every Ling program is `令 启 = 执 { ... }`.

```ling
令 启 = 执 {
    令 x = 10
    令 y = 20
    印(x + y)
}
```

## Conditionals

```ling
若 x > 0 {
    印("positive")
}

若 x > 0 {
    印("positive")
} 否则 {
    印("non-positive")
}
```

## While loops

```ling
令 i = 0
循 i < 10 {
    印(i)
    令 i = i + 1
}
```

> **Note:** Ling has no unary minus operator. Write `0 - x` instead of `-x`.

## Lists

```ling
令 lst = list_new()
令 lst = list_push(lst, 42)
令 lst = list_push(lst, "hello")
令 n   = list_len(lst)         # 2
令 v   = list_get(lst, 0)      # 42
```

## Comments

```ling
# This is a comment
令 x = 1   # inline comment
```

## Arithmetic and comparison

```ling
令 sum  = a + b
令 diff = a - b
令 prod = a * b
令 quot = a / b
令 rem  = a % b
令 eq   = a == b
令 neq  = a != b
令 lt   = a < b
令 lte  = a <= b
令 gt   = a > b
令 gte  = a >= b
```

## Built-in math functions

```ling
sin(x)   cos(x)   tan(x)
asin(x)  acos(x)  atan2(y, x)
sqrt(x)  abs(x)   floor(x)   ceil(x)
log(x)   exp(x)   pow(x, y)
```
