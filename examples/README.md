# Ling Examples

Example programs for the Ling omniglot language, grouped by theme. Run any of
them with:

```sh
ling run examples/<category>/<file>.ling
# or, from a checkout without an installed `ling` binary:
cargo run --bin ling -- run examples/<category>/<file>.ling
```

Most graphical demos open a window and loop until you close it or press
`Escape`. Generated `.svg` output lives in [`output/`](output/).

---

## basics/
Small programs for learning the language and its multilingual syntax.

| File | Description |
|------|-------------|
| [fib.ling](basics/fib.ling) | Fibonacci — minimal numeric example |
| [thai_hello_world.ling](basics/thai_hello_world.ling) | Hello world written in Thai keywords |
| [สวัสดี.ling](basics/สวัสดี.ling) | "Hello" — fully Thai source + filename |
| [thai.ling](basics/thai.ling) | Thai-language pangram exercising bind/form/choose/can/give |
| [complex_hello.ling](basics/complex_hello.ling) | A more elaborate hello-world demonstration |
| [gfx.ling](basics/gfx.ling) | Coloured geometric shapes — graphics primer |
| [advanced.ling](basics/advanced.ling) | Language tech demo covering many features |

## 3d-rooms/
Walkable 3-D architectural environments rendered with the holographic cel renderer.

| File | Description |
|------|-------------|
| [Gallery.ling](3d-rooms/Gallery.ling) | Hub world + 9 rooms switchable by numpad |
| [3d-room.ling](3d-rooms/3d-room.ling) | Thai Matrix Cathedral |
| [3d-american-room.ling](3d-rooms/3d-american-room.ling) | The Grand Lodge of the Americas |
| [3d-chinese-room.ling](3d-rooms/3d-chinese-room.ling) | 東亞禪寺 — East Asian Chan temple |
| [3d-family-room.ling](3d-rooms/3d-family-room.ling) | 囍家团圆 — Double Happiness reunion |
| [3d-japanese-room.ling](3d-rooms/3d-japanese-room.ling) | 円相禅堂 — Enso Zen meditation hall |
| [3d-korean-room.ling](3d-rooms/3d-korean-room.ling) | 육각 한옥 선원 — Hexagonal hanok |
| [3d-sakohn-room.ling](3d-rooms/3d-sakohn-room.ling) | สกลนคร temple room |
| [3d-vietnamese-room.ling](3d-rooms/3d-vietnamese-room.ling) | 靈臺五行殿 — Linh Đài five-elements hall |
| [3d-vietnamese-room-v2.ling](3d-rooms/3d-vietnamese-room-v2.ling) | Linh Đài hall, revised |
| [3d-meta.ling](3d-rooms/3d-meta.ling) | Hypercube-Möbius mirror room of pyramids |
| [3d-thai-japanese-clocktown.ling](3d-rooms/3d-thai-japanese-clocktown.ling) | Thai/Japanese clocktown junction |
| [sakohn.ling](3d-rooms/sakohn.ling) | สกลนคร scene |
| [ling-dao-chamber.ling](3d-rooms/ling-dao-chamber.ling) | 道场 — Temple of the Way |
| [ling-gyro-temple.ling](3d-rooms/ling-gyro-temple.ling) | 混沌殿 — Palace of Chaos and Order |
| [chinese_chakras.ling](3d-rooms/chinese_chakras.ling) | 中脈輪 — chakra spine from floor to crown dome |
| [PeyoteOrgan.ling](3d-rooms/PeyoteOrgan.ling) | The higher-dimensional organism |

## audiovisual/
Synesthetic pieces combining physics, audio synthesis, and graphics.

| File | Description |
|------|-------------|
| [Garden.ling](audiovisual/Garden.ling) | The Synesthetic Garden — shape→tone (pentatonic) |
| [Lounge.ling](audiovisual/Lounge.ling) | The Synesthetic Lounge |
| [MirrorGarden.ling](audiovisual/MirrorGarden.ling) | Garden of 灵: Sun, Moon, and Mirror |
| [mystic_chamber.ling](audiovisual/mystic_chamber.ling) | Omniglot physics + audio synthesis demo |
| [mystical_symphony.ling](audiovisual/mystical_symphony.ling) | Interactive physics + audio + graphics |
| [sacred_sounds.ling](audiovisual/sacred_sounds.ling) | Thai interactive musical physics |
| [sonic_realm.ling](audiovisual/sonic_realm.ling) | Physics + audio + graphics omniglot demo |
| [windchime_pentatonic.ling](audiovisual/windchime_pentatonic.ling) | 风铃 — pentatonic windchime spiral |
| [PO2.ling](audiovisual/PO2.ling) | Peyote Organ v2 — audio-driven organism |
| [พิหารเสียงวิเศษ.ling](audiovisual/พิหารเสียงวิเศษ.ling) | Hall of Mystical Sounds (Thai) |

## geometry/
Procedural meshes, voxels, and fractal geometry.

| File | Description |
|------|-------------|
| [shapes_demo.ling](geometry/shapes_demo.ling) | 3-D shape gallery — primitive library + cel shading |
| [voxel_sphere.ling](geometry/voxel_sphere.ling) | Voxelized sphere |
| [voxel_sphere_debug.ling](geometry/voxel_sphere_debug.ling) | Voxel sphere with position debug output (no fullscreen) |
| [voxel_world.ling](geometry/voxel_world.ling) | Minecraft-like interactive blocks |
| [3d-serpentine.ling](geometry/3d-serpentine.ling) | สามเหลี่ยม serpentine triangle form |
| [color-serpinski.ling](geometry/color-serpinski.ling) | Coloured Sierpiński triangle |

## physics/
Rigid-body simulation, collisions, and curved-space movement.

| File | Description |
|------|-------------|
| [physics.ling](physics/physics.ling) | Physics abstractions — vector & force library |
| [physics_demo.ling](physics/physics_demo.ling) | Bouncing ball in a cage, physics-driven |
| [dodecahedron.ling](physics/dodecahedron.ling) | Bouncing ball in a dodecahedron cage |
| [hyperbolic3d.ling](physics/hyperbolic3d.ling) | Playable symmetrical maze in hyperbolic 3-D space |
| [hyperbolic_world.ling](physics/hyperbolic_world.ling) | H²×ℝ block walker (Hypermine-flavoured) |

## crypto/
Cryptography library and visualizations.

| File | Description |
|------|-------------|
| [crypto.ling](crypto/crypto.ling) | Ling cryptography library (multilingual wrappers) |
| [crypto_hologram.ling](crypto/crypto_hologram.ling) | 4-D visualization of cryptographic operations |
| [crypto_physics_world.ling](crypto/crypto_physics_world.ling) | Cryptographically-verified physics world |
| [crypto_physics_game_demo.ling](crypto/crypto_physics_game_demo.ling) | Crypto + physics game demo |

## procedural/
Phase 1 "DMT Trip Coder" features: Perlin/FBM noise, lerp/smoothstep, real-time
clock, and additive blending.

| File | Description |
|------|-------------|
| [dmt_demo.ling](procedural/dmt_demo.ling) | Full showcase — noise, lerp, time, circles, additive blend |
| [noise_waves.ling](procedural/noise_waves.ling) | Layered Perlin & FBM wave field |
| [additive_glow.ling](procedural/additive_glow.ling) | Circles with additive blending |

## tools/
Utilities that consume other `.ling` source files and emit SVG.

| File | Description |
|------|-------------|
| [ling_map.ling](tools/ling_map.ling) | Convert any `.ling` source file into a cryptographic map SVG |
| [visualize.ling](tools/visualize.ling) | Ling source → dark-themed SVG pattern visualizer |

## output/
Generated `.svg` artifacts from the demos and tools above. Safe to delete and
regenerate.
