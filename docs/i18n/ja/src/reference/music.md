# 音楽

`music_*` 組み込み関数は、曲全体を扱います。音声ファイルのデコードと解析、1 曲ずつの再生、楽器パッチでの音符の演奏、音符と周波数の変換、MIDI と `.lrc` 歌詞ファイルの読み込み、リズムゲームの判定ができます。`daw_*` 組み込み関数はさらに進んで、デジタルオーディオワークステーション LingStationX のエンジンを動かします。スクリプトから曲を組み立て、エフェクトをかけ、ファイルに書き出せます。

トーン、効果音、サンプルファイルについては[オーディオ](audio.md)を見てください。

約束事：

- トラック、パッチ、MIDI、歌詞のハンドルは 0 から数える小さな整数です。読み込みに失敗すると `-1` を返します。知らないハンドルを渡すと `0`、空文字列、空リストのいずれかが返り、エラーにはなりません。
- パスは、ファイルが存在すればそのまま、なければ実行中のスクリプトのフォルダーからの相対パスとして使われます。
- **音高**は MIDI 番号（60 が中央のド、69 が A4）か、`"C4"`、`"A#3"`、`"Db5"` のような音名です。
- 解析、MIDI、歌詞、オフラインの DAW 作業にはオーディオデバイスが要らないので、それらの例はどこでも動きます。

## 一覧

| 組み込み関数 | 中文 | 日本語 | 한국어 | ไทย | 用途 |
|---|---|---|---|---|---|
| `music_load(path)` | `载入音乐` | `音楽読込` | `음악로드` | `โหลดเพลง` | 音声ファイルをデコード |
| `music_duration(track)` | `音乐时长` | `音楽長さ` | `음악길이` | `ความยาวเพลง` | 長さ（秒） |
| `music_bpm(track)` | `节拍速度` | `テンポ` | `템포` | `จังหวะต่อนาที` | テンポの推定 |
| `music_key(track)` | `调性` | `調性` | `조성` | `คีย์เพลง` | 調の推定 |
| `music_onsets(track)` | `音符起点` | `オンセット` | `온셋` | `จุดเริ่มเสียง` | 打音や音の始まりの時刻 |
| `music_beat_grid(track)` | `节拍网格` | `ビートグリッド` | `비트그리드` | `กริดจังหวะ` | 等間隔のビート時刻 |
| `music_fft(track, bands)` | `音乐频谱` | `音楽スペクトル` | `음악스펙트럼` | `สเปกตรัมเพลง` | 再生位置のスペクトル |
| `music_fft_groups(track, size, groups)` | — | — | — | — | 1 回の大きな FFT からの音量グループ |
| `music_play(track)` | `播放音乐` | `音楽再生` | `음악재생` | `เล่นเพลง` | 曲を最初から再生 |
| `music_pause()` | `暂停音乐` | `音楽一時停止` | `음악일시정지` | `หยุดเพลงชั่วคราว` | 一時停止 |
| `music_stop()` | `停止音乐` | `音楽停止` | `음악정지` | `หยุดเพลง` | 停止して巻き戻す |
| `music_seek(seconds)` | `定位音乐` | `音楽シーク` | `음악탐색` | `ค้นหาเพลง` | 位置へ移動 |
| `music_pos()` | `音乐位置` | `音楽位置` | `음악위치` | `ตำแหน่งเพลง` | 再生位置 |
| `music_volume(level)` | `音乐音量` | `音楽音量` | `음악음량` | `ระดับเพลง` | 音楽エンジンの音量 |
| `music_patch(path)` | `乐器音色` | `音色読込` | `악기패치` | `แพตช์เครื่องดนตรี` | 楽器パッチを読み込む |
| `music_note(inst, pitch, duration, velocity)` | `弹音符` | `音符演奏` | `음표연주` | `เล่นโน้ต` | 音符を演奏 |
| `music_note_on(inst, pitch, velocity)` | `音符开始` | `音符オン` | `음표켜기` | `โน้ตเริ่ม` | 鍵盤を押して保持 |
| `music_note_off(inst, pitch)` | `音符结束` | `音符オフ` | `음표끄기` | `โน้ตจบ` | 鍵盤を離す |
| `music_hz(pitch)` | `音符频率` | `音符周波数` | `음표주파수` | `ความถี่โน้ต` | 音高から周波数へ |
| `music_note_name(hz)` | `音名` | `音名称` | `음이름` | `ชื่อโน้ต` | 周波数から音名へ |
| `music_pitch_score(sung_hz, target_hz)` | `音准评分` | `音程スコア` | `음정점수` | `คะแนนเสียง` | 歌った音程を採点 |
| `music_mic_pitch()` | `麦克风音高` | `マイク音程` | `마이크음정` | `ระดับเสียงไมค์` | マイクで聞こえる音程 |
| `music_judge(delta_ms)` | `判定` | `判定する` | `판정` | `ตัดสินจังหวะ` | リズムゲームのヒットを判定 |
| `music_grade_name(index)` | `判定名` | `判定名称` | `판정이름` | `ชื่อการตัดสิน` | 判定の名前 |
| `music_lrc(path)` | `载入歌词` | `歌詞読込` | `가사로드` | `โหลดเนื้อเพลง` | `.lrc` 歌詞を読み込む |
| `music_lyric(lyrics, time)` | `当前歌词` | `現在歌詞` | `현재가사` | `เนื้อเพลงปัจจุบัน` | ある時刻の歌詞の行 |
| `music_midi_load(path)` | `载入MIDI` | `MIDI読込` | `미디로드` | `โหลดมิดี` | MIDI ファイルを読み込む |
| `music_midi_count(midi)` | `MIDI数量` | `MIDI数` | `미디수` | `จำนวนมิดี` | 音符の数 |
| `music_midi_notes(midi)` | `MIDI音符` | `MIDIノート` | `미디음표` | `โน้ตมิดี` | 音符の始まり `[time, key, ...]` |
| `music_midi_bars(midi)` | `MIDI音条` | `MIDIバー` | `미디바` | `แท่งมิดี` | 長さ付きの音符 `[time, key, length, ...]` |

`daw_*` 組み込み関数にはローカライズ名がありません。

## 音符と周波数

`music_hz(pitch)` は MIDI 番号か音名を Hz に変換します。A4（69）が 440 です。読めない音名は 69 として扱われます。`music_note_name(hz)` はシャープ表記で最も近い音名を返し、`hz` が正でなければ空文字列を返します。オクターブは C4 が MIDI 60 となる国際式の表記に従います。

```ling
束縛 開始 = 実行 {
    印刷(音符周波数("A4"))
    印刷(四捨五入(音符周波数(60) * 100) / 100)
    印刷(四捨五入(音符周波数("C5") * 100) / 100)
    印刷(音名称(440.0))
    印刷(音名称(262.0))
    印刷(音名称(466.2))
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

音階の組み込み関数はありませんが、音階は半音の歩幅のリストにすぎません。これは C のメジャーペンタトニックを表示します。

```ling
束縛 開始 = 実行 {
    束縛 歩幅 = [0, 2, 4, 7, 9]
    束縛 結果 = []
    束縛 番号 = 0
    間 番号 < 長さ(歩幅) {
        束縛 結果 = 結果.追加(音名称(音符周波数(60 + 歩幅[番号])))
        束縛 番号 = 番号 + 1
    }
    印刷(結果)
}
```

```output
[C4, D4, E4, G4, A4]
```

## 読み込みと解析

| 組み込み関数 | 戻り値 |
|---|---|
| `music_load(path)` | トラックのハンドル、または `-1`。WAV、FLAC、OGG（Vorbis）、MP3、AAC/M4A を受け付けます。ファイル全体をメモリにデコードします。 |
| `music_duration(track)` | 長さ（秒）。 |
| `music_bpm(track)` | テンポの推定値。0.1 単位に丸め、70〜170 BPM に折り込みます。素材によってはオクターブずれることがあります。 |
| `music_key(track)` | `"A minor"` や `"C major"` のような文字列（長調と短調のみ）。 |
| `music_onsets(track)` | 音や打音が始まる時刻（秒）のリスト。 |
| `music_beat_grid(track)` | 検出したテンポでの等間隔のビート時刻。最初のオンセットから始まります。テンポの変化には追従しません。 |
| `music_fft(track, bands)` | 現在の再生位置から 2048 サンプルの窓で求めた、対数間隔で平滑化した振幅 `bands` 個（既定値 16）。低音が先頭です。`music_play` が始まるまでは無音です。 |
| `music_fft_groups(track, size, groups)` | `size` サンプル（既定値 8192）の FFT 1 回から求めた、30 Hz〜16 kHz の音量値 `groups` 個（既定値 7）。フルスケールのサイン波で約 0.5 になり、静かな部分は静かなままです。 |

この例はファイルを必要としません。DAW（下を参照）で 2 小節のキックドラムのループを書き出し、それを解析します。

```ling
束縛 開始 = 実行 {
    daw_new()
    daw_set_bpm(120)
    束縛 ドラム = daw_track_add("drums")
    束縛 クリップ = daw_clip_add_midi(ドラム, 0, 8)
    束縛 ビート = 0
    間 ビート < 16 {
        daw_note_add(ドラム, クリップ, ビート * 0.5, 0.25, 36, 0.9)
        束縛 ビート = ビート + 1
    }
    daw_bounce("loop.wav", 0, 8, 0.0, 16, "wav", 0.5)

    束縛 トラック = 音楽読込("loop.wav")
    印刷("秒数: " + str(四捨五入(音楽長さ(トラック))))
    印刷("BPM: " + str(四捨五入(テンポ(トラック))))
    印刷("オンセット数: " + str(長さ(オンセット(トラック))))
    印刷("グリッドのビート数: " + str(長さ(ビートグリッド(トラック))))
    印刷(音楽読込("missing.wav"))
}
```

```output
秒数: 4
BPM: 120
オンセット数: 15
グリッドのビート数: 8
-1
```

## 再生

音楽エンジンは、初めて使うときに[オーディオ](audio.md)エンジンとは別の出力ストリームを開き、一度に 1 曲を再生します。

- `music_play(track)` は `track` を現在の曲にして**最初から**再生します。もう一度呼ぶと最初からやり直します。曲の終わりで止まります（ループしません）。
- `music_pause()` は位置を保ったまま一時停止します。再開するには `music_play(-1)` を呼びます。読み込まれていない ID を渡すと、現在の曲の続きを再生します。
- `music_stop()` は停止して巻き戻します。`music_seek(seconds)` は移動し、`music_pos()` は位置を秒で返します。
- `music_volume(level)` は音楽エンジン（曲とシンセサイザーの両方）の音量を設定します（既定値 0.8、0..2 にクランプ）。出力はソフトクリップされるので、1 を超えると穏やかにひずみます。

```ling-gui
束縛 開始 = 実行 {
    ウィンドウ開く(640, 360, "ビジュアライザー")
    束縛 曲 = 音楽読込("song.ogg")
    音楽音量(0.8)
    音楽再生(曲)
    間 開いている() {
        塗り潰し(10, 10, 20)
        束縛 帯 = 音楽スペクトル(曲, 16)
        束縛 番号 = 0
        間 番号 < 16 {
            束縛 高さ = 最小(300, 帯[番号] * 40)
            色設定(80 + 番号 * 10, 200, 255)
            線描く(30 + 番号 * 38, 340, 30 + 番号 * 38, 340 - 高さ)
            束縛 番号 = 番号 + 1
        }
        表示()
    }
}
```

## シンセサイザー

音楽エンジンには、ポリフォニックな FM/減算方式のシンセサイザーが入っています。楽器は**パッチ**ファイルです。`key=value` の書き並べで、`#` から後ろはコメントです。オペレーターは 1 から番号を振り、`opN.fm` で別のオペレーターを周波数変調できます。

```text
name = "Rhodes EP"
poly = 16
op1.wave=sine  op1.ratio=1.0  op1.level=1.0  op1.adsr=0.002,1.2,0.0,0.4
op2.wave=sine  op2.ratio=14.0 op2.level=0.5  op2.fm=op1  op2.adsr=0.001,0.2,0.0,0.2
filter.cutoff=0.8 filter.reso=0.2 filter.env=0.3 filter.adsr=0,0.5,0.2,0.3
lfo.rate=5.0 lfo.pitch=0.05
amp = 0.7
```

キー：`name`、`gm`（General MIDI のプログラム番号）、`poly`（ボイス数、既定値 16）、`amp`、`lfo.rate`、`lfo.pitch`、`lfo.cutoff`、`lfo.amp`、`filter.cutoff`、`filter.reso`、`filter.env`、`filter.adsr`、`filter.on`、そしてオペレーターごとの `opN.wave`（`sine`、`saw`、`square`/`pulse`、`tri`、`noise`）、`opN.ratio`、`opN.fixed`（Hz）、`opN.level`、`opN.detune`、`opN.adsr`（アタック、ディケイ、サステイン、リリース）、`opN.fm`。知らないキーは無視されます。リポジトリの `instruments/` フォルダーに、すぐ使えるパッチが入っています。

- `music_patch(path)` はパッチを読み込み、楽器のハンドル（トラックのハンドルとは別）か `-1` を返します。
- `music_note(inst, pitch, duration, velocity)` は、`duration` 秒（既定値 0.5）押さえた音を、ベロシティ 0〜1（既定値 0.9）で鳴らします。その後にパッチのリリースが続きます。
- `music_note_on(inst, pitch, velocity)` は鍵盤を押し、`music_note_off(inst, pitch)` は離します。必ず対にしてください。

```ling-gui
束縛 開始 = 実行 {
    束縛 ピアノ = 音色読込("instruments/grand_piano.ling")
    音符演奏(ピアノ, "C4", 0.5, 0.9)
    音符演奏(ピアノ, "E4", 0.5, 0.8)
    音符オン(ピアノ, 55, 0.7)
    スリープ(1000)
    音符オフ(ピアノ, 55)
}
```

## 歌とリズムゲーム

`music_mic_pitch()` はマイクで聞こえる基本周波数（55〜1000 Hz）を返します。マイクが閉じている、信号が小さすぎる、はっきりした音程が見つからない、のいずれかなら 0 です。先に `mic_open()` でマイクを開いてください。

`music_pitch_score(sung_hz, target_hz)` は歌った音程を 0〜1 で採点します。1 はぴったり、0 は半音以上ずれています。誤差は最も近いオクターブに折り込まれるので、1 オクターブ違いで歌っても高得点になります。

`music_judge(delta_ms)` はタイミングの誤差（ミリ秒、符号は無視）でヒットを判定します。0 Perfect（30 ms まで）、1 Great（60）、2 Good（100）、3 Ok（140）、4 Miss です。`music_grade_name(index)` は `"PERFECT"`、`"GREAT"`、`"GOOD"`、`"OK"`、`"MISS"` のいずれかを返します。

```ling
束縛 開始 = 実行 {
    印刷(音程スコア(440.0, 440.0))
    印刷(四捨五入(音程スコア(450.0, 440.0) * 100) / 100)
    印刷(判定する(12.0))
    印刷(判定名称(判定する(-45.0)))
    印刷(判定名称(判定する(500.0)))
}
```

```output
1
0.61
0
GREAT
MISS
```

## 歌詞と MIDI ファイル

`music_lrc(path)` は、各行が `[mm:ss.xx] テキスト` の形の時刻付き歌詞ファイルを読み込み、ハンドルを返します。`music_lyric(lyrics, time)` は、時刻が `time`（ふつうは `music_pos()`）以前である最後の行を返し、最初の行より前なら空文字列を返します。

```ling
束縛 開始 = 実行 {
    write_file("song.lrc", "[00:01.00] 一行目\n[00:03.50] 二行目\n")
    束縛 歌詞 = 歌詞読込("song.lrc")
    印刷("[" + 現在歌詞(歌詞, 0.5) + "]")
    印刷(現在歌詞(歌詞, 2.0))
    印刷(現在歌詞(歌詞, 99.0))
}
```

```output
[]
一行目
二行目
```

`music_midi_load(path)` は Standard MIDI File をデータとして読み込みます。何も鳴りません。音符を使ってコインを出したり、落ちてくるバーを描いたり、合図を出したりします。`music_midi_count` は音符の数、`music_midi_notes` は平たいリスト `[時刻, キー, 時刻, キー, ...]`、`music_midi_bars` は平たいリスト `[時刻, キー, 長さ, ...]` を返します。単位はすべて秒です。

```ling
束縛 開始 = 実行 {
    daw_new()
    daw_set_bpm(120)
    束縛 トラック = daw_track_add("synth")
    束縛 クリップ = daw_clip_add_midi(トラック, 0, 4)
    daw_note_add(トラック, クリップ, 0, 1, 60, 0.8)
    daw_note_add(トラック, クリップ, 2, 2, 67, 0.8)
    daw_midi_export_project("song.mid")

    束縛 ミディ = MIDI読込("song.mid")
    印刷(MIDI数(ミディ))
    印刷(MIDIノート(ミディ))
    印刷(MIDIバー(ミディ))
}
```

```output
2
[0, 60, 1, 67]
[0, 60, 0.5, 1, 67, 1]
```

## DAW

`daw_*` 組み込み関数は、Ling に付属するデジタルオーディオワークステーション LingStationX のエンジンを動かします。ウィンドウの有無にかかわらず、どのスクリプトからでも使えます。プロジェクトを組み立て、音符を編集し、エフェクトをかけ、音声を書き出せます。エンジンは `daw_*` 組み込み関数を初めて呼んだときに作られます。

知っておくこと：

- クリップの位置と長さ、ループ点、書き出し範囲の単位は**拍**です。`daw_secs_at(beat)` で秒に変換できます。
- トラック、クリップ、音符、エフェクトは 0 から始まる番号で指定します。項目を挿入・削除すると番号がずれます。
- はい/いいえの答えは `1` と `0` で返ります。何かを作る組み込み関数はその番号か `-1` を返します。
- 編集と書き出しにはオーディオデバイスが要りません。`daw_play()` はデバイスがあればリアルタイムで再生します。
- `daw_status()` は直前の操作が残したメッセージを返します。
- エフェクトの*対象*はトラック番号か、マスターバスを表す `-1` です。

主なグループ：

| グループ | 組み込み関数 |
|---|---|
| プロジェクト | `daw_new`, `daw_project_save(path)`, `daw_project_load(path)`, `daw_set_project_name(name)`, `daw_status` |
| トランスポートとテンポ | `daw_play`, `daw_pause`, `daw_stop`, `daw_seek(beat)`, `daw_position`, `daw_bpm`, `daw_set_bpm(bpm)`, `daw_set_loop(on, start, end)`, `daw_end_beat`, `daw_tempo_add(beat, bpm)`, `daw_secs_at(beat)` |
| トラック | `daw_track_add(kind, path)`, `daw_track_count`, `daw_track_remove(track)`, `daw_track_name(track)`, `daw_track_set_name(track, name)`, `daw_track_get(track, field)`, `daw_track_set(track, field, value)` |
| クリップと音符 | `daw_clip_add_midi(track, start, length)`, `daw_clip_add_audio(track, path, start)`, `daw_clip_notes(track, clip)`, `daw_note_add(track, clip, start, length, key, velocity)`, `daw_note_remove(track, clip, note)` |
| 楽器 | `daw_inst_presets(track)`, `daw_inst_preset(track, name)`, `daw_inst_params(track)`, `daw_inst_set(track, name, value)` |
| エフェクト | `daw_fx_types`, `daw_fx_add(target, name)`, `daw_fx_params(target, index)`, `daw_fx_set(target, index, param, value)`, `daw_fx_set_wet(target, index, wet)`, `daw_fx_bypass(target, index, on)` |
| 書き出しと取り消し | `daw_bounce(path, start, end, tail, bits, format, quality)`, `daw_render_start(...)`, `daw_undo_push`, `daw_undo`, `daw_redo` |
| MIDI | `daw_midi_import(path, beat)`, `daw_midi_export(track, path)`, `daw_midi_export_project(path)`, `daw_midi_inputs`, `daw_midi_open(index)` |

`daw_track_add` のトラックの種類は `"synth"`、`"drums"`、`"audio"`、`"sf2"`（SoundFont のパス付き）、`"growl"`、`"voice"`、`"prism"`、`"sampler"`、`"pluck"`、`"chip"`、`"cinema"`、`"script"` です。エンジンには、オートメーション、ボーカル、曲の生成、ミックス、書き出しのための `daw_*` 組み込み関数がさらに数百ありますが、ほとんどは LingStationX アプリケーション自身のためのものです。

オフラインの作業の一例：

```ling
束縛 開始 = 実行 {
    daw_new()
    daw_set_project_name("Demo")
    daw_set_bpm(100)

    束縛 リード = daw_track_add("synth")
    daw_inst_preset(リード, "FM Bell")
    束縛 クリップ = daw_clip_add_midi(リード, 0, 4)
    束縛 音高 = [60, 64, 67, 72]
    束縛 番号 = 0
    間 番号 < 4 {
        daw_note_add(リード, クリップ, 番号, 0.75, 音高[番号], 0.8)
        束縛 番号 = 番号 + 1
    }

    束縛 残響 = daw_fx_add(リード, "Reverb")
    daw_fx_set_wet(リード, 残響, 0.3)

    印刷("トラック数: " + str(daw_track_count()))
    印刷("プロジェクトの終わりの拍: " + str(daw_end_beat()))
    印刷("書き出し成功: " + str(daw_bounce("demo.wav", 0, 4, 1.0, 16, "wav", 0.5)))
}
```

```output
トラック数: 1
プロジェクトの終わりの拍: 4
書き出し成功: 1
```
