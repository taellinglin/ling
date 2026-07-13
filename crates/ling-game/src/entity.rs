//! Entity-Component-System primitives.

use std::collections::HashMap;

pub type EntityId = u32;

/// Sparse-set component storage for one component type.
pub struct ComponentStore<T> {
    dense: Vec<T>,
    sparse: HashMap<EntityId, usize>,
}

impl<T> ComponentStore<T> {
    pub fn new() -> Self {
        Self { dense: vec![], sparse: HashMap::new() }
    }

    pub fn insert(&mut self, entity: EntityId, component: T) {
        if let Some(&idx) = self.sparse.get(&entity) {
            self.dense[idx] = component;
        } else {
            let idx = self.dense.len();
            self.dense.push(component);
            self.sparse.insert(entity, idx);
        }
    }

    pub fn get(&self, entity: EntityId) -> Option<&T> {
        self.sparse.get(&entity).map(|&i| &self.dense[i])
    }

    pub fn get_mut(&mut self, entity: EntityId) -> Option<&mut T> {
        self.sparse.get(&entity).map(|&i| &mut self.dense[i])
    }

    pub fn remove(&mut self, entity: EntityId) {
        self.sparse.remove(&entity);
    }

    pub fn iter(&self) -> impl Iterator<Item = (&EntityId, &T)> {
        self.sparse.iter().map(|(id, &i)| (id, &self.dense[i]))
    }
}

impl<T> Default for ComponentStore<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple entity allocator.
pub struct Entity {
    next: EntityId,
    free: Vec<EntityId>,
}

impl Entity {
    pub fn new() -> Self {
        Self { next: 1, free: vec![] }
    }

    pub fn spawn(&mut self) -> EntityId {
        self.free.pop().unwrap_or_else(|| {
            let id = self.next;
            self.next += 1;
            id
        })
    }

    pub fn despawn(&mut self, id: EntityId) {
        self.free.push(id);
    }
}

impl Default for Entity {
    fn default() -> Self {
        Self::new()
    }
}
