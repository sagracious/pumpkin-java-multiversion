use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::mappings::MappingData;

#[must_use]
pub fn remap_enchantment_id_for_version(enchantment_id: u32, version: JavaMinecraftVersion) -> u32 {
    MappingData::get()
        .composed(version)
        .enchantments
        .map(enchantment_id)
        .unwrap_or(0)
}
