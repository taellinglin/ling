//! Vector operations and linear algebra primitives for physics and graphics.
//!
//! Provides:
//! - Vector creation and normalization
//! - Dot product, cross product, distance
//! - Vector rotation and transformation
//! - Linear interpolation and smoothing

use glam::{Mat4, Quat, Vec2, Vec3, Vec4};

/// Create a normalized vector (magnitude = 1.0)
pub fn normalize(v: Vec3) -> Vec3 {
    v.normalize()
}

/// Dot product: measure of parallel alignment (range: -1 to 1)
pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a.dot(b)
}

/// Cross product: perpendicular vector to both inputs
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    a.cross(b)
}

/// Distance between two points
pub fn distance(a: Vec3, b: Vec3) -> f32 {
    (a - b).length()
}

/// Vector magnitude (length)
pub fn magnitude(v: Vec3) -> f32 {
    v.length()
}

/// Squared distance (faster, avoids sqrt)
pub fn distance_squared(a: Vec3, b: Vec3) -> f32 {
    (a - b).length_squared()
}

/// Linear interpolation: blend between two vectors
/// t ranges from 0 (all a) to 1 (all b)
pub fn lerp(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a.lerp(b, t)
}

/// Angle between two vectors in radians
pub fn angle_between(a: Vec3, b: Vec3) -> f32 {
    let cos_angle = (a.normalize().dot(b.normalize())).clamp(-1.0, 1.0);
    cos_angle.acos()
}

/// Project vector a onto vector b
pub fn project(a: Vec3, b: Vec3) -> Vec3 {
    let b_normalized = b.normalize();
    (a.dot(b_normalized)) * b_normalized
}

/// Reflect a vector off a surface (with normal)
pub fn reflect(incoming: Vec3, normal: Vec3) -> Vec3 {
    incoming - 2.0 * incoming.dot(normal) * normal
}

/// Rotate vector around axis by angle (radians)
pub fn rotate_axis_angle(v: Vec3, axis: Vec3, angle: f32) -> Vec3 {
    let quat = Quat::from_axis_angle(axis.normalize(), angle);
    quat * v
}

/// Create a rotation quaternion from Euler angles (radians)
pub fn quat_from_euler(pitch: f32, yaw: f32, roll: f32) -> Quat {
    Quat::from_euler(glam::EulerRot::XYZ, pitch, yaw, roll)
}

/// Quaternion multiplication (compose rotations)
pub fn quat_mul(q1: Quat, q2: Quat) -> Quat {
    q1 * q2
}

/// Interpolate smoothly between two vectors (cubic ease-in-out)
pub fn smooth_lerp(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    let smooth_t = t * t * (3.0 - 2.0 * t); // smoothstep
    a.lerp(b, smooth_t)
}

/// Clamp vector magnitude to max length
pub fn clamp_magnitude(v: Vec3, max_length: f32) -> Vec3 {
    let len = v.length();
    if len > max_length {
        (v / len) * max_length
    } else {
        v
    }
}

/// Vector rejection: remove component of a along b
pub fn reject(a: Vec3, b: Vec3) -> Vec3 {
    a - project(a, b)
}

/// Perpendicular vector in 2D (rotated 90° counter-clockwise)
pub fn perpendicular_2d(v: Vec2) -> Vec2 {
    Vec2::new(-v.y, v.x)
}

/// Signed area of 2D triangle (positive if counter-clockwise)
pub fn signed_area_2d(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    0.5 * ((b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y))
}

/// Convert Vec3 to Vec4 with w component
pub fn vec3_to_vec4(v: Vec3, w: f32) -> Vec4 {
    Vec4::new(v.x, v.y, v.z, w)
}

/// Transform vector by 4x4 matrix (perspective-aware)
pub fn transform_vec3(v: Vec3, mat: Mat4) -> Vec3 {
    let v4 = mat * Vec4::new(v.x, v.y, v.z, 1.0);
    Vec3::new(v4.x / v4.w, v4.y / v4.w, v4.z / v4.w)
}

/// Transform direction vector by matrix (ignores translation)
pub fn transform_dir(v: Vec3, mat: Mat4) -> Vec3 {
    let v4 = mat * Vec4::new(v.x, v.y, v.z, 0.0);
    Vec3::new(v4.x, v4.y, v4.z)
}

// ──────────────────────────────────────────────────────────────────────────
// 4D VECTOR OPERATIONS
// ──────────────────────────────────────────────────────────────────────────

/// Normalize a 4D vector (magnitude = 1.0)
pub fn normalize_4d(v: Vec4) -> Vec4 {
    v.normalize()
}

/// 4D dot product
pub fn dot_4d(a: Vec4, b: Vec4) -> f32 {
    a.dot(b)
}

/// 4D vector magnitude (length)
pub fn magnitude_4d(v: Vec4) -> f32 {
    v.length()
}

/// 4D distance between two points
pub fn distance_4d(a: Vec4, b: Vec4) -> f32 {
    (a - b).length()
}

/// 4D squared distance (faster, avoids sqrt)
pub fn distance_4d_squared(a: Vec4, b: Vec4) -> f32 {
    (a - b).length_squared()
}

/// 4D linear interpolation
pub fn lerp_4d(a: Vec4, b: Vec4, t: f32) -> Vec4 {
    a.lerp(b, t)
}

/// 4D angle between two vectors
pub fn angle_between_4d(a: Vec4, b: Vec4) -> f32 {
    let cos_angle = (a.normalize().dot(b.normalize())).clamp(-1.0, 1.0);
    cos_angle.acos()
}

/// Project 4D vector a onto vector b
pub fn project_4d(a: Vec4, b: Vec4) -> Vec4 {
    let b_normalized = b.normalize();
    (a.dot(b_normalized)) * b_normalized
}

/// Reflect 4D vector off a hyperplane
pub fn reflect_4d(incoming: Vec4, normal: Vec4) -> Vec4 {
    incoming - 2.0 * incoming.dot(normal) * normal
}

/// Clamp 4D vector magnitude to max length
pub fn clamp_magnitude_4d(v: Vec4, max_length: f32) -> Vec4 {
    let len = v.length();
    if len > max_length {
        (v / len) * max_length
    } else {
        v
    }
}

/// 4D vector rejection: remove component of a along b
pub fn reject_4d(a: Vec4, b: Vec4) -> Vec4 {
    a - project_4d(a, b)
}

/// Convert 4D point to 3D homogeneous coordinates (perspective division)
pub fn vec4_to_vec3(v: Vec4) -> Vec3 {
    Vec3::new(v.x / v.w, v.y / v.w, v.z / v.w)
}

/// Create a 4D rotation matrix (4x4) from axis-angle in 3D subspace
pub fn rotation_4d_from_axis_angle(axis: Vec3, angle: f32) -> Mat4 {
    let quat = Quat::from_axis_angle(axis.normalize(), angle);
    Mat4::from_quat(quat)
}

/// 4D smooth interpolation with smoothstep
pub fn smooth_lerp_4d(a: Vec4, b: Vec4, t: f32) -> Vec4 {
    let smooth_t = t * t * (3.0 - 2.0 * t);
    a.lerp(b, smooth_t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_distance() {
        let a = Vec3::ZERO;
        let b = Vec3::new(3.0, 4.0, 0.0);
        assert!((distance(a, b) - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_dot_product() {
        let a = Vec3::new(1.0, 0.0, 0.0);
        let b = Vec3::new(0.0, 1.0, 0.0);
        assert!((dot(a, b)).abs() < 0.01); // perpendicular
    }

    #[test]
    fn test_lerp() {
        let a = Vec3::ZERO;
        let b = Vec3::new(10.0, 0.0, 0.0);
        let mid = lerp(a, b, 0.5);
        assert!((mid.x - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_distance_4d() {
        let a = Vec4::ZERO;
        let b = Vec4::new(3.0, 4.0, 0.0, 0.0);
        assert!((distance_4d(a, b) - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_dot_4d() {
        let a = Vec4::new(1.0, 0.0, 0.0, 0.0);
        let b = Vec4::new(0.0, 1.0, 0.0, 0.0);
        assert!((dot_4d(a, b)).abs() < 0.01);
    }

    #[test]
    fn test_lerp_4d() {
        let a = Vec4::ZERO;
        let b = Vec4::new(10.0, 0.0, 0.0, 0.0);
        let mid = lerp_4d(a, b, 0.5);
        assert!((mid.x - 5.0).abs() < 0.01);
    }
}
