use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::mappings::MappingData;

#[must_use]
pub fn remap_block_state_for_version(state_id: u16, version: JavaMinecraftVersion) -> u16 {
    MappingData::get()
        .composed(version)
        .blockstates
        .map(u32::from(state_id))
        .and_then(|id| u16::try_from(id).ok())
        .unwrap_or(0)
}
