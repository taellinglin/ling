# 道场 — 神殿房间

源代码：[`examples/3d-rooms/ling-dao-chamber.ling`](https://github.com/taellinglin/ling/blob/master/examples/3d-rooms/ling-dao-chamber.ling)

一座安静的中日风格神殿，完全用矢量几何（`vtex_*`）绘制，没有任何像素填充。所有东西都以金、玉、赤三色缓缓移动，
背景里响着轻柔的五声音阶钟声。这个程序主要用泰语关键字和内置函数编写，中间夹杂了几个中文名字。

在仓库根目录运行：

```sh
ling run examples/3d-rooms/ling-dao-chamber.ling
```

## 房间里有什么

| 部分 | 使用的内置函数 |
|---|---|
| 远处的鸟居 | `vtex_torii` |
| 鸟居后面的五层宝塔 | `vtex_pagoda` |
| 祭坛曼陀罗：齿轮环、法轮、莲花、圆环 | `vtex_spiked_cog`、`vtex_chakra`、`vtex_lotus`、`vtex_rings` |
| 带吉祥图案（yantra）边框的石砖地面 | `vtex_grid`、`vtex_yantra` |
| 天花板上的星形灯笼 | `vtex_star`、`vtex_rings` |
| 两侧墙上的水墨面板 | `vtex_letter_rain`、`vtex_lotus` |
| 四角的柱子 | `vtex_rings`、`vtex_spiked_cog`、`vtex_lotus` |

## 声音

五个音构成中国五声音阶 宫 商 角 徵 羽（C4、D4、E4、G4、A4），另外还有一个微弱的低音。每个音都用 `audio_tone`
放在 3D 空间中，`audio_listener` 跟随摄像机，所以视角摆动时钟声的方位也会变化。

## 摄像机与光照

摄像机像一个缓慢的钟摆，向两侧各摆动约 30 度，始终朝向祭坛。五盏灯（`add_light`）分别是顶部的金色主灯、
祭坛的暖光、柱子上的玉色和赤色点缀，以及鸟居处的一盏灯。

## 可以试试

- 修改文件开头的色相基准 `GOLD`、`JADE` 和 `CRIMSON`。
- 调大 `orbit_spd`，让摄像机动得更快。
- 修改 `init_audio` 中的频率，换一种音阶。

这里用到的内置函数见 [3D 形状与网格](../reference/shapes.md)和[音频](../reference/audio.md)。
