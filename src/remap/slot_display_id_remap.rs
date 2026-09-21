use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::mappings::MappingData;

#[must_use]
pub fn remap_slot_display_id_for_version(
    slot_display_id: u32,
    version: JavaMinecraftVersion,
) -> u32 {
    MappingData::get()
        .composed(version)
        .slot_displays
        .map(slot_display_id)
        .unwrap_or(0)
}
