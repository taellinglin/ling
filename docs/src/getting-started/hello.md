# Hello World

Every Ling program starts at a binding named `start` whose value is a `do` block.
Here is the smallest program that prints something.

## Your first program

Save this as `hello.ling`:

```ling
bind start = do {
    print("Hello, World!")
}
```
```output
Hello, World!
```

Run it:

```sh
ling run hello.ling
```

- `bind start = ...` gives the name `start` to a value. Ling looks for this name to
  know where your program begins.
- `do { ... }` is a block of steps that run in order.
- `print(...)` writes a line to the terminal. `println` does the same thing.

## A little more

Bindings can live outside `start` too. This program greets a name and then loops
over a list:

```ling
# Say hello to a few friends
bind name = "Ling"
bind start = do {
    print("Hello, " + name + "!")
    for friend in ["Ada", "Bo", "Chen"] {
        print("Hi, " + friend)
    }
}
```
```output
Hello, Ling!
Hi, Ada
Hi, Bo
Hi, Chen
```

`+` joins text, and `for ... in` runs its block once for each item of the list.

## Mixing languages

Every keyword and builtin has a name in English, Chinese, Japanese, Korean and Thai,
and you can mix them freely in one file. This program uses all five:

```ling
令 名字 = "Ling"
bind start = do {
    พิมพ์("สวัสดี " + 名字)
    만약 참 {
        印刷("こんにちは")
    }
}
```
```output
สวัสดี Ling
こんにちは
```

When a file is mostly in another language, `ling` notes the language it detected
on standard error, for example `[detected language: Chinese (中文)]`. It doesn't
change how the program runs.

To learn how the five languages fit together, read
[Writing in Five Languages](../language/multilingual.md).

## Next steps

- [Running Programs](running.md): the JIT, the interpreter, and projects.
- [Values and Bindings](../tutorials/values.md): the first tutorial.
