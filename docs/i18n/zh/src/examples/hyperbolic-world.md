# 双曲世界（H²×ℝ）

源代码：[`examples/physics/hyperbolic_world.ling`](https://github.com/taellinglin/ling/blob/master/examples/physics/hyperbolic_world.ling)

一个小小的方块世界，地面是**双曲平面**。它的灵感来自 [Hypermine](https://github.com/Ralith/hypermine)，
Hypermine 的舞台是完整的三维双曲空间（H³）。这个演示用的是最简单、可以玩的近亲 **H²×ℝ**：你在双曲平面
（画成庞加莱圆盘）上行走，而竖直方向的重力和平常一样。

在仓库根目录运行：

```sh
ling run examples/physics/hyperbolic_world.ling
```

## 操作

| 按键 | 动作 |
|---|---|
| `W` / `S` | 前进 / 后退 |
| `A` / `D` | 向左 / 向右平移 |
| `Q` / `E` | 向左 / 向右转 |
| `Space` | 跳跃 |

## 为什么说它是双曲的

- **越往边缘越拥挤。** 方块排列在一圈圈的环上，每一圈都比里面那圈多（`cells = 6 * ring`）。圆盘的边缘代表无穷远，
  越靠近边缘，方块就越密集，并按系数 `1 − |z|²` 缩小。
- **莫比乌斯移动。** 你一直待在圆盘中心，整个世界通过莫比乌斯平移 `z ↦ (z + a) / (1 + ā z)` 在你脚下滑动。
  转身则是让世界绕中心旋转。
- **和乐（holonomy）。** 沿着一个闭合路线走一圈，比如正方形，回来时你会发现自己转了个角度。这正是在负曲率空间中移动的感觉。
- **普通的重力。** 跳跃和下落用的是普通的竖直轴，所以手感和别的游戏一样。

## 不用纹理的方块

每个方块把自己所在的环和位置做哈希（`hash01`，一种 `fract(sin(...))` 哈希），得到类型（草、石头、泥土或水晶）
和高度。颜色由类型决定，剩下的交给卡通着色设置（`set_shade_mode`、`set_cel_bands`、`set_rim`）。不需要任何纹理文件。
详见[后期特效与光照](../reference/postfx.md)。

## 可以试试

- 方块是用 `cube` 画的。试试 `cylinder` 或 [3D 形状与网格](../reference/shapes.md)中的其他形状。
- 把环数上限（`ring <= 4`）调大，让世界更大。
- 修改 `STEP` 和 `TURN`，走得或转得更快。

想看另一种 3D 演示，请见[双曲 3D](hyperbolic-3d.md)。
