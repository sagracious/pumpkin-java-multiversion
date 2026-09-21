use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::mappings::MappingData;

#[must_use]
pub fn remap_particle_id_for_version(particle_id: u16, version: JavaMinecraftVersion) -> u16 {
    MappingData::get()
        .composed(version)
        .particles
        .map(u32::from(particle_id))
        .and_then(|id| u16::try_from(id).ok())
        .unwrap_or(0)
}
