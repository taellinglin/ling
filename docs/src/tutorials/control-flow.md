# Decisions and Loops

Programs get interesting when they can choose between actions and repeat them. In this
lesson you will use `if` and `else` to make decisions, `while` and `for` to repeat work,
and `match` to pick between many cases. You will also learn the one scope rule that
surprises nearly every Ling beginner.

## Making a decision with `if`

```ling
bind start = do {
  bind temperature = 31
  if temperature > 30 {
    println("It is hot. Drink water.")
  }
  println("Done checking.")
}
```
```output
It is hot. Drink water.
Done checking.
```

The pattern is `if condition { ... }`. The condition is anything that gives `true` or
`false`, usually a comparison. If it is true, the block runs; if not, Ling skips it.
There are no parentheses around the condition, and the braces are required.

Change `temperature` to `20` and run it again: only `Done checking.` is printed.

## `else` and `else if`

`else` gives the other road. Chain more tests with `else if`. Ling checks them from top to
bottom and runs the first block whose condition is true:

```ling
bind start = do {
  bind score = 83
  if score >= 90 {
    println("Grade A")
  } else if score >= 80 {
    println("Grade B")
  } else if score >= 70 {
    println("Grade C")
  } else {
    println("Keep practising")
  }
}
```
```output
Grade B
```

83 is also at least 70, but the `score >= 80` branch came first and won. Put the largest
threshold first.

Combine conditions with `&&` (and), `||` (or) and `!` (not):

```ling
bind start = do {
  bind hour = 14
  bind raining = true
  bind have_umbrella = false
  if hour >= 9 && hour < 17 {
    println("The shop is open")
  }
  if raining && !have_umbrella {
    println("You will get wet")
  }
  if hour < 6 || hour > 22 {
    println("Go to sleep")
  } else {
    println("Stay awake")
  }
}
```
```output
The shop is open
You will get wet
Stay awake
```

## Repeating with `while`

A `while` loop repeats its block as long as the condition stays true:

```ling
bind start = do {
  bind i = 1
  while i <= 5 {
    println("Count: ", i)
    bind i = i + 1
  }
  println("Liftoff!")
}
```
```output
Count: 1
Count: 2
Count: 3
Count: 4
Count: 5
Liftoff!
```

Every `while` loop has three parts:

1. Set a counter before the loop (`bind i = 1`).
2. A condition that keeps the loop going (`i <= 5`).
3. Inside the loop, do the work **and move the counter** (`bind i = i + 1`).

If you forget step 3, the condition stays true forever. Press **Ctrl+C** to stop a program
that never ends.

### Collecting a result

A `bind` inside a `while` (or an `if`) block changes the name outside it. That makes
`while` the right tool for running totals:

```ling
bind start = do {
  bind total = 0
  bind n = 1
  while n <= 100 {
    bind total = total + n
    bind n = n + 1
  }
  println("1 + 2 + ... + 100 = ", total)
}
```
```output
1 + 2 + ... + 100 = 5050
```

## Counting with `for`

When you know how many times to repeat, `for` is shorter. It walks through a **range**
of numbers, or through the items of a list:

```ling
bind start = do {
  for i in 0..4 {
    println("i is ", i)
  }
  for k in 1..4 {
    println(k, " x 7 = ", k * 7)
  }
  for color in ["red", "green", "blue"] {
    println("I like ", color)
  }
}
```
```output
i is 0
i is 1
i is 2
i is 3
1 x 7 = 7
2 x 7 = 14
3 x 7 = 21
I like red
I like green
I like blue
```

`0..4` means 0, 1, 2 and 3: the range stops **just before** the second number. So
`for i in 0..4` runs four times.

The end of a range must be a number, a name, or something in parentheses. Write
`0..(n + 1)` and `0..(len(xs))`, never `0..len(xs)`: that version silently runs zero times.

### The `for` scope rule

This is the rule that surprises everyone. Each pass of a `for` loop gets its own fresh
copy of the names around it. A `bind` inside a `for` body **does not change** the name
outside:

```ling
bind start = do {
  bind total = 0
  for i in 1..5 {
    bind total = total + i
  }
  println("after for: ", total)

  bind total = 0
  bind i = 1
  while i < 5 {
    bind total = total + i
    bind i = i + 1
  }
  println("after while: ", total)
}
```
```output
after for: 0
after while: 10
```

The rule of thumb: use `for` for loops that only **do** something (print, draw, play a
note). When a loop must **leave a result behind** (a total, a count, a growing list),
use `while`.

Ling has no working `break` or `continue`. To leave a `while` loop early, make its
condition false, for example with a flag such as `bind searching = false`.

### Loops inside loops

A loop can sit inside another. Here the outer `for` only prints, and the inner `while`
builds up the text of each row:

```ling
bind start = do {
  for row in 1..5 {
    bind line = ""
    bind col = 0
    while col < row {
      bind line = line + "*"
      bind col = col + 1
    }
    println(line)
  }
}
```
```output
*
**
***
****
```

## Choosing with `match`

When you compare one value against many possibilities, `match` is tidier than a long
`else if` chain. Each **arm** is written `pattern => action`. Ling runs the first arm that
fits, and `_` fits anything:

```ling
bind start = do {
  bind day = 3
  match day {
    1 => println("Monday")
    2 => println("Tuesday")
    3 => println("Wednesday")
    _ => println("Some other day")
  }
  bind word = match "green" {
    "red" => "stop"
    "green" => "go"
    _ => "wait"
  }
  println(word)
}
```
```output
Wednesday
go
```

`match` works with numbers and text, and it can produce a value that you bind. Always end
with a `_` arm: if nothing matches, `match` quietly gives `()`, the "nothing" value.

## Try it

1. Bind a number `n` and print `positive`, `negative` or `zero`.
2. Print the 6 times table from `6 x 1` to `6 x 10` with a `for` loop.
3. Use a `while` loop to add up the even numbers from 2 to 50. (The answer is 650.)
4. Start with 1 and double it until it is larger than 1000. Print how many doublings it took.
5. Rewrite the grade example with the tests in the opposite order (`score >= 70` first).
   What goes wrong?

Next: [Functions](functions.md). For the full rules, see [Syntax](../language/syntax.md)
and [Known Limits](../language/limits.md).
