//! Main game application loop.

use std::time::{Duration, Instant};

pub trait GameSystem: Send {
    fn update(&mut self, dt: f32);
}

/// Top-level game application.
pub struct GameApp {
    pub title:    String,
    pub width:    u32,
    pub height:   u32,
    systems:      Vec<Box<dyn GameSystem>>,
    target_fps:   u32,
    running:      bool,
}

impl GameApp {
    pub fn new(title: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            title: title.into(), width, height,
            systems: vec![], target_fps: 60, running: false,
        }
    }

    pub fn add_system(&mut self, sys: impl GameSystem + 'static) {
        self.systems.push(Box::new(sys));
    }

    pub fn set_fps(&mut self, fps: u32) { self.target_fps = fps.max(1); }

    /// Run the game loop until the application is closed.
    pub fn run(&mut self) {
        self.running = true;
        let frame_dur = Duration::from_secs_f64(1.0 / self.target_fps as f64);
        let mut last  = Instant::now();

        while self.running {
            let now = Instant::now();
            let dt  = now.duration_since(last).as_secs_f32().min(0.1);
            last    = now;

            for sys in &mut self.systems { sys.update(dt); }

            let elapsed = now.elapsed();
            if let Some(sleep) = frame_dur.checked_sub(elapsed) {
                std::thread::sleep(sleep);
            }
        }
    }

    pub fn quit(&mut self) { self.running = false; }
}
