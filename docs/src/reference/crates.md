# Crates & Functions

Ling is a Cargo workspace: the `ling` interpreter is backed by ~two dozen
focused crates under [`crates/`](https://github.com/taellinglin/ling/tree/master/crates).
This page lists every workspace crate, what it does, and its public functions
(generated from each crate's source — the Rust API behind the Ling builtins).

## `ling-ai`  <sub>v2030.0.1</sub>

> Tensor computation, neural networks, and LLM inference

**51 public functions:**

`add` `as_str` `assistant` `backward` `config` `corpus_len` `decode` `dense` `encode` `forward` `forward_cache` `from_bytes` `from_vec` `from_vocab` `get` `id_to_token` `learn` `learn_dataset` `matmul` `meta` `mul` `new` `next_f32` `next_u64` `numel` `ones` `out_width` `parse` `push` `rank` `relu` `say` `set` `softmax` `status` `system` `tick` `to_bytes` `to_chatml` `to_lingdialog` `to_plain` `token_to_id` `train` `train_mse` `training_lines` `turn_count` `uniform` `user` `utterances` `vocab_size` `zeros`

## `ling-animation`  <sub>v2030.0.0</sub>

> Anima — Ling's unified animation system: one rig, any target, any temperament (organic 灵 ↔ mechanical 机)

**61 public functions:**

`add` `add_rig` `add_timeline` `apply` `biped_legs` `blend` `breathe` `breathing_scale` `cam_lift` `duration` `eye_aperture_l` `find` `four_bar` `from_name` `from_translation` `gait_lift` `gait_phase` `gait_swing` `gear` `gear_train` `idle_noise` `is_empty` `is_mechanical` `is_organic` `key` `len` `lerp` `liveliness` `matrix` `new` `normalized` `offset` `pause` `piston` `play` `quadruped_legs` `rack` `reset` `resolve` `rest` `rig` `rig_mut` `rigidity` `sample` `set_local` `skinning_matrices` `solve` `solve_point` `spring_step` `stop` `tick` `tick_all` `timeline` `topology` `tween_ease` `two_bone_fk` `two_bone_ik` `value` `wobble` `world` `world_pos`

## `ling-ast`  <sub>v2030.0.0</sub>

> Abstract Syntax Tree types for the Ling language

**5 public functions:**

`is_empty` `len` `new` `span` `union`

## `ling-audio`  <sub>v2030.0.3</sub>

> 4D positional audio synthesis and WAV BGM for Ling

**34 public functions:**

`add_sample` `beat_ratio` `blip` `clear_tone` `dominant_freq` `eval` `eval_animated` `fire` `freq_bands` `freq_texture` `frequency` `from_name` `fx_delay` `fx_lowpass` `fx_reverb` `is_beat` `load_bgm` `map_bands` `new` `ocean` `play_sample` `psychedelic` `push_samples` `rainbow` `rms` `set_bgm_volume` `set_listener` `set_listener_pos` `set_master_volume` `set_tone` `set_window` `sfx` `stop_sample` `waveform_texture`

## `ling-codegen`  <sub>v2030.0.1</sub>

> Code generation backends for Ling (bytecode, WASM, native)

**4 public functions:**

`compile_mir_program` `load` `new` `run_main`

## `ling-core`  <sub>v2030.0.1</sub>

> Core types, errors, and compiler infrastructure for Ling

_No public free functions (types / re-exports only)._

## `ling-crypto`  <sub>v2030.1.3</sub>

> Post-quantum and classical cryptography for Ling — real ML-KEM-768 (FIPS 203), X25519+ML-KEM hybrid KEM, AES-GCM, XChaCha20, Blake3, SHA3, Argon2id, Shamir over GF(2^8), Ristretto Schnorr ZKP, VRF

**49 public functions:**

`decapsulate` `decrypt` `derive_key` `diffie_hellman` `encapsulate` `encapsulation_key` `encrypt` `encryption_key` `evaluate` `expand` `from_bytes` `from_digest` `from_scalar_bytes` `from_seed` `gather` `generate` `generate_key` `hash` `hash_many` `hash_password` `hkdf_sha3` `holo_hash` `holo_open` `holo_seal` `kem_seed` `key` `keyed_hash` `knot_encapsulate` `knot_for_public_key` `label` `new` `params` `prove` `public_bytes` `public_key` `public_knot` `reconstruct_secret` `scatter` `schnorr_verify` `seed` `sign` `signing_seed` `split_secret` `subkey` `to_bytes` `verify` `verify_password` `vrf_verify` `xof`

## `ling-fu`  <sub>v2030.1.6</sub>

> 灵符 — The Ling project manager. lingfu new/build/run/normalize and more.

**12 public functions:**

`dir_names` `from_code` `from_str` `name` `name_in` `normalize_content` `normalize_project` `sanitize_project_kind` `scaffold_in_dir` `scaffold_project` `to_chinese` `to_english`

## `ling-game`  <sub>v2030.1.1</sub>

> Ling game engine — ECS, physics, and scene management

**61 public functions:**

`add_system` `advance` `apply_force` `checkerboard` `close` `color_cycle` `cube` `cylinder` `despawn` `dot` `eval` `fire` `forest` `freq_map` `get` `get_mut` `gradient` `grid` `halftone` `hyperbolic_patch` `hypnotic_spiral` `icosphere` `index` `insert` `integrate` `into_vec` `is_closed` `is_last_page` `is_typing` `iter` `julia` `len` `mandelbrot` `merge` `mobius` `neon` `new` `noise` `normalise` `normals` `ocean` `overlaps` `pentagon` `positions` `psychedelic` `quit` `rainbow` `register` `remove` `rgba` `rgba_alpha` `ripple` `run` `set` `set_fps` `spawn` `sphere` `torus` `update` `visible_runs` `voronoi`

## `ling-gpu`  <sub>v2030.0.1</sub>

> GPU compute backend for Ling (CUDA via cudarc, with a portable CPU fallback)

**4 public functions:**

`backend` `device_name` `gpu_active` `new`

## `ling-graphics`  <sub>v2030.0.4</sub>

> 3D/4D rendering, geometry, animation, and font tools for the Ling ecosystem

**132 public functions:**

`add` `add_child` `add_node` `add_root` `add_triangle` `add_vertex` `advance` `apply` `ascent` `at` `blend` `blend_pixel` `build` `cel_ramp` `center` `clamp` `clear` `collect_render_items` `compute_normals` `compute_tangents` `cone` `contains` `contains_aabb` `contains_point` `contains_sphere` `cube` `cylinder` `descent` `distance` `dot` `draw_text` `duration` `expand` `forward` `from_bytes` `from_color` `from_hex` `from_path` `from_path_weight` `from_point_normal` `from_points` `from_rotation` `from_scale` `from_srgb` `from_srgba` `from_three_points` `from_translation` `from_trs` `from_view_proj` `generate_text_mesh` `get_or_rasterize` `get_pixel` `global_matrix` `global_transform` `glyph_outline` `half_extents` `icosphere` `identity` `intersect_aabb` `intersect_plane` `intersect_sphere` `length` `lerp` `lit_vertex` `look_at` `luminance` `matrix` `measure` `measure_text` `minkowski_dot` `move_by` `move_forward` `move_right` `move_up` `mul` `mul_vec` `new` `node` `node_mut` `normalize` `normalized_time` `orbit` `orthographic` `pack` `pause` `perspective` `plane` `play` `posterize` `premultiply_alpha` `present_terminal` `project` `projection_matrix` `pyramid` `render_scene` `right` `roots` `run` `sample` `sample_albedo` `set_light` `set_pixel` `signed_distance` `size` `spawn_mesh` `sphere` `stop` `tick` `to_klein` `to_poincare` `to_rgba_bytes` `torus` `translation_4d` `transpose` `triangle_count` `union` `unpack` `unproject_ray` `up` `view_matrix` `view_proj` `with_alpha` `with_color` `with_emissive` `with_material` `with_mesh` `with_metallic_roughness` `with_terminal_dimensions` `with_texture` `with_transform` `xyz` `zero`

## `ling-icon`  <sub>v2030.0.1</sub>

> Rasterize SVG/PNG icons to multi-resolution .ico and embed them in Windows executables

**4 public functions:**

`embed_in_build` `ico_bytes` `rasterize_and_embed` `write_ico`

## `ling-input`  <sub>v2030.0.1</sub>

> Sensorium — Ling's unified input system: gamepads, joysticks, haptics, motion, XR controllers, and themeable on-screen touch controls (灵 organic ↔ 机 mechanical)

**104 public functions:**

`active` `active_count` `add` `analog` `apply` `assign_player` `axis` `axis2d` `begin` `begin_frame` `bind` `button` `button_mut` `cancel` `center` `click` `contains` `contains_xz` `count` `current_opacity` `define` `delta` `disconnect` `dpad` `drift` `duration` `end` `events` `faded` `floating` `for_player` `forward` `from_center_size` `from_degrees` `gamepad` `get` `get_mut` `glyph` `gyro_aim` `hand` `hand_mut` `hat` `heartbeat` `high_contrast` `ingest` `is_active` `is_down` `is_playing` `is_silent` `iter` `joint` `just_pressed` `just_released` `lerp` `ling` `map` `map_mut` `max` `minimal` `modern_gamepad` `mouse` `move_by` `move_to` `moved` `neon` `new` `of_kind` `pick` `play` `player` `pop` `pressed` `prune` `pulse` `pump` `push` `push_text` `ray` `recenter` `right` `sample` `scaled` `scroll` `set` `set_axis` `set_button` `set_held` `stick` `stop` `tick` `to_f32` `transform_point` `trigger` `twin_stick` `up` `update` `update_analog` `update_buttons` `value` `vector` `with_keyboard` `with_layout` `with_mouse` `xr_controller`

## `ling-lex`  <sub>v2030.0.0</sub>

> Lexical analysis and polyglot tokenization for Ling

**9 public functions:**

`into_tokens` `is_keyword` `is_literal` `is_operator` `is_type` `new` `next_token` `peek` `pos`

## `ling-macro`  <sub>v2030.0.0</sub>

> Procedural macros for the Ling language ecosystem

**1 public functions:**

`derive_ling_type`

## `ling-mic`  <sub>v2030.0.2</sub>

> Microphone input and real-time audio capture for Ling

**7 public functions:**

`latest_samples` `open` `peak` `rms` `sample_rate` `start` `stop`

## `ling-mir`  <sub>v2030.0.1</sub>

> Ling MIR - Mid-level Intermediate Representation for codegen backends

**7 public functions:**

`clone_blocks` `compute` `find_loops` `inline_function` `is_move_type` `new` `run`

## `ling-music`  <sub>v2030.0.1</sub>

> 2030-ready music toolkit for Ling — decode, BPM/key analysis, GM-capable synthesis, rhythm-game & karaoke tools

**53 public functions:**

`accuracy` `active_voices` `add_patch` `all_notes_off` `beat_grid` `bpm` `chroma` `detect` `detect_range` `duration` `from_beats` `from_bytes` `from_index` `from_name` `from_path` `from_str` `hz_to_midi` `hz_to_name` `index` `index_at` `judge` `key` `key_name` `line_at` `load` `midi_to_hz` `midi_to_name` `name` `name_to_hz` `name_to_midi` `new` `next_sample` `note` `note_off` `note_off_pitch` `note_on` `onset_envelope` `onsets` `parse` `parse_pitch` `pause` `pitch_class_name` `pitch_score` `play` `position` `register` `rms` `scale_steps` `seek` `set_track` `set_track_volume` `set_volume` `stop`

## `ling-net`  <sub>v2030.0.0</sub>

> Async networking — HTTP, WebSocket, QUIC, and peer-to-peer

**56 public functions:**

`add` `append` `apply_delta` `approx_eq` `as_slice` `as_str` `at` `blob` `bool` `clear` `contains` `count` `delay` `delta_from` `expect_end` `finish` `fixed` `from_bytes` `from_seed` `generate` `get` `hash_snapshot` `head` `header` `is_empty` `is_success` `iter` `keyframe` `latest_tick` `len` `lerp` `matches_local` `new` `ok` `post` `public_key` `push` `quat` `raw` `remaining` `sample` `server_public_key` `set` `set_delay` `sign` `sign_snapshot` `stats_of` `str` `text` `to_bytes` `vec3` `verify` `with` `with_capacity` `with_genesis` `with_stats`

## `ling-physics`  <sub>v2030.1.8</sub>

> Physics, world generation, and hyperbolic geometry for Ling

**142 public functions:**

`aabb_max` `aabb_min` `acceleration_from_force` `acceleration_from_force_4d` `add` `add_room` `advance` `ambient_light` `angle_between` `angle_between_4d` `angular_speed` `angular_velocity` `apply_force` `apply_impulse` `apply_impulse_at` `apply_spin` `apply_torque` `bounce_floor` `bounce_velocity` `bounce_velocity_4d` `buoyancy_force` `centripetal_force` `centroid` `clamp_magnitude` `clamp_magnitude_4d` `contain` `cross` `damping_force` `damping_force_4d` `day_fraction` `deformation` `directional_light` `distance` `distance_4d` `distance_4d_squared` `distance_squared` `dot` `dot_4d` `drag_force` `drag_force_4d` `eval_node_transform` `exp_map` `fbm` `find` `floor_collision` `fog_color` `from_poincare` `gen_grass` `generate` `get_chunk` `get_height` `gravitational_attraction` `gravitational_attraction_4d` `gravity_dir` `gravity_force` `gravity_force_4d` `gust` `integrate` `is_day` `is_raining` `is_snowing` `joint_matrices` `kick` `lambda` `lerp` `lerp_4d` `load` `load_chunk` `log_map` `lorentz_force` `magnitude` `magnitude_4d` `mean_velocity` `mix_amount` `mobius_add` `moon_direction` `nbody_accel` `new` `normalize` `normalize_4d` `oil_at` `overlaps` `parallel_transport` `pentagon_tiling` `pentagon_vertices` `perpendicular_2d` `place` `portal_at` `pressure_force` `project` `project_4d` `quat_from_euler` `quat_mul` `reflect` `reflect_4d` `refresh_inertia` `reject` `reject_4d` `remove` `repulsive_force` `repulsive_force_4d` `resolve_collision` `rotate_axis_angle` `rotation_4d_from_axis_angle` `sample` `sample_height` `sample_rgb` `set_colors` `set_gravity` `signed_area_2d` `sky_color` `smooth_lerp` `smooth_lerp_4d` `spawn_entity` `sphere` `spin` `splat` `spring_force` `spring_force_4d` `step` `sun_direction` `terminal_velocity` `to_poincare` `total_oil` `total_water` `transform` `transform_dir` `transform_vec3` `translate_pentagon` `unit_cube` `up_at` `update` `update_lod` `vec3_to_vec4` `vec4_to_vec3` `viscous_drag` `water_at` `wind_deform` `wind_force` `wind_vector` `world_distance` `world_origin`

## `ling-polyglot`  <sub>v2030.0.0</sub>

> Polyglot lexicon system - 16+ human languages

**12 public functions:**

`as_str` `builtin_ja` `builtin_ko` `builtin_th` `builtin_zh` `detect_language` `from_str` `insert` `new` `to_canonical` `to_native` `translate_keyword`

## `ling-py`  <sub>v2030.0.0</sub>

> Python bindings for ling-audio, ling-physics, and ling-net

_No public free functions (types / re-exports only)._

## `ling-runtime`  <sub>v2030.0.0</sub>

> Ling language runtime — GC, allocator, and standard library

**7 public functions:**

`into_inner` `ling_debug` `ling_panic` `ling_print` `ling_sleep_ms` `ling_time` `new`

## `ling-ui`  <sub>v2030.0.1</sub>

> Ling UI framework — retained-mode widgets and flex layout

**54 public functions:**

`add` `apply` `bar` `battery` `beveled_rect` `button` `checkbox` `clear` `compass` `cooldown` `corner_brackets` `counter` `done` `dpad` `ease` `from_name` `gauge` `gauge3d` `glyph` `healthbar` `hit_rect` `is_settled` `minimap` `mix` `new` `panel` `panel3d` `pixels` `progress` `radar` `radar3d` `render_widget` `reticle` `ring` `scanlines` `segbar` `set_target` `shade` `simple_layout` `slider` `slotgrid` `spark` `stepper` `tabs` `target` `text_lines` `text_width` `toggle` `tooltip` `ui` `update` `value` `vignette` `vu`

## `ling-wasm`  <sub>v2030.0.8</sub>

> Ling language — WebGL2 WASM runtime

**2 public functions:**

`init_canvas` `run_program`

