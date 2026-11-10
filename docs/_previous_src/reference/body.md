# Builtins — Acoustic Body (`body_*`)

`ling-body` builds humanoid skin from sound. A body scaffold (a parametric T-pose
figure, or a glTF/GLB, `.lmesh` or OBJ mesh) is voxelised into a signed distance field.
Acoustic sources excite a finite-difference pressure field around and inside it, a thin
elastic level-set membrane responds to the field, and its zero level set is extracted as
a watertight mesh. Bodies round-trip through WAV files, and any audio can be analysed
back into source parameters.

Operations that can fail return `1` or `0` and leave their text for `body_message()`.
Y is up and units are metres; the left side of the body is +X and it faces +Z.

## Parameters

`body_param_get(name)` · `body_param_set(name, v)` (clamped to the declared range, returns
`1` for a known name) · `body_param_reset()` · `body_param_count()` ·
`body_param_name(i)` · `body_param_group(i|name)` · `body_param_min/max/default(i|name)`.

| Group | Name | Default | Meaning |
|-------|------|---------|---------|
| body | `height` `build` `arm_span` `head_scale` | 1.7 1 1 1 | scaffold proportions; `height` also scales imported meshes |
| body | `seal` | 3 | voxel radius used to close open meshes before filling |
| sim | `sr` | 22050 | sample rate; sets the time step and the wave-grid spacing |
| sim | `steps` `margin` `up` | 800 16 2 | time steps, air margin in cells, membrane grid refinement |
| medium | `c_air` `c_body` `rho_body` | 343 1 4 | sound speed, body speed ratio, body density ratio |
| medium | `damping` `sponge` | 0.002 8 | per-step loss, absorbing layer width in cells |
| mic | `mic_sep` `mic_dist` | 0.18 0.15 | ear spacing and distance in front of the face |
| membrane | `relief` `coupling` `threshold` | 0.008 1 0 | push per unit drive (m), gain, drive offset |
| membrane | `tension` `stiffness` | 0.5 1 | lateral spreading and restoring force |
| membrane | `mem_time` `mem_every` `reinit_every` `band` | 3 8 20 6 | settle time, update cadence, redistancing, narrow band |
| membrane | `drive_mode` `drive_tau` | 1 0.01 | 1 = field energy, 0 = instantaneous pressure; smoothing |
| mesh | `smooth` `decimate` `iso` | 3 0 0 | Taubin passes, vertex-cluster size (m), level offset |
| audio | `wav_bits` `wav_gain` `wav_repeat` `wav_gap` | 16 0.9 4 0.25 | export format and pulse-train rendering |
| detail | `crease_mode` `crease_levels` `crease_tol` `crease_min` | 0 3 2 60 | energy contours or nodal lines, contour count, Bézier fit tolerance (mm), shortest curve (mm) |
| detail | `detail_level` `crease_depth` `crease_width` `noise_amp` `noise_freq` | 1 3.5 5 1.2 60 | subdivisions, groove depth and half width (mm), noise amplitude (mm) and frequency (per m) |
| listen | `sig_dur` `sig_sr` `sig_rho` `sig_excite` | 0.5 16000 40 1 | length (s), bandwidth (Hz), body density ratio, excitation 0 impulse / 1 knock / 2 voice |

## Sources and joints

`body_src_add(x, y, z, wave, freq)` → index · `body_src_at_joint(joint, wave, freq)` ·
`body_src_del(i)` · `body_src_clear()` · `body_src_count()` · `body_src_name(i)` ·
`body_src_get(i, field)` · `body_src_set(i, field, v)` with fields `x y z wave freq amp phase
start dur`. Waveforms: `0` Ricker wavelet, `1` sine burst, `2` impulse.

`body_joint_count()` · `body_joint_name(i)` · `body_joint_pos(i)` → `[x, y, z]` ·
`body_joint_parent(i)`. The 21 scaffold joints are `hips spine chest neck head` and
`l_/r_` `shoulder elbow wrist hand hip knee ankle toe`.

## Bodies

`body_model_load(path)` → handle or `-1` (GLB/glTF, `.lmesh`, `.obj`) ·
`body_model_use(handle)` (negative selects the scaffold) · `body_model_active()` ·
`body_model_part_count(h)` · `body_model_part_name/tris/enabled(h, i)` ·
`body_model_part_set(h, i, on)`. Accessory parts such as flowers and cubes start disabled.
glTF files are read for geometry and node transforms only, so files that require texture
extensions still load.

## Simulation

`body_preview()` voxelises the body and extracts its rest shape. `body_sim_begin()` → total
steps (or `-1`), then call `body_sim_step(ms)` each frame; it runs about that long and
returns `[steps_done, total]`. `body_sim_active()` · `body_sim_done()` · `body_mesh_update()`
extracts the membrane as it is now. `body_displacement()` → `[inward_mm, outward_mm]` ·
`body_slice(axis, frac)` → `[w, h, v0, v1, …]` pressure normalised to ±1 ·
`body_mic_tail(n)` → the last `n` samples of the left microphone.

## Creases and detail

`body_crease_detect()` needs a finished simulation. It samples the acoustic field on the mesh and
traces its contour lines (energy contours at `crease_levels` quantiles, or the zero line of the
pressure with `crease_mode = 1`), then fits each line with cubic Béziers (Schneider's
algorithm, tolerance `crease_tol`). `body_crease_count()` · `body_crease_segments()` ·
`body_crease_apply()` subdivides the mesh `detail_level` times, snaps the new vertices onto the
smooth surface, and carves a groove with soft lips along every curve plus field-weighted
fractal noise · `body_detail_reset()` restores the smooth mesh · `body_crease_clear()` ·
`body_crease_export(path)` writes `.json` (control points) or `.py` (a Blender script that
builds Bézier curve objects) · `body_draw_creases(yaw, ox, oy, oz, scale)`.

## Listening

`body_listen_begin()` voxelises the current mesh as a dense scatterer, fires an impulse in front
of the mouth and records two microphones beside the head. Call `body_listen_step(ms)` each frame
(returns `[done, total]`), then `body_listen_finish(path)` convolves the response with the
chosen excitation, writes a WAV and updates `body_listen_spectrum()` (48 log-spaced bands) and
`body_notes()` (the strongest resonances). `body_listen_done()`.

## Mesh and drawing

`body_mesh_counts()` → `[vertices, triangles, watertight, restored_from_wav]` ·
`body_mesh_bounds()` · `body_mesh_positions()` · `body_mesh_normals()` ·
`body_mesh_indices()`.

`body_draw(mode, yaw, ox, oy, oz, scale [, arg])` draws the mesh with the pen colour, flipped
into the renderer's Y-down space. Mode `0` is solid with smoothed normals, `1` is edges of
every `arg`-th triangle, `2` is solid tinted by skin displacement with `arg` as the full-scale
push in millimetres, mode `3` colours triangles by surface normal and mode `4` is single-light clay shading; both reveal fine relief.

## Files

`body_export_mesh(path)` writes OBJ, STL or GLB by extension; the GLB carries the T-pose
skeleton and envelope skin weights. `body_export_wav(path)` writes the stereo microphone
pressure, repeated `wav_repeat` times, with two private RIFF chunks: `LBDY` (all parameters
and sources as JSON) and `LBMS` (the mesh, quantised, delta-coded and deflated).
`body_import_wav(path, analyze_only)` restores both chunks when present; with
`analyze_only = 1`, or when the chunks are missing, it estimates sources from the audio
(`body_notes()` explains each estimate): spectral peaks become sources placed by
log-frequency (height) and stereo balance (left/right), onset and decay set timing and
damping, and brightness and loudness set membrane tension and coupling. WAV, FLAC, Ogg and
MP3 are accepted. `body_project_save(path)` / `body_project_load(path)` keep parameters,
sources and the chosen body as JSON.
