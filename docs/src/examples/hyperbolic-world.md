# Hyperbolic World (H²×ℝ)

Source: [`examples/physics/hyperbolic_world.ling`](https://github.com/taellinglin/ling/blob/master/examples/physics/hyperbolic_world.ling)

A small block world where the ground is the **hyperbolic plane**. It is inspired by
[Hypermine](https://github.com/Ralith/hypermine), which is set in full 3D hyperbolic space
(H³). This demo uses its simplest playable cousin, **H²×ℝ**: you walk on the hyperbolic
plane (drawn as a Poincaré disk) and gravity works normally in the vertical direction.

Run it from the repository root:

```sh
ling run examples/physics/hyperbolic_world.ling
```

## Controls

| Key | Action |
|---|---|
| `W` / `S` | Walk forward / back |
| `A` / `D` | Step left / right |
| `Q` / `E` | Turn left / right |
| `Space` | Jump |

## What makes it hyperbolic

- **Crowding toward the edge.** Blocks sit on rings, and each ring has more blocks than
  the one inside it (`cells = 6 * ring`). Toward the edge of the disk, which stands for
  infinity, they pile up and shrink by the factor `1 − |z|²`.
- **Möbius movement.** You stay at the centre of the disk and the world slides under you
  by a Möbius translation, `z ↦ (z + a) / (1 + ā z)`. Turning rotates the world around
  the centre.
- **Holonomy.** Walk in a closed loop, such as a square, and you come back turned. This
  is what moving through negatively curved space feels like.
- **Normal gravity.** Jumping and falling use the ordinary vertical axis, so they feel
  the same as in any game.

## Blocks without textures

Each block hashes its ring and position (`hash01`, a `fract(sin(...))` hash) into a type
(grass, stone, dirt or crystal) and a height. The colour comes from the type, and the
cel-shading settings (`set_shade_mode`, `set_cel_bands`, `set_rim`) do the rest. No
texture files are needed. See [Post-FX and Lighting](../reference/postfx.md).

## Ideas to try

- The blocks are drawn with `cube`. Try `cylinder` or another shape from
  [3D Shapes and Meshes](../reference/shapes.md).
- Raise the ring limit (`ring <= 4`) for a bigger world.
- Change `STEP` and `TURN` to move or turn faster.

For a different kind of 3D demo, see [Hyperbolic 3D](hyperbolic-3d.md).
