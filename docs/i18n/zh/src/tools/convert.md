# 资源转换

`ling convert` 把外部资源（3D 模型、声音、MIDI 乐曲或 SVG 图形）转换成一个独立的 `.ling` 文件。
在程序里 `载` 入这个文件，它就会用 Ling 自己的内置函数重建资源。运行时不需要加载器，也不需要原文件。

```sh
ling convert model.glb                  # 写出 model.ling（压缩、无损）
ling convert sfx.wav -o sfx.ling        # 指定输出路径
ling convert logo.svg --no-compression  # 可阅读、可比较差异的普通数组
lingfu convert song.mid                 # lingfu 会转交给 ling convert
```

| 选项 | 含义 |
|---|---|
| `-o <out.ling>`（或 `--out`） | 输出路径。默认是输入文件名加 `.ling`。 |
| `--no-compression` | 把数字写成普通列表，而不是压缩数据块。 |

## 各格式转换后的内容

| 输入 | 生成的绑定 | 安装配置 |
|---|---|---|
| `.gltf`, `.glb` | 每个网格：`<name>_<mesh>_pos`、`_nrm`、`_uv`、`_idx`，以及线框绘制函数 `draw_<name>_<mesh>(ox, oy, oz, scale)`。`draw_<name>(ox, oy, oz, scale)` 绘制整个模型。节点名和变换以注释形式保留。 | `game` |
| `.wav`, `.ogg`, `.flac`, `.mp3` | `<name>_pcm`（单声道采样）、`<name>_rate`、`<name>_dur` | `audio` |
| `.mid`, `.midi` | `<name>_notes`（每个音符一组 `[time, dur, midi, vel, channel]` 的扁平列表）和 `<name>_dur` | `audio` |
| `.svg` | `<name>_xy`（点）、`<name>_lens`（每条折线的点数）和 `draw_<name>(ox, oy, scale)` | 任意 |
| `.blend` | 先由 Blender 导出为 glTF，再像 `.glb` 一样转换。需要 Blender 在 `PATH` 中或设置了 `$BLENDER`。 | `game` |

`<name>` 是输入文件名，经过整理后成为合法的标识符。对于 SVG，`<path>` 数据（`M`、`L`、`H`、
`V`、`C`、`Q`、`Z`）以及 `<line>`、`<rect>`、`<polyline>`、`<polygon>` 元素会被转换成折线。
其他扩展名会以 `unsupported extension` 失败。

## 压缩还是不压缩

默认情况下，长的数字列表会无损地保存为经过 zlib 压缩和 base64 编码的文本，由两个内置函数解码：

- `blob_f32("...")` 解码成数字列表（小端 32 位浮点数）。
- `blob_i32("...")` 解码成整数列表（32 位整数），用于索引和长度。

`--no-compression` 把同样的数字写成字面量列表 `[ ... ]`。文件更大，但容易阅读。
不超过八个数字的列表总是以普通形式写出。

## 示例：一个 SVG

假设有一个小小的 `logo.svg`，里面有一个 `<rect>` 和一个 `<path>`：

```text
$ ling convert logo.svg
[convert] logo.svg → logo.ling  (1 KB, deflate+base64 lossless)
```

生成的 `logo.ling` 开头是这样的：

```ling-module
# 2 polylines, 7 points
bind logo_xy = blob_f32("eNpdibENAAAIgzjFV/RmDzTRwaYDAwCRLF2Huvf/A/L0CM0=")
bind logo_lens = [2, 5]
```

后面跟着一个 `draw_logo(ox, oy, scale)` 函数。在程序中这样使用：

```ling-gui
载 "logo.ling"

令 启 = 执 {
    开窗(400, 400, "标志")
    循 窗开() {
        填(0, 0, 0)
        设色(255, 255, 255)
        draw_logo(100, 100, 2.0)
        显()
    }
}
```

## 说明

- 音频以源采样率下的单声道混音保存。生成的文件只保存采样数据，播放或分析由你的程序负责。
- 生成的代码里，函数用的是泰文关键字（`ฟังก์ชัน`），生成的绑定用的是 `bind`。这很正常：
  每种语言在任何文件里都能用。
- 模型的绘制函数用 `draw_line_3d` 画线框。如果需要实心、带纹理的网格，参见
  [3D 形状与网格](../reference/shapes.md)。
