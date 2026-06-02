# Hyperbolic 3D — an H³ mandala temple

`examples/hyperbolic3d.ling` is a fully 3-dimensional hyperbolic space walker,
implementing the **hyperboloid model** (Minkowski space) with **Lorentz boosts**
for movement. It's the full H³ cousin to the earlier [H²×ℝ Hyperbolic World](./hyperbolic-world.md).

```
ling run examples/hyperbolic3d.ling
```

## The geometry

**Hyperboloid model**: points `(px, py, pz, pw)` satisfy `-pw² + px² + py² + pz² = -1` with `pw > 0`.

Movement happens via **Lorentz boosts**, the hyperbolic equivalent of Euclidean translations:
- In direction `(dx, 0, dz)` by rapidity `v`:
  - `dot = dx*px + dz*pz`
  - `ch = cosh(v)`, `sh = sinh(v)`
  - `nx = px + (ch-1)*dx*dot + sh*dx*pw` (and similarly for z, w)
  - `ny = py` (unchanged; boost stays in the XZ plane)

The player always stays at the H³ origin (`pw=1, px=py=pz=0`). Each frame, an **inverse boost** is applied to all room positions (`sh → -sh`), so the world translates around the player — same pattern as the 2D Möbius demo.

Rendering uses the **Poincaré ball projection**:
- `bx = px/(1+pw)`, `by = py/(1+pw)`, `bz = pz/(1+pw)` (normalized ball coords)
- **Conformal factor**: `conf = 1 - (bx²+by²+bz²)` — objects shrink toward the horizon
- Rooms beyond `conf < 0.03` are culled; distant rooms (`conf < 0.5`) render wireframe-only

## The temple

A **9-room mandala** floats in H³ space:
- **1 centre room** at the H³ origin
- **8 satellite rooms** along axial and diagonal directions at hyperbolic distance 1.2
  - 4 axial: ±X, ±Z axes
  - 4 diagonal: ±(X, Z) at 45° angles

Each room has:
- **Floor and ceiling** slabs (scale by conformal factor, drawn with LOD)
- **4 walls** (only when conf > 0.15)
- **Arched doorways** on each wall (conf > 0.2)
- **Central pedestal** with a spinning **Platonic dice** (tetrahedron, octahedron, icosahedron, dodecahedron, or cube; randomly selected by room type)
- **Alchemy symbol** (`vtex_yantra`) on the back wall

Colors are **procedurally hashed** from each room's type index, cycling smoothly via sine waves.

## Controls

| Key | Action |
|-----|--------|
| `W` / `S` | move forward / back (very slow hyperbolic) |
| `A` / `D` | strafe left / right (very slow hyperbolic) |
| `Q` / `E` | turn left / right (Euclidean yaw) |
| `SPACE` | charged jump (hold longer for higher jump; releases on key-up) |

Movement in the XZ plane is **slowly hyperbolic** (rapidity 0.008 per frame, about 7× slower than earlier
versions). This deliberate slowness reveals the subtle geometry of hyperbolic space as you navigate.
Vertical motion (Y) uses **Euclidean gravity** — the ball bounces with elasticity (0.86) and damping (0.995).
Jumping feels natural even in curved space.

**The Player**: a large physics ball (radius 1.6) rendered as a spinning gyro. The ball's size
makes the H³ temple geometry feel monumental around you. Color intensity modulates with kinetic energy
(brighter when fast, dimmer at rest).

## What makes it hyperbolic

- **Exponential crowding**: as you move hyperbolically, rooms recede toward the horizon,
  shrinking by the conformal factor. The space curves away faster than Euclidean distance.
- **Lorentz locomotion**: movement is a boost, not a translation. The math encodes
  relativistic spacetime geometry.
- **Holonomy**: walk a "closed loop" in H³ and you come back rotated — the signature
  effect of negative curvature.
- **No preferred direction**: boosts can point any direction in the XZ plane; unlike the 2D
  demo, you have full 2D freedom on the horizontal plane (plus ordinary vertical gravity).

## Physics-Encoded Audio

The ball emits a continuous tone that **encodes its physics state**:
- **Pitch** ← height (low when near floor, rises toward ceiling) + speed + impact spikes on bounce
- **Amplitude** ← speed (faster ball = louder tone; resting ball = quiet)
- **Pan** ← horizontal X position (ball on left → left speaker, right → right speaker)

This creates an immersive **physical feedback loop**: you hear the geometry and motion encoded as sound.
A bounce sounds like a sharp frequency spike; accelerating sounds like a rising tone; quiet zones in the
temple feel acoustically dead.

## Lighting & appearance

The scene uses **cel shading** (smooth vertices → posterized bands, no holographic sheen):
- **Warm overhead light** (yellow, slowly drifting around the scene)
- **Cool fill** from the front-left (cyan, wide area light)
- **Purple ball-tracking light** (follows the physics ball, providing specular highlight as it moves)
- **Crisp cel bands** (5 posterization levels for clean, flat toon appearance)
- **Coloured shadows** (deep indigo in unlit regions)
- **Subtle purple rim** (small Fresnel edge highlight, toned down for clean look)

Each room's color hue is **independently procedural** — no texture assets, just per-room
hash-based RGB cycling.

## Implementation notes

- **No list_set**: room positions are rebuilt every frame using parallel lists (`rx`, `ry`,
  `rz`, `rw`) and `list_push` (the proven pattern from H²×ℝ).
- **Inline Lorentz math**: the boost formula is unrolled inline (no function call) to avoid
  the Ling interpreter's list-return quirk.
- **Normalization**: after each boost, the hyperboloid equation `-nw²+nx²+ny²+nz² = -1` 
  is re-normalized to prevent numerical drift: `n = sqrt(nw²-nx²-ny²-nz²)`, divide all by n.
- **Function restrictions**: helper functions return scalars only (cosh, sinh, hash) using
  direct expressions (no local variables).

## Extending the demo

- **More rooms**: duplicate the room initialization code and adjust HDIST
  (hyperbolic distance), or place rooms at computed positions via additional boosts.
- **Collision detection**: add sphere-sphere checks in H³ metric (Lorentz norm).
- **Non-Euclidean gravity**: replace vertical motion with a Lorentz boost toward a
  floor hyperplane in H³ (more exotic physics).
- **Tessellation**: use a honeycomb lattice or other hyperbolic tiling scheme instead
  of hand-placed rooms.

---

For a simpler introduction to hyperbolic geometry in Ling, see [Hyperbolic World](./hyperbolic-world.md) (H²×ℝ).
