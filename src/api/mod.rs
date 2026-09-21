pub mod connection;
pub mod entity_data;
pub mod protocol;
pub mod rewriter;
pub mod types;
pub mod wrapper;

pub use crate::data::mappings::{
    ComposedMappings, IdMapping, MappingData, StepMappings, TagEntry, TagMappings,
};
pub use connection::{
    EntityTracker, UserConnection, bind_player, is_bound, remove_connection, remove_player,
    with_connection,
};
pub use entity_data::{EntityDataEntry, EntityDataListT, MetaValue};
pub use protocol::{Ctx, Handler, IdPass, Protocol, Registered, Registry};
pub use types::*;
pub use wrapper::{PacketWrapper, Translated};

use pumpkin_protocol::ser::{ReadingError, WritingError};
use pumpkin_util::version::JavaMinecraftVersion;

/// One boundary of the ViaBackwards chain, `from` newer than `to`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub from: JavaMinecraftVersion,
    pub to: JavaMinecraftVersion,
}

#[derive(Debug)]
pub enum TranslateError {
    Read(ReadingError),
    Write(WritingError),
    Unsupported(&'static str),
    TrailingBytes(usize),
}

impl From<ReadingError> for TranslateError {
    fn from(error: ReadingError) -> Self {
        Self::Read(error)
    }
}

impl From<WritingError> for TranslateError {
    fn from(error: WritingError) -> Self {
        Self::Write(error)
    }
}

impl std::fmt::Display for TranslateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(error) => write!(f, "read: {error}"),
            Self::Write(error) => write!(f, "write: {error}"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::TrailingBytes(count) => write!(f, "{count} trailing bytes"),
        }
    }
}
