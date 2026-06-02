//! Force calculations for physics simulations.
//!
//! Provides common forces:
//! - Gravity (constant acceleration downward)
//! - Drag / air resistance (velocity-dependent friction)
//! - Spring force (Hooke's law: F = -k * x)
//! - Electromagnetic repulsion/attraction
//! - Buoyancy
//! - Wind force (directional + turbulence)

use glam::{Vec3, Vec4};

/// Gravitational acceleration (9.8 m/s² downward)
pub const GRAVITY: f32 = 9.8;

/// Calculate gravity force given mass
pub fn gravity_force(mass: f32) -> Vec3 {
    Vec3::new(0.0, -GRAVITY * mass, 0.0)
}

/// Drag force opposing motion: F = -drag_coeff * velocity
/// drag_coeff ranges from 0 (no drag) to 1 (strong friction)
pub fn drag_force(velocity: Vec3, drag_coeff: f32) -> Vec3 {
    -drag_coeff * velocity
}

/// Viscous drag (proportional to velocity magnitude): F = -viscosity * speed * velocity
/// Useful for liquids and heavy damping
pub fn viscous_drag(velocity: Vec3, viscosity: f32) -> Vec3 {
    let speed = velocity.length();
    -viscosity * speed * velocity
}

/// Spring force (Hooke's law): F = -k * (x - rest_length) * direction
/// position: current position of mass
/// anchor: fixed attachment point
/// spring_constant: stiffness (higher = stiffer)
/// rest_length: natural length of spring
pub fn spring_force(position: Vec3, anchor: Vec3, spring_constant: f32, rest_length: f32) -> Vec3 {
    let displacement = position - anchor;
    let distance = displacement.length();
    if distance < 0.001 {
        return Vec3::ZERO;
    }
    let direction = displacement.normalize();
    let extension = distance - rest_length;
    -spring_constant * extension * direction
}

/// Damping force for springs: F = -damping * velocity
/// Reduces oscillation in spring systems
pub fn damping_force(velocity: Vec3, damping_coeff: f32) -> Vec3 {
    -damping_coeff * velocity
}

/// Attractive force (inverse square law, like gravity): F = -G * m1 * m2 / r²
/// source: position of attracting body
/// target: position of attracted body
/// mass1, mass2: masses
/// G: gravitational constant (tune for desired strength)
pub fn gravitational_attraction(source: Vec3, target: Vec3, mass1: f32, mass2: f32, g: f32) -> Vec3 {
    let displacement = target - source;
    let distance = displacement.length();
    if distance < 0.01 {
        return Vec3::ZERO;
    }
    let direction = displacement.normalize();
    let force_magnitude = g * mass1 * mass2 / (distance * distance);
    force_magnitude * direction
}

/// Repulsive force (inverse square law): F = G * m1 * m2 / r²
pub fn repulsive_force(source: Vec3, target: Vec3, mass1: f32, mass2: f32, g: f32) -> Vec3 {
    -gravitational_attraction(source, target, mass1, mass2, g)
}

/// Buoyancy force: F = ρ * g * V (upward, proportional to volume submerged)
/// volume: volume of submerged object
/// liquid_density: density of fluid (water ≈ 1000 kg/m³)
pub fn buoyancy_force(volume: f32, liquid_density: f32) -> Vec3 {
    Vec3::new(0.0, GRAVITY * liquid_density * volume, 0.0)
}

/// Wind force: constant directional force with turbulence
/// wind_direction: unit vector pointing wind direction
/// wind_strength: base force magnitude
/// turbulence: adds random variation (0 = none, 1 = chaotic)
pub fn wind_force(wind_direction: Vec3, wind_strength: f32, turbulence: f32, time: f32) -> Vec3 {
    let base = wind_direction.normalize() * wind_strength;
    if turbulence < 0.001 {
        return base;
    }
    // Simple sine-based turbulence for deterministic variation
    let turb_x = (time * 1.3).sin() * turbulence * wind_strength;
    let turb_y = (time * 0.7).sin() * turbulence * wind_strength * 0.5;
    let turb_z = (time * 1.9).sin() * turbulence * wind_strength;
    base + Vec3::new(turb_x, turb_y, turb_z)
}

/// Centripetal force (for circular motion): F = m * v² / r
/// Directed toward center of rotation
pub fn centripetal_force(mass: f32, speed: f32, radius: f32) -> f32 {
    if radius < 0.001 {
        return 0.0;
    }
    mass * speed * speed / radius
}

/// Elasticity/collision response: bounce velocity
/// velocity: velocity before collision
/// normal: surface normal (pointing away from surface)
/// elasticity: bounciness (0 = absorb, 1 = perfect bounce)
pub fn bounce_velocity(velocity: Vec3, normal: Vec3, elasticity: f32) -> Vec3 {
    let reflected = velocity - 2.0 * velocity.dot(normal) * normal;
    reflected * elasticity
}

/// Terminal velocity in drag: velocity at which drag = weight
/// mass: object mass
/// drag_coeff: drag coefficient
/// g: gravitational acceleration
pub fn terminal_velocity(mass: f32, drag_coeff: f32, g: f32) -> f32 {
    if drag_coeff < 0.001 {
        return f32::INFINITY;
    }
    (mass * g / drag_coeff).sqrt()
}

/// Lorentz force (electromagnetic): F = q * (E + v × B)
/// charge: electric charge
/// electric_field: electric field vector
/// velocity: particle velocity
/// magnetic_field: magnetic field vector
pub fn lorentz_force(charge: f32, electric_field: Vec3, velocity: Vec3, magnetic_field: Vec3) -> Vec3 {
    let electric = charge * electric_field;
    let magnetic = charge * velocity.cross(magnetic_field);
    electric + magnetic
}

/// Pressure force (on surface area): F = P * A * normal
/// pressure: force per unit area
/// area: surface area
/// normal: surface normal
pub fn pressure_force(pressure: f32, area: f32, normal: Vec3) -> Vec3 {
    pressure * area * normal.normalize()
}

/// Calculate acceleration from forces: a = F / m
pub fn acceleration_from_force(force: Vec3, mass: f32) -> Vec3 {
    if mass < 0.001 {
        return Vec3::ZERO;
    }
    force / mass
}

// ──────────────────────────────────────────────────────────────────────────
// 4D FORCE OPERATIONS
// ──────────────────────────────────────────────────────────────────────────

/// 4D gravity force (projects into 4D space)
pub fn gravity_force_4d(mass: f32) -> Vec4 {
    Vec4::new(0.0, -GRAVITY * mass, 0.0, 0.0)
}

/// 4D drag force: F = -drag_coeff * velocity
pub fn drag_force_4d(velocity: Vec4, drag_coeff: f32) -> Vec4 {
    -drag_coeff * velocity
}

/// 4D spring force (Hooke's law in 4D)
pub fn spring_force_4d(position: Vec4, anchor: Vec4, spring_constant: f32, rest_length: f32) -> Vec4 {
    let displacement = position - anchor;
    let distance = displacement.length();
    if distance < 0.001 {
        return Vec4::ZERO;
    }
    let direction = displacement.normalize();
    let extension = distance - rest_length;
    -spring_constant * extension * direction
}

/// 4D damping force: F = -damping * velocity
pub fn damping_force_4d(velocity: Vec4, damping_coeff: f32) -> Vec4 {
    -damping_coeff * velocity
}

/// 4D gravitational attraction (inverse square law in 4D space)
pub fn gravitational_attraction_4d(source: Vec4, target: Vec4, mass1: f32, mass2: f32, g: f32) -> Vec4 {
    let displacement = target - source;
    let distance = displacement.length();
    if distance < 0.01 {
        return Vec4::ZERO;
    }
    let direction = displacement.normalize();
    let force_magnitude = g * mass1 * mass2 / (distance * distance);
    force_magnitude * direction
}

/// 4D repulsive force (inverse square law)
pub fn repulsive_force_4d(source: Vec4, target: Vec4, mass1: f32, mass2: f32, g: f32) -> Vec4 {
    -gravitational_attraction_4d(source, target, mass1, mass2, g)
}

/// 4D bounce/reflection: elastic collision response
pub fn bounce_velocity_4d(velocity: Vec4, normal: Vec4, elasticity: f32) -> Vec4 {
    let reflected = velocity - 2.0 * velocity.dot(normal) * normal;
    reflected * elasticity
}

/// 4D acceleration from force: a = F / m
pub fn acceleration_from_force_4d(force: Vec4, mass: f32) -> Vec4 {
    if mass < 0.001 {
        return Vec4::ZERO;
    }
    force / mass
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gravity() {
        let g = gravity_force(1.0);
        assert!((g.y + GRAVITY).abs() < 0.01);
    }

    #[test]
    fn test_spring_force() {
        let anchor = Vec3::ZERO;
        let position = Vec3::new(1.0, 0.0, 0.0);
        let force = spring_force(position, anchor, 1.0, 0.0);
        assert!(force.x < -0.9);
    }

    #[test]
    fn test_terminal_velocity() {
        let v_term = terminal_velocity(1.0, 0.5, 10.0);
        assert!(v_term > 0.0 && v_term < 10.0);
    }
}
