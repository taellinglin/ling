// src/gfx/camera.rs — 3-D camera: Y-then-X rotation + perspective projection.
//
// The camera is stored in GfxState and used by the 3-D draw builtins
// (`วาดสามเหลี่ยม3มิติ`, `วาดเส้น3มิติ`).  Ling programs call `set_camera`
// once per frame after computing their trig values.

#[derive(Debug, Clone)]
pub struct Camera3D {
    /// Precomputed cos/sin of the Y-axis rotation angle.
    pub cry: f32, pub sry: f32,
    /// Precomputed cos/sin of the X-axis rotation angle.
    pub crx: f32, pub srx: f32,
    /// Screen-centre in pixels (set automatically when the window opens).
    pub cx:  f32, pub cy:  f32,
    /// Focal length in pixels — controls field of view.
    pub focal: f32,
    /// Z offset added before the perspective divide (keeps objects in front of
    /// the camera; typical value 4–6).
    pub zdist: f32,
    /// World-space camera position — subtracted from every point before rotation.
    /// Move the camera with set_camera_pos / move_camera.
    pub tx: f32, pub ty: f32, pub tz: f32,
}

impl Default for Camera3D {
    fn default() -> Self {
        Self {
            cry: 1.0, sry: 0.0,
            crx: 1.0, srx: 0.0,
            cx:  960.0, cy: 540.0,
            focal: 1080.0,
            zdist: 5.0,
            tx: 0.0, ty: 0.0, tz: 0.0,
        }
    }
}

impl Camera3D {
    /// Camera-space depth only — cheaper than a full project() when you only
    /// need to test whether a point is in front of the camera.
    #[inline]
    pub fn depth(&self, wx: f32, wy: f32, wz: f32) -> f32 {
        let wx = wx - self.tx;
        let wy = wy - self.ty;
        let wz = wz - self.tz;
        let rz1 = wx * self.sry + wz * self.cry;
        let rz  = wy * self.srx + rz1 * self.crx;
        rz
    }

    /// Project a world-space point to (screen_x, screen_y, camera_depth).
    /// Pipeline: translate → Y-rotation → X-rotation → perspective divide.
    #[inline]
    pub fn project(&self, wx: f32, wy: f32, wz: f32) -> (f32, f32, f32) {
        let wx = wx - self.tx;
        let wy = wy - self.ty;
        let wz = wz - self.tz;
        // — Y rotation —
        let rx  =  wx * self.cry - wz * self.sry;
        let rz1 =  wx * self.sry + wz * self.cry;
        // — X rotation —
        let ry  =  wy * self.crx - rz1 * self.srx;
        let rz  =  wy * self.srx + rz1 * self.crx;
        // — Perspective —
        let d   = rz + self.zdist;
        let sx  = self.cx    + self.focal * rx / d;
        let sy  = self.cy    + self.focal * ry / d;
        (sx, sy, rz)
    }
}
