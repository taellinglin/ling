//! Mechanical (机) presets — exact kinematic linkages built from [`crate::scalar`].

use crate::scalar::gear;

/// Angles of every gear in a meshed train, given the first gear's angle and the
/// tooth count of each gear in order. `teeth[0]` drives `teeth[1]`, etc.
/// Each successive gear reverses direction and scales by the tooth ratio.
pub fn gear_train(input_angle: f32, teeth: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(teeth.len());
    if teeth.is_empty() { return out; }
    out.push(input_angle);
    for w in teeth.windows(2) {
        let prev = *out.last().unwrap();
        out.push(gear(prev, w[0], w[1]));
    }
    out
}

/// Four-bar linkage position analysis. Given the ground span and the crank,
/// coupler, and rocker lengths plus a crank angle, returns the rocker angle (the
/// "open" / crossed-free configuration), or `None` if the linkage can't close at
/// that crank angle.
///
/// Pivots: A at origin, D at `(ground, 0)`. Crank A→B, coupler B→C, rocker D→C.
pub fn four_bar(crank_angle: f32, ground: f32, crank: f32, coupler: f32, rocker: f32) -> Option<f32> {
    // B from the crank.
    let bx = crank * crank_angle.cos();
    let by = crank * crank_angle.sin();
    let dx = ground;
    let dy = 0.0;

    // C is an intersection of circle(B, coupler) and circle(D, rocker).
    let dxbd = dx - bx;
    let dybd = dy - by;
    let dist = (dxbd * dxbd + dybd * dybd).sqrt();
    if dist > coupler + rocker || dist < (coupler - rocker).abs() || dist < 1e-6 {
        return None;
    }
    let a = (coupler * coupler - rocker * rocker + dist * dist) / (2.0 * dist);
    let h2 = coupler * coupler - a * a;
    if h2 < 0.0 { return None; }
    let h = h2.sqrt();
    // Midpoint along B→D, then offset perpendicular by ±h (pick + branch).
    let mx = bx + a * dxbd / dist;
    let my = by + a * dybd / dist;
    let cx = mx + h * (dybd) / dist;
    let cy = my - h * (dxbd) / dist;
    Some((cy - dy).atan2(cx - dx))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn train_scales_and_reverses() {
        let a = gear_train(6.0, &[12.0, 24.0, 12.0]);
        assert_eq!(a.len(), 3);
        assert!((a[1] - (-3.0)).abs() < 1e-6); // 12→24 halves and flips
        assert!((a[2] - 6.0).abs() < 1e-6);    // 24→12 doubles and flips back
    }
    #[test]
    fn four_bar_closes_for_valid_linkage() {
        // A balanced crank-rocker; should solve at θ=90°.
        let r = four_bar(std::f32::consts::FRAC_PI_2, 4.0, 1.0, 4.0, 2.0);
        assert!(r.is_some());
    }
}
