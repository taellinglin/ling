# Physics

Ling ships a small physics engine, the `ling-physics` crate, and drives it from
script. You get three independent simulations: **soft bodies** that squash and
stretch, a **rigid-body world** with spin and torque, and a fast **2D liquid**
of water and oil that you can wrap onto 3D shapes. On top of that, `coll_*`
turns a mesh into a static collision shape for spheres and rays, and the glTF
loader that the animation builtins use lives in the same crate.

All simulation builtins run without a window, so you can test physics headless
and print the numbers. Only the `*_draw*` builtins need an open window.

Conventions:

- **Handles** are integers counting from 0 in creation order. An unknown handle
  does nothing and returns `0`, an empty list or nothing. It never raises an error.
- **Defaults.** An omitted argument takes the default given in its entry.
- **Names.** `soft_*`, `rb_*` and `liquid_*` have names in all five languages.
  `coll_*` and `gltf_*` are English only.

## Which way is up

Two coordinate conventions meet here, and mixing them up is the usual cause of
"my body falls through the floor".

- The renderer is **y-down**: positive y points to the bottom of the screen.
  Soft bodies follow it. A floor is the *largest* y a body may reach, and
  gravity is a positive number.
- The rigid-body world is **y-up**: its default gravity is `(0, -9.81, 0)`, and
  `rb_floor` catches a body that is *above* the floor and *falling toward
  negative y*.

Keep rigid bodies y-up and flip y when you draw them. If you set a positive
`rb_gravity` to match the screen, `rb_floor` stops working.

## Summary

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `soft_ball(x, y, z, r)` | `软球` | `ソフトボール` | `소프트볼` | `ลูกบอลนุ่ม` | Create a deformable ball. |
| `soft_step(id, dt, gravity)` | `软体步进` | `ソフト更新` | `소프트스텝` | `ก้าวนุ่ม` | Advance one soft body. |
| `soft_bounce(id, floor_y, restitution)` | `软体落地` | `ソフト着地` | `소프트바운스` | `เด้งนุ่ม` | Collide with a horizontal floor. |
| `soft_contain(id, x0, y0, z0, x1, y1, z1, restitution)` | `软体边界` | `ソフト箱` | `소프트경계` | `กล่องนุ่ม` | Keep the body inside a box. |
| `soft_kick(id, dx, dy, dz, strength)` | `软体踢` | `ソフト衝撃` | `소프트킥` | `เตะนุ่ม` | Push the whole body. |
| `soft_spin(id, ax, ay, az, rate)` | `软体自旋` | `ソフト回転` | `소프트회전` | `หมุนนุ่ม` | Spin about an axis through the centre. |
| `soft_deform(id)` | `形变量` | `変形量` | `변형량` | `ความบิดเบี้ยว` | How squashed it is, 0 to 1. |
| `soft_angular_speed(id)` | `软体角速` | `ソフト角速度` | `소프트각속도` | `ความเร็วเชิงมุมนุ่ม` | Tumbling speed. |
| `soft_centroid(id)` | `软体质心` | `ソフト重心` | `소프트중심` | `จุดศูนย์กลางนุ่ม` | Centre of mass `[x, y, z]`. |
| `soft_nodes(id)` | `软体节点` | `ソフト節点` | `소프트노드` | `จุดนุ่ม` | Every node as a flat list. |
| `rb_add(x, y, z, mass)` | `刚体添加` | `剛体追加` | `강체추가` | `เพิ่มวัตถุแข็ง` | Add a rigid body. |
| `rb_gravity(gx, gy, gz)` | `刚体重力` | `剛体重力` | `강체중력` | `แรงโน้มถ่วงแข็ง` | Set world gravity. |
| `rb_step(dt)` | `刚体步进` | `剛体更新` | `강체스텝` | `ก้าวแข็ง` | Step the whole world. |
| `rb_floor(i, floor_y, restitution, friction)` | `刚体落地` | `剛体着地` | `강체바닥` | `พื้นแข็ง` | Bounce a body off a floor. |
| `rb_impulse(i, ix, iy, iz)` | `刚体冲量` | `剛体インパルス` | `강체충격` | `แรงดลแข็ง` | Apply a linear impulse. |
| `rb_torque(i, tx, ty, tz)` | `扭矩` | `トルク` | `토크` | `แรงบิด` | Apply torque for the next step. |
| `rb_spin(i, wx, wy, wz)` | `自旋` | `スピン` | `스핀` | `หมุน` | Add angular velocity. |
| `rb_pos(i)` | `刚体位置` | `剛体位置` | `강체위치` | `ตำแหน่งแข็ง` | Position `[x, y, z]`. |
| `rb_rot(i)` | `刚体旋转` | `剛体回転` | `강체회전` | `การหมุนแข็ง` | Orientation quaternion `[x, y, z, w]`. |
| `liquid_new(w, h)` | `新建液体` | `液体新規` | `액체생성` | `สร้างของเหลว` | Create a fluid grid. |
| `liquid_splat(id, x, y, kind, amount, radius)` | `液体注入` | `液体追加` | `액체분사` | `หยดของเหลว` | Add water or oil in a disc. |
| `liquid_gravity(id, gx, gy)` | `液体重力` | `液体重力ベクトル` | `액체중력` | `แรงโน้มถ่วงเหลว` | Set the grid's gravity. |
| `liquid_step(id, dt)` | `液体步进` | `液体更新` | `액체스텝` | `ก้าวของเหลว` | Advance one grid. |
| `liquid_step_all(dt)` | `液体全步进` | `液体全更新` | `전체액체스텝` | `ก้าวของเหลวทั้งหมด` | Advance every grid in parallel. |
| `liquid_mix(id)` | `液体混合` | `液体混合度` | `액체혼합` | `การผสมของเหลว` | How mixed the liquids are, 0 to 1. |
| `liquid_set_colors(id, wr, wg, wb, or, og, ob)` | `液体颜色` | `液体配色` | `액체색상` | `สีของเหลว` | Water and oil colours. |
| `liquid_rainbow(id, on)` | `液体彩虹` | `液体虹` | `액체무지개` | `ของเหลวสายรุ้ง` | Rainbow marble colouring. |
| `liquid_draw(id, x, y, scale)` | `绘制液体` | `液体描画` | `액체그리기` | `วาดของเหลว` | Blit the grid in 2D. |
| `liquid_draw_surface(id, kind, cx, cy, cz, radius, height)` | `液体贴面` | `液体曲面` | `액체곡면` | `ของเหลวบนพื้นผิว` | Wrap the grid onto a 3D surface. |
| `coll_build(mesh, group, x, y, z, scale, rot, cell)` | — | — | — | — | Build a collision shape from a mesh. |
| `coll_tris(id)` | — | — | — | — | Triangle count of a shape. |
| `coll_sphere(id, x, y, z, radius)` | — | — | — | — | Push a sphere out of a shape. |
| `coll_ray(id, ox, oy, oz, dx, dy, dz, max_t)` | — | — | — | — | Cast a ray against a shape. |
| `gltf_load(path)` | — | — | — | — | Load a `.glb`/`.gltf` model. |
| `gltf_tris(h)` | — | — | — | — | Triangle count of a model. |
| `gltf_bounds(h, clip, time)` | — | — | — | — | Bounding box of a posed model. |

## Soft bodies

A soft body is a spring-mass sphere of 86 nodes. It squashes on impact and
springs back. Soft bodies are independent: they don't collide with each other
or with the rigid world.

### `soft_ball(x, y, z, r)`

Creates a ball centred at `(x, y, z)` (default 0) with radius `r` (default `1.0`).
Returns its id.

### `soft_step(id, dt, gravity)`

Advances the body by `dt` seconds (default `0.016`) under `gravity` along +y
(default `15.0`; positive pulls toward the bottom of the screen). Each call runs
four internal sub-steps. Call it once per frame.

### `soft_bounce(id, floor_y, restitution)` and `soft_contain(...)`

`soft_bounce` collides the body with the floor plane `floor_y` (default 0). In
y-down terms the body lives at `y <= floor_y`. `restitution` (default `0.5`,
limited to 0..0.99) is the bounciness. A slow contact settles instead of
bouncing, so a resting ball doesn't creep.

`soft_contain(id, min_x, min_y, min_z, max_x, max_y, max_z, restitution)` keeps
the body inside a box (default -5 to 5 on every axis, restitution `0.6`) and
squashes it against every wall.

```ling
bind start = do {
  bind ball = soft_ball(0.0, 0.0 - 3.0, 0.0, 1.0)
  print(len(soft_nodes(ball)) / 3)
  bind frame = 0
  while frame < 300 {
    soft_step(ball, 0.016, 9.8)
    soft_bounce(ball, 2.0, 0.5)
    bind frame = frame + 1
  }
  bind c = soft_centroid(ball)
  print("rests at y = " + str(round(c[1] * 100) / 100))
}
```

```output
86
rests at y = 1.09
```

The ball has radius 1 and the floor is at y = 2. A rigid ball would rest at 1.0;
this one rests at 1.09 because its weight squashes it slightly.

### `soft_kick(id, dx, dy, dz, strength)` and `soft_spin(id, ax, ay, az, rate)`

`soft_kick` adds `strength` (default `0.1`) of speed along the direction
`(dx, dy, dz)`, which is normalised. A zero vector does nothing.
`soft_spin` adds angular velocity about an axis through the centroid. `rate`
(default `0.1`) is radians per step. To roll without slipping, use surface
speed divided by radius.

### Reading a soft body

- `soft_deform(id)`: `0` for a perfect sphere, rising toward `1` when squished.
- `soft_angular_speed(id)`: size of the angular velocity.
- `soft_centroid(id)`: `[x, y, z]`, or `[0, 0, 0]` for an unknown id.
- `soft_nodes(id)`: `[x, y, z, x, y, z, ...]` for all 86 nodes. Draw these
  points or a mesh through them to show the deformed surface.

## Rigid bodies

One shared world of unit cubes (half-size 0.5) with linear and angular motion.
Bodies collide with each other, and `rb_floor` gives them a floor. Friction on
the floor turns sliding into rolling, so boxes tumble.

### `rb_add(x, y, z, mass)`

Adds a body at `(x, y, z)` with `mass` (default `1.0`) and returns its index.
New bodies have a restitution of 0.6.

### `rb_gravity(gx, gy, gz)`

Sets gravity for the whole world. Mind the sign: before any call, gravity is
`(0, -9.81, 0)`, but an omitted `gy` in this call becomes `+9.81`. Pass all
three values.

### `rb_step(dt)`

Advances every body by `dt` seconds (default `0.016`), including rotation, and
resolves body-to-body collisions.

### `rb_floor(i, floor_y, restitution, friction)`

Bounces body `i` off the floor at `floor_y` (default 0). `restitution` and
`friction` both default to `0.6`. Call it every frame, after `rb_step`, for each
body that can hit the floor. It acts only when the body's lower face is below
`floor_y` and it's moving toward negative y.

```ling
bind start = do {
  bind box = rb_add(0.0, 5.0, 0.0, 1.0)
  rb_impulse(box, 2.0, 0.0, 0.0)
  bind frame = 0
  while frame < 240 {
    rb_step(0.016)
    rb_floor(box, 0.0, 0.5, 0.6)
    bind frame = frame + 1
  }
  bind p = rb_pos(box)
  print("x = " + str(round(p[0] * 100) / 100))
  print("y = " + str(round(p[1] * 100) / 100))
}
```

```output
x = 4.5
y = 0.5
```

The box falls from y = 5 and comes to rest with its centre at 0.5, sitting on the
floor, after sliding 4.5 units.

### Forces and state

- `rb_impulse(i, ix, iy, iz)`: an instant change of velocity times mass.
- `rb_torque(i, tx, ty, tz)`: a torque about the world axes, applied on the next step.
- `rb_spin(i, wx, wy, wz)`: add radians per second of angular velocity.
- `rb_pos(i)`: `[x, y, z]`.
- `rb_rot(i)`: a quaternion `[x, y, z, w]`. `[0, 0, 0, 1]` is no rotation. Rotate
  your mesh by it to draw a tumbling body.

## Liquid

A fast 2D grid with two liquids that don't mix, water and oil, moved by one
incompressible velocity field. They blend only where they meet, so the physics
decides where the colours swirl. The grid wraps at its edges, which makes it
seamless on a sphere or cylinder.

### `liquid_new(w, h)`

Creates a grid of `w` by `h` cells (default 64, at least 2) and returns its id.

### `liquid_splat(id, x, y, kind, amount, radius)`

Adds liquid in a disc centred at cell `(x, y)`. `kind` is `0` for water or `1` for
oil, `amount` defaults to `1.0` and `radius` to `4.0` cells.

### `liquid_gravity(id, gx, gy)`

Sets the gravity vector of one grid (default `(0, 60)`). Rotate it over time to
make the liquids slosh.

### `liquid_step(id, dt)` and `liquid_step_all(dt)`

`liquid_step` advances one grid by `dt` (default `0.016`). Grids share no state,
so `liquid_step_all` advances every grid at once, spread across all CPU cores.
Use it when a scene has many liquid surfaces.

### `liquid_mix(id)`

How intermixed the two liquids are: `0` fully separated, up to `1` fully mixed.

```ling
bind start = do {
  bind pool = liquid_new(16, 16)
  liquid_splat(pool, 8.0, 4.0, 1, 1.0, 3.0)
  liquid_splat(pool, 8.0, 12.0, 0, 1.0, 3.0)
  print(liquid_mix(pool))
  bind frame = 0
  while frame < 50 {
    liquid_step(pool, 0.016)
    bind frame = frame + 1
  }
  print(round(liquid_mix(pool) * 1000) / 1000)
}
```

```output
0
0.138
```

### Colours and drawing

- `liquid_set_colors(id, wr, wg, wb, or, og, ob)`: water and oil colours, 0 to
  255 (defaults blue `40, 110, 235` and amber `240, 175, 45`).
- `liquid_rainbow(id, on)`: colour the fluid as a flowing seven-colour marble
  (`on` above 0.5 turns it on).
- `liquid_draw(id, x, y, scale)`: copy the grid to the window at pixel `(x, y)`,
  `scale` pixels per cell (default 4). It writes straight into the frame and
  ignores 3D depth.
- `liquid_draw_surface(id, kind, cx, cy, cz, radius, height)`: wrap the grid onto
  a 3D surface centred at `(cx, cy, cz)`. `kind` is `0` plane, `1` sphere
  (default), `2` cylinder, `3` cone or `4` dome.

```ling-gui
bind start = do {
  open_window(640, 480)
  bind pool = liquid_new(84, 84)
  liquid_splat(pool, 42.0, 25.0, 1, 1.0, 3.0)
  liquid_splat(pool, 42.0, 58.0, 0, 1.0, 3.0)
  while window_is_open() {
    bind t = time_now()
    liquid_gravity(pool, sin(t) * 55.0, cos(t) * 55.0)
    liquid_step(pool, 0.016)
    fill(0, 0, 0)
    set_camera(cos(t * 0.3), sin(t * 0.3), cos(0.4), sin(0.4))
    liquid_draw_surface(pool, 1, 0.0, 0.0, 0.0, 2.2, 3.2)
    present()
  }
}
```

## Collision shapes

`coll_*` turns a loaded mesh into a static triangle soup in a uniform grid, then
tests spheres and rays against it. Use it for level geometry: walls, floors,
ramps.

### `coll_build(mesh, group, x, y, z, scale, rot, cell)`

Builds a shape from a `tm_load` mesh. `group` is one group or `-1` for all.
`(x, y, z)`, `scale` and `rot` (a yaw in radians or a row-major 3x3 matrix) place
it in the world. `cell` (default 8) is the lookup grid size. Returns the shape
id, or `-1` for an unknown mesh. The shape is static, so build it once with its
final transform.

### `coll_sphere(id, x, y, z, radius)`

Pushes a sphere out of the shape, with up to four passes. Returns
`[x, y, z, nx, ny, nz, contacts, deepest]`: the corrected centre, the summed
contact normal (zeros if nothing touched), the number of contacts and the
deepest penetration. Feed the corrected centre back into your player position.

### `coll_ray(id, ox, oy, oz, dx, dy, dz, max_t)`

Casts a ray from `(ox, oy, oz)` along `(dx, dy, dz)` (normalised) up to `max_t`
(default 1000). Returns `[t, nx, ny, nz]`, the distance and the surface normal,
or `[-1, 0, 0, 0]` for no hit. `coll_tris(id)` returns the triangle count.

```ling
bind start = do {
  file_write_hex("floor.ltm", "4c544d310100000005666c6f6f7204000000020000000000a0c0000000000000a0c0000000000000000000000000000080bf000000000000a040000000000000a0c00000803f0000000000000000000080bf000000000000a040000000000000a0400000803f0000803f00000000000080bf000000000000a0c0000000000000a040000000000000803f00000000000080bf00000000000000000100000002000000000000000200000003000000")
  bind mesh = tm_load("floor.ltm")
  bind floor = coll_build(mesh, -1, 0.0, 0.0, 0.0, 1.0, 0.0, 8.0)
  print("triangles: " + str(coll_tris(floor)))
  print(coll_ray(floor, 0.0, -3.0, 0.0, 0.0, 1.0, 0.0, 10.0))
  print(coll_ray(floor, 0.0, -3.0, 0.0, 0.0, 1.0, 0.0, 1.0))
}
```

```output
triangles: 2
[3, 0, -1, 0]
[-1, 0, 0, 0]
```

The hex string is a tiny two-triangle floor in Ling's `.ltm` mesh format. The
first ray hits it 3 units away; the second stops after 1 unit and misses.

## glTF models

The glTF loader is part of `ling-physics` too. `gltf_load(path)` reads a `.glb`
or `.gltf` file (meshes, skeleton, skin and animation clips) and returns a
handle, or `-1` on failure with the reason on standard error.

- `gltf_tris(h)`: total triangle count, a quick performance check.
- `gltf_bounds(h, clip, time)`: `[min_x, min_y, min_z, max_x, max_y, max_z]` of
  the model as drawn with that animation clip (default 0) at that time
  (default 0), in glTF space (y-up, unscaled). All zeros for a bad handle. Use it
  to stand a model on the ground or size a collision sphere.

```ling
bind start = do {
  bind model = gltf_load("missing.glb")
  print(model)
  print(gltf_tris(model))
  print(gltf_bounds(model))
}
```

```output
-1
0
[0, 0, 0, 0, 0, 0]
```

Drawing, posing and playing clips are covered in [Animation](../reference/animation.md).

## See also

- [Acoustic Bodies](../reference/body.md): the `body_*` toolkit that simulates how a shape sounds.
- [3D Shapes and Meshes](../reference/shapes.md): `tm_load` and drawing meshes.
- [Window and Camera](../reference/window.md): `set_camera` and the frame loop.
