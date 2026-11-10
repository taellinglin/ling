# Hyperbolic 3D

Source: [`examples/physics/hyperbolic3d.ling`](https://github.com/taellinglin/ling/blob/master/examples/physics/hyperbolic3d.ling)

Despite its file name, the current version of this demo is a physics maze in ordinary
(Euclidean) 3D space. You roll a ball through a wireframe maze, bounce off walls and charge
up jumps, while a tone follows the ball's motion. For real hyperbolic geometry, see
[Hyperbolic World](hyperbolic-world.md).

Run it from the repository root:

```sh
ling run examples/physics/hyperbolic3d.ling
```

## Controls

| Key | Action |
|---|---|
| `W` / `S` | Push the ball forward / back |
| `A` / `D` | Push the ball left / right |
| `Space` | Hold to charge a jump, release to jump |
| `←` / `→` | Orbit the camera |
| `↑` / `↓` | Tilt the camera |
| `Z` / `X` | Zoom in / out |

## How it works

- **Physics.** Each frame adds gravity, multiplies the velocity by a damping factor
  (`DAMPING = 0.99`) and moves the ball. When the ball crosses a wall, the program pushes it
  back and reverses its speed, scaled by `ELASTICITY = 0.80`.
- **The maze.** An outer box of 300 × 200 × 300 units and five inner walls. The walls are
  checked as rectangles and drawn as lines with `draw_line_3d`.
- **Jumping.** Holding `Space` on the ground builds up a charge, up to `MAX_CHARGE`.
  Releasing it launches the ball. A small meter at the ball shows the charge.
- **The ball.** Drawn as a spinning `vtex_chakra` wheel whose colour changes with height.
  A shadow cross on the floor, a line down to the floor and a red velocity arrow help you
  see where it is.
- **Sound that follows physics.** One `audio_tone` voice changes every frame: the pitch
  rises with height and speed and jumps briefly on each bounce, the volume grows with
  speed, and the stereo pan follows the ball's x position.
- **Lights.** Three lights: a slowly drifting warm light, a cool fill light, and a purple
  light that follows the ball.

## Ideas to try

- Change `GRAVITY`, `DAMPING` and `ELASTICITY` at the top of the file and feel the difference.
- Add a wall: copy one of the wall collision checks and draw it with matching `draw_line_3d` calls.
- Map another value to sound, for example the charge level to the pitch.
