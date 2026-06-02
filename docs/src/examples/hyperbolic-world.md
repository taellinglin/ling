# Hyperbolic World — an H²×ℝ block walker

`examples/hyperbolic_world.ling` is a tiny game world inspired by
[Hypermine](https://github.com/Ralith/hypermine). Hypermine is set in **H³**
(fully 3-D hyperbolic space); this demo uses the simplest playable cousin,
**H²×ℝ**: you walk on the **hyperbolic plane** (the Poincaré disk) with ordinary
vertical **gravity**.

```
ling run examples/hyperbolic_world.ling
```

## Controls

| Key | Action |
|-----|--------|
| `W` / `S` | walk forward / back |
| `A` / `D` | strafe left / right |
| `Q` / `E` | turn left / right |
| `SPACE` | jump (gravity pulls you back down) |

## What makes it hyperbolic

- **Exponential crowding.** Blocks are placed on a lattice of growing rings;
  because the plane is hyperbolic, ever more cells pile up toward the horizon
  (the disk boundary = infinity) and shrink via the Poincaré conformal factor
  `1 − |z|²`.
- **Möbius locomotion.** You stay at the disk centre and the whole world slides
  under you by a **Möbius translation** `z ↦ (z + a)/(1 + ā z)`. Turning is a
  rotation about the centre.
- **Holonomy.** Walk a closed loop and you come back *rotated* — the signature
  effect of moving through negatively-curved space. (Try strafing in a square.)
- **Gravity** acts on the ordinary vertical axis (the ℝ factor), so jumping and
  falling feel completely normal even though the ground is hyperbolic.

## Procedural block "textures"

Each cell hashes its lattice id (`fract(sin(...))`) into a block **type**
(grass / stone / dirt / crystal) and a **height**, so the world is generated
without any texture assets — the colour *is* the procedural texture, shaded with
the [holographic cel renderer](../reference/shapes.md#rendering--lighting-modes-holographic-cel).

## Notes & ideas

- The blocks are drawn with the `cube` primitive; swap in `prism`, `cylinder`,
  or `icosahedron` for different tilings.
- Increase the ring count or `cells = 6 * ring` for a denser world.
- This is H²×ℝ, not full H³ — a true Hypermine-style H³ voxel engine would
  track an `{p,q,r}` honeycomb and node graph, which is a much larger build.
