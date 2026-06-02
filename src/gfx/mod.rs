// src/gfx/mod.rs — unified graphics state + sub-modules.
//
// Sub-modules
//   raster  — pixel-level fill_triangle / draw_line  (native only)
//   camera  — Camera3D: rotation storage + world→screen projection
//   light   — Light struct + cel-shading quantiser
//   depth   — DepthQueue: deferred draw accumulator
//   vtex    — vector texture primitives
//   webgl   — WebGL2 backend (wasm32 only)

#[cfg(not(target_arch = "wasm32"))]
pub mod raster;
pub mod camera;
pub mod light;
pub mod depth;
pub mod vtex;
pub mod shapes;
#[cfg(target_arch = "wasm32")]
pub mod webgl;
#[cfg(target_arch = "wasm32")]
pub mod audio_web;

pub use camera::Camera3D;
pub use light::Light;
pub use depth::DepthQueue;

// ─── Native GfxState (minifb window + software framebuffer) ──────────────────

#[cfg(not(target_arch = "wasm32"))]
pub struct GfxState {
    pub window:      Option<minifb::Window>,
    pub buffer:      Vec<u32>,
    pub width:       usize,
    pub height:      usize,
    /// Current pen colour (0x00RRGGBB) set by `สีดินสอ` / `set_color`.
    pub color:       u32,
    /// 3-D camera — set once per frame with `set_camera`.
    pub camera:      Camera3D,
    /// Active point lights for this frame — cleared by `clear_lights`.
    pub lights:      Vec<Light>,
    /// Ambient fill level [0..1].  Default 0.15.
    pub ambient:     f32,
    /// Depth-sorted draw queue — flushed by `แสดงผล` / `present`.
    pub depth_queue: DepthQueue,
    /// Mouse position delta since last frame (pixels).
    pub mouse_dx: f32, pub mouse_dy: f32,
    /// Previous mouse position for delta computation; NaN = no prior sample.
    pub last_mx: f32, pub last_my: f32,
    /// When true: cursor is hidden and reset to center every frame for infinite rotation.
    pub mouse_captured: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl GfxState {
    pub fn new() -> Self {
        Self {
            window:      None,
            buffer:      Vec::new(),
            width:       0,
            height:      0,
            color:       0x00FF_FFFF,
            camera:      Camera3D::default(),
            lights:      Vec::new(),
            ambient:     0.15,
            depth_queue: DepthQueue::default(),
            mouse_dx:       0.0,
            mouse_dy:       0.0,
            last_mx:        f32::NAN,
            last_my:        f32::NAN,
            mouse_captured: false,
        }
    }

    pub fn sync_projection(&mut self) {
        self.camera.cx    = self.width  as f32 / 2.0;
        self.camera.cy    = self.height as f32 / 2.0;
        self.camera.focal = self.height as f32;
        self.camera.zdist = 5.0;
    }
}

// ─── WASM GfxState (no window, no software framebuffer) ──────────────────────

#[cfg(target_arch = "wasm32")]
pub struct GfxState {
    pub width:       usize,
    pub height:      usize,
    /// Current pen colour (0x00RRGGBB).
    pub color:       u32,
    /// Fill / clear colour components [0..1].
    pub fill_r:      f32,
    pub fill_g:      f32,
    pub fill_b:      f32,
    pub camera:      Camera3D,
    pub lights:      Vec<Light>,
    pub ambient:     f32,
    /// Accumulates projected screen-space draw calls; flushed to WebGL by present().
    pub depth_queue: DepthQueue,
}

#[cfg(target_arch = "wasm32")]
impl GfxState {
    pub fn new() -> Self {
        Self {
            width:       800,
            height:      600,
            color:       0x00FF_FFFF,
            fill_r:      0.0,
            fill_g:      0.0,
            fill_b:      0.0,
            camera:      Camera3D::default(),
            lights:      Vec::new(),
            ambient:     0.15,
            depth_queue: DepthQueue::default(),
        }
    }

    pub fn sync_projection(&mut self) {
        self.camera.cx    = self.width  as f32 / 2.0;
        self.camera.cy    = self.height as f32 / 2.0;
        self.camera.focal = self.height as f32;
        self.camera.zdist = 5.0;
    }
}
