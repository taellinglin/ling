# lingfu

`lingfu` (灵符, "spirit token") is Ling's project manager. It creates projects, runs
them and their tests, builds executables, talks to the package registry, and can
rewrite a whole project into another human language. For running and building it
calls `ling`, which it finds next to itself or on your `PATH`.

```text
lingfu <command> [arguments]
lingfu --help | -h | help
lingfu --version | -V | version
```

`lingfu` finds the project by searching the current folder and its parents for a
manifest: `Ling.toml`, `灵符.toml`, `霊符.toml`, `영부.toml` or `ลิงฟู.toml`.

## Commands at a glance

| Command | Aliases | Purpose |
|---|---|---|
| `new <name>` | `create` | Create a project folder. |
| `init` | `initialize` | Create a project in the current folder. |
| `run` | `execute`, `start` | Run the project. |
| `test` | `verify` | Run the project's tests. |
| `build` | `compile`, `make` | Build a native executable. |
| `clean` | `purge`, `clear` | Delete build output. |
| `tree` | `structure`, `ls` | Print the project tree. |
| `manifest` | `show`, `info` | Print the manifest. |
| `normalize <lang>` | `norm`, `localize` | Rewrite the project into another language. |
| `add <package>` | `install`, `include` | Install a package from the registry. |
| `search <query>` | `find`, `seek` | Search the registry. |
| `publish` | `release`, `share` | Publish the project. |
| `login <api-key>` | `sign-in`, `auth` | Save registry credentials. |
| `logout` | `sign-out`, `deauth` | Forget credentials. |
| `doc` | `docs`, `document` | Build documentation. |
| `bench`, `update` | | Run `cargo bench` / `cargo update`. |
| `wizard` | `guide`, `create-wizard` | Interactive project creator. |
| `seed` | | Integrity fingerprint of the project. |
| `ast`, `convert` | | Forwarded to `ling ast` / `ling convert`. |
| `check`, `fmt`, `translate` | | Don't use these yet (see below). |

Common options: `--lang`/`-l <code>` (scaffolding language: `en`, `zh`, `ja`, `ko`,
`th`, `ru`), `--type`/`-t <kind>`, `--root-lang <code>`, `--crates`, `--path <dir>`,
`--dry-run`/`-n`.

Each command also has names in other languages, for example `试` for `test` in
Chinese or `ทดสอบ` in Thai. These only work when the executable itself has the
localized name (such as `灵符.exe` or `ลิงฟู`). Under the name `lingfu`, use the English
commands.

### `lingfu new` and `lingfu init`

```text
lingfu new <name> [--type bin|lib|game|ui|ai|crypto|web|polyglot] [--lang en|zh|ja|ko|th|ru]
```

Creates `<name>/` with a manifest, a source folder with an entry program, a tests
folder, a lexicon folder, `README.md`, `.gitignore` and `ling.ignore`. `lingfu init`
does the same in the current folder, using its name as the project name.

`--lang` picks the language of the generated files **and** of the folder names:

| Language | Manifest | Sources | Entry | Tests | Build |
|---|---|---|---|---|---|
| English | `Ling.toml` | `src` | `start.ling` | `tests` | `build` |
| Chinese | `灵符.toml` | `灵源` | `启.灵` | `灵镜` | `灵碑` |
| Japanese | `霊符.toml` | `霊源` | `啓.霊` | `霊鏡` | `霊碑` |
| Korean | `영부.toml` | `령원` | `시작.령` | `령경` | `령비` |
| Thai | `ลิงฟู.toml` | `ต้นกำเนิด` | `เริ่ม.ลิง` | `ทดสอบ` | `สร้าง` |

The manifest has the tables `[spirit]` (`name`, `kind`, `version`, optional `entry`),
`[author]`, `[realm]` (`language`, `level`, `source`), `[root]` (the `core` dependency)
and `[depends]`. See [Running Programs](../getting-started/running.md#lingtoml) for
an example.

### `lingfu run`

Runs the project's entry program with `ling run`. The entry is the first file that
exists among: the manifest's `entry`, `<src>/<entry file>`, `<root>/<entry file>`,
`<src>/main.ling` and `<root>/main.ling`. Extra arguments are passed to `ling run`.

```text
$ lingfu run
Running ...\demo\src\start.ling...
🌊 Welcome to demo Spirit Realm 🌊
Spirit Level: 铁
```

### `lingfu test`

Runs every `.ling` file under the tests folder with `ling run`, stopping at the first
one that fails. A test is an ordinary program with an entry point (any `bind` of a
`do` block counts).

```text
$ lingfu test
Testing Ling project...
  test: ...\demo\tests\test.ling
✨ All mirrors are clear ✨
🔮 Ling tests passed for demo
✓ All tests passed!
```

Tests run on the JIT, which doesn't stop on an unknown function, so a broken test can
still "pass". Have each test print a clear PASS or FAIL line, or run it yourself with
`ling run --interp`. Helper files inside the tests folder are treated as tests too.

### `lingfu build`

Calls `ling build <project> --aot` and passes on `--out`, `--platform`, `--icon` and
`--pack`. `--no-aot` embeds the interpreter instead; `--path <dir>` picks the project
folder. If a `Cargo.toml` sits next to the manifest, it runs `cargo build` instead.

Before your first build, add `entry = "src/start.ling"` under `[spirit]`. Otherwise
`ling build` looks for `main.ling` and stops with `entry '...' not found`. Building
needs a Rust toolchain; see [The ling Command](ling.md#ling-build).

### `lingfu clean`, `tree`, `manifest`

- `clean` deletes the build and lock folders (`build` and `.lock`, or their localized
  names) and runs `cargo clean` if there is a Rust crate.
- `tree` prints the project's folder tree.
- `manifest` prints the manifest's path and contents.

### `lingfu normalize`

```text
lingfu normalize <lang> [--dry-run|-n] [--content-only] [--files-only]
```

Rewrites the whole project in the current folder into one human language. It
translates keywords and builtin names in every source file, renames source files and
the standard folders, and fixes imports to match. `<lang>` is a code such as `en`,
`zh`, `ja`, `ko` or `th` (names like `chinese` or `中文` work too). Without it you get
a menu.

| Option | Effect |
|---|---|
| `--dry-run`, `-n` | List the changes without touching anything. |
| `--content-only` | Rewrite file contents, keep the names. |
| `--files-only` | Rename files and folders, keep the contents. |

```text
$ lingfu normalize zh --dry-run
Normalizing to Chinese  [dry-run]
  ~ lexicons\en.ling
  ~ src\start.ling
  ~ tests\test.ling
  mv tests\test.ling → tests\测试.ling
  mv build → 构建
  mv src → 源码
  mv tests → 测试

Dry run complete — no files changed.
  ✓ 3 file(s) rewritten
  ~ 1 file(s) renamed
  ↕ 3 folder(s) renamed
```

> **Warning:** `normalize` rewrites **every** Ling file in the folder, in place. Commit
> your project (or make a copy) and do a dry run first.

Identifiers, comments and text inside strings are left as they are; only keywords and
builtins change. `normalize` also uses its own folder names (`源码`, `测试`, `构建`),
which differ from the ones `lingfu new --lang zh` creates.

### `lingfu check`, `fmt`, `translate`

These commands exist but call `ling` subcommands that don't exist (`ling check`,
`ling fmt`, `ling <from> <to> <file>`). `check` and `fmt` report success without doing
anything, and `translate` prints `ling`'s usage text. To change a project's language,
use `lingfu normalize`.

### Registry commands

The registry is `https://fu.ling-lang.org` by default. Set `LINGFU_REGISTRY` to use
another one.

| Command | What it does |
|---|---|
| `login <api-key>` | Saves the key to `~/.lingfu/credentials.toml`. |
| `logout` | Removes the saved credentials. |
| `publish` | Packs the project as `.tar.gz`, skipping files listed in `ling.ignore`, and uploads it. Needs `login`. |
| `search <query>` | Searches the registry. With `--crates`, searches crates.io instead. |
| `add <package>` | Downloads a package into `./ling_modules/<name>/` and prints the `use` line to import it. With `--crates`, runs `cargo add`. |

### `lingfu doc`, `bench`, `update`

`doc` runs `mdbook build` if there is a `docs/book.toml` or `book/book.toml`, and
`cargo doc --no-deps` otherwise. `bench` and `update` run `cargo bench` and
`cargo update`; they are useful only in projects that contain a Rust crate.

### `lingfu seed`

Makes an encrypted integrity fingerprint of a git project: a BLAKE3 Merkle root of
every tracked file, written as a playable `seed.wav`. `--check` compares the project
against a saved seed. It needs a git repository.

### `lingfu wizard`

An interactive questionnaire (language, project type, name, ...) that ends by creating
a project like `new`. It reads from the terminal, so you can't script it.

## Exit status

`lingfu` returns 0 on success. On failure it prints `Error: <message>` (for example
`build failed` or `tests failed`) and returns 1.
