# Values and Bindings

Every program works with information: a score, a name, a yes-or-no answer. In this
lesson you will meet the basic kinds of values in Ling, give them names with `bind`,
"change" them by rebinding, and print them. Each step has a small program you can
copy into a file and run with `ling run`.

## Values

A single piece of information is called a **value**. These are the kinds you will use most:

| Kind | What it holds | Examples |
|---|---|---|
| number | any number, whole or with a fraction | `42`, `3.14`, `-7` |
| text | letters, words, sentences (also called a *string*) | `"hello"`, `"Room 101"` |
| boolean | one of two answers: yes or no | `true`, `false` |
| list | several values in a row | `[1, 2, 3]`, `["red", "blue"]` |

Print one of each:

```ling
bind start = do {
  println(42)
  println(3.14)
  println("a piece of text")
  println(true)
  println([1, 2, 3])
}
```
```output
42
3.14
a piece of text
true
[1, 2, 3]
```

Numbers are written bare. Text goes between double quotes. `true` and `false` have no
quotes, because they are not text. A list is written between square brackets, with
commas between the items.

A few rules for writing values:

- Write numbers with ordinary digits and at most one decimal point. Write `0.5`, not `.5`.
  There is no `1e3` or `0xff`.
- Text uses double quotes only. Inside text, `\n` starts a new line and `\"` is a quote mark.

## Bindings: giving values a name

You give a value a name with `bind`. From then on, the name stands for the value:

```ling
bind start = do {
  bind name = "Mina"
  bind age = 12
  println("Hello, " + name)
  println(name, " is ", age)
  println("Next year: ", age + 1)
}
```
```output
Hello, Mina
Mina is 12
Next year: 13
```

Read `bind age = 12` as "bind the name `age` to the value 12". Names can contain letters,
digits and underscores, and must not start with a digit. Pick names that say what the value
means: `age` is better than `a`.

Every program you write starts the same way: `bind start = do { ... }`. The instructions
inside the braces run from top to bottom.

## Rebinding: how Ling "changes" things

Ling has **no assignment statement**. You can't write `score = score + 5`; that is a
parse error. To give a name a new value, bind it again:

```ling
bind start = do {
  bind score = 10
  println("score is ", score)
  bind score = score + 5
  println("score is ", score)
  bind score = score * 2
  println("score is ", score)
}
```
```output
score is 10
score is 15
score is 30
```

`bind score = score + 5` works out the right-hand side first, using the current value
of `score`, and then puts the label `score` on the result. Make it a habit: whenever you
want to change something, type `bind`.

## Printing

`println` prints its arguments and ends the line. `print` does the same: in Ling both
finish with a new line. When you pass several arguments, they are printed one after the
other with **nothing in between**, not even a space:

```ling
bind start = do {
  println("sum", 1 + 2)
  println("sum: ", 1 + 2)
  println("a", "b", "c")
  println("line one\nline two")
  println("She said \"hi\"")
}
```
```output
sum3
sum: 3
abc
line one
line two
She said "hi"
```

If you want a space, put it inside your text, as in `"sum: "`.

## Arithmetic

Ling uses the usual symbols: `+`, `-`, `*`, `/`, and `%` for the remainder.

```ling
bind start = do {
  println(7 + 3)
  println(7 / 2)
  println(7 % 2)
  println(2 + 3 * 4)
  println((2 + 3) * 4)
  println(round(3.7))
  println(sqrt(81))
}
```
```output
10
3.5
1
14
20
4
9
```

Division gives an exact answer (`3.5`), not a rounded one. Multiplication happens before
addition, so `2 + 3 * 4` is `14`. Use parentheses when you mean something else.
`round` and `sqrt` are **builtins**: functions that come with Ling.

## Converting between text and numbers

You can join two pieces of text with `+`. To join a number to text, turn it into text first
with `str` (`to_str` is the same function). To go the other way, `to_number` turns text into
a number:

```ling
bind start = do {
  bind apples = 3
  println("I have " + str(apples) + " apples")
  bind typed = "12"
  println(to_number(typed) + 1)
  println(format("{} + {} = {}", 2, 3, 2 + 3))
  println(format("about {:.2}", 3.14159))
}
```
```output
I have 3 apples
13
2 + 3 = 5
about 3.14
```

`format` fills each `{}` in a template with the next argument. `{:.2}` shows a number with
two decimal places.

## Comparisons and booleans

A comparison asks a yes-or-no question and gives back `true` or `false`:

```ling
bind start = do {
  println(5 > 3)
  println(5 == 5)
  println(5 != 5)
  println("cat" == "cat")
  bind big = 100 > 50
  println("big is ", big)
  println(true && false)
  println(true || false)
  println(!true)
}
```
```output
true
true
false
true
big is true
false
true
false
```

The comparisons are `==`, `!=`, `<`, `>`, `<=` and `>=`. A single `=` is never a comparison;
it only appears in `bind`. Combine answers with `&&` (and: both must be true), `||` (or: at
least one must be true) and `!` (not). Don't chain comparisons as in maths: write
`x > 1 && x < 10`, not `1 < x < 10`.

## Comments

Everything after `#` or `//` on a line is a comment. Ling ignores it; it is a note for people.

```ling
# A tiny bill splitter
bind start = do {
  bind bill = 84.50
  bind people = 4      // friends at the table
  bind each = bill / people
  println("Each pays ", round(each * 100) / 100)
}
```
```output
Each pays 21.13
```

`round(x * 100) / 100` rounds a number to two decimal places for display.

## Try it

1. Bind your age and print `In 10 years I will be ...` with the right number.
2. A rectangle is 12.5 wide and 8 tall. Print its area and its perimeter, each with a label.
3. Bind `n` to 10. Using only `bind`, make it 11, then 22, then 4. Print `n` after each step.
4. Convert 98.6 degrees Fahrenheit to Celsius with `(f - 32) * 5 / 9` and print it rounded
   to one decimal place.
5. Print `"7" + "3"` and then `to_number("7") + to_number("3")`. Why are they different?

Next: [Decisions and Loops](control-flow.md). For the builtins used here, see
[Output, Math and Logic](../reference/core.md) and [Text and Lists](../reference/text-lists.md).
