# Acoustic Bodies

The `body_*` builtins expose `ling-body`, a toolkit that grows a humanoid skin from sound. A body scaffold (a parametric T-pose figure, or an imported model) is voxelised into a signed distance field. Acoustic sources placed on the body excite a pressure wave inside and around it. A thin elastic membrane responds to the wave, and its surface is extracted as a watertight triangle mesh.

The recipe round-trips through WAV files: an exported WAV plays as sound and also carries the parameters and the mesh, so it loads back as the same body. Any other audio file can be analysed into source parameters.

Things to know:

- There is one toolkit per program; every `body_*` call works on it.
- Operations that can fail return `1` for success and `0` for failure, and leave a message you read with `body_message()`.
- Values cross the boundary as numbers, strings and flat lists.
- Units are metres. The mesh is Y-up, the figure faces +Z and its left side is +X.
- Everything except `body_draw` and `body_draw_creases` runs without a window.
- These builtins have no localized names.

## A first body

Set a few parameters, add a source, run the simulation in slices, then read and export the mesh:

```ling
bind start = do {
    body_param_set("sr", 8000)
    body_param_set("steps", 60)
    body_param_set("seal", 1)
    body_src_add(0.0, 1.0, 0.0, 0, 220)
    print("steps: " + str(body_sim_begin()))

    bind progress = [0, 1]
    while progress[0] < progress[1] {
        bind progress = body_sim_step(40)
    }
    print("done: " + str(body_sim_done()))
    print("mesh: " + str(body_mesh_update()) + " " + body_message())
    bind counts = body_mesh_counts()
    print("watertight: " + str(counts[2]))
    print(body_grid_info())
    print("exported: " + str(body_export_mesh("body.obj")) + " " + body_message())
}
```

```output
steps: 60
done: 1
mesh: 1 mesh updated
watertight: 1
109x107x75 @ 41.3 mm
exported: 1 10008 triangles → body.obj
```

## Summary

| Builtin | Purpose |
|---|---|
| `body_message()` | Message from the last operation |
| `body_grid_info()` | Grid size and spacing, such as `"109x107x75 @ 41.3 mm"` |
| `body_notes()` | Notes from the last listening pass or WAV analysis |
| `body_param_get(name)` / `body_param_set(name, v)` | Read or set a parameter |
| `body_param_reset()` | Restore all parameters and remove all sources |
| `body_param_count()`, `body_param_name(i)`, `body_param_group(i)` | List the parameters |
| `body_param_min(i)`, `body_param_max(i)`, `body_param_default(i)` | A parameter's range and default |
| `body_src_add(x, y, z, wave, freq)` | Add a source at a position |
| `body_src_at_joint(joint, wave, freq)` | Add a source at a scaffold joint |
| `body_src_count()`, `body_src_name(i)`, `body_src_del(i)`, `body_src_clear()` | Manage sources |
| `body_src_get(i, field)` / `body_src_set(i, field, v)` | Read or change a source field |
| `body_joint_count()`, `body_joint_name(i)`, `body_joint_pos(i)`, `body_joint_parent(i)` | The scaffold's joints |
| `body_model_load(path)`, `body_model_use(h)`, `body_model_active()` | Use an imported model as the body |
| `body_model_part_count(h)`, `body_model_part_name(h, i)`, `body_model_part_tris(h, i)` | Inspect model parts |
| `body_model_part_enabled(h, i)`, `body_model_part_set(h, i, on)` | Include or exclude a part |
| `body_preview()` | Voxelise and show the rest shape without simulating |
| `body_sim_begin()`, `body_sim_step(ms)` | Start and advance the simulation |
| `body_sim_active()`, `body_sim_done()` | Simulation state |
| `body_mesh_update()` | Extract the membrane as the working mesh |
| `body_displacement()`, `body_slice(axis, frac)`, `body_mic_tail(n)` | Inspect the simulation |
| `body_mesh_counts()`, `body_mesh_bounds()` | Mesh statistics |
| `body_mesh_positions()`, `body_mesh_normals()`, `body_mesh_indices()` | Raw mesh data |
| `body_crease_detect()`, `body_crease_apply()`, `body_detail_reset()` | Carve creases from the sound field |
| `body_crease_count()`, `body_crease_segments()`, `body_crease_clear()`, `body_crease_export(path)` | Work with crease curves |
| `body_listen_begin()`, `body_listen_step(ms)`, `body_listen_done()`, `body_listen_finish(path)` | Record the body's acoustic signature |
| `body_listen_spectrum()` | 48 log-spaced bands of the signature |
| `body_export_mesh(path)`, `body_export_wav(path)`, `body_import_wav(path, analyze_only)` | Files |
| `body_project_save(path)`, `body_project_load(path)` | Save and load the recipe as JSON |
| `body_draw(mode, yaw, x, y, z, scale, extra)`, `body_draw_creases(yaw, x, y, z, scale)` | Draw into the window |

## Parameters

Parameters are a fixed table of 47 named numbers. Each has a group, a default and a range, and setting a value clamps it to the range. `body_param_set` returns `1` for a known name and `0` otherwise. The listing builtins take an index from 0 to 46 or a name.

| Group | Names (defaults) | Meaning |
|---|---|---|
| body | `height` (1.7), `build` (1), `arm_span` (1), `head_scale` (1) | Scaffold proportions; `height` also scales imported models |
| body | `seal` (3) | Voxel radius used to close openings in imported models |
| sim | `sr` (22050), `steps` (800), `margin` (16), `up` (2) | Sample rate, time steps, air margin in cells, membrane grid refinement |
| medium | `c_air` (343), `c_body` (1), `rho_body` (4), `damping` (0.002), `sponge` (8) | Sound speed, body speed and density ratios, loss, absorbing layer |
| mic | `mic_sep` (0.18), `mic_dist` (0.15) | Ear spacing and distance in front of the face |
| membrane | `relief`, `coupling`, `threshold`, `tension`, `stiffness`, `mem_time`, `mem_every`, `reinit_every`, `band`, `drive_mode`, `drive_tau` | How the skin follows the wave |
| mesh | `smooth` (3), `decimate` (0), `iso` (0) | Smoothing passes, vertex-merge cell size in metres, surface level offset |
| audio | `wav_bits` (16), `wav_gain` (0.9), `wav_repeat` (4), `wav_gap` (0.25) | WAV export |
| detail | `crease_mode`, `crease_levels`, `crease_tol`, `crease_min`, `detail_level`, `crease_depth`, `crease_width`, `noise_amp`, `noise_freq` | Crease detection and carving |
| listen | `sig_dur`, `sig_sr`, `sig_rho`, `sig_excite` | The listening pass |

```ling
bind start = do {
    print(body_param_count())
    print(body_param_get("height"))
    print(body_param_set("height", 5))
    print(body_param_get("height"))
    print(body_param_set("nope", 1))
    print(body_joint_count())
    print(body_joint_name(4))
}
```

```output
47
1.7
1
2.4
0
21
head
```

`height` only goes up to 2.4, so setting 5 stores 2.4.

## Sources and joints

A source is an excitation in space. Its fields are `x`, `y`, `z` (metres), `wave` (`0` Ricker wavelet, `1` sine burst, `2` impulse), `freq` (Hz), `amp`, `phase`, `start` and `dur` (seconds).

- `body_src_add(x, y, z, wave, freq)` adds one (defaults 0, 1, 0, wave 0, 220 Hz) and returns its index.
- `body_src_at_joint(joint, wave, freq)` adds one at a named joint and names the source after it. It returns `-1` for an unknown joint.

The scaffold has 21 joints: `hips`, `spine`, `chest`, `neck`, `head`, and `l_` / `r_` versions of `shoulder`, `elbow`, `wrist`, `hand`, `hip`, `knee`, `ankle` and `toe`. `body_joint_pos(i)` returns `[x, y, z]`, fitted to the current body size, and `body_joint_parent(i)` returns the parent index (`-1` for the root).

## Imported models

`body_model_load(path)` loads a `.glb`, `.gltf`, `.lmesh` or `.obj` file and returns a handle (or `-1`). Loading does not select it: call `body_model_use(h)`, or pass a negative handle to go back to the scaffold. glTF files are read for geometry and node transforms only, so files that need texture extensions still load. Accessory parts such as flowers and cubes start disabled; switch parts with `body_model_part_set(h, i, on)`.

## Simulation

The simulation runs in time-boxed slices so a window can keep updating.

- `body_preview()` voxelises the body and extracts its undisturbed shape without simulating.
- `body_sim_begin()` starts a simulation and returns the total number of steps, or `-1`.
- `body_sim_step(ms)` works for about `ms` milliseconds (default 30) and returns `[steps_done, total]`.
- `body_mesh_update()` extracts the membrane as it is now. You can call it while the simulation runs to watch the skin grow.
- `body_displacement()` returns `[inward_mm, outward_mm]`. `body_slice(axis, frac)` returns `[w, h, v0, v1, ...]`, a slice of the pressure field normalised to -1..1. `body_mic_tail(n)` returns the last `n` samples of the first microphone.

## Creases and detail

After a finished simulation, `body_crease_detect()` samples the sound field on the mesh, traces its contour lines and fits them with cubic Bézier curves. `body_crease_apply()` subdivides the mesh and carves a soft groove along every curve, plus fine noise weighted by the field. `body_detail_reset()` restores the smooth mesh. `body_crease_export(path)` writes the curves as `.json`, or as a `.py` Blender script that builds curve objects.

## Listening

`body_listen_begin()` fires an impulse in front of the mouth and records two microphones beside the head. Call `body_listen_step(ms)` until it finishes (it returns `[done, total]`), then `body_listen_finish(path)` writes a WAV of the body's sound and fills `body_listen_spectrum()` and `body_notes()` (the strongest resonances).

## Files

- `body_export_mesh(path)` writes OBJ, STL or GLB, chosen by extension. The GLB carries the T-pose skeleton and skin weights.
- `body_export_wav(path)` writes the stereo microphone signal with two extra chunks holding the full recipe and the compressed mesh.
- `body_import_wav(path, analyze_only)` restores both chunks when present. With `analyze_only` set to 1, or when the chunks are missing, it estimates sources from the audio instead, and `body_notes()` explains each estimate. WAV, FLAC, Ogg and MP3 are accepted.
- `body_project_save(path)` and `body_project_load(path)` keep the parameters, sources and chosen body as JSON.

## Drawing

`body_draw(mode, yaw, x, y, z, scale, extra)` draws the working mesh with the pen colour, flipped into the renderer's y-down space. Modes:

| Mode | Look |
|---|---|
| 0 | Solid, smoothed normals |
| 1 | Wireframe of every `extra`-th triangle |
| 2 | Tinted by skin displacement; `extra` is the full-scale push in millimetres (default 10) |
| 3 | Coloured by surface normal |
| 4 | Single-light clay shading |

Modes 3 and 4 make fine relief easy to judge. `body_draw_creases(yaw, x, y, z, scale)` draws the detected crease curves on top.

This window grows a body while you watch:

```ling-gui
bind start = do {
    open_window(640, 480, "Body")
    body_param_set("steps", 400)
    body_src_at_joint("chest", 1, 180)
    body_sim_begin()
    bind yaw = 0.0
    while window_is_open() {
        if body_sim_done() == 0 {
            body_sim_step(20)
            body_mesh_update()
        }
        fill(16, 16, 24)
        set_color(220, 200, 180)
        body_draw(2, yaw, 0, 1, 4, 1, 10)
        flush_3d()
        present()
        bind yaw = yaw + 0.01
    }
}
```

See also: [Physics](physics.md), [Music](music.md).
