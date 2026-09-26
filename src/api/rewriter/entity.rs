use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_protocol::{
    ClientPacket,
    java::client::play::{CSpawnEntity, attribute_name_to_id},
};
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::connection::GameTimeStorage;
use crate::api::entity_data::{EntityDataEntry, EntityDataListT, MetaValue};
use crate::api::rewriter::particle::{
    ParticleData, write_particle, write_particle_with_connection,
};
use crate::api::types::{VAR_INT, VAR_LONG, WireType};
use crate::api::{MappingData, PacketWrapper, TranslateError, UserConnection};
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
    server_entity_type: u16,
    client_entity_type: u16,
    entries: &[EntityDataEntry],
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
    mut connection: Option<&mut UserConnection>,
    game_time: i64,
) -> Vec<EntityDataEntry> {
    let mut rewritten = Vec::with_capacity(entries.len() + 1);
    for entry in entries {
        // Up to 1.20.3 the area effect cloud stores color at index 9 and its
        // waiting flag at 10. Newer servers fold that color into the
        // entity-effect particle at index 10, so recover the old color before
        // dropping the particle for clients that cannot represent the shape.
        if server_entity_type == pumpkin_data::entity::EntityType::AREA_EFFECT_CLOUD.id
            && layout <= JavaMinecraftVersion::V_1_20_3
            && entry.index == 10
            && let MetaValue::Particle(particle) = &entry.value
            && particle.id == i32::from(pumpkin_data::particle::Particle::EntityEffect.to_id())
            && let ParticleData::Color(color) = &particle.data
        {
            let mut value = Vec::new();
            if VAR_INT
                .write(&mut value, &VarInt(*color & 0x00ff_ffff))
                .is_err()
            {
                continue;
            }
            if let Some(serializer) = meta_data_type_id_for_name("int", layout) {
                rewritten.push(EntityDataEntry {
                    index: 9,
                    serializer,
                    value: MetaValue::Raw(value),
                });
            }
            continue;
        }

        // The 26.3 cushion is represented by a falling block for 26.2.
        // Only base-entity metadata indices 0 through 7 are meaningful on
        // that stand-in; ViaBackwards cancels every later field.
        if server_entity_type == pumpkin_data::entity::EntityType::CUSHION.id
            && client_entity_type == pumpkin_data::entity::EntityType::FALLING_BLOCK.id
            && entry.index > 7
        {
            continue;
        }
        if stand_in_metadata_removed(server_entity_type, entry.index, layout) {
            continue;
        }
        let Some(index) = tracked_index_for_version(client_entity_type, entry.index, layout) else {
            continue;
        };
        let Some((serializer, value)) = rewrite_entry_value(
            server_entity_type,
            entry,
            layout,
            ids,
            connection.as_deref_mut(),
            game_time,
        ) else {
            continue;
        };
        rewritten.push(EntityDataEntry {
            index,
            serializer,
            value,
        });
    }
    rewritten.sort_by_key(|entry| entry.index);
    rewritten
}

fn stand_in_metadata_removed(
    server_entity_type: u16,
    index: u8,
    layout: JavaMinecraftVersion,
) -> bool {
    if layout > JavaMinecraftVersion::V_1_21_9 {
        return false;
    }
    match server_entity_type {
        entity if entity == pumpkin_data::entity::EntityType::NAUTILUS.id => {
            (17..=20).contains(&index)
        }
        entity if entity == pumpkin_data::entity::EntityType::ZOMBIE_NAUTILUS.id => {
            (17..=21).contains(&index)
        }
        _ => false,
    }
}

fn rewrite_entry_value(
    entity_type: u16,
    entry: &EntityDataEntry,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
    connection: Option<&mut UserConnection>,
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
        rewrite_value(&entry.value, layout, ids, connection)?,
    ))
}

fn rewrite_value(
    value: &MetaValue,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
    mut connection: Option<&mut UserConnection>,
) -> Option<MetaValue> {
    Some(match value {
        // Entity metadata is decoded in Pumpkin's native 26.3 form before
        // this target-version pass.
        MetaValue::Raw(_) => value.clone(),
        MetaValue::Item(bytes) => {
            let mut input = bytes.as_slice();
            let item = match connection.as_deref_mut() {
                Some(connection) => crate::api::rewriter::item::rewrite_item_value_with_connection(
                    &mut input, layout, ids, connection,
                )?,
                None => crate::api::rewriter::item::rewrite_item_value(&mut input, layout, ids)?,
            };
            if !input.is_empty() {
                return None;
            }
            MetaValue::Item(item)
        }
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
            let written = match connection.as_deref_mut() {
                Some(connection) => {
                    write_particle_with_connection(&mut out, particle, layout, ids, connection)
                }
                None => write_particle(&mut out, particle, layout, ids),
            }
            .ok()?;
            if !written {
                return None;
            }
            MetaValue::Raw(out)
        }
        MetaValue::Particles(particles) => {
            let mut body = Vec::new();
            let mut kept = 0;
            for particle in particles {
                let mut one = Vec::new();
                let written = match connection.as_deref_mut() {
                    Some(connection) => {
                        write_particle_with_connection(&mut one, particle, layout, ids, connection)
                    }
                    None => write_particle(&mut one, particle, layout, ids),
                }
                .ok()?;
                if written {
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

/// Core writes native 26.3 entity metadata. Decode it once, then rewrite the
/// indices, serializer ids, and values for the client's version.
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
    // Pumpkin emits entity metadata in its native 26.3 serializer layout.
    // Decode that once, then write the client's serializer/index layout after
    // the entity-specific conversion below.
    let entries = wrapper.read(&EntityDataListT::for_version(JavaMinecraftVersion::V_26_3))?;
    // Without the entity type there is no index rule to apply, and an entry
    // under the wrong one is fatal to the client.
    let game_time = connection
        .get::<GameTimeStorage>()
        .map_or(0, |storage| storage.game_time);
    let source_type = connection.entity_tracker.entity_type(entity_id.0);
    let client_type = connection
        .entity_tracker
        .client_entity_type(entity_id.0)
        .or(source_type);
    let entries = source_type
        .zip(client_type)
        .map(|(server_type, client_type)| {
            rewrite_entries(
                server_type,
                client_type,
                &entries,
                layout,
                ids,
                Some(connection),
                game_time,
            )
        })
        .unwrap_or_default();
    wrapper.write(&EntityDataListT::for_version(layout), &entries)
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

    /// 1.20.5 stores the cloud color in the entity-effect particle; 1.20.3
    /// needs that color restored to its own metadata field.
    #[test]
    fn a_cloud_effect_color_is_restored_below_1_20_5() {
        let cloud = EntityType::AREA_EFFECT_CLOUD.id;
        let entries = vec![
            EntityDataEntry {
                index: 8,
                serializer: meta_data_type_id_for_name("float", V::V_26_3).unwrap(),
                value: MetaValue::Raw(3.0f32.to_be_bytes().to_vec()),
            },
            EntityDataEntry {
                index: 9,
                serializer: meta_data_type_id_for_name("boolean", V::V_26_3).unwrap(),
                value: MetaValue::Raw(vec![0]),
            },
            EntityDataEntry {
                index: 10,
                serializer: meta_data_type_id_for_name("particle", V::V_26_3).unwrap(),
                value: MetaValue::Particle(crate::api::rewriter::particle::Particle {
                    id: i32::from(Particle::EntityEffect.to_id()),
                    data: ParticleData::Color(0x1122_3344),
                }),
            },
        ];
        let before_1_20_5 = rewrite_entries(
            cloud,
            cloud,
            &entries,
            V::V_1_20_3,
            MappingData::get().composed(V::V_1_20_3),
            None,
            0,
        );
        assert_eq!(
            before_1_20_5
                .iter()
                .map(|entry| entry.index)
                .collect::<Vec<_>>(),
            [8, 9, 10]
        );
        assert_eq!(
            before_1_20_5[1].serializer,
            meta_data_type_id_for_name("int", V::V_1_20_3).unwrap()
        );
        let MetaValue::Raw(color) = &before_1_20_5[1].value else {
            panic!("legacy cloud color is a varint");
        };
        let mut color_reader = color.as_slice();
        assert_eq!(VAR_INT.read(&mut color_reader).unwrap().0, 0x0022_3344);
        assert!(color_reader.is_empty());
        assert_eq!(
            before_1_20_5[2].serializer,
            meta_data_type_id_for_name("boolean", V::V_1_20_3).unwrap()
        );

        let mapped = MappingData::get()
            .composed(V::V_1_20_5)
            .particles
            .map(u32::from(Particle::EntityEffect.to_id()))
            .unwrap();
        let at_1_20_5 = rewrite_entries(
            cloud,
            cloud,
            &entries,
            V::V_1_20_5,
            MappingData::get().composed(V::V_1_20_5),
            None,
            0,
        );
        assert_eq!(at_1_20_5[1].index, 9);
        assert_eq!(
            at_1_20_5[1].serializer,
            meta_data_type_id_for_name("boolean", V::V_1_20_5).unwrap()
        );
        assert_eq!(at_1_20_5[2].index, 10);
        assert_eq!(
            at_1_20_5[2].serializer,
            meta_data_type_id_for_name("particle", V::V_1_20_5).unwrap()
        );
        let MetaValue::Raw(particle) = &at_1_20_5[2].value else {
            panic!("particle metadata is emitted as target bytes");
        };
        let mut particle_reader = particle.as_slice();
        assert_eq!(VAR_INT.read(&mut particle_reader).unwrap().0, mapped as i32);
        assert_eq!(particle_reader, [0x11, 0x22, 0x33, 0x44]);
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
        // Core sends native 26.3 metadata: float health is at index 9/type 3.
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
        use crate::api::rewriter::particle::{Particle as CanonicalParticle, ParticleData};

        let effect = CanonicalParticle {
            id: i32::from(Particle::EntityEffect.to_id()),
            data: ParticleData::Color(0x1122_3344),
        };
        let geyser = CanonicalParticle {
            id: i32::from(Particle::GeyserBase.to_id()),
            data: ParticleData::Geyser {
                water_blocks: 5,
                impulse: 1.0,
            },
        };
        let ids = MappingData::get().composed(layout);
        let mapped = ids
            .particles
            .map(u32::from(Particle::EntityEffect.to_id()))
            .unwrap();
        let MetaValue::Raw(rewritten) = rewrite_value(
            &MetaValue::Particles(vec![effect, geyser]),
            layout,
            ids,
            None,
        )
        .unwrap() else {
            panic!("particle list is emitted as wire bytes");
        };
        assert_eq!(
            rewritten,
            [1, u8::try_from(mapped).unwrap(), 0x11, 0x22, 0x33, 0x44]
        );
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
        let entries = vec![entry];
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(entity_id)).unwrap();
        EntityDataListT::for_version(V::V_26_3)
            .write(&mut payload, &entries)
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

#[cfg(test)]
mod cushion_and_nested_item_tests {
    use super::*;
    use crate::api::entity_data::{EntityDataEntry, EntityDataListT, MetaValue};
    use crate::api::rewriter::item::ClientboundItemT;
    use crate::api::rewriter::item_backup::restore_full_item;
    use crate::api::types::{Item as WireItem, ItemComponent, ItemT};
    use crate::data::entity_data_types::meta_data_type_id_for_name;
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::entity::EntityType;
    use pumpkin_protocol::ser::NetworkWriteExt;
    use pumpkin_util::version::JavaMinecraftVersion as V;

    #[test]
    fn cushion_metadata_above_the_base_entity_fields_is_removed() {
        let ids = MappingData::get().composed(V::V_26_2);
        let serializer = meta_data_type_id_for_name("int", V::V_26_3).unwrap();
        let entries: Vec<_> = (0..10)
            .map(|index| EntityDataEntry {
                index,
                serializer,
                value: MetaValue::Raw(vec![0]),
            })
            .collect();

        let rewritten = rewrite_entries(
            EntityType::CUSHION.id,
            EntityType::FALLING_BLOCK.id,
            &entries,
            V::V_26_2,
            ids,
            None,
            0,
        );
        assert_eq!(
            rewritten
                .iter()
                .map(|entry| entry.index)
                .collect::<Vec<_>>(),
            (0..8).collect::<Vec<_>>()
        );
    }

    #[test]
    fn item_inside_entity_metadata_uses_the_structured_rewriter() {
        let target = V::V_1_21_4;
        let item = WireItem::Structured {
            count: 1,
            id: i32::from(pumpkin_data::item::Item::DIAMOND.id),
            added: vec![ItemComponent {
                id: i32::from(DataComponent::AttackAnimation.to_id()),
                data: vec![1, 6],
            }],
            removed: Vec::new(),
        };

        let mut item_payload = Vec::new();
        ItemT::for_version(V::V_26_3)
            .write(&mut item_payload, &item)
            .unwrap();
        let entry = EntityDataEntry {
            index: 9,
            serializer: meta_data_type_id_for_name("item_stack", V::V_26_3).unwrap(),
            value: MetaValue::Item(item_payload),
        };
        let mut metadata_26_3 = Vec::new();
        EntityDataListT::for_version(V::V_26_3)
            .write(&mut metadata_26_3, &vec![entry])
            .unwrap();

        // Read the packet in Pumpkin's native 26.3 layout, then verify the
        // entity rewrite converts its nested stack for the older client.
        let mut input = metadata_26_3.as_slice();
        let parsed = EntityDataListT::for_version(V::V_26_3)
            .read(&mut input)
            .expect("metadata list parses");
        assert!(input.is_empty());
        let ids = MappingData::get().composed(target);
        let mut connection = UserConnection::new(23, target);
        let rewritten = rewrite_entries(
            EntityType::ITEM_FRAME.id,
            EntityType::ITEM_FRAME.id,
            &parsed,
            target,
            ids,
            Some(&mut connection),
            0,
        );
        let MetaValue::Item(bytes) = &rewritten[0].value else {
            panic!("item metadata stays an item");
        };
        assert_eq!(rewritten[0].index, 8);
        let mut item_input = bytes.as_slice();
        let mut returned = ClientboundItemT::new(target, ids)
            .read(&mut item_input)
            .unwrap();
        assert!(item_input.is_empty());
        restore_full_item(&connection, &mut returned, target, ids);
        let WireItem::Structured { added, .. } = returned else {
            panic!("nested item remains structured");
        };
        assert!(added.contains(&ItemComponent {
            id: i32::from(DataComponent::AttackAnimation.to_id()),
            data: vec![1, 6],
        }));
    }
}

#[cfg(test)]
mod stand_in_metadata_tests {
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

    fn raw_entry(index: u8, serializer: &str, value: Vec<u8>) -> EntityDataEntry {
        EntityDataEntry {
            index,
            serializer: meta_data_type_id_for_name(serializer, V::V_26_3).unwrap(),
            value: MetaValue::Raw(value),
        }
    }

    fn translate(
        key: u64,
        server_type: u16,
        client_type: u16,
        entries: Vec<EntityDataEntry>,
    ) -> Vec<EntityDataEntry> {
        const ENTITY_ID: i32 = 77;
        with_connection(key, V::V_1_21_9, |connection| {
            connection
                .entity_tracker
                .add_mapped(ENTITY_ID, server_type, client_type);
        });
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(ENTITY_ID)).unwrap();
        EntityDataListT::for_version(V::V_26_3)
            .write(&mut payload, &entries)
            .unwrap();
        let translated = translate_clientbound(
            key,
            V::V_1_21_9,
            PLAY,
            clientbound::play::SET_ENTITY_DATA.v26_3,
            &payload,
        )
        .unwrap();
        let mut read = translated.payload.as_slice();
        assert_eq!(VAR_INT.read(&mut read).unwrap().0, ENTITY_ID);
        let entries = EntityDataListT::for_version(V::V_1_21_9)
            .read(&mut read)
            .unwrap();
        assert!(read.is_empty(), "metadata reader consumes the terminator");
        remove_connection(key);
        entries
    }

    #[test]
    fn nautilus_metadata_uses_squid_base_fields_and_drops_its_own_fields() {
        let entries = translate(
            0x2111_2001,
            EntityType::NAUTILUS.id,
            EntityType::SQUID.id,
            vec![
                raw_entry(9, "float", 20.0f32.to_be_bytes().to_vec()),
                raw_entry(17, "boolean", vec![1]),
                raw_entry(18, "byte", vec![1]),
                raw_entry(19, "optional_living_entity_reference", vec![0]),
                raw_entry(20, "boolean", vec![1]),
            ],
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].index, 9);
        assert_eq!(
            entries[0].value,
            MetaValue::Raw(20.0f32.to_be_bytes().to_vec())
        );
    }

    #[test]
    fn zombie_nautilus_variant_is_removed_for_the_glow_squid_standin() {
        let entries = translate(
            0x2111_2002,
            EntityType::ZOMBIE_NAUTILUS.id,
            EntityType::GLOW_SQUID.id,
            vec![
                raw_entry(9, "float", 20.0f32.to_be_bytes().to_vec()),
                raw_entry(17, "boolean", vec![1]),
                raw_entry(18, "byte", vec![1]),
                raw_entry(19, "optional_living_entity_reference", vec![0]),
                raw_entry(20, "boolean", vec![1]),
                raw_entry(21, "zombie_nautilus_variant", vec![0]),
            ],
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].index, 9);
    }

    #[test]
    fn camel_husk_metadata_shifts_onto_the_camel_standin() {
        let entries = translate(
            0x2111_2003,
            EntityType::CAMEL_HUSK.id,
            EntityType::CAMEL.id,
            vec![
                raw_entry(17, "boolean", vec![1]),
                raw_entry(18, "byte", vec![2]),
                raw_entry(19, "boolean", vec![1]),
                raw_entry(20, "long", {
                    let mut value = Vec::new();
                    VAR_LONG.write(&mut value, &VarLong(33)).unwrap();
                    value
                }),
            ],
        );
        assert_eq!(
            entries.iter().map(|entry| entry.index).collect::<Vec<_>>(),
            vec![17, 18, 19]
        );
    }
}
