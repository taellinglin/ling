# Functions

A function is a named recipe: you write the steps once and use them as often as you
like. You have already used builtin functions such as `println`, `round` and `str`. In this
lesson you will write your own with `fn`, give them parameters, get answers back with
`return`, and let a function call itself.

## Defining and calling a function

```ling
fn greet(name) {
  println("Hello, " + name + "!")
}

bind start = do {
  greet("Ada")
  greet("Grace")
  greet("Mina")
}
```
```output
Hello, Ada!
Hello, Grace!
Hello, Mina!
```

- `fn` says "I am defining a function".
- `greet` is the function's name.
- `name` in parentheses is a **parameter**: a blank that is filled in each time you call
  the function.
- The braces hold the **body**, the steps the function carries out.

Defining a function doesn't run it. The body runs only when you **call** the function,
as in `greet("Ada")`. The value in the parentheses is the **argument**; it becomes the value
of `name` for that call.

Functions live at the top level of the file, outside `bind start`. The instructions that
actually do things live inside `start`.

## Getting answers back with `return`

Often you want a function to compute something and hand the answer back. That is what
`return` does. A function can take several parameters, separated by commas:

```ling
fn square(n) {
  return n * n
}

fn average(a, b) {
  return (a + b) / 2
}

bind start = do {
  println(square(7))
  println(average(10, 25))
  println(square(average(2, 4)))
}
```
```output
49
17.5
9
```

Because `square(7)` has a value, you can use it anywhere a value fits: inside `println`,
inside a bigger expression, or as the argument of another call. `square(average(2, 4))`
is worked out from the inside: `average(2, 4)` is 3, and `square(3)` is 9.

### Early returns

The first `return` that runs ends the function. Use that to handle special cases first:

```ling
fn describe(n) {
  if n < 0 {
    return "negative"
  }
  if n == 0 {
    return "zero"
  }
  return "positive"
}

fn cube(n) { n * n * n }

bind start = do {
  println(describe(-5))
  println(describe(0))
  println(describe(12))
  println(cube(3))
}
```
```output
negative
zero
positive
27
```

No `else` is needed in `describe`, because a `return` already leaves the function. `cube`
shows a shortcut: without `return`, a function gives back the value of its last
expression. A function that computes nothing, like `greet`, gives back `()`, the "nothing"
value.

## What a function can see

Each call gets its own private names: its parameters and anything it binds. They can't
clash with names in the code that called it:

```ling
bind tax_rate = 0.2

fn with_tax(price) {
  bind total = price + price * tax_rate
  return total
}

bind start = do {
  bind total = 99
  println(with_tax(50))
  println("my total is still ", total)
}
```
```output
60
my total is still 99
```

A function can see bindings at the top level of the file, such as `tax_rate`. That is a
good place for constants. It **can't** see the local names of the function that called it,
so pass what it needs as arguments.

## Functions that call themselves

A function may call itself. This is **recursion**. It needs two parts:

1. A **base case**, so simple that the answer is known directly.
2. A **recursive case**, which solves a smaller version of the problem by calling itself.

The factorial of 5 is `5 * 4 * 3 * 2 * 1`, which is the same as `5 * factorial(4)`:

```ling
fn factorial(n) {
  if n <= 1 {
    return 1
  }
  return n * factorial(n - 1)
}

fn fib(n) {
  if n < 2 {
    return n
  }
  return fib(n - 1) + fib(n - 2)
}

bind start = do {
  println("5! = ", factorial(5))
  println("10! = ", factorial(10))
  for i in 0..10 {
    println("fib(", i, ") = ", fib(i))
  }
}
```
```output
5! = 120
10! = 3628800
fib(0) = 0
fib(1) = 1
fib(2) = 1
fib(3) = 2
fib(4) = 3
fib(5) = 5
fib(6) = 8
fib(7) = 13
fib(8) = 21
fib(9) = 34
```

Follow `factorial(3)` by hand: it returns `3 * factorial(2)`, which needs
`2 * factorial(1)`, and `factorial(1)` is the base case, 1. The answers climb back up:
`2 * 1 = 2`, then `3 * 2 = 6`. Without a base case, the function would call itself until
it runs out of room.

## Putting it together: FizzBuzz

Count from 1 to 15. For multiples of 3 print `Fizz`, for multiples of 5 print `Buzz`, for
multiples of both print `FizzBuzz`, otherwise print the number:

```ling
fn fizzbuzz(n) {
  if n % 15 == 0 {
    return "FizzBuzz"
  }
  if n % 3 == 0 {
    return "Fizz"
  }
  if n % 5 == 0 {
    return "Buzz"
  }
  return str(n)
}

bind start = do {
  for n in 1..16 {
    println(fizzbuzz(n))
  }
}
```
```output
1
2
Fizz
4
Buzz
Fizz
7
8
Fizz
Buzz
11
Fizz
13
14
FizzBuzz
```

The test for 15 comes first; otherwise 15 would stop at the test for 3 and print `Fizz`.
The function turns a number into a word, and `start` only loops and prints. A good
function does one thing, takes what it needs as parameters, and has a name that says what
it does.

## Functions as values

In some languages you store a function in a binding or pass it to another function. Ling
has a short closure syntax for this, `|x| x * 2`, but in the current version function values
are the least reliable part of the language: a closure stored in a binding, or a function
passed as an argument, may give `()` or report `unknown function`. The one form that always
works is calling a closure right where you write it:

```ling
bind start = do {
  println((|x| x + 1)(5))
}
```
```output
6
```

For now, write a named `fn` and call it by name. See [Known Limits](../language/limits.md)
for the details.

> **Tip:** If you misspell a function name, the default engine prints `()` instead of an
> error. When a function seems to "do nothing", run the program with
> `ling run --interp yourfile.ling`, which reports `unknown function`.

## Try it

1. Write `is_even(n)` that returns `true` or `false`. Print `is_even(10)` and `is_even(7)`.
2. Write `max3(a, b, c)` that returns the largest of three numbers. (The builtin `max`
   takes two; you can nest it.)
3. Write `sum_to(n)` with a `while` loop, then `sum_to_r(n)` with recursion. Both should
   give 5050 for 100.
4. Write `power(base, exp)` recursively, using only multiplication. `power(2, 10)` is 1024.
5. Write `grade(score)` returning `"A"`, `"B"`, `"C"` or `"F"`, and print the grades for 95,
   85, 75 and 40.

Next: [Lists and Text](lists-text.md). For the full rules, see [Syntax](../language/syntax.md).
