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
pub mod grade;
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
#[cfg(all(target_arch = "wasm32", feature = "gl-web"))]
pub mod webgl_scene;
#[cfg(feature = "gpu")]
pub mod wgpu_raster;

/// Per-thread pools of reusable work buffers for the full-frame passes
/// (outlines, bloom, FXAA, depth of field). A 1080p pass needs several 2–8 MB
/// planes; allocating them fresh every frame means fresh OS pages and a page
/// fault on first touch of each 4 KB — measurable milliseconds per frame.
/// `take_*` hands back a buffer of exactly `n` elements (contents unspecified),
/// `put_*` returns it for the next frame.
pub(crate) mod scratch {
    use std::cell::RefCell;

    macro_rules! pool {
        ($take:ident, $put:ident, $t:ty, $tl:ident) => {
            thread_local! {
                static $tl: RefCell<Vec<Vec<$t>>> = const { RefCell::new(Vec::new()) };
            }
            pub(crate) fn $take(n: usize) -> Vec<$t> {
                let mut v = $tl.with(|p| {
                    let mut p = p.borrow_mut();
                    match p.iter().position(|v| v.capacity() >= n) {
                        Some(i) => p.swap_remove(i),
                        None => p.pop().unwrap_or_default(),
                    }
                });
                v.resize(n, <$t>::default());
                v
            }
            pub(crate) fn $put(v: Vec<$t>) {
                $tl.with(|p| {
                    let mut p = p.borrow_mut();
                    if p.len() < 12 {
                        p.push(v);
                    }
                });
            }
        };
    }
    pool!(take_u8, put_u8, u8, POOL_U8);
    pool!(take_u32, put_u32, u32, POOL_U32);
    pool!(take_f32, put_f32, f32, POOL_F32);
}

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
    /// open UI layer (ui_begin): (real buffer, real w, real h, untouched virtual copy)
    pub ui_layer: Option<(Vec<u32>, usize, usize, Vec<u32>)>,
    /// Native scaled render: UI layers kept at their own (display) resolution and
    /// composited AFTER the 3-D frame is upscaled, at present time — pixel text stays
    /// crisp instead of being averaged down into the low-res frame. (off, base, w, h)
    pub ui_overlays: Vec<(Vec<u32>, Vec<u32>, usize, usize)>,
    /// True ⇒ the next depth flush clears the z-buffer first (set by `เติม` /
    /// `clear_depth`). Lets the z-buffer span a frame's many `flush_3d` calls.
    pub zbuf_needs_clear: bool,
    /// True ⇒ `flush_post` already ran the toon post-chain this frame, so
    /// `present` must skip it (keeps UI drawn afterwards out of the post FX).
    pub post_done: bool,
    /// tm_lens_guard: solids / quads / 3-D triangles with a corner nearer the eye than this are
    /// skipped (0 = off), so geometry brushing the camera can't smear across the whole view.
    pub lens_guard: f32,
    /// Backface culling for `draw_triangle_3d` / mesh draws (`set_cull`):
    /// 0 off (double-sided, default) · 1 cull triangles wound clockwise on
    /// screen · 2 cull counter-clockwise ones. Culled faces skip lighting,
    /// sorting and rasterising entirely. 3 = auto: per baked / textured mesh,
    /// cull only closed meshes, on the side their own winding says is the back
    /// (open meshes stay double-sided).
    pub cull: u8,
    /// `set_cull(3)` per-mesh orientation cache: id -> (tri count, first-vertex
    /// bits, orientation); the key revalidates a recycled mesh slot.
    pub mesh_orient: std::collections::HashMap<usize, (usize, u32, i8)>,
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
    /// Saved raster layers (`layer_save` / `layer_restore`): colour + depth
    /// snapshots of static content, recalled by id instead of re-rendering.
    pub layers: std::collections::HashMap<i64, RasterLayer>,
    /// True when the current window is the borderless-fullscreen one
    /// (`fullscreen()`/전체화면, which sets HWND_TOPMOST so it covers the
    /// taskbar). Only that window needs its topmost style dropped on
    /// alt-tab and restored on refocus — a plain `open_window()` window was
    /// never topmost, so leave it alone.
    pub topmost_window: bool,
    /// Window has a resize border / maximize button; the framebuffer follows its client size.
    pub resizable: bool,
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
            ui_layer: None,
            ui_overlays: Vec::new(),
            zbuf_needs_clear: true,
            post_done: false,
            lens_guard: 0.0,
            // LING_CULL=0..3 sets the starting set_cull mode (a launcher can turn
            // auto culling on without the game calling set_cull)
            cull: std::env::var("LING_CULL").ok().and_then(|v| v.trim().parse::<u8>().ok()).map_or(0, |v| v.min(3)),
            mesh_orient: std::collections::HashMap::new(),
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
            layers: std::collections::HashMap::new(),
            topmost_window: false,
            resizable: false,
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
        // the color-grading LUT (set_color_grade / set_color_lut): last, over the whole world layer
        grade::apply(&mut self.buffer[..w * h]);
    }

    /// Rasterise everything queued so far into the framebuffer (what
    /// `flush_3d` does): honours the depth test and keeps the pen's blend state.
    pub fn flush_queue(&mut self) {
        if self.depth_queue.is_empty() {
            return;
        }
        let (w, h) = (self.width, self.height);
        let reset_z = self.zbuf_needs_clear;
        let (bm, ba, aa) = (self.blend, self.alpha, self.antialias);
        let queue = std::mem::take(&mut self.depth_queue);
        raster::set_persp_zd(self.camera.zdist);
        let z = if self.depth_test { Some(&mut self.depth_buf) } else { None };
        queue.flush(&mut self.buffer, z, reset_z, w, h, aa);
        self.zbuf_needs_clear = false;
        self.depth_queue.set_state(bm, ba);
    }

    /// Could anything of the world-space sphere (center `c`, radius `r`) land on
    /// screen? False only when it is wholly behind the near plane or off the view
    /// (with the hyperbolic view: outside the warp's 1.85x screen-radius window) —
    /// the same test the textured-mesh draws use, so a primitive skipped here
    /// would not have drawn a pixel.
    pub fn sphere_in_view(&self, c: [f32; 3], r: f32) -> bool {
        let cam = &self.camera;
        if !(r.is_finite() && c.iter().all(|v| v.is_finite())) {
            return true;
        }
        let rw = r + 0.5;
        let (sx, sy, rz) = cam.project(c[0], c[1], c[2]);
        let near = -cam.zdist + 0.05;
        if rz + rw < near {
            return false;
        }
        let dd = rz + cam.zdist;
        if dd <= rw + 0.5 {
            return true;
        }
        let pr = cam.focal * rw / (dd - rw) * 1.15 + 16.0;
        if cam.hyp_r <= 0.0 {
            let (w, h) = (self.width as f32, self.height as f32);
            !(sx + pr < 0.0 || sx - pr > w || sy + pr < 0.0 || sy - pr > h)
        } else {
            let rad = ((sx - cam.cx) * (sx - cam.cx) + (sy - cam.cy) * (sy - cam.cy)).sqrt();
            rad - pr <= 1.85 * cam.cx
        }
    }

    /// `layer_save(id)`: flush pending 3-D, then snapshot the frame's colour and
    /// (when the depth test is on) depth under `id`, reusing the layer's
    /// storage. Draw static scenery once, save it, and recall it each frame.
    pub fn layer_save(&mut self, id: i64) {
        self.flush_queue();
        let (w, h) = (self.width, self.height);
        let n = (w * h).min(self.buffer.len());
        let depth = self.depth_test && self.depth_buf.len() >= n;
        let layer = self.layers.entry(id).or_default();
        layer.w = w;
        layer.h = h;
        layer.color.resize(n, 0);
        par_copy(&mut layer.color, &self.buffer[..n]);
        if depth {
            layer.depth.resize(n, f32::INFINITY);
            par_copy(&mut layer.depth, &self.depth_buf[..n]);
        } else {
            layer.depth.clear();
        }
    }

    /// `layer_restore(id)`: put a saved layer back as the current frame —
    /// colour, and depth so whatever is drawn next is still occluded by the
    /// restored scenery. False (nothing changed) when there is no such layer
    /// or the framebuffer was resized since; re-render and `layer_save` then.
    /// Restore at the start of a frame: it replaces what is already drawn.
    pub fn layer_restore(&mut self, id: i64) -> bool {
        let (w, h) = (self.width, self.height);
        let n = w * h;
        let Some(layer) = self.layers.get(&id) else { return false };
        if layer.w != w || layer.h != h || layer.color.len() != n || self.buffer.len() < n {
            return false;
        }
        par_copy(&mut self.buffer[..n], &layer.color);
        if layer.depth.len() == n {
            self.depth_buf.resize(n, f32::INFINITY);
            par_copy(&mut self.depth_buf[..n], &layer.depth);
            self.zbuf_needs_clear = false;
        } else {
            self.zbuf_needs_clear = true;
        }
        true
    }
}

/// A saved frame layer (see [`GfxState::layer_save`]).
#[derive(Default)]
pub struct RasterLayer {
    pub w: usize,
    pub h: usize,
    pub color: Vec<u32>,
    /// Empty when saved without a depth test.
    pub depth: Vec<f32>,
}

/// Multi-threaded memcpy for full-frame buffers (one 1080p frame is 8 MB).
#[cfg(not(target_arch = "wasm32"))]
fn par_copy<T: Copy + Send + Sync>(dst: &mut [T], src: &[T]) {
    use rayon::prelude::*;
    const CH: usize = 64 * 1024;
    dst.par_chunks_mut(CH).zip(src.par_chunks(CH)).for_each(|(d, s)| d.copy_from_slice(s));
}

// ─── WASM keyboard state (thread-local, accessed from JS via wasm_bindgen) ────

#[cfg(target_arch = "wasm32")]
thread_local! {
    static WASM_KEYS_PRESSED: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
    static WASM_KEYS_DOWN: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}


// ─── WASM host display state: detached UI layer, display size, live resize ────
// The browser shows the 3-D frame (low-res, for speed) and the UI as TWO stacked
// canvases: with ui_detach(1) a ui_begin/ui_end layer is not composited into the
// frame but kept here at its virtual (screen-sized) resolution and posted to the
// page separately, so pixel text never gets shrunk with the scene.
#[cfg(target_arch = "wasm32")]
thread_local! {
    static UI_DETACH: std::cell::Cell<bool> = std::cell::Cell::new(false);
    static UI_OVERLAY: std::cell::RefCell<(Vec<u32>, usize, usize, bool)> = std::cell::RefCell::new((Vec::new(), 0, 0, false));
    static DISPLAY: std::cell::Cell<(f64, f64)> = std::cell::Cell::new((0.0, 0.0));
    static PENDING_RESIZE: std::cell::Cell<Option<(usize, usize)>> = std::cell::Cell::new(None);
}
#[cfg(target_arch = "wasm32")]
pub fn wasm_set_display(w: f64, h: f64) { DISPLAY.with(|d| d.set((w, h))); }
#[cfg(target_arch = "wasm32")]
pub fn wasm_display() -> (f64, f64) { DISPLAY.with(|d| d.get()) }
#[cfg(target_arch = "wasm32")]
pub fn wasm_request_resize(w: usize, h: usize) { PENDING_RESIZE.with(|p| p.set(Some((w.max(64), h.max(64))))); }
#[cfg(target_arch = "wasm32")]
pub fn wasm_take_resize() -> Option<(usize, usize)> { PENDING_RESIZE.with(|p| p.take()) }
#[cfg(target_arch = "wasm32")]
pub fn wasm_ui_detach(on: bool) { UI_DETACH.with(|d| d.set(on)); }
#[cfg(target_arch = "wasm32")]
pub fn wasm_ui_detached() -> bool { UI_DETACH.with(|d| d.get()) }
/// ui_end in detached mode: copy the pixels this layer changed into the overlay.
#[cfg(target_arch = "wasm32")]
pub fn wasm_ui_overlay_put(off: &[u32], base: &[u32], vw: usize, vh: usize) {
    UI_OVERLAY.with(|o| {
        let mut o = o.borrow_mut();
        if o.1 != vw || o.2 != vh || o.0.len() != vw * vh {
            *o = (vec![0u32; vw * vh], vw, vh, false);
        }
        for i in 0..(vw * vh).min(off.len()).min(base.len()) {
            if off[i] != base[i] {
                o.0[i] = 0xFF00_0000 | (off[i] & 0x00FF_FFFF);
            }
        }
        o.3 = true;
    });
}
/// present: hand the frame's overlay to the page (None when no UI was drawn) and reset it.
#[cfg(target_arch = "wasm32")]
pub fn wasm_ui_overlay_take() -> Option<(Vec<u8>, usize, usize)> {
    UI_OVERLAY.with(|o| {
        let mut o = o.borrow_mut();
        if !o.3 {
            return None;
        }
        let (w, h) = (o.1, o.2);
        let mut px = vec![0u8; w * h * 4];
        for (i, c) in o.0.iter_mut().enumerate() {
            let v = *c;
            if v >> 24 != 0 {
                px[i * 4] = ((v >> 16) & 255) as u8;
                px[i * 4 + 1] = ((v >> 8) & 255) as u8;
                px[i * 4 + 2] = (v & 255) as u8;
                px[i * 4 + 3] = 255;
                *c = 0;
            }
        }
        o.3 = false;
        Some((px, w, h))
    })
}

// ─── native Unicode text input (IME / non-ASCII keyboards) ──────────────────
// minifb reports WM_CHAR code points through an InputCallback. ASCII keeps going
// through the VK polling path, so only code points >= 0x80 are queued here.
#[cfg(not(target_arch = "wasm32"))]
static NATIVE_TEXT: std::sync::Mutex<Vec<char>> = std::sync::Mutex::new(Vec::new());

#[cfg(not(target_arch = "wasm32"))]
pub struct UnicodeInput;

#[cfg(not(target_arch = "wasm32"))]
impl minifb::InputCallback for UnicodeInput {
    fn add_char(&mut self, uni_char: u32) {
        if uni_char >= 0x80 {
            if let Some(c) = char::from_u32(uni_char) {
                if !c.is_control() {
                    if let Ok(mut q) = NATIVE_TEXT.lock() {
                        if q.len() < 256 {
                            q.push(c);
                        }
                    }
                }
            }
        }
    }
}

/// Take the non-ASCII characters typed since the last call.
#[cfg(not(target_arch = "wasm32"))]
pub fn native_text_drain() -> Vec<char> {
    NATIVE_TEXT.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

// ─── WASM text input (typed characters for text_poll) ────────────────────────
#[cfg(target_arch = "wasm32")]
thread_local! {
    static WASM_TEXT: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
}

/// Called from JavaScript with typed / pasted / IME-composed text. "\u{8}" means Backspace.
#[cfg(target_arch = "wasm32")]
pub fn wasm_text_input(s: &str) {
    WASM_TEXT.with(|q| {
        let mut q = q.borrow_mut();
        if q.len() < 512 {
            q.push(s.to_string());
        }
    });
}

/// Take everything typed since the last call.
#[cfg(target_arch = "wasm32")]
pub fn wasm_text_drain() -> Vec<String> {
    WASM_TEXT.with(|q| std::mem::take(&mut *q.borrow_mut()))
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
    // mouse_dx()/mouse_dy() are per-frame deltas: without this reset they
    // accumulated forever (a right-drag orbit pinned the pitch at its clamp
    // and kept spinning the yaw)
    wasm_clear_frame_mouse_delta();
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
pub fn wasm_gamepad_button(_index: u32, _button: &str, _pressed: bool, _value: f32) {}
#[cfg(target_arch = "wasm32")]
pub fn wasm_gamepad_axis(_index: u32, _axis: u32, _value: f32) {}
#[cfg(target_arch = "wasm32")]
pub fn wasm_gamepad_connected(_index: u32, _name: &str) {}
#[cfg(target_arch = "wasm32")]
pub fn wasm_gamepad_disconnected(_index: u32) {}
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
        // the browser reports one "Shift" for either side; desktop scripts ask for
        // a side ("lshift") — on the web both sides answer as the one key
        "Shift" | "ShiftLeft" | "ShiftRight" | "lshift" | "rshift" | "leftshift" | "rightshift"
        | "left_shift" | "right_shift" | "LShift" | "RShift" => "shift".to_string(),
        "Control" | "ControlLeft" | "ControlRight" | "lctrl" | "rctrl" | "leftctrl" | "rightctrl"
        | "lcontrol" | "rcontrol" => "ctrl".to_string(),
        "Alt" | "AltLeft" | "AltRight" | "lalt" | "ralt" | "leftalt" | "rightalt" => "alt".to_string(),
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
    pub font_antialias: bool,
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
    /// open UI layer (ui_begin): (real buffer, real w, real h, untouched virtual copy)
    pub ui_layer: Option<(Vec<u32>, usize, usize, Vec<u32>)>,
    /// Native scaled render: UI layers kept at their own (display) resolution and
    /// composited AFTER the 3-D frame is upscaled, at present time — pixel text stays
    /// crisp instead of being averaged down into the low-res frame. (off, base, w, h)
    pub ui_overlays: Vec<(Vec<u32>, Vec<u32>, usize, usize)>,
    /// Mirrors native: next depth flush clears the z-buffer first.
    pub zbuf_needs_clear: bool,
    /// Mirrors native: `flush_post` ran the post-chain; `present` skips it.
    pub post_done: bool,
    /// tm_lens_guard: solids / quads / 3-D triangles with a corner nearer the eye than this are
    /// skipped (0 = off), so geometry brushing the camera can't smear across the whole view.
    pub lens_guard: f32,
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
            ui_layer: None,
            ui_overlays: Vec::new(),
            zbuf_needs_clear: true,
            post_done: false,
            lens_guard: 0.0,
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
        let t0 = std::time::Instant::now();
        toon::apply(&self.toon, &mut self.buffer, &self.depth_buf, w, h);
        grade::apply(&mut self.buffer[..w * h]);
        if std::env::var_os("LING_GPU_LOG").is_some() {
            eprintln!("[post] toon::apply {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
        }
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
        let near = -self.camera.zdist + 0.05;
        // Warp each vertex once (the hyperbolic warp is the costly part of a
        // projection); depth, clipping and projection all work on the warped
        // points, lighting on the true world positions.
        let ha = self.camera.hyper(ax, ay, az);
        let hb = self.camera.hyper(bx, by, bz);
        let hc = self.camera.hyper(cx, cy, cz);
        let da = self.camera.depth_unwarped(ha.0, ha.1, ha.2);
        let db = self.camera.depth_unwarped(hb.0, hb.1, hb.2);
        let dc = self.camera.depth_unwarped(hc.0, hc.1, hc.2);
        // wholly behind the near plane: nothing to light, clip or push
        if da <= near && db <= near && dc <= near {
            return;
        }
        // backface cull on the projected winding, before any lighting work.
        // (Triangles crossing the near plane are kept: their projection flips.)
        if (self.cull == 1 || self.cull == 2) && da > near && db > near && dc > near {
            let pa = self.camera.project_unwarped(ha.0, ha.1, ha.2);
            let pb = self.camera.project_unwarped(hb.0, hb.1, hb.2);
            let pc = self.camera.project_unwarped(hc.0, hc.1, hc.2);
            // > 0: clockwise on screen (y grows downward)
            let area = (pb.0 - pa.0) * (pc.1 - pa.1) - (pb.1 - pa.1) * (pc.0 - pa.0);
            if (self.cull == 1 && area > 0.0) || (self.cull == 2 && area < 0.0) {
                return;
            }
        }
        let ux = bx - ax;
        let uy = by - ay;
        let uz = bz - az;
        let vx = cx - ax;
        let vy = cy - ay;
        let vz = cz - az;
        let normal = self.normal_override.unwrap_or([
            uy * vz - uz * vy,
            uz * vx - ux * vz,
            ux * vy - uy * vx,
        ]);

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

        if self.lens_guard > 0.0 {
            let g = near + self.lens_guard;
            if da < g || db < g || dc < g {
                return;
            }
        }
        let vw = [
            (ha.0, ha.1, ha.2, da, c0),
            (hb.0, hb.1, hb.2, db, c1),
            (hc.0, hc.1, hc.2, dc, c2),
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
            let (sx, sy, sz) = self.camera.project_unwarped(poly[pi].0, poly[pi].1, poly[pi].2);
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
        // set_cull(3): cull this mesh only if it is closed, on its own back side
        let saved_cull = self.cull;
        if saved_cull == 3 {
            let key = (mesh.len(), mesh.first().map_or(0, |t| t.0[0].to_bits()));
            let orient = match self.mesh_orient.get(&id) {
                Some(&(n, f, o)) if (n, f) == key => o,
                _ => {
                    let o = poly::closed_mesh_orientation(
                        mesh.iter().map(|(t, _)| [[t[0], t[1], t[2]], [t[3], t[4], t[5]], [t[6], t[7], t[8]]]),
                    );
                    self.mesh_orient.insert(id, (key.0, key.1, o));
                    o
                },
            };
            // the frame (r, u, r x u) is a proper rotation; only a negative scale mirrors
            self.cull = poly::auto_cull_mode(orient, s);
        }
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
        self.cull = saved_cull;
        self.meshes[id] = mesh;
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod layer_tests {
    use super::*;

    fn state(w: usize, h: usize) -> GfxState {
        let mut g = GfxState::new();
        (g.width, g.height) = (w, h);
        g.buffer = vec![0x0010_2030; w * h];
        g.depth_test = true;
        g.zbuf_needs_clear = true;
        g
    }

    #[test]
    fn layer_round_trips_colour_and_depth_and_keeps_occlusion() {
        let (w, h) = (160, 120);
        let mut g = state(w, h);
        raster::set_persp_zd(5.0);
        // static "wall" at depth 10 over the left half
        g.depth_queue.push_triangle_zv(0x00AA_0000, 0.0, 0.0, 10.0, 80.0, 0.0, 10.0, 0.0, 120.0, 10.0);
        g.depth_queue.push_triangle_zv(0x00AA_0000, 80.0, 0.0, 10.0, 80.0, 120.0, 10.0, 0.0, 120.0, 10.0);
        g.layer_save(7);
        let (saved_c, saved_z) = (g.buffer.clone(), g.depth_buf.clone());
        // next frame: scribble, then restore
        g.buffer.fill(0);
        g.depth_buf.fill(0.0);
        g.zbuf_needs_clear = true;
        assert!(g.layer_restore(7));
        assert_eq!(g.buffer, saved_c);
        assert_eq!(g.depth_buf, saved_z);
        // a dynamic quad BEHIND the restored wall must stay hidden where the wall is
        g.depth_queue.push_triangle_zv(0x0000_00FF, 40.0, 40.0, 20.0, 120.0, 40.0, 20.0, 40.0, 80.0, 20.0);
        g.flush_queue();
        assert_eq!(g.buffer[60 * w + 50] & RGB_MASK, 0x00AA_0000, "wall occludes the far object");
        assert_eq!(g.buffer[45 * w + 100] & RGB_MASK, 0x0000_00FF, "far object visible beside the wall");
        // resized framebuffer: stale layer refuses to restore
        let mut g2 = state(w + 1, h);
        g2.layers = std::mem::take(&mut g.layers);
        assert!(!g2.layer_restore(7));
        assert!(!g2.layer_restore(99));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod cull_tests {
    use super::*;

    /// A closed cube drawn with each cull mode: exactly one mode must give the
    /// same image as no culling (front faces win the z-test anyway) while
    /// submitting about half the triangles; the other shows inside faces.
    #[test]
    fn one_cull_mode_matches_double_sided_with_half_the_triangles() {
        let (w, h) = (200usize, 150usize);
        let v = |i: u32| -> [f32; 3] { [if i & 1 != 0 { 0.5 } else { -0.5 }, if i & 2 != 0 { 0.5 } else { -0.5 }, if i & 4 != 0 { 0.5 } else { -0.5 }] };
        // consistently wound faces (outward normals by the right-hand rule)
        let faces = [[0u32, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        let mut tris = Vec::new();
        for (k, f) in faces.iter().enumerate() {
            for t in [[f[0], f[1], f[2]], [f[0], f[2], f[3]]] {
                let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
                tris.push(([a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2]], 0x203040 + 0x202000 * k as u32));
            }
        }
        let render = |cull: u8| -> (Vec<u32>, usize) {
            let mut g = GfxState::new();
            (g.width, g.height) = (w, h);
            g.buffer = vec![0; w * h];
            g.depth_test = true;
            g.zbuf_needs_clear = true;
            g.flat_shade = true;
            g.camera.cx = 100.0;
            g.camera.cy = 75.0;
            g.camera.focal = 150.0;
            let (a, b) = (0.6f32, 0.4f32);
            (g.camera.cry, g.camera.sry, g.camera.crx, g.camera.srx) = (a.cos(), a.sin(), b.cos(), b.sin());
            g.cull = cull;
            let id = g.mesh_register(tris.clone());
            g.mesh_draw(id, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, true);
            let n = g.depth_queue.calls.len();
            g.flush_queue();
            (g.buffer, n)
        };
        let (none, n0) = render(0);
        let (c1, n1) = render(1);
        let (c2, n2) = render(2);
        assert_eq!(n0, 12);
        assert_eq!(n1 + n2, 12, "every face is culled by exactly one mode");
        let same1 = c1 == none;
        let same2 = c2 == none;
        assert!(same1 ^ same2, "exactly one cull mode keeps the double-sided image");
        assert!(n1.min(n2) <= 6 && n1.max(n2) >= 6);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod auto_cull_tests {
    use super::*;

    fn cube(outward: bool) -> Vec<([f32; 9], u32)> {
        let v = |i: u32| -> [f32; 3] { [if i & 1 != 0 { 0.5 } else { -0.5 }, if i & 2 != 0 { 0.5 } else { -0.5 }, if i & 4 != 0 { 0.5 } else { -0.5 }] };
        let faces = [[0u32, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        let mut tris = Vec::new();
        for (k, f) in faces.iter().enumerate() {
            for t in [[f[0], f[1], f[2]], [f[0], f[2], f[3]]] {
                let t = if outward { t } else { [t[0], t[2], t[1]] };
                let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
                tris.push(([a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2]], 0x203040 + 0x202000 * k as u32));
            }
        }
        tris
    }

    /// Render `mesh` at the origin with cull mode `cull`, through a camera with
    /// the hyperbolic warp ON and a wide FOV. Returns (image, submitted tris).
    fn render(mesh: &[([f32; 9], u32)], cull: u8, scale: f32) -> (Vec<u32>, usize) {
        let (w, h) = (220usize, 160usize);
        let mut g = GfxState::new();
        (g.width, g.height) = (w, h);
        g.buffer = vec![0; w * h];
        g.depth_test = true;
        g.zbuf_needs_clear = true;
        g.flat_shade = true;
        g.camera.cx = 110.0;
        g.camera.cy = 80.0;
        g.camera.focal = 90.0; // wide FOV
        let (a, b) = (0.7f32, 0.35f32);
        (g.camera.cry, g.camera.sry, g.camera.crx, g.camera.srx) = (a.cos(), a.sin(), b.cos(), b.sin());
        (g.camera.hyp_r, g.camera.hyp_s, g.camera.hx, g.camera.hy, g.camera.hz) = (3.0, 0.4, 0.2, 0.0, 0.3);
        g.cull = cull;
        let id = g.mesh_register(mesh.to_vec());
        g.mesh_draw(id, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, scale, true);
        let n = g.depth_queue.calls.len();
        assert_eq!(g.cull, cull, "mesh_draw must restore the pen's cull mode");
        g.flush_queue();
        (g.buffer, n)
    }

    #[test]
    fn orientation_detects_closed_and_open_meshes() {
        let tri = |m: &[([f32; 9], u32)]| m.iter().map(|(t, _)| [[t[0], t[1], t[2]], [t[3], t[4], t[5]], [t[6], t[7], t[8]]]).collect::<Vec<_>>();
        let out = poly::closed_mesh_orientation(tri(&cube(true)));
        let inv = poly::closed_mesh_orientation(tri(&cube(false)));
        assert!(out != 0 && inv == -out, "closed cube: {out}, inside-out: {inv}");
        // an open box (one face missing) must never be culled
        let open: Vec<_> = cube(true).into_iter().skip(2).collect();
        assert_eq!(poly::closed_mesh_orientation(tri(&open)), 0);
    }

    #[test]
    fn auto_cull_keeps_the_image_and_drops_back_faces_under_hyperbolic_warp() {
        for (outward, scale) in [(true, 1.0f32), (false, 1.0), (true, -1.0), (false, -1.0)] {
            let mesh = cube(outward);
            let (full, n_full) = render(&mesh, 0, scale);
            let (auto, n_auto) = render(&mesh, 3, scale);
            assert_eq!(n_full, 12);
            assert!(n_auto < n_full, "outward={outward} scale={scale}: auto culled nothing ({n_auto})");
            assert_eq!(auto, full, "outward={outward} scale={scale}: auto cull changed the image");
        }
        // open mesh: auto leaves every face
        let open: Vec<_> = cube(true).into_iter().skip(2).collect();
        let (a, na) = render(&open, 0, 1.0);
        let (b, nb) = render(&open, 3, 1.0);
        assert_eq!((na, a), (nb, b));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod view_cull_tests {
    use super::*;

    /// Every sphere `sphere_in_view` rejects must draw nothing when rendered for real
    /// (with and without the hyperbolic warp), and it must keep a fair share.
    #[test]
    fn rejected_spheres_draw_no_pixels() {
        for hyper in [false, true] {
            let (w, h) = (240usize, 160usize);
            let mut seed = 0x2545F4914F6CDD1Du64;
            let mut rnd = || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; (seed >> 40) as f32 / (1u64 << 24) as f32 };
            let (mut rejected, mut kept) = (0, 0);
            for _ in 0..400 {
                let mut g = GfxState::new();
                (g.width, g.height) = (w, h);
                g.buffer = vec![0; w * h];
                g.depth_test = true;
                g.zbuf_needs_clear = true;
                g.flat_shade = true;
                g.color = 0xFFFFFF;
                (g.camera.cx, g.camera.cy, g.camera.focal) = (120.0, 80.0, 140.0);
                let (a, b) = (0.4f32, 0.3f32);
                (g.camera.cry, g.camera.sry, g.camera.crx, g.camera.srx) = (a.cos(), a.sin(), b.cos(), b.sin());
                if hyper {
                    (g.camera.hyp_r, g.camera.hyp_s) = (12.0, 0.4);
                }
                let c = [(rnd() - 0.5) * 80.0, (rnd() - 0.5) * 40.0, (rnd() - 0.5) * 80.0];
                let s = 0.3 + rnd() * 4.0;
                let base = shapes::base_radius("sphere", 10.0, 6.0, 0.0).unwrap();
                if g.sphere_in_view(c, base * s) {
                    kept += 1;
                    continue;
                }
                rejected += 1;
                let m = shapes::build("sphere", [c[0], c[1], c[2], s, s, s, 0.0, 0.0, 0.0], 10.0, 6.0, 0.0).unwrap();
                g.emit_mesh(&m, 0);
                g.flush_queue();
                assert!(g.buffer.iter().all(|&p| p == 0), "hyper={hyper}: culled sphere at {c:?} r={s} drew pixels");
            }
            assert!(rejected > 50 && kept > 50, "hyper={hyper}: rejected {rejected}, kept {kept}");
        }
    }
}
