# 运行程序

用 `ling run` 运行单个文件，用 `lingfu run` 运行整个项目。本页介绍这两种方式，
解释 Ling 执行代码的两种后端，以及如何把程序变成可以发布的成品。

## 运行一个文件

```sh
ling run hello.ling
ling hello.ling          # 同一件事的简写
```

`ling run` 会解析文件，找到入口（`令 启 = 执 { ... }`）并运行它。源文件的扩展名可以是
`.ling`、`.灵`、`.霊`、`.령` 或 `.ลิง`。

文件名后面的参数会留在命令行上，供程序读取：`get_args()` 返回完整的命令行，
`cli_arg("port", "8080")` 读取 `--port 99` 这样的选项，没有时返回 `"8080"`。

## JIT 还是解释器

Ling 有两个后端：

| 命令 | 后端 | 适合 |
|---|---|---|
| `ling run file.ling` | Cranelift JIT（默认） | 速度。先把程序编译成机器码。 |
| `ling run --interp file.ling` | 树遍历解释器 | 诊断。它是 Ling 行为的标准参照。 |

如果 JIT 无法编译某个程序，`ling run` 会让解释器运行整个程序。如果一个很大的程序编译太慢，
可以用 `--O0`、`--O1`、`--O2` 或 `--O3`（默认 `--O3`）降低 JIT 的优化级别。

### 程序“什么都没做”时，试试 `--interp`

在 JIT 上，调用一个不存在的函数只会返回空值，程序照常继续。下面的程序把函数名写错成了 `draw_rectt`：

```ling
令 启 = 执 {
    印("之前")
    draw_rectt(1, 2)
    印("之后")
}
```
```output
之前
之后
```

解释器会停下来，告诉你哪里出了问题：

```text
$ ling run --interp typo.ling
[detected language: Chinese (中文)]
之前
error[runtime]: unknown function 'draw_rectt'
 --> typo.ling
  traceback (deepest call last):
    0: start
  hint: check the spelling, or `use` the module that defines it
```

只要觉得某段代码好像被忽略了，就用 `--interp` 再跑一遍。两个后端的其他差异见[已知限制](../language/limits.md)。

### 服务器程序在解释器上运行

调用了 `http_serve` 或 `lingtp_serve` 的程序总是在解释器上运行，
即使没有加 `--interp`。请求处理函数是传给内置函数的闭包，而 JIT 目前还不支持这种用法。
Web 服务大部分时间都在等待网络，所以几乎感觉不到性能差别。参见[一个小型 Web 服务](../tutorials/web-service.md)。

### 在浏览器中运行

`ling run --wasm file.ling`（或 `--web`）会把程序构建成 WebAssembly，在本地 Web 服务器上提供，
并打开浏览器。第一次构建需要编译运行时，大约要一分钟。它需要 `ling` 旁边有 `lingc` 程序。

## 用 lingfu 管理项目

超过一个文件的程序，就让 `lingfu` 创建一个项目：

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

`lingfu new` 会生成这样的结构：

```text
demo/
├── Ling.toml          清单文件
├── README.md
├── .gitignore
├── ling.ignore        发布时要排除的文件
├── lexicons/en.ling
├── src/start.ling     入口程序
└── tests/test.ling
```

| 命令 | 作用 |
|---|---|
| `lingfu run` | 用 `ling run` 运行入口程序，额外的参数会传过去。 |
| `lingfu test` | 逐个运行 `tests/` 下的所有 `.ling` 文件。 |
| `lingfu build` | 用 `ling build --aot` 构建原生可执行文件。 |
| `lingfu clean` | 删除构建产物。 |

`lingfu new demo --lang zh` 会用中文创建项目，连文件夹和文件名也是中文：清单是 `灵符.toml`，
入口是 `灵源/启.灵`。所有命令见 [lingfu](../tools/lingfu.md)。

## Ling.toml

`lingfu new` 写出的清单文件如下：

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

- `[spirit]` 给项目命名。也可以在这里加上 `entry = "src/start.ling"`。
- `[realm]` 设置项目的语言和源码文件夹。
- `[root]` 和 `[depends]` 列出 `core` 依赖和你的包。

`lingfu run` 和 `lingfu test` 会自己找到 `src/start.ling`。而 `ling build`（也就是
`lingfu build`）在清单没有另行指定时会去找 `main.ling`，所以构建之前**请在 `[spirit]`
下加上 `entry = "src/start.ling"`**。

## 构建程序

`ling build` 把一个文件或项目文件夹变成可以分发的成品：

```sh
ling build game.ling                      # 本机平台 + web，输出到 dist/
ling build . --platform win,web --out dist
ling build . --platform all --pack --icon logo.svg
```

| 选项 | 含义 |
|---|---|
| `--out <dir>` | 输出文件夹（默认 `dist`）。每个平台一个子文件夹。 |
| `--platform <list>` | 逗号分隔：`web`、`win`、`lin`、`mac`、`macarm`、`pios`、`kernel`、`rpi`、`lingos`、`lingos-rpi`。`all` 表示 `win,lin,mac,web`。 |
| `--icon <file>` | 应用图标（`.svg`、`.png`、`.ico` 等）。 |
| `--pack` | 把清单 `[includes]` 中的文件嵌入可执行文件。 |
| `--aot` | 提前编译成机器码，而不是嵌入解释器。 |

不加 `--platform` 时，构建会使用清单里的 `targets` 列表；没有的话就构建本机平台加 `web`。
桌面平台会生成并编译一个小型 Rust 项目，所以需要安装 Rust 工具链，第一次构建可能要几分钟。
`web` 目标使用 `lingc` 程序。

完整的选项和清单键见 [ling 命令](../tools/ling.md)。
