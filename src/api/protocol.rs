use std::collections::HashMap;

use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::connection::UserConnection;
use crate::api::wrapper::PacketWrapper;
use crate::api::{Step, TranslateError};
use crate::data::mappings::{ComposedMappings, StepMappings};
use crate::packet::mappings::PacketId;

pub type Handler = fn(&mut PacketWrapper, &mut UserConnection, &Ctx) -> Result<(), TranslateError>;

/// Runs in the layout core wrote, with the composed 26.3 to target ids.
pub type IdPass = fn(
    &mut PacketWrapper,
    &mut UserConnection,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<(), TranslateError>;

pub struct Ctx<'a> {
    pub step: Step,
    pub mappings: &'a StepMappings,
    /// The version whose layout the payload is in.
    pub layout: JavaMinecraftVersion,
}

/// Handlers are keyed by the address of the `PacketId` they belong to, which
/// is why `packet::mappings` declares them as statics.
type Key = usize;

#[must_use]
pub fn packet_key(packet: &'static PacketId) -> Key {
    std::ptr::from_ref(packet) as Key
}

const fn identity(handler: Handler) -> Registered {
    Registered {
        handler,
        layout: false,
    }
}

const fn layout(handler: Handler) -> Registered {
    Registered {
        handler,
        layout: true,
    }
}

fn cancel_handler(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.cancel();
    Ok(())
}

#[derive(Clone, Copy)]
pub struct Registered {
    pub handler: Handler,
    /// Converts the packet between the step's `from` and `to` layouts, so it is
    /// owed only where core did not already write at or below `from`.
    pub layout: bool,
}

impl Registered {
    /// Whether the step still owes this handler for a payload in `layout`.
    #[must_use]
    pub fn runs(&self, from: JavaMinecraftVersion, layout: JavaMinecraftVersion) -> bool {
        !self.layout || from.protocol_version() <= layout.protocol_version()
    }
}

#[derive(Default)]
pub struct Registry {
    clientbound: HashMap<Key, Registered>,
    serverbound: HashMap<Key, Registered>,
}

impl Registry {
    pub fn clientbound(&mut self, p: &'static PacketId, h: Handler) {
        self.clientbound.insert(packet_key(p), identity(h));
    }

    /// For a handler that rewrites the payload from the step's `from` layout
    /// into its `to` layout.
    pub fn clientbound_layout(&mut self, p: &'static PacketId, h: Handler) {
        self.clientbound.insert(packet_key(p), layout(h));
    }

    pub fn cancel_clientbound(&mut self, p: &'static PacketId) {
        self.clientbound
            .insert(packet_key(p), identity(cancel_handler));
    }

    pub fn serverbound(&mut self, p: &'static PacketId, h: Handler) {
        self.serverbound.insert(packet_key(p), identity(h));
    }

    pub fn serverbound_layout(&mut self, p: &'static PacketId, h: Handler) {
        self.serverbound.insert(packet_key(p), layout(h));
    }

    pub fn cancel_serverbound(&mut self, p: &'static PacketId) {
        self.serverbound
            .insert(packet_key(p), identity(cancel_handler));
    }

    #[must_use]
    pub fn clientbound_handler(&self, p: &'static PacketId) -> Option<Registered> {
        self.clientbound.get(&packet_key(p)).copied()
    }

    #[must_use]
    pub fn serverbound_handler(&self, p: &'static PacketId) -> Option<Registered> {
        self.serverbound.get(&packet_key(p)).copied()
    }

    pub fn clientbound_keys(&self) -> impl Iterator<Item = Key> + '_ {
        self.clientbound.keys().copied()
    }

    pub fn serverbound_keys(&self) -> impl Iterator<Item = Key> + '_ {
        self.serverbound.keys().copied()
    }
}

pub trait Protocol: Sync {
    fn step(&self) -> Step;
    fn register(&self, reg: &mut Registry);
    fn init(&self, _conn: &mut UserConnection) {}
}
