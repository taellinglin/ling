# Crate 概览

Ling 用 Rust 编写，并拆分成一个由多个 crate 组成的工作区。主 crate `ling-lang` 包含编译器前端、
解释器、JIT 驱动和 `ling` 命令；下面的各个 crate 提供语言的组成部分和内置函数库。
只有当你想在自己的 Rust 代码里使用这些 crate，或者想了解哪个 crate 支撑哪些内置函数时，才需要看这一页。

下面的版本号是撰写本文时仓库中的版本。大多数 crate 沿用 Ling 的 `2030.x` 编号；
`ling-http` 和 `ling-web` 有自己的版本号。

## 主 crate

| Crate | 版本 | 说明 |
|---|---|---|
| `ling-lang` | 2030.1.53 | 语言本身：词法分析器、语法分析器、解释器、Cranelift JIT 与 AOT，以及内置函数。库名为 `ling`；可执行文件有 `ling`、`lingc`、`lx`、`ling-repl`。 |
| `lingfu`（`crates/ling-fu`） | 2030.1.19 | 项目管理器：`lingfu new`、`run`、`build`、`normalize` 等。参见 [lingfu](../tools/lingfu.md)。 |

## 编译器

| Crate | 版本 | 说明 |
|---|---|---|
| `ling-core` | 2030.0.9 | 核心类型、错误和编译器基础设施。 |
| `ling-lex` | 2030.0.8 | 词法分析和多语言分词。 |
| `ling-ast` | 2030.0.8 | 抽象语法树类型。 |
| `ling-polyglot` | 2030.0.8 | 词库系统，提供 16 种以上人类语言的关键字和内置函数名。 |
| `ling-mir` | 2030.0.12 | 供代码生成器使用的中层中间表示。 |
| `ling-codegen` | 2030.0.13 | 代码生成后端（字节码、WASM、原生）。 |
| `ling-macro` | 2030.0.8 | Ling 生态的过程宏。 |
| `ling-runtime` | 2030.0.8 | 运行时支持：垃圾回收器、分配器、标准库。 |
| `ling-wasm` | 2030.0.16 | `ling build --platform web` 使用的 WebGL2 WebAssembly 运行时。 |

## 图形、界面与输入

| Crate | 版本 | 说明 |
|---|---|---|
| `ling-graphics` | 2030.0.13 | 3D/4D 渲染、几何、动画和字体工具。 |
| `ling-gl` | 2030.0.10 | 投影场景的 GPU 光栅化器（wgpu：Vulkan、Metal、DX12；在 wasm 上使用 WebGL2）。 |
| `ling-gpu` | 2030.0.10 | GPU 计算（通过 cudarc 使用 CUDA，并带有 CPU 后备）。 |
| `ling-ui` | 2030.0.9 | 保留模式控件和弹性布局（`ui_*`）。 |
| `ling-animation` | 2030.0.8 | Anima，统一的动画系统：一套骨骼，适用于任何目标。 |
| `ling-input` | 2030.0.9 | Sensorium：手柄、摇杆、触觉反馈、体感、XR 控制器、触屏控件。 |
| `ling-paint` | 2030.0.1 | LingAnime 绘画引擎：图层、画笔、矢量、网点、2.5D 骨骼、数位板（`art_*`）。 |
| `ling-icon` | 2030.0.9 | 把 SVG/PNG 图标栅格化为多分辨率 `.ico`，并嵌入 Windows 可执行文件（`ling build --icon`）。 |

## 游戏与物理

| Crate | 版本 | 说明 |
|---|---|---|
| `ling-game` | 2030.1.9 | 游戏引擎：ECS、物理和场景管理。 |
| `ling-physics` | 2030.1.18 | 物理、世界生成、双曲几何、glTF 加载。 |
| `ling-body` | 2030.0.2 | 声学形体：人形 SDF 骨架、FDTD 波动求解器、膜、网格与 WAV 编解码。 |
| `ling-emu` | 2030.0.1 | 主机模拟宿主（`emu_*`）：SNES 和 N64 核心。 |

## 音频与音乐

| Crate | 版本 | 说明 |
|---|---|---|
| `ling-audio` | 2030.0.13 | 4D 定位音频合成和 WAV 背景音乐。 |
| `ling-music` | 2030.0.9 | 音乐解码、BPM 与调性分析、合成、节奏游戏和卡拉 OK 工具。 |
| `ling-mic` | 2030.0.12 | 麦克风输入和实时音频采集。 |
| `ling-daw` | 2030.0.2 | LingStationX 背后的 DAW 引擎：合成器、鼓、效果器、混音器、音序器。 |
| `ling-osdaw` | 2030.0.2 | LingOS 版 LingStation：`no_std` 的 tracker 音序器和合成器。 |

## 网络、Web 与安全

| Crate | 版本 | 说明 |
|---|---|---|
| `ling-http` | 0.1.8 | 基于 axum 的 HTTPS/JSON/SQLite 框架。Ling 的 `http_serve` 就运行在它上面。参见 [ling-http 与 ling-web](web.md)。 |
| `ling-web` | 0.1.9 | `ling-http` 之上的 Flask 风格层：会话、闪现消息、模板、静态文件、蓝图、CSRF。 |
| `ling-net` | 2030.0.9 | 异步网络：HTTP、WebSocket、QUIC 和点对点。 |
| `ling-crypto` | 2030.1.13 | 后量子与经典密码学：ML-KEM-768、混合 KEM、AES-GCM、XChaCha20、BLAKE3、SHA-3、Argon2id、Shamir、Schnorr 零知识证明、VRF。支持 `no_std`。 |

## AI

| Crate | 版本 | 说明 |
|---|---|---|
| `ling-ai` | 2030.0.10 | 张量、神经网络和 LLM 推理。 |
| `ling-llm` | 2030.0.2 | 面向 Qwen3 的 `no_std` GGUF 推理引擎，在 CPU 上运行，不需要操作系统（用于 LingOS）。 |
| `ling-diffusion` | 2030.0.1 | 进程内的 Stable Diffusion（SD 1.x / SDXL），支撑 `sd_*` 内置函数。 |

## LingOS

| Crate | 版本 | 说明 |
|---|---|---|
| `ling-kernel` | 2030.1.37 | 最小内核运行时：`no_std` CPU 内建函数、VGA、串口、引导。 |
| `ling-user` | 2030.1.4 | LingOS 上 Ling 程序的用户态运行时。 |

## 绑定

| Crate | 版本 | 说明 |
|---|---|---|
| `ling-py` | 2030.0.8 | `ling-audio`、`ling-physics` 和 `ling-net` 的 Python 绑定。 |

## 各安装配置包含哪些 crate

[安装配置](../getting-started/install.md#安装配置)决定哪些 crate 会被编译进 `ling`：

| 安装配置 | Crate |
|---|---|
| `web` | `ling-http`、`ling-crypto`、`ling-icon`、`lingfu` |
| `privacy` | `web` 再加 `ling-net`、`ling-ui` |
| `game` | `ling-physics`、`ling-gpu`、`ling-game`、`ling-animation`、`ling-input`、`ling-body`、`ling-ui`、`ling-icon` |
| `ai` | `ling-ai`、`ling-music` |
| `audio` | `ling-audio`、`ling-music`、`ling-mic`、`ling-daw` |
| `art`（包含在 `full` 中） | `ling-paint` |
