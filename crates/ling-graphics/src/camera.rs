use crate::math::{Mat5, Ray3, Vec4H};
use glam::{Mat4, Quat, Vec3, Vec4};

// ── 3D Camera ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Projection {
    Perspective { fov_y: f32, near: f32, far: f32 },
    Orthographic { half_width: f32, near: f32, far: f32 },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Camera3D {
    pub position: Vec3,
    pub rotation: Quat,
    pub projection: Projection,
    pub aspect: f32,
}

impl Camera3D {
    pub fn perspective(fov_y_deg: f32, aspect: f32, near: f32, far: f32) -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            projection: Projection::Perspective { fov_y: fov_y_deg.to_radians(), near, far },
            aspect,
        }
    }

    pub fn orthographic(half_width: f32, aspect: f32, near: f32, far: f32) -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            projection: Projection::Orthographic { half_width, near, far },
            aspect,
        }
    }

    pub fn forward(&self) -> Vec3 {
        self.rotation * -Vec3::Z
    }

    pub fn right(&self) -> Vec3 {
        self.rotation * Vec3::X
    }

    pub fn up(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }

    pub fn look_at(&mut self, target: Vec3, world_up: Vec3) {
        let dir = (target - self.position).normalize();
        if dir.length_squared() < 1e-8 {
            return;
        }
        let mat = glam::camera::rh::view::look_at_mat4(self.position, target, world_up);
        let (_, rot, _) = mat.inverse().to_scale_rotation_translation();
        self.rotation = rot;
    }

    pub fn view_matrix(&self) -> Mat4 {
        Mat4::from_rotation_translation(self.rotation, self.position).inverse()
    }

    pub fn projection_matrix(&self) -> Mat4 {
        match self.projection {
            Projection::Perspective { fov_y, near, far } => {
                glam::camera::rh::proj::directx::perspective(fov_y, self.aspect, near, far)
            },
            Projection::Orthographic { half_width, near, far } => {
                let h = half_width / self.aspect;
                glam::camera::rh::proj::directx::orthographic(-half_width, half_width, -h, h, near, far)
            },
        }
    }

    pub fn view_proj(&self) -> Mat4 {
        self.projection_matrix() * self.view_matrix()
    }

    /// Unproject a screen-space point [−1,1]×[−1,1] into a world-space ray.
    pub fn unproject_ray(&self, ndc_x: f32, ndc_y: f32) -> Ray3 {
        let inv_vp = self.view_proj().inverse();
        let near = inv_vp * Vec4::new(ndc_x, ndc_y, -1.0, 1.0);
        let far = inv_vp * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
        let near = near.truncate() / near.w;
        let far = far.truncate() / far.w;
        Ray3::new(near, (far - near).normalize())
    }

    pub fn move_forward(&mut self, dist: f32) {
        self.position += self.forward() * dist;
    }

    pub fn move_right(&mut self, dist: f32) {
        self.position += self.right() * dist;
    }

    pub fn move_up(&mut self, dist: f32) {
        self.position += self.up() * dist;
    }

    pub fn orbit(&mut self, target: Vec3, yaw: f32, pitch: f32) {
        let rot = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);
        let offset = self.position - target;
        self.position = target + rot * offset;
        self.look_at(target, Vec3::Y);
    }
}

impl Default for Camera3D {
    fn default() -> Self {
        Self::perspective(60.0, 16.0 / 9.0, 0.1, 1000.0)
    }
}

// ── 4D Hyperbolic Camera ──────────────────────────────────────────────────────

/// How the 4D hyperbolic scene is projected to 3D for rendering.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum HyperModel {
    /// Klein (Beltrami-Klein) gnomonic projection: geodesics appear as straight lines.
    Klein,
    /// Poincaré ball model: angles are preserved, geodesics are circular arcs.
    Poincare,
    /// Cross-section: slice 4D scene at a fixed w-value, render the 3D slice.
    CrossSection { w_slice: f32 },
}

/// A point in the hyperboloid model of ℍ⁴.
/// Satisfies: x₀² − x₁² − x₂² − x₃² − x₄² = 1, x₀ > 0.
/// Components: (x0=time, x1..x4=space).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HyperPoint4D {
    pub x0: f32,
    pub x1: f32,
    pub x2: f32,
    pub x3: f32,
    pub x4: f32,
}

impl HyperPoint4D {
    /// The origin of ℍ⁴: (1, 0, 0, 0, 0).
    pub const ORIGIN: Self = Self { x0: 1.0, x1: 0.0, x2: 0.0, x3: 0.0, x4: 0.0 };

    pub fn new(x0: f32, x1: f32, x2: f32, x3: f32, x4: f32) -> Self {
        Self { x0, x1, x2, x3, x4 }
    }

    pub fn minkowski_dot(&self, other: &Self) -> f32 {
        -self.x0 * other.x0
            + self.x1 * other.x1
            + self.x2 * other.x2
            + self.x3 * other.x3
            + self.x4 * other.x4
    }

    /// Hyperbolic distance from self to other.
    pub fn distance(&self, other: &Self) -> f32 {
        (-self.minkowski_dot(other)).max(1.0).acosh()
    }

    /// Normalize back onto the hyperboloid after floating-point drift.
    pub fn normalize(&self) -> Self {
        let sq = self.x0 * self.x0
            - self.x1 * self.x1
            - self.x2 * self.x2
            - self.x3 * self.x3
            - self.x4 * self.x4;
        if sq <= 0.0 {
            return *self;
        }
        let s = sq.sqrt();
        Self::new(
            self.x0 / s,
            self.x1 / s,
            self.x2 / s,
            self.x3 / s,
            self.x4 / s,
        )
    }

    #[allow(clippy::wrong_self_convention)]
    pub fn to_klein(&self) -> Vec4H {
        Vec4H::new(
            self.x1 / self.x0,
            self.x2 / self.x0,
            self.x3 / self.x0,
            self.x4 / self.x0,
        )
    }

    #[allow(clippy::wrong_self_convention)]
    pub fn to_poincare(&self) -> Vec4H {
        let d = 1.0 + self.x0;
        Vec4H::new(self.x1 / d, self.x2 / d, self.x3 / d, self.x4 / d)
    }
}

/// Camera in 4D hyperbolic space.
/// The camera sits at a point in ℍ⁴ and projects scenes to 3D for final rendering.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Camera4D {
    /// Camera position in ℍ⁴ (hyperboloid model).
    pub position: HyperPoint4D,
    /// Local reference frame as a 5×5 Lorentz matrix (columns = local axes).
    pub frame: Mat5,
    pub model: HyperModel,
    /// 3D camera used for the final perspective pass after 4D→3D projection.
    pub cam3d: Camera3D,
}

impl Camera4D {
    pub fn new(model: HyperModel) -> Self {
        Self {
            position: HyperPoint4D::ORIGIN,
            frame: Mat5::identity(),
            model,
            cam3d: Camera3D::perspective(60.0, 16.0 / 9.0, 0.01, 100.0),
        }
    }

    /// Project a 4D hyperbolic point to 3D Euclidean for rasterization.
    pub fn project(&self, point: &HyperPoint4D) -> Option<Vec3> {
        match self.model {
            HyperModel::Klein => {
                let k = point.to_klein();
                // Drop one spatial axis (x4 → w, render xyz)
                Some(Vec3::new(k.x, k.y, k.z))
            },
            HyperModel::Poincare => {
                let p = point.to_poincare();
                Some(Vec3::new(p.x, p.y, p.z))
            },
            HyperModel::CrossSection { w_slice } => {
                // Keep only points near the slice w_slice
                let k = point.to_klein();
                if (k.w - w_slice).abs() > 0.5 {
                    return None;
                }
                Some(Vec3::new(k.x, k.y, k.z))
            },
        }
    }

    /// Move the camera along a hyperbolic geodesic (Lorentz boost).
    /// `direction` is a 4D spatial direction vector; `dist` is hyperbolic distance.
    pub fn move_by(&mut self, direction: Vec4H, dist: f32) {
        let len = direction.length();
        if len < 1e-8 {
            return;
        }
        let d = direction * (1.0 / len);
        let ch = dist.cosh();
        let sh = dist.sinh();
        // Boost the origin point along `d`
        let p = &self.position;
        self.position = HyperPoint4D::new(
            ch * p.x0 + sh * (d.x * p.x1 + d.y * p.x2 + d.z * p.x3 + d.w * p.x4),
            p.x1 + (sh * p.x0 + (ch - 1.0) * (d.x * p.x1 + d.y * p.x2 + d.z * p.x3 + d.w * p.x4))
                * d.x,
            p.x2 + (sh * p.x0 + (ch - 1.0) * (d.x * p.x1 + d.y * p.x2 + d.z * p.x3 + d.w * p.x4))
                * d.y,
            p.x3 + (sh * p.x0 + (ch - 1.0) * (d.x * p.x1 + d.y * p.x2 + d.z * p.x3 + d.w * p.x4))
                * d.z,
            p.x4 + (sh * p.x0 + (ch - 1.0) * (d.x * p.x1 + d.y * p.x2 + d.z * p.x3 + d.w * p.x4))
                * d.w,
        )
        .normalize();
    }
}

impl Default for Camera4D {
    fn default() -> Self {
        Self::new(HyperModel::Klein)
    }
}
