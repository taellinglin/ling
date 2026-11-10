# Lists and Text

Until now each name held one value. In this lesson you will keep many values together
in a **list**, walk through them with loops, and take text apart and put it back
together. You will finish with a small word counter.

## Making a list

Write a list between square brackets, with commas between the items:

```ling
bind start = do {
  bind fruits = ["apple", "banana", "cherry"]
  println(fruits)
  println(len(fruits))
  println(list_get(fruits, 0))
  println(fruits[2])
}
```
```output
[apple, banana, cherry]
3
apple
cherry
```

- Printing a list shows its items in square brackets.
- `len(fruits)` is the number of items.
- `list_get(fruits, 0)` reads the item at position 0. `fruits[2]` is a shorter way to
  write `list_get(fruits, 2)`.

**Positions start at 0.** The first item is at position 0, and the last of three items is
at position 2. Think of the position as "how many items do I skip?". Keep positions between
0 and `len(xs) - 1`: asking for a position that doesn't exist gives an empty value on the
default engine and an error on the interpreter.

## Changing a list: always rebind

`list_push` adds an item at the end, and `list_set` replaces the item at a position. Both
give you a **new** list. Bind the name again to keep it:

```ling
bind start = do {
  bind fruits = ["apple", "banana", "cherry"]
  bind fruits = list_push(fruits, "date")
  println(fruits)
  bind fruits = list_set(fruits, 1, "blueberry")
  println(fruits)
  println("now ", len(fruits), " fruits")
}
```
```output
[apple, banana, cherry, date]
[apple, blueberry, cherry, date]
now 4 fruits
```

Always write `bind fruits = list_push(fruits, "date")`, never just
`list_push(fruits, "date")` on its own. This is the same rebinding habit you learned in
[Values and Bindings](values.md).

## Building a list in a loop

Start with an empty list from `list_new()` and push items into it. Remember the scope rule
from [Decisions and Loops](control-flow.md): a `for` body can't leave results behind, so
build lists with `while`:

```ling
bind start = do {
  bind squares = list_new()
  bind i = 1
  while i < 8 {
    bind squares = list_push(squares, i * i)
    bind i = i + 1
  }
  println(squares)
}
```
```output
[1, 4, 9, 16, 25, 36, 49]
```

## Walking through a list

For **reading** a list, `for` is fine. When you also need a running result, use `while`
with a position:

```ling
bind start = do {
  bind scores = [72, 95, 88, 61, 79]
  for s in scores {
    println("score: ", s)
  }
  bind total = 0
  bind best = scores[0]
  bind i = 0
  while i < len(scores) {
    bind s = scores[i]
    bind total = total + s
    if s > best {
      bind best = s
    }
    bind i = i + 1
  }
  println("total: ", total)
  println("average: ", total / len(scores))
  println("best: ", best)
  println("list_max says ", list_max(scores, 0))
}
```
```output
score: 72
score: 95
score: 88
score: 61
score: 79
total: 395
average: 79
best: 95
list_max says 95
```

`best` starts as the first item, and every bigger item takes its place. The builtins
`list_max` and `list_min` do the same job; their second argument is the answer for an empty
list.

## Text tools

Ling has a set of builtins for working with text:

```ling
bind start = do {
  bind s = "  Hello, Ling World  "
  println(len(s))
  bind s = trim(s)
  println("[" + s + "]")
  println(str_upper(s))
  println(str_lower(s))
  println(substr(s, 0, 5))
  println(str_find(s, "Ling"))
  println(str_find(s, "Python"))
  println(str_replace(s, "World", "Friend"))
  println(starts_with(s, "Hello"))
  println(str_repeat("ab", 3))
}
```
```output
21
[Hello, Ling World]
HELLO, LING WORLD
hello, ling world
Hello
7
-1
Hello, Ling Friend
true
ababab
```

| Builtin | Purpose |
|---|---|
| `len(s)` | number of characters |
| `trim(s)` | remove spaces from both ends |
| `str_upper(s)`, `str_lower(s)` | change case |
| `substr(s, start, count)` | `count` characters starting at position `start` (from 0) |
| `str_find(s, part)` | position of the first match, or `-1` if there is none |
| `str_replace(s, from, to)` | replace every `from` with `to` |
| `starts_with(s, part)`, `ends_with(s, part)` | `true` or `false` |
| `str_repeat(s, n)` | the text repeated `n` times |

Like the list builtins, these never change the original text. They return a new text, so
bind the result: `bind s = trim(s)`.

## Splitting and joining

`split` cuts a text into a list wherever a separator appears. `list_join` glues a list back
into one text (`join` is the same builtin):

```ling
bind start = do {
  bind line = "red,green,blue"
  bind parts = split(line, ",")
  println(parts)
  println(len(parts))
  println(list_join(parts, " | "))
  bind numbers = split("3 8 15", " ")
  println(to_number(numbers[1]) + 1)
}
```
```output
[red, green, blue]
3
red | green | blue
9
```

The pieces from `split` are text. Use `to_number` before doing arithmetic with them.

## Letter by letter

`substr(s, i, 1)` gives the single character at position `i`. Here it reverses a word:

```ling
fn reverse_text(s) {
  bind out = ""
  bind i = len(s) - 1
  while i >= 0 {
    bind out = out + substr(s, i, 1)
    bind i = i - 1
  }
  return out
}

bind start = do {
  println(reverse_text("Ling"))
  println(reverse_text("level") == "level")
}
```
```output
gniL
true
```

## A word counter

Ling has no dictionary type yet, so keep two lists side by side: `words` holds each
distinct word and `counts` holds how often it appeared. A helper finds a word's position:

```ling
fn index_of(items, target) {
  bind i = 0
  while i < len(items) {
    if items[i] == target {
      return i
    }
    bind i = i + 1
  }
  return -1
}

bind start = do {
  bind all = split("the cat saw the dog and the dog ran", " ")
  bind words = list_new()
  bind counts = list_new()
  bind i = 0
  while i < len(all) {
    bind w = all[i]
    bind pos = index_of(words, w)
    if pos == -1 {
      bind words = list_push(words, w)
      bind counts = list_push(counts, 1)
    } else {
      bind counts = list_set(counts, pos, counts[pos] + 1)
    }
    bind i = i + 1
  }
  bind k = 0
  while k < len(words) {
    println(words[k], ": ", counts[k])
    bind k = k + 1
  }
}
```
```output
the: 3
cat: 1
saw: 1
dog: 2
and: 1
ran: 1
```

> **Note:** `==` compares numbers, text and booleans. Comparing two lists with `==` is
> always `false`, even when they hold the same items. Compare them item by item.

## Try it

1. Make a list of five numbers and print their total with a `while` loop.
2. Split `"3,8,15,4"` on commas, convert each piece with `to_number`, and print the biggest.
3. Write `count_vowels(s)` that counts the letters a, e, i, o, u. (Hint: lower-case the text,
   then test each character with `str_find("aeiou", ch) != -1`.)
4. Use `str_replace` and `str_lower` to make a palindrome test that ignores spaces, and try it
   on `"never odd or even"`.
5. Extend the word counter so that it also prints the most common word.

Next: [Drawing and Games](drawing-games.md). For every list and text builtin, see
[Text and Lists](../reference/text-lists.md).
