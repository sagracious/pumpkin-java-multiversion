use pumpkin_util::version::JavaMinecraftVersion;

/// No mapping file in the chain renumbers environment attributes.
#[must_use]
pub fn remap_environment_attribute_id_for_version(
    environment_attribute_id: u32,
    version: JavaMinecraftVersion,
) -> u32 {
    let _ = version;
    environment_attribute_id
}
