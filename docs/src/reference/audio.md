# Audio

The audio engine plays everything a game needs: continuous tones placed in 3D space, short blips and sound effects, sample files, a looping background track and a master effect chain. A separate FFT analyser turns sample data into frequency bands for audio-reactive visuals.

For whole songs, instrument patches, MIDI and the DAW, see [Music](music.md). For the microphone, see [Input and Microphone](input.md).

Things to know first:

- The runtime opens the audio device when the program starts. With no output device, the `audio_*` builtins silently do nothing.
- Times are in seconds. Arguments shown in a signature are optional from the point where a default is listed.
- Audio is native only: in the browser build the FFT builtins do nothing and some calls take fewer arguments (noted below).
- Because these examples make sound, they are `ling-gui` programs. The FFT example at the end runs anywhere.

## Summary

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `audio_volume(level)` | `音量` | `音量` | `음량` | `ระดับเสียง` | Master volume |
| `audio_bgm(path, volume)` | `背景乐` | `BGM` | `배경음악` | `เพลงประกอบ` | Loop a WAV as background music |
| `audio_bgm_volume(level)` | `背景乐音量` | `BGM音量` | `배경음악음량` | `ระดับเพลงประกอบ` | Background-music volume |
| `audio_tone(slot, x, y, z, w, freq, amp, lfo_rate, lfo_depth, wave, duty)` | `音调` | `音調` | `음조` | `เสียงโทน` | Continuous positional oscillator |
| `audio_glide(amp_ms, freq_ms)` | — | — | — | — | Smoothing time for tone changes |
| `audio_listener(cos_yaw, sin_yaw, cos_pitch, sin_pitch)` | `音频监听` | `音声リスナー` | `오디오리스너` | `ผู้ฟัง` | Listener orientation |
| `audio_blip(freq, duration, wave, amp)` | `提示音` | `ビープ音` | `효과음` | `เสียงบี๊บ` | Short interface blip |
| `ui_sound(name)` | `界面音` | `UI音` | `인터페이스음` | `เสียงปุ่ม` | Built-in interface sound |
| `audio_sfx(x, y, z, w, freq, amp, duration, wave)` | `音效` | `空間効果音` | `공간효과음` | `เสียงเอฟเฟกต์` | Positional one-shot effect |
| `morph_note(x, y, z, w, freq, amp, duration, material, morph)` | `变形音` | `モーフ音` | `모프음` | `โน้ตมอร์ฟ` | Physical/digital morph note |
| `audio_stop_sfx()` | `停止音效` | `効果音停止` | `효과음정지` | `หยุดเอฟเฟกต์ทั้งหมด` | Stop all one-shot voices |
| `audio_sample_load(path)` | `载入采样` | `サンプル読込` | `샘플로드` | `โหลดตัวอย่างเสียง` | Decode a sound file |
| `audio_sample_play(id, x, y, z, w, volume, loop, pitch)` | `播放采样` | `サンプル再生` | `샘플재생` | `เล่นตัวอย่างเสียง` | Play a sample in 3D |
| `audio_sample_play_flat(id, volume, loop, pitch)` | — | — | — | — | Play a sample without positioning |
| `audio_sample_stop(voice)` | `停止采样` | `サンプル停止` | `샘플정지` | `หยุดตัวอย่างเสียง` | Stop a sample voice |
| `audio_sample_duration(id)` | `采样时长` | `サンプル長さ` | `샘플길이` | `ความยาวตัวอย่างเสียง` | Length of a sample |
| `audio_fx_delay(time, feedback, mix)` | `回声` | `ディレイ効果` | `딜레이` | `เสียงสะท้อน` | Master echo |
| `audio_fx_reverb(mix)` | `混响` | `リバーブ` | `리버브` | `เสียงก้อง` | Master reverb |
| `audio_fx_lowpass(cutoff)` | `低通滤波` | `ローパス` | `저역통과` | `กรองความถี่ต่ำ` | Master low-pass filter |
| `fft_push(samples)` | `频谱输入` | `FFT入力` | `FFT입력` | `วิเคราะห์เสียง` | Feed samples to the analyser |
| `fft_bands(n)` | `频段` | `周波数帯` | `주파수대` | `แถบความถี่` | Log-spaced frequency bands |
| `fft_beat()` | `节拍检测` | `ビート検出` | `비트` | `จังหวะเสียง` | Is there a beat now? |
| `fft_beat_ratio()` | `节拍比` | `ビート比` | `비트비율` | `อัตราจังหวะ` | Beat strength |
| `fft_rms()` | `均方根` | `二乗平均` | `RMS레벨` | `ระดับRMS` | Overall loudness |
| `fft_dominant_freq()` | `主频` | `主要周波数` | `주파수` | `ความถี่หลัก` | Strongest frequency in Hz |

## How the engine mixes

One output stream carries four kinds of sound:

- **Tones**: long-lived oscillators in numbered slots, placed in space (`audio_tone`).
- **One-shots**: blips (`audio_blip`, `ui_sound`), positional effects (`audio_sfx`) and morph notes (`morph_note`).
- **Samples**: decoded files played in 3D or flat (`audio_sample_*`).
- **Background music**: one looping WAV (`audio_bgm`).

The listener is the camera. Pan is equal-power left/right from the sound's position relative to the camera, and level falls off with distance. `set_camera_pos` moves the listener; `audio_listener` turns it. The final mix is soft-clipped, so loud mixes distort gently instead of clipping hard.

Tone slots grow on demand. There are 32 blip voices, 64 effect voices, 32 morph voices and 64 sample voices; when a pool is full, the oldest voice is dropped.

## Volume and background music

### `audio_volume(level)`

Master gain for everything this engine mixes (default 0.7; 1.0 is unity). It does not affect the music engine, which has its own `music_volume`.

### `audio_bgm(path, volume)` and `audio_bgm_volume(level)`

`audio_bgm` loads a **WAV** file and loops it as background music (default volume 0.5). A second call replaces the track. If the file can't be read, an error goes to standard error and the program continues in silence. The path is used exactly as given. For MP3, OGG, FLAC or AAC, use `audio_sample_load` or `music_load`.

`audio_bgm_volume` changes the level without reloading.

```ling-gui
bind start = do {
    audio_volume(0.6)
    audio_bgm("theme.wav", 0.4)
    audio_bgm_volume(0.15)
}
```

## Tones

### `audio_tone(slot, x, y, z, w, freq, amp, lfo_rate, lfo_depth, wave, duty)`

Creates or retunes a continuous oscillator. Reusing a slot updates that tone in place, and frequency and amplitude glide to their new values, so you can call it every frame.

| Parameter | Default | Meaning |
|---|---|---|
| `slot` | 0 | Tone slot number. |
| `x, y, z` | 0 | World position of the source. |
| `w` | 1 | Fourth-dimension value. It drives a slow sub-oscillator that cross-modulates the tone; 0 disables it. |
| `freq` | 220 | Frequency in Hz. |
| `amp` | 0.15 | Amplitude. Pass 0 to silence the slot (there is no delete). |
| `lfo_rate` | 0.5 | Vibrato speed in Hz. |
| `lfo_depth` | 0.02 | Vibrato depth as a fraction of `freq`. |
| `wave` | `"sine"` | `"sine"`, `"square"`, `"saw"`, `"triangle"` or `"noise"` (stepped noise pitched at `freq`). |
| `duty` | 0.5 | Square pulse width, for example 0.125, 0.25, 0.5, 0.75 for chiptune voices. |

### `audio_glide(amp_ms, freq_ms)`

Sets how long tone amplitude and frequency take to glide to a new value, in milliseconds (0 or less keeps the default). A chiptune player that updates tones once per frame sounds smoother at about 10 ms.

### `audio_listener(cos_yaw, sin_yaw, cos_pitch, sin_pitch)`

Turns the listener. Pass the same four numbers you give `set_camera` (defaults 1, 0, 1, 0). Call it each frame when the camera turns.

```ling-gui
bind start = do {
    open_window(640, 360, "Tones")
    audio_tone(0, 0, 0, 6, 0, 261.63, 0.10, 0.5, 0.01)
    audio_tone(1, 0, 0, 6, 0, 440.0, 0.06, 0, 0, "square", 0.25)
    bind yaw = 0.0
    while window_is_open() {
        bind yaw = yaw + 0.01
        set_camera(cos(yaw), sin(yaw), 1, 0)
        audio_listener(cos(yaw), sin(yaw), 1, 0)
        fill(10, 12, 28)
        present()
    }
}
```

## One-shot sounds

### `audio_blip(freq, duration, wave, amp)`

A short, non-positional blip with a fast attack and decay. Defaults: `freq` 660 Hz, `duration` 0.08 s (minimum 0.01), `wave` `"sine"`, `amp` 0.25. Wave names: `"sine"`, `"square"` (`"sq"`), `"saw"` (`"sawtooth"`), `"tri"` (`"triangle"`), `"noise"`. Unknown names play a sine.

### `ui_sound(name)`

One of the built-in interface sounds: `"click"` (default), `"hover"`, `"confirm"`, `"error"`, `"toggle"`, `"tick"`. Any other name plays `"click"`.

### `audio_sfx(x, y, z, w, freq, amp, duration, wave)`

A synthesised one-shot at a world position, panned and attenuated like a tone. Defaults: position 0, `w` 1, `freq` 440, `amp` 0.3, `duration` 0.15, `wave` `"sine"`.

### `morph_note(x, y, z, w, freq, amp, duration, material, morph)`

A "yin-yang" note that blends a physical model with a digital one. `material` is 0 bowed string, 1 plucked, 2 blown, 3 struck metal. `morph` goes from 0 (light, acoustic) to 1 (dark, digital: FM, bit crush, drive). Defaults: `freq` 220, `amp` 0.3, `duration` 0.6.

### `audio_stop_sfx()`

Stops every one-shot effect, morph note and sample voice at once, so tails don't leak across a scene change. Tones, background music and the music engine keep playing.

```ling-gui
bind start = do {
    ui_sound("confirm")
    audio_blip(880, 0.06, "square", 0.2)
    audio_sfx(1.5, 0, 4, 1, 330, 0.3, 0.2, "triangle")
    morph_note(0, 0, 5, 1, 220, 0.3, 1.2, 1, 0.4)
}
```

## Samples

### `audio_sample_load(path)`

Decodes a WAV, FLAC, OGG, MP3 or AAC file into memory, mixed down to mono. If the path doesn't exist as given, it is tried relative to the script's folder. Returns a sample id (from 0), or `-1` on failure or when there is no audio device.

### `audio_sample_play(id, x, y, z, w, volume, loop, pitch)`

Plays a loaded sample at a world position, panned and attenuated relative to the listener. `volume` defaults to 1, `loop` above 0.5 loops, and `pitch` is a playback-rate multiplier (2 is an octave up, 0.5 an octave down). `w` has no audible effect on samples. Returns a voice id, or 0 on failure. In the browser build the signature is `(id, x, y, z, w, volume, loop)`, without pitch.

### `audio_sample_play_flat(id, volume, loop, pitch)`

Plays a sample with no pan and no distance falloff. Use it for narration, voice-over and interface cues. Returns a voice id, or 0.

### `audio_sample_stop(voice)` and `audio_sample_duration(id)`

`audio_sample_stop` stops a voice returned by a play call. `audio_sample_duration` returns a sample's length in seconds (0 for an unknown id), handy for pacing a cut-scene to the clip that was actually loaded.

```ling-gui
bind start = do {
    bind step = audio_sample_load("step.ogg")
    bind hum = audio_sample_load("hum.wav")
    audio_sample_play(step, 1.0, 0.0, 3.0, 1.0, 0.8, 0, 1.2)
    bind v = audio_sample_play_flat(hum, 0.5, 1, 1.0)
    print(audio_sample_duration(hum))
    audio_sample_stop(v)
}
```

## Master effects

The master chain is delay, then reverb, then a low-pass filter. All three start off or fully open.

| Builtin | Parameters |
|---|---|
| `audio_fx_delay(time, feedback, mix)` | Echo time in seconds (default 0.3), feedback 0 to 0.95 (default 0.3), wet mix 0 to 1 (default 0.3; 0 turns it off). |
| `audio_fx_reverb(mix)` | Reverb wet mix, 0 to 1 (default 0.3). |
| `audio_fx_lowpass(cutoff)` | 0 to 1. 1 is open; lower values muffle the mix, for an underwater feel. The filter glides to the new value. |

```ling-gui
bind start = do {
    audio_fx_delay(0.35, 0.4, 0.25)
    audio_fx_reverb(0.4)
    audio_fx_lowpass(0.15)
}
```

## FFT analysis

The FFT analyser works on samples you give it. Push a block of mono samples (numbers between -1 and 1, at 44.1 kHz) with `fft_push`, then read the results:

| Builtin | Returns |
|---|---|
| `fft_bands(n)` | A list of `n` log-spaced magnitude bands (default 32), bass first. |
| `fft_beat()` | `true` when the latest block is a beat. |
| `fft_beat_ratio()` | Beat strength: 1 is at the detection threshold, above 1 is a strong beat. |
| `fft_rms()` | Root-mean-square level of the signal. |
| `fft_dominant_freq()` | The strongest frequency in Hz. |

This program builds a 1000 Hz sine wave and analyses it. The answer is close to 1000, limited by the analyser's frequency resolution:

```ling
bind start = do {
    bind samples = []
    bind i = 0
    while i < 4096 {
        bind samples = samples.push(sin(i * 6.2831853 * 1000 / 44100))
        bind i = i + 1
    }
    fft_push(samples)
    print(round(fft_dominant_freq()))
    print(round(fft_rms() * 1000) / 1000)
    bind bands = fft_bands(8)
    bind loudest = 0
    bind k = 1
    while k < len(bands) {
        if bands[k] > bands[loudest] {
            bind loudest = k
        }
        bind k = k + 1
    }
    print("loudest band: " + str(loudest))
}
```

```output
991
0.707
loudest band: 4
```

To react to a song that is playing, use `music_fft(track, bands)` on the [Music](music.md) page; for the microphone, `mic_fft` on [Input and Microphone](input.md).
