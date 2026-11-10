# ling 命令

`ling` 用来运行、构建、绘制和转换 Ling 程序。本页列出所有子命令和选项。
如果想先看简单的入门，请从[运行程序](../getting-started/running.md)开始。

```text
ling run [--interp|--wasm|--O0|--O1|--O2|--O3] <file.ling>
ling <file.ling>                       (shorthand for: ling run <file.ling>)
ling build <file.ling|dir> [--out dir] [--platform list] [--icon file] [--pack] [--aot]
ling ast [path] [--technical|--artwork|--ling|--all] [--out dir]
ling visualize <file.ling>
ling convert <asset> [-o out.ling] [--no-compression]
```

| 子命令 | 用途 |
|---|---|
| `run` | 运行程序（默认使用 JIT）。 |
| `build` | 把文件或项目编译成可分发的成品，支持一个或多个平台。 |
| `ast` | 把文件或项目的语法树画成 SVG 文件。 |
| `visualize` | 把一个文件的语法树以单个 SVG 输出到标准输出。 |
| `convert` | 把资源（模型、声音、MIDI、SVG）转换成可导入的 `.ling` 文件。 |

不带参数运行 `ling`，或者传入它不认识的内容，它会打印用法说明并以状态 0 退出。第一行就是版本号。
没有 `--version` 或 `--help` 选项，也没有 `check`、`fmt`、`translate` 或 `test` 子命令。

源文件可以使用 `.ling`、`.灵`、`.霊`、`.령` 或 `.ลิง` 任意一种扩展名。

### `ling run`

解析文件，找到入口并运行。

| 选项 | 效果 |
|---|---|
| *(无)* | Cranelift JIT，默认后端。如果 JIT 无法编译，就让解释器运行整个程序。 |
| `--interp` | 树遍历解释器。它是语义上的标准参照，错误信息也最详细。 |
| `--wasm`（或 `--web`） | 把程序构建成 WebAssembly，在本地提供并打开浏览器。需要 `ling` 旁边有 `lingc`。 |
| `--O0` `--O1` `--O2` `--O3` | JIT 优化级别（默认 `--O3`）。很大的程序编译太慢时可以降低它。 |

选项可以写在文件名前面或后面；第一个带 Ling 扩展名的参数就是程序。其后的参数留给程序，
用 `get_args()` 和 `cli_arg(name, default)` 读取。

值得了解的几点：

- 调用了 `http_serve` 或 `lingtp_serve`（或泰文的 `เว็บเสิร์ฟ` 等别名）的程序会自动在解释器上运行，
  因为 JIT 还不支持闭包参数。简写 `ling file.ling` 不做这项检查，所以运行服务器时请用 `ling run`。
- 在 JIT 上，未知函数会悄悄返回 `()`。解释器则会报 `unknown function`。参见[已知限制](../language/limits.md)。
- 没有入口的文件被当作库，运行时报 `error[runtime]: no entry point`。
- 对于用其他语言写的源文件，`ling` 会在运行前向标准错误输出 `[detected language: ...]`。
- 文件不存在时输出 `[ling] error: file does not exist: <path>`。语法错误是 `error[parse]: ...`，
  运行时错误是带调用栈的 `error[runtime]: ...`。这些情况的退出状态都是 1。

### `ling build`

把程序或项目编译成可分发的成品。

| 选项 | 含义 |
|---|---|
| `<target>` | 一个 `.ling` 文件或一个目录（默认 `.`）。目录通过其清单读取（依次找 `灵符.toml`、`ลิงฟู.toml`、`Ling.toml`）。没有清单时，`ling` 会找 `main.ling`、`start.ling`、`启.灵`、`เริ่ม.ลิง`、`src` 文件夹，或任意 `.ling` 文件。 |
| `--out <dir>` | 输出文件夹，默认 `dist`。每个平台放在 `dist/web`、`dist/windows` 这样的子文件夹里。 |
| `--platform <list>` | 逗号分隔的目标平台，见下表。`all` 表示 `win,lin,mac,web`。 |
| `--icon <file>` | 应用图标（`.svg`、`.png`、`.jpg`、`.bmp`、`.ico`），会覆盖清单里的 `icon`。 |
| `--pack` | 把 `[includes]` 资源和程序嵌入可执行文件，变成一个独立的文件。 |
| `--aot` | 提前编译成机器码，而不是嵌入解释器。 |

| 平台 | 别名 | 产物 |
|---|---|---|
| `web` | | WebGL/WebAssembly 包，由 `lingc` 构建 |
| `win` | `windows` | Windows 可执行文件 |
| `lin` | `linux` | Linux 可执行文件 |
| `mac` | `macos`, `darwin` | macOS（Intel）可执行文件 |
| `macarm` | `mac-arm`, `mac-arm64`, `macos-arm64`, `apple-silicon` | macOS（Apple 芯片）可执行文件 |
| `pios` | `pi`, `raspios`, `linux-arm64`, `linarm`, `arm64` | Raspberry Pi OS / 64 位 ARM Linux |
| `kernel` | `bare` | LingOS 裸机内核镜像（`os` 配置） |
| `rpi` | `raspberrypi` | 树莓派用 LingOS 镜像（`os` 配置） |
| `lingos` | `app`, `apps` | LingOS 用户态应用（`os` 配置） |
| `lingos-rpi` | `apps-rpi`, `app-rpi` | 树莓派用 LingOS 用户态应用（`os` 配置） |

不加 `--platform` 时，构建使用清单里的 `targets`；没有的话就构建本机平台加 `web`。
如果平台列表来自清单，本机无法构建（且没有安装 `cross`）的桌面平台会被跳过并给出提示。
显式指定 `--platform` 时则会直接报错。

原生平台会生成并编译一个小型 Rust 项目，所以**必须安装 Rust 工具链**，第一次构建可能要几分钟。

#### 清单键

`ling build` 按行读取清单，而不是用完整的 TOML 解析器。每个设置都要写在一行里，
格式为 `key = value`，数组也要写在一行内。

| 键 | 含义 |
|---|---|
| `name` | 项目名，用作可执行文件名。 |
| `version` | 版本字符串。默认 `0.1.0`。 |
| `entry` | 相对于清单的入口程序。默认 `main.ling`。 |
| `kind` | `bin`（默认）、`web`、`game`、`ui`、`ai`、`crypto`、`lib`、`polyglot`、`kernel`。 |
| `icon` | 图标文件。 |
| `targets` | 默认平台，例如 `targets = ["win", "lin", "web"]`。 |
| `pack` | `pack = true` 等同于 `--pack`。 |
| `includes` | 资源通配符，或者一个每行一个带引号模式的 `[includes]` 段。`*` 匹配文件夹内部，`**` 跨文件夹匹配。 |
| `ling-features` | 要在可执行文件中启用的 `ling` 的 Cargo 特性，例如使用 `db_*` 的程序需要 `["web"]`。 |

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

包含的文件会按原有的目录结构复制到可执行文件旁边，或者用 `--pack` 嵌入。
无论哪种方式，`read_file` 等文件内置函数都照常工作。

### `ling ast`

把一个文件或整个项目的语法树画成 SVG。目录会被递归遍历（跳过隐藏文件夹、`target`、`dist`、
`node_modules`、`AST` 和 `灵碑`），并当作一个程序处理。

| 选项 | 含义 |
|---|---|
| `[path]` | 文件或目录，默认 `.`。 |
| `--technical`、`--artwork`、`--ling` | 选择一种或多种绘制风格。都不选时三种都生成。 |
| `--all` | 全部三种风格。 |
| `--out <dir>` | 输出文件夹，默认 `AST`。 |

```text
$ ling ast hello.ling
ling ast: 'hello' — 1 file(s), 0 fn(s)
  ✓ AST\hello.technical.svg
  ✓ AST\hello.artwork.svg
  ✓ AST\hello.ling.svg
```

### `ling visualize`

把一个文件的语法树画成单张 SVG，输出到标准输出。把它重定向到文件：

```sh
ling visualize hello.ling > hello.svg
```

它只解析文件，所以也可以用来快速确认程序能否通过语法分析。

### `ling convert`

把资源转换成可以 `载` 入的 `.ling` 文件。参见[资源转换](convert.md)。

```sh
ling convert model.glb -o model.ling
```

## 退出状态

`ling` 成功时返回 0，报告任何错误时返回 1。
