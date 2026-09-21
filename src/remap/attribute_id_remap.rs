use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::mappings::MappingData;

#[must_use]
pub fn remap_attribute_id_for_version(attribute_id: u32, version: JavaMinecraftVersion) -> u32 {
    MappingData::get()
        .composed(version)
        .attributes
        .map(attribute_id)
        .unwrap_or(0)
}
