use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::mappings::MappingData;

#[must_use]
pub fn remap_sound_id_for_version(sound_id: u16, version: JavaMinecraftVersion) -> u16 {
    MappingData::get()
        .composed(version)
        .sounds
        .map(u32::from(sound_id))
        .and_then(|id| u16::try_from(id).ok())
        .unwrap_or(0)
}
