//! World descriptor — scene config, entity placement, chunk manager integration.

use glam::{Mat4, Quat, Vec3};
use std::collections::HashMap;

// ── World geometry ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub enum WorldGeometry {
    /// Infinite flat plane (standard Euclidean world).
    #[default]
    Flat,
    /// Inside a sphere — gravity points outward, normals flipped.
    HyperbolicSphere { radius: f32, curvature: f32 },
    /// Standard sphere surface (exterior).
    Sphere { radius: f32 },
    /// Toroidal world.
    Torus { major_r: f32, minor_r: f32 },
}

// ── Entity placement ──────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct EntityDesc {
    pub id: u64,
    pub kind: String,
    pub pos: Vec3,
    pub rot: Quat,
    pub scale: Vec3,
    pub props: HashMap<String, String>,
}

impl EntityDesc {
    pub fn new(id: u64, kind: impl Into<String>, pos: Vec3) -> Self {
        Self {
            id,
            kind: kind.into(),
            pos,
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            props: HashMap::new(),
        }
    }

    pub fn transform(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rot, self.pos)
    }
}

// ── World descriptor ──────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct WorldDescriptor {
    pub name: String,
    pub seed: u64,
    pub geometry: WorldGeometry,
    pub spawn_pos: Vec3,
    pub entities: Vec<EntityDesc>,
    pub sky_shader: String, // e.g. "cell_day", "psychedelic", "hyperbolic"
    pub cel_shading: bool,
    pub long_exposure_blend: f32, // 0 = off, 0.03 = trail effect
}

impl Default for WorldDescriptor {
    fn default() -> Self {
        Self {
            name: "New World".into(),
            seed: 0,
            geometry: WorldGeometry::Flat,
            spawn_pos: Vec3::ZERO,
            entities: Vec::new(),
            sky_shader: "cell_day".into(),
            cel_shading: true,
            long_exposure_blend: 0.0,
        }
    }
}

impl WorldDescriptor {
    pub fn place(&mut self, entity: EntityDesc) {
        self.entities.push(entity);
    }

    pub fn remove(&mut self, id: u64) {
        self.entities.retain(|e| e.id != id);
    }

    pub fn find(&self, id: u64) -> Option<&EntityDesc> {
        self.entities.iter().find(|e| e.id == id)
    }
}

// ── Room / interior zone ──────────────────────────────────────────────────────

/// A discrete interior space the player can enter/exit (house, dungeon room, etc.).
#[derive(Clone, Debug)]
pub struct Room {
    pub id: u64,
    pub name: String,
    pub floor_plan: RoomShape,
    pub ceiling_h: f32,
    pub portal_pos: Vec3, // world position of the door
    pub entities: Vec<EntityDesc>,
}

#[derive(Clone, Debug)]
pub enum RoomShape {
    Box { half_extents: Vec3 },
    Pentagon { side_len: f32, height: f32 },
    Hexagon { side_len: f32, height: f32 },
    Custom { verts: Vec<[f32; 2]>, height: f32 },
}

impl Room {
    pub fn new(id: u64, name: impl Into<String>, shape: RoomShape, portal_pos: Vec3) -> Self {
        Self {
            id,
            name: name.into(),
            floor_plan: shape,
            ceiling_h: 3.0,
            portal_pos,
            entities: Vec::new(),
        }
    }
}

// ── World state ───────────────────────────────────────────────────────────────

/// Full mutable runtime world.
pub struct WorldState {
    pub desc: WorldDescriptor,
    pub rooms: Vec<Room>,
    /// Entity that "is" the player (id index).
    pub player_id: Option<u64>,
    /// Current room the player is in (None = outdoors).
    pub current_room: Option<u64>,
    pub next_entity_id: u64,
}

impl WorldState {
    pub fn new(desc: WorldDescriptor) -> Self {
        Self {
            desc,
            rooms: Vec::new(),
            player_id: None,
            current_room: None,
            next_entity_id: 1,
        }
    }

    pub fn spawn_entity(&mut self, kind: impl Into<String>, pos: Vec3) -> u64 {
        let id = self.next_entity_id;
        self.next_entity_id += 1;
        self.desc.place(EntityDesc::new(id, kind, pos));
        id
    }

    pub fn add_room(&mut self, room: Room) {
        self.rooms.push(room);
    }

    /// Check if `pos` is close enough to a room portal to trigger an enter/exit.
    pub fn portal_at(&self, pos: Vec3, threshold: f32) -> Option<u64> {
        self.rooms
            .iter()
            .find(|r| (r.portal_pos - pos).length() < threshold)
            .map(|r| r.id)
    }
}
