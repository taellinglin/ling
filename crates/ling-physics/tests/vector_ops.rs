//! ling-physics smoke tests: core vector algebra.

use glam::Vec3;
use ling_physics::vector;

const EPS: f32 = 1e-5;

#[test]
fn dot_product() {
    let a = Vec3::new(1.0, 2.0, 3.0);
    let b = Vec3::new(4.0, -5.0, 6.0);
    assert!((vector::dot(a, b) - (4.0 - 10.0 + 18.0)).abs() < EPS);
}

#[test]
fn cross_product_is_orthogonal() {
    let a = Vec3::X;
    let b = Vec3::Y;
    let c = vector::cross(a, b);
    assert!((c - Vec3::Z).length() < EPS, "X × Y == Z");
}

#[test]
fn distance_and_magnitude() {
    let a = Vec3::ZERO;
    let b = Vec3::new(3.0, 4.0, 0.0);
    assert!((vector::distance(a, b) - 5.0).abs() < EPS, "3-4-5 triangle");
    assert!((vector::magnitude(b) - 5.0).abs() < EPS);
}

#[test]
fn normalize_yields_unit_length() {
    let v = Vec3::new(0.0, 3.0, 4.0);
    let n = vector::normalize(v);
    assert!((vector::magnitude(n) - 1.0).abs() < EPS, "normalized vector is unit length");
}

#[test]
fn lerp_midpoint() {
    let a = Vec3::ZERO;
    let b = Vec3::new(10.0, 0.0, 0.0);
    let m = vector::lerp(a, b, 0.5);
    assert!((m - Vec3::new(5.0, 0.0, 0.0)).length() < EPS);
}
