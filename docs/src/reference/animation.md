# Animation

Anima is Ling's animation system. Anything that moves, whether a 2D drawing, a 3D mesh, a particle or a plain number, is driven by small **drivers**: functions you call every frame with the current time. Each driver returns a number, and you feed that number into a size, a position, an angle or a pattern argument.

The drivers come in two temperaments:

- **Organic** drivers (灵, "spirit") make soft, lifelike motion: breathing, swaying, walking, springy follow-through and two-bone inverse kinematics.
- **Mechanical** drivers (机, "machine") make exact, rigid motion: meshed gears, cams, slider-cranks and rack and pinion.

You can mix both in one character, for example a creature with a breathing chest and a mechanical jaw. For imported 3D characters with a skeleton, the `gltf_*` builtins at the end of this page play skeletal animation clips.

## Summary

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `tween(a, b, t)` | `补间` | `補間` | `트윈` | `แทรกค่า` | Linear interpolation |
| `tween_ease(a, b, t, curve)` | `缓动补间` | `緩和補間` | `이징트윈` | `แทรกนุ่ม` | Eased interpolation |
| `ease(curve, t)` | — | — | — | — | Value of a UI easing curve |
| `breathe(t, rate, depth)` | `呼吸` | `呼吸` | `호흡` | `หายใจ` | Breathing scale around 1 |
| `wobble(t, freq, amp, phase)` | `摆动` | `揺れ` | `흔들림` | `โยก` | Sway or jiggle |
| `gait_phase(t, speed)` | `步相` | `歩相` | `걸음위상` | `เฟสก้าว` | Walk-cycle phase |
| `gait_swing(t, speed, stride)` | `步摆` | `歩振り` | `걸음흔들` | `ก้าวแกว่ง` | Leg swing |
| `gait_lift(t, speed, height)` | `抬脚` | `足上げ` | `발들기` | `ยกเท้า` | Foot lift |
| `spring_to(pos, vel, target, k, d, dt)` | `弹向` | `バネ寄せ` | `스프링이동` | `สปริงไป` | One damped-spring step |
| `ik2(l1, l2, tx, ty)` | `反解` | `逆運動` | `역운동` | `ไอเค2` | Two-bone inverse kinematics |
| `gear_couple(angle, teeth_in, teeth_out)` | `齿轮联动` | `歯車連動` | `기어연동` | `เฟืองทด` | Two meshed gears |
| `gear_train(angle, teeth)` | `齿轮组` | `歯車列` | `기어열` | `ชุดเฟือง` | A chain of meshed gears |
| `cam_lift(angle, lift)` | `凸轮升程` | `カム揚程` | `캠리프트` | `ยกลูกเบี้ยว` | Cam follower |
| `piston(angle, crank, rod)` | `活塞` | `ピストン` | `피스톤` | `ลูกสูบ` | Slider-crank |
| `rack(angle, radius)` | `齿条` | `ラック` | `랙` | `แร็ค` | Rack and pinion |

## How drivers work

Drivers are **pure functions**. They keep no hidden state, so the same arguments always give the same answer.

- Time-based drivers (`breathe`, `wobble`, `gait_*`) take the current time from you, usually `time_now()` or a variable you advance by `1/60` each frame. Nothing plays in the background.
- The spring driver (`spring_to`) returns its new state, and you pass it back in on the next frame. Your program owns the state.

Angles are in radians. Lengths use whatever units your drawing uses. The drivers compute in single precision, so the examples round their results with a small helper `r3`.

## Interpolation

### `tween(a, b, t)` and `tween_ease(a, b, t, curve)`

`tween` returns `a + (b - a) * t`. The progress `t` is **clamped** to 0..1, so you can keep increasing it past 1 and the value stays at `b`.

`tween_ease` does the same through an easing curve. `curve` is a name from the Anima table below (default `"linear"`). Elastic and back curves can overshoot `b`.

### `ease(curve, t)`

The raw value of a UI-toolkit easing curve at progress `t` (clamped to 0..1): 0 at the start, 1 at the end. The curve name comes **first**. Use it when you need the curve itself, for example to blend two colours. It has no localized names.

```ling
fn r3(x: number) -> number {
    round(x * 1000) / 1000
}

bind start = do {
    print(tween(0, 10, 0.25))
    print(tween(0, 10, 2.0))
    print(r3(tween_ease(0, 100, 0.5, "cubic_in")))
    print(r3(tween_ease(0, 100, 0.5, "bounce_out")))
    print(ease("quad_in", 0.5))
    print(r3(ease("smooth", 0.25)))
}
```

```output
2.5
10
12.5
76.563
0.25
0.125
```

### Easing curve names

The two builtins use **different name sets**. Both ignore case.

**`tween_ease`** (Anima curves). Underscores are optional. Unknown names give `linear`.

| Family | Names |
|---|---|
| Basic | `linear`, `step` |
| Quadratic | `quad_in`, `quad_out`, `quad_in_out` |
| Cubic | `cubic_in`, `cubic_out`, `cubic_in_out` (also `smooth`) |
| Sine | `sine_in`, `sine_out`, `sine_in_out` |
| Exponential | `expo_in`, `expo_out`, `expo_in_out` |
| Elastic | `elastic_in`, `elastic_out` (also `elastic`) |
| Back | `back_in`, `back_out`, `back_in_out` (also `overshoot`) |
| Bounce | `bounce_in`, `bounce_out` (also `bounce`) |

**`ease`** (UI curves). Dashes and underscores are interchangeable. Unknown names give `quad_inout`.

| Name | Shape |
|---|---|
| `linear` | Straight line |
| `quad_in`, `ease_in` | Slow start |
| `quad_out`, `ease_out` | Slow finish |
| `quad_inout`, `ease`, `smooth` | Slow start and finish (the default) |
| `cubic_in`, `cubic_out`, `cubic_inout` | Stronger versions |
| `quart_out` | Fast start, long slow finish |
| `sine`, `sine_inout` | Gentle sine in-out |
| `expo`, `expo_out` | Exponential decay |
| `back`, `back_out` | Overshoots, then settles |
| `elastic`, `elastic_out` | Springy overshoot |
| `bounce`, `bounce_out` | Bounces at the end |

## Organic drivers

### `breathe(t, rate, depth)`

A breathing multiplier centred on 1: `1 + depth * sin(2π * rate * t)`. `rate` is breaths per second (default 1), `depth` the amplitude (default 0.1). Multiply a size or a pattern radius by it.

### `wobble(t, freq, amp, phase)`

A sine sway for secondary motion: `amp * sin(2π * (freq * t + phase))`. Defaults: `freq` 1, `amp` 1, `phase` 0. The phase is in cycles, so `0.25` is a quarter-cycle shift. Give each limb a different phase and the body sways out of lockstep.

### `gait_phase(t, speed)`, `gait_swing(t, speed, stride)` and `gait_lift(t, speed, height)`

A walk cycle. `speed` is cycles per second (default 1).

- `gait_phase` returns where the cycle is, from 0 up to (not including) 1.
- `gait_swing` returns the forward and backward swing of a leg, between `-stride` and `stride`. For the other leg, use `t + 0.5 / speed`.
- `gait_lift` returns how high the foot is raised: positive during the swing half, 0 while planted. Screen y points down, so subtract it: `foot_y = ground_y - gait_lift(...)`.

```ling
fn r3(x: number) -> number {
    round(x * 1000) / 1000
}

bind start = do {
    bind t = 0.125
    print(r3(breathe(t, 1, 0.2)))
    print(r3(wobble(t, 1, 10, 0)))
    print(r3(gait_phase(t, 2)))
    print(r3(gait_swing(t, 1, 30)))
    print(r3(gait_lift(t, 1, 10)))
}
```

```output
1.141
7.071
0.25
21.213
7.071
```

### `spring_to(pos, vel, target, stiffness, damping, dt)`

One step of a damped spring pulling `pos` toward `target`. Use it for follow-through, camera smoothing and tails. Defaults: `stiffness` 120, `damping` 14, `dt` 1/60. It returns a two-element list `[new_pos, new_vel]`. Keep both numbers and pass them back next frame.

With the defaults, the value overshoots a little, then settles within about a second:

```ling
bind start = do {
    bind pos = 0.0
    bind vel = 0.0
    bind frame = 0
    while frame < 60 {
        bind s = spring_to(pos, vel, 1.0, 120, 14, 1 / 60)
        bind pos = s[0]
        bind vel = s[1]
        bind frame = frame + 1
        if frame % 15 == 0 {
            print(str(frame) + ": " + str(round(pos * 1000) / 1000))
        }
    }
}
```

```output
15: 0.982
30: 1.021
45: 0.997
60: 1
```

### `ik2(l1, l2, tx, ty)`

Planar two-bone inverse kinematics: the shoulder and elbow angles that bring the tip of a two-segment arm or leg to a target. `l1` and `l2` are the bone lengths (default 1), and `(tx, ty)` is the target relative to the shoulder.

It returns `[shoulder, elbow]` in radians. `shoulder` is the angle of the first bone, and `elbow` is the bend of the second bone **relative to the first**. A target out of reach is clamped, so the limb stretches toward it. To place the joints:

- elbow = shoulder position + `l1 * (cos(shoulder), sin(shoulder))`
- hand = elbow + `l2 * (cos(shoulder + elbow), sin(shoulder + elbow))`

```ling
fn r3(x: number) -> number {
    round(x * 1000) / 1000
}

bind start = do {
    bind a = ik2(1, 1, 1, 1)
    print([r3(a[0]), r3(a[1])])
}
```

```output
[1.571, -1.571]
```

## Mechanical drivers

| Builtin | Returns |
|---|---|
| `gear_couple(angle, teeth_in, teeth_out)` | The driven gear's angle: `-angle * teeth_in / teeth_out` (0 if `teeth_out` is 0). Meshed gears turn the opposite way. |
| `gear_train(angle, teeth)` | A list with the angle of every gear in a chain. `teeth` is a list of tooth counts; the first element of the result is `angle` itself. |
| `cam_lift(angle, lift)` | Cam follower height: `lift * (1 - cos(angle)) / 2`, from 0 at angle 0 to `lift` at π. |
| `piston(angle, crank, rod)` | Piston pin position along the cylinder from the crank centre. Defaults: `crank` 1, `rod` 2 (the rod should be longer than the crank). |
| `rack(angle, radius)` | Rack travel: `angle * radius`. |

A 12-tooth gear driving a 36-tooth gear turns it a third as fast, the other way:

```ling
fn r3(x: number) -> number {
    round(x * 1000) / 1000
}

bind start = do {
    print(r3(gear_couple(3, 12, 36)))
    bind g = gear_train(1, [12, 36, 12])
    print([r3(g[0]), r3(g[1]), r3(g[2])])
    print(r3(cam_lift(3.14159265, 2)))
    print(r3(piston(0, 1, 3)))
    print(rack(2, 0.5))
}
```

```output
-1
[1, -0.333, 1]
2
4
1
```

## A moving scene

This window combines a breathing disc, a walking foot and a spring that chases a moving target. The spring's position and velocity are carried from frame to frame.

```ling-gui
bind start = do {
    open_window(640, 360, "Anima")
    bind t = 0.0
    bind px = 0.0
    bind pv = 0.0
    while window_is_open() {
        fill(10, 12, 28)
        # organic: a breathing disc
        set_color(120, 220, 255)
        draw_disc(120, 120, 40 * breathe(t, 0.4, 0.12))
        # organic: a walking foot
        bind sw = gait_swing(t, 1.0, 30)
        bind lift = gait_lift(t, 1.0, 20)
        set_color(255, 210, 90)
        draw_line(120, 200, 120 + sw, 260 - lift)
        draw_disc(120 + sw, 260 - lift, 10)
        # organic: a spring chasing a target
        bind target = 400 + 80 * sin(t * 1.3)
        bind s = spring_to(px, pv, target, 120, 10, 0.016)
        bind px = s[0]
        bind pv = s[1]
        set_color(120, 255, 160)
        draw_disc(px, 180, 12)
        present()
        bind t = t + 0.016
    }
}
```

## Skeletal animation (glTF)

Rigged characters exported as `.glb` or `.gltf` carry a skeleton, skin weights and animation clips. These builtins load such a model and draw it skinned. They have no localized names, except the two textured draws: `gltf_anim_draw_tex` is also `骨骼动画纹理绘制` (zh), `スキンアニメ描画` (ja), `글꼴애니텍스처` (ko), `วาดแอนิเมชันมีพื้นผิว` (th), and `gltf_pose_draw_tex` is also `骨骼姿势纹理绘制`, `スキンポーズ描画`, `글꼴포즈텍스처`, `วาดสกินมีพื้นผิว`.

| Builtin | Purpose |
|---|---|
| `gltf_load(path)` | Load a model. Returns a handle, or `-1` on failure. |
| `gltf_anim_count(h)` | Number of animation clips. |
| `gltf_anim_name(h, i)` | Name of clip `i`. |
| `gltf_anim_dur(h, i)` | Length of clip `i` in seconds. |
| `gltf_joint_count(h)` | Number of skin joints (bones). |
| `gltf_joint_name(h, j)` | Name of joint `j`. |
| `gltf_autorig(h)` | Give a model without a skeleton a procedural 20-bone humanoid rig. Returns the bone count. |
| `gltf_anim_draw_tex(h, x, y, z, scale, yaw, clip, time)` | Draw the model textured and skinned by clip `clip` at `time` seconds. |
| `gltf_pose_draw(h, x, y, z, scale, yaw, pose)` | Draw the model skinned by your own pose (flat colours). |
| `gltf_pose_draw_tex(h, x, y, z, scale, yaw, pose)` | The same, textured. |
| `gltf_bounds(h, clip, time)` | `[minX, minY, minZ, maxX, maxY, maxZ]` of the model as drawn with that clip and time. |

Notes:

- Clip time wraps, so you can pass an ever-growing time and the clip loops.
- `pose` is a flat list of Euler angles in radians, three per bone (X, Y, Z).
- glTF models are Y-up. The draws flip them into the renderer's y-down space, so `(x, y, z)` is where the model's origin (usually its feet) goes.
- `gltf_anim_draw_tex` takes more optional arguments after `time`: ambient light, back-face culling (`1` to cull), a source model whose clips drive this one (for equipment matched by joint name), a list of extra per-bone rotations, and a `0xRRGGBB` tint.
- `gltf_draw(h, x, y, z, scale, yaw)` draws the model unskinned. `gltf_mesh_count`, `gltf_mesh_name`, `gltf_mesh_show(h, i, on)` and `gltf_mesh_tint(h, i, 0xRRGGBB)` show, hide or tint individual meshes, for example to swap equipment.

```ling-gui
bind start = do {
    open_window(960, 540, "Walk")
    set_depth_test(1)
    bind hero = gltf_load("hero.glb")
    bind i = 0
    while i < gltf_anim_count(hero) {
        print(gltf_anim_name(hero, i) + ": " + str(gltf_anim_dur(hero, i)) + " s")
        bind i = i + 1
    }
    while window_is_open() {
        fill(20, 24, 40)
        gltf_anim_draw_tex(hero, 0, 2, 6, 1, 0.5, 0, time_now())
        flush_3d()
        present()
    }
}
```

## What is not a builtin

The `ling-animation` crate also has keyframe tracks and timelines, a joint rig with skinning matrices, procedural biped and quadruped gaits, a four-bar linkage solver and a facial rig. Those are Rust APIs for embedders, not Ling builtins. Names such as `rig_load` or `anim_play` that appear in older documents do not exist, and `delta_time()` is not implemented. Use `time_now()` for time.

See also: [Post-FX and Lighting](postfx.md), [Window and Camera](window.md).
