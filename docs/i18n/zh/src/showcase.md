# 作品展示

下面这些应用和演示都是用 Ling 编写、在 Ling 运行时上运行的。应用截图请见官网：
[ling-lang.org/#apps](https://ling-lang.org/#apps)。

## 应用

| 应用 | 简介 |
|---|---|
| **LingStationX** | 数字音频工作站：编曲视图、钢琴卷帘、音轨器（tracker）、会话视图、节点图音色编辑器、混音台，以及 Nova、Prism 和 Growl 合成器。支持导入 MIDI，带切片器、时间伸缩和母带处理链。 |
| **LingDev** | 3D 游戏编辑器，带节点树、时间轴，以及用于摄像机、对话和特效的过场动画轨道。用它做的游戏有 Dance Ling、Ling Hazard、Ling Breaker、Moko、Ling Agent 和 Soul Symphony 系列。 |
| **LingModeler** | 建模、绑定骨骼、权重绘制、摆姿势和动画，还有矢量插画、带洋葱皮的 2D 动画和程序化纹理。它也能在 x86-64 和树莓派上的 LingOS 中运行。 |
| **LingForFun** | 在线动作 RPG，有大地图、城镇、地下城、飞行、战斗和任务。包含桌面客户端、WASM 浏览器客户端和服务器集群，全部用 Ling 编写，并且使用中文关键字。仍在开发中。 |
| **Ling Arcade** | 主机游戏库，内置用 Ling 编写的 NES 模拟器：6502 CPU、PPU、映射器和 APU 都是运行在 Ling JIT 上的普通 `.ling` 代码。它能完整通过 nestest 日志（8,991 条指令，周期精确）。 |
| **Ling Wallet** | 面向 LNG 的后量子隐私钱包，同时也是全节点和矿工：ML-DSA-87 身份，以及 ML-KEM-1024 + X25519 混合密钥交换。属于实验性质，未经审计。 |

## 社区项目

| 项目 | 简介 |
|---|---|
| [**Soul Symphony**](https://github.com/taellinglin/soul-symphony-ling-edition) | 一款 RPG 街机物理游戏。滚动一颗光球穿过 9 个程序生成的关卡，每关结束都有首领战。支持街机对战、联网和合作，提供 5 种语言。 |
| [**LingSoul**](https://github.com/taellinglin/LingSoul) | 一款氛围感十足的灵魂平衡模拟游戏，采用 Ling 的线框风格，游戏内 HUD 使用自创的字母表。 |

## 仓库里的演示

[Ling 仓库](https://github.com/taellinglin/ling)里有很多可以直接运行的示例。可以先试试这几个：

| 演示 | 展示内容 |
|---|---|
| `examples/music/hallway_runner.ling` | 随音乐变化的无尽跑酷，金币以空间音效奏出旋律 |
| `examples/music/karaoke_hero.ling` | 根据人声 MIDI 生成的卡拉 OK 音高条，带实时麦克风调音器 |
| `examples/music/synth_jam.ling` | 由 `.ling` 音色文件合成的 GM 乐器 |
| `examples/physics/bouncy_ball.ling` | 弹跳时会被压扁的弹簧质点软体 |
| `examples/physics/tumbling.ling` | 翻滚、滚动的刚体 |
| `examples/physics/liquid_marble.ling` | 在旋转的球体、圆锥和穹顶上的水油模拟 |
| `examples/ui/hud_showcase.ling` | UI 工具包：HUD、仪表、控件和游戏组件 |
| `examples/crypto/crypto_donut_physics.ling` | 由 SHA3 哈希决定形状的弹跳环面纽结，带有随之变化的音频 |

在仓库根目录运行其中一个：

```sh
ling run examples/physics/liquid_marble.ling
```

本文档的[示例](examples/dao-chamber.md)部分会更详细地介绍四个 3D 场景。
