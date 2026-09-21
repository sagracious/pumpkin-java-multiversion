use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::mappings::MappingData;

#[must_use]
pub fn remap_item_id_for_version(item_id: u16, version: JavaMinecraftVersion) -> u16 {
    MappingData::get()
        .composed(version)
        .items
        .map(u32::from(item_id))
        .and_then(|id| u16::try_from(id).ok())
        .unwrap_or(0)
}

#[must_use]
pub fn remap_item_id_from_version(item_id: u16, version: JavaMinecraftVersion) -> u16 {
    MappingData::get()
        .composed(version)
        .items_inverse()
        .map(u32::from(item_id))
        .and_then(|id| u16::try_from(id).ok())
        .unwrap_or(0)
}
