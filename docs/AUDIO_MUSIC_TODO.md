# Audio / Music — implemented vs. missing

Tracker for `ling-audio` + `ling-music` capabilities and their `.ling` builtins
(all builtins are wired in 5 languages: en / zh / ja / ko / th). Update as
features land.

## ✅ Implemented

### ling-audio (`crates/ling-audio`)
- Positional **continuous tones** (3D + 4D `w` cross-mod): `audio_tone`, `audio_listener`, `audio_volume`
- WAV **BGM** loop: `audio_bgm`, `audio_bgm_volume`
- One-shot **UI blips** + waveforms: `audio_blip`, `ui_sound`
- **Positional one-shot SFX** (2D/3D/4D, ADSR-ish, sine/saw/square/tri/noise): `audio_sfx`
- **Sample playback** — load any decodable file, play at a world position, looping or one-shot: `audio_sample_load`, `audio_sample_play`, `audio_sample_stop`
- **Master FX**: feedback delay `audio_fx_delay`, Schroeder reverb `audio_fx_reverb`, resonant low-pass for muffled/**underwater** `audio_fx_lowpass`
- Real-time **FFT**: `fft_push`/`fft_bands`/`fft_beat`/`fft_rms`/`fft_dominant_freq`, plus `music_fft` (FFT of the playing track at the current position)

### ling-music (`crates/ling-music`)
- **Decode** WAV/FLAC/OGG/MP3/AAC (symphonia): `music_load`, `music_duration`
- **Analysis**: `music_bpm`, `music_key`, `music_onsets`, `music_beat_grid`
- **Playback**: `music_play`/`pause`/`stop`/`seek`/`pos`/`volume`
- **GM-capable synth** from `.ling` patches: `music_patch`, `music_note`, `music_note_on`, `music_note_off`
- **Rhythm**: `music_judge`, `music_grade_name`
- **Karaoke**: `music_lrc`, `music_lyric`, `music_mic_pitch`, `music_pitch_score`, `music_note_name`, `music_hz`
- **MIDI** load + note events: `music_midi_load`, `music_midi_count`, `music_midi_notes`

## 🚧 Missing / future

### ling-audio
- Per-source filters / EQ (only a master low-pass exists)
- Doppler shift for moving sources; velocity-aware panning
- True HRTF / binaural spatialization (currently equal-power pan + distance only)
- Sidechain / ducking (e.g. duck music under SFX)
- Convolution reverb (IR-based); per-send reverb/delay buses (currently master-only)
- Pitch/rate control on sample voices (samples play at native pitch only)
- Streaming decode for very large files (whole file is decoded into RAM today)
- Recording/bounce of the master bus to WAV
- Crossfades / fade-in-out helpers

### ling-music
- Tempo/beat **tracking that follows live input** (BPM is offline-only)
- Polyphonic pitch detection (karaoke pitch is monophonic)
- MIDI **output** / virtual instrument routing; SoundFont (`.sf2`) playback
- Score/sequencer object (notes scheduled in time) — today notes are one-shots
- Time-stretch / pitch-shift; key-aware transpose
- Stems / multi-track mixing; loop regions with markers
- Chord detection; structural (verse/chorus) segmentation

### Keywords / language
- No dedicated audio **keywords** (all features are builtins); a `sound { … }`
  block or `on beat { … }` event sugar could be added to the lexer/parser later.
- `music_play_score(...)` (sequence playback) and `audio_bus(...)` (FX sends) are
  unimplemented builtin names reserved for the items above.
