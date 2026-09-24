use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::entity_types::stand_in_type_for_version;
use crate::data::mappings::MappingData;

#[must_use]
pub fn remap_entity_id_for_version(entity_id: u16, version: JavaMinecraftVersion) -> u16 {
    let entity_id = stand_in_type_for_version(entity_id, version);
    MappingData::get()
        .composed(version)
        .entities
        .map(u32::from(entity_id))
        .and_then(|id| u16::try_from(id).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::entity::EntityType;
    use pumpkin_util::version::JavaMinecraftVersion as V;

    #[test]
    fn new_entity_spawns_reuse_their_via_backwards_stand_in_ids() {
        for (source, stand_in) in [
            (EntityType::NAUTILUS.id, EntityType::SQUID.id),
            (EntityType::ZOMBIE_NAUTILUS.id, EntityType::GLOW_SQUID.id),
            (EntityType::CAMEL_HUSK.id, EntityType::CAMEL.id),
            (EntityType::PARCHED.id, EntityType::SKELETON.id),
        ] {
            assert_eq!(
                remap_entity_id_for_version(source, V::V_1_21_9),
                remap_entity_id_for_version(stand_in, V::V_1_21_9)
            );
        }
    }
}

#[must_use]
pub fn remap_object_type_for_version(
    entity_id: u16,
    _version: pumpkin_util::version::JavaMinecraftVersion,
) -> u8 {
    match entity_id {
        10..=17 => 1,
        74 => 2,
        3 => 3,
        82 | 24 | 25 | 26 | 27 | 28 | 29 => 10,
        140 => 50,
        44 => 51,
        4 => 60,
        122 => 61,
        42 => 62,
        54 => 63,
        121 => 64,
        46 => 65,
        153 => 66,
        117 => 67,
        78 => 68,
        52 => 70,
        75 | 58 => 71,
        50 => 72,
        104 => 73,
        49 => 75,
        55 => 76,
        77 => 77,
        2 => 78,
        48 => 79,
        56 => 90,
        124 => 91,
        40 => 93,
        143 => 94,
        _ => entity_id as u8,
    }
}
