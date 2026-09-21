use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::mappings::MappingData;

/// `brigadier:string`, sent for argument types the client does not know.
const STRING_ARGUMENT: u32 = 5;

#[must_use]
pub fn remap_argument_type_id_for_version(
    argument_type_id: u32,
    version: JavaMinecraftVersion,
) -> u32 {
    MappingData::get()
        .composed(version)
        .argumenttypes
        .map(argument_type_id)
        .unwrap_or(STRING_ARGUMENT)
}
