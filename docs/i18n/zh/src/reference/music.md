# 音乐

`music_*` 内置函数处理整首音乐。它们解码和分析音频文件，一次播放一首曲目，用乐器音色演奏音符，在音符和频率之间换算，读取 MIDI 和 `.lrc` 歌词文件，并为节奏游戏判定击打。`daw_*` 内置函数更进一步：它们驱动数字音频工作站 LingStationX 的引擎，让脚本可以编排一首歌、添加效果并渲染成文件。

音调、音效和采样文件请见[音频](audio.md)。

约定：

- 曲目、音色、MIDI 和歌词的句柄都是从 0 开始计数的小整数。加载失败返回 `-1`。未知句柄返回 `0`、空字符串或空列表，从不报错。
- 文件存在时路径原样使用，否则相对于正在运行的脚本所在文件夹。
- **音高**可以是 MIDI 编号（60 是中央 C，69 是 A4），也可以是 `"C4"`、`"A#3"`、`"Db5"` 这样的音名。
- 分析、MIDI、歌词和离线 DAW 工作不需要音频设备，所以这些示例在任何地方都能运行。

## 一览

| 内置函数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `music_load(path)` | `载入音乐` | `音楽読込` | `음악로드` | `โหลดเพลง` | 解码音频文件 |
| `music_duration(track)` | `音乐时长` | `音楽長さ` | `음악길이` | `ความยาวเพลง` | 长度（秒） |
| `music_bpm(track)` | `节拍速度` | `テンポ` | `템포` | `จังหวะต่อนาที` | 估计速度 |
| `music_key(track)` | `调性` | `調性` | `조성` | `คีย์เพลง` | 估计调性 |
| `music_onsets(track)` | `音符起点` | `オンセット` | `온셋` | `จุดเริ่มเสียง` | 击打和音符起始的时间 |
| `music_beat_grid(track)` | `节拍网格` | `ビートグリッド` | `비트그리드` | `กริดจังหวะ` | 等间隔的节拍时间 |
| `music_fft(track, bands)` | `音乐频谱` | `音楽スペクトル` | `음악스펙트럼` | `สเปกตรัมเพลง` | 播放位置处的频谱 |
| `music_fft_groups(track, size, groups)` | — | — | — | — | 一次大型 FFT 得到的响度分组 |
| `music_play(track)` | `播放音乐` | `音楽再生` | `음악재생` | `เล่นเพลง` | 从头播放曲目 |
| `music_pause()` | `暂停音乐` | `音楽一時停止` | `음악일시정지` | `หยุดเพลงชั่วคราว` | 暂停 |
| `music_stop()` | `停止音乐` | `音楽停止` | `음악정지` | `หยุดเพลง` | 停止并倒回开头 |
| `music_seek(seconds)` | `定位音乐` | `音楽シーク` | `음악탐색` | `ค้นหาเพลง` | 跳到某个位置 |
| `music_pos()` | `音乐位置` | `音楽位置` | `음악위치` | `ตำแหน่งเพลง` | 播放位置 |
| `music_volume(level)` | `音乐音量` | `音楽音量` | `음악음량` | `ระดับเพลง` | 音乐引擎音量 |
| `music_patch(path)` | `乐器音色` | `音色読込` | `악기패치` | `แพตช์เครื่องดนตรี` | 加载乐器音色 |
| `music_note(inst, pitch, duration, velocity)` | `弹音符` | `音符演奏` | `음표연주` | `เล่นโน้ต` | 演奏一个音符 |
| `music_note_on(inst, pitch, velocity)` | `音符开始` | `音符オン` | `음표켜기` | `โน้ตเริ่ม` | 按下并保持一个键 |
| `music_note_off(inst, pitch)` | `音符结束` | `音符オフ` | `음표끄기` | `โน้ตจบ` | 松开一个键 |
| `music_hz(pitch)` | `音符频率` | `音符周波数` | `음표주파수` | `ความถี่โน้ต` | 音高转频率 |
| `music_note_name(hz)` | `音名` | `音名称` | `음이름` | `ชื่อโน้ต` | 频率转音名 |
| `music_pitch_score(sung_hz, target_hz)` | `音准评分` | `音程スコア` | `음정점수` | `คะแนนเสียง` | 给唱出的音高打分 |
| `music_mic_pitch()` | `麦克风音高` | `マイク音程` | `마이크음정` | `ระดับเสียงไมค์` | 麦克风听到的音高 |
| `music_judge(delta_ms)` | `判定` | `判定する` | `판정` | `ตัดสินจังหวะ` | 判定节奏游戏的击打 |
| `music_grade_name(index)` | `判定名` | `判定名称` | `판정이름` | `ชื่อการตัดสิน` | 判定等级的名称 |
| `music_lrc(path)` | `载入歌词` | `歌詞読込` | `가사로드` | `โหลดเนื้อเพลง` | 加载 `.lrc` 歌词 |
| `music_lyric(lyrics, time)` | `当前歌词` | `現在歌詞` | `현재가사` | `เนื้อเพลงปัจจุบัน` | 某个时间的歌词行 |
| `music_midi_load(path)` | `载入MIDI` | `MIDI読込` | `미디로드` | `โหลดมิดี` | 加载 MIDI 文件 |
| `music_midi_count(midi)` | `MIDI数量` | `MIDI数` | `미디수` | `จำนวนมิดี` | 音符数量 |
| `music_midi_notes(midi)` | `MIDI音符` | `MIDIノート` | `미디음표` | `โน้ตมิดี` | 音符起点 `[time, key, ...]` |
| `music_midi_bars(midi)` | `MIDI音条` | `MIDIバー` | `미디바` | `แท่งมิดี` | 带长度的音符 `[time, key, length, ...]` |

`daw_*` 内置函数没有本地化名称。

## 音符与频率

`music_hz(pitch)` 把 MIDI 编号或音名换算成 Hz；A4（69）是 440。无法识别的音名按 69 处理。`music_note_name(hz)` 给出最接近的音符，用升号拼写；`hz` 不是正数时返回空字符串。八度采用科学音高记号，C4 即 MIDI 60。

```ling
令 始 = 执 {
    打印(音符频率("A4"))
    打印(四舍五入(音符频率(60) * 100) / 100)
    打印(四舍五入(音符频率("C5") * 100) / 100)
    打印(音名(440.0))
    打印(音名(262.0))
    打印(音名(466.2))
}
```

```output
440
261.63
523.25
A4
C4
A#4
```

没有音阶内置函数，但音阶不过是一串半音步数的列表。下面打印 C 大调五声音阶：

```ling
令 始 = 执 {
    令 步数 = [0, 2, 4, 7, 9]
    令 结果 = []
    令 i = 0
    循 i < 长度(步数) {
        令 结果 = 结果.添加(音名(音符频率(60 + 步数[i])))
        令 i = i + 1
    }
    打印(结果)
}
```

```output
[C4, D4, E4, G4, A4]
```

## 加载与分析

| 内置函数 | 返回值 |
|---|---|
| `music_load(path)` | 曲目句柄，或 `-1`。接受 WAV、FLAC、OGG（Vorbis）、MP3 和 AAC/M4A。整个文件会被解码到内存中。 |
| `music_duration(track)` | 长度（秒）。 |
| `music_bpm(track)` | 速度估计值，取整到 0.1，并折叠到 70 到 170 BPM 之间。对某些素材可能差一个八度。 |
| `music_key(track)` | `"A minor"` 或 `"C major"` 这样的字符串（只报告大调和小调）。 |
| `music_onsets(track)` | 音符或击打开始的时间（秒）组成的列表。 |
| `music_beat_grid(track)` | 按检测到的速度、从第一个起点开始的等间隔节拍时间。不跟随速度变化。 |
| `music_fft(track, bands)` | `bands` 个对数间隔、平滑后的幅度（默认 16），低音在前，取自当前播放位置的 2048 个采样窗口。`music_play` 开始之前是静音。 |
| `music_fft_groups(track, size, groups)` | 对 `size` 个采样（默认 8192）做一次 FFT，得到 30 Hz 到 16 kHz 之间的 `groups` 个响度值（默认 7）。满幅正弦波约为 0.5；安静的段落保持安静。 |

这个示例不需要任何文件：它先用 DAW（见下文）渲染一段两小节的底鼓循环，然后分析它。

```ling
令 始 = 执 {
    daw_new()
    daw_set_bpm(120)
    令 鼓 = daw_track_add("drums")
    令 片段 = daw_clip_add_midi(鼓, 0, 8)
    令 节拍 = 0
    循 节拍 < 16 {
        daw_note_add(鼓, 片段, 节拍 * 0.5, 0.25, 36, 0.9)
        令 节拍 = 节拍 + 1
    }
    daw_bounce("loop.wav", 0, 8, 0.0, 16, "wav", 0.5)

    令 音轨 = 载入音乐("loop.wav")
    打印("秒数：" + str(四舍五入(音乐时长(音轨))))
    打印("速度：" + str(四舍五入(节拍速度(音轨))))
    打印("起点数：" + str(长度(音符起点(音轨))))
    打印("网格节拍数：" + str(长度(节拍网格(音轨))))
    打印(载入音乐("missing.wav"))
}
```

```output
秒数：4
速度：120
起点数：15
网格节拍数：8
-1
```

## 播放

音乐引擎在第一次使用时打开自己的输出流，与[音频](audio.md)引擎分开，一次播放一首曲目。

- `music_play(track)` 把 `track` 设为当前曲目并**从头**开始播放；再次调用会重新开始。播放到结尾就停止（不循环）。
- `music_pause()` 暂停并保留位置。要继续播放，调用 `music_play(-1)`：传入一个不是已加载曲目的 id，会继续当前曲目。
- `music_stop()` 停止并倒回开头。`music_seek(seconds)` 跳转。`music_pos()` 返回以秒计的位置。
- `music_volume(level)` 设置音乐引擎的音量，曲目和合成器一起（默认 0.8，限制在 0..2）。输出会软削波，所以大于 1 的音量只会柔和地失真。

```ling-gui
令 始 = 执 {
    开窗(640, 360, "可视化")
    令 歌曲 = 载入音乐("song.ogg")
    音乐音量(0.8)
    播放音乐(歌曲)
    循 窗开() {
        填(10, 10, 20)
        令 柱 = 音乐频谱(歌曲, 16)
        令 i = 0
        循 i < 16 {
            令 柱高 = 最小(300, 柱[i] * 40)
            设色(80 + i * 10, 200, 255)
            画线(30 + i * 38, 340, 30 + i * 38, 340 - 柱高)
            令 i = i + 1
        }
        显()
    }
}
```

## 合成

音乐引擎包含一个复音 FM/减法合成器。乐器就是一个**音色**文件：纯文本的 `key=value` 标记，`#` 开始注释。算子从 1 开始编号，一个算子可以用 `opN.fm` 对另一个算子做调频。

```text
name = "Rhodes EP"
poly = 16
op1.wave=sine  op1.ratio=1.0  op1.level=1.0  op1.adsr=0.002,1.2,0.0,0.4
op2.wave=sine  op2.ratio=14.0 op2.level=0.5  op2.fm=op1  op2.adsr=0.001,0.2,0.0,0.2
filter.cutoff=0.8 filter.reso=0.2 filter.env=0.3 filter.adsr=0,0.5,0.2,0.3
lfo.rate=5.0 lfo.pitch=0.05
amp = 0.7
```

可用的键：`name`、`gm`（General MIDI 音色号）、`poly`（声部数，默认 16）、`amp`、`lfo.rate`、`lfo.pitch`、`lfo.cutoff`、`lfo.amp`、`filter.cutoff`、`filter.reso`、`filter.env`、`filter.adsr`、`filter.on`，以及每个算子的 `opN.wave`（`sine`、`saw`、`square`/`pulse`、`tri`、`noise`）、`opN.ratio`、`opN.fixed`（Hz）、`opN.level`、`opN.detune`、`opN.adsr`（起音、衰减、延音、释音）和 `opN.fm`。未知的键会被忽略。仓库的 `instruments/` 文件夹里附带了现成的音色。

- `music_patch(path)` 加载音色并返回乐器句柄（与曲目句柄互不相关），或 `-1`。
- `music_note(inst, pitch, duration, velocity)` 演奏一个保持 `duration` 秒（默认 0.5）、力度为 `velocity`（0 到 1，默认 0.9）的音符；之后是音色的释音。
- `music_note_on(inst, pitch, velocity)` 按下一个键；`music_note_off(inst, pitch)` 松开它。两者要成对使用。

```ling-gui
令 始 = 执 {
    令 钢琴 = 乐器音色("instruments/grand_piano.ling")
    弹音符(钢琴, "C4", 0.5, 0.9)
    弹音符(钢琴, "E4", 0.5, 0.8)
    音符开始(钢琴, 55, 0.7)
    睡眠(1000)
    音符结束(钢琴, 55)
}
```

## 演唱与节奏游戏

`music_mic_pitch()` 返回麦克风听到的基频（55 到 1000 Hz）；麦克风未打开、信号太弱或找不到清晰的音高时返回 0。请先用 `mic_open()` 打开麦克风。

`music_pitch_score(sung_hz, target_hz)` 给唱出的音高打 0 到 1 分：1 表示完全准确，0 表示偏离一个半音或更多。误差会折叠到最近的八度，所以唱高或低一个八度仍能得高分。

`music_judge(delta_ms)` 根据以毫秒计的时间误差判定击打（忽略正负号）：0 Perfect（不超过 30 毫秒）、1 Great（60）、2 Good（100）、3 Ok（140）、4 Miss。`music_grade_name(index)` 返回 `"PERFECT"`、`"GREAT"`、`"GOOD"`、`"OK"` 或 `"MISS"`。

```ling
令 始 = 执 {
    打印(音准评分(440.0, 440.0))
    打印(四舍五入(音准评分(450.0, 440.0) * 100) / 100)
    打印(判定(12.0))
    打印(判定名(判定(-45.0)))
    打印(判定名(判定(500.0)))
}
```

```output
1
0.61
0
GREAT
MISS
```

## 歌词与 MIDI 文件

`music_lrc(path)` 加载一个带时间的歌词文件，其中每行形如 `[mm:ss.xx] 文本`，并返回句柄。`music_lyric(lyrics, time)` 返回时间不晚于 `time`（通常是 `music_pos()`）的最后一行；在第一行之前返回空字符串。

```ling
令 始 = 执 {
    write_file("song.lrc", "[00:01.00] 第一句\n[00:03.50] 第二句\n")
    令 歌词 = 载入歌词("song.lrc")
    打印("[" + 当前歌词(歌词, 0.5) + "]")
    打印(当前歌词(歌词, 2.0))
    打印(当前歌词(歌词, 99.0))
}
```

```output
[]
第一句
第二句
```

`music_midi_load(path)` 把标准 MIDI 文件作为数据加载；不会播放任何声音。你可以用这些音符生成金币、绘制下落的音条或触发提示。`music_midi_count` 返回音符数量，`music_midi_notes` 返回扁平列表 `[时间, 键, 时间, 键, ...]`，`music_midi_bars` 返回扁平列表 `[时间, 键, 长度, ...]`，单位都是秒。

```ling
令 始 = 执 {
    daw_new()
    daw_set_bpm(120)
    令 轨 = daw_track_add("synth")
    令 片段 = daw_clip_add_midi(轨, 0, 4)
    daw_note_add(轨, 片段, 0, 1, 60, 0.8)
    daw_note_add(轨, 片段, 2, 2, 67, 0.8)
    daw_midi_export_project("song.mid")

    令 m = 载入MIDI("song.mid")
    打印(MIDI数量(m))
    打印(MIDI音符(m))
    打印(MIDI音条(m))
}
```

```output
2
[0, 60, 1, 67]
[0, 60, 0.5, 1, 67, 1]
```

## DAW

`daw_*` 内置函数驱动随 Ling 一起提供的数字音频工作站 LingStationX 的引擎。任何脚本都能使用它，有没有窗口都可以：创建工程、编辑音符、添加效果、渲染音频。第一次调用 `daw_*` 内置函数时会创建引擎。

需要了解：

- 片段的位置、长度、循环点和渲染范围都以**拍**为单位。`daw_secs_at(beat)` 可换算为秒。
- 音轨、片段、音符和效果都用从零开始的索引来指定。插入或删除项目后索引会变化。
- 是/否的结果以 `1` 和 `0` 返回。创建东西的内置函数返回其索引，或 `-1`。
- 编辑和渲染不需要音频设备。有设备时，`daw_play()` 会实时播放。
- `daw_status()` 返回上一次操作留下的消息。
- 效果的*目标*是音轨索引，或者用 `-1` 表示主总线。

主要分组：

| 分组 | 内置函数 |
|---|---|
| 工程 | `daw_new`、`daw_project_save(path)`、`daw_project_load(path)`、`daw_set_project_name(name)`、`daw_status` |
| 走带与速度 | `daw_play`、`daw_pause`、`daw_stop`、`daw_seek(beat)`、`daw_position`、`daw_bpm`、`daw_set_bpm(bpm)`、`daw_set_loop(on, start, end)`、`daw_end_beat`、`daw_tempo_add(beat, bpm)`、`daw_secs_at(beat)` |
| 音轨 | `daw_track_add(kind, path)`、`daw_track_count`、`daw_track_remove(track)`、`daw_track_name(track)`、`daw_track_set_name(track, name)`、`daw_track_get(track, field)`、`daw_track_set(track, field, value)` |
| 片段与音符 | `daw_clip_add_midi(track, start, length)`、`daw_clip_add_audio(track, path, start)`、`daw_clip_notes(track, clip)`、`daw_note_add(track, clip, start, length, key, velocity)`、`daw_note_remove(track, clip, note)` |
| 乐器 | `daw_inst_presets(track)`、`daw_inst_preset(track, name)`、`daw_inst_params(track)`、`daw_inst_set(track, name, value)` |
| 效果 | `daw_fx_types`、`daw_fx_add(target, name)`、`daw_fx_params(target, index)`、`daw_fx_set(target, index, param, value)`、`daw_fx_set_wet(target, index, wet)`、`daw_fx_bypass(target, index, on)` |
| 渲染与撤销 | `daw_bounce(path, start, end, tail, bits, format, quality)`、`daw_render_start(...)`、`daw_undo_push`、`daw_undo`、`daw_redo` |
| MIDI | `daw_midi_import(path, beat)`、`daw_midi_export(track, path)`、`daw_midi_export_project(path)`、`daw_midi_inputs`、`daw_midi_open(index)` |

`daw_track_add` 的音轨类型有 `"synth"`、`"drums"`、`"audio"`、`"sf2"`（需提供 SoundFont 路径）、`"growl"`、`"voice"`、`"prism"`、`"sampler"`、`"pluck"`、`"chip"`、`"cinema"` 和 `"script"`。引擎还有数百个用于自动化、人声、歌曲生成、混音和导出的 `daw_*` 内置函数；它们主要服务于 LingStationX 应用本身。

一个完整的离线会话：

```ling
令 始 = 执 {
    daw_new()
    daw_set_project_name("Demo")
    daw_set_bpm(100)

    令 主旋律 = daw_track_add("synth")
    daw_inst_preset(主旋律, "FM Bell")
    令 片段 = daw_clip_add_midi(主旋律, 0, 4)
    令 音高 = [60, 64, 67, 72]
    令 i = 0
    循 i < 4 {
        daw_note_add(主旋律, 片段, i, 0.75, 音高[i], 0.8)
        令 i = i + 1
    }

    令 混响器 = daw_fx_add(主旋律, "Reverb")
    daw_fx_set_wet(主旋律, 混响器, 0.3)

    打印("音轨数：" + str(daw_track_count()))
    打印("工程结束于第 " + str(daw_end_beat()) + " 拍")
    打印("渲染成功：" + str(daw_bounce("demo.wav", 0, 4, 1.0, 16, "wav", 0.5)))
}
```

```output
音轨数：1
工程结束于第 4 拍
渲染成功：1
```
