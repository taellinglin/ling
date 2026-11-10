# The ling Command

`ling` runs, builds, draws and converts Ling programs. This page lists every
subcommand and option. For a gentler introduction, start with
[Running Programs](../getting-started/running.md).

```text
ling run [--interp|--wasm|--O0|--O1|--O2|--O3] <file.ling>
ling <file.ling>                       (shorthand for: ling run <file.ling>)
ling build <file.ling|dir> [--out dir] [--platform list] [--icon file] [--pack] [--aot]
ling ast [path] [--technical|--artwork|--ling|--all] [--out dir]
ling visualize <file.ling>
ling convert <asset> [-o out.ling] [--no-compression]
```

| Subcommand | Purpose |
|---|---|
| `run` | Run a program (JIT by default). |
| `build` | Compile a file or project into a distributable for one or more platforms. |
| `ast` | Draw the syntax tree of a file or project as SVG files. |
| `visualize` | Write one SVG of a file's syntax tree to standard output. |
| `convert` | Turn an asset (model, sound, MIDI, SVG) into an importable `.ling` file. |

Running `ling` with no arguments, or with anything it doesn't recognise, prints the
usage text and exits with status 0. The first line shows the version. There is no
`--version` or `--help` flag, and there are no `check`, `fmt`, `translate` or `test`
subcommands.

A source file can use any of the extensions `.ling`, `.灵`, `.霊`, `.령` or `.ลิง`.

### `ling run`

Parses the file, finds the entry point and runs it.

| Flag | Effect |
|---|---|
| *(none)* | Cranelift JIT, the default. Falls back to the interpreter for the whole program if the JIT can't compile it. |
| `--interp` | Tree-walking interpreter. The semantic reference, with the best error messages. |
| `--wasm` (or `--web`) | Builds the program to WebAssembly, serves it locally and opens the browser. Needs `lingc` next to `ling`. |
| `--O0` `--O1` `--O2` `--O3` | JIT optimisation level (default `--O3`). Lower it if a very large program compiles slowly. |

Flags can come before or after the file name; the first argument with a Ling
extension is the program. Arguments after it are left for the program to read with
`get_args()` and `cli_arg(name, default)`.

Things worth knowing:

- Programs that call `http_serve` or `lingtp_serve` (or a localized alias such as
  `เว็บเสิร์ฟ`) run on the interpreter automatically, because the JIT doesn't support
  closure arguments yet. The `ling file.ling` shorthand skips this check, so use
  `ling run` for servers.
- On the JIT, an unknown function returns `()` silently. The interpreter reports
  `unknown function`. See [Known Limits](../language/limits.md).
- A file without an entry point is treated as a library and fails with
  `error[runtime]: no entry point`.
- For a source file in another language, `ling` prints `[detected language: ...]` to
  standard error before running.
- A missing file prints `[ling] error: file does not exist: <path>`. Parse errors are
  `error[parse]: ...`, runtime errors are `error[runtime]: ...` with a traceback. All
  of these exit with status 1.

### `ling build`

Compiles a program or a project into a distributable.

| Option | Meaning |
|---|---|
| `<target>` | A `.ling` file or a directory (default `.`). A directory is read through its manifest (`灵符.toml`, `ลิงฟู.toml`, then `Ling.toml`). Without one, `ling` looks for `main.ling`, `start.ling`, `启.灵`, `เริ่ม.ลิง`, a `src` folder, or any `.ling` file. |
| `--out <dir>` | Output folder, default `dist`. Each platform goes in a sub-folder such as `dist/web` or `dist/windows`. |
| `--platform <list>` | Comma-separated targets, listed below. `all` means `win,lin,mac,web`. |
| `--icon <file>` | App icon (`.svg`, `.png`, `.jpg`, `.bmp`, `.ico`), overriding the manifest's `icon`. |
| `--pack` | Embed the `[includes]` resources and the program in the executable, so it is one self-contained file. |
| `--aot` | Compile ahead of time to native code instead of embedding the interpreter. |

| Platform | Aliases | Output |
|---|---|---|
| `web` | | WebGL/WebAssembly bundle, built by `lingc` |
| `win` | `windows` | Windows executable |
| `lin` | `linux` | Linux executable |
| `mac` | `macos`, `darwin` | macOS (Intel) executable |
| `macarm` | `mac-arm`, `mac-arm64`, `macos-arm64`, `apple-silicon` | macOS (Apple Silicon) executable |
| `pios` | `pi`, `raspios`, `linux-arm64`, `linarm`, `arm64` | Raspberry Pi OS / 64-bit ARM Linux |
| `kernel` | `bare` | LingOS bare-metal kernel image (`os` profile) |
| `rpi` | `raspberrypi` | LingOS image for Raspberry Pi (`os` profile) |
| `lingos` | `app`, `apps` | LingOS userland app (`os` profile) |
| `lingos-rpi` | `apps-rpi`, `app-rpi` | LingOS userland app for Raspberry Pi (`os` profile) |

Without `--platform`, the build uses the manifest's `targets`, or the host platform
plus `web` if there are none. When the list comes from the manifest, desktop targets
your machine can't build (and `cross` isn't installed) are skipped with a note. An
explicit `--platform` fails instead.

Native targets generate and compile a small Rust project, so a **Rust toolchain must
be installed** and the first build can take minutes.

#### Manifest keys

`ling build` reads its manifest line by line, not with a full TOML parser. Write every
setting on one line as `key = value`, and keep arrays on a single line.

| Key | Meaning |
|---|---|
| `name` | Project name, used for the executable. |
| `version` | Version string. Default `0.1.0`. |
| `entry` | Entry program relative to the manifest. Default `main.ling`. |
| `kind` | `bin` (default), `web`, `game`, `ui`, `ai`, `crypto`, `lib`, `polyglot`, `kernel`. |
| `icon` | Icon file. |
| `targets` | Default platforms, e.g. `targets = ["win", "lin", "web"]`. |
| `pack` | `pack = true` is the same as `--pack`. |
| `includes` | Resource globs, or an `[includes]` section with one quoted pattern per line. `*` matches inside a folder, `**` across folders. |
| `ling-features` | Cargo features of `ling` to enable in the executable, e.g. `["web"]` for programs that use `db_*`. |

```toml
# Ling.toml
[project]
name = "my-game"
version = "1.0.0"
entry = "main.ling"
icon = "images/logo.svg"
targets = ["win", "lin", "web"]
pack = true

[includes]
"/music/*.wav"
"/font/*.otf"
"/levels/**/*.json"
```

Included files are copied next to the executable in the same layout, or embedded with
`--pack`. File builtins such as `read_file` work the same either way.

### `ling ast`

Draws the syntax tree of a file or a whole project as SVG. A directory is walked
recursively (skipping hidden folders, `target`, `dist`, `node_modules`, `AST` and
`灵碑`) and treated as one program.

| Option | Meaning |
|---|---|
| `[path]` | File or directory, default `.`. |
| `--technical`, `--artwork`, `--ling` | Pick one or more drawing styles. With none, all three are made. |
| `--all` | All three styles. |
| `--out <dir>` | Output folder, default `AST`. |

```text
$ ling ast hello.ling
ling ast: 'hello' — 1 file(s), 0 fn(s)
  ✓ AST\hello.technical.svg
  ✓ AST\hello.artwork.svg
  ✓ AST\hello.ling.svg
```

### `ling visualize`

Writes a single SVG picture of a file's syntax tree to standard output. Redirect it
to a file:

```sh
ling visualize hello.ling > hello.svg
```

It only parses the file, so it is also a quick way to check that a program parses.

### `ling convert`

Turns an asset into a `.ling` file you can `use`. See [Asset Conversion](convert.md).

```sh
ling convert model.glb -o model.ling
```

## Exit status

`ling` returns 0 on success and 1 on any reported error.
