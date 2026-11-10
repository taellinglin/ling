# Showcase

These apps and demos are written in Ling and run on the Ling runtime. Screenshots of
the apps are on the website: [ling-lang.org/#apps](https://ling-lang.org/#apps).

## Apps

| App | What it is |
|---|---|
| **LingStationX** | A digital audio workstation: arranger, piano roll, tracker, session view, a node-graph patcher, a mixer and the Nova, Prism and Growl synths. It imports MIDI and has a slicer, time-warp and a mastering chain. |
| **LingDev** | A 3D game editor with a node tree, a timeline and cutscene tracks for camera, dialog and effects. Games made with it include Dance Ling, Ling Hazard, Ling Breaker, Moko, Ling Agent and the Soul Symphony series. |
| **LingModeler** | Modelling, rigging, weight painting, posing and animation, plus vector illustration, 2D animation with onion skin and procedural textures. It also runs inside LingOS on x86-64 and Raspberry Pi. |
| **LingForFun** | An online action RPG with an overworld, towns, dungeons, flight, combat and quests. It has a desktop client, a WASM browser client and a server cluster, all in Ling. Written with Chinese keywords. In development. |
| **Ling Arcade** | A console-game library with an NES emulator written in Ling: the 6502 CPU, PPU, mappers and APU are plain `.ling` code on Ling's JIT. It passes the full nestest log (8,991 instructions, cycle-exact). |
| **Ling Wallet** | A post-quantum privacy wallet, full node and miner for LNG: ML-DSA-87 identities and hybrid ML-KEM-1024 + X25519 key exchange. Experimental and unaudited. |

## Community projects

| Project | What it is |
|---|---|
| [**Soul Symphony**](https://github.com/taellinglin/soul-symphony-ling-edition) | An RPG arcade physics game. Roll an orb across 9 procedural levels with a boss fight after each one. Arcade versus, networking and co-op, in 5 languages. |
| [**LingSoul**](https://github.com/taellinglin/LingSoul) | An atmospheric soul-balance sim in Ling's wireframe style, with a constructed glyph alphabet for its in-world HUD. |

## Demos in the repository

The [Ling repository](https://github.com/taellinglin/ling) has many runnable examples.
A few to start with:

| Demo | What it shows |
|---|---|
| `examples/music/hallway_runner.ling` | An infinite runner that reacts to the music; coins ring the melody as spatial sound |
| `examples/music/karaoke_hero.ling` | Karaoke pitch bars from a vocal MIDI, with a live microphone tuner |
| `examples/music/synth_jam.ling` | GM instruments synthesized from `.ling` patches |
| `examples/physics/bouncy_ball.ling` | A spring-mass soft body that squashes as it bounces |
| `examples/physics/tumbling.ling` | Rigid bodies tumbling and rolling |
| `examples/physics/liquid_marble.ling` | A water and oil simulation on a spinning sphere, cone and dome |
| `examples/ui/hud_showcase.ling` | The UI toolkit: HUD, meters, controls and game widgets |
| `examples/crypto/crypto_donut_physics.ling` | A SHA3 hash shapes a bouncing torus knot, with reactive audio |

Run one from the repository root:

```sh
ling run examples/physics/liquid_marble.ling
```

The [Examples](examples/dao-chamber.md) section of these docs walks through four 3D scenes
in more detail.
