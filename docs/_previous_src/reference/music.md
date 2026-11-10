# Builtins — Music & Spatial Audio

`ling-music` is a full music layer (separate from the SFX-oriented `ling-audio`): decode any
common format, analyse tempo & key, synthesize GM-style instruments from `.ling` patches, read
MIDI, and build rhythm games & karaoke. `ling-audio` adds positional one-shots, sample
playback/loops, and master FX.

## Playback & analysis
`music_load("song.wav")` → id (WAV/FLAC/OGG/MP3/AAC) · `music_play(id)` / `music_pause` /
`music_stop` / `music_seek(sec)` / `music_pos()` / `music_volume(v)` / `music_duration(id)` ·
`music_bpm(id)` · `music_key(id)` → e.g. `"A minor"` · `music_onsets(id)` · `music_beat_grid(id)` ·
`music_fft(id, nbands)` → spectrum at the current position (drives reactive visuals).

## Synthesis (GM-capable, from `.ling` patches)
`music_patch("instruments/rhodes_ep.ling")` → inst · `music_note(inst, "C4", dur, vel)` ·
`music_note_on(inst, pitch, vel)` / `music_note_off(inst, pitch)`. A patch is a small modular
FM/subtractive voice (operators + ADSR + filter + LFO) able to approximate any GM family.

## MIDI · rhythm · karaoke
`music_midi_load("song.mid")` → id · `music_midi_notes(id)` → `[time,midi,…]` ·
`music_midi_bars(id)` → `[time,midi,dur,…]` (karaoke note bars) · `music_midi_count(id)`.
`music_judge(delta_ms)` → grade · `music_grade_name(g)`. `music_lrc("song.lrc")` /
`music_lyric(id,t)`. `music_mic_pitch()` → Hz (sung pitch) · `music_note_name(hz)` ·
`music_hz(midi)` · `music_pitch_score(hz, target_hz)`.

## Spatial audio (ling-audio)
`audio_tone(idx,x,y,z,w,freq,amp,lfo_rate,lfo_depth)` (continuous, 3D/4D) ·
`audio_sfx(x,y,z,w,freq,amp,dur,wave)` (positional one-shot) ·
`audio_sample_load(path)` / `audio_sample_play(id,x,y,z,w,vol,loop)` / `audio_sample_stop(v)` ·
`audio_listener(cry,sry,crx,srx)` (match the camera) · `audio_volume(v)` ·
master FX: `audio_fx_delay(time,fb,mix)` · `audio_fx_reverb(mix)` · `audio_fx_lowpass(cutoff)`
(1.0 = open, lower = muffled/underwater).

See `examples/music/{synth_jam,rhythm_game,karaoke_hero,hallway_runner}.ling`. Full
implemented-vs-missing list: [`docs/AUDIO_MUSIC_TODO.md`](https://github.com/taellinglin/ling/blob/master/docs/AUDIO_MUSIC_TODO.md).
