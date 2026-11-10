# 安装

一条命令就能装好 Ling。你会得到两个程序：`ling` 用来运行和构建 Ling 程序，
`lingfu` 是项目管理器。

## 一行安装

```sh
# macOS / Linux
curl --proto '=https' --tlsv1.2 -sSf https://ling-lang.org/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://ling-lang.org/install.ps1 | iex
```

如果你还没有 Rust 工具链，脚本会先安装它（方式与 [rustup](https://rustup.rs) 相同），
然后从 crates.io 构建 `ling` 和 `lingfu`。

## 用 Cargo 安装

如果已经装好了 Rust，可以直接从 crates.io 安装这两个程序：

```sh
cargo install ling-lang lingfu
```

两者会装到同一个目录（`~/.cargo/bin`）。`lingfu` 会先在自己旁边找 `ling`，
找不到再去 `PATH` 里找。

## 从源码构建

```sh
git clone https://github.com/taellinglin/ling
cd ling
cargo build --release
# 可执行文件在 target/release/（ling，以及 lingc、lx 和 ling-repl）
```

要从同一份代码构建 `lingfu`，运行 `cargo build --release -p lingfu`。

## 确认安装成功

不带参数运行 `ling`。用法说明的第一行就是版本号：

```sh
ling
# ling 2030.1.53 — The Omniglot Systems Language
# Usage:
#   ling run <file.ling>   ...
lingfu --version
# lingfu v2030.1.19
```

`ling` 没有 `--version` 选项，查看版本就看这段用法说明。

## 安装配置

`cargo install ling-lang` 会安装全部功能（`full` 配置）。如果想少编译一些，
可以关掉默认值，只选需要的安装配置：

```sh
cargo install ling-lang --no-default-features --features core        # 只有编译器和解释器
cargo install ling-lang --no-default-features --features web         # 加上 HTTP 服务器/客户端、SQLite
cargo install ling-lang --no-default-features --features game,audio  # 带声音的游戏套件
```

| 安装配置 | 增加的内容 |
|---|---|
| `core` | 词法分析器、语法分析器、Cranelift JIT 与 AOT、解释器、软件窗口与绘图（`draw_*`、字体、图片、网格）。所有配置都包含它。 |
| `web` | `http_*` 服务器与客户端（[Web 服务器与 HTTP](../reference/web.md)）、`db_*` SQLite（[数据库](../reference/database.md)）、静态文件、TOTP、zip/tar、视频播放、进程、哈希与 `password_hash`。 |
| `privacy` | 后量子身份、`lingtp://`、点对点 `p2p_*`、Ling 钱包 `lw_*`、`ssh_*`，并包含 `web` 和 `ui_*` 控件。 |
| `game` | 物理（软体、刚体、液体、glTF）、GPU 光栅化、动画、对话、手柄、声学形体、`ui_*` 控件。 |
| `ai` | 神经网络（`nn_*`）、行为树（`bt_*`）、可学习的对话和语音转文字（`stt_*`）。 |
| `audio` | 声音引擎、音乐解码、合成与 MIDI、麦克风、LingStationX 数字音频工作站。 |
| `os` | `ling build --platform kernel`、`rpi`、`lingos`、`lingos-rpi`（LingOS 镜像）。 |
| `full` | 以上全部（默认）。另外还包含 `art`，即 LingAnime 绘画引擎（`art_*`）。 |

如果程序调用了当前构建没有包含的配置里的内置函数，它会报错并停止，错误信息会告诉你
需要用哪个配置重新安装。还有一些较重的附加功能，在任何配置之上都需要单独开启：
`llm`、`llm-cuda`、`vision`、`cuda`、`gpu-cuda`、`gpu`、`llvm`、`emu`。

## 系统要求

- Rust 1.85 或更新版本（安装脚本会帮你装好）。
- Linux：音频需要 `libasound2-dev`。
- macOS 和 Windows：不需要额外依赖。
- 为桌面平台执行 `ling build` 时会编译一个小型 Rust 项目，所以同样需要 Rust 工具链。
  参见 [ling 命令](../tools/ling.md)。

## 下一步

- 写第一个程序：[你好，世界](hello.md)。
- 学习常用命令：[运行程序](running.md)。
- 阅读 [Ling 之书](../books.md)，一套免费的四卷 PDF：入门、进阶、高级，以及完整参考。
