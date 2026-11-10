# Music

The `music_*` builtins work with whole pieces of music. They decode and analyse audio files, play one track at a time, play notes on instrument patches, convert between notes and frequencies, read MIDI and `.lrc` lyric files, and judge rhythm-game hits. The `daw_*` builtins go further: they drive the engine of the LingStationX digital audio workstation, so a script can build a song, add effects and render it to a file.

For tones, sound effects and sample files, see [Audio](audio.md).

Conventions:

- Track, patch, MIDI and lyric handles are small integers counting from 0. A failed load returns `-1`. An unknown handle gives `0`, an empty string or an empty list, never an error.
- Paths are used as given if the file exists, otherwise relative to the running script's folder.
- A **pitch** is a MIDI number (60 is middle C, 69 is A4) or a note name such as `"C4"`, `"A#3"` or `"Db5"`.
- Analysis, MIDI, lyrics and offline DAW work need no audio device, so those examples run anywhere.

## Summary

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `music_load(path)` | `载入音乐` | `音楽読込` | `음악로드` | `โหลดเพลง` | Decode an audio file |
| `music_duration(track)` | `音乐时长` | `音楽長さ` | `음악길이` | `ความยาวเพลง` | Length in seconds |
| `music_bpm(track)` | `节拍速度` | `テンポ` | `템포` | `จังหวะต่อนาที` | Estimated tempo |
| `music_key(track)` | `调性` | `調性` | `조성` | `คีย์เพลง` | Estimated key |
| `music_onsets(track)` | `音符起点` | `オンセット` | `온셋` | `จุดเริ่มเสียง` | Times of hits and note starts |
| `music_beat_grid(track)` | `节拍网格` | `ビートグリッド` | `비트그리드` | `กริดจังหวะ` | Regular beat times |
| `music_fft(track, bands)` | `音乐频谱` | `音楽スペクトル` | `음악스펙트럼` | `สเปกตรัมเพลง` | Spectrum at the playback position |
| `music_fft_groups(track, size, groups)` | — | — | — | — | Loudness groups from one large FFT |
| `music_play(track)` | `播放音乐` | `音楽再生` | `음악재생` | `เล่นเพลง` | Play a track from the start |
| `music_pause()` | `暂停音乐` | `音楽一時停止` | `음악일시정지` | `หยุดเพลงชั่วคราว` | Pause |
| `music_stop()` | `停止音乐` | `音楽停止` | `음악정지` | `หยุดเพลง` | Stop and rewind |
| `music_seek(seconds)` | `定位音乐` | `音楽シーク` | `음악탐색` | `ค้นหาเพลง` | Jump to a position |
| `music_pos()` | `音乐位置` | `音楽位置` | `음악위치` | `ตำแหน่งเพลง` | Playback position |
| `music_volume(level)` | `音乐音量` | `音楽音量` | `음악음량` | `ระดับเพลง` | Music-engine volume |
| `music_patch(path)` | `乐器音色` | `音色読込` | `악기패치` | `แพตช์เครื่องดนตรี` | Load an instrument patch |
| `music_note(inst, pitch, duration, velocity)` | `弹音符` | `音符演奏` | `음표연주` | `เล่นโน้ต` | Play a note |
| `music_note_on(inst, pitch, velocity)` | `音符开始` | `音符オン` | `음표켜기` | `โน้ตเริ่ม` | Press and hold a key |
| `music_note_off(inst, pitch)` | `音符结束` | `音符オフ` | `음표끄기` | `โน้ตจบ` | Release a key |
| `music_hz(pitch)` | `音符频率` | `音符周波数` | `음표주파수` | `ความถี่โน้ต` | Pitch to frequency |
| `music_note_name(hz)` | `音名` | `音名称` | `음이름` | `ชื่อโน้ต` | Frequency to note name |
| `music_pitch_score(sung_hz, target_hz)` | `音准评分` | `音程スコア` | `음정점수` | `คะแนนเสียง` | Score a sung pitch |
| `music_mic_pitch()` | `麦克风音高` | `マイク音程` | `마이크음정` | `ระดับเสียงไมค์` | Pitch heard on the microphone |
| `music_judge(delta_ms)` | `判定` | `判定する` | `판정` | `ตัดสินจังหวะ` | Grade a rhythm-game hit |
| `music_grade_name(index)` | `判定名` | `判定名称` | `판정이름` | `ชื่อการตัดสิน` | Name of a grade |
| `music_lrc(path)` | `载入歌词` | `歌詞読込` | `가사로드` | `โหลดเนื้อเพลง` | Load `.lrc` lyrics |
| `music_lyric(lyrics, time)` | `当前歌词` | `現在歌詞` | `현재가사` | `เนื้อเพลงปัจจุบัน` | Lyric line at a time |
| `music_midi_load(path)` | `载入MIDI` | `MIDI読込` | `미디로드` | `โหลดมิดี` | Load a MIDI file |
| `music_midi_count(midi)` | `MIDI数量` | `MIDI数` | `미디수` | `จำนวนมิดี` | Number of notes |
| `music_midi_notes(midi)` | `MIDI音符` | `MIDIノート` | `미디음표` | `โน้ตมิดี` | Note onsets `[time, key, ...]` |
| `music_midi_bars(midi)` | `MIDI音条` | `MIDIバー` | `미디바` | `แท่งมิดี` | Notes with lengths `[time, key, length, ...]` |

The `daw_*` builtins have no localized names.

## Notes and frequencies

`music_hz(pitch)` converts a MIDI number or note name to Hz; A4 (69) is 440. An unreadable name counts as 69. `music_note_name(hz)` gives the nearest note with sharp spelling, or an empty string if `hz` is not positive. Octaves follow the scientific convention, where C4 is MIDI 60.

```ling
bind start = do {
    print(music_hz("A4"))
    print(round(music_hz(60) * 100) / 100)
    print(round(music_hz("C5") * 100) / 100)
    print(music_note_name(440.0))
    print(music_note_name(262.0))
    print(music_note_name(466.2))
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

There are no scale builtins, but a scale is just a list of semitone steps. This prints a C major pentatonic scale:

```ling
bind start = do {
    bind steps = [0, 2, 4, 7, 9]
    bind out = []
    bind i = 0
    while i < len(steps) {
        bind out = out.push(music_note_name(music_hz(60 + steps[i])))
        bind i = i + 1
    }
    print(out)
}
```

```output
[C4, D4, E4, G4, A4]
```

## Loading and analysis

| Builtin | Returns |
|---|---|
| `music_load(path)` | A track handle, or `-1`. Accepts WAV, FLAC, OGG (Vorbis), MP3 and AAC/M4A. The whole file is decoded into memory. |
| `music_duration(track)` | Length in seconds. |
| `music_bpm(track)` | Tempo estimate, rounded to 0.1 and folded into 70 to 170 BPM. It can be off by an octave on some material. |
| `music_key(track)` | A string such as `"A minor"` or `"C major"` (major and minor only). |
| `music_onsets(track)` | A list of times (seconds) where notes or hits begin. |
| `music_beat_grid(track)` | A regular grid of beat times at the detected tempo, starting at the first onset. It does not follow tempo changes. |
| `music_fft(track, bands)` | `bands` log-spaced, smoothed magnitudes (default 16), bass first, from a 2048-sample window at the current playback position. Silent until `music_play` has started. |
| `music_fft_groups(track, size, groups)` | `groups` loudness values (default 7) between 30 Hz and 16 kHz from one FFT of `size` samples (default 8192). A full-scale sine reads about 0.5; quiet passages stay quiet. |

This example needs no files: it renders a two-bar kick-drum loop with the DAW (see below), then analyses it.

```ling
bind start = do {
    daw_new()
    daw_set_bpm(120)
    bind drums = daw_track_add("drums")
    bind clip = daw_clip_add_midi(drums, 0, 8)
    bind beat = 0
    while beat < 16 {
        daw_note_add(drums, clip, beat * 0.5, 0.25, 36, 0.9)
        bind beat = beat + 1
    }
    daw_bounce("loop.wav", 0, 8, 0.0, 16, "wav", 0.5)

    bind track = music_load("loop.wav")
    print("seconds: " + str(round(music_duration(track))))
    print("bpm: " + str(round(music_bpm(track))))
    print("onsets: " + str(len(music_onsets(track))))
    print("beats in grid: " + str(len(music_beat_grid(track))))
    print(music_load("missing.wav"))
}
```

```output
seconds: 4
bpm: 120
onsets: 15
beats in grid: 8
-1
```

## Playback

The music engine opens its own output stream on first use, separate from the [Audio](audio.md) engine, and plays one track at a time.

- `music_play(track)` makes `track` current and starts it **from the beginning**; calling it again restarts. Playback stops at the end (no loop).
- `music_pause()` pauses and keeps the position. To resume, call `music_play(-1)`: an id that is not a loaded track continues the current one.
- `music_stop()` stops and rewinds. `music_seek(seconds)` jumps. `music_pos()` returns the position in seconds.
- `music_volume(level)` sets the volume of the music engine, tracks and synthesiser together (default 0.8, clamped to 0..2). Output is soft-clipped, so levels above 1 distort gently.

```ling-gui
bind start = do {
    open_window(640, 360, "Visualizer")
    bind song = music_load("song.ogg")
    music_volume(0.8)
    music_play(song)
    while window_is_open() {
        fill(10, 10, 20)
        bind bars = music_fft(song, 16)
        bind i = 0
        while i < 16 {
            bind h = min(300, bars[i] * 40)
            set_color(80 + i * 10, 200, 255)
            draw_line(30 + i * 38, 340, 30 + i * 38, 340 - h)
            bind i = i + 1
        }
        present()
    }
}
```

## Synthesis

The music engine includes a polyphonic FM/subtractive synthesiser. An instrument is a **patch** file: plain `key=value` tokens, with `#` starting a comment. Operators are numbered from 1, and an operator can frequency-modulate another with `opN.fm`.

```text
name = "Rhodes EP"
poly = 16
op1.wave=sine  op1.ratio=1.0  op1.level=1.0  op1.adsr=0.002,1.2,0.0,0.4
op2.wave=sine  op2.ratio=14.0 op2.level=0.5  op2.fm=op1  op2.adsr=0.001,0.2,0.0,0.2
filter.cutoff=0.8 filter.reso=0.2 filter.env=0.3 filter.adsr=0,0.5,0.2,0.3
lfo.rate=5.0 lfo.pitch=0.05
amp = 0.7
```

Keys: `name`, `gm` (General MIDI program), `poly` (voices, default 16), `amp`, `lfo.rate`, `lfo.pitch`, `lfo.cutoff`, `lfo.amp`, `filter.cutoff`, `filter.reso`, `filter.env`, `filter.adsr`, `filter.on`, and per operator `opN.wave` (`sine`, `saw`, `square`/`pulse`, `tri`, `noise`), `opN.ratio`, `opN.fixed` (Hz), `opN.level`, `opN.detune`, `opN.adsr` (attack, decay, sustain, release) and `opN.fm`. Unknown keys are ignored. Ready-made patches ship in the repository's `instruments/` folder.

- `music_patch(path)` loads a patch and returns an instrument handle (separate from track handles), or `-1`.
- `music_note(inst, pitch, duration, velocity)` plays a note held for `duration` seconds (default 0.5) at `velocity` 0 to 1 (default 0.9); the patch's release follows.
- `music_note_on(inst, pitch, velocity)` presses a key; `music_note_off(inst, pitch)` releases it. Pair them.

```ling-gui
bind start = do {
    bind piano = music_patch("instruments/grand_piano.ling")
    music_note(piano, "C4", 0.5, 0.9)
    music_note(piano, "E4", 0.5, 0.8)
    music_note_on(piano, 55, 0.7)
    sleep(1000)
    music_note_off(piano, 55)
}
```

## Singing and rhythm games

`music_mic_pitch()` returns the fundamental frequency heard on the microphone (55 to 1000 Hz), or 0 if the microphone is closed, the signal is too quiet or no clear pitch is found. Open the microphone with `mic_open()` first.

`music_pitch_score(sung_hz, target_hz)` scores a sung pitch from 0 to 1: 1 is exactly on pitch, 0 is a semitone or more off. The error is folded to the nearest octave, so singing an octave away still scores well.

`music_judge(delta_ms)` grades a hit by its timing error in milliseconds (the sign is ignored): 0 Perfect (up to 30 ms), 1 Great (60), 2 Good (100), 3 Ok (140), 4 Miss. `music_grade_name(index)` returns `"PERFECT"`, `"GREAT"`, `"GOOD"`, `"OK"` or `"MISS"`.

```ling
bind start = do {
    print(music_pitch_score(440.0, 440.0))
    print(round(music_pitch_score(450.0, 440.0) * 100) / 100)
    print(music_judge(12.0))
    print(music_grade_name(music_judge(-45.0)))
    print(music_grade_name(music_judge(500.0)))
}
```

```output
1
0.61
0
GREAT
MISS
```

## Lyrics and MIDI files

`music_lrc(path)` loads a timed-lyrics file whose lines look like `[mm:ss.xx] text`, and returns a handle. `music_lyric(lyrics, time)` returns the latest line whose time is not after `time` (usually `music_pos()`), or an empty string before the first line.

```ling
bind start = do {
    write_file("song.lrc", "[00:01.00] First line\n[00:03.50] Second line\n")
    bind lyrics = music_lrc("song.lrc")
    print("[" + music_lyric(lyrics, 0.5) + "]")
    print(music_lyric(lyrics, 2.0))
    print(music_lyric(lyrics, 99.0))
}
```

```output
[]
First line
Second line
```

`music_midi_load(path)` loads a Standard MIDI File as data; nothing plays. Use the notes to spawn coins, draw falling bars or trigger cues. `music_midi_count` returns the number of notes, `music_midi_notes` a flat list `[time, key, time, key, ...]` and `music_midi_bars` a flat list `[time, key, length, ...]`, all in seconds.

```ling
bind start = do {
    daw_new()
    daw_set_bpm(120)
    bind t = daw_track_add("synth")
    bind c = daw_clip_add_midi(t, 0, 4)
    daw_note_add(t, c, 0, 1, 60, 0.8)
    daw_note_add(t, c, 2, 2, 67, 0.8)
    daw_midi_export_project("song.mid")

    bind m = music_midi_load("song.mid")
    print(music_midi_count(m))
    print(music_midi_notes(m))
    print(music_midi_bars(m))
}
```

```output
2
[0, 60, 1, 67]
[0, 60, 0.5, 1, 67, 1]
```

## The DAW

The `daw_*` builtins drive the engine of LingStationX, the digital audio workstation that ships with Ling. Any script can use it, with or without a window: build a project, edit notes, add effects and render audio. The engine is created the first time a `daw_*` builtin is called.

Things to know:

- Clip positions, lengths, loop points and render ranges are in **beats**. `daw_secs_at(beat)` converts to seconds.
- Tracks, clips, notes and effects are addressed by zero-based index. Indices shift when you insert or remove items.
- Yes/no answers come back as `1` and `0`. Builtins that create something return its index, or `-1`.
- Editing and rendering need no audio device. `daw_play()` plays in real time when a device exists.
- `daw_status()` returns the message left by the last operation.
- An effect *target* is a track index, or `-1` for the master bus.

The main groups:

| Group | Builtins |
|---|---|
| Project | `daw_new`, `daw_project_save(path)`, `daw_project_load(path)`, `daw_set_project_name(name)`, `daw_status` |
| Transport and tempo | `daw_play`, `daw_pause`, `daw_stop`, `daw_seek(beat)`, `daw_position`, `daw_bpm`, `daw_set_bpm(bpm)`, `daw_set_loop(on, start, end)`, `daw_end_beat`, `daw_tempo_add(beat, bpm)`, `daw_secs_at(beat)` |
| Tracks | `daw_track_add(kind, path)`, `daw_track_count`, `daw_track_remove(track)`, `daw_track_name(track)`, `daw_track_set_name(track, name)`, `daw_track_get(track, field)`, `daw_track_set(track, field, value)` |
| Clips and notes | `daw_clip_add_midi(track, start, length)`, `daw_clip_add_audio(track, path, start)`, `daw_clip_notes(track, clip)`, `daw_note_add(track, clip, start, length, key, velocity)`, `daw_note_remove(track, clip, note)` |
| Instruments | `daw_inst_presets(track)`, `daw_inst_preset(track, name)`, `daw_inst_params(track)`, `daw_inst_set(track, name, value)` |
| Effects | `daw_fx_types`, `daw_fx_add(target, name)`, `daw_fx_params(target, index)`, `daw_fx_set(target, index, param, value)`, `daw_fx_set_wet(target, index, wet)`, `daw_fx_bypass(target, index, on)` |
| Rendering and undo | `daw_bounce(path, start, end, tail, bits, format, quality)`, `daw_render_start(...)`, `daw_undo_push`, `daw_undo`, `daw_redo` |
| MIDI | `daw_midi_import(path, beat)`, `daw_midi_export(track, path)`, `daw_midi_export_project(path)`, `daw_midi_inputs`, `daw_midi_open(index)` |

Track kinds for `daw_track_add` are `"synth"`, `"drums"`, `"audio"`, `"sf2"` (with a SoundFont path), `"growl"`, `"voice"`, `"prism"`, `"sampler"`, `"pluck"`, `"chip"`, `"cinema"` and `"script"`. The engine has several hundred more `daw_*` builtins for automation, vocals, song generation, mixing and export; they mostly serve the LingStationX application itself.

A complete offline session:

```ling
bind start = do {
    daw_new()
    daw_set_project_name("Demo")
    daw_set_bpm(100)

    bind lead = daw_track_add("synth")
    daw_inst_preset(lead, "FM Bell")
    bind clip = daw_clip_add_midi(lead, 0, 4)
    bind pitches = [60, 64, 67, 72]
    bind i = 0
    while i < 4 {
        daw_note_add(lead, clip, i, 0.75, pitches[i], 0.8)
        bind i = i + 1
    }

    bind verb = daw_fx_add(lead, "Reverb")
    daw_fx_set_wet(lead, verb, 0.3)

    print("tracks: " + str(daw_track_count()))
    print("project ends at beat " + str(daw_end_beat()))
    print("render ok: " + str(daw_bounce("demo.wav", 0, 4, 1.0, 16, "wav", 0.5)))
}
```

```output
tracks: 1
project ends at beat 4
render ok: 1
```
