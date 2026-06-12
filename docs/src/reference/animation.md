# Builtins — Animation (Anima)

**Anima** is Ling's unified animation system (the `ling-animation` crate). Its guiding
idea: *one rig, any target, any temperament.* Every animated thing — a 2-D vtexture, a
3-D mesh, a particle field, or a bag of scalar parameters — is a **rig** of abstract
joints, driven by composable **drivers**, and deformed onto its target by a **binding**.

What makes Anima different is the **temperament axis**: a per-chain scalar from
**organic (灵)** to **mechanical (机)** that selects and cross-fades the solver — muscle,
breath, jiggle, and IK on one end; exact gear, cam, and linkage coupling on the other.
A robot can have organic hydraulic sag; a creature can have a mechanical jaw.

The **scalar drivers** below are available today as builtins. They are pure functions —
call them every frame with the current time (and any state you keep) to drive a scale, a
position, a rotation, or a vtex parameter. Each is available in all five languages.

## Organic drivers (灵)

| English | Signature | Returns | Description |
|---|---|---|---|
| `tween` | `tween(a, b, t)` | number | Linear interpolation `a → b`, `t` clamped to `[0,1]`. |
| `tween_ease` | `tween_ease(a, b, t, "kind")` | number | Eased interpolation. `kind` is a curve name (see below). |
| `breathe` | `breathe(t, rate, depth)` | number | Breathing multiplier centred on `1.0`: `1 + depth·sin(2π·rate·t)`. |
| `wobble` | `wobble(t, freq, amp, phase)` | number | Secondary sway / jiggle sine. |
| `gait_phase` | `gait_phase(t, speed)` | number | Locomotion phase in `[0,1)`. |
| `gait_swing` | `gait_swing(t, speed, stride)` | number | Forward/back leg swing for a walk cycle. |
| `gait_lift` | `gait_lift(t, speed, height)` | number | Foot lift (rises only during the swing half). |
| `spring_to` | `spring_to(pos, vel, target, stiffness, damping, dt)` | `[pos, vel]` | One spring step (spring-bone / secondary motion). Returns the new position and velocity. |
| `ik2` | `ik2(l1, l2, tx, ty)` | `[shoulder, elbow]` | Planar two-bone IK: joint angles that reach target `(tx, ty)`. |

### Easing curve names (for `tween_ease`)

`linear`, `step`, `quad_in/out/in_out`, `cubic_in/out/in_out` (`smooth`),
`sine_in/out/in_out`, `expo_in/out/in_out`, `elastic_in/out` (`elastic`),
`back_in/out/in_out` (`overshoot`), `bounce_in/out` (`bounce`).

## Mechanical drivers (机)

| English | Signature | Returns | Description |
|---|---|---|---|
| `gear_couple` | `gear_couple(angle, teeth_in, teeth_out)` | number | Meshed-gear output angle — opposite sign, scaled by the tooth ratio. |
| `gear_train` | `gear_train(angle, [t0, t1, t2, …])` | `[angle…]` | Angles of every gear down a meshed train. |
| `cam_lift` | `cam_lift(angle, lift)` | number | Cam follower displacement over a rotation (`0` at `θ=0`, peak at `θ=π`). |
| `piston` | `piston(angle, crank, rod)` | number | Slider-crank pin position from a crank angle. |
| `rack` | `rack(angle, radius)` | number | Rack-and-pinion linear travel: `θ · radius`. |

## Multilingual names

| en | zh | ja | ko | th |
|---|---|---|---|---|
| `tween` | `补间` | `補間` | `트윈` | `แทรกค่า` |
| `breathe` | `呼吸` | `呼吸` | `호흡` | `หายใจ` |
| `wobble` | `摆动` | `揺れ` | `흔들림` | `โยก` |
| `gait_swing` | `步摆` | `歩振り` | `걸음흔들` | `ก้าวแกว่ง` |
| `spring_to` | `弹向` | `バネ寄せ` | `스프링이동` | `สปริงไป` |
| `ik2` | `反解` | `逆運動` | `역운동` | `ไอเค2` |
| `gear_couple` | `齿轮联动` | `歯車連動` | `기어연동` | `เฟืองทด` |
| `piston` | `活塞` | `ピストン` | `피스톤` | `ลูกสูบ` |

(The full table lives in `lexicons/*.ling` and `lingfu normalize`'s `BUILTINS_ANIM`.)

## Example — a breathing, walking, geared scene

```ling
bind start = do {
    open_window("Anima")
    bind t = 0.0
    while window_is_open() {
        bind t = t + delta_time()
        fill(10, 12, 24)

        // organic 灵 — a body that breathes and a foot that walks
        bind chest = breathe(t, 0.4, 0.08)      // ~1.0 ± 0.08
        bind foot  = gait_swing(t, 1.6, 30.0)   // leg swing in pixels

        // mechanical 机 — a gear train driven by a turning shaft
        bind shaft = t * 2.0
        bind gears = gear_train(shaft, [12, 36, 12])

        present()
    }
}
```

## For Rust authors — the crate (`ling-animation`)

The builtins above are the scalar layer. The crate also exposes the structural core
that the runtime rig/skin builtins will build on:

- `ease` — `EaseFunction`, `Lerp`, `tween_ease`.
- `track` — `Keyframe`, `Track`, `Timeline`.
- `rig` — `Rig`, `Joint`, `Pose`, world-pose + skinning matrices.
- `temperament` — the `Temperament` (灵 ↔ 机) axis with `blend`, `liveliness`, `rigidity`.
- `scalar` — the per-frame driver math used by the builtins.
- `mechanism` — exact linkages (`gear_train`, `four_bar`).
- `creature` — procedural `biped_legs`, `quadruped_legs`, `breathing_scale`.

## Not yet wired (roadmap)

The full rig pipeline is staged. Still to land as builtins:

- `rig_load` (glTF skin import), `rig_joint`, `pose`/`anim_clip`/`anim_play`/`anim_tick`.
- `SkinBinding` (3-D linear-blend skinning over a mesh), `WarpBinding` (2-D vtex
  control lattice), `ParamBinding` (drive vtex parameters / blendshapes).
- The driver **motion graph** (masked, weighted blending of clip + procedural + physics).
- GPU skinning (via `ling-gpu`) and driver-state snapshotting (via `ling-net`).
