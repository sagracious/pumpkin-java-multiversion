use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::mappings::MappingData;

#[must_use]
pub fn remap_painting_variant_id_for_version(
    painting_variant_id: u32,
    version: JavaMinecraftVersion,
) -> u32 {
    MappingData::get()
        .composed(version)
        .paintings
        .map(painting_variant_id)
        .unwrap_or(0)
}

#[must_use]
pub fn remap_motive_id_for_version(motive_id: u32, version: JavaMinecraftVersion) -> u32 {
    remap_painting_variant_id_for_version(motive_id, version)
}
