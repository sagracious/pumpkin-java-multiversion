use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::mappings::MappingData;

#[must_use]
pub fn remap_block_entity_type_id_for_version(
    block_entity_type_id: u32,
    version: JavaMinecraftVersion,
) -> u32 {
    MappingData::get()
        .composed(version)
        .blockentities
        .map(block_entity_type_id)
        .unwrap_or(0)
}
