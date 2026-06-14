# Moon Pond — an Anima walking demo

A small living world: a grassy field swaying in the wind, a few trees, a flowing
ink pond, and a turning sun & moon. The **KING** is yours to drive; the **QUEEN**
sits by the water, meditating. The point of the demo is the *motion* — walk, run,
idle, sit‑down → meditate, and swim — built from the `ling-animation` (Anima)
drivers.

```
ling run examples/anim/moon_pond/moon_pond.ling
```

### Controls (free-fly)
| Input | Action |
|---|---|
| Mouse | look around |
| `W A S D` | fly relative to where you look |
| `Space` / `Ctrl` | ascend / descend |
| `Shift` | boost |
| `C` | sit / meditate pose while hovering |
| fly high or into the pond | the swim / fly cycle takes over |
| `Esc` | quit |

The anime face **emotes per state** — blink, smile, raised/furrowed brow — driven by
`animations/face.ling`, which rides the **face + skull vector map** (a labelled
landmark topology + FACS-style expression blendshapes) in
`crates/ling-animation/src/face.rs`.

## Layout

```
examples/anim/
├─ face_demo/
│  └─ face_demo.ling      # detailed 2-D anime face; press 1-9 for emotions
├─ moon_pond/
│  └─ moon_pond.ling      # the world + camera + procedural characters
├─ animations/            # one clip per file — pure motion, no rendering
│  ├─ idle.ling           # breathing sway
│  ├─ walk.ling           # gait-driven walk cycle
│  ├─ run.ling            # faster, longer-stride run
│  ├─ sit.ling            # sit-down / stand-up, by amount 0..1
│  ├─ meditate.ling       # held cross-legged meditation
│  ├─ swim.ling           # front-crawl
│  ├─ face.ling           # facial-expression map (blink/smile/brow per state)
│  └─ animation_map.ling  # state machine + cross-fade blending
└─ models/                # king.glb / queen.glb (see "glTF" below)
```

## How it fits together

Every clip returns a **POSE vector** — a list of 16 joint values. Each leg and
arm has *flexion* (forward/back), *abduction* (out to the side), and a bend
(knee/elbow), so the rig can fold into a real cross-legged lotus, not just swing:

```
[0]=bob  [1]=lean  [2]=head
[3]=lhipF [4]=lhipA [5]=lknee   [6]=rhipF [7]=rhipA [8]=rknee
[9]=lshF [10]=lshA [11]=lelb   [12]=rshF [13]=rshA [14]=relb
[15]=crouch
```

The clips are pure functions of time, written with the Anima scalar drivers
(`gait_swing`, `gait_lift`, `wobble`, `breathe`, …). They do no rendering.

`animation_map.ling` is the brain:
- `choose AnimState { Idle, Walk, Run, Sit, Meditate, Swim }` — the states (a Ling
  enum / `choose` type).
- `anim_next(moving, running, want_sit, seated, in_water)` — the transition graph.
- `anim_pose(state, phase, sit_amt)` — `match`-dispatches to the right clip.
- `blend_pose(a, b, t)` — element-wise eased cross-fade for smooth transitions.

`moon_pond.ling` is the renderer: it reads input, runs the state machine, blends
between the outgoing and incoming pose, and draws each character as a **filled,
anime‑styled figure** (`draw_figure`, 16‑joint FK with elbows, knees, hip/shoulder
abduction and filled hands) — a stylised head with detailed eyes (white + iris
ring + iris + pupil + twin highlights + lash line + brow), **skull/face contour
shading** (forehead & chin highlights, jaw taper, cheekbone & nose‑bridge shadow),
and layered **hair** (back volume, side locks, crown, bangs) — plus filled,
lightly‑shaded torso / hips / limbs (no outlines), the ground, swaying grass,
trees, the day/night sun & moon, and the **liquid‑ink pond** (`liquid_*`).

The face/skull **vector map** the head rides on — labelled landmarks (incl. scalp
& hairline anchors for hair) plus FACS expression blendshapes — lives in
`crates/ling-animation/src/face.rs`.

**Orientation:** the Ling software renderer uses **screen‑down +y** (world +y maps
*downward*; the floor sits at the largest y). So in this demo "up" is −y — the
ground is at y = 0 and trees, heads, sun & moon all extend toward −y.

## glTF skins (models/king.glb, models/queen.glb)

The eventual plan is to skin `king.glb` / `queen.glb` and drive the *same* Anima
clips through their skeletons. Runtime glTF skin‑loading (`rig_load` /
`SkinBinding` / per‑vertex `joints`+`weights`) **isn't wired into the interpreter
yet** (see the roadmap gap list), so each character is drawn here as a procedural
bone figure instead. Because the clips and the state machine are independent of
the rendering, swapping in real skinned meshes later is a drop‑in change —
`draw_figure(...)` becomes `skin_draw(rig, pose)` and nothing in `animations/`
has to change.

> Verified: all 8 files parse, and the full animation subsystem (state machine,
> `match` dispatch, gait/blend math, cross‑file `use`) runs headlessly. The live
> window needs a display to view.
