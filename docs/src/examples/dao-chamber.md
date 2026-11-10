# 道场 — Shrine Room

Source: [`examples/3d-rooms/ling-dao-chamber.ling`](https://github.com/taellinglin/ling/blob/master/examples/3d-rooms/ling-dao-chamber.ling)

A quiet Chinese-Japanese shrine drawn only with vector geometry (`vtex_*`), with no pixel
fills. Everything moves slowly in a gold, jade and crimson palette, and a soft pentatonic
bell chord plays in the background. The program is written mostly with Thai keywords and
builtins, with a few Chinese names mixed in.

Run it from the repository root:

```sh
ling run examples/3d-rooms/ling-dao-chamber.ling
```

## What's in the room

| Part | Built with |
|---|---|
| Torii gate at the far end | `vtex_torii` |
| Five-tier pagoda behind the gate | `vtex_pagoda` |
| Altar mandala: gear ring, dharma wheel, lotus, rings | `vtex_spiked_cog`, `vtex_chakra`, `vtex_lotus`, `vtex_rings` |
| Stone-tile floor with a yantra border | `vtex_grid`, `vtex_yantra` |
| Star lantern on the ceiling | `vtex_star`, `vtex_rings` |
| Ink-brush panels on the side walls | `vtex_letter_rain`, `vtex_lotus` |
| Four corner pillars | `vtex_rings`, `vtex_spiked_cog`, `vtex_lotus` |

## Sound

Five tones form a Chinese pentatonic scale, 宫 商 角 徵 羽 (C4, D4, E4, G4, A4), plus a
faint sub-bass. Each tone is placed in 3D space with `audio_tone`, and `audio_listener`
follows the camera, so the bells shift as the view swings.

## Camera and light

The camera swings like a slow pendulum, about 30 degrees to each side, always facing the
altar. Five lights (`add_light`) give a gold overhead lamp, a warm altar glow, jade and
crimson accents on the pillars, and a lamp at the gate.

## Ideas to try

- Change the hue anchors `GOLD`, `JADE` and `CRIMSON` at the top of the file.
- Speed up the camera by raising `orbit_spd`.
- Change the frequencies in `init_audio` to try a different scale.

See [3D Shapes and Meshes](../reference/shapes.md) and [Audio](../reference/audio.md) for
the builtins used here.
