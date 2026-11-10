# 声学物体

`body_*` 内置函数提供 `ling-body` 的功能，这是一个用声音“生长”出人形皮肤的工具包。身体支架（参数化的 T 姿势人形，或导入的模型）被体素化为有符号距离场。放在身体上的声源激发身体内外的压力波。一层薄薄的弹性膜响应这股波动，它的表面被提取成一个水密的三角形网格。

整套配方可以通过 WAV 文件往返：导出的 WAV 既能当声音播放，又携带参数和网格，所以能重新加载成同一个身体。任何其他音频文件也可以被分析成声源参数。

需要了解：

- 每个程序只有一个工具包；所有 `body_*` 调用都作用于它。
- 可能失败的操作成功时返回 `1`，失败时返回 `0`，并留下一条可以用 `body_message()` 读取的消息。
- 值以数字、字符串和扁平列表的形式传递。
- 单位是米。网格 Y 轴向上，人形面朝 +Z，左侧是 +X。
- 除了 `body_draw` 和 `body_draw_creases`，其他函数都不需要窗口。
- 这些内置函数没有本地化名称。

## 第一个身体

设置几个参数，添加一个声源，分段运行模拟，然后读取并导出网格：

```ling
令 始 = 执 {
    body_param_set("sr", 8000)
    body_param_set("steps", 60)
    body_param_set("seal", 1)
    body_src_add(0.0, 1.0, 0.0, 0, 220)
    打印("步数：" + str(body_sim_begin()))

    令 当前进度 = [0, 1]
    循 当前进度[0] < 当前进度[1] {
        令 当前进度 = body_sim_step(40)
    }
    打印("完成：" + str(body_sim_done()))
    打印("网格：" + str(body_mesh_update()) + " " + body_message())
    令 统计 = body_mesh_counts()
    打印("水密：" + str(统计[2]))
    打印(body_grid_info())
    打印("导出：" + str(body_export_mesh("body.obj")) + " " + body_message())
}
```

```output
步数：60
完成：1
网格：1 mesh updated
水密：1
109x107x75 @ 41.3 mm
导出：1 10008 triangles → body.obj
```

`body_message()` 返回的消息来自运行时，总是英文。

## 一览

| 内置函数 | 用途 |
|---|---|
| `body_message()` | 上一次操作的消息 |
| `body_grid_info()` | 网格尺寸和间距，例如 `"109x107x75 @ 41.3 mm"` |
| `body_notes()` | 上一次聆听或 WAV 分析的说明 |
| `body_param_get(name)` / `body_param_set(name, v)` | 读取或设置参数 |
| `body_param_reset()` | 恢复所有参数并移除所有声源 |
| `body_param_count()`、`body_param_name(i)`、`body_param_group(i)` | 列出参数 |
| `body_param_min(i)`、`body_param_max(i)`、`body_param_default(i)` | 参数的范围和默认值 |
| `body_src_add(x, y, z, wave, freq)` | 在某个位置添加声源 |
| `body_src_at_joint(joint, wave, freq)` | 在支架关节处添加声源 |
| `body_src_count()`、`body_src_name(i)`、`body_src_del(i)`、`body_src_clear()` | 管理声源 |
| `body_src_get(i, field)` / `body_src_set(i, field, v)` | 读取或修改声源字段 |
| `body_joint_count()`、`body_joint_name(i)`、`body_joint_pos(i)`、`body_joint_parent(i)` | 支架的关节 |
| `body_model_load(path)`、`body_model_use(h)`、`body_model_active()` | 用导入的模型作为身体 |
| `body_model_part_count(h)`、`body_model_part_name(h, i)`、`body_model_part_tris(h, i)` | 查看模型部件 |
| `body_model_part_enabled(h, i)`、`body_model_part_set(h, i, on)` | 包含或排除某个部件 |
| `body_preview()` | 体素化并显示静止形状，不进行模拟 |
| `body_sim_begin()`、`body_sim_step(ms)` | 开始并推进模拟 |
| `body_sim_active()`、`body_sim_done()` | 模拟状态 |
| `body_mesh_update()` | 把膜提取为当前网格 |
| `body_displacement()`、`body_slice(axis, frac)`、`body_mic_tail(n)` | 查看模拟 |
| `body_mesh_counts()`、`body_mesh_bounds()` | 网格统计 |
| `body_mesh_positions()`、`body_mesh_normals()`、`body_mesh_indices()` | 原始网格数据 |
| `body_crease_detect()`、`body_crease_apply()`、`body_detail_reset()` | 根据声场雕刻褶皱 |
| `body_crease_count()`、`body_crease_segments()`、`body_crease_clear()`、`body_crease_export(path)` | 处理褶皱曲线 |
| `body_listen_begin()`、`body_listen_step(ms)`、`body_listen_done()`、`body_listen_finish(path)` | 录制身体的声学特征 |
| `body_listen_spectrum()` | 声学特征的 48 个对数间隔频段 |
| `body_export_mesh(path)`、`body_export_wav(path)`、`body_import_wav(path, analyze_only)` | 文件 |
| `body_project_save(path)`、`body_project_load(path)` | 以 JSON 保存和加载配方 |
| `body_draw(mode, yaw, x, y, z, scale, extra)`、`body_draw_creases(yaw, x, y, z, scale)` | 绘制到窗口 |

## 参数

参数是一张固定的表，包含 47 个命名数值。每个参数都有分组、默认值和范围，设置时会被限制在范围内。`body_param_set` 对已知名称返回 `1`，否则返回 `0`。列举参数的内置函数接受 0 到 46 的索引或名称。

| 分组 | 名称（默认值） | 含义 |
|---|---|---|
| body | `height`（1.7）、`build`（1）、`arm_span`（1）、`head_scale`（1） | 支架比例；`height` 也会缩放导入的模型 |
| body | `seal`（3） | 用于封闭导入模型开口的体素半径 |
| sim | `sr`（22050）、`steps`（800）、`margin`（16）、`up`（2） | 采样率、时间步数、空气边距（格数）、膜网格细分 |
| medium | `c_air`（343）、`c_body`（1）、`rho_body`（4）、`damping`（0.002）、`sponge`（8） | 声速、身体的声速比和密度比、损耗、吸收层 |
| mic | `mic_sep`（0.18）、`mic_dist`（0.15） | 双耳间距和到脸前方的距离 |
| membrane | `relief`、`coupling`、`threshold`、`tension`、`stiffness`、`mem_time`、`mem_every`、`reinit_every`、`band`、`drive_mode`、`drive_tau` | 皮肤如何跟随波动 |
| mesh | `smooth`（3）、`decimate`（0）、`iso`（0） | 平滑次数、顶点合并单元尺寸（米）、表面等值偏移 |
| audio | `wav_bits`（16）、`wav_gain`（0.9）、`wav_repeat`（4）、`wav_gap`（0.25） | WAV 导出 |
| detail | `crease_mode`、`crease_levels`、`crease_tol`、`crease_min`、`detail_level`、`crease_depth`、`crease_width`、`noise_amp`、`noise_freq` | 褶皱检测与雕刻 |
| listen | `sig_dur`、`sig_sr`、`sig_rho`、`sig_excite` | 聆听过程 |

```ling
令 始 = 执 {
    打印(body_param_count())
    打印(body_param_get("height"))
    打印(body_param_set("height", 5))
    打印(body_param_get("height"))
    打印(body_param_set("不存在", 1))
    打印(body_joint_count())
    打印(body_joint_name(4))
}
```

```output
47
1.7
1
2.4
0
21
head
```

`height` 最大只到 2.4，所以设置 5 时存储的是 2.4。

## 声源与关节

声源是空间中的一个激励。它的字段有 `x`、`y`、`z`（米）、`wave`（`0` Ricker 小波、`1` 正弦脉冲、`2` 冲激）、`freq`（Hz）、`amp`、`phase`、`start` 和 `dur`（秒）。

- `body_src_add(x, y, z, wave, freq)` 添加一个声源（默认 0, 1, 0，波形 0，220 Hz），并返回其索引。
- `body_src_at_joint(joint, wave, freq)` 在指定名称的关节处添加声源，并以该关节命名。关节未知时返回 `-1`。

支架有 21 个关节：`hips`、`spine`、`chest`、`neck`、`head`，以及 `shoulder`、`elbow`、`wrist`、`hand`、`hip`、`knee`、`ankle`、`toe` 的 `l_` / `r_` 版本。`body_joint_pos(i)` 返回按当前身体尺寸拟合的 `[x, y, z]`，`body_joint_parent(i)` 返回父关节索引（根关节为 `-1`）。

## 导入的模型

`body_model_load(path)` 加载 `.glb`、`.gltf`、`.lmesh` 或 `.obj` 文件，并返回句柄（或 `-1`）。加载并不会选中它：要调用 `body_model_use(h)`，或者传入负数句柄回到支架。glTF 文件只读取几何体和节点变换，所以需要纹理扩展的文件也能加载。花朵、方块之类的装饰部件一开始是禁用的；用 `body_model_part_set(h, i, on)` 切换部件。

## 模拟

模拟按时间片分段运行，这样窗口可以持续更新。

- `body_preview()` 体素化身体并提取其未受扰动的形状，不进行模拟。
- `body_sim_begin()` 开始模拟，返回总步数，或 `-1`。
- `body_sim_step(ms)` 工作约 `ms` 毫秒（默认 30），返回 `[已完成步数, 总步数]`。
- `body_mesh_update()` 提取膜当前的样子。你可以在模拟运行时调用它，观察皮肤生长。
- `body_displacement()` 返回 `[向内毫米数, 向外毫米数]`。`body_slice(axis, frac)` 返回 `[w, h, v0, v1, ...]`，即归一化到 -1..1 的压力场切片。`body_mic_tail(n)` 返回第一个麦克风最近的 `n` 个采样。

## 褶皱与细节

模拟完成后，`body_crease_detect()` 在网格上采样声场，追踪其等值线，并用三次贝塞尔曲线拟合。`body_crease_apply()` 细分网格，沿每条曲线雕出一道柔和的沟槽，再加上按声场加权的细微噪声。`body_detail_reset()` 恢复平滑网格。`body_crease_export(path)` 把曲线写成 `.json`，或者写成一个生成曲线对象的 Blender `.py` 脚本。

## 聆听

`body_listen_begin()` 在嘴前方发出一个冲激，并用头部两侧的两个麦克风录音。反复调用 `body_listen_step(ms)` 直到完成（它返回 `[已完成, 总数]`），然后 `body_listen_finish(path)` 写出身体声音的 WAV，并填充 `body_listen_spectrum()` 和 `body_notes()`（最强的共振）。

## 文件

- `body_export_mesh(path)` 按扩展名写出 OBJ、STL 或 GLB。GLB 带有 T 姿势骨骼和蒙皮权重。
- `body_export_wav(path)` 写出立体声麦克风信号，附带两个额外的数据块，保存完整配方和压缩后的网格。
- `body_import_wav(path, analyze_only)` 在数据块存在时恢复两者。当 `analyze_only` 为 1 或缺少数据块时，它会改为从音频中估计声源，`body_notes()` 会解释每项估计。接受 WAV、FLAC、Ogg 和 MP3。
- `body_project_save(path)` 和 `body_project_load(path)` 以 JSON 保存参数、声源和所选身体。

## 绘制

`body_draw(mode, yaw, x, y, z, scale, extra)` 用画笔颜色绘制当前网格，并翻转到渲染器 y 轴向下的空间。模式：

| 模式 | 外观 |
|---|---|
| 0 | 实体，平滑法线 |
| 1 | 每隔 `extra` 个三角形画一个的线框 |
| 2 | 按皮肤位移着色；`extra` 是满量程推移的毫米数（默认 10） |
| 3 | 按表面法线着色 |
| 4 | 单光源黏土着色 |

模式 3 和 4 便于观察细微的起伏。`body_draw_creases(yaw, x, y, z, scale)` 在上面绘制检测到的褶皱曲线。

这个窗口让你看着身体生长出来：

```ling-gui
令 始 = 执 {
    开窗(640, 480, "身体")
    body_param_set("steps", 400)
    body_src_at_joint("chest", 1, 180)
    body_sim_begin()
    令 偏航 = 0.0
    循 窗开() {
        若 body_sim_done() == 0 {
            body_sim_step(20)
            body_mesh_update()
        }
        填(16, 16, 24)
        设色(220, 200, 180)
        body_draw(2, 偏航, 0, 1, 4, 1, 10)
        flush_3d()
        显()
        令 偏航 = 偏航 + 0.01
    }
}
```

另见：[物理](physics.md)、[音乐](music.md)。
