// src/gfx/mod.rs — unified graphics state + sub-modules.
//
// Sub-modules
//   raster   — pixel-level fill_triangle / draw_line
//   camera   — Camera3D: rotation + world→screen projection
//   light    — Light struct + cel-shade quantiser
//   depth    — DepthQueue: deferred draw accumulator
//   poly     — EdgeSet (shared-edge dedup) + fan triangulation
//   material — LingMaterial: principled BSDF + toon quantisation
//   photon   — PhotonBuf: water-photon HDR accumulation
//   toon     — Screen-space post-process (outlines, shadow edges, highlights)
//   vtex     — vector texture primitives
//   webgl    — WebGL2 backend (wasm32 only)

#[cfg(target_arch = "wasm32")]
pub mod audio_web;
pub mod camera;
pub mod color;
pub mod depth;
pub mod light;
pub mod material;
pub mod photon;
pub mod poly;
pub mod raster;
pub mod shapes;
pub mod toon;
pub mod vtex;
#[cfg(target_arch = "wasm32")]
pub mod webgl;
#[cfg(feature = "gpu")]
pub mod wgpu_raster;

pub use camera::Camera3D;
pub use depth::DepthQueue;
pub use light::Light;
pub use material::LingMaterial;
pub use toon::ToonConfig;

/// Framebuffer pixels are 0x00RRGGBB; bit 24 tags unlit line/text ink so the
/// toon post-process leaves it exact instead of cel-quantising it.
pub const UNLIT: u32 = 0x0100_0000;
pub const RGB_MASK: u32 = 0x00FF_FFFF;

/// Tunable mapping for `cast_shadow`: how a blob/contact shadow's size and
/// opacity change with the caster's height above the surface. Defaults give the
/// natural look — small/dark/sharp when the caster touches down, growing larger,
/// fainter and softer as it rises. Pass a negative `fade` to invert the opacity
/// ramp (fainter when close, more opaque when far).
#[derive(Clone, Copy)]
pub struct ShadowParams {
    /// Radius (px) when the caster sits on the surface (height 0).
    pub base: f32,
    /// Extra radius per unit of height — the shadow grows as the caster rises.
    pub grow: f32,
    /// Opacity at height 0 (0..1) — darkest/sharpest when touching the surface.
    pub alpha: f32,
    /// Opacity lost per unit of height — the shadow fades as the caster rises.
    pub fade: f32,
    /// Edge softness 0..1 at height 0 — feathering also widens with height.
    pub soft: f32,
}

impl Default for ShadowParams {
    fn default() -> Self {
        Self { base: 14.0, grow: 0.6, alpha: 0.55, fade: 0.012, soft: 0.45 }
    }
}

// ─── Native GfxState (minifb window + software framebuffer) ──────────────────

#[cfg(not(target_arch = "wasm32"))]
pub struct GfxState {
    pub window: Option<minifb::Window>,
    pub buffer: Vec<u32>,
    /// Reusable scratch for `distort()` — avoids an 8 MB clone+alloc every frame.
    pub distort_buf: Vec<u32>,
    pub width: usize,
    pub height: usize,
    /// Current pen colour (0x00RRGGBB) set by `สีดินสอ` / `set_color`.
    pub color: u32,
    /// 3-D camera — set once per frame with `set_camera`.
    pub camera: Camera3D,
    /// Active point lights for this frame — cleared by `clear_lights`.
    pub lights: Vec<Light>,
    /// Ambient fill level [0..1].  Default 0.15.
    pub ambient: f32,
    /// Depth-sorted draw queue — flushed by `แสดงผล` / `present`.
    pub depth_queue: DepthQueue,
    /// Mouse position delta since last frame (pixels).
    pub mouse_dx: f32,
    pub mouse_dy: f32,
    /// Previous mouse position for delta computation; NaN = no prior sample.
    pub last_mx: f32,
    pub last_my: f32,
    /// When true: cursor is hidden and reset to center every frame for infinite rotation.
    pub mouse_captured: bool,
    /// Shading mode for 3-D shape meshes: 0 flat · 1 cel · 2 holo (default).
    pub shade_mode: u8,
    /// Tunable cel/holo parameters (bands, shadow tint, rim, …).
    pub shade: ling_graphics::shading::ShadeParams,
    /// Blend mode for pixel writes: 0 = normal (overwrite), 1 = additive.
    pub blend: u8,
    /// Pen opacity [0..1] for the alpha-blended fills (gradient surfaces,
    /// shadow blobs). Set by `set_alpha`; 1.0 = fully opaque.
    pub alpha: f32,
    /// Anti-alias wireframe strokes (lines / edges / arcs / circle outlines).
    /// Set by `set_antialias`; default false = crisp, opaque, aliased pixels.
    /// When true, strokes use Xiaolin-Wu coverage blending for smooth edges.
    pub antialias: bool,
    /// Anti-alias `font_text`/`font_text_fill` glyphs, independent of
    /// `antialias` (a game may want crisp pixel-hinted UI text while still
    /// smoothing wireframe strokes). Set by `set_font_antialias`; default
    /// false = crisp, hard-edged glyphs, matching the engine-wide default of
    /// aliased-unless-opted-in.
    pub font_antialias: bool,
    /// Hue rotation (radians) applied to baked per-tri colours in
    /// `draw_color_mesh` (.lmesh). Set by `mesh_hue`; 0 = colours as-is.
    pub mesh_hue: f32,
    /// Brightness gain applied with the hue rotation (2nd arg of `mesh_hue`).
    pub mesh_hue_gain: f32,
    /// Frame accumulation (afterimage trails): blend of the previous presented
    /// frame into the current one at present time. 0 = off. Set by `set_frame_blur`.
    pub frame_blur: f32,
    /// Previous presented frame for `frame_blur` (lazily sized).
    pub prev_frame: Vec<u32>,
    /// Tunable height→size/opacity mapping for `cast_shadow`.
    pub shadow: ShadowParams,
    /// Gamma-correct compositing: blend alpha/gradients in linear light instead
    /// of sRGB. Set by `set_color_space`; default false (legacy sRGB).
    pub linear_blend: bool,
    /// Interpolate gradients perceptually through OkLab. Set by
    /// `set_gradient_space`; default true.
    pub grad_oklab: bool,
    /// Per-pixel depth test (true z-buffer) for the deferred queue instead of
    /// pure painter's sort. Set by `set_depth_test`; default false.
    pub depth_test: bool,
    /// Z-buffer (camera-space depth per pixel); sized to width*height when
    /// depth testing is on. Reset to +∞ on the next flush after a screen clear
    /// (`เติม`) so it persists across a frame's multiple flushes (like
    /// `glClear(DEPTH)`), then accumulates correct occlusion across all layers.
    pub depth_buf: Vec<f32>,
    /// True ⇒ the next depth flush clears the z-buffer first (set by `เติม` /
    /// `clear_depth`). Lets the z-buffer span a frame's many `flush_3d` calls.
    pub zbuf_needs_clear: bool,
    /// True ⇒ `flush_post` already ran the toon post-chain this frame, so
    /// `present` must skip it (keeps UI drawn afterwards out of the post FX).
    pub post_done: bool,
    /// Distance fog: triangles/lines fade toward `fog_color` from `fog_start`
    /// to `fog_end` (camera-space depth). `fog_end <= 0` disables fog.
    pub fog_color: u32,
    pub fog_start: f32,
    pub fog_end: f32,
    /// Perf test: force flat *unlit* shading — triangle/mesh draws skip
    /// `compute_lit_color` and use the raw pen colour. Toggle via `set_flat_shade`.
    pub flat_shade: bool,
    /// Pace the window to the monitor's refresh rate (`set_vsync`). Default on.
    pub vsync: bool,
    /// Per-frame shared-edge dedup: `draw_line_3d` skips edges already drawn.
    pub edge_set: poly::EdgeSet,
    /// Active material override.  When `Some`, polygon draws use the BSDF
    /// instead of `compute_lit_color_linear`.  `None` = legacy path.
    pub material: Option<LingMaterial>,
    /// Optional world-space normal override for stylized surfaces.
    pub normal_override: Option<[f32; 3]>,
    /// Toon post-processing configuration (outlines, shadow softness, highlight).
    pub toon: ToonConfig,
    /// Baked local-space triangle meshes (display lists) indexed by handle.
    /// Each entry is a flat run of `[ax,ay,az, bx,by,bz, cx,cy,cz]` local verts.
    pub meshes: Vec<Vec<([f32; 9], u32)>>,
    /// Active mesh capture buffer. While `Some`, `draw_triangle_3d` records raw
    /// local coords here instead of submitting to the depth queue.
    pub mesh_capture: Option<Vec<([f32; 9], u32)>>,
    /// Reclaimable mesh slots (freed on keyed-cache eviction) for `mesh_register`.
    pub mesh_free: Vec<usize>,
    /// Keyed display-list cache (e.g. world rooms): key → mesh handle, bounded.
    pub mesh_cache: std::collections::HashMap<i64, usize>,
    /// Was the window OS-focused as of last frame? Used to detect focus-loss/
    /// regain transitions (alt-tab) — see `focus_grace_frames`.
    pub was_active: bool,
    /// Frames remaining to suppress raw input (`key_down`/`mouse_down_*`)
    /// after regaining focus. minifb has no WM_KILLFOCUS handler, so a key
    /// released while another window was focused can read as still "down"
    /// for one stale frame right after alt-tabbing back; a short grace
    /// window after refocus swallows that instead of jerking the camera.
    pub focus_grace_frames: u8,
    /// True when the current window is the borderless-fullscreen one
    /// (`fullscreen()`/전체화면, which sets HWND_TOPMOST so it covers the
    /// taskbar). Only that window needs its topmost style dropped on
    /// alt-tab and restored on refocus — a plain `open_window()` window was
    /// never topmost, so leave it alone.
    pub topmost_window: bool,
    /// Previous-frame down-state per Win32 virtual-key code (0-255), for
    /// edge detection when reading keyboard state via `GetAsyncKeyState`
    /// instead of minifb's message-queue-based (`WM_KEYDOWN`) tracking. The
    /// borderless-fullscreen/topmost window can end up visually in front
    /// without ever actually holding real Win32 keyboard focus (Windows'
    /// foreground-lock), in which case `WM_KEYDOWN` never arrives and typing
    /// silently does nothing even though the window is clearly on top —
    /// `GetAsyncKeyState` reads the global key-state table directly and
    /// doesn't require focus, so `key_down`/`key_pressed`/`text_poll` fall
    /// back to it while `topmost_window` is set (see `runtime/mod.rs`).
    #[cfg(windows)]
    pub raw_keys_prev: [bool; 256],
    /// Time (`now_secs()`) each Win32 VK code was first observed down, for
    /// the `GetAsyncKeyState` fallback's key-repeat in `text_poll` — holding
    /// a key should eventually start retyping its character, same as any
    /// normal text field, not just fire once on the initial press.
    #[cfg(windows)]
    pub raw_keys_down_since: [f64; 256],
    /// Time (`now_secs()`) each Win32 VK code last emitted a character
    /// (initial press or a repeat), so repeats can be paced at a fixed rate
    /// once the initial hold delay has passed. See `raw_keys_down_since`.
    #[cfg(windows)]
    pub raw_keys_last_fire: [f64; 256],
    /// Native window handle (HWND on Windows) of the topmost/fullscreen
    /// window, captured when it's created. `GetAsyncKeyState` reads the
    /// OS-wide key table regardless of which window is actually focused, so
    /// the `topmost_window` input fallback needs this to check whether we're
    /// really the foreground app before trusting it — otherwise alt-tabbing
    /// away to type in another window would still feed keystrokes into the
    /// game sitting behind it. See `window_is_foreground`.
    #[cfg(windows)]
    pub hwnd: isize,
    /// Set by `quit()`/`종료()` — makes `창열림()`/`is_open()` report closed
    /// on the next check, so a script-drawn UI element (an exit button) can
    /// close the window the same way pressing Escape already does.
    pub want_quit: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl GfxState {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            window: None,
            buffer: Vec::new(),
            distort_buf: Vec::new(),
            width: 0,
            height: 0,
            color: 0x00FF_FFFF,
            camera: Camera3D::default(),
            lights: Vec::new(),
            ambient: 0.15,
            depth_queue: DepthQueue::default(),
            mouse_dx: 0.0,
            mouse_dy: 0.0,
            last_mx: f32::NAN,
            last_my: f32::NAN,
            mouse_captured: false,
            shade_mode: 2,
            shade: ling_graphics::shading::ShadeParams::default(),
            blend: 0,
            alpha: 1.0,
            antialias: false,
            font_antialias: false,
            mesh_hue: 0.0,
            mesh_hue_gain: 1.0,
            frame_blur: 0.0,
            prev_frame: Vec::new(),
            shadow: ShadowParams::default(),
            linear_blend: false,
            grad_oklab: true,
            depth_test: false,
            depth_buf: Vec::new(),
            zbuf_needs_clear: true,
            post_done: false,
            fog_color: 0x0000_0000,
            fog_start: 0.0,
            fog_end: 0.0,
            flat_shade: false,
            vsync: true,
            edge_set: poly::EdgeSet::default(),
            material: None,
            normal_override: None,
            toon: ToonConfig::default(),
            meshes: Vec::new(),
            mesh_capture: None,
            mesh_free: Vec::new(),
            mesh_cache: std::collections::HashMap::new(),
            was_active: true,
            focus_grace_frames: 0,
            topmost_window: false,
            #[cfg(windows)]
            raw_keys_prev: [false; 256],
            #[cfg(windows)]
            raw_keys_down_since: [0.0; 256],
            #[cfg(windows)]
            raw_keys_last_fire: [0.0; 256],
            #[cfg(windows)]
            hwnd: 0,
            want_quit: false,
        }
    }

    /// True while raw input (key_down/mouse_down*) should read as released:
    /// the window is unfocused (alt-tabbed away), or we're in the short
    /// grace window right after regaining focus (see `focus_grace_frames`).
    #[inline]
    pub fn input_suppressed(&mut self) -> bool {
        self.focus_grace_frames > 0 || !self.window.as_mut().map(|w| w.is_active()).unwrap_or(true)
    }

    /// Blend a colour toward the fog colour by camera-space `depth`.
    #[inline]
    pub fn fog_apply(&self, color: u32, depth: f32) -> u32 {
        if self.fog_end <= 0.0 {
            return color;
        }
        let span = self.fog_end - self.fog_start;
        if span <= 0.0 {
            return color;
        }
        let f = ((depth - self.fog_start) / span).clamp(0.0, 1.0);
        if f <= 0.0 {
            return color;
        }
        let lerp = |a: u32, b: u32| -> u32 { (a as f32 + (b as f32 - a as f32) * f) as u32 & 0xff };
        let r = lerp((color >> 16) & 0xff, (self.fog_color >> 16) & 0xff);
        let g = lerp((color >> 8) & 0xff, (self.fog_color >> 8) & 0xff);
        let b = lerp(color & 0xff, self.fog_color & 0xff);
        (r << 16) | (g << 8) | b
    }

    pub fn sync_projection(&mut self) {
        self.camera.cx = self.width as f32 / 2.0;
        self.camera.cy = self.height as f32 / 2.0;
        self.camera.focal = self.height as f32;
        self.camera.zdist = 5.0;
    }

    /// Run all enabled toon post-process passes on the pixel buffer.
    /// Call this after `depth_queue.flush()` and before presenting to screen.
    pub fn toon_post_process(&mut self) {
        let w = self.width;
        let h = self.height;
        if self.buffer.len() < w * h {
            return;
        }
        toon::apply(&self.toon, &mut self.buffer, &self.depth_buf, w, h);
    }
}

// ─── WASM keyboard state (thread-local, accessed from JS via wasm_bindgen) ────

#[cfg(target_arch = "wasm32")]
thread_local! {
    static WASM_KEYS_PRESSED: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
    static WASM_KEYS_DOWN: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

/// Called from JavaScript when a key is pressed down
#[cfg(target_arch = "wasm32")]
pub fn wasm_key_down(key: &str) {
    let key = normalize_key(key);
    WASM_KEYS_DOWN.with(|keys_down| {
        let mut down = keys_down.borrow_mut();
        if !down.contains(&key) {
            WASM_KEYS_PRESSED.with(|keys_pressed| {
                keys_pressed.borrow_mut().insert(key.clone());
            });
        }
        down.insert(key);
    });
}

/// Called from JavaScript when a key is released
#[cfg(target_arch = "wasm32")]
pub fn wasm_key_up(key: &str) {
    let key = normalize_key(key);
    WASM_KEYS_DOWN.with(|keys_down| {
        keys_down.borrow_mut().remove(&key);
    });
}

/// Resume the Web Audio AudioContext after a user gesture.
#[cfg(target_arch = "wasm32")]
pub fn audio_resume() {
    audio_web::resume();
}

/// Clear the per-frame pressed keys (call at start of each frame)
#[cfg(target_arch = "wasm32")]
pub fn wasm_clear_frame_keys() {
    WASM_KEYS_PRESSED.with(|keys| {
        keys.borrow_mut().clear();
    });
}

/// Check if a key was pressed this frame
#[cfg(target_arch = "wasm32")]
pub fn wasm_is_key_pressed(key: &str) -> bool {
    let key = normalize_key(key);
    WASM_KEYS_PRESSED.with(|keys| keys.borrow().contains(&key))
}

/// Check if a key is currently held down
#[cfg(target_arch = "wasm32")]
pub fn wasm_is_key_down(key: &str) -> bool {
    let key = normalize_key(key);
    WASM_KEYS_DOWN.with(|keys| keys.borrow().contains(&key))
}

// ─── WASM mouse state (thread-local, accessed from JS via wasm_bindgen) ───────

#[cfg(target_arch = "wasm32")]
thread_local! {
    static WASM_MOUSE_X: std::cell::Cell<f32> = std::cell::Cell::new(0.0);
    static WASM_MOUSE_Y: std::cell::Cell<f32> = std::cell::Cell::new(0.0);
    static WASM_MOUSE_DX: std::cell::Cell<f32> = std::cell::Cell::new(0.0);
    static WASM_MOUSE_DY: std::cell::Cell<f32> = std::cell::Cell::new(0.0);
    static WASM_MOUSE_LEFT: std::cell::Cell<bool> = std::cell::Cell::new(false);
    static WASM_MOUSE_RIGHT: std::cell::Cell<bool> = std::cell::Cell::new(false);
    static WASM_MOUSE_MIDDLE: std::cell::Cell<bool> = std::cell::Cell::new(false);
}

/// Called from JavaScript on pointer move; `x`/`y` are canvas-relative pixels.
#[cfg(target_arch = "wasm32")]
pub fn wasm_mouse_move(x: f32, y: f32) {
    let dx = WASM_MOUSE_X.with(|c| x - c.replace(x));
    let dy = WASM_MOUSE_Y.with(|c| y - c.replace(y));
    WASM_MOUSE_DX.with(|c| c.set(c.get() + dx));
    WASM_MOUSE_DY.with(|c| c.set(c.get() + dy));
}

/// Called from JavaScript on mousedown/mouseup. `button` follows the DOM
/// MouseEvent.button convention: 0 = left, 1 = middle, 2 = right.
#[cfg(target_arch = "wasm32")]
pub fn wasm_mouse_button(button: u32, pressed: bool, x: f32, y: f32) {
    wasm_mouse_move(x, y);
    match button {
        0 => WASM_MOUSE_LEFT.with(|c| c.set(pressed)),
        1 => WASM_MOUSE_MIDDLE.with(|c| c.set(pressed)),
        2 => WASM_MOUSE_RIGHT.with(|c| c.set(pressed)),
        _ => {},
    }
}

#[cfg(target_arch = "wasm32")]
pub fn wasm_mouse_x() -> f32 {
    WASM_MOUSE_X.with(|c| c.get())
}
#[cfg(target_arch = "wasm32")]
pub fn wasm_mouse_y() -> f32 {
    WASM_MOUSE_Y.with(|c| c.get())
}
#[cfg(target_arch = "wasm32")]
pub fn wasm_mouse_dx() -> f32 {
    WASM_MOUSE_DX.with(|c| c.get())
}
#[cfg(target_arch = "wasm32")]
pub fn wasm_mouse_dy() -> f32 {
    WASM_MOUSE_DY.with(|c| c.get())
}
#[cfg(target_arch = "wasm32")]
pub fn wasm_mouse_down() -> bool {
    WASM_MOUSE_LEFT.with(|c| c.get())
}
#[cfg(target_arch = "wasm32")]
pub fn wasm_mouse_down_right() -> bool {
    WASM_MOUSE_RIGHT.with(|c| c.get())
}
#[cfg(target_arch = "wasm32")]
pub fn wasm_mouse_down_middle() -> bool {
    WASM_MOUSE_MIDDLE.with(|c| c.get())
}

/// Clear the per-frame mouse delta (call at the start of each frame,
/// alongside `wasm_clear_frame_keys`).
#[cfg(target_arch = "wasm32")]
pub fn wasm_clear_frame_mouse_delta() {
    WASM_MOUSE_DX.with(|c| c.set(0.0));
    WASM_MOUSE_DY.with(|c| c.set(0.0));
}

/// Queue a decoded mono PCM buffer for one-shot playback through Web Audio.
#[cfg(target_arch = "wasm32")]
pub fn wasm_play_audio_buffer(pcm_data: &[f32], sample_rate: u32) {
    let id = audio_web::add_sample(pcm_data, 1, sample_rate);
    if id >= 0 {
        audio_web::play_sample(id as usize, 0.0, 0.0, 0.0, 1.0, false);
    }
}

/// Set master output volume (0.0 to 1.0) for the Web Audio engine.
#[cfg(target_arch = "wasm32")]
pub fn wasm_set_master_volume(volume: f32) {
    audio_web::set_master_volume(volume);
}

/// Normalize browser key names to match Ling's key naming convention
#[cfg(target_arch = "wasm32")]
fn normalize_key(key: &str) -> String {
    match key {
        " " => "space".to_string(),
        "ArrowUp" => "up".to_string(),
        "ArrowDown" => "down".to_string(),
        "ArrowLeft" => "left".to_string(),
        "ArrowRight" => "right".to_string(),
        "Enter" => "enter".to_string(),
        "Escape" => "escape".to_string(),
        "Shift" | "ShiftLeft" | "ShiftRight" => "shift".to_string(),
        "Control" | "ControlLeft" | "ControlRight" => "ctrl".to_string(),
        "Alt" | "AltLeft" | "AltRight" => "alt".to_string(),
        "Tab" => "tab".to_string(),
        "Backspace" => "backspace".to_string(),
        _ => key.to_lowercase(),
    }
}

// ─── WASM GfxState (no window, no software framebuffer) ──────────────────────

#[cfg(target_arch = "wasm32")]
pub struct GfxState {
    pub width: usize,
    pub height: usize,
    /// Current pen colour (0x00RRGGBB).
    pub color: u32,
    /// Fill / clear colour components [0..1].
    pub fill_r: f32,
    pub fill_g: f32,
    pub fill_b: f32,
    pub camera: Camera3D,
    pub lights: Vec<Light>,
    pub ambient: f32,
    /// Accumulates projected screen-space draw calls; flushed to WebGL by present().
    pub depth_queue: DepthQueue,
    pub shade_mode: u8,
    pub shade: ling_graphics::shading::ShadeParams,
    /// Software framebuffer — the same CPU raster path as native. On the web,
    /// `present()` uploads this to the canvas, so 2-D builtins render identically.
    pub buffer: Vec<u32>,
    /// Reusable scratch for `distort()` — avoids a per-frame clone.
    pub distort_buf: Vec<u32>,
    /// Blend mode for pixel writes: 0 = normal (overwrite), 1 = additive.
    pub blend: u8,
    /// Pen opacity [0..1] for the alpha-blended fills (mirrors native).
    pub alpha: f32,
    /// Hue rotation (radians) for `draw_color_mesh` baked colours (mirrors native).
    pub mesh_hue: f32,
    /// Brightness gain applied with the hue rotation (mirrors native).
    pub mesh_hue_gain: f32,
    /// Frame accumulation amount (mirrors native; unused on wasm).
    pub frame_blur: f32,
    /// Previous frame buffer (mirrors native; unused on wasm).
    pub prev_frame: Vec<u32>,
    /// Anti-alias wireframe strokes (mirrors native). Default false = aliased.
    pub antialias: bool,
    /// Tunable height→size/opacity mapping for `cast_shadow`.
    pub shadow: ShadowParams,
    /// Gamma-correct (linear-light) compositing — mirrors native.
    pub linear_blend: bool,
    /// Perceptual OkLab gradient interpolation — mirrors native.
    pub grad_oklab: bool,
    /// Per-pixel depth test (z-buffer) for the deferred queue — mirrors native.
    pub depth_test: bool,
    /// Z-buffer (camera-space depth per pixel).
    pub depth_buf: Vec<f32>,
    /// Mirrors native: next depth flush clears the z-buffer first.
    pub zbuf_needs_clear: bool,
    /// Mirrors native: `flush_post` ran the post-chain; `present` skips it.
    pub post_done: bool,
    /// Distance fog (mirrors native): fade toward `fog_color` from `fog_start`
    /// to `fog_end`. `fog_end <= 0` disables fog.
    pub fog_color: u32,
    pub fog_start: f32,
    pub fog_end: f32,
    /// Perf test: force flat *unlit* shading (mirrors native).
    pub flat_shade: bool,
    /// Keyboard state: keys pressed this frame (cleared each frame)
    pub keys_pressed: std::collections::HashSet<String>,
    /// Keyboard state: keys currently held down
    pub keys_down: std::collections::HashSet<String>,
    /// Per-frame shared-edge dedup (mirrors native).
    pub edge_set: poly::EdgeSet,
    /// Active material override (mirrors native).
    pub material: Option<LingMaterial>,
    /// Optional world-space normal override (mirrors native).
    pub normal_override: Option<[f32; 3]>,
    /// Toon post-processing configuration (mirrors native).
    pub toon: ToonConfig,
    /// Baked local-space triangle meshes (mirrors native).
    pub meshes: Vec<Vec<([f32; 9], u32)>>,
    /// Active mesh capture buffer (mirrors native).
    pub mesh_capture: Option<Vec<([f32; 9], u32)>>,
    /// Reclaimable mesh slots (mirrors native).
    pub mesh_free: Vec<usize>,
    /// Keyed display-list cache (mirrors native).
    pub mesh_cache: std::collections::HashMap<i64, usize>,
}

#[cfg(target_arch = "wasm32")]
impl GfxState {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            width: 800,
            height: 600,
            color: 0x00FF_FFFF,
            fill_r: 0.0,
            fill_g: 0.0,
            fill_b: 0.0,
            camera: Camera3D::default(),
            lights: Vec::new(),
            ambient: 0.15,
            depth_queue: DepthQueue::default(),
            shade_mode: 2,
            shade: ling_graphics::shading::ShadeParams::default(),
            buffer: vec![0u32; 800 * 600],
            distort_buf: Vec::new(),
            blend: 0,
            alpha: 1.0,
            antialias: false,
            mesh_hue: 0.0,
            mesh_hue_gain: 1.0,
            frame_blur: 0.0,
            prev_frame: Vec::new(),
            shadow: ShadowParams::default(),
            linear_blend: false,
            grad_oklab: true,
            depth_test: false,
            depth_buf: Vec::new(),
            zbuf_needs_clear: true,
            post_done: false,
            fog_color: 0x0000_0000,
            fog_start: 0.0,
            fog_end: 0.0,
            flat_shade: false,
            keys_pressed: std::collections::HashSet::new(),
            keys_down: std::collections::HashSet::new(),
            edge_set: poly::EdgeSet::default(),
            material: None,
            normal_override: None,
            toon: ToonConfig::default(),
            meshes: Vec::new(),
            mesh_capture: None,
            mesh_free: Vec::new(),
            mesh_cache: std::collections::HashMap::new(),
        }
    }

    /// Clear the keys_pressed set at the start of each frame
    pub fn clear_frame_keys(&mut self) {
        self.keys_pressed.clear();
    }

    /// Register a key press (called from JS)
    pub fn on_key_down(&mut self, key: String) {
        if !self.keys_down.contains(&key) {
            self.keys_pressed.insert(key.clone());
        }
        self.keys_down.insert(key);
    }

    /// Register a key release (called from JS)
    pub fn on_key_up(&mut self, key: String) {
        self.keys_down.remove(&key);
    }

    /// Blend a colour toward the fog colour by camera-space `depth`
    /// (identical to the native path).
    #[inline]
    pub fn fog_apply(&self, color: u32, depth: f32) -> u32 {
        if self.fog_end <= 0.0 {
            return color;
        }
        let span = self.fog_end - self.fog_start;
        if span <= 0.0 {
            return color;
        }
        let f = ((depth - self.fog_start) / span).clamp(0.0, 1.0);
        if f <= 0.0 {
            return color;
        }
        let lerp = |a: u32, b: u32| -> u32 { (a as f32 + (b as f32 - a as f32) * f) as u32 & 0xff };
        let r = lerp((color >> 16) & 0xff, (self.fog_color >> 16) & 0xff);
        let g = lerp((color >> 8) & 0xff, (self.fog_color >> 8) & 0xff);
        let b = lerp(color & 0xff, self.fog_color & 0xff);
        (r << 16) | (g << 8) | b
    }

    pub fn sync_projection(&mut self) {
        self.camera.cx = self.width as f32 / 2.0;
        self.camera.cy = self.height as f32 / 2.0;
        self.camera.focal = self.height as f32;
        self.camera.zdist = 5.0;
    }

    /// Run all enabled toon post-process passes (mirrors native).
    pub fn toon_post_process(&mut self) {
        let w = self.width;
        let h = self.height;
        if self.buffer.len() < w * h {
            return;
        }
        toon::apply(&self.toon, &mut self.buffer, &self.depth_buf, w, h);
    }
}

// Mesh display lists + the shared world-space triangle pipeline. Field names
// match on both the native and wasm `GfxState`, so one impl serves both targets.
impl GfxState {
    /// Light, near-plane clip, project, and fan-push a world-space triangle to
    /// the depth queue. Shared by `draw_triangle_3d` and `mesh_draw`.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn submit_triangle(
        &mut self,
        ax: f32,
        ay: f32,
        az: f32,
        bx: f32,
        by: f32,
        bz: f32,
        cx: f32,
        cy: f32,
        cz: f32,
    ) {
        let ux = bx - ax;
        let uy = by - ay;
        let uz = bz - az;
        let vx = cx - ax;
        let vy = cy - ay;
        let vz = cz - az;
        let normal = self
            .normal_override
            .unwrap_or([uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx]);

        let (c0, c1, c2) = if self.flat_shade {
            (self.color, self.color, self.color)
        } else if let Some(mut m) = self.material.clone() {
            // Baked-mesh BSDF: keep each triangle's baked colour as the albedo so the
            // model's own palette survives, but shade it with the active Principled
            // material + scene lights (used for the companion-orb king/queen models).
            m.albedo = self.color;
            let cam_x = self.camera.cx;
            let cam_y = self.camera.cy;
            let cam_z = self.camera.zdist;
            let amb = self.ambient;
            let s0 = crate::gfx::material::shade(
                &m,
                normal,
                [cam_x - ax, cam_y - ay, cam_z - az],
                [ax, ay, az],
                &self.lights,
                amb,
            );
            let s1 = crate::gfx::material::shade(
                &m,
                normal,
                [cam_x - bx, cam_y - by, cam_z - bz],
                [bx, by, bz],
                &self.lights,
                amb,
            );
            let s2 = crate::gfx::material::shade(
                &m,
                normal,
                [cam_x - cx, cam_y - cy, cam_z - cz],
                [cx, cy, cz],
                &self.lights,
                amb,
            );
            (s0, s1, s2)
        } else {
            crate::gfx::light::compute_lit_color_vertices(
                self.color,
                normal,
                [ax, ay, az],
                [bx, by, bz],
                [cx, cy, cz],
                &self.lights,
                self.ambient,
            )
        };

        let near = -self.camera.zdist + 0.05;
        let vw = [
            (ax, ay, az, self.camera.depth(ax, ay, az), c0),
            (bx, by, bz, self.camera.depth(bx, by, bz), c1),
            (cx, cy, cz, self.camera.depth(cx, cy, cz), c2),
        ];
        let mut poly: [(f32, f32, f32, u32); 4] = [(0.0, 0.0, 0.0, 0); 4];
        let mut pn = 0usize;
        let mut ei = 0;
        while ei < 3 {
            let a = vw[ei];
            let b = vw[(ei + 1) % 3];
            let ain = a.3 > near;
            let bin = b.3 > near;
            if ain && pn < 4 {
                poly[pn] = (a.0, a.1, a.2, a.4);
                pn += 1;
            }
            if ain != bin && pn < 4 {
                let tt = (near - a.3) / (b.3 - a.3);
                poly[pn] = (
                    a.0 + (b.0 - a.0) * tt,
                    a.1 + (b.1 - a.1) * tt,
                    a.2 + (b.2 - a.2) * tt,
                    crate::gfx::light::lerp_color(a.4, b.4, tt),
                );
                pn += 1;
            }
            ei += 1;
        }
        if pn < 3 {
            return;
        }
        let mut proj: [(f32, f32, f32, u32); 4] = [(0.0, 0.0, 0.0, 0); 4];
        let mut pi = 0;
        while pi < pn {
            let (sx, sy, sz) = self.camera.project(poly[pi].0, poly[pi].1, poly[pi].2);
            let fc = self.fog_apply(poly[pi].3, sz);
            proj[pi] = (sx, sy, sz, fc);
            pi += 1;
        }
        let mut fk = 1;
        while fk + 1 < pn {
            self.depth_queue.push_triangle_g_zv(
                proj[0].0,
                proj[0].1,
                proj[0].2,
                proj[0].3,
                proj[fk].0,
                proj[fk].1,
                proj[fk].2,
                proj[fk].3,
                proj[fk + 1].0,
                proj[fk + 1].1,
                proj[fk + 1].2,
                proj[fk + 1].3,
                3,
                self.flat_shade,
            );
            fk += 1;
        }
    }

    /// Bake captured local geometry (per-triangle coords + pen colour) into a
    /// mesh, returning its handle.
    pub fn mesh_register(&mut self, tris: Vec<([f32; 9], u32)>) -> usize {
        if let Some(id) = self.mesh_free.pop() {
            self.meshes[id] = tris;
            id
        } else {
            let id = self.meshes.len();
            self.meshes.push(tris);
            id
        }
    }

    /// Draw a baked mesh transformed by origin `o`, right `r`, up `u`, scale `s`.
    /// Forward axis is `r × u` so 3-D meshes baked at identity reconstruct exactly.
    /// `use_baked_color` replays each triangle's captured colour (multi-colour
    /// models); otherwise the current pen colour applies (e.g. tinted glyphs).
    #[allow(clippy::too_many_arguments)]
    pub fn mesh_draw(
        &mut self,
        id: usize,
        ox: f32,
        oy: f32,
        oz: f32,
        rx: f32,
        ry: f32,
        rz: f32,
        ux: f32,
        uy: f32,
        uz: f32,
        s: f32,
        use_baked_color: bool,
    ) {
        if id >= self.meshes.len() {
            return;
        }
        let fx = ry * uz - rz * uy;
        let fy = rz * ux - rx * uz;
        let fz = rx * uy - ry * ux;
        let pen = self.color;
        let mesh = std::mem::take(&mut self.meshes[id]);
        for (t, col) in &mesh {
            if use_baked_color {
                self.color = *col;
            }
            let wx0 = ox + s * (t[0] * rx + t[1] * ux + t[2] * fx);
            let wy0 = oy + s * (t[0] * ry + t[1] * uy + t[2] * fy);
            let wz0 = oz + s * (t[0] * rz + t[1] * uz + t[2] * fz);
            let wx1 = ox + s * (t[3] * rx + t[4] * ux + t[5] * fx);
            let wy1 = oy + s * (t[3] * ry + t[4] * uy + t[5] * fy);
            let wz1 = oz + s * (t[3] * rz + t[4] * uz + t[5] * fz);
            let wx2 = ox + s * (t[6] * rx + t[7] * ux + t[8] * fx);
            let wy2 = oy + s * (t[6] * ry + t[7] * uy + t[8] * fy);
            let wz2 = oz + s * (t[6] * rz + t[7] * uz + t[8] * fz);
            self.submit_triangle(wx0, wy0, wz0, wx1, wy1, wz1, wx2, wy2, wz2);
        }
        self.color = pen;
        self.meshes[id] = mesh;
    }
}
