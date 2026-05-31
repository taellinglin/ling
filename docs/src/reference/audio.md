# Audio Builtins

Audio builtins are available on native targets. On WASM they are no-ops.

## Tone synthesis

```ling
# audio_tone(slot, x, y, z, w,  freq_hz, amplitude, lfo_rate, lfo_depth)
audio_tone(0,  0.0, 0.0, 8.0,  1.5,   261.63, 0.08, 0.10, 0.008)
```

| Param | Description |
|-------|-------------|
| `slot` | Tone slot index (0–15) |
| `x, y, z` | 3D world position of the sound source |
| `w` | Spatial spread / falloff width |
| `freq_hz` | Oscillator frequency in Hz |
| `amplitude` | Base amplitude (0..1) |
| `lfo_rate` | LFO modulation rate in Hz |
| `lfo_depth` | LFO depth (0..1) |

## Volume

```ling
audio_volume(0.5)         # master volume
audio_bgm_volume(0.4)     # background music volume
```

## Background music

```ling
audio_bgm("path/to/file.wav", 0.5)   # load and play WAV BGM
```

## Listener position

```ling
# Must be called each frame to update 3D audio perspective
audio_listener(cos(ry), sin(ry), cos(rx), sin(rx))
```

## Pentatonic reference

Common frequencies for East Asian pentatonic scales:

| Note | 宫 (Gōng) | 商 (Shāng) | 角 (Jué) | 徵 (Zhǐ) | 羽 (Yǔ) |
|------|-----------|------------|---------|---------|---------|
| Octave 3 | C3 130.81 | D3 146.83 | E3 164.81 | G3 196.00 | A3 220.00 |
| Octave 4 | C4 261.63 | D4 293.66 | E4 329.63 | G4 392.00 | A4 440.00 |
| Octave 5 | C5 523.25 | D5 587.33 | E5 659.25 | G5 783.99 | A5 880.00 |

## FFT analysis

```ling
# Push mono samples to the rolling FFT window
令 smp = list_new()
令 k = 0
循 k < 64 {
    令 smp = list_push(smp, sin(k * 0.1))
    令 k = k + 1
}
fft_push(smp)

# Get 32 log-spaced frequency bands (values 0..1)
令 bands = fft_bands(32)

# Beat detection
若 fft_beat() {
    印("beat!")
}
令 ratio = fft_beat_ratio()   # 1.0 = at threshold
令 rms   = fft_rms()
令 freq  = fft_dominant_freq()
```
