// Data tables for the Ling language: keywords, types, constants, and builtins.
// Sourced from lexicons/en.ling and src/lexer/mod.rs. Pure data — no vscode deps.

// Canonical English keywords with a one-line description.
const keywords = {
  bind:   "Bind a value or function to a name.",
  do:     "Block expression: `do { ... }`.",
  fn:     "Function type / lambda.",
  mod:    "Declare a module.",
  type:   "Declare a type.",
  use:    "Import a module.",
  if:     "Conditional branch.",
  else:   "Alternative branch.",
  while:  "Loop while a condition holds.",
  for:    "Iterate over a range or collection.",
  in:     "Iteration source: `for x in 0..10`.",
  match:  "Pattern match.",
  return: "Return a value from a function.",
  post:   "Return a value (idiomatic Ling form of `return`).",
  give:   "Yield / return a value.",
  again:  "Continue the loop.",
  stop:   "Break out of the loop.",
  own:    "Ownership modifier: take ownership.",
  lend:   "Ownership modifier: borrow immutably.",
  share:  "Ownership modifier: shared reference.",
  move:   "Ownership modifier: move value.",
  copy:   "Ownership modifier: copy value.",
  async:  "Mark a function/block as asynchronous.",
  wait:   "Await an async value.",
  spawn:  "Spawn a concurrent task.",
  as:     "Type cast / alias.",
  where:  "Constraint clause.",
  fit:    "Pattern / refinement.",
  form:   "Construct a form/struct.",
  choose: "Selection construct.",
  can:    "Capability / permission.",
  change: "Mutate a binding.",
  try:    "Fallible expression.",
  sure:   "Assert success.",
  maybe:  "Optional value.",
  pure:   "Mark as pure (no side effects).",
  start:  "Program entry point: `bind start = do { ... }`.",
  result: "Result value."
};

const types = {
  number: "Numeric type (integer or float).",
  text:   "Text / string type.",
  bool:   "Boolean type (true/false).",
  list:   "Ordered list type.",
  map:    "Key/value map type.",
  tuple:  "Fixed-size tuple type."
};

const constants = {
  true:  "Boolean true.",
  false: "Boolean false.",
  ok:    "Success result.",
  bad:   "Error result.",
  none:  "Absent / null value."
};

// Builtins grouped by category. The category string becomes the completion `detail`.
const categories = [
  ["I/O", ["print", "debug", "panic"]],
  ["Window / graphics", ["open_window", "open_fullscreen", "fill", "clear", "set_color", "set_color_hsl", "present", "draw_line", "draw_pixel", "triangle", "draw_rect", "draw_circle", "draw_text", "draw_disc", "draw_poly", "window_is_open", "get_width", "get_height"]],
  ["Monitor / framerate", ["monitor_width", "monitor_height", "monitor_refresh", "monitor_info", "set_fps"]],
  ["Input", ["key_down", "key_pressed", "mouse_x", "mouse_y", "mouse_dx", "mouse_dy", "mouse_button", "capture_mouse"]],
  ["3-D camera / mesh", ["camera_pos", "camera_look", "camera_fov", "draw_mesh", "draw_triangle_3d", "draw_line_3d", "set_camera", "set_camera_pos", "set_zdist", "set_projection", "project_3d"]],
  ["Lighting", ["set_ambient", "add_light", "clear_lights"]],
  ["Render / shading", ["set_shade_mode", "set_cel_bands", "set_shadow_color", "set_rim", "set_blend"]],
  ["3-D primitives", ["cube", "box", "sphere", "icosphere", "dome", "cylinder", "cone", "capsule", "torus", "pyramid", "prism", "frustum", "tetrahedron", "octahedron", "dodecahedron", "icosahedron", "gear", "gyro", "helix", "spring", "arch", "stairs", "star_prism", "capsule_chain", "mobius"]],
  ["Texture patterns (vtex)", ["vtex_grid", "vtex_rings", "vtex_star", "vtex_spiral", "vtex_flower", "vtex_lotus", "vtex_chakra", "vtex_yantra", "vtex_letter_rain", "vtex_hyperbolic_uv", "vtex_tessellated", "vtex_halftone", "vtex_spiked_cog", "vtex_torii", "vtex_pagoda"]],
  ["Audio", ["tone", "play_wav", "stop_audio", "set_volume", "mic_open", "mic_peak", "mic_rms", "mic_fft", "audio_tone", "audio_volume", "audio_listener", "audio_bgm", "audio_bgm_volume", "audio_blip", "audio_sfx", "audio_sample_load", "audio_sample_play", "audio_sample_stop", "audio_fx_delay", "audio_fx_reverb", "audio_fx_lowpass"]],
  ["Music toolkit", ["music_load", "music_duration", "music_bpm", "music_key", "music_onsets", "music_beat_grid", "music_play", "music_pause", "music_stop", "music_seek", "music_pos", "music_volume", "music_patch", "music_note", "music_note_on", "music_note_off", "music_judge", "music_grade_name", "music_lrc", "music_lyric", "music_mic_pitch", "music_note_name", "music_hz", "music_pitch_score", "music_midi_load", "music_midi_count", "music_midi_notes", "music_fft", "music_midi_bars"]],
  ["Color helpers", ["hsl_color", "hsv_to_rgb", "lerp_color"]],
  ["SVG export", ["svg_begin", "svg_rect", "svg_circle", "svg_line", "svg_polyline", "svg_text", "svg_end"]],
  ["Math", ["trunc", "int", "sin", "cos", "tan", "sqrt", "abs", "floor", "ceil", "round", "pow", "log", "min", "max", "clamp", "pi", "tau", "lerp", "smoothstep", "rand", "sign"]],
  ["Noise", ["vnoise", "fbm", "perlin"]],
  ["Time", ["time_now", "frame_count", "time", "sleep"]],
  ["Collections", ["len", "push", "pop", "new_list", "new_map", "keys", "values", "contains"]],
  ["Network", ["http_get", "http_post", "ws_connect"]],
  ["AI", ["ai_chat", "ai_embed", "nn_new", "nn_dense", "nn_forward", "nn_train", "nn_save", "nn_load", "bt_build", "bt_set", "bt_tick", "bt_status", "dialog_new", "dialog_learn", "dialog_load", "dialog_train", "dialog_say", "dialog_save", "dialog_load_model"]],
  ["Crypto", ["hash", "encrypt", "decrypt", "sign", "verify", "crypto_hash", "knot_points", "knot_label", "knot_keygen", "knot_public", "knot_encapsulate", "knot_decapsulate", "crypto_seal", "crypto_open", "holo_points", "holo_fragment_count"]],
  ["Physics — vectors", ["physics_normalize", "physics_dot", "physics_cross", "physics_distance", "physics_magnitude", "physics_lerp", "physics_angle", "physics_project", "physics_reflect"]],
  ["Physics — forces", ["physics_gravity", "physics_drag", "physics_spring", "physics_damping", "physics_attraction", "physics_repulsion", "physics_buoyancy", "physics_wind", "physics_bounce", "physics_accel"]],
  ["Physics — bodies", ["soft_ball", "soft_step", "soft_bounce", "soft_contain", "soft_kick", "soft_deform", "soft_centroid", "soft_nodes", "soft_spin", "soft_angular_speed", "rb_add", "rb_torque", "rb_spin", "rb_impulse", "rb_floor", "rb_gravity", "rb_step", "rb_pos", "rb_rot", "liquid_new", "liquid_splat", "liquid_gravity", "liquid_step", "liquid_draw", "liquid_draw_surface", "liquid_rainbow", "liquid_mix", "orb_shell", "orb_particles", "sparkle"]],
  ["UI toolkit", ["ui_theme", "ui_text", "ui_frame", "ui_bevel", "ui_hot", "ui_radar", "ui_compass", "ui_reticle", "ui_target", "ui_panel", "ui_scanlines", "ui_bar", "ui_segbar", "ui_gauge", "ui_ring", "ui_vu", "ui_spark", "ui_battery", "ui_button", "ui_toggle", "ui_slider", "ui_checkbox", "ui_tabs", "ui_progress", "ui_tooltip", "ui_stepper", "ui_healthbar", "ui_cooldown", "ui_counter", "ui_minimap", "ui_dpad", "ui_slotgrid", "ui_vignette", "ui_gauge3d", "ui_panel3d", "ui_radar3d", "ui_sound"]],
  ["Vector fonts", ["font_load", "font_text", "font_text_fill", "font_text_3d", "font_width"]],
  ["Dialog", ["dialog_show", "dialog_step", "dialog_advance", "dialog_active", "dialog_typing", "dialog_close", "dialog_color", "dialog_draw"]]
];

// Curated call signatures for the most-used builtins.
const signatures = {
  print: "print(...args)",
  debug: "debug(value)",
  panic: "panic(message: text)",
  open_window: "open_window(width: number, height: number, title: text)",
  open_fullscreen: "open_fullscreen(title: text)",
  fill: "fill(r: number, g: number, b: number)",
  clear: "clear()",
  set_color: "set_color(r: number, g: number, b: number)",
  set_color_hsl: "set_color_hsl(h: number, s: number, l: number)",
  present: "present()",
  draw_line: "draw_line(x0: number, y0: number, x1: number, y1: number)",
  draw_pixel: "draw_pixel(x: number, y: number)",
  draw_rect: "draw_rect(x: number, y: number, w: number, h: number)",
  draw_circle: "draw_circle(cx: number, cy: number, radius: number)",
  draw_disc: "draw_disc(cx: number, cy: number, radius: number)",
  draw_text: "draw_text(x: number, y: number, text: text)",
  triangle: "triangle(x0, y0, x1, y1, x2, y2)",
  window_is_open: "window_is_open() -> bool",
  get_width: "get_width() -> number",
  get_height: "get_height() -> number",
  set_fps: "set_fps(fps: number)",
  key_down: "key_down(key: text) -> bool",
  key_pressed: "key_pressed(key: text) -> bool",
  mouse_x: "mouse_x() -> number",
  mouse_y: "mouse_y() -> number",
  set_camera: "set_camera(x, y, z, look_x, look_y, look_z)",
  add_light: "add_light(x, y, z, r, g, b)",
  set_ambient: "set_ambient(r: number, g: number, b: number)",
  cube: "cube(cx, cy, cz, sx, sy, sz, rx, ry, rz, mode, e0, e1, e2)",
  sphere: "sphere(cx, cy, cz, sx, sy, sz, rx, ry, rz, mode, e0, e1, e2)",
  torus: "torus(cx, cy, cz, sx, sy, sz, rx, ry, rz, mode, e0, e1, e2)",
  sin: "sin(x: number) -> number",
  cos: "cos(x: number) -> number",
  tan: "tan(x: number) -> number",
  sqrt: "sqrt(x: number) -> number",
  abs: "abs(x: number) -> number",
  floor: "floor(x: number) -> number",
  ceil: "ceil(x: number) -> number",
  round: "round(x: number) -> number",
  pow: "pow(base: number, exp: number) -> number",
  log: "log(x: number) -> number",
  min: "min(a: number, b: number) -> number",
  max: "max(a: number, b: number) -> number",
  clamp: "clamp(x: number, lo: number, hi: number) -> number",
  lerp: "lerp(a: number, b: number, t: number) -> number",
  smoothstep: "smoothstep(edge0: number, edge1: number, x: number) -> number",
  rand: "rand() -> number",
  sign: "sign(x: number) -> number",
  pi: "pi -> number",
  tau: "tau -> number",
  len: "len(collection) -> number",
  push: "push(list, value)",
  pop: "pop(list) -> value",
  new_list: "new_list() -> list",
  new_map: "new_map() -> map",
  keys: "keys(map) -> list",
  values: "values(map) -> list",
  contains: "contains(collection, value) -> bool",
  tone: "tone(freq: number, duration: number)",
  time_now: "time_now() -> number",
  frame_count: "frame_count() -> number",
  sleep: "sleep(seconds: number)",
  http_get: "http_get(url: text) -> text",
  http_post: "http_post(url: text, body: text) -> text",
  ai_chat: "ai_chat(prompt: text) -> text"
};

// Flat builtin -> category lookup.
const builtinCategory = {};
for (const [cat, items] of categories) {
  for (const name of items) builtinCategory[name] = cat;
}
const builtins = Object.keys(builtinCategory);

module.exports = { keywords, types, constants, categories, signatures, builtins, builtinCategory };
