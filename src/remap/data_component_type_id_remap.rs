use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::mappings::MappingData;

#[must_use]
pub fn remap_data_component_type_id_for_version(
    data_component_type_id: u32,
    version: JavaMinecraftVersion,
) -> u32 {
    MappingData::get()
        .composed(version)
        .data_component_type
        .map(data_component_type_id)
        .unwrap_or(0)
}

#[must_use]
pub fn remap_data_component_type_id_from_version(
    data_component_type_id: u32,
    version: JavaMinecraftVersion,
) -> u32 {
    MappingData::get()
        .composed(version)
        .data_component_type_inverse()
        .map(data_component_type_id)
        .unwrap_or(0)
}
