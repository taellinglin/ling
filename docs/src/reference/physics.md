# Builtins — Physics

Ling exposes the `ling-physics` engine to scripts: **rigid bodies with angular dynamics**,
**deformable soft bodies**, and a **fast 2-D immiscible liquid simulation** that maps onto 3-D
surfaces. Every builtin has aliases in all five languages (see
[Builtin Aliases by Language](../multilingual/builtins.md)).

Coordinate note: in the renderer **+y points down**, so "gravity" is a positive `y` value.

## Soft bodies (deformable / bouncy)

A spring-mass sphere that squashes and stretches on impact.

| Builtin | Meaning |
|---|---|
| `soft_ball(x, y, z, r)` → id | Create a deformable ball at `(x,y,z)` radius `r`. |
| `soft_step(id, dt, gravity)` | Advance the body `dt` seconds under `gravity` (y). |
| `soft_bounce(id, floor_y, restitution)` | Bounce nodes off a horizontal floor. |
| `soft_contain(id, minx,miny,minz, maxx,maxy,maxz, restitution)` | Keep the body inside a box, bouncing (squashing) off every wall. |
| `soft_kick(id, dx, dy, dz, strength)` | Impulse the whole body along a direction. |
| `soft_deform(id)` → f | Deformation factor `0` (round) … `1` (squished). |
| `soft_centroid(id)` → `[x,y,z]` | Centre of mass. |
| `soft_nodes(id)` → `[x,y,z, …]` | Flat list of every mesh node — draw it to show the deformed surface. |

```ling
bind ball = soft_ball(0.0, 0.0-2.5, 0.0, 1.2)
while window_is_open() {
    soft_step(ball, 0.016, 9.0)
    soft_contain(ball, 0.0-4.0,0.0-4.0,0.0-4.0, 4.0,4.0,4.0, 0.55)
    bind ns = soft_nodes(ball)            # draw the squashing mesh
    # … render nodes …
}
```
See `examples/physics/bouncy_ball.ling`.

## Rigid bodies + angular dynamics

A shared rigid-body world with torque, spin, inertia and a spin-inducing floor bounce, so
bodies tumble and roll.

| Builtin | Meaning |
|---|---|
| `rb_add(x, y, z, mass)` → i | Add a body, returns its index. |
| `rb_gravity(gx, gy, gz)` | Set world gravity. |
| `rb_torque(i, tx, ty, tz)` | Apply torque (world axes). |
| `rb_spin(i, wx, wy, wz)` | Add to angular velocity. |
| `rb_impulse(i, ix, iy, iz)` | Apply a linear impulse. |
| `rb_floor(i, y, restitution, friction)` | Bounce off a floor; friction converts motion into roll/spin. |
| `rb_step(dt)` | Step the whole world (integrate + collide). |
| `rb_pos(i)` → `[x,y,z]` | Body position. |
| `rb_rot(i)` → `[qx,qy,qz,qw]` | Body orientation quaternion (rotate your mesh by it). |

See `examples/physics/tumbling.ling` (cubes rotated by `rb_rot`).

## Liquid simulation (water + oil)

Two **immiscible** density fields advected by one incompressible velocity grid. The fluids only
blend at their interface, so the physics decides where the blue (water) and amber (oil) colours
mix. Built for speed; `wrap` makes it seamless for mapping onto curved surfaces.

| Builtin | Meaning |
|---|---|
| `liquid_new(w, h)` → id | Create a `w×h` fluid grid. |
| `liquid_splat(id, x, y, kind, amount, radius)` | Add fluid (`kind` 0 = water, 1 = oil) in a disc. |
| `liquid_gravity(id, gx, gy)` | Set the gravity vector (rotate it to slosh). |
| `liquid_step(id, dt)` | Advance the simulation. |
| `liquid_draw(id, x, y, scale)` | Fast flat 2-D blit of the colour field. |
| `liquid_draw_surface(id, kind, cx,cy,cz, radius, height)` | Map/wrap the field onto a 3-D surface — `kind`: 0 plane · 1 sphere · 2 cylinder · 3 cone · 4 dome. |

```ling
bind liq = liquid_new(84.0, 84.0)
liquid_splat(liq, 42.0, 25.0, 1.0, 1.0, 3.0)   # oil
liquid_splat(liq, 42.0, 58.0, 0.0, 1.0, 3.0)   # water
while window_is_open() {
    liquid_gravity(liq, sin(time_now())*55.0, cos(time_now())*55.0)
    liquid_step(liq, 0.016)
    set_camera(cos(time_now()*0.3), sin(time_now()*0.3), cos(0.4), sin(0.4))
    liquid_draw_surface(liq, 1.0, 0.0,0.0,0.0, 2.2, 3.2)   # onto a sphere
    present()
}
```
See `examples/physics/liquid_marble.ling`.
