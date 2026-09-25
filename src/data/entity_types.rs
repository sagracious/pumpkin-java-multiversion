//! Entity types each version lacks, derived from `assets/tracked_data`.

use pumpkin_data::entity::EntityType;
use pumpkin_util::version::JavaMinecraftVersion::{self, *};

/// Entity replacements used by ViaBackwards from the 1.21.11 to 1.21.9 step.
#[must_use]
pub fn stand_in_type_for_version(entity_type: u16, version: JavaMinecraftVersion) -> u16 {
    // Sulfur cubes only exist from 26.2. ViaBackwards renders them as slimes
    // for clients whose registry predates the new entity.
    if version < V_26_2 && entity_type == EntityType::SULFUR_CUBE.id {
        return EntityType::SLIME.id;
    }
    if version > V_1_21_9 {
        return entity_type;
    }
    match entity_type {
        id if id == EntityType::NAUTILUS.id => EntityType::SQUID.id,
        id if id == EntityType::ZOMBIE_NAUTILUS.id => EntityType::GLOW_SQUID.id,
        id if id == EntityType::CAMEL_HUSK.id => EntityType::CAMEL.id,
        id if id == EntityType::PARCHED.id => EntityType::SKELETON.id,
        _ => entity_type,
    }
}

static ABSENT_V_1_16_2: &[u16] = &[
    1, 2, 4, 7, 8, 13, 15, 16, 17, 18, 19, 20, 24, 28, 31, 33, 35, 56, 59, 61, 62, 63, 70, 73, 76,
    83, 84, 85, 89, 91, 93, 96, 98, 107, 122, 129, 133, 134, 135, 146, 147, 156,
];
static ABSENT_V_1_17: &[u16] = &[
    1, 2, 4, 8, 13, 15, 16, 17, 18, 19, 20, 24, 28, 31, 33, 35, 56, 59, 70, 73, 76, 83, 84, 89, 91,
    93, 96, 98, 107, 122, 129, 133, 134, 135, 146, 147, 156,
];
static ABSENT_V_1_19: &[u16] = &[
    4, 15, 16, 17, 18, 19, 20, 28, 31, 33, 59, 70, 73, 84, 89, 93, 98, 122, 133, 135, 147, 156,
];
static ABSENT_V_1_19_3: &[u16] = &[
    4, 15, 16, 17, 18, 20, 28, 31, 33, 59, 70, 73, 84, 89, 93, 98, 122, 133, 135, 147, 156,
];
static ABSENT_V_1_19_4: &[u16] = &[
    4, 16, 17, 18, 20, 28, 31, 33, 59, 84, 89, 93, 98, 133, 147, 156,
];
static ABSENT_V_1_20_3: &[u16] = &[4, 16, 18, 20, 28, 31, 33, 59, 84, 89, 93, 98, 133, 156];
static ABSENT_V_1_20_5: &[u16] = &[20, 28, 31, 33, 59, 84, 89, 98, 133, 156];
static ABSENT_V_1_21_4: &[u16] = &[20, 28, 33, 59, 84, 89, 98, 133, 156];
static ABSENT_V_1_21_6: &[u16] = &[20, 28, 33, 84, 89, 98, 133, 156];
static ABSENT_V_1_21_9: &[u16] = &[20, 33, 89, 98, 133, 156];
static ABSENT_V_1_21_11: &[u16] = &[33, 133];
static ABSENT_V_26_2: &[u16] = &[33];

/// Whether `version` has no entity of 26.3 type `entity_type` at all.
///
/// Such an entity is spawned as a stand in type whose metadata layout only
/// shares the base entity fields.
#[must_use]
pub fn entity_type_absent_on_version(entity_type: u16, version: JavaMinecraftVersion) -> bool {
    let table = match version {
        V_1_16_2 | V_1_16_3 | V_1_16_4 => ABSENT_V_1_16_2,
        V_1_17 | V_1_17_1 | V_1_18 | V_1_18_2 => ABSENT_V_1_17,
        V_1_19 | V_1_19_1 => ABSENT_V_1_19,
        V_1_19_3 => ABSENT_V_1_19_3,
        V_1_19_4 | V_1_20 | V_1_20_2 => ABSENT_V_1_19_4,
        V_1_20_3 => ABSENT_V_1_20_3,
        V_1_20_5 | V_1_21 | V_1_21_2 => ABSENT_V_1_20_5,
        V_1_21_4 | V_1_21_5 => ABSENT_V_1_21_4,
        V_1_21_6 | V_1_21_7 => ABSENT_V_1_21_6,
        V_1_21_9 => ABSENT_V_1_21_9,
        V_1_21_11 | V_26_1 => ABSENT_V_1_21_11,
        V_26_2 => ABSENT_V_26_2,
        _ => return false,
    };
    table.binary_search(&entity_type).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_type_is_absent_until_the_version_that_added_it() {
        assert!(entity_type_absent_on_version(
            EntityType::CREAKING.id,
            V_1_21
        ));
        assert!(!entity_type_absent_on_version(
            EntityType::CREAKING.id,
            V_1_21_4
        ));
        assert!(entity_type_absent_on_version(EntityType::FROG.id, V_1_18_2));
        assert!(!entity_type_absent_on_version(EntityType::FROG.id, V_1_19));
        assert!(!entity_type_absent_on_version(EntityType::PIG.id, V_1_16_2));
        assert!(!entity_type_absent_on_version(EntityType::PIG.id, V_26_3));
    }

    #[test]
    fn via_1_21_11_entity_types_use_their_documented_stand_ins() {
        for (source, stand_in) in [
            (EntityType::NAUTILUS.id, EntityType::SQUID.id),
            (EntityType::ZOMBIE_NAUTILUS.id, EntityType::GLOW_SQUID.id),
            (EntityType::CAMEL_HUSK.id, EntityType::CAMEL.id),
            (EntityType::PARCHED.id, EntityType::SKELETON.id),
        ] {
            assert_eq!(stand_in_type_for_version(source, V_1_21_9), stand_in);
            assert_eq!(stand_in_type_for_version(source, V_1_16_2), stand_in);
            assert!(entity_type_absent_on_version(source, V_1_21_9));
            assert_eq!(stand_in_type_for_version(source, V_1_21_11), source);
            assert_eq!(stand_in_type_for_version(source, V_26_2), source);
        }
    }

    #[test]
    fn sulfur_cubes_use_the_26_1_slime_stand_in() {
        for version in [V_26_1, V_1_21_11, V_1_16_2] {
            assert_eq!(
                stand_in_type_for_version(EntityType::SULFUR_CUBE.id, version),
                EntityType::SLIME.id,
                "{version}"
            );
        }
        assert_eq!(
            stand_in_type_for_version(EntityType::SULFUR_CUBE.id, V_26_2),
            EntityType::SULFUR_CUBE.id
        );
        assert_eq!(
            stand_in_type_for_version(EntityType::SULFUR_CUBE.id, V_26_3),
            EntityType::SULFUR_CUBE.id
        );
    }

    #[test]
    fn every_table_is_sorted() {
        for table in [
            ABSENT_V_1_16_2,
            ABSENT_V_1_17,
            ABSENT_V_1_19,
            ABSENT_V_1_19_3,
            ABSENT_V_1_19_4,
            ABSENT_V_1_20_3,
            ABSENT_V_1_20_5,
            ABSENT_V_1_21_4,
            ABSENT_V_1_21_6,
            ABSENT_V_1_21_9,
            ABSENT_V_1_21_11,
            ABSENT_V_26_2,
        ] {
            assert!(table.windows(2).all(|pair| pair[0] < pair[1]));
        }
    }
}
