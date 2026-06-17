//! Day/night cycle, atmosphere, weather states, wind.

use glam::{Vec2, Vec3};

// ── Time ──────────────────────────────────────────────────────────────────────

/// In-world time.  `hour` runs 0..24.
#[derive(Clone, Debug)]
pub struct WorldTime {
    /// Current hour of day, 0..24.
    pub hour: f32,
    /// Day counter (starts at 0).
    pub day: u32,
    /// Real seconds per in-game day.
    pub day_length: f32,
}

impl Default for WorldTime {
    fn default() -> Self {
        Self { hour: 8.0, day: 0, day_length: 120.0 }
    }
}

impl WorldTime {
    pub fn advance(&mut self, dt: f32) {
        self.hour += dt / self.day_length * 24.0;
        if self.hour >= 24.0 {
            self.hour -= 24.0;
            self.day += 1;
        }
    }

    pub fn is_day(&self) -> bool {
        self.hour >= 6.0 && self.hour < 20.0
    }

    /// 0 = midnight, 1 = noon.
    pub fn day_fraction(&self) -> f32 {
        (1.0 - ((self.hour - 12.0) / 12.0).abs()).clamp(0.0, 1.0)
    }
}

// ── Weather ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum WeatherState {
    Clear,
    Cloudy,
    Overcast,
    Rain,
    Snow,
    Storm,
    Fog,
}

#[derive(Clone, Debug)]
pub struct Weather {
    pub state: WeatherState,
    pub wind_dir: Vec2,     // normalised
    pub wind_speed: f32,    // m/s
    pub precipitation: f32, // 0..1
    pub cloud_cover: f32,   // 0..1
    pub fog_density: f32,   // 0..1
    pub temperature: f32,   // Celsius
}

impl Default for Weather {
    fn default() -> Self {
        Self {
            state: WeatherState::Clear,
            wind_dir: Vec2::new(1.0, 0.0),
            wind_speed: 3.0,
            precipitation: 0.0,
            cloud_cover: 0.1,
            fog_density: 0.0,
            temperature: 20.0,
        }
    }
}

// ── Atmosphere (combines time + weather) ─────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Atmosphere {
    pub time: WorldTime,
    pub weather: Weather,
}

impl Default for Atmosphere {
    fn default() -> Self {
        Self { time: WorldTime::default(), weather: Weather::default() }
    }
}

impl Atmosphere {
    pub fn update(&mut self, dt: f32) {
        self.time.advance(dt);
    }

    /// Sun direction in world space (Y up).  At noon, points straight down (-Y).
    pub fn sun_direction(&self) -> Vec3 {
        let angle = (self.time.hour - 6.0) / 12.0 * std::f32::consts::PI;
        Vec3::new(angle.sin(), -angle.cos(), 0.2).normalize()
    }

    /// Moon direction — opposite to sun.
    pub fn moon_direction(&self) -> Vec3 {
        -self.sun_direction()
    }

    /// Sky colour: deep night → twilight orange → bright day → cool dusk.
    pub fn sky_color(&self) -> [f32; 3] {
        let t = self.time.hour;
        // Pre-baked colour stops at key hours.
        let stops: [(f32, [f32; 3]); 7] = [
            (0.0, [0.01, 0.02, 0.06]),  // midnight
            (5.0, [0.05, 0.04, 0.10]),  // pre-dawn
            (6.5, [0.95, 0.45, 0.15]),  // sunrise
            (10.0, [0.50, 0.75, 1.00]), // morning
            (12.0, [0.40, 0.70, 1.00]), // noon
            (18.5, [0.98, 0.40, 0.10]), // sunset
            (20.5, [0.02, 0.03, 0.08]), // dusk
        ];
        let looped_hour = t % 24.0;
        // Find the two surrounding stops.
        let (lo, hi) = stops
            .windows(2)
            .find(|w| looped_hour >= w[0].0 && looped_hour < w[1].0)
            .map_or((stops[6], stops[0]), |w| (w[0], w[1]));
        let frac = ((looped_hour - lo.0) / (hi.0 - lo.0)).clamp(0.0, 1.0);
        let lerp = |a: f32, b: f32| a + (b - a) * frac;
        let base = [
            lerp(lo.1[0], hi.1[0]),
            lerp(lo.1[1], hi.1[1]),
            lerp(lo.1[2], hi.1[2]),
        ];
        // Darken by cloud cover.
        let cover = self.weather.cloud_cover;
        [
            base[0] * (1.0 - cover * 0.4),
            base[1] * (1.0 - cover * 0.35),
            base[2] * (1.0 - cover * 0.2),
        ]
    }

    /// Ambient light colour (used by cell-shading shadow colour).
    pub fn ambient_light(&self) -> [f32; 3] {
        let sky = self.sky_color();
        let df = self.time.day_fraction();
        [
            sky[0] * 0.35 * df + 0.01,
            sky[1] * 0.35 * df + 0.01,
            sky[2] * 0.35 * df + 0.02,
        ]
    }

    /// Directional light colour (sun or moon).
    pub fn directional_light(&self) -> [f32; 3] {
        let _df = self.time.day_fraction();
        let hour = self.time.hour;
        // Warm sunrise/sunset, white noon, blue moonlight.
        if self.time.is_day() {
            let warmth = 1.0 - (hour - 12.0).abs() / 8.0;
            [0.9 + warmth * 0.1, 0.8 + warmth * 0.15, 0.6 + warmth * 0.1]
        } else {
            [0.15, 0.18, 0.35] // cool blue moonlight
        }
    }

    /// Fog colour (usually close to sky horizon colour).
    pub fn fog_color(&self) -> [f32; 3] {
        let sky = self.sky_color();
        [
            sky[0] * 0.9 + 0.05,
            sky[1] * 0.9 + 0.05,
            sky[2] * 0.9 + 0.05,
        ]
    }

    /// Wind vector in XZ (world units per second).
    pub fn wind_vector(&self) -> Vec2 {
        self.weather.wind_dir * self.weather.wind_speed
    }

    /// Gust multiplier (0..1, varies sinusoidally with time).
    pub fn gust(&self, real_time: f32) -> f32 {
        let base = self.weather.wind_speed / 20.0;
        (base + (real_time * 0.4).sin() * 0.1 + (real_time * 1.3).sin() * 0.05).clamp(0.0, 1.0)
    }

    /// Is it raining hard enough to see?
    pub fn is_raining(&self) -> bool {
        self.weather.precipitation > 0.15
    }

    /// Is it snowing?
    pub fn is_snowing(&self) -> bool {
        self.weather.temperature < 0.0 && self.weather.precipitation > 0.1
    }
}
