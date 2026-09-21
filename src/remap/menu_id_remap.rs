use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::mappings::MappingData;

#[must_use]
pub fn remap_menu_id_for_version(menu_id: u8, version: JavaMinecraftVersion) -> u8 {
    MappingData::get()
        .composed(version)
        .menus
        .map(u32::from(menu_id))
        .and_then(|id| u8::try_from(id).ok())
        .unwrap_or(0)
}
