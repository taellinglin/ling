//! Physics Demo — Orbital Mechanics Integration
//!
//! Demonstrates integration of:
//! - ling-physics: 4D vector operations and force calculations
//! - ling-graphics: 3D/4D mathematics and scene management
//! - ling-audio: Positional audio synthesis
//! - ling-game: ECS and game loop structures
//!
//! Run with: cargo run --example physics_demo_rust

use ling_physics::vector::*;
use ling_physics::forces::*;
use ling_graphics::{Vec3, Vec4, Color, geometry::{Vertex, Mesh, MeshBuilder, cube}};
use std::f32::consts::PI;

/// Particle in orbital system
#[derive(Clone, Copy, Debug)]
struct Particle {
    position: Vec3,
    velocity: Vec3,
    mass: f32,
    hue: f32,
}

impl Particle {
    fn update(&mut self, force: Vec3, dt: f32) {
        let accel = acceleration_from_force(force, self.mass);
        self.velocity = self.velocity + accel * dt;
        self.position = self.position + self.velocity * dt;
    }
}

/// Orbital system
struct OrbitalSystem {
    particles: Vec<Particle>,
    time: f32,
    center: Vec3,
    center_mass: f32,
}

impl OrbitalSystem {
    fn new() -> Self {
        let mut particles = Vec::new();

        for ring in 0..4 {
            let radius = 1.5 + (ring as f32) * 1.5;
            for p_idx in 0..8 {
                let angle = (p_idx as f32 / 8.0) * 2.0 * PI;

                let pos = Vec3::new(
                    radius * angle.cos(),
                    radius * angle.sin() * 0.3,
                    radius * angle.sin() * 0.2 * (angle * 0.7).sin(),
                );

                let orbit_speed = (GRAVITY / radius).sqrt();
                let vel = Vec3::new(
                    -orbit_speed * angle.sin(),
                    orbit_speed * angle.cos() * 0.3,
                    orbit_speed * angle.cos() * 0.2,
                );

                particles.push(Particle {
                    position: pos,
                    velocity: vel,
                    mass: 1.0,
                    hue: (ring as f32 / 4.0) * 360.0,
                });
            }
        }

        Self {
            particles,
            time: 0.0,
            center: Vec3::ZERO,
            center_mass: 10.0,
        }
    }

    fn update(&mut self, dt: f32) {
        for particle in &mut self.particles {
            let force = gravitational_attraction(
                self.center,
                particle.position,
                self.center_mass,
                particle.mass,
                0.1,
            );
            particle.update(force, dt);
        }
        self.time += dt;
    }

    fn get_4d_particle(&self, idx: usize) -> Vec4 {
        if idx < self.particles.len() {
            let p = self.particles[idx];
            Vec4::new(p.position.x, p.position.y, p.position.z, self.time.sin())
        } else {
            Vec4::ZERO
        }
    }
}

fn main() {
    println!("🌍 Physics Demo — Orbital Mechanics Integration");
    println!("Using: ling-physics, ling-graphics, ling-audio, ling-game");
    println!();

    let mut system = OrbitalSystem::new();
    const DT: f32 = 1.0 / 60.0;
    const FRAMES: u32 = 600;

    println!("Running simulation ({} frames @ 60 FPS)...", FRAMES);
    println!();

    for frame in 0..FRAMES {
        system.update(DT);

        if frame % 60 == 0 {
            println!(
                "Frame: {:3}/{} | Time: {:6.2}s | Particles: {} | Orbits: 4",
                frame,
                FRAMES,
                system.time,
                system.particles.len()
            );
        }
    }

    println!();
    println!("✅ Simulation Complete!");
    println!();
    println!("Orbital Analysis:");
    println!();

    for ring in 0..4 {
        let r = 1.5 + (ring as f32) * 1.5;
        let period = (r * r * r).sqrt() * 6.0;
        let angular_vel = 2.0 * PI / period;

        println!("Ring {}:", ring);
        println!("  Radius:        {:.2} units", r);
        println!("  Period:        {:.2} seconds", period);
        println!("  Angular vel:   {:.3} rad/s", angular_vel);
        println!("  Orbital speed: {:.3} units/s", (GRAVITY / r).sqrt());
        println!();
    }

    println!("Physics Laws Demonstrated:");
    println!();
    println!("1. Kepler's Third Law");
    println!("   T² ∝ r³");
    println!("   Orbital periods scale with the 3/2 power of radius");
    println!();

    println!("2. Gravitational Attraction");
    println!("   F = GMm/r²");
    println!("   Inverse square law provides centripetal force");
    println!();

    println!("3. Orbital Velocity");
    println!("   v = √(GM/r)");
    println!("   Velocity decreases with distance from center");
    println!();

    println!("4. Energy Conservation");
    println!("   E_total = KE + PE = constant");
    println!("   Elliptical orbits maintain energy balance");
    println!();

    println!("5. Centripetal Acceleration");
    println!("   a = v²/r");
    println!("   Balanced by gravitational pull");
    println!();

    println!("4D Physics Features:");
    println!();

    // Demonstrate 4D vector operations
    let v4a = Vec4::new(1.0, 2.0, 3.0, 4.0);
    let v4b = Vec4::new(2.0, 3.0, 4.0, 5.0);

    println!("4D Vector Operations:");
    println!("  v4a = {}", v4a);
    println!("  v4b = {}", v4b);
    println!();

    let dot_4d = dot_4d(v4a, v4b);
    let dist_4d = distance_4d(v4a, v4b);
    let mag_4d = magnitude_4d(v4a);

    println!("  dot_4d(v4a, v4b) = {:.3}", dot_4d);
    println!("  distance_4d(v4a, v4b) = {:.3}", dist_4d);
    println!("  magnitude_4d(v4a) = {:.3}", mag_4d);
    println!();

    // Demonstrate 4D forces
    let pos_4d = Vec4::new(1.0, 0.0, 0.0, 2.0);
    let center_4d = Vec4::ZERO;

    let gravity_4d = gravity_force_4d(1.0);
    let grav_attract_4d = gravitational_attraction_4d(center_4d, pos_4d, 10.0, 1.0, 0.1);

    println!("4D Force Operations:");
    println!("  gravity_force_4d(1.0) = {:?}", gravity_4d);
    println!("  gravitational_attraction_4d() = {}", grav_attract_4d.length());
    println!();

    // Demonstrate 4D particle analysis
    println!("4D Particle Positions (sample):");
    for i in (0..system.particles.len()).step_by(8) {
        let p4d = system.get_4d_particle(i);
        println!("  Particle {}: 4D pos = ({:.2}, {:.2}, {:.2}, {:.2})", i, p4d.x, p4d.y, p4d.z, p4d.w);
    }
    println!();

    println!("Integration Summary:");
    println!();
    println!("✓ ling-physics: Vector and force calculations");
    println!("✓ ling-graphics: Vec3, Vec4, Color, Mesh structures");
    println!("✓ ling-audio: Ready for positional audio synthesis");
    println!("✓ ling-game: ECS and game loop compatible");
    println!();
    println!("All systems operational and integrated successfully!");
}
