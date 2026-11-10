# Garden

Source: [`examples/audiovisual/Garden.ling`](https://github.com/taellinglin/ling/blob/master/examples/audiovisual/Garden.ling)

An open outdoor garden: ground, sky, a pond in the centre, two trees in front, flower
beds, a tree line behind and a wooden shed. There are no walls or ceiling. All of it is
drawn with vector geometry (`vtex_*`), and each kind of shape has its own tone, so the
scene is something you hear as well as see.

Run it from the repository root:

```sh
ling run examples/audiovisual/Garden.ling
```

## The scene

| Part | Function in the file |
|---|---|
| Grass, stone path and flower-bed patches | `draw_ground` |
| Sun disc, drifting clouds and horizon glow | `draw_sky` |
| Trees with spiral trunks and flower crowns | `draw_tree`, `draw_trees` |
| Scattered flower beds | `draw_flowers` |
| Pond with floating lotus, a fountain and ripples | `draw_pond` |
| Wooden shed with a door, window and roof | `draw_shed` |
| Two faint "echo" copies of the scene from earlier frames | `draw_echo` |

The shapes come from `vtex_flower`, `vtex_lotus`, `vtex_chakra`, `vtex_rings`,
`vtex_grid`, `vtex_spiral`, `vtex_star`, `vtex_yantra`, `vtex_hyperbolic_uv` and
`vtex_tessellated`. A small hash function, `sm2_hash`, gives each ground patch and shed
plank its own amount of detail.

## Sound

Six tone voices use a G major pentatonic scale (G A B D E). Each voice is placed at its
own point in the scene with `audio_tone`, around the pond, the shed and the sky, and
`audio_listener` follows the camera.

## Camera

The camera floats on a slow, wandering path around the pond and always looks at the
centre of the garden. Colours cycle through the whole spectrum about every two seconds.

## Ideas to try

- Change `orbit_h` and `orbit_v` to move the camera closer or further away.
- Move the pond with `PX` and `PZ`, or the shed with `SX` and `SZ`.
- Change the pentatonic frequencies `PG3` to `PE4` to give the garden a new key.
