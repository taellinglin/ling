# lingfu

`lingfu`（灵符）是 Ling 的项目管理器。它可以创建项目、运行项目和测试、构建可执行文件、
与包注册中心交互，还能把整个项目改写成另一种人类语言。运行和构建时它会调用 `ling`，
先在自己旁边找，再去 `PATH` 里找。

```text
lingfu <command> [arguments]
lingfu --help | -h | help
lingfu --version | -V | version
```

`lingfu` 会在当前文件夹及其上级文件夹中查找清单来确定项目：`Ling.toml`、`灵符.toml`、
`霊符.toml`、`영부.toml` 或 `ลิงฟู.toml`。

## 命令一览

| 命令 | 别名 | 用途 |
|---|---|---|
| `new <name>` | `create` | 创建项目文件夹。 |
| `init` | `initialize` | 在当前文件夹中创建项目。 |
| `run` | `execute`, `start` | 运行项目。 |
| `test` | `verify` | 运行项目的测试。 |
| `build` | `compile`, `make` | 构建原生可执行文件。 |
| `clean` | `purge`, `clear` | 删除构建产物。 |
| `tree` | `structure`, `ls` | 打印项目目录树。 |
| `manifest` | `show`, `info` | 打印清单。 |
| `normalize <lang>` | `norm`, `localize` | 把项目改写成另一种语言。 |
| `add <package>` | `install`, `include` | 从注册中心安装包。 |
| `search <query>` | `find`, `seek` | 搜索注册中心。 |
| `publish` | `release`, `share` | 发布项目。 |
| `login <api-key>` | `sign-in`, `auth` | 保存注册中心凭据。 |
| `logout` | `sign-out`, `deauth` | 删除凭据。 |
| `doc` | `docs`, `document` | 构建文档。 |
| `bench`, `update` | | 运行 `cargo bench` / `cargo update`。 |
| `wizard` | `guide`, `create-wizard` | 交互式创建项目。 |
| `seed` | | 项目的完整性指纹。 |
| `ast`, `convert` | | 转交给 `ling ast` / `ling convert`。 |
| `check`, `fmt`, `translate` | | 暂时不要使用（见下文）。 |

常用选项：`--lang`/`-l <code>`（脚手架语言：`en`、`zh`、`ja`、`ko`、`th`、`ru`）、
`--type`/`-t <kind>`、`--root-lang <code>`、`--crates`、`--path <dir>`、`--dry-run`/`-n`。

每个命令还有其他语言的名字，例如中文用 `试` 表示 `test`，泰文用 `ทดสอบ`。
这些名字只有在可执行文件本身改成本地化名字（如 `灵符.exe` 或 `ลิงฟู`）时才生效。
以 `lingfu` 这个名字运行时，请使用英文命令。

### `lingfu new` 和 `lingfu init`

```text
lingfu new <name> [--type bin|lib|game|ui|ai|crypto|web|polyglot] [--lang en|zh|ja|ko|th|ru]
```

创建 `<name>/`，其中包含清单、带入口程序的源码文件夹、测试文件夹、词库文件夹、`README.md`、
`.gitignore` 和 `ling.ignore`。`lingfu init` 在当前文件夹中做同样的事，并用文件夹名作为项目名。

`--lang` 决定生成文件的语言，**同时**决定文件夹的名字：

| 语言 | 清单 | 源码 | 入口 | 测试 | 构建 |
|---|---|---|---|---|---|
| 英文 | `Ling.toml` | `src` | `start.ling` | `tests` | `build` |
| 中文 | `灵符.toml` | `灵源` | `启.灵` | `灵镜` | `灵碑` |
| 日文 | `霊符.toml` | `霊源` | `啓.霊` | `霊鏡` | `霊碑` |
| 韩文 | `영부.toml` | `령원` | `시작.령` | `령경` | `령비` |
| 泰文 | `ลิงฟู.toml` | `ต้นกำเนิด` | `เริ่ม.ลิง` | `ทดสอบ` | `สร้าง` |

清单包含这些表：`[spirit]`（`name`、`kind`、`version`，可选 `entry`）、`[author]`、
`[realm]`（`language`、`level`、`source`）、`[root]`（`core` 依赖）和 `[depends]`。
示例见[运行程序](../getting-started/running.md#lingtoml)。

### `lingfu run`

用 `ling run` 运行项目的入口程序。入口是下列文件中第一个存在的：清单中的 `entry`、
`<src>/<入口文件>`、`<root>/<入口文件>`、`<src>/main.ling`、`<root>/main.ling`。
额外的参数会传给 `ling run`。

```text
$ lingfu run
Running ...\demo\src\start.ling...
🌊 Welcome to demo Spirit Realm 🌊
Spirit Level: 铁
```

### `lingfu test`

用 `ling run` 逐个运行测试文件夹下的每个 `.ling` 文件，遇到第一个失败就停止。
测试就是一个普通的、带入口的程序（任何绑定到 `执` 代码块的 `令` 都算入口）。

```text
$ lingfu test
Testing Ling project...
  test: ...\demo\tests\test.ling
✨ All mirrors are clear ✨
🔮 Ling tests passed for demo
✓ All tests passed!
```

测试在 JIT 上运行，而 JIT 遇到未知函数不会停下，所以有问题的测试也可能“通过”。
让每个测试打印清楚的 PASS 或 FAIL，或者自己用 `ling run --interp` 运行。
测试文件夹里的辅助文件也会被当成测试。

### `lingfu build`

调用 `ling build <project> --aot`，并把 `--out`、`--platform`、`--icon`、`--pack` 传过去。
`--no-aot` 改为嵌入解释器；`--path <dir>` 指定项目文件夹。如果清单旁边有 `Cargo.toml`，
则改为运行 `cargo build`。

第一次构建之前，请在 `[spirit]` 下加上 `entry = "src/start.ling"`。否则 `ling build`
会去找 `main.ling`，并以 `entry '...' not found` 停止。构建需要 Rust 工具链，
参见 [ling 命令](ling.md#ling-build)。

### `lingfu clean`、`tree`、`manifest`

- `clean` 删除构建文件夹和锁文件夹（`build` 和 `.lock`，或它们的本地化名字），
  如果有 Rust crate 还会运行 `cargo clean`。
- `tree` 打印项目的目录树。
- `manifest` 打印清单的路径和内容。

### `lingfu normalize`

```text
lingfu normalize <lang> [--dry-run|-n] [--content-only] [--files-only]
```

把当前文件夹里的整个项目改写成一种人类语言。它会翻译每个源文件中的关键字和内置函数名，
重命名源文件和标准文件夹，并相应地修正导入。`<lang>` 是 `en`、`zh`、`ja`、`ko`、`th`
这样的代码（`chinese`、`中文` 这样的名字也可以）。不给出语言时会显示一个菜单。

| 选项 | 效果 |
|---|---|
| `--dry-run`, `-n` | 只列出会发生的改动，不修改任何东西。 |
| `--content-only` | 改写文件内容，保留名字。 |
| `--files-only` | 重命名文件和文件夹，保留内容。 |

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

> **警告：** `normalize` 会就地改写文件夹中的**每一个** Ling 文件。请先提交项目（或复制一份），
> 并先做一次试运行。

标识符、注释和字符串里的文本保持不变，只改关键字和内置函数。`normalize` 还使用自己的文件夹名
（`源码`、`测试`、`构建`），与 `lingfu new --lang zh` 生成的名字不同。

### `lingfu check`、`fmt`、`translate`

这些命令存在，但它们调用的是 `ling` 中并不存在的子命令（`ling check`、`ling fmt`、
`ling <from> <to> <file>`）。`check` 和 `fmt` 什么都不做就报告成功，`translate` 只会打印
`ling` 的用法说明。要改变项目的语言，请使用 `lingfu normalize`。

### 注册中心命令

注册中心默认是 `https://fu.ling-lang.org`。设置 `LINGFU_REGISTRY` 可以换成别的。

| 命令 | 作用 |
|---|---|
| `login <api-key>` | 把密钥保存到 `~/.lingfu/credentials.toml`。 |
| `logout` | 删除保存的凭据。 |
| `publish` | 把项目打包成 `.tar.gz`（跳过 `ling.ignore` 中列出的文件）并上传。需要先 `login`。 |
| `search <query>` | 搜索注册中心。加 `--crates` 则改为搜索 crates.io。 |
| `add <package>` | 把包下载到 `./ling_modules/<name>/`，并打印导入它所需的 `use` 行。加 `--crates` 则运行 `cargo add`。 |

### `lingfu doc`、`bench`、`update`

如果有 `docs/book.toml` 或 `book/book.toml`，`doc` 会运行 `mdbook build`，否则运行
`cargo doc --no-deps`。`bench` 和 `update` 分别运行 `cargo bench` 和 `cargo update`，
只对包含 Rust crate 的项目有用。

### `lingfu seed`

为 git 项目生成加密的完整性指纹：所有被跟踪文件的 BLAKE3 默克尔根，写成一个可以播放的
`seed.wav`。`--check` 会把项目与保存的指纹进行比较。需要 git 仓库。

### `lingfu wizard`

一个交互式问卷（语言、项目类型、名字等），最后像 `new` 一样创建项目。
它从终端读取输入，所以无法写成脚本。

## 退出状态

`lingfu` 成功时返回 0。失败时会打印 `Error: <message>`（例如 `build failed` 或
`tests failed`）并返回 1。
