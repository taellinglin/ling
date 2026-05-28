// src/gfx/mod.rs — unified graphics state + sub-modules.
//
// Sub-modules
//   raster  — pixel-level fill_triangle / draw_line
//   camera  — Camera3D: rotation storage + world→screen projection
//   light   — Light struct + cel-shading quantiser
//   depth   — DepthQueue: painter's-algorithm deferred renderer

pub mod raster;
pub mod camera;
pub mod light;
pub mod depth;
pub mod vtex;

pub use camera::Camera3D;
pub use light::Light;
pub use depth::DepthQueue;

/// All mutable graphics state for one Ling program.
pub struct GfxState {
    pub window:  Option<minifb::Window>,
    pub buffer:  Vec<u32>,
    pub width:   usize,
    pub height:  usize,
    /// Current pen colour (0x00RRGGBB) set by `สีดินสอ` / `set_color`.
    pub color:   u32,
    /// 3-D camera — set once per frame with `set_camera`.
    pub camera:  Camera3D,
    /// Active point lights for this frame — cleared by `clear_lights`.
    pub lights:  Vec<Light>,
    /// Ambient fill level [0..1].  Default 0.15.
    pub ambient: f32,
    /// Depth-sorted draw queue — flushed by `แสดงผล` / `present`.
    pub depth_queue: DepthQueue,
}

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
        }
    }

    /// Convenience: update the camera's screen-centre and focal length to
    /// match the current window size.  Called automatically when a window
    /// is opened.
    pub fn sync_projection(&mut self) {
        self.camera.cx    = self.width  as f32 / 2.0;
        self.camera.cy    = self.height as f32 / 2.0;
        self.camera.focal = self.height as f32;        // fills height at unit distance
        self.camera.zdist = 5.0;
    }
}
