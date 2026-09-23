use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;

use pumpkin_plugin_api::Player;
use pumpkin_util::version::JavaMinecraftVersion;

#[derive(Default)]
pub struct EntityTracker {
    pub client_entity_id: Option<i32>,
    entities: HashMap<i32, u16>,
    pub min_y: i32,
    pub height: i32,
}

impl EntityTracker {
    pub fn add(&mut self, id: i32, entity_type: u16) {
        self.entities.insert(id, entity_type);
    }

    pub fn remove(&mut self, id: i32) {
        self.entities.remove(&id);
    }

    #[must_use]
    pub fn entity_type(&self, id: i32) -> Option<u16> {
        self.entities.get(&id).copied()
    }

    pub fn clear(&mut self) {
        self.entities.clear();
    }
}

pub struct UserConnection {
    pub id: u64,
    pub version: JavaMinecraftVersion,
    pub entity_tracker: EntityTracker,
    pub bound: bool,
    storages: HashMap<TypeId, Box<dyn Any>>,
}

impl UserConnection {
    #[must_use]
    pub fn new(id: u64, version: JavaMinecraftVersion) -> Self {
        Self {
            id,
            version,
            entity_tracker: EntityTracker::default(),
            bound: false,
            storages: HashMap::new(),
        }
    }

    #[must_use]
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.storages
            .get(&TypeId::of::<T>())
            .and_then(|value| value.downcast_ref())
    }

    pub fn get_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.storages
            .get_mut(&TypeId::of::<T>())
            .and_then(|value| value.downcast_mut())
    }

    pub fn put<T: 'static>(&mut self, value: T) {
        self.storages.insert(TypeId::of::<T>(), Box::new(value));
    }
}

thread_local! {
    static CONNECTIONS: RefCell<HashMap<u64, UserConnection>> = RefCell::new(HashMap::new());
    static PLAYERS: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());
}

/// The wasm host runs one instance per plugin and serialises calls into it.
pub fn with_connection<R>(
    key: u64,
    version: JavaMinecraftVersion,
    f: impl FnOnce(&mut UserConnection) -> R,
) -> R {
    CONNECTIONS.with_borrow_mut(|connections| {
        let connection = connections
            .entry(key)
            .or_insert_with(|| UserConnection::new(key, version));
        connection.version = version;
        f(connection)
    })
}

pub fn remove_connection(key: u64) {
    CONNECTIONS.with_borrow_mut(|connections| connections.remove(&key));
}

#[must_use]
pub fn player_key(player: &Player) -> u64 {
    let uuid = player.get_id();
    uuid.high.rotate_left(32) ^ uuid.low
}

#[must_use]
pub fn is_bound(key: u64) -> bool {
    CONNECTIONS.with_borrow(|connections| connections.get(&key).is_some_and(|c| c.bound))
}

/// Remembers which player a connection belongs to, so the leave event can drop its state.
pub fn bind_player(key: u64, version: JavaMinecraftVersion, player: &Player) {
    let player = player_key(player);
    with_connection(key, version, |connection| connection.bound = true);
    PLAYERS.with_borrow_mut(|players| players.insert(player, key));
}

pub fn remove_player(player: &Player) {
    let player = player_key(player);
    if let Some(key) = PLAYERS.with_borrow_mut(|players| players.remove(&player)) {
        remove_connection(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Marker(u8);

    #[test]
    fn state_survives_between_calls_and_is_dropped_on_remove() {
        with_connection(7, JavaMinecraftVersion::V_1_20, |connection| {
            connection.entity_tracker.add(3, 12);
            connection.put(Marker(9));
        });
        with_connection(7, JavaMinecraftVersion::V_1_19, |connection| {
            assert_eq!(connection.version, JavaMinecraftVersion::V_1_19);
            assert_eq!(connection.entity_tracker.entity_type(3), Some(12));
            assert_eq!(connection.get::<Marker>().unwrap().0, 9);
        });
        remove_connection(7);
        with_connection(7, JavaMinecraftVersion::V_1_19, |connection| {
            assert!(connection.entity_tracker.entity_type(3).is_none());
            assert!(connection.get::<Marker>().is_none());
        });
        remove_connection(7);
    }

    #[test]
    fn clearing_the_tracker_forgets_every_entity() {
        let mut tracker = EntityTracker::default();
        tracker.add(1, 2);
        tracker.client_entity_id = Some(1);
        tracker.remove(1);
        assert!(tracker.entity_type(1).is_none());
        tracker.add(4, 5);
        tracker.clear();
        assert!(tracker.entity_type(4).is_none());
    }

    #[test]
    fn a_connection_starts_unbound() {
        with_connection(8, JavaMinecraftVersion::V_1_20, |_| {});
        assert!(!is_bound(8));
        remove_connection(8);
    }
}
