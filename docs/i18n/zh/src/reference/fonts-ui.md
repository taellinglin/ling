# 字体与界面

本页介绍文字与界面：以清晰矢量轮廓绘制的 TrueType 和 OpenType 字体、内置的笔画字体，以及由仪表、量表和控件组成的
`ui_*` 工具包。它们都和 [2D 绘图](drawing.md) 调用绘制到同一个帧缓冲区，所以应放在 `fill()` 和 `present()` 之间。
要把它们画在 3D 场景之上，请先调用 `flush_3d()`。打字机式对话框有单独的页面：[对话](dialog.md)。

## 字体

Ling 能绘制两种文字：

* **矢量字体。** `font_load` 读取 `.ttf` 或 `.otf` 文件；`font_text_fill`、`font_text` 和 `font_text_3d` 绘制其字形轮廓。
  曲线得以保留，所以文字在任何尺寸下都很清晰。只要字体包含相应字形，拉丁文、中文、日文、韩文和泰文都可以显示。
* **笔画字体。** `ui_text` 绘制一种简单的等宽“全息”字体，不需要字体文件，适用于基本的 ASCII 文本。

字形第一次使用时，其轮廓会缓存到程序运行目录下的 `cache/fonts/` 中，所以之后的运行启动更快。
在 Web 运行时中 `font_load` 返回 `-1`；如果希望程序在两处都能运行，请检查句柄并在失败时退回 `ui_text`。

```ling-gui
令 start = 执 {
    开窗(720, 320, "字体")
    令 拉丁 = 加载字体("assets/fonts/Exo2.ttf", 600)
    令 中日 = 加载字体("assets/fonts/NotoSansSC.ttf")
    循 窗开() {
        填(8, 10, 24)
        设色(0, 230, 255)
        填充字体(拉丁, 30, 40, 48, "Hello Ling")
        设色(255, 200, 80)
        字体文本(拉丁, 30, 110, 48, "Stroked Outline")
        设色(255, 255, 255)
        填充字体(中日, 30, 180, 40, "你好，灵语言")
        设色(120, 255, 160)
        界面文字(30, 260, 16, "UI TEXT 0123")
        显()
    }
}
```

### 概览：字体

| 内置函数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `font_load(path, weight)` | `加载字体` | `フォント読込` | `글꼴로드` | `โหลดฟอนต์` | 加载字体，得到句柄 |
| `font_text_fill(font, x, y, px, text)` | `填充字体` | `フォント塗り` | `글꼴채움` | `เติมฟอนต์` | 实心文字 |
| `font_text(font, x, y, px, text)` | `字体文本` | `フォント文字` | `글꼴텍스트` | `ข้อความฟอนต์` | 轮廓文字 |
| `font_text_3d(font, ..., size, text, rows)` | `字体3D` | `フォント3D` | `글꼴3D` | `ข้อความฟอนต์3มิติ` | 3D 平面上的文字 |
| `font_width(font, px, text)` | `字体宽度` | `フォント幅` | `글꼴너비` | `ความกว้างฟอนต์` | 以像素测量字符串宽度 |
| `font_advance(font, char)` | — | — | — | — | 单个字形的步进宽度（em） |
| `font_glyph_outline(font, char, tol)` | — | — | — | — | 以折线表示的字形轮廓 |
| `font_fallback(primary, fallback, scale)` | — | — | — | — | 缺字时使用的后备字体 |
| `set_font_antialias(on)`, `get_font_antialias()` | — | — | — | — | 平滑字形边缘 |
| `ui_text(x, y, scale, text)` | `界面文字` | `UI文字` | `UI텍스트` | `ข้อความหน้าจอ` | 内置笔画字体 |

### `font_load(path, weight)`

加载字体并返回句柄（第一个字体为 0，下一个为 1，依此类推）；无法读取时返回 `-1`，并在错误流中输出一条消息。
路径先按原样尝试，再相对于脚本所在文件夹尝试。`weight` 可选：对可变字体而言，600 之类的数值会得到粗体效果；
0 或省略则使用字体的默认字重。

Ling 仓库在 `assets/fonts/` 中附带 Exo2、Orbitron、Rajdhani、Noto Sans（SC、JP、KR、Thai）和 Galmuri。
如果程序可能从不同文件夹启动，可以再试一个路径：

```ling-gui
令 start = 执 {
    令 字 = 加载字体("assets/fonts/Exo2.ttf", 600)
    若 字 < 0 { 令 字 = 加载字体("../../assets/fonts/Exo2.ttf", 600) }
}
```

### `font_text_fill(font, x, y, px, text)` 和 `font_text(font, x, y, px, text)`

`font_text_fill` 用画笔颜色绘制实心字形；`font_text` 绘制字形轮廓，开销更小，并遵循 `set_blend` 和 `set_antialias`。
`x, y` 是这一行的**左上角**，而不是基线：大写字母大约从 `y` 下方 `0.3 * px` 处开始，基线约在 `y` 下方 `0.97 * px` 处。
`px` 是像素字号（默认 16）。除非调用 `set_font_antialias(1)`，否则边缘是锐利的。无效句柄不绘制任何内容。

### `font_text_3d(font, cx, cy, cz, ux, uy, uz, vx, vy, vz, size, text, fill_rows)`

在 3D 平面上绘制文字，与场景一起按深度排序，因此会随摄像机转动。`c` 是行的起点，`u` 是文字前进的方向（默认 `1, 0, 0`），
`v` 是字形的向上方向（默认 `0, 1, 0`），`size` 是一个 em 的世界单位高度（默认 1）。由于 +y 朝下，
竖直的文字需要 `v = (0, -1, 0)`。默认只画轮廓；当 `fill_rows` 大于 0 时，每个字形用这么多条扫描线填充。

```ling-gui
令 start = 执 {
    令 字 = 加载字体("assets/fonts/Exo2.ttf", 600)
    设色(120, 230, 255)
    字体3D(字, -2, 0, 8, 1, 0, 0, 0, -1, 0, 0.6, "LING 3D")
}
```

### `font_width(font, px, text)`

字符串的像素宽度（无效句柄为 0）。注意顺序：字号在文本**之前**。要把标签居中放在方框 `bx, by, bw, bh` 中，
在 `x = bx + (bw - font_width(f, px, text)) / 2`、`y = by + bh / 2 - 0.64 * px` 处绘制。

### `font_advance(font, char)` 和 `font_glyph_outline(font, char, tol)`

`font_advance` 以 em 为单位给出一个字符的步进宽度（乘以像素字号即得像素）。
`font_glyph_outline` 返回字符的轮廓，供你制作自己的效果：一个轮廓列表，每个轮廓是 em 单位的扁平列表 `[x0, y0, x1, y1, ...]`，
基线为 0，**y 朝上**。`tol` 是折线偏离真实曲线的最大距离（默认 0.01 em）。失败时为空。

### `font_fallback(primary, fallback, scale)`

对 `primary` 缺少的每个字符改用 `fallback`，例如在一款时尚的拉丁字体后面放一款覆盖面广的 Unicode 字体。
`scale` 设定后备字体相对主字体的大小（默认 1）。把 `fallback` 设为 `-1` 可以移除它。
它返回主字体句柄；句柄无效时返回 `-1`。

```ling-gui
令 start = 执 {
    令 拉丁 = 加载字体("assets/fonts/Orbitron.ttf", 600)
    令 宽字库 = 加载字体("assets/fonts/NotoSansSC.ttf")
    font_fallback(拉丁, 宽字库, 0.9)
}
```

### `set_font_antialias(on)` 和 `get_font_antialias()`

开启或关闭字形平滑（默认关闭，文字锐利）。不带参数的 `set_font_antialias()` 会开启它。
它与只影响线条笔画的 `set_antialias` 相互独立。

### `ui_text(x, y, scale, text)`

用内置笔画字体、以画笔颜色绘制文字。`x, y` 是左上角；字形高 `scale` 像素（默认 16），每个宽 `scale * 0.62`，
间距 `scale * 0.24`。字母画成大写，字体中没有的字符（包括所有非 ASCII 文字，例如中文）显示为小方框。

## 界面工具包

`ui_*` 内置函数是一个**即时模式**工具包。帧与帧之间不保存任何东西：你每帧用当前值调用每个控件，它绘制自身，
交互控件则返回新值。所有状态都由你的程序保存在普通变量中。

```ling-gui
令 start = 执 {
    开窗(480, 220, "控件")
    令 音量值 = 0.4
    令 已启用 = 0
    循 窗开() {
        填(6, 10, 22)
        令 音量值 = 滑块(40, 60, 280, 音量值, 0, 1)
        令 已启用 = 开关(40, 100, 52, 24, 已启用)
        若 按钮(40, 150, 120, 36) == 1 { println("点击了") }
        设色(220, 240, 255)
        界面文字(64, 160, 14, "PLAY")
        显()
    }
}
```

通用约定：

* **坐标**是屏幕像素。矩形控件接受 `x, y, w, h`；圆形控件接受中心 `cx, cy` 和半径 `r`。
* **颜色**来自主题（`ui_theme`）。大多数控件接受可选的末尾 `r, g, b`，在本次调用中覆盖其主色。三个要么全给，要么都不给。
* **控件不绘制文字。** 按钮没有标签，标签页没有标题。请用 `ui_text` 或 `font_text_fill` 自己绘制。
* **交互**读取鼠标左键。按钮只在鼠标在其上按下的那一帧返回 `1`，所以要用 `== 1` 比较。这需要每帧调用一次 `present()`。
* **混合。** 控件遵循 `set_blend`，所以 `set_blend(1)` 会让它们发光。

### 概览：界面工具包

| 内置函数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `ui_theme(...)` | `界面主题` | `UIテーマ` | `인터페이스테마` | `ธีมส่วนติดต่อ` | 设置六种主题色 |
| `ui_theme_colors()` | — | — | — | — | 读回主题 |
| `ui_hot(x, y, w, h)` | `热区` | `ホットエリア` | `핫존` | `พื้นที่สัมผัส` | 鼠标是否在其中？ |
| `ui_frame(x, y, w, h, bracket)` | `边框` | `フレーム枠` | `프레임틀` | `กรอบ` | 角括号框 |
| `ui_bevel(x, y, w, h, bevel)` | `斜角框` | `ベベル枠` | `베벨틀` | `กรอบเฉียง` | 切角轮廓 |
| `ui_panel(x, y, w, h, bevel)` | `面板` | `パネル` | `패널` | `แผง` | 填充的背景面板 |
| `ui_scanlines(x, y, w, h, density)` | `扫描线` | `走査線` | `스캔라인` | `เส้นสแกน` | CRT 扫描线 |
| `ui_radar(cx, cy, r, sweep)` | `雷达` | `レーダー` | `레이더` | `เรดาร์` | 雷达屏 |
| `ui_compass(x, y, w, h, heading)` | `罗盘` | `コンパス` | `나침반` | `เข็มทิศ` | 罗盘条（度） |
| `ui_reticle(cx, cy, r, spread)` | `准星` | `照準` | `조준선` | `เป้าเล็ง` | 瞄准准星 |
| `ui_target(x, y, w, h, lock)` | `锁定框` | `ターゲット` | `표적` | `กรอบเป้า` | 目标锁定框 |
| `ui_bar(x, y, w, h, value, max)` | `进度条` | `バー` | `막대` | `แถบ` | 填充条 |
| `ui_segbar(x, y, w, h, value, max, segments)` | `分段条` | `分割バー` | `분할막대` | `แถบแบ่ง` | 分块条 |
| `ui_gauge(cx, cy, r, value, max)` | `仪表` | `ゲージ` | `게이지` | `มาตรวัด` | 带指针的表盘 |
| `ui_ring(cx, cy, r, value, max)` | `环表` | `リングメーター` | `링미터` | `วงแหวนวัด` | 进度环 |
| `ui_vu(x, y, w, h, levels)` | `音量条` | `VUメーター` | `음량막대` | `มาตรเสียง` | 来自列表的电平表 |
| `ui_spark(x, y, w, h, values)` | `迷你图` | `スパークライン` | `스파크라인` | `กราฟจิ๋ว` | 迷你折线图 |
| `ui_battery(x, y, w, h, value, max)` | `电池` | `バッテリー` | `배터리` | `แบตเตอรี่` | 电池图标 |
| `ui_progress(x, y, w, h, fraction)` | `进度` | `プログレス` | `진행바` | `ความคืบหน้า` | 细进度条 |
| `ui_button(x, y, w, h)` | `按钮` | `ボタン` | `버튼` | `ปุ่ม` | 按钮；点击时为 `1` |
| `ui_toggle(x, y, w, h, state)` | `开关` | `トグル` | `토글` | `สวิตช์` | 开关切换 |
| `ui_slider(x, y, w, value, min, max)` | `滑块` | `スライダー` | `슬라이더` | `แถบเลื่อน` | 水平滑块 |
| `ui_checkbox(x, y, size, checked)` | `复选框` | `チェックボックス` | `체크박스` | `ช่องเลือก` | 复选框 |
| `ui_tabs(x, y, w, h, count, active)` | `标签页` | `タブ` | `탭` | `แท็บ` | 标签条 |
| `ui_stepper(x, y, w, h, value, step)` | `步进器` | `ステッパー` | `스테퍼` | `ตัวปรับค่า` | 加减按钮 |
| `ui_tooltip(x, y, w, h)` | `提示框` | `ツールチップ` | `툴팁` | `คำแนะนำ` | 提示气泡 |
| `ui_healthbar(x, y, w, h, value, max, pulse)` | `血条` | `体力バー` | `체력바` | `แถบพลังชีวิต` | 游戏血条 |
| `ui_cooldown(cx, cy, r, fraction)` | `冷却` | `クールダウン` | `쿨다운` | `คูลดาวน์` | 冷却扇形 |
| `ui_counter(x, y, dw, dh, value, digits)` | `计数器` | `カウンター` | `카운터` | `ตัวนับ` | 七段数码管数字 |
| `ui_minimap(x, y, w, h)` | `小地图` | `ミニマップ` | `미니맵` | `แผนที่ย่อ` | 小地图框 |
| `ui_dpad(cx, cy, r)` | `方向键` | `方向パッド` | `방향패드` | `ปุ่มทิศทาง` | 屏幕方向键 |
| `ui_slotgrid(x, y, cols, rows, cell, selected)` | `物品格` | `スロットグリッド` | `슬롯격자` | `ช่องไอเทม` | 物品栏网格 |
| `ui_vignette(intensity)` | `暗角` | `ビネット` | `비네트` | `ขอบมืด` | 屏幕边缘警示光晕 |
| `ui_gauge3d(cx, cy, r, value, max, spin)` | `立体仪表` | `立体ゲージ` | `입체게이지` | `มาตรวัด3มิติ` | 旋转环形量表 |
| `ui_panel3d(x, y, w, h, depth)` | `立体面板` | `立体パネル` | `입체패널` | `แผง3มิติ` | 立体凸起面板 |
| `ui_radar3d(cx, cy, r, tilt, sweep)` | `立体雷达` | `立体レーダー` | `입체레이더` | `เรดาร์3มิติ` | 倾斜的雷达盘 |
| `ui_sound(name)` | `界面音` | `UI音` | `인터페이스음` | `เสียงปุ่ม` | 简短的界面音效 |

### `ui_theme(...)` 和 `ui_theme_colors()`

`ui_theme` 设置所有控件使用的颜色：六组 `r, g, b`，顺序为 primary（主色）、accent（强调色）、track（仪表的暗色背景）、
warn（警告色）、text（文字色）和 bg（面板填充色）。省略的组保持原值，所以 `ui_theme(255, 0, 128)` 只改变主色。
`ui_theme_colors()` 以 18 个数字的列表返回当前主题，方便手绘的界面与之匹配：

```ling
令 start = 执 {
    界面主题(255, 0, 128)
    println(ui_theme_colors())
}
```

```output
[255, 0, 128, 40, 255, 180, 44, 62, 100, 255, 90, 90, 190, 235, 255, 10, 16, 24]
```

输出中也能看到其余五种颜色的内置默认值。

### 布局辅助：`ui_hot`、`ui_frame`、`ui_bevel`、`ui_panel`、`ui_scanlines`

鼠标在矩形内时 `ui_hot` 返回 `true`；用它实现悬停效果和自己的控件。`ui_frame`（括号臂长，默认 14）和
`ui_bevel`（切角大小，默认 10）用画笔颜色绘制轮廓。`ui_panel` 绘制用主题 bg 色填充的斜角面板
（切角默认 12；第六个参数起可选边框颜色）。`ui_scanlines` 绘制 `density` 条水平线（默认 24）。

```ling-gui
令 start = 执 {
    面板(20, 20, 340, 200, 14)
    若 热区(40, 140, 120, 36) { 设色(255, 255, 255) } 否则 { 设色(120, 140, 160) }
    斜角框(40, 140, 120, 36, 8)
}
```

### HUD：`ui_radar`、`ui_compass`、`ui_reticle`、`ui_target`

`ui_radar` 绘制距离环和位于 `sweep`（弧度）角度的扫描线；用 `time_now()` 驱动它。`ui_compass` 显示以 `heading`（**度**）
为中心的罗盘条，每 15 度一个刻度。`ui_reticle` 绘制一个环，其缺口随 `spread` 从 0 到 1 而张开。
`ui_target` 绘制随 `lock` 从 0 到 1 而收拢的括号。

### 仪表：`ui_bar`、`ui_segbar`、`ui_gauge`、`ui_ring`、`ui_vu`、`ui_spark`、`ui_battery`、`ui_progress`

每种仪表显示 `value` 占 `max`（默认 1）的比例。`ui_segbar` 把条分成 `segments` 块（默认 10）。`ui_gauge` 是带指针的表盘；
`ui_ring` 顺时针填充。`ui_vu` 接受 0 到 1 的电平列表，每个值一根柱，数值越高越接近警告色。`ui_spark` 把至少两个数字的列表缩放到框内。
`ui_battery` 低于 25% 时变为警告色。`ui_progress` 接受单个 0 到 1 的 `fraction`。

```ling-gui
令 start = 执 {
    进度条(40, 70, 280, 14, 70, 100)
    分段条(40, 96, 280, 14, 6, 10, 10)
    仪表(450, 120, 50, 62, 100)
    音量条(330, 330, 160, 60, [0.2, 0.5, 0.8, 1.0, 0.7, 0.3])
}
```

### 控件：`ui_button`、`ui_toggle`、`ui_slider`、`ui_checkbox`、`ui_tabs`、`ui_stepper`、`ui_tooltip`

- `ui_button` 在悬停时高亮，被点击的那一帧返回 `1`，否则返回 `0`。
- `ui_toggle` 和 `ui_checkbox` 接受当前状态并返回新状态，`1` 或 `0`。
- `ui_slider` 没有高度参数（可拖动区域高 20 像素）。它返回新值，按住鼠标键时该值跟随光标。
- `ui_tabs` 绘制 `count` 个等宽标签，并返回当前选中的索引（从 0 开始）。用宽度除以 `count` 来放置标题。
- `ui_stepper` 在点击某一侧时返回 `value` 加或减 `step`。它不绘制数字。
- `ui_tooltip` 是提示气泡；在某物被悬停时把它画在光标附近。

```ling-gui
令 start = 执 {
    令 标签 = 0
    令 计数 = 3
    令 标签 = 标签页(20, 250, 300, 28, 3, 标签)
    令 计数 = 步进器(160, 178, 140, 28, 计数, 1)
    界面文字(222, 186, 14, str(计数))
}
```

### 游戏界面：`ui_healthbar`、`ui_cooldown`、`ui_counter`、`ui_minimap`、`ui_dpad`、`ui_slotgrid`、`ui_vignette`

`ui_healthbar` 空时为警告色，满时为强调色；`pulse`（0 到 1）让它在低于 30% 时向白色闪烁。`ui_cooldown` 从十二点方向
顺时针填充扇形。`ui_counter` 以 `digits` 位七段数码显示 `value`。`ui_minimap` 绘制一个空的地图框；用 `draw_disc` 自己画点。
鼠标按在 `ui_dpad` 上时，它返回 `0` 无、`1` 上、`2` 右、`3` 下或 `4` 左。`ui_slotgrid` 高亮第 `selected` 格（`-1` 表示不高亮）。
`ui_vignette` 沿屏幕边缘发光，可用作受伤闪烁。

```ling-gui
令 start = 执 {
    令 生命值 = 24
    血条(20, 300, 220, 16, 生命值, 100, 0.5 + 0.5 * 正弦(当前时间() * 8))
    计数器(20, 340, 16, 28, 1234, 5)
    物品格(400, 260, 5, 2, 40, 1)
}
```

### 伪 3D：`ui_gauge3d`、`ui_panel3d`、`ui_radar3d`

`ui_gauge3d` 绘制随 `spin` 转动、点亮到 `value` 的线框环带。`ui_panel3d` 把斜角面板抬离屏幕 `depth` 像素（默认 14）。
`ui_radar3d` 把雷达向屏幕内倾斜（`tilt` 0 到 1，默认 0.9）。

### `ui_sound(name)`

播放一段简短的合成界面音效：`"click"`（也是未知名称的回退）、`"hover"`、`"confirm"`、`"toggle"`、`"tick"` 或 `"error"`。
音频未运行时没有声音。

```ling-gui
令 start = 执 {
    若 按钮(20, 400, 120, 36) == 1 { 界面音("confirm") }
}
```
