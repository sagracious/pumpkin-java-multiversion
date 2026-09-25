use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use pumpkin_plugin_api::Player;
use pumpkin_util::version::JavaMinecraftVersion;

const PRE_PLAYER_CONNECTION_TTL: Duration = Duration::from_secs(300);
const MAX_UNBOUND_CONNECTIONS: usize = 2_048;
const PRUNE_EVERY_NEW_CONNECTIONS: usize = 32;

#[derive(Default)]
pub struct EntityTracker {
    pub client_entity_id: Option<i32>,
    entities: HashMap<i32, TrackedEntityTypes>,
    pub min_y: i32,
    pub height: i32,
}

#[derive(Clone, Copy)]
struct TrackedEntityTypes {
    server: u16,
    client: u16,
}

/// Server-side game clock shared by protocol rewrites that need tick-relative values.
pub(crate) struct GameTimeStorage {
    pub(crate) game_time: i64,
}

impl EntityTracker {
    pub fn add(&mut self, id: i32, entity_type: u16) {
        self.add_mapped(id, entity_type, entity_type);
    }

    pub(crate) fn add_mapped(&mut self, id: i32, server_type: u16, client_type: u16) {
        self.entities.insert(
            id,
            TrackedEntityTypes {
                server: server_type,
                client: client_type,
            },
        );
    }

    pub fn remove(&mut self, id: i32) {
        self.entities.remove(&id);
    }

    #[must_use]
    pub fn entity_type(&self, id: i32) -> Option<u16> {
        self.entities.get(&id).map(|entity| entity.server)
    }

    #[must_use]
    pub(crate) fn client_entity_type(&self, id: i32) -> Option<u16> {
        self.entities.get(&id).map(|entity| entity.client)
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
    last_used: Instant,
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
            last_used: Instant::now(),
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
    static NEW_CONNECTIONS: Cell<usize> = const { Cell::new(0) };
}

fn prune_unbound(connections: &mut HashMap<u64, UserConnection>, now: Instant) {
    connections.retain(|_, connection| {
        connection.bound || now.duration_since(connection.last_used) < PRE_PLAYER_CONNECTION_TTL
    });

    let mut unbound: Vec<_> = connections
        .iter()
        .filter(|(_, connection)| !connection.bound)
        .map(|(id, connection)| (*id, connection.last_used))
        .collect();
    // This runs immediately before a new state entry is inserted.
    let keep = MAX_UNBOUND_CONNECTIONS.saturating_sub(1);
    if unbound.len() <= keep {
        return;
    }
    let excess = unbound.len() - keep;
    unbound.sort_unstable_by_key(|(_, created_at)| *created_at);
    for (id, _) in unbound.into_iter().take(excess) {
        connections.remove(&id);
    }
}

/// The wasm host runs one instance per plugin and serialises calls into it.
pub fn with_connection<R>(
    key: u64,
    version: JavaMinecraftVersion,
    f: impl FnOnce(&mut UserConnection) -> R,
) -> R {
    CONNECTIONS.with_borrow_mut(|connections| {
        let now = Instant::now();
        if connections.get(&key).is_some_and(|connection| {
            !connection.bound
                && now.duration_since(connection.last_used) >= PRE_PLAYER_CONNECTION_TTL
        }) {
            connections.remove(&key);
        }
        if !connections.contains_key(&key) {
            let should_prune = NEW_CONNECTIONS.with(|counter| {
                let next = counter.get() + 1;
                counter.set(if next >= PRUNE_EVERY_NEW_CONNECTIONS {
                    0
                } else {
                    next
                });
                next >= PRUNE_EVERY_NEW_CONNECTIONS
            });
            if should_prune
                || connections
                    .values()
                    .filter(|connection| !connection.bound)
                    .count()
                    >= MAX_UNBOUND_CONNECTIONS
            {
                prune_unbound(connections, now);
            }
        }
        let connection = connections
            .entry(key)
            .or_insert_with(|| UserConnection::new(key, version));
        connection.last_used = now;
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

    #[test]
    fn pruning_removes_expired_and_excess_unbound_connections_only() {
        let now = Instant::now();
        let mut connections = HashMap::new();
        let mut expired = UserConnection::new(1, JavaMinecraftVersion::V_1_20);
        expired.last_used = now - PRE_PLAYER_CONNECTION_TTL - Duration::from_secs(1);
        connections.insert(1, expired);

        let mut active_player = UserConnection::new(2, JavaMinecraftVersion::V_1_20);
        active_player.last_used = now - PRE_PLAYER_CONNECTION_TTL - Duration::from_secs(1);
        active_player.bound = true;
        connections.insert(2, active_player);

        for id in 10..(10 + MAX_UNBOUND_CONNECTIONS as u64 + 1) {
            let mut connection = UserConnection::new(id, JavaMinecraftVersion::V_1_20);
            connection.last_used = now - Duration::from_secs(id - 9);
            connections.insert(id, connection);
        }

        prune_unbound(&mut connections, now);
        assert!(!connections.contains_key(&1), "expired login state is removed");
        assert!(connections.contains_key(&2), "live player state is retained");
        assert!(!connections.contains_key(&(9 + MAX_UNBOUND_CONNECTIONS as u64)));
        assert!(!connections.contains_key(&(10 + MAX_UNBOUND_CONNECTIONS as u64)));
        assert_eq!(
            connections.values().filter(|connection| !connection.bound).count(),
            MAX_UNBOUND_CONNECTIONS - 1
        );
    }
}
