// src/gfx/mod.rs — unified graphics state + sub-modules.
//
// Sub-modules
//   raster  — pixel-level fill_triangle / draw_line  (native only)
//   camera  — Camera3D: rotation storage + world→screen projection
//   light   — Light struct + cel-shading quantiser
//   depth   — DepthQueue: deferred draw accumulator
//   vtex    — vector texture primitives
//   webgl   — WebGL2 backend (wasm32 only)

// `raster` is a pure-CPU software rasteriser (operates on `Vec<u32>`); it is
// wasm-safe and powers the software framebuffer on both native and web.
pub mod camera;
pub mod color;
pub mod depth;
pub mod light;
pub mod raster;
pub mod shapes;
pub mod vtex;
// WebGL2 backend — wasm-only (depends on web_sys / js_sys / WebGL).
#[cfg(target_arch = "wasm32")]
pub mod audio_web;
#[cfg(target_arch = "wasm32")]
pub mod webgl;

pub use camera::Camera3D;
pub use depth::DepthQueue;
pub use light::Light;

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
    /// Distance fog: triangles/lines fade toward `fog_color` from `fog_start`
    /// to `fog_end` (camera-space depth). `fog_end <= 0` disables fog.
    pub fog_color: u32,
    pub fog_start: f32,
    pub fog_end: f32,
}

#[cfg(not(target_arch = "wasm32"))]
impl GfxState {
    pub fn new() -> Self {
        Self {
            window: None,
            buffer: Vec::new(),
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
            shade_mode: 2, // holographic cel by default
            shade: ling_graphics::shading::ShadeParams::default(),
            blend: 0, // normal (overwrite) by default
            alpha: 1.0,
            shadow: ShadowParams::default(),
            linear_blend: false,
            grad_oklab: true,
            depth_test: false,
            depth_buf: Vec::new(),
            zbuf_needs_clear: true,
            fog_color: 0x0000_0000,
            fog_start: 0.0,
            fog_end: 0.0, // fog off by default
        }
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
    /// Blend mode for pixel writes: 0 = normal (overwrite), 1 = additive.
    pub blend: u8,
    /// Pen opacity [0..1] for the alpha-blended fills (mirrors native).
    pub alpha: f32,
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
    /// Distance fog (mirrors native): fade toward `fog_color` from `fog_start`
    /// to `fog_end`. `fog_end <= 0` disables fog.
    pub fog_color: u32,
    pub fog_start: f32,
    pub fog_end: f32,
}

#[cfg(target_arch = "wasm32")]
impl GfxState {
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
            // Sized to width*height so the CPU-raster builtins (vtex / ui_*) can
            // write safely; `present()` uploads it to the canvas.
            buffer: vec![0u32; 800 * 600],
            blend: 0,
            alpha: 1.0,
            shadow: ShadowParams::default(),
            linear_blend: false,
            grad_oklab: true,
            depth_test: false,
            depth_buf: Vec::new(),
            zbuf_needs_clear: true,
            fog_color: 0x0000_0000,
            fog_start: 0.0,
            fog_end: 0.0,
        }
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
}
