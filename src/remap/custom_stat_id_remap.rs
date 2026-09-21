use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::mappings::MappingData;

#[must_use]
pub fn remap_custom_stat_id_for_version(custom_stat_id: u32, version: JavaMinecraftVersion) -> u32 {
    MappingData::get()
        .composed(version)
        .statistics
        .map(custom_stat_id)
        .unwrap_or(0)
}

#[must_use]
pub fn remap_statistic_id_for_version(statistic_id: u32, version: JavaMinecraftVersion) -> u32 {
    remap_custom_stat_id_for_version(statistic_id, version)
}
