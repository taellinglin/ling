//! Chunk-based terrain with fractal noise LOD.

use glam::Vec3;
use std::collections::HashMap;

// ── Noise helpers ─────────────────────────────────────────────────────────────

fn hash2(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32)
        .wrapping_add((y as u32).wrapping_mul(1619))
        .wrapping_add(seed.wrapping_mul(6271));
    h ^= h >> 17;
    h = h.wrapping_mul(0xbf324c81);
    h ^= h >> 31;
    h = h.wrapping_mul(0x45d9f3b);
    h as f32 / u32::MAX as f32
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let xf = smoothstep(x - xi as f32);
    let yf = smoothstep(y - yi as f32);
    let a = hash2(xi, yi, seed);
    let b = hash2(xi + 1, yi, seed);
    let c = hash2(xi, yi + 1, seed);
    let d = hash2(xi + 1, yi + 1, seed);
    let ab = a + (b - a) * xf;
    let cd = c + (d - c) * xf;
    ab + (cd - ab) * yf
}

/// Fractional Brownian Motion — stacks octaves for natural-looking terrain.
pub fn fbm(x: f32, y: f32, octaves: u32, seed: u32) -> f32 {
    let mut val = 0.0f32;
    let mut amp = 0.5f32;
    let mut freq = 1.0f32;
    for i in 0..octaves {
        val += value_noise(x * freq, y * freq, seed.wrapping_add(i * 3571)) * amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    val
}

// ── Terrain types ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct TerrainConfig {
    pub chunk_size: u32, // number of quads per side
    pub max_height: f32,
    pub noise_scale: f32,
    pub octaves: u32,
    pub lod_levels: u32,
    pub lod_dist: f32, // distance at which to reduce LOD
}

impl Default for TerrainConfig {
    fn default() -> Self {
        Self {
            chunk_size: 64,
            max_height: 80.0,
            noise_scale: 0.005,
            octaves: 6,
            lod_levels: 4,
            lod_dist: 128.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Chunk {
    pub cx: i32,
    pub cz: i32,
    pub lod: u32,
    pub heights: Vec<f32>, // (verts_per_side)² height values
    pub verts: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl Chunk {
    pub fn world_origin(&self, cfg: &TerrainConfig) -> Vec3 {
        Vec3::new(
            self.cx as f32 * cfg.chunk_size as f32,
            0.0,
            self.cz as f32 * cfg.chunk_size as f32,
        )
    }

    pub fn sample_height(&self, cfg: &TerrainConfig, wx: f32, wz: f32) -> f32 {
        let step = cfg.chunk_size as f32 / (self.heights_side(cfg) - 1) as f32;
        let origin = self.world_origin(cfg);
        let lx = ((wx - origin.x) / step).clamp(0.0, (self.heights_side(cfg) - 2) as f32);
        let lz = ((wz - origin.z) / step).clamp(0.0, (self.heights_side(cfg) - 2) as f32);
        let xi = lx as usize;
        let zi = lz as usize;
        let side = self.heights_side(cfg);
        let xf = lx - xi as f32;
        let zf = lz - zi as f32;
        let h00 = self.heights[zi * side + xi];
        let h10 = self.heights[zi * side + xi + 1];
        let h01 = self.heights[(zi + 1) * side + xi];
        let h11 = self.heights[(zi + 1) * side + xi + 1];
        h00 + (h10 - h00) * xf + (h01 - h00) * zf + (h11 - h10 - h01 + h00) * xf * zf
    }

    fn heights_side(&self, cfg: &TerrainConfig) -> usize {
        let step = 1 << self.lod;
        (cfg.chunk_size as usize / step) + 1
    }
}

fn build_chunk(cx: i32, cz: i32, lod: u32, cfg: &TerrainConfig, seed: u32) -> Chunk {
    let step = 1u32 << lod;
    let verts = (cfg.chunk_size / step) as usize + 1;
    let world_x = cx as f32 * cfg.chunk_size as f32;
    let world_z = cz as f32 * cfg.chunk_size as f32;
    let cell = step as f32;

    let mut heights = vec![0.0f32; verts * verts];
    let mut pos_arr = vec![[0.0f32; 3]; verts * verts];
    let mut nrm_arr = vec![[0.0f32; 3]; verts * verts];

    for zi in 0..verts {
        for xi in 0..verts {
            let wx = world_x + xi as f32 * cell;
            let wz = world_z + zi as f32 * cell;
            let h = fbm(
                wx * cfg.noise_scale,
                wz * cfg.noise_scale,
                cfg.octaves,
                seed,
            ) * cfg.max_height;
            heights[zi * verts + xi] = h;
            pos_arr[zi * verts + xi] = [wx, h, wz];
        }
    }

    // Compute face normals → average to vertex normals.
    for zi in 0..verts {
        for xi in 0..verts {
            let get = |xo: i32, zo: i32| -> Vec3 {
                let nx = (xi as i32 + xo).clamp(0, verts as i32 - 1) as usize;
                let nz = (zi as i32 + zo).clamp(0, verts as i32 - 1) as usize;
                Vec3::from(pos_arr[nz * verts + nx])
            };
            let dx = get(1, 0) - get(-1, 0);
            let dz = get(0, 1) - get(0, -1);
            let n = dz.cross(dx).normalize_or_zero();
            nrm_arr[zi * verts + xi] = n.to_array();
        }
    }

    let quads = verts - 1;
    let mut indices = Vec::with_capacity(quads * quads * 6);
    for zi in 0..quads {
        for xi in 0..quads {
            let tl = (zi * verts + xi) as u32;
            let tr = tl + 1;
            let bl = ((zi + 1) * verts + xi) as u32;
            let br = bl + 1;
            indices.extend_from_slice(&[tl, bl, tr, tr, bl, br]);
        }
    }

    Chunk {
        cx,
        cz,
        lod,
        heights,
        verts: pos_arr,
        normals: nrm_arr,
        indices,
    }
}

// ── Terrain manager ───────────────────────────────────────────────────────────

pub struct TerrainManager {
    pub config: TerrainConfig,
    pub seed: u32,
    chunks: HashMap<(i32, i32), Chunk>,
}

impl TerrainManager {
    pub fn new(config: TerrainConfig, seed: u32) -> Self {
        Self { config, seed, chunks: HashMap::new() }
    }

    pub fn get_height(&self, wx: f32, wz: f32) -> f32 {
        let cs = self.config.chunk_size as f32;
        let cx = (wx / cs).floor() as i32;
        let cz = (wz / cs).floor() as i32;
        if let Some(c) = self.chunks.get(&(cx, cz)) {
            c.sample_height(&self.config, wx, wz)
        } else {
            fbm(
                wx * self.config.noise_scale,
                wz * self.config.noise_scale,
                self.config.octaves,
                self.seed,
            ) * self.config.max_height
        }
    }

    pub fn get_chunk(&self, cx: i32, cz: i32) -> Option<&Chunk> {
        self.chunks.get(&(cx, cz))
    }

    pub fn load_chunk(&mut self, cx: i32, cz: i32, lod: u32) {
        let lod = lod.min(self.config.lod_levels - 1);
        let chunk = build_chunk(cx, cz, lod, &self.config, self.seed);
        self.chunks.insert((cx, cz), chunk);
    }

    /// Update loaded chunks and their LOD based on camera position.
    pub fn update_lod(&mut self, camera: Vec3, view_radius: i32) {
        let cs = self.config.chunk_size as f32;
        let ccx = (camera.x / cs).floor() as i32;
        let ccz = (camera.z / cs).floor() as i32;

        for cz in (ccz - view_radius)..=(ccz + view_radius) {
            for cx in (ccx - view_radius)..=(ccx + view_radius) {
                let dx = (cx - ccx) as f32;
                let dz = (cz - ccz) as f32;
                let dist = (dx * dx + dz * dz).sqrt() * cs;
                let lod = (dist / self.config.lod_dist) as u32;
                let lod = lod.min(self.config.lod_levels - 1);

                let needs_rebuild = self.chunks.get(&(cx, cz)).is_none_or(|c| c.lod != lod);

                if needs_rebuild {
                    self.load_chunk(cx, cz, lod);
                }
            }
        }

        // Unload distant chunks.
        let far = view_radius + 2;
        self.chunks
            .retain(|&(cx, cz), _| (cx - ccx).abs() <= far && (cz - ccz).abs() <= far);
    }
}
