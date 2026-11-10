# 花园

源代码：[`examples/audiovisual/Garden.ling`](https://github.com/taellinglin/ling/blob/master/examples/audiovisual/Garden.ling)

一座开放的户外花园：地面、天空、中央的池塘、前景的两棵树、花坛、后方的一排树和一间木棚。没有墙也没有天花板。
所有东西都用矢量几何（`vtex_*`）绘制，而且每种形状都有自己的音调，所以这个场景既能看也能听。

在仓库根目录运行：

```sh
ling run examples/audiovisual/Garden.ling
```

## 场景

| 部分 | 文件中的函数 |
|---|---|
| 草地、石板路和花坛 | `draw_ground` |
| 太阳、飘动的云和地平线的光晕 | `draw_sky` |
| 螺旋树干、花朵树冠的树 | `draw_tree`、`draw_trees` |
| 散布的花坛 | `draw_flowers` |
| 漂着莲花、带喷泉和涟漪的池塘 | `draw_pond` |
| 有门、窗和屋顶的木棚 | `draw_shed` |
| 来自之前帧的两层淡淡的“回声”残影 | `draw_echo` |

形状来自 `vtex_flower`、`vtex_lotus`、`vtex_chakra`、`vtex_rings`、`vtex_grid`、`vtex_spiral`、`vtex_star`、
`vtex_yantra`、`vtex_hyperbolic_uv` 和 `vtex_tessellated`。一个小小的哈希函数 `sm2_hash` 让每块地面和每块木板
都有各自不同的细节密度。

## 声音

六个音调声部使用 G 大调五声音阶（G A B D E）。每个声部都用 `audio_tone` 放在场景中自己的位置上，分布在池塘、
木棚和天空周围，`audio_listener` 跟随摄像机。

## 摄像机

摄像机沿着一条缓慢、蜿蜒的路径绕着池塘飘动，始终望向花园中央。颜色大约每两秒循环一遍整个色谱。

## 可以试试

- 修改 `orbit_h` 和 `orbit_v`，让摄像机离得更近或更远。
- 用 `PX` 和 `PZ` 移动池塘，用 `SX` 和 `SZ` 移动木棚。
- 修改五声音阶的频率 `PG3` 到 `PE4`，给花园换个调。
