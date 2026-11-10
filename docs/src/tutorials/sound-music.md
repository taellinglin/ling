# Sound and Music

Ling can make sound from nothing but numbers. On this page you turn notes into
frequencies, play short tones from the keyboard, hold a tone that follows the mouse,
play sound files, write a melody as two lists, and render a song to a WAV file.

Programs that play sound need speakers or headphones. Most of them also open a small
window so they keep running while you listen, which is why their code blocks have no
output. The ones that only compute and print are checked like any other example.

## Notes are numbers

The pitch of a sound is its **frequency**, in hertz (Hz). Concert A is 440 Hz, and
doubling a frequency raises the note by one **octave**.

Musicians name notes ("C4", "A#3"); computers often number them with **MIDI numbers**:
middle C is 60, each step up is one piano key, and A4 is 69. Two builtins convert
between the two:

- `music_hz(pitch)` turns a MIDI number, or a name such as `"A4"`, into hertz.
- `music_note_name(hz)` gives the name of the nearest note.

```ling
bind start = do {
    print("A4 is ", music_hz("A4"), " Hz")
    print("middle C (60) is ", round(music_hz(60) * 100) / 100, " Hz")
    print("440 Hz is ", music_note_name(440))
    print("300 Hz is nearest to ", music_note_name(300))
    bind scale = [60, 62, 64, 65, 67, 69, 71, 72]
    for m in scale {
        print(m, "  ", music_note_name(music_hz(m)), "  ", round(music_hz(m)), " Hz")
    }
}
```

```output
A4 is 440 Hz
middle C (60) is 261.63 Hz
440 Hz is A4
300 Hz is nearest to D4
60  C4  262 Hz
62  D4  294 Hz
64  E4  330 Hz
65  F4  349 Hz
67  G4  392 Hz
69  A4  440 Hz
71  B4  494 Hz
72  C5  523 Hz
```

Those eight numbers are the white keys from middle C to the next C: a C major scale.

## Play a tone

`audio_blip(freq, seconds, wave, volume)` plays one short sound and returns at once.

```text
audio_blip(440, 0.5, "triangle", 0.3)     # A4 for half a second, softly
```

The **waveform** sets the character of the sound:

| Wave | Sounds like |
|---|---|
| `"sine"` | pure and smooth, like a whistle |
| `"triangle"` | soft and hollow |
| `"square"` | buzzy, like an old video game |
| `"saw"` | bright and harsh |
| `"noise"` | hiss, good for drums and wind |

An unknown name falls back to sine. For ready-made interface sounds there is also
`ui_sound(name)` with `"click"`, `"hover"`, `"confirm"`, `"error"`, `"toggle"` and `"tick"`.

### A keyboard piano

Combine `audio_blip` with keyboard input. The keys A to K play a scale:

```ling-gui
bind start = do {
    open_window(640, 200, "Keyboard piano")
    bind keys = ["a", "s", "d", "f", "g", "h", "j", "k"]
    bind notes = [60, 62, 64, 65, 67, 69, 71, 72]
    bind lit = -1
    while window_is_open() {
        bind i = 0
        while i < len(keys) {
            if key_pressed(keys[i]) {
                audio_blip(music_hz(notes[i]), 0.5, "triangle", 0.3)
                bind lit = i
            }
            bind i = i + 1
        }
        fill(20, 20, 35)
        bind i = 0
        while i < len(keys) {
            if i == lit {
                set_color(255, 200, 80)
            } else {
                set_color(240, 240, 240)
            }
            triangle(20 + i * 75, 40, 90 + i * 75, 40, 55 + i * 75, 170)
            bind i = i + 1
        }
        present()
    }
}
```

`key_pressed` is true for exactly one frame per press, so each press plays exactly one note.

### A tone that follows the mouse

`audio_tone` makes a **continuous** sound that keeps playing until you silence it:

```text
audio_tone(slot, x, y, z, w, freq, amp, lfo_rate, lfo_depth)
```

- `slot` numbers the tone. Calling again with the same slot retunes that tone instead of
  starting a new one, so you can call it every frame.
- `x, y, z` place the sound relative to the camera; `z = 3` is straight ahead. `w` is an
  extra timbre parameter; 0 turns it off.
- `freq` is the pitch in hertz and `amp` the loudness.
- `lfo_rate` and `lfo_depth` add vibrato.

```ling-gui
bind start = do {
    open_window(640, 400, "Theremin")
    audio_volume(0.5)
    while window_is_open() {
        bind freq = 110 * pow(2, mouse_x() / 640 * 3)
        bind loud = (1 - mouse_y() / 400) * 0.25
        audio_tone(0, 0, 0, 3, 0, freq, loud, 5, 0.01)
        fill(10, 10, 30)
        set_color(120, 220, 255)
        draw_disc(mouse_x(), mouse_y(), 12 + loud * 80)
        present()
    }
    audio_tone(0, 0, 0, 3, 0, 220, 0, 5, 0.01)
}
```

From the left edge to the right edge the pitch climbs three octaves, from 110 Hz to 880 Hz.
A tone can't be deleted, so the last line sets its loudness to 0. `audio_volume(level)` sets
the master volume; keep it low while you experiment.

## Play sound files

Ling reads WAV, MP3, OGG, FLAC and AAC files. There are three ways to play one:

| Builtin | Use it for |
|---|---|
| `music_load(path)` then `music_play(track)` | a song: one track at a time, with `music_pause()`, `music_stop()` and `music_pos()` |
| `audio_bgm(path, volume)` | looping background music (WAV only) |
| `audio_sample_load(path)` then `audio_sample_play_flat(id, volume, loop, pitch)` | short sound effects you play many times |

```ling-gui
bind start = do {
    bind song = music_load("song.ogg")
    bind hit = audio_sample_load("hit.wav")
    music_play(song)
    open_window(400, 200, "Player")
    while window_is_open() {
        if key_pressed("space") {
            audio_sample_play_flat(hit, 1.0, 0, 1.0)
        }
        fill(15, 25, 40)
        set_color(120, 255, 190)
        ui_text(20, 80, 20, "TIME " + to_str(round(music_pos())))
        present()
    }
}
```

`music_load` returns `-1` if the file can't be read. `music_play` always starts the track
from the beginning, so don't call it every frame. To resume after `music_pause()`, call
`music_play(-1)`.

## A melody as two lists

A melody is a list of pitches and a matching list of durations. Durations are in **beats**,
so the same tune plays faster or slower when you change the tempo. This player uses
`time_now()` to start each note on time; `0` in the pitch list is a rest.

```ling-gui
bind start = do {
    bind pitches = [64, 64, 65, 67, 67, 65, 64, 62, 60, 60, 62, 64, 64, 62, 62, 0]
    bind beats = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1.5, 0.5, 2, 1]
    bind tempo = 132
    bind spb = 60 / tempo              # seconds per beat
    open_window(400, 200, "Ode to Joy")
    bind begun = time_now()
    bind next_at = 0
    bind index = 0
    while window_is_open() {
        bind now = time_now() - begun
        if index < len(pitches) && now >= next_at {
            bind d = beats[index] * spb
            if pitches[index] > 0 {
                audio_blip(music_hz(pitches[index]), d * 0.9, "triangle", 0.3)
            }
            bind next_at = next_at + d
            bind index = index + 1
        }
        if index >= len(pitches) && now > next_at + 0.5 {
            quit()
        }
        fill(25, 15, 45)
        present()
    }
    print("Played ", index, " notes")
}
```

Whenever the clock reaches `next_at`, the player plays the next note and moves `next_at`
forward by that note's length. The rhythm comes from the clock, not from the frame rate.
`quit()` closes the window half a second after the last note.

## Render a song to a file

Ling includes a small music studio (a DAW) that you script with `daw_*` builtins. You build
a project with tracks, put clips on the tracks and notes in the clips, then **bounce** it to
an ordinary WAV file. It all happens in memory, so it works without a sound card.

```ling
bind start = do {
    daw_new()
    daw_set_bpm(120)
    bind lead = daw_track_add("synth")
    bind clip = daw_clip_add_midi(lead, 0, 16)

    bind pitches = [60, 60, 67, 67, 69, 69, 67, 65, 65, 64, 64, 62, 62, 60]
    bind beats = [1, 1, 1, 1, 1, 1, 2, 1, 1, 1, 1, 1, 1, 2]
    bind when = 0
    bind i = 0
    while i < len(pitches) {
        daw_note_add(lead, clip, when, beats[i] * 0.9, pitches[i], 0.8)
        bind when = when + beats[i]
        bind i = i + 1
    }
    print("song length: ", when, " beats")

    bind ok = daw_bounce("twinkle.wav", 0, when, 0.5, 16, "wav", 0.5)
    print("rendered: ", ok)
    bind song = music_load("twinkle.wav")
    print("about ", round(music_duration(song)), " seconds")
}
```

```output
song length: 16 beats
rendered: 1
about 9 seconds
```

1. `daw_new()` starts an empty project and `daw_set_bpm(120)` sets the tempo.
2. `daw_track_add("synth")` adds a synthesizer track. `"drums"` is another kind: on a drum
   track, note 36 is the kick, 38 the snare and 42 the closed hi-hat.
3. `daw_clip_add_midi(track, start, length)` adds an empty clip, measured in beats.
4. `daw_note_add(track, clip, start, length, pitch, velocity)` adds one note. Velocity is
   how hard the key is hit, from 0 to 1.
5. `daw_bounce(path, from, to, tail, bits, format, quality)` renders the beats `from` to `to`
   plus a `tail` in seconds, and returns `1` on success.

The file `twinkle.wav` appears in the folder where you ran the program. Play it with
`music_load` and `music_play`.

## Try it

1. Print the frequency of every MIDI note from 60 to 72. What is the ratio between the
   first and the last?
2. In the keyboard piano, make the key `"z"` play a drum-like hit:
   `audio_blip(120, 0.15, "noise", 0.4)`.
3. Change the piano's notes to an A minor pentatonic scale: `[57, 60, 62, 64, 67, 69, 72, 74]`.
4. Put your own tune in the melody player. "Happy Birthday" starts with 60, 60, 62, 60, 65, 64.
5. Render a drum loop: a `"drums"` track with a kick (36) on every beat and a snare (38)
   on beats 2 and 4, bounced to `drums.wav`.

Next: [A Small Web Service](web-service.md). For every sound builtin, see
[Audio](../reference/audio.md) and [Music](../reference/music.md).
