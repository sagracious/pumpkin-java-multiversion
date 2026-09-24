use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_protocol::{
    ClientPacket,
    java::client::play::{CSpawnEntity, attribute_name_to_id},
};
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::connection::GameTimeStorage;
use crate::api::entity_data::{EntityDataEntry, EntityDataListT, MetaValue};
use crate::api::rewriter::particle::write_particle;
use crate::api::types::{VAR_INT, VAR_LONG};
use crate::api::{PacketWrapper, TranslateError, UserConnection};
use crate::data::entity_data_types::{
    MetaKind, meta_data_type_id_for_name, meta_data_type_id_for_version, meta_kind,
};
use crate::data::mappings::ComposedMappings;
use crate::data::tracked_index::tracked_index_for_version;
use crate::packet::mappings::PacketId;

const WOLF_ANGER_END_TIME_INDEX_26_3: u8 = 22;
const BEE_ANGER_END_TIME_INDEX_26_3: u8 = 19;

/// The 26.3 entity type the tracker holds for the spawn payload in `wrapper`.
#[must_use]
pub fn spawned_type(wrapper: &PacketWrapper, connection: &UserConnection) -> Option<u16> {
    let mut cursor = wrapper.remaining();
    let entity_id = cursor.get_var_int().ok()?.0;
    connection.entity_tracker.entity_type(entity_id)
}

pub fn read_spawn(
    wrapper: &PacketWrapper,
    layout: JavaMinecraftVersion,
) -> Result<CSpawnEntity, TranslateError> {
    Ok(CSpawnEntity::read_packet_data(
        wrapper.remaining(),
        &layout,
    )?)
}

/// Sends `value` under `packet` instead of what is left of the input.
pub fn replace<P: ClientPacket>(
    wrapper: &mut PacketWrapper,
    packet: &'static PacketId,
    value: &P,
    layout: JavaMinecraftVersion,
) -> Result<(), TranslateError> {
    let mut buf = Vec::new();
    value.write_packet_data(&mut buf, &layout)?;
    wrapper.replace_remaining(buf);
    wrapper.set_packet(packet);
    Ok(())
}

/// Leaves out the entries `layout` cannot read, and renumbers the rest.
fn rewrite_entries(
    entity_type: u16,
    entries: &[EntityDataEntry],
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
    game_time: i64,
) -> Vec<EntityDataEntry> {
    entries
        .iter()
        .filter_map(|entry| {
            let index = tracked_index_for_version(entity_type, entry.index, layout)?;
            let (serializer, value) =
                rewrite_entry_value(entity_type, entry, layout, ids, game_time)?;
            Some(EntityDataEntry {
                index,
                serializer,
                value,
            })
        })
        .collect()
}

fn rewrite_entry_value(
    entity_type: u16,
    entry: &EntityDataEntry,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
    game_time: i64,
) -> Option<(i32, MetaValue)> {
    let anger_time = (entity_type == pumpkin_data::entity::EntityType::WOLF.id
        && entry.index == WOLF_ANGER_END_TIME_INDEX_26_3)
        || (entity_type == pumpkin_data::entity::EntityType::BEE.id
            && entry.index == BEE_ANGER_END_TIME_INDEX_26_3);
    if layout <= JavaMinecraftVersion::V_1_21_9
        && anger_time
        && meta_kind(entry.serializer) == Some(MetaKind::VarLong)
    {
        let MetaValue::Raw(raw) = &entry.value else {
            return None;
        };
        let mut read = raw.as_slice();
        let absolute_time = VAR_LONG.read(&mut read).ok()?.0;
        if !read.is_empty() {
            return None;
        }
        let remaining = i32::try_from(
            (i128::from(absolute_time) - i128::from(game_time)).clamp(0, i128::from(i32::MAX)),
        )
        .ok()?;
        let mut value = Vec::new();
        VAR_INT.write(&mut value, &VarInt(remaining)).ok()?;
        return Some((
            meta_data_type_id_for_name("int", layout)?,
            MetaValue::Raw(value),
        ));
    }

    Some((
        meta_data_type_id_for_version(entry.serializer, layout)?,
        rewrite_value(&entry.value, layout, ids)?,
    ))
}

fn rewrite_value(
    value: &MetaValue,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Option<MetaValue> {
    Some(match value {
        MetaValue::Raw(_) | MetaValue::Item(_) => value.clone(),
        // A state the client lacks falls back to air, as every other state id does.
        MetaValue::BlockState(state) => MetaValue::BlockState(block_state(*state, ids)),
        // Zero is "no block state" and is not an id.
        MetaValue::OptionalBlockState(0) => MetaValue::OptionalBlockState(0),
        MetaValue::OptionalBlockState(state) => {
            MetaValue::OptionalBlockState(block_state(*state, ids))
        }
        // The particle is converted here and not in the writer, because a
        // particle the client cannot show leaves the whole entry out.
        MetaValue::Particle(particle) => {
            let mut out = Vec::new();
            if !write_particle(&mut out, particle, layout, ids).ok()? {
                return None;
            }
            MetaValue::Raw(out)
        }
        MetaValue::Particles(particles) => {
            let mut body = Vec::new();
            let mut kept = 0;
            for particle in particles {
                let mut one = Vec::new();
                if write_particle(&mut one, particle, layout, ids).ok()? {
                    body.extend(one);
                    kept += 1;
                }
            }
            let mut out = Vec::new();
            out.write_var_int(&VarInt(kept)).ok()?;
            out.extend(body);
            MetaValue::Raw(out)
        }
        // A holder: zero carries the variant inline, which cannot be renumbered.
        MetaValue::PaintingVariant(0) => return None,
        MetaValue::PaintingVariant(variant) => MetaValue::PaintingVariant(
            i32::try_from(ids.paintings.map(u32::try_from(*variant - 1).ok()?)?).ok()? + 1,
        ),
    })
}

fn block_state(state: i32, ids: &ComposedMappings) -> i32 {
    u32::try_from(state)
        .ok()
        .and_then(|state| ids.blockstates.map(state))
        .and_then(|state| i32::try_from(state).ok())
        .unwrap_or(0)
}

/// Core writes the indices, serializer ids and values of 26.3 in the client's
/// layout, so this renumbers all three.
pub fn set_entity_data(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    if layout >= JavaMinecraftVersion::V_26_3 {
        wrapper.passthrough_all();
        return Ok(());
    }
    let entity_id = wrapper.passthrough(&VAR_INT)?;
    let list = EntityDataListT::for_version(layout);
    let entries = wrapper.read(&list)?;
    // Without the entity type there is no index rule to apply, and an entry
    // under the wrong one is fatal to the client.
    let game_time = connection
        .get::<GameTimeStorage>()
        .map_or(0, |storage| storage.game_time);
    let entries = connection
        .entity_tracker
        .entity_type(entity_id.0)
        .map(|entity_type| rewrite_entries(entity_type, &entries, layout, ids, game_time))
        .unwrap_or_default();
    wrapper.write(&list, &entries)
}

/// One attribute, with the id part kept as written so a name can go back out.
struct Attribute {
    id: Option<u32>,
    head: Vec<u8>,
    tail: Vec<u8>,
}

fn read_attributes(
    r: &mut &[u8],
    layout: JavaMinecraftVersion,
) -> Result<Vec<Attribute>, TranslateError> {
    let count = if layout >= JavaMinecraftVersion::V_1_17 {
        r.get_var_int()?.0
    } else {
        r.get_i32_be()?
    };
    let mut attributes = Vec::new();
    for _ in 0..count {
        let before = *r;
        let id = if layout >= JavaMinecraftVersion::V_1_20_5 {
            u32::try_from(r.get_var_int()?.0).ok()
        } else {
            attribute_name_to_id(&r.get_str()?).map(u32::from)
        };
        let head = before[..before.len() - r.len()].to_vec();
        let before = *r;
        r.get_f64_be()?;
        let modifiers = r.get_var_int()?.0;
        for _ in 0..modifiers {
            if layout >= JavaMinecraftVersion::V_1_21 {
                r.get_str()?;
            } else {
                r.get_uuid()?;
            }
            r.get_f64_be()?;
            r.get_u8()?;
        }
        let tail = before[..before.len() - r.len()].to_vec();
        attributes.push(Attribute { id, head, tail });
    }
    Ok(attributes)
}

/// Attribute ids, with the ones the client has no attribute for left out.
pub fn update_attributes(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    if layout >= JavaMinecraftVersion::V_26_3 {
        wrapper.passthrough_all();
        return Ok(());
    }
    wrapper.passthrough(&VAR_INT)?;
    let attributes = {
        let mut cursor = wrapper.remaining();
        let attributes = read_attributes(&mut cursor, layout)?;
        if !cursor.is_empty() {
            return Err(TranslateError::TrailingBytes(cursor.len()));
        }
        attributes
    };

    let kept: Vec<(u32, &Attribute)> = attributes
        .iter()
        .filter_map(|attribute| Some((ids.attributes.map(attribute.id?)?, attribute)))
        .collect();

    let mut out = Vec::new();
    let count = i32::try_from(kept.len()).map_err(|_| TranslateError::Unsupported("attributes"))?;
    if layout >= JavaMinecraftVersion::V_1_17 {
        out.write_var_int(&VarInt(count))?;
    } else {
        out.write_i32_be(count)?;
    }
    for (id, attribute) in kept {
        if layout >= JavaMinecraftVersion::V_1_20_5 {
            out.write_var_int(&VarInt(
                i32::try_from(id).map_err(|_| TranslateError::Unsupported("attribute id"))?,
            ))?;
        } else {
            out.write_slice(&attribute.head)?;
        }
        out.write_slice(&attribute.tail)?;
    }
    wrapper.replace_remaining(out);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::entity_data::TERMINATOR;
    use crate::data::mappings::MappingData;
    use crate::packet::mappings::clientbound;
    use pumpkin_data::attributes::Attributes;
    use pumpkin_data::entity::EntityType;
    use pumpkin_data::particle::Particle;
    use pumpkin_protocol::java::client::play::attribute_id_to_1_16_name;
    use pumpkin_util::version::JavaMinecraftVersion as V;

    /// Shared flags, health, an empty effect particle list, the baby flag and
    /// the variant, at the indices and serializer ids 26.3 gives a pig.
    fn pig_list() -> Vec<u8> {
        let mut out = vec![7u8, 0, 0, 0x08, 9, 3];
        out.extend(20.0f32.to_be_bytes());
        out.extend([10, 17, 0, 16, 8, 1, 19, 28, 2, TERMINATOR]);
        out
    }

    fn translate(payload: &[u8], entity_type: u16, layout: V) -> Vec<u8> {
        let mut connection = UserConnection::new(0, layout);
        connection.entity_tracker.add(7, entity_type);
        let mut wrapper = PacketWrapper::new(&clientbound::play::SET_ENTITY_DATA, payload);
        set_entity_data(
            &mut wrapper,
            &mut connection,
            layout,
            MappingData::get().composed(layout),
        )
        .unwrap();
        wrapper.finish().unwrap().unwrap().payload
    }

    fn health() -> [u8; 4] {
        20.0f32.to_be_bytes()
    }

    /// Indices and serializer ids as ViaBackwards and minecraft-data have them
    /// for each layout.
    #[test]
    fn a_pig_is_renumbered_for_every_layout() {
        let mut v26_2 = vec![7u8, 0, 0, 0x08, 9, 3];
        v26_2.extend(health());
        v26_2.extend([10, 17, 0, 16, 8, 1, 19, 28, 2, TERMINATOR]);

        let mut v1_21_4 = vec![7u8, 0, 0, 0x08, 9, 3];
        v1_21_4.extend(health());
        v1_21_4.extend([10, 18, 0, 16, 8, 1, TERMINATOR]);

        let mut v1_20_3 = vec![7u8, 0, 0, 0x08, 9, 3];
        v1_20_3.extend(health());
        v1_20_3.extend([16, 8, 1, TERMINATOR]);

        let mut v1_18_2 = vec![7u8, 0, 0, 0x08, 9, 2];
        v1_18_2.extend(health());
        v1_18_2.extend([16, 7, 1, TERMINATOR]);

        let mut v1_16_2 = vec![7u8, 0, 0, 0x08, 8, 2];
        v1_16_2.extend(health());
        v1_16_2.extend([15, 7, 1, TERMINATOR]);

        for (layout, want) in [
            (V::V_26_2, v26_2),
            (V::V_1_21_4, v1_21_4),
            (V::V_1_20_3, v1_20_3),
            (V::V_1_18_2, v1_18_2),
            (V::V_1_16_2, v1_16_2),
        ] {
            assert_eq!(
                translate(&pig_list(), EntityType::PIG.id, layout),
                want,
                "{layout}"
            );
        }
    }

    /// The effect particle list arrived in 1.20.5, so 1.20.3 has no id for it.
    #[test]
    fn an_entry_whose_serializer_is_missing_is_left_out() {
        let out = translate(&pig_list(), EntityType::PIG.id, V::V_1_20_3);
        assert!(!out.windows(2).any(|pair| pair == [10, 17]));
    }

    /// 1.20.5 folded the cloud colour into the particle, so the particle entry
    /// cannot be read below it and the waiting flag moves up for the colour.
    #[test]
    fn an_area_effect_cloud_loses_its_particle_below_1_20_5() {
        let effect = u8::try_from(Particle::EntityEffect.to_id()).unwrap();
        let mut payload = vec![7u8, 8, 3];
        payload.extend(3.0f32.to_be_bytes());
        payload.extend([9, 8, 0, 10, 16, effect, 0x11, 0x22, 0x33, 0x44, TERMINATOR]);

        let cloud = EntityType::AREA_EFFECT_CLOUD.id;
        let mut want = vec![7u8, 8, 3];
        want.extend(3.0f32.to_be_bytes());
        want.extend([10, 8, 0, TERMINATOR]);
        assert_eq!(translate(&payload, cloud, V::V_1_20_3), want);

        let mapped = MappingData::get()
            .composed(V::V_1_20_5)
            .particles
            .map(u32::from(Particle::EntityEffect.to_id()))
            .unwrap();
        let mut want = vec![7u8, 8, 3];
        want.extend(3.0f32.to_be_bytes());
        want.extend([9, 8, 0, 10, 17, u8::try_from(mapped).unwrap()]);
        want.extend([0x11, 0x22, 0x33, 0x44, TERMINATOR]);
        assert_eq!(translate(&payload, cloud, V::V_1_20_5), want);
    }

    /// An entity the client has no type for is a stand in, and only the base
    /// fields of the stand in line up.
    #[test]
    fn an_absent_entity_type_keeps_its_base_fields_only() {
        let mut payload = vec![7u8, 0, 0, 0x08, 9, 3];
        payload.extend(20.0f32.to_be_bytes());
        payload.extend([16, 8, 1, TERMINATOR]);
        let out = translate(&payload, EntityType::CREAKING.id, V::V_1_21);
        assert_eq!(out, [7, 0, 0, 0x08, TERMINATOR]);
    }

    #[test]
    fn an_untracked_entity_gets_no_entries_at_all() {
        let mut connection = UserConnection::new(0, V::V_1_20_3);
        let payload = pig_list();
        let mut wrapper = PacketWrapper::new(&clientbound::play::SET_ENTITY_DATA, &payload);
        set_entity_data(
            &mut wrapper,
            &mut connection,
            V::V_1_20_3,
            MappingData::get().composed(V::V_1_20_3),
        )
        .unwrap();
        assert_eq!(wrapper.finish().unwrap().unwrap().payload, [7, TERMINATOR]);
    }

    #[test]
    fn the_native_layout_is_left_alone() {
        let payload = pig_list();
        let out = translate(&payload, EntityType::PIG.id, V::V_26_3);
        assert_eq!(out, payload);
    }

    fn attributes(ids: &[u32], layout: V) -> Vec<u8> {
        let mut out = vec![1u8];
        out.write_var_int(&VarInt(i32::try_from(ids.len()).unwrap()))
            .unwrap();
        for id in ids {
            if layout >= V::V_1_20_5 {
                out.write_var_int(&VarInt(i32::try_from(*id).unwrap()))
                    .unwrap();
            } else {
                out.write_string(attribute_id_to_1_16_name(u8::try_from(*id).unwrap()))
                    .unwrap();
            }
            out.write_f64_be(1.0).unwrap();
            out.write_var_int(&VarInt(0)).unwrap();
        }
        out
    }

    fn translate_attributes(payload: &[u8], layout: V) -> Vec<u8> {
        let mut connection = UserConnection::new(0, layout);
        let mut wrapper = PacketWrapper::new(&clientbound::play::UPDATE_ATTRIBUTES, payload);
        update_attributes(
            &mut wrapper,
            &mut connection,
            layout,
            MappingData::get().composed(layout),
        )
        .unwrap();
        wrapper.finish().unwrap().unwrap().payload
    }

    #[test]
    fn an_attribute_the_client_lacks_is_left_out_and_the_list_recounted() {
        let layout = V::V_1_20_5;
        let ids = MappingData::get().composed(layout);
        let absent = (0..u32::try_from(ids.attributes.len()).unwrap())
            .find(|id| ids.attributes.map(*id).is_none())
            .expect("an attribute 1.20.5 does not have");
        let armor = u32::from(Attributes::ARMOR.id);

        let out = translate_attributes(&attributes(&[armor, absent], layout), layout);
        let mut read = &out[1..];
        assert_eq!(read.get_var_int().unwrap().0, 1);
        assert_eq!(
            u32::try_from(read.get_var_int().unwrap().0).unwrap(),
            ids.attributes.map(armor).unwrap()
        );
        assert_eq!(read.get_f64_be().unwrap(), 1.0);
        assert_eq!(read.get_var_int().unwrap().0, 0);
        assert!(read.is_empty());
    }

    /// Below 1.20.5 core writes the attribute name, so only the drop applies.
    #[test]
    fn the_name_form_keeps_its_names_and_drops_max_absorption_on_1_20() {
        let layout = V::V_1_20;
        let armor = u32::from(Attributes::ARMOR.id);
        let absorption = u32::from(Attributes::MAX_ABSORPTION.id);

        let out = translate_attributes(&attributes(&[armor, absorption], layout), layout);
        let mut read = &out[1..];
        assert_eq!(read.get_var_int().unwrap().0, 1);
        assert_eq!(
            &*read.get_str().unwrap(),
            attribute_id_to_1_16_name(u8::try_from(armor).unwrap())
        );
    }

    #[test]
    fn a_modifier_is_a_uuid_below_1_21_and_a_name_from_it() {
        for layout in [V::V_1_20_5, V::V_1_21] {
            let mut payload = vec![1u8, 1];
            payload
                .write_var_int(&VarInt(i32::from(Attributes::ARMOR.id)))
                .unwrap();
            payload.write_f64_be(1.0).unwrap();
            payload.write_var_int(&VarInt(1)).unwrap();
            if layout >= V::V_1_21 {
                payload.write_string("abc").unwrap();
            } else {
                payload.write_uuid(&uuid::Uuid::from_u128(5)).unwrap();
            }
            payload.write_f64_be(2.0).unwrap();
            payload.write_u8(0).unwrap();

            let out = translate_attributes(&payload, layout);
            assert_eq!(out.len(), payload.len(), "{layout}");
            assert_eq!(out[3..], payload[3..], "{layout}");
        }
    }

    /// The spawn packet is what tells the metadata pass which entity class the
    /// indices belong to.
    #[test]
    fn a_spawn_teaches_the_tracker_what_the_metadata_belongs_to() {
        use pumpkin_protocol::java::client::play::CSpawnEntity;
        use pumpkin_util::math::vector3::Vector3;

        let layout = V::V_1_16_2;
        let spawn = CSpawnEntity::new(
            VarInt(7),
            uuid::Uuid::from_u128(1),
            VarInt(i32::from(EntityType::PIG.id)),
            Vector3::new(1.0, 2.0, 3.0),
            0.0,
            0.0,
            0.0,
            VarInt(0),
            Vector3::new(0.0, 0.0, 0.0),
        );
        let mut payload = Vec::new();
        spawn.write_packet_data(&mut payload, &layout).unwrap();

        let key = 0x656e74_u64;
        crate::api::remove_connection(key);
        let mut data = vec![7u8, 0, 0, 0x08, 9, 3];
        data.extend(health());
        data.push(TERMINATOR);
        let before = crate::pipeline::translate_clientbound(
            key,
            layout,
            5,
            clientbound::play::SET_ENTITY_DATA.v26_3,
            &data,
        )
        .unwrap();
        assert_eq!(before.payload, [7, TERMINATOR]);

        crate::pipeline::translate_clientbound(
            key,
            layout,
            5,
            clientbound::play::ADD_ENTITY.v26_3,
            &payload,
        )
        .unwrap();
        let after = crate::pipeline::translate_clientbound(
            key,
            layout,
            5,
            clientbound::play::SET_ENTITY_DATA.v26_3,
            &data,
        )
        .unwrap();
        let mut want = vec![7u8, 0, 0, 0x08, 8, 2];
        want.extend(health());
        want.push(TERMINATOR);
        assert_eq!(after.payload, want);
        crate::api::remove_connection(key);
    }

    /// The geyser particles are 26.2's, so one of them inside an effect list
    /// goes and the surviving count is written.
    #[test]
    fn a_particle_the_client_lacks_leaves_the_list_shorter() {
        let layout = V::V_1_21_4;
        let effect = u8::try_from(Particle::EntityEffect.to_id()).unwrap();
        let geyser = u8::try_from(Particle::GeyserBase.to_id()).unwrap();

        let mut payload = vec![7u8, 10, 17, 2, effect, 0x11, 0x22, 0x33, 0x44, geyser];
        payload.extend(5i32.to_be_bytes());
        payload.extend(1.0f32.to_be_bytes());
        payload.push(TERMINATOR);

        let mapped = MappingData::get()
            .composed(layout)
            .particles
            .map(u32::from(Particle::EntityEffect.to_id()))
            .unwrap();
        let want = vec![
            7,
            10,
            18,
            1,
            u8::try_from(mapped).unwrap(),
            0x11,
            0x22,
            0x33,
            0x44,
            TERMINATOR,
        ];
        assert_eq!(translate(&payload, EntityType::PIG.id, layout), want);
    }
}

#[cfg(test)]
mod anger_time_tests {
    use super::*;
    use crate::api::connection::with_connection;
    use crate::api::entity_data::TERMINATOR;
    use crate::api::remove_connection;
    use crate::data::entity_data_types::meta_data_type_id_for_name;
    use crate::packet::mappings::clientbound;
    use crate::pipeline::translate_clientbound;
    use pumpkin_data::entity::EntityType;
    use pumpkin_protocol::codec::var_long::VarLong;
    use pumpkin_util::version::JavaMinecraftVersion as V;

    const PLAY: u8 = 5;

    fn anger_payload(entity_id: i32, index: u8, absolute_time: i64) -> Vec<u8> {
        let mut anger = Vec::new();
        VAR_LONG.write(&mut anger, &VarLong(absolute_time)).unwrap();
        let entry = EntityDataEntry {
            index,
            serializer: meta_data_type_id_for_name("long", V::V_26_3).unwrap(),
            value: MetaValue::Raw(anger),
        };
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(entity_id)).unwrap();
        EntityDataListT::for_version(V::V_26_3)
            .write(&mut payload, &[entry])
            .unwrap();
        payload
    }

    #[test]
    fn wolf_and_bee_absolute_anger_times_become_relative_target_ticks() {
        for (offset, entity_type, source_index, target_index) in [
            (
                1u64,
                EntityType::WOLF.id,
                WOLF_ANGER_END_TIME_INDEX_26_3,
                21u8,
            ),
            (
                2u64,
                EntityType::BEE.id,
                BEE_ANGER_END_TIME_INDEX_26_3,
                18u8,
            ),
        ] {
            let key = 0x2111_1000 + offset;
            let entity_id = i32::try_from(offset).unwrap();
            with_connection(key, V::V_1_21_9, |connection| {
                connection.entity_tracker.add(entity_id, entity_type);
                connection.put(GameTimeStorage { game_time: 1_000 });
            });

            let payload = anger_payload(entity_id, source_index, 1_050);
            let translated = translate_clientbound(
                key,
                V::V_1_21_9,
                PLAY,
                clientbound::play::SET_ENTITY_DATA.v26_3,
                &payload,
            )
            .unwrap();

            let mut read = translated.payload.as_slice();
            assert_eq!(VAR_INT.read(&mut read).unwrap().0, entity_id);
            assert_eq!(read[0], target_index);
            read = &read[1..];
            assert_eq!(
                VAR_INT.read(&mut read).unwrap().0,
                meta_data_type_id_for_name("int", V::V_1_21_9).unwrap()
            );
            assert_eq!(VAR_INT.read(&mut read).unwrap().0, 50);
            assert_eq!(read, &[TERMINATOR]);
            remove_connection(key);
        }
    }

    #[test]
    fn expired_anger_time_is_clamped_to_zero() {
        let key = 0x2111_1003;
        let entity_id = 3;
        with_connection(key, V::V_1_21_9, |connection| {
            connection
                .entity_tracker
                .add(entity_id, EntityType::WOLF.id);
            connection.put(GameTimeStorage { game_time: 1_000 });
        });

        let payload = anger_payload(entity_id, WOLF_ANGER_END_TIME_INDEX_26_3, 900);
        let translated = translate_clientbound(
            key,
            V::V_1_21_9,
            PLAY,
            clientbound::play::SET_ENTITY_DATA.v26_3,
            &payload,
        )
        .unwrap();
        let mut read = translated.payload.as_slice();
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, entity_id);
        assert_eq!(read[0], 21);
        read = &read[1..];
        assert_eq!(
            VAR_INT.read(&mut read).unwrap().0,
            meta_data_type_id_for_name("int", V::V_1_21_9).unwrap()
        );
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, 0);
        assert_eq!(read, &[TERMINATOR]);
        remove_connection(key);
    }
}
