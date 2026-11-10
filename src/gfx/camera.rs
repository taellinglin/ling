// src/gfx/camera.rs — 3-D camera: Y-then-X rotation + perspective projection.
//
// The camera is stored in GfxState and used by the 3-D draw builtins
// (`วาดสามเหลี่ยม3มิติ`, `วาดเส้น3มิติ`).  Ling programs call `set_camera`
// once per frame after computing their trig values.

#[derive(Debug, Clone)]
pub struct Camera3D {
    /// Precomputed cos/sin of the Y-axis rotation angle.
    pub cry: f32,
    pub sry: f32,
    /// Precomputed cos/sin of the X-axis rotation angle.
    pub crx: f32,
    pub srx: f32,
    /// Screen-centre in pixels (set automatically when the window opens).
    pub cx: f32,
    pub cy: f32,
    /// Focal length in pixels — controls field of view.
    pub focal: f32,
    /// Z offset added before the perspective divide (keeps objects in front of
    /// the camera; typical value 4–6).
    pub zdist: f32,
    /// World-space camera position — subtracted from every point before rotation.
    /// Move the camera with set_camera_pos / move_camera.
    pub tx: f32,
    pub ty: f32,
    pub tz: f32,
    /// Precomputed cos/sin of the roll (about the view axis, after the X rotation).
    pub crz: f32,
    pub srz: f32,
    /// Hyperbolic space (set_camera_hyper): 0 = off, else the radius R of the Poincare-style
    /// compression - a point d away (across the ground) from the pivot is drawn R tanh(d / R)
    /// away - and the saddle's strength (negative curvature: the sides rise, ahead and behind dip;
    /// mirrored above the mid-plane hy).
    pub hyp_r: f32,
    pub hyp_s: f32,
    pub hx: f32,
    pub hy: f32,
    pub hz: f32,
}

impl Default for Camera3D {
    fn default() -> Self {
        Self {
            cry: 1.0,
            sry: 0.0,
            crx: 1.0,
            srx: 0.0,
            cx: 960.0,
            cy: 540.0,
            focal: 1080.0,
            zdist: 5.0,
            tx: 0.0,
            ty: 0.0,
            tz: 0.0,
            crz: 1.0,
            srz: 0.0,
            hyp_r: 0.0,
            hyp_s: 0.0,
            hx: 0.0,
            hy: 0.0,
            hz: 0.0,
        }
    }
}

impl Camera3D {
    /// The hyperbolic warp of a world point (identity when off).
    #[inline]
    pub fn hyper(&self, wx: f32, wy: f32, wz: f32) -> (f32, f32, f32) {
        if self.hyp_r <= 0.0 {
            return (wx, wy, wz);
        }
        // only what's in front of the lens and near the view is warped: a point behind the camera
        // (or far off to the side) would be pulled in toward the pivot and land between the eye and
        // the scene. The view cone comes from the live focal length, so it follows FOV / zoom.
        let (vx, vy, vz) = (wx - self.tx, wy - self.ty, wz - self.tz);
        let qx = vx * self.cry - vz * self.sry;
        let qz1 = vx * self.sry + vz * self.cry;
        let qy = vy * self.crx - qz1 * self.srx;
        let qz = vy * self.srx + qz1 * self.crx;
        let eye = qz + self.zdist;
        let k_front = ((eye - 1.0) / 6.0).clamp(0.0, 1.0);
        if k_front <= 0.0 {
            return (wx, wy, wz);
        }
        let tan_h = (self.cx / self.focal.max(1.0)).max(0.05);
        // outside the 1.8x view cone the blend factor is exactly 0: bail before the sqrt/tanh
        let lim = 1.8 * eye.max(0.1) * tan_h;
        if qx * qx + qy * qy >= lim * lim {
            return (wx, wy, wz);
        }
        let q = (qx * qx + qy * qy).sqrt() / (eye.max(0.1) * tan_h);
        let k = k_front * ((1.8 - q) / 0.6).clamp(0.0, 1.0);
        if k <= 0.0 {
            return (wx, wy, wz);
        }
        let (mut dx, mut dz) = (wx - self.hx, wz - self.hz);
        let d = (dx * dx + dz * dz).sqrt();
        if d > 1e-4 {
            let s = self.hyp_r * (d / self.hyp_r).tanh() / d;
            dx *= s;
            dz *= s;
        }
        // the saddle, turned with the view: u across, v along
        let u = dx * self.cry - dz * self.sry;
        let v = dx * self.sry + dz * self.cry;
        // the ground (below hy, the mid-plane) and the ceiling (above it) bend opposite ways, so the
        // space between them stays open; the bend is capped softly at 9 units
        let raw = self.hyp_s * (v * v - u * u) / self.hyp_r;
        let f = ((wy - self.hy) / 12.0).clamp(-1.0, 1.0);
        let wy2 = wy + 9.0 * (raw / 9.0).tanh() * f;
        // blend toward the warped point by k
        (wx + (self.hx + dx - wx) * k, wy + (wy2 - wy) * k, wz + (self.hz + dz - wz) * k)
    }

    /// Camera-space depth only — cheaper than a full project() when you only
    /// need to test whether a point is in front of the camera.
    #[inline]
    pub fn depth(&self, wx: f32, wy: f32, wz: f32) -> f32 {
        let (wx, wy, wz) = self.hyper(wx, wy, wz);
        self.depth_unwarped(wx, wy, wz)
    }

    /// [`Self::depth`] of a point that is already warped (see [`Self::hyper`]).
    #[inline]
    pub fn depth_unwarped(&self, wx: f32, wy: f32, wz: f32) -> f32 {
        let wx = wx - self.tx;
        let wy = wy - self.ty;
        let wz = wz - self.tz;
        let rz1 = wx * self.sry + wz * self.cry;
        wy * self.srx + rz1 * self.crx
    }

    /// Project a world-space point to (screen_x, screen_y, camera_depth).
    /// Pipeline: translate → Y-rotation → X-rotation → perspective divide.
    #[inline]
    pub fn project(&self, wx: f32, wy: f32, wz: f32) -> (f32, f32, f32) {
        let (wx, wy, wz) = self.hyper(wx, wy, wz);
        self.project_unwarped(wx, wy, wz)
    }

    /// [`Self::project`] of a point that is already warped (see [`Self::hyper`]):
    /// lets a caller that needs both depth and screen position warp once.
    #[inline]
    pub fn project_unwarped(&self, wx: f32, wy: f32, wz: f32) -> (f32, f32, f32) {
        let wx = wx - self.tx;
        let wy = wy - self.ty;
        let wz = wz - self.tz;
        // — Y rotation —
        let rx = wx * self.cry - wz * self.sry;
        let rz1 = wx * self.sry + wz * self.cry;
        // — X rotation —
        let ry = wy * self.crx - rz1 * self.srx;
        let rz = wy * self.srx + rz1 * self.crx;
        // — roll —
        let (rx, ry) = (rx * self.crz - ry * self.srz, rx * self.srz + ry * self.crz);
        // — Perspective —
        let d = rz + self.zdist;
        let sx = self.cx + self.focal * rx / d;
        let sy = self.cy + self.focal * ry / d;
        (sx, sy, rz)
    }
}

#[cfg(feature = "game")]
impl Camera3D {
    /// This camera as `ling_gpu` batch-projection parameters.
    pub fn gpu_params(&self) -> (ling_gpu::CameraParams, ling_gpu::ViewWarp) {
        (
            ling_gpu::CameraParams {
                cry: self.cry,
                sry: self.sry,
                crx: self.crx,
                srx: self.srx,
                cx: self.cx,
                cy: self.cy,
                focal: self.focal,
                zdist: self.zdist,
                tx: self.tx,
                ty: self.ty,
                tz: self.tz,
            },
            ling_gpu::ViewWarp {
                crz: self.crz,
                srz: self.srz,
                hyp_r: self.hyp_r,
                hyp_s: self.hyp_s,
                hx: self.hx,
                hy: self.hy,
                hz: self.hz,
            },
        )
    }

    /// Project a whole interleaved-xyz vertex buffer into (sx, sy, depth)
    /// triples — exactly what [`Self::project`] gives per point, but on the
    /// best `ling_gpu` backend (CUDA for big buffers, every CPU core otherwise),
    /// including the hyperbolic warp and roll.
    pub fn project_batch(&self, world: &[f32], out: &mut [f32]) {
        let (cp, warp) = self.gpu_params();
        if warp.is_identity() {
            ling_gpu::backend().project_points(world, &cp, out);
        } else {
            ling_gpu::backend().project_points_warped(world, &cp, &warp, out);
        }
    }
}

#[cfg(all(test, feature = "game"))]
mod tests {
    use super::*;

    #[test]
    fn gpu_twin_matches_camera_project() {
        let mut c = Camera3D::default();
        let (a, b, r) = (0.7f32, 0.25f32, 0.15f32);
        (c.cry, c.sry, c.crx, c.srx, c.crz, c.srz) = (a.cos(), a.sin(), b.cos(), b.sin(), r.cos(), r.sin());
        (c.tx, c.ty, c.tz) = (2.0, -1.0, -8.0);
        (c.hyp_r, c.hyp_s, c.hx, c.hy, c.hz) = (9.0, 0.5, 1.0, 0.0, 3.0);
        let (cp, w) = c.gpu_params();
        for i in 0..20_000 {
            let f = i as f32;
            let p = ((f * 0.731).sin() * 20.0, (f * 0.377).cos() * 6.0, (f * 0.913).sin() * 20.0);
            let a = c.project(p.0, p.1, p.2);
            let b = ling_gpu::project_warped(&cp, &w, p.0, p.1, p.2);
            assert_eq!(a, b, "point {p:?}");
        }
    }
}
