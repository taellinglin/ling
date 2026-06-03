//! ling-game smoke tests: entity allocation and the sparse component store.

use ling_game::entity::{Entity, ComponentStore};

#[test]
fn spawns_unique_entities() {
    let mut world = Entity::new();
    let a = world.spawn();
    let b = world.spawn();
    let c = world.spawn();
    assert_ne!(a, b);
    assert_ne!(b, c);
    assert_ne!(a, c);
}

#[test]
fn despawned_ids_are_recycled() {
    let mut world = Entity::new();
    let a = world.spawn();
    world.despawn(a);
    let reused = world.spawn();
    assert_eq!(a, reused, "a freed entity id is reused by the next spawn");
}

#[test]
fn component_store_insert_get_remove() {
    let mut world = Entity::new();
    let e = world.spawn();
    let mut store: ComponentStore<i32> = ComponentStore::new();

    assert!(store.get(e).is_none());
    store.insert(e, 42);
    assert_eq!(store.get(e), Some(&42));

    if let Some(v) = store.get_mut(e) {
        *v += 1;
    }
    assert_eq!(store.get(e), Some(&43));

    store.remove(e);
    assert!(store.get(e).is_none(), "removed component is gone");
}
