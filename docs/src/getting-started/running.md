# Running Programs

You run a single file with `ling run`, and a whole project with `lingfu run`. This
page shows both, explains the two ways Ling can execute your code, and how to
turn a program into something you can ship.

## Run a file

```sh
ling run hello.ling
ling hello.ling          # shorthand for the same thing
```

`ling run` parses the file, finds the entry point (`bind start = do { ... }`) and runs
it. A source file can end in `.ling`, `.灵`, `.霊`, `.령` or `.ลิง`.

Anything after the file name stays on the command line for your program to read:
`get_args()` returns the whole command line, and `cli_arg("port", "8080")` reads a
`--port 99` option, falling back to `"8080"`.

## JIT or interpreter

Ling has two backends:

| Command | Backend | Use it for |
|---|---|---|
| `ling run file.ling` | Cranelift JIT (the default) | Speed. Compiles your program to native code first. |
| `ling run --interp file.ling` | Tree-walking interpreter | Diagnostics. It is the reference for how Ling behaves. |

If the JIT can't compile a program, `ling run` falls back to the interpreter for the
whole program. You can also lower the JIT's optimisation level with `--O0`, `--O1`,
`--O2` or `--O3` (the default is `--O3`) if a very large program takes too long to
compile.

### When a program "does nothing", try `--interp`

On the JIT, calling a function that doesn't exist returns nothing and the program
carries on. This program has a typo in `draw_rectt`:

```ling
bind start = do {
    print("before")
    draw_rectt(1, 2)
    print("after")
}
```
```output
before
after
```

The interpreter stops and tells you what went wrong:

```text
$ ling run --interp typo.ling
before
error[runtime]: unknown function 'draw_rectt'
 --> typo.ling
  traceback (deepest call last):
    0: start
  hint: check the spelling, or `use` the module that defines it
```

Make `--interp` part of your routine whenever something seems to be ignored.
[Known Limits](../language/limits.md) lists the other differences between the backends.

### Servers run on the interpreter

A program that calls `http_serve` or `lingtp_serve` always runs on the interpreter,
even without `--interp`. Request handlers are closures passed to a builtin, and the
JIT doesn't support those yet. A web service waits on the network most of the time,
so you don't lose anything noticeable. See [A Small Web Service](../tutorials/web-service.md).

### Run in the browser

`ling run --wasm file.ling` (or `--web`) builds the program to WebAssembly, serves it
on a local web server and opens your browser. The first build compiles the runtime
and can take about a minute. It needs the `lingc` binary next to `ling`.

## Projects with lingfu

For anything bigger than one file, let `lingfu` create a project:

```sh
lingfu new demo
cd demo
lingfu run
```

```text
Running ...\demo\src\start.ling...
🌊 Welcome to demo Spirit Realm 🌊
Spirit Level: 铁
```

`lingfu new` creates this layout:

```text
demo/
├── Ling.toml          the manifest
├── README.md
├── .gitignore
├── ling.ignore        files to leave out when publishing
├── lexicons/en.ling
├── src/start.ling     the entry program
└── tests/test.ling
```

| Command | What it does |
|---|---|
| `lingfu run` | Runs the entry program with `ling run`. Extra arguments are passed on. |
| `lingfu test` | Runs every `.ling` file under `tests/`, one by one. |
| `lingfu build` | Builds a native executable with `ling build --aot`. |
| `lingfu clean` | Deletes the build output. |

`lingfu new --lang zh` (or `ja`, `ko`, `th`) creates the project in that language,
including the folder and file names. [lingfu](../tools/lingfu.md) covers every command.

## Ling.toml

The manifest that `lingfu new` writes looks like this:

```toml
[spirit]
name = "demo"
kind = "bin"
version = "2030.0.0"

[author]
ling-lin = { email = "you@example.com" }

[realm]
language = "en"
level    = "铁"
source   = "src"

[root]
core = { version = "2030.0", language = "en" }

[depends]
# Add dependencies: spirit-name = { version = "x.y.z" }
```

- `[spirit]` names the project. You can add `entry = "src/start.ling"` here.
- `[realm]` sets the project's language and its source folder.
- `[root]` and `[depends]` list the `core` dependency and your packages.

`lingfu run` and `lingfu test` find `src/start.ling` on their own. `ling build`
(and therefore `lingfu build`) looks for `main.ling` unless the manifest says
otherwise, so **add `entry = "src/start.ling"` under `[spirit]`** before you build.

## Build a program

`ling build` turns a file or a project folder into something you can distribute:

```sh
ling build game.ling                      # host platform + web, into dist/
ling build . --platform win,web --out dist
ling build . --platform all --pack --icon logo.svg
```

| Option | Meaning |
|---|---|
| `--out <dir>` | Output folder (default `dist`). Each platform gets a sub-folder. |
| `--platform <list>` | Comma-separated: `web`, `win`, `lin`, `mac`, `macarm`, `pios`, `kernel`, `rpi`, `lingos`, `lingos-rpi`. `all` means `win,lin,mac,web`. |
| `--icon <file>` | App icon (`.svg`, `.png`, `.ico`, ...). |
| `--pack` | Embed the manifest's `[includes]` files in the executable. |
| `--aot` | Compile to native code ahead of time instead of embedding the interpreter. |

Without `--platform`, the build uses the manifest's `targets` list, or your host
platform plus `web` if there is none. Desktop targets generate and compile a small
Rust project, so you need a Rust toolchain installed, and the first build can take a
few minutes. The `web` target uses the `lingc` binary.

[The ling Command](../tools/ling.md) has the full list of options and manifest keys.
