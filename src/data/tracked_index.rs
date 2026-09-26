//! `ViaBackwards`' tracked field index rules for the 26.3 to 1.16.2 chain.

use pumpkin_data::entity::EntityType;
use pumpkin_util::version::JavaMinecraftVersion;

use crate::data::entity_types::entity_type_absent_on_version;
use crate::data::mappings::MappingData;

/// Entity types with the `AgeableMob` layout on 26.3.
static AGEABLE: &[u16] = &[
    4, 7, 11, 19, 20, 21, 26, 30, 36, 37, 55, 56, 59, 62, 63, 65, 67, 79, 87, 88, 89, 92, 97, 99,
    101, 105, 111, 114, 119, 122, 130, 132, 134, 138, 141, 143, 145, 152, 155, 156,
];

/// Drops `index` when it is one of `removed`, otherwise shifts it down by the
/// number of removed fields below it.
fn without(index: u8, removed: &[u8]) -> Option<u8> {
    if removed.contains(&index) {
        return None;
    }
    Some(index - removed.iter().filter(|&&r| r < index).count() as u8)
}

fn absent(entity_type: u16, version: JavaMinecraftVersion) -> bool {
    entity_type_absent_on_version(entity_type, version)
        || MappingData::get()
            .composed(version)
            .entities
            .map(u32::from(entity_type))
            .is_none()
}

/// Maps a 26.3 tracked field index of `entity_type` onto `version`, `None` when
/// the client has no such field and the entry has to be left out.
#[must_use]
pub fn tracked_index_for_version(
    entity_type: u16,
    index: u8,
    version: JavaMinecraftVersion,
) -> Option<u8> {
    // The shared per-entity index table currently ends at 1.16.2. Do not
    // silently reuse that layout for earlier clients while their rules are
    // still being ported.
    if version < JavaMinecraftVersion::V_1_16_2 {
        return None;
    }
    // An entity type the client does not have is spawned as a stand in whose
    // layout only shares the base fields 0 to 7.
    let absent = absent(entity_type, version);
    if index >= 8 && absent {
        return None;
    }
    // Every pose added since 1.16 arrived with the entity that uses it, so a
    // stand in would send an ordinal the client's pose enum has no entry for.
    if index == 6 && absent {
        return None;
    }
    if version >= JavaMinecraftVersion::V_26_2 {
        return Some(index);
    }
    let mut index = index;

    // 26.1: cube mobs had not become ageable yet.
    let is_cube = entity_type == EntityType::SLIME.id
        || entity_type == EntityType::MAGMA_CUBE.id
        || entity_type == EntityType::SULFUR_CUBE.id;
    if is_cube {
        index = without(index, &[16, 17])?;
    }

    // 1.21.11: no age lock, no sound variants, no "villager data finalized".
    if version <= JavaMinecraftVersion::V_1_21_11 && !is_cube {
        let removed: &[u8] =
            if entity_type == EntityType::COW.id || entity_type == EntityType::CHICKEN.id {
                &[17, 19]
            } else if entity_type == EntityType::PIG.id || entity_type == EntityType::VILLAGER.id {
                &[17, 20]
            } else if entity_type == EntityType::CAT.id {
                &[17, 24]
            } else if entity_type == EntityType::ZOMBIE_VILLAGER.id {
                &[21]
            } else if entity_type == EntityType::TADPOLE.id
                || AGEABLE.binary_search(&entity_type).is_ok()
            {
                &[17]
            } else {
                &[]
            };
        index = without(index, removed)?;
    }

    // 1.21.7: player fields sat in a different order, and the shoulder parrots
    // were NBT compounds rather than variant ids.
    if version <= JavaMinecraftVersion::V_1_21_7 && entity_type == EntityType::PLAYER.id {
        index = match index {
            15 => 18,
            16 => 17,
            17 => 15,
            18 => 16,
            19 | 20 => return None,
            other => other,
        };
    }

    // 1.21.5: hanging entities had no direction field.
    if version <= JavaMinecraftVersion::V_1_21_5
        && (entity_type == EntityType::PAINTING.id
            || entity_type == EntityType::ITEM_FRAME.id
            || entity_type == EntityType::GLOW_ITEM_FRAME.id)
    {
        index = without(index, &[8])?;
    }

    index_before_1_21_5(entity_type, index, version)
}

/// The 1.21.5 to 1.21 steps, applied to an index in 1.21.5 numbering.
fn index_before_1_21_5(entity_type: u16, index: u8, version: JavaMinecraftVersion) -> Option<u8> {
    let mut index = index;

    // 1.21.4: registry backed variants were not data driven yet, the minecart
    // display block was a plain int plus a "present" flag, the experience orb
    // value sat in its own spawn packet and saddles were a metadata flag.
    if version <= JavaMinecraftVersion::V_1_21_4 {
        let dropped = (is_minecart(entity_type) && index == 11)
            || (entity_type == EntityType::MOOSHROOM.id && index == 17)
            || (entity_type == EntityType::CHICKEN.id && index == 17)
            || (entity_type == EntityType::COW.id && index == 17)
            || (entity_type == EntityType::PIG.id && index == 18)
            || (entity_type == EntityType::WOLF.id && (index == 22 || index == 23))
            || (entity_type == EntityType::CAT.id && index == 19)
            || (entity_type == EntityType::FROG.id && index == 17)
            || (entity_type == EntityType::EXPERIENCE_ORB.id && index == 8);
        if dropped {
            return None;
        }
        let shifted = (is_minecart(entity_type) && index >= 13)
            || (entity_type == EntityType::PIG.id && index == 17)
            || (entity_type == EntityType::DOLPHIN.id && index >= 17)
            || (entity_type == EntityType::TURTLE.id && index >= 17)
            || (entity_type == EntityType::STRIDER.id && index >= 19);
        if shifted {
            index += 1;
        }
    }

    // 1.21.2: the creaking had no tearing down flag or home position, and the
    // salmon size was a string.
    if version <= JavaMinecraftVersion::V_1_21_2
        && ((entity_type == EntityType::CREAKING.id && index >= 18)
            || (entity_type == EntityType::SALMON.id && index == 17))
    {
        return None;
    }

    // 1.21: no creaking, one boat type with the wood kind at index 11, no "in
    // ground" flag on arrows and no baby flag on water creatures.
    if version <= JavaMinecraftVersion::V_1_21 {
        if entity_type == EntityType::CREAKING.id && index >= 16 {
            return None;
        }
        if is_boat(entity_type) && index >= 11 {
            index += 1;
        }
        if is_arrow(entity_type) {
            index = without(index, &[10])?;
        }
        if is_water_creature(entity_type) {
            index = without(index, &[16])?;
        }
    }

    // 1.20.5: the 1.21 step moves no index, and the painting variant is a
    // holder over a data driven registry on 1.21 but a plain id into the
    // client's own painting list on 1.20.5.
    if version <= JavaMinecraftVersion::V_1_20_5
        && entity_type == EntityType::PAINTING.id
        && index == 8
    {
        return None;
    }

    index_before_1_20_5(entity_type, index, version)
}

/// The 1.20.5 to 1.20.2 steps, applied to an index in 1.20.5 numbering.
fn index_before_1_20_5(entity_type: u16, index: u8, version: JavaMinecraftVersion) -> Option<u8> {
    let mut index = index;

    // 1.20.3: the area effect cloud still carried its own colour field and the
    // llama still carried the carpet colour in metadata.
    if version <= JavaMinecraftVersion::V_1_20_3 {
        // 1.20.5 folded the cloud colour into the `entity_effect` particle as a
        // trailing ARGB int, which 1.20.3 does not read.
        if entity_type == EntityType::AREA_EFFECT_CLOUD.id {
            if index == 10 {
                return None;
            }
            if index >= 9 {
                index += 1;
            }
        }
        if is_llama(entity_type) && index >= 20 {
            index += 1;
        }
        if entity_type == EntityType::WOLF.id {
            index = without(index, &[22])?;
        }
    }

    // 1.20.2: primed TNT had no block state field yet.
    if version <= JavaMinecraftVersion::V_1_20_2 && entity_type == EntityType::TNT.id {
        index = without(index, &[9])?;
    }

    index_before_1_20_2(entity_type, index, version)
}

/// The 1.20.2 to 1.16.2 steps, applied to an index in 1.20.2 numbering.
fn index_before_1_20_2(entity_type: u16, index: u8, version: JavaMinecraftVersion) -> Option<u8> {
    let mut index = index;

    // 1.20: display entities had no position/rotation interpolation duration.
    if version <= JavaMinecraftVersion::V_1_20 && is_display(entity_type) {
        index = without(index, &[10])?;
    }

    // 1.19.3: every horse still carried the owner uuid at index 18.
    if version <= JavaMinecraftVersion::V_1_19_3 && is_abstract_horse(entity_type) && index >= 18 {
        index += 1;
    }

    // 1.19: the allay could neither dance nor duplicate yet.
    if version <= JavaMinecraftVersion::V_1_19
        && entity_type == EntityType::ALLAY.id
        && (index == 16 || index == 17)
    {
        return None;
    }

    // 1.18.2: the goat had no horns.
    if version <= JavaMinecraftVersion::V_1_18_2
        && entity_type == EntityType::GOAT.id
        && (index == 18 || index == 19)
    {
        return None;
    }

    // 1.16.4 and below: the shulker still had its attachment position at 17,
    // and 1.17 inserted the frozen ticks counter at 7 for every entity.
    if version <= JavaMinecraftVersion::V_1_16_4 {
        if entity_type == EntityType::SHULKER.id && index >= 17 {
            index += 1;
        }
        index = without(index, &[7])?;
    }

    Some(index)
}

fn is_display(entity_type: u16) -> bool {
    [
        EntityType::BLOCK_DISPLAY,
        EntityType::ITEM_DISPLAY,
        EntityType::TEXT_DISPLAY,
    ]
    .iter()
    .any(|kind| kind.id == entity_type)
}

fn is_abstract_horse(entity_type: u16) -> bool {
    [
        EntityType::HORSE,
        EntityType::DONKEY,
        EntityType::MULE,
        EntityType::SKELETON_HORSE,
        EntityType::ZOMBIE_HORSE,
        EntityType::LLAMA,
        EntityType::TRADER_LLAMA,
        EntityType::CAMEL,
    ]
    .iter()
    .any(|kind| kind.id == entity_type)
}

fn is_llama(entity_type: u16) -> bool {
    [EntityType::LLAMA, EntityType::TRADER_LLAMA]
        .iter()
        .any(|kind| kind.id == entity_type)
}

fn is_minecart(entity_type: u16) -> bool {
    [
        EntityType::MINECART,
        EntityType::CHEST_MINECART,
        EntityType::FURNACE_MINECART,
        EntityType::HOPPER_MINECART,
        EntityType::TNT_MINECART,
        EntityType::SPAWNER_MINECART,
        EntityType::COMMAND_BLOCK_MINECART,
    ]
    .iter()
    .any(|kind| kind.id == entity_type)
}

fn is_boat(entity_type: u16) -> bool {
    [
        EntityType::OAK_BOAT,
        EntityType::OAK_CHEST_BOAT,
        EntityType::SPRUCE_BOAT,
        EntityType::SPRUCE_CHEST_BOAT,
        EntityType::BIRCH_BOAT,
        EntityType::BIRCH_CHEST_BOAT,
        EntityType::JUNGLE_BOAT,
        EntityType::JUNGLE_CHEST_BOAT,
        EntityType::ACACIA_BOAT,
        EntityType::ACACIA_CHEST_BOAT,
        EntityType::CHERRY_BOAT,
        EntityType::CHERRY_CHEST_BOAT,
        EntityType::DARK_OAK_BOAT,
        EntityType::DARK_OAK_CHEST_BOAT,
        EntityType::PALE_OAK_BOAT,
        EntityType::PALE_OAK_CHEST_BOAT,
        EntityType::MANGROVE_BOAT,
        EntityType::MANGROVE_CHEST_BOAT,
        EntityType::POPLAR_BOAT,
        EntityType::POPLAR_CHEST_BOAT,
        EntityType::BAMBOO_RAFT,
        EntityType::BAMBOO_CHEST_RAFT,
    ]
    .iter()
    .any(|kind| kind.id == entity_type)
}

fn is_arrow(entity_type: u16) -> bool {
    [
        EntityType::ARROW,
        EntityType::SPECTRAL_ARROW,
        EntityType::TRIDENT,
    ]
    .iter()
    .any(|kind| kind.id == entity_type)
}

fn is_water_creature(entity_type: u16) -> bool {
    [
        EntityType::DOLPHIN,
        EntityType::SQUID,
        EntityType::GLOW_SQUID,
    ]
    .iter()
    .any(|kind| kind.id == entity_type)
}

#[cfg(test)]
mod tests {
    use super::*;
    use JavaMinecraftVersion as V;

    #[test]
    fn older_than_the_audited_index_floor_fails_closed() {
        assert_eq!(tracked_index_for_version(EntityType::PIG.id, 18, V::V_1_15_2), None);
        assert_eq!(tracked_index_for_version(EntityType::PIG.id, 18, V::V_1_16_2), Some(18));
    }

    #[test]
    fn fox_flags_move_down_one_on_1_21_11() {
        // 26.3 fox: 17 age locked, 18 variant, 19 flags; 1.21.11: 17 variant, 18 flags.
        let fox = EntityType::FOX.id;
        assert_eq!(tracked_index_for_version(fox, 19, V::V_1_21_11), Some(18));
        assert_eq!(tracked_index_for_version(fox, 17, V::V_1_21_11), None);
        assert_eq!(tracked_index_for_version(fox, 16, V::V_1_21_11), Some(16));
        assert_eq!(tracked_index_for_version(fox, 19, V::V_26_2), Some(19));
    }

    #[test]
    fn cow_loses_two_fields_on_1_21_11() {
        let cow = EntityType::COW.id;
        assert_eq!(tracked_index_for_version(cow, 18, V::V_1_21_11), Some(17));
        assert_eq!(tracked_index_for_version(cow, 19, V::V_1_21_11), None);
    }

    #[test]
    fn item_frame_item_is_index_8_on_1_21_5() {
        let frame = EntityType::ITEM_FRAME.id;
        assert_eq!(tracked_index_for_version(frame, 9, V::V_1_21_5), Some(8));
        assert_eq!(tracked_index_for_version(frame, 9, V::V_1_21_6), Some(9));
        assert_eq!(tracked_index_for_version(frame, 8, V::V_1_21_5), None);
    }

    #[test]
    fn pig_boost_time_moves_to_18_on_1_21_4() {
        // 26.3 pig: 18 boost time, 19 variant; 1.21.4: 17 saddled, 18 boost time.
        let pig = EntityType::PIG.id;
        assert_eq!(tracked_index_for_version(pig, 18, V::V_1_21_4), Some(18));
        assert_eq!(tracked_index_for_version(pig, 19, V::V_1_21_4), None);
        assert_eq!(tracked_index_for_version(pig, 18, V::V_1_21_5), Some(17));
    }

    #[test]
    fn furnace_minecart_fuel_moves_up_on_1_21_4() {
        let cart = EntityType::FURNACE_MINECART.id;
        assert_eq!(tracked_index_for_version(cart, 13, V::V_1_21_4), Some(14));
        assert_eq!(tracked_index_for_version(cart, 11, V::V_1_21_4), None);
        assert_eq!(tracked_index_for_version(cart, 13, V::V_1_21_5), Some(13));
    }

    #[test]
    fn dolphin_fields_on_older_versions() {
        // 26.3 dolphin: 16 baby, 17 age locked, 18 got fish, 19 moistness.
        let dolphin = EntityType::DOLPHIN.id;
        assert_eq!(
            tracked_index_for_version(dolphin, 18, V::V_1_21_4),
            Some(18)
        );
        assert_eq!(tracked_index_for_version(dolphin, 18, V::V_1_21), Some(17));
        assert_eq!(tracked_index_for_version(dolphin, 16, V::V_1_21), None);
    }

    #[test]
    fn boat_paddles_shift_on_1_21() {
        let boat = EntityType::OAK_BOAT.id;
        assert_eq!(tracked_index_for_version(boat, 11, V::V_1_21), Some(12));
        assert_eq!(tracked_index_for_version(boat, 11, V::V_1_21_2), Some(11));
    }

    #[test]
    fn arrow_loses_in_ground_on_1_21() {
        let arrow = EntityType::ARROW.id;
        assert_eq!(tracked_index_for_version(arrow, 11, V::V_1_21), Some(10));
        assert_eq!(tracked_index_for_version(arrow, 10, V::V_1_21), None);
    }

    #[test]
    fn absent_entity_keeps_only_base_fields() {
        let creaking = EntityType::CREAKING.id;
        assert_eq!(tracked_index_for_version(creaking, 5, V::V_1_21), Some(5));
        assert_eq!(tracked_index_for_version(creaking, 9, V::V_1_21), None);
    }

    #[test]
    fn painting_variant_is_left_out_on_1_20_5() {
        let painting = EntityType::PAINTING.id;
        assert_eq!(tracked_index_for_version(painting, 9, V::V_1_21), Some(8));
        assert_eq!(tracked_index_for_version(painting, 9, V::V_1_20_5), None);
        assert_eq!(tracked_index_for_version(painting, 2, V::V_1_20_5), Some(2));
    }

    #[test]
    fn wolf_keeps_1_21_indices_on_1_20_5() {
        let wolf = EntityType::WOLF.id;
        for index in 0..=24 {
            assert_eq!(
                tracked_index_for_version(wolf, index, V::V_1_20_5),
                tracked_index_for_version(wolf, index, V::V_1_21),
                "wolf index {index}"
            );
        }
        assert_eq!(tracked_index_for_version(wolf, 21, V::V_1_20_5), Some(20));
        assert_eq!(tracked_index_for_version(wolf, 23, V::V_1_20_5), None);
    }

    #[test]
    fn area_effect_cloud_gains_a_colour_field_on_1_20_3() {
        // 26.3 cloud: 8 radius, 9 waiting, 10 particle.
        let cloud = EntityType::AREA_EFFECT_CLOUD.id;
        assert_eq!(tracked_index_for_version(cloud, 8, V::V_1_20_3), Some(8));
        assert_eq!(tracked_index_for_version(cloud, 9, V::V_1_20_3), Some(10));
        assert_eq!(tracked_index_for_version(cloud, 10, V::V_1_20_3), None);
        assert_eq!(tracked_index_for_version(cloud, 9, V::V_1_20_2), Some(10));
        assert_eq!(tracked_index_for_version(cloud, 9, V::V_1_20_5), Some(9));
        assert_eq!(tracked_index_for_version(cloud, 10, V::V_1_20_5), Some(10));
    }

    #[test]
    fn llama_variant_moves_up_for_the_carpet_colour_on_1_20_3() {
        for llama in [EntityType::LLAMA.id, EntityType::TRADER_LLAMA.id] {
            assert_eq!(tracked_index_for_version(llama, 20, V::V_1_20_5), Some(19));
            assert_eq!(tracked_index_for_version(llama, 21, V::V_1_20_5), Some(20));
            assert_eq!(tracked_index_for_version(llama, 20, V::V_1_20_3), Some(19));
            assert_eq!(tracked_index_for_version(llama, 21, V::V_1_20_3), Some(21));
            assert_eq!(tracked_index_for_version(llama, 21, V::V_1_20_2), Some(21));
        }
        assert_eq!(
            tracked_index_for_version(EntityType::DONKEY.id, 20, V::V_1_20_3),
            tracked_index_for_version(EntityType::DONKEY.id, 20, V::V_1_20_5)
        );
    }

    #[test]
    fn tnt_loses_its_block_state_on_1_20_2() {
        let tnt = EntityType::TNT.id;
        assert_eq!(tracked_index_for_version(tnt, 9, V::V_1_20_3), Some(9));
        assert_eq!(tracked_index_for_version(tnt, 9, V::V_1_20_2), None);
        assert_eq!(tracked_index_for_version(tnt, 8, V::V_1_20_2), Some(8));
    }

    #[test]
    fn breeze_pose_is_left_out_on_1_20_2() {
        let breeze = EntityType::BREEZE.id;
        assert_eq!(tracked_index_for_version(breeze, 6, V::V_1_20_3), Some(6));
        assert_eq!(tracked_index_for_version(breeze, 6, V::V_1_20_2), None);
        assert_eq!(tracked_index_for_version(breeze, 9, V::V_1_20_2), None);
        assert_eq!(tracked_index_for_version(breeze, 2, V::V_1_20_2), Some(2));
    }

    #[test]
    fn entities_added_in_1_20_5_keep_only_base_fields_on_1_20_3() {
        for entity_type in [EntityType::ARMADILLO.id, EntityType::BOGGED.id] {
            assert_eq!(
                tracked_index_for_version(entity_type, 5, V::V_1_20_3),
                Some(5)
            );
            assert_eq!(
                tracked_index_for_version(entity_type, 16, V::V_1_20_3),
                None
            );
            assert!(tracked_index_for_version(entity_type, 16, V::V_1_20_5).is_some());
        }
    }

    #[test]
    fn display_loses_the_interpolation_duration_on_1_20() {
        let display = EntityType::TEXT_DISPLAY.id;
        assert_eq!(
            tracked_index_for_version(display, 10, V::V_1_20_2),
            Some(10)
        );
        assert_eq!(tracked_index_for_version(display, 10, V::V_1_20), None);
        assert_eq!(tracked_index_for_version(display, 11, V::V_1_20), Some(10));
        assert_eq!(
            tracked_index_for_version(display, 11, V::V_1_19_4),
            Some(10)
        );
        assert_eq!(tracked_index_for_version(display, 11, V::V_1_19_3), None);
    }

    #[test]
    fn horses_gain_the_owner_uuid_on_1_19_3() {
        let horse = EntityType::HORSE.id;
        assert_eq!(tracked_index_for_version(horse, 19, V::V_1_19_4), Some(18));
        assert_eq!(tracked_index_for_version(horse, 19, V::V_1_19_3), Some(19));
        assert_eq!(tracked_index_for_version(horse, 18, V::V_1_19_3), Some(17));
        let llama = EntityType::LLAMA.id;
        assert_eq!(tracked_index_for_version(llama, 19, V::V_1_19_3), Some(19));
        assert_eq!(tracked_index_for_version(llama, 20, V::V_1_19_3), Some(20));
        assert_eq!(tracked_index_for_version(llama, 21, V::V_1_19_3), Some(22));
    }

    #[test]
    fn allay_dancing_is_left_out_on_1_19() {
        let allay = EntityType::ALLAY.id;
        assert_eq!(tracked_index_for_version(allay, 16, V::V_1_19_1), Some(16));
        assert_eq!(tracked_index_for_version(allay, 16, V::V_1_19), None);
        assert_eq!(tracked_index_for_version(allay, 15, V::V_1_19), Some(15));
        assert_eq!(tracked_index_for_version(allay, 15, V::V_1_18), None);
    }

    #[test]
    fn goat_horns_are_left_out_on_1_18() {
        let goat = EntityType::GOAT.id;
        assert_eq!(tracked_index_for_version(goat, 19, V::V_1_19), Some(18));
        assert_eq!(tracked_index_for_version(goat, 19, V::V_1_18_2), None);
        assert_eq!(tracked_index_for_version(goat, 20, V::V_1_18_2), None);
        assert_eq!(tracked_index_for_version(goat, 18, V::V_1_18_2), Some(17));
    }

    #[test]
    fn shulker_keeps_its_attachment_position_on_1_16_4() {
        let shulker = EntityType::SHULKER.id;
        assert_eq!(tracked_index_for_version(shulker, 17, V::V_1_17), Some(17));
        assert_eq!(
            tracked_index_for_version(shulker, 17, V::V_1_16_4),
            Some(17)
        );
        assert_eq!(
            tracked_index_for_version(shulker, 18, V::V_1_16_2),
            Some(18)
        );
        assert_eq!(
            tracked_index_for_version(shulker, 16, V::V_1_16_2),
            Some(15)
        );
    }

    #[test]
    fn frozen_ticks_is_left_out_on_1_16_2() {
        for entity_type in [
            EntityType::COW.id,
            EntityType::ZOMBIE.id,
            EntityType::ARMOR_STAND.id,
        ] {
            assert_eq!(
                tracked_index_for_version(entity_type, 7, V::V_1_17),
                Some(7)
            );
            for version in [V::V_1_16_2, V::V_1_16_3, V::V_1_16_4] {
                assert_eq!(tracked_index_for_version(entity_type, 7, version), None);
                assert_eq!(tracked_index_for_version(entity_type, 6, version), Some(6));
                assert_eq!(tracked_index_for_version(entity_type, 8, version), Some(7));
            }
        }
        assert_eq!(
            tracked_index_for_version(EntityType::HORSE.id, 19, V::V_1_16_2),
            Some(18)
        );
    }

    #[test]
    fn pose_is_left_out_for_an_entity_the_version_lacks() {
        let warden = EntityType::WARDEN.id;
        assert_eq!(tracked_index_for_version(warden, 6, V::V_1_19), Some(6));
        assert_eq!(tracked_index_for_version(warden, 6, V::V_1_18), None);
        assert_eq!(tracked_index_for_version(warden, 5, V::V_1_18), Some(5));
    }

    #[test]
    fn nothing_else_moves_between_1_20_2_and_1_17() {
        for entity_type in [
            EntityType::COW.id,
            EntityType::CREEPER.id,
            EntityType::VILLAGER.id,
            EntityType::WOLF.id,
            EntityType::OAK_BOAT.id,
            EntityType::PLAYER.id,
        ] {
            for index in 0..=24 {
                let expected = tracked_index_for_version(entity_type, index, V::V_1_20_2);
                for version in [
                    V::V_1_20,
                    V::V_1_19_4,
                    V::V_1_19_3,
                    V::V_1_19_1,
                    V::V_1_19,
                    V::V_1_18_2,
                    V::V_1_18,
                    V::V_1_17_1,
                    V::V_1_17,
                ] {
                    assert_eq!(
                        tracked_index_for_version(entity_type, index, version),
                        expected,
                        "entity {entity_type} index {index} on {version:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn player_customisation_is_17_on_1_21_7() {
        let player = EntityType::PLAYER.id;
        assert_eq!(tracked_index_for_version(player, 16, V::V_1_21_7), Some(17));
        assert_eq!(tracked_index_for_version(player, 16, V::V_1_21_9), Some(16));
    }
}
