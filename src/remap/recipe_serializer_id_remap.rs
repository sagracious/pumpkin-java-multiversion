use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::mappings::MappingData;

#[must_use]
pub fn remap_recipe_serializer_id_for_version(
    recipe_serializer_id: u16,
    version: JavaMinecraftVersion,
) -> u16 {
    MappingData::get()
        .composed(version)
        .recipe_serializers
        .map(u32::from(recipe_serializer_id))
        .and_then(|id| u16::try_from(id).ok())
        .unwrap_or(0)
}
