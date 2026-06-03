# Hyperbolic 3D DOOM — Wireframe Maze in H³

`examples/physics/hyperbolic3d.ling` is a playable symmetrical maze in **hyperbolic 3D space** using
the **hyperboloid model** (Minkowski geometry). Navigate wireframe hallways and alcove rooms
connected by prominent archways. Roll a physics ball through a "hyperbolic doom" temple.

It's the game-like cousin to the earlier [H²×ℝ Hyperbolic World](./hyperbolic-world.md).

```
ling run examples/physics/hyperbolic3d.ling
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

## The Maze

A **symmetrical 8-spoke maze** in H³ hyperbolic space:
- **1 central atrium** at the H³ origin (mandala floor pattern with concentric hexagons)
- **8 radial spokes** extending outward (cardinal N/S/E/W + diagonals NE/NW/SE/SW)
- **Hallways** connecting center to alcove rooms (wireframe, proportional scaling)
- **Alcove rooms** at the end of each spoke (small wireframe chambers)
- **Archways** (FILLED, cyan/blue) serving as doorways into each alcove

Visual style:
- **Wireframe architecture** (clean, minimal, shows structure clearly)
- **Ground plane** at y=0.2 (the walkable floor)
- **Archways only** are rendered filled (prominent, visible doorways)
- **Decorative mandala circles** in each alcove (concentric rings)

Colors are **procedurally hashed** per-spoke, shifting via sine waves with frame time.

## Controls

| Key | Action |
|-----|--------|
| `W` / `S` | move forward / back (FASTER hyperbolic) |
| `A` / `D` | strafe left / right (FASTER hyperbolic) |
| `Q` / `E` | turn left / right (Euclidean yaw) |
| `SPACE` | charged jump (hold to build impulse; release to jump) |

Movement in the XZ plane is **hyperbolic with moderate speed** (rapidity 0.025 per frame).
Fast enough to navigate the maze efficiently, yet slow enough to appreciate the hyperbolic geometry.
Vertical motion (Y) uses **Euclidean gravity** — the ball bounces with elasticity (0.86) and realistic damping (0.995).

**The Player**: a large physics ball (radius 1.4) rendered as a rotating gyro (spinning rings).
The ball morphs colors based on speed: **cyan when slow, red when fast** (energy indicator).
Velocity arrow (cyan line) shows momentum direction.

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

The ball emits a continuous drone tone that **encodes its physics state in real time**:
- **Pitch** ← ball height (low near floor, rises as you jump/climb) + speed + impact spikes on bounces
- **Amplitude** ← kinetic energy (faster = louder; resting = quiet)
- **Pan** ← horizontal X position for **spatial audio** (left/right speaker follows ball)

Result: sound becomes **physical feedback**. Bounces produce frequency spikes. Acceleration is audible.
Silence indicates you're at rest. The audio landscape reflects the maze geometry.

## Lighting & appearance

**Cel shading** (smooth vertices → posterized bands):
- **Warm overhead drift light** (yellow, slowly orbits the scene)
- **Cool ambient fill** (cyan, subtle background light)
- **Purple ball-tracking light** (follows player position, provides dynamic highlight)
- **4-level posterization** (crisp, flat toon bands for clean maze readability)
- **Deep indigo shadows** (unlit areas tinted cool for atmosphere)
- **Minimal rim light** (subtle edge highlight, doesn't wash out geometry)

The wireframe + cel shading combination creates a **clean, architectural aesthetic**—like a blueprint
that's alive and animated. Archways pop out (filled, brightly lit). Hallways recede (wireframe, subtle).

Each room's color hue is **independently procedural** — no texture assets, just per-room
hash-based RGB cycling.

## Implementation notes

- **H³ geometry**: 9 rooms (1 center + 8 spokes) positioned via Lorentz boosts at distance ~1.5
- **Wireframe drawing**: custom `draw_wireframe_box()` function renders hallway/room geometry with 12 line calls per box
- **Archways**: only filled geometry; everything else is wireframe (visual clarity)
- **Lorentz transforms**: inlined scalar arithmetic (no functions that return lists) to avoid interpreter quirks
- **Hyperboloid normalization**: after each boost, `n = sqrt(nw²-nx²-ny²-nz²)` re-normalizes to prevent drift
- **Conformal scaling**: all maze geometry scales by `conf = 1 - (bx²+by²+bz²)`, making distant rooms shrink
- **Physics**: ball bounces elastically (0.86) with air damping (0.995), collision with boundary box
- **List pattern**: room coordinates rebuilt each frame using `list_push` (no list_set in Ling)

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
