# 음악

`music_*` 내장 함수는 곡 단위의 음악을 다룹니다. 오디오 파일을 디코드하고 분석하며, 한 번에 한 곡을 재생하고, 악기 패치로 음을 연주하고, 음과 주파수를 서로 바꾸고, MIDI와 `.lrc` 가사 파일을 읽고, 리듬 게임의 판정을 내립니다. `daw_*` 내장 함수는 한 걸음 더 나아가 디지털 오디오 워크스테이션 LingStationX의 엔진을 움직입니다. 스크립트로 곡을 만들고, 이펙트를 걸고, 파일로 렌더링할 수 있습니다.

지속음, 효과음, 샘플 파일은 [오디오](audio.md)를 보세요.

규칙:

- 트랙, 패치, MIDI, 가사 핸들은 0부터 세는 작은 정수입니다. 불러오기에 실패하면 `-1`을 돌려줍니다. 모르는 핸들을 넘기면 오류 대신 `0`, 빈 문자열, 빈 리스트가 나옵니다.
- 경로는 파일이 있으면 그대로 쓰고, 없으면 실행 중인 스크립트 폴더를 기준으로 찾습니다.
- **음높이**는 MIDI 번호(60이 가운데 도, 69가 A4) 또는 `"C4"`, `"A#3"`, `"Db5"` 같은 음 이름입니다.
- 분석, MIDI, 가사, 오프라인 DAW 작업에는 오디오 장치가 필요 없으므로, 그 예제들은 어디서나 실행됩니다.

## 요약

| 내장 함수 | 中文 | 日本語 | 한국어 | ไทย | 용도 |
|---|---|---|---|---|---|
| `music_load(path)` | `载入音乐` | `音楽読込` | `음악로드` | `โหลดเพลง` | 오디오 파일 디코드 |
| `music_duration(track)` | `音乐时长` | `音楽長さ` | `음악길이` | `ความยาวเพลง` | 길이(초) |
| `music_bpm(track)` | `节拍速度` | `テンポ` | `템포` | `จังหวะต่อนาที` | 추정 템포 |
| `music_key(track)` | `调性` | `調性` | `조성` | `คีย์เพลง` | 추정 조성 |
| `music_onsets(track)` | `音符起点` | `オンセット` | `온셋` | `จุดเริ่มเสียง` | 타격과 음 시작 시각 |
| `music_beat_grid(track)` | `节拍网格` | `ビートグリッド` | `비트그리드` | `กริดจังหวะ` | 규칙적인 비트 시각 |
| `music_fft(track, bands)` | `音乐频谱` | `音楽スペクトル` | `음악스펙트럼` | `สเปกตรัมเพลง` | 재생 위치의 스펙트럼 |
| `music_fft_groups(track, size, groups)` | — | — | — | — | 큰 FFT 하나로 구한 크기 그룹 |
| `music_play(track)` | `播放音乐` | `音楽再生` | `음악재생` | `เล่นเพลง` | 처음부터 재생 |
| `music_pause()` | `暂停音乐` | `音楽一時停止` | `음악일시정지` | `หยุดเพลงชั่วคราว` | 일시 정지 |
| `music_stop()` | `停止音乐` | `音楽停止` | `음악정지` | `หยุดเพลง` | 정지 후 되감기 |
| `music_seek(seconds)` | `定位音乐` | `音楽シーク` | `음악탐색` | `ค้นหาเพลง` | 위치 이동 |
| `music_pos()` | `音乐位置` | `音楽位置` | `음악위치` | `ตำแหน่งเพลง` | 재생 위치 |
| `music_volume(level)` | `音乐音量` | `音楽音量` | `음악음량` | `ระดับเพลง` | 음악 엔진 음량 |
| `music_patch(path)` | `乐器音色` | `音色読込` | `악기패치` | `แพตช์เครื่องดนตรี` | 악기 패치 불러오기 |
| `music_note(inst, pitch, duration, velocity)` | `弹音符` | `音符演奏` | `음표연주` | `เล่นโน้ต` | 음 연주 |
| `music_note_on(inst, pitch, velocity)` | `音符开始` | `音符オン` | `음표켜기` | `โน้ตเริ่ม` | 건반 누르고 유지 |
| `music_note_off(inst, pitch)` | `音符结束` | `音符オフ` | `음표끄기` | `โน้ตจบ` | 건반 떼기 |
| `music_hz(pitch)` | `音符频率` | `音符周波数` | `음표주파수` | `ความถี่โน้ต` | 음높이를 주파수로 |
| `music_note_name(hz)` | `音名` | `音名称` | `음이름` | `ชื่อโน้ต` | 주파수를 음 이름으로 |
| `music_pitch_score(sung_hz, target_hz)` | `音准评分` | `音程スコア` | `음정점수` | `คะแนนเสียง` | 노래한 음높이 채점 |
| `music_mic_pitch()` | `麦克风音高` | `マイク音程` | `마이크음정` | `ระดับเสียงไมค์` | 마이크로 들린 음높이 |
| `music_judge(delta_ms)` | `判定` | `判定する` | `판정` | `ตัดสินจังหวะ` | 리듬 게임 타격 판정 |
| `music_grade_name(index)` | `判定名` | `判定名称` | `판정이름` | `ชื่อการตัดสิน` | 판정 이름 |
| `music_lrc(path)` | `载入歌词` | `歌詞読込` | `가사로드` | `โหลดเนื้อเพลง` | `.lrc` 가사 불러오기 |
| `music_lyric(lyrics, time)` | `当前歌词` | `現在歌詞` | `현재가사` | `เนื้อเพลงปัจจุบัน` | 그 시각의 가사 줄 |
| `music_midi_load(path)` | `载入MIDI` | `MIDI読込` | `미디로드` | `โหลดมิดี` | MIDI 파일 불러오기 |
| `music_midi_count(midi)` | `MIDI数量` | `MIDI数` | `미디수` | `จำนวนมิดี` | 음표 수 |
| `music_midi_notes(midi)` | `MIDI音符` | `MIDIノート` | `미디음표` | `โน้ตมิดี` | 음 시작 `[시각, 키, ...]` |
| `music_midi_bars(midi)` | `MIDI音条` | `MIDIバー` | `미디바` | `แท่งมิดี` | 길이가 있는 음표 `[시각, 키, 길이, ...]` |

`daw_*` 내장 함수에는 현지화된 이름이 없습니다.

## 음과 주파수

`music_hz(pitch)`는 MIDI 번호나 음 이름을 Hz로 바꿉니다. A4(69)가 440입니다. 읽을 수 없는 이름은 69로 봅니다. `music_note_name(hz)`는 가장 가까운 음을 샵 표기로 돌려주며, `hz`가 양수가 아니면 빈 문자열을 돌려줍니다. 옥타브는 C4가 MIDI 60인 과학적 표기를 따릅니다.

```ling
바인드 시작 = 실행 {
    출력(음표주파수("A4"))
    출력(반올림(음표주파수(60) * 100) / 100)
    출력(반올림(음표주파수("C5") * 100) / 100)
    출력(음이름(440.0))
    출력(음이름(262.0))
    출력(음이름(466.2))
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

음계용 내장 함수는 없지만, 음계는 반음 간격의 리스트일 뿐입니다. 이 예제는 C 장조 5음 음계를 출력합니다.

```ling
바인드 시작 = 실행 {
    바인드 간격 = [0, 2, 4, 7, 9]
    바인드 결과 = []
    바인드 i = 0
    동안 i < 길이(간격) {
        바인드 결과 = 결과.추가(음이름(음표주파수(60 + 간격[i])))
        바인드 i = i + 1
    }
    출력(결과)
}
```

```output
[C4, D4, E4, G4, A4]
```

## 불러오기와 분석

| 내장 함수 | 반환값 |
|---|---|
| `music_load(path)` | 트랙 핸들 또는 `-1`. WAV, FLAC, OGG(Vorbis), MP3, AAC/M4A를 받습니다. 파일 전체를 메모리에 디코드합니다. |
| `music_duration(track)` | 길이(초). |
| `music_bpm(track)` | 템포 추정값. 0.1 단위로 반올림하고 70~170 BPM 안으로 접습니다. 곡에 따라 한 옥타브 어긋날 수 있습니다. |
| `music_key(track)` | `"A minor"`, `"C major"` 같은 문자열(장조와 단조만). |
| `music_onsets(track)` | 음이나 타격이 시작되는 시각(초)의 리스트. |
| `music_beat_grid(track)` | 첫 온셋에서 시작해 추정 템포로 늘어선 규칙적인 비트 시각. 템포 변화는 따라가지 않습니다. |
| `music_fft(track, bands)` | 현재 재생 위치의 2048 샘플 창에서 구한, 로그 간격으로 스무딩된 크기 `bands`개(기본값 16), 저음부터. `music_play`가 시작되기 전에는 무음입니다. |
| `music_fft_groups(track, size, groups)` | `size` 샘플(기본값 8192) FFT 하나에서 30 Hz~16 kHz 사이를 나눈 크기 값 `groups`개(기본값 7). 최대 크기의 사인파가 약 0.5로 읽히며, 조용한 부분은 조용하게 나옵니다. |

이 예제는 파일이 필요 없습니다. DAW(아래 참고)로 두 마디짜리 킥 드럼 루프를 렌더링한 뒤 분석합니다.

```ling
바인드 시작 = 실행 {
    daw_new()
    daw_set_bpm(120)
    바인드 드럼 = daw_track_add("drums")
    바인드 클립 = daw_clip_add_midi(드럼, 0, 8)
    바인드 박자 = 0
    동안 박자 < 16 {
        daw_note_add(드럼, 클립, 박자 * 0.5, 0.25, 36, 0.9)
        바인드 박자 = 박자 + 1
    }
    daw_bounce("loop.wav", 0, 8, 0.0, 16, "wav", 0.5)

    바인드 트랙 = 음악로드("loop.wav")
    출력("초: " + str(반올림(음악길이(트랙))))
    출력("BPM: " + str(반올림(템포(트랙))))
    출력("온셋: " + str(길이(온셋(트랙))))
    출력("그리드의 비트: " + str(길이(비트그리드(트랙))))
    출력(음악로드("missing.wav"))
}
```

```output
초: 4
BPM: 120
온셋: 15
그리드의 비트: 8
-1
```

## 재생

음악 엔진은 처음 쓸 때 [오디오](audio.md) 엔진과 별개인 자체 출력 스트림을 열고, 한 번에 한 곡을 재생합니다.

- `music_play(track)`은 `track`을 현재 곡으로 정하고 **처음부터** 재생합니다. 다시 부르면 처음부터 다시 시작합니다. 곡이 끝나면 멈춥니다(반복하지 않음).
- `music_pause()`는 위치를 유지한 채 일시 정지합니다. 이어서 재생하려면 `music_play(-1)`을 부르세요. 불러온 곡이 아닌 ID를 넘기면 현재 곡이 이어집니다.
- `music_stop()`은 멈추고 되감습니다. `music_seek(seconds)`는 위치를 옮깁니다. `music_pos()`는 위치를 초로 돌려줍니다.
- `music_volume(level)`은 음악 엔진(트랙과 신시사이저 모두)의 음량을 정합니다(기본값 0.8, 0..2로 잘림). 출력은 소프트 클리핑되므로, 1을 넘는 값은 부드럽게 일그러집니다.

```ling-gui
바인드 시작 = 실행 {
    창열기(640, 360, "비주얼라이저")
    바인드 노래 = 음악로드("song.ogg")
    음악음량(0.8)
    음악재생(노래)
    동안 창열림() {
        채우기(10, 10, 20)
        바인드 막대들 = 음악스펙트럼(노래, 16)
        바인드 i = 0
        동안 i < 16 {
            바인드 막대높이 = 최솟값(300, 막대들[i] * 40)
            색설정(80 + i * 10, 200, 255)
            선그리기(30 + i * 38, 340, 30 + i * 38, 340 - 막대높이)
            바인드 i = i + 1
        }
        표시()
    }
}
```

## 합성

음악 엔진에는 FM/감산 방식의 폴리포닉 신시사이저가 들어 있습니다. 악기는 **패치** 파일입니다. 평범한 `key=value` 토큰으로 이루어지며, `#`부터는 주석입니다. 오퍼레이터는 1부터 번호를 매기고, `opN.fm`으로 다른 오퍼레이터를 주파수 변조할 수 있습니다.

```text
name = "Rhodes EP"
poly = 16
op1.wave=sine  op1.ratio=1.0  op1.level=1.0  op1.adsr=0.002,1.2,0.0,0.4
op2.wave=sine  op2.ratio=14.0 op2.level=0.5  op2.fm=op1  op2.adsr=0.001,0.2,0.0,0.2
filter.cutoff=0.8 filter.reso=0.2 filter.env=0.3 filter.adsr=0,0.5,0.2,0.3
lfo.rate=5.0 lfo.pitch=0.05
amp = 0.7
```

키: `name`, `gm`(General MIDI 프로그램), `poly`(보이스 수, 기본값 16), `amp`, `lfo.rate`, `lfo.pitch`, `lfo.cutoff`, `lfo.amp`, `filter.cutoff`, `filter.reso`, `filter.env`, `filter.adsr`, `filter.on`, 그리고 오퍼레이터별 `opN.wave`(`sine`, `saw`, `square`/`pulse`, `tri`, `noise`), `opN.ratio`, `opN.fixed`(Hz), `opN.level`, `opN.detune`, `opN.adsr`(어택, 디케이, 서스테인, 릴리스), `opN.fm`. 모르는 키는 무시됩니다. 저장소의 `instruments/` 폴더에 바로 쓸 수 있는 패치가 들어 있습니다.

- `music_patch(path)`는 패치를 불러와 악기 핸들(트랙 핸들과 별개)을 돌려주며, 실패하면 `-1`입니다.
- `music_note(inst, pitch, duration, velocity)`는 음을 `duration`초(기본값 0.5) 동안 `velocity` 0~1(기본값 0.9)로 누릅니다. 그 뒤로 패치의 릴리스가 이어집니다.
- `music_note_on(inst, pitch, velocity)`는 건반을 누르고, `music_note_off(inst, pitch)`는 뗍니다. 짝을 맞춰 쓰세요.

```ling-gui
바인드 시작 = 실행 {
    바인드 피아노 = 악기패치("instruments/grand_piano.ling")
    음표연주(피아노, "C4", 0.5, 0.9)
    음표연주(피아노, "E4", 0.5, 0.8)
    음표켜기(피아노, 55, 0.7)
    잠(1000)
    음표끄기(피아노, 55)
}
```

## 노래와 리듬 게임

`music_mic_pitch()`는 마이크로 들린 기본 주파수(55~1000 Hz)를 돌려줍니다. 마이크가 닫혀 있거나, 신호가 너무 작거나, 뚜렷한 음높이가 없으면 0입니다. 먼저 `mic_open()`으로 마이크를 여세요.

`music_pitch_score(sung_hz, target_hz)`는 노래한 음높이를 0~1로 채점합니다. 1은 정확히 맞은 것이고, 0은 반음 이상 벗어난 것입니다. 오차는 가장 가까운 옥타브로 접히므로, 한 옥타브 차이로 불러도 점수가 잘 나옵니다.

`music_judge(delta_ms)`는 타이밍 오차(밀리초, 부호는 무시)로 타격을 판정합니다. 0 Perfect(30 ms까지), 1 Great(60), 2 Good(100), 3 Ok(140), 4 Miss입니다. `music_grade_name(index)`는 `"PERFECT"`, `"GREAT"`, `"GOOD"`, `"OK"`, `"MISS"` 중 하나를 돌려줍니다.

```ling
바인드 시작 = 실행 {
    출력(음정점수(440.0, 440.0))
    출력(반올림(음정점수(450.0, 440.0) * 100) / 100)
    출력(판정(12.0))
    출력(판정이름(판정(-45.0)))
    출력(판정이름(판정(500.0)))
}
```

```output
1
0.61
0
GREAT
MISS
```

## 가사와 MIDI 파일

`music_lrc(path)`는 각 줄이 `[mm:ss.xx] 텍스트` 형식인 시간 가사 파일을 불러와 핸들을 돌려줍니다. `music_lyric(lyrics, time)`은 시각이 `time`(보통 `music_pos()`)보다 늦지 않은 마지막 줄을 돌려주며, 첫 줄 전에는 빈 문자열입니다.

```ling
바인드 시작 = 실행 {
    write_file("song.lrc", "[00:01.00] 첫째 줄\n[00:03.50] 둘째 줄\n")
    바인드 가사 = 가사로드("song.lrc")
    출력("[" + 현재가사(가사, 0.5) + "]")
    출력(현재가사(가사, 2.0))
    출력(현재가사(가사, 99.0))
}
```

```output
[]
첫째 줄
둘째 줄
```

`music_midi_load(path)`는 Standard MIDI File을 데이터로 불러옵니다. 아무것도 재생되지 않습니다. 음표로 동전을 만들거나, 떨어지는 막대를 그리거나, 신호를 보내는 데 쓰세요. `music_midi_count`는 음표 수를, `music_midi_notes`는 평평한 리스트 `[시각, 키, 시각, 키, ...]`를, `music_midi_bars`는 평평한 리스트 `[시각, 키, 길이, ...]`를 돌려주며, 모두 초 단위입니다.

```ling
바인드 시작 = 실행 {
    daw_new()
    daw_set_bpm(120)
    바인드 트랙 = daw_track_add("synth")
    바인드 클립 = daw_clip_add_midi(트랙, 0, 4)
    daw_note_add(트랙, 클립, 0, 1, 60, 0.8)
    daw_note_add(트랙, 클립, 2, 2, 67, 0.8)
    daw_midi_export_project("song.mid")

    바인드 미디 = 미디로드("song.mid")
    출력(미디수(미디))
    출력(미디음표(미디))
    출력(미디바(미디))
}
```

```output
2
[0, 60, 1, 67]
[0, 60, 0.5, 1, 67, 1]
```

## DAW

`daw_*` 내장 함수는 Ling과 함께 제공되는 디지털 오디오 워크스테이션 LingStationX의 엔진을 움직입니다. 창이 있든 없든 어떤 스크립트에서나 쓸 수 있습니다. 프로젝트를 만들고, 음표를 편집하고, 이펙트를 걸고, 오디오를 렌더링합니다. 엔진은 `daw_*` 내장 함수를 처음 부를 때 만들어집니다.

알아 둘 것:

- 클립 위치, 길이, 루프 지점, 렌더 범위는 **비트** 단위입니다. `daw_secs_at(beat)`로 초로 바꿉니다.
- 트랙, 클립, 음표, 이펙트는 0부터 시작하는 인덱스로 가리킵니다. 항목을 넣거나 빼면 인덱스가 밀립니다.
- 예/아니요 답은 `1`과 `0`으로 돌아옵니다. 무언가를 만드는 내장 함수는 그 인덱스를, 실패하면 `-1`을 돌려줍니다.
- 편집과 렌더링에는 오디오 장치가 필요 없습니다. `daw_play()`는 장치가 있으면 실시간으로 재생합니다.
- `daw_status()`는 마지막 작업이 남긴 메시지를 돌려줍니다.
- 이펙트의 *대상*은 트랙 인덱스이며, 마스터 버스는 `-1`입니다.

주요 그룹:

| 그룹 | 내장 함수 |
|---|---|
| 프로젝트 | `daw_new`, `daw_project_save(path)`, `daw_project_load(path)`, `daw_set_project_name(name)`, `daw_status` |
| 트랜스포트와 템포 | `daw_play`, `daw_pause`, `daw_stop`, `daw_seek(beat)`, `daw_position`, `daw_bpm`, `daw_set_bpm(bpm)`, `daw_set_loop(on, start, end)`, `daw_end_beat`, `daw_tempo_add(beat, bpm)`, `daw_secs_at(beat)` |
| 트랙 | `daw_track_add(kind, path)`, `daw_track_count`, `daw_track_remove(track)`, `daw_track_name(track)`, `daw_track_set_name(track, name)`, `daw_track_get(track, field)`, `daw_track_set(track, field, value)` |
| 클립과 음표 | `daw_clip_add_midi(track, start, length)`, `daw_clip_add_audio(track, path, start)`, `daw_clip_notes(track, clip)`, `daw_note_add(track, clip, start, length, key, velocity)`, `daw_note_remove(track, clip, note)` |
| 악기 | `daw_inst_presets(track)`, `daw_inst_preset(track, name)`, `daw_inst_params(track)`, `daw_inst_set(track, name, value)` |
| 이펙트 | `daw_fx_types`, `daw_fx_add(target, name)`, `daw_fx_params(target, index)`, `daw_fx_set(target, index, param, value)`, `daw_fx_set_wet(target, index, wet)`, `daw_fx_bypass(target, index, on)` |
| 렌더링과 실행 취소 | `daw_bounce(path, start, end, tail, bits, format, quality)`, `daw_render_start(...)`, `daw_undo_push`, `daw_undo`, `daw_redo` |
| MIDI | `daw_midi_import(path, beat)`, `daw_midi_export(track, path)`, `daw_midi_export_project(path)`, `daw_midi_inputs`, `daw_midi_open(index)` |

`daw_track_add`의 트랙 종류는 `"synth"`, `"drums"`, `"audio"`, `"sf2"`(SoundFont 경로와 함께), `"growl"`, `"voice"`, `"prism"`, `"sampler"`, `"pluck"`, `"chip"`, `"cinema"`, `"script"`입니다. 엔진에는 오토메이션, 보컬, 곡 생성, 믹싱, 내보내기를 위한 `daw_*` 내장 함수가 수백 개 더 있으며, 대부분 LingStationX 애플리케이션 자체를 위한 것입니다.

완전한 오프라인 세션:

```ling
바인드 시작 = 실행 {
    daw_new()
    daw_set_project_name("Demo")
    daw_set_bpm(100)

    바인드 리드 = daw_track_add("synth")
    daw_inst_preset(리드, "FM Bell")
    바인드 클립 = daw_clip_add_midi(리드, 0, 4)
    바인드 음높이들 = [60, 64, 67, 72]
    바인드 i = 0
    동안 i < 4 {
        daw_note_add(리드, 클립, i, 0.75, 음높이들[i], 0.8)
        바인드 i = i + 1
    }

    바인드 잔향 = daw_fx_add(리드, "Reverb")
    daw_fx_set_wet(리드, 잔향, 0.3)

    출력("트랙: " + str(daw_track_count()))
    출력("프로젝트 끝 비트: " + str(daw_end_beat()))
    출력("렌더 성공: " + str(daw_bounce("demo.wav", 0, 4, 1.0, 16, "wav", 0.5)))
}
```

```output
트랙: 1
프로젝트 끝 비트: 4
렌더 성공: 1
```
