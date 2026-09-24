//! Per-version ids for the registries that appear inside tag groups.

use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::entity_types::stand_in_type_for_version;
use crate::data::mappings::{IdMapping, MappingData};

fn map(mapping: &IdMapping, id: u16) -> Option<u16> {
    mapping
        .map(u32::from(id))
        .and_then(|id| u16::try_from(id).ok())
}

/// Maps a 26.3 block id onto `version`.
#[must_use]
pub fn block_id_for_version(id: u16, version: JavaMinecraftVersion) -> Option<u16> {
    map(&MappingData::get().composed(version).blocks, id)
}

/// Maps a 26.3 item id onto `version`.
#[must_use]
pub fn item_id_for_version(id: u16, version: JavaMinecraftVersion) -> Option<u16> {
    map(&MappingData::get().composed(version).items, id)
}

/// Maps a 26.3 entity type id onto `version`.
#[must_use]
pub fn entity_type_id_for_version(id: u16, version: JavaMinecraftVersion) -> Option<u16> {
    map(
        &MappingData::get().composed(version).entities,
        stand_in_type_for_version(id, version),
    )
}
