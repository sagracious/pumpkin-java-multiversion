use std::collections::HashMap;
use std::sync::OnceLock;

use pumpkin_data::entity::EntityType;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::NetworkReadExt;
use pumpkin_protocol::{ClientPacket, java::client::play::CSpawnEntity};
use pumpkin_util::version::JavaMinecraftVersion;

use crate::api::protocol::packet_key;
use crate::api::rewriter::entity as entity_rewriter;
use crate::api::rewriter::{block, command, particle, screen, sound, stats};
use crate::api::{ComposedMappings, IdPass, PacketWrapper, TranslateError, UserConnection};
use crate::data::entity_types::stand_in_type_for_version;
use crate::packet::mappings::{PacketId, clientbound, serverbound};
use crate::packet::{block_update, chunk_remap, entity, join, status, update_tags};
use crate::pipeline::item_pass;
use crate::registry;
use crate::remap::block_state_remap::remap_block_state_for_version;
use crate::remap::entity_id_remap::remap_object_type_for_version;

#[must_use]
pub fn id_pass_for(packet: &'static PacketId) -> Option<IdPass> {
    table().get(&packet_key(packet)).copied()
}

fn table() -> &'static HashMap<usize, IdPass> {
    static TABLE: OnceLock<HashMap<usize, IdPass>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table: HashMap<usize, IdPass> = HashMap::new();
        let mut put = |packet: &'static PacketId, pass: IdPass| {
            table.insert(packet_key(packet), pass);
        };

        put(&clientbound::play::LEVEL_CHUNK_WITH_LIGHT, chunk);
        put(&clientbound::play::BLOCK_UPDATE, block_update_pass);
        put(
            &clientbound::play::SECTION_BLOCKS_UPDATE,
            section_blocks_update,
        );
        put(&clientbound::play::LEVEL_EVENT, level_event);
        put(&clientbound::play::ADD_ENTITY, add_entity);
        put(&clientbound::play::REMOVE_ENTITIES, remove_entities);
        put(&clientbound::play::UPDATE_TAGS, tags);
        put(&clientbound::config::UPDATE_TAGS, tags);
        put(&clientbound::config::REGISTRY_DATA, registry_data);
        put(&clientbound::status::STATUS_RESPONSE, status_response);
        put(
            &clientbound::play::SET_ENTITY_DATA,
            entity_rewriter::set_entity_data,
        );
        put(
            &clientbound::play::UPDATE_ATTRIBUTES,
            entity_rewriter::update_attributes,
        );
        put(&clientbound::play::LOGIN, login);
        put(&clientbound::play::RESPAWN, respawn);
        put(
            &clientbound::play::CONTAINER_SET_CONTENT,
            item_pass::container_content,
        );
        put(
            &clientbound::play::CONTAINER_SET_SLOT,
            item_pass::container_slot,
        );
        put(&clientbound::play::SET_CURSOR_ITEM, item_pass::cursor_item);
        put(
            &clientbound::play::SET_PLAYER_INVENTORY,
            item_pass::player_inventory,
        );
        put(&clientbound::play::SET_EQUIPMENT, item_pass::equipment);
        put(
            &clientbound::play::MERCHANT_OFFERS,
            item_pass::merchant_offers,
        );
        put(
            &clientbound::play::UPDATE_ADVANCEMENTS,
            item_pass::advancements,
        );
        put(
            &serverbound::play::SET_CREATIVE_MODE_SLOT,
            item_pass::creative_slot,
        );
        put(
            &serverbound::play::CONTAINER_CLICK,
            item_pass::click_container,
        );
        put(&clientbound::play::SOUND, sound::sound);
        put(&clientbound::play::SOUND_ENTITY, sound::sound);
        put(&clientbound::play::EXPLODE, particle::explode);
        put(
            &clientbound::play::LEVEL_PARTICLES,
            particle::level_particles,
        );
        put(
            &clientbound::play::BLOCK_ENTITY_DATA,
            block::block_entity_data,
        );
        put(&clientbound::play::BLOCK_EVENT, block::block_event);
        put(&clientbound::play::AWARD_STATS, stats::award_stats);
        put(&clientbound::play::COMMANDS, command::commands);
        put(&clientbound::play::OPEN_SCREEN, screen::open_screen);
        put(&clientbound::play::MAP_ITEM_DATA, screen::map_item_data);

        table
    })
}

fn rewrite(
    wrapper: &mut PacketWrapper,
    what: &'static str,
    f: impl FnOnce(&[u8]) -> Option<Vec<u8>>,
) -> Result<(), TranslateError> {
    let out = f(wrapper.remaining()).ok_or(TranslateError::Unsupported(what))?;
    wrapper.replace_remaining(out);
    Ok(())
}

fn rewrite_or_pass(
    wrapper: &mut PacketWrapper,
    f: impl FnOnce(&[u8]) -> Option<Vec<u8>>,
) -> Result<(), TranslateError> {
    let out = f(wrapper.remaining());
    match out {
        Some(payload) => wrapper.replace_remaining(payload),
        None => wrapper.passthrough_all(),
    }
    Ok(())
}

fn chunk(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    _ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    rewrite(wrapper, "chunk", |payload| {
        chunk_remap::remap_chunk_payload(payload, layout)
    })
}

fn block_update_pass(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    _ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    if layout < block_update::OLDEST_LAYOUT {
        wrapper.passthrough_all();
        return Ok(());
    }
    rewrite(wrapper, "block update", |payload| {
        block_update::remap_block_update(payload, layout)
    })
}

fn section_blocks_update(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    _ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    if layout < block_update::OLDEST_LAYOUT {
        wrapper.passthrough_all();
        return Ok(());
    }
    rewrite(wrapper, "section blocks update", |payload| {
        block_update::remap_section_blocks_update(payload, layout)
    })
}

fn level_event(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    _ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    if layout < block_update::OLDEST_LAYOUT {
        wrapper.passthrough_all();
        return Ok(());
    }
    rewrite(wrapper, "level event", |payload| {
        block_update::remap_level_event(payload, layout)
    })
}

/// Entity id and 26.3 entity type, the prefix `ADD_ENTITY` has carried since 1.14.
fn spawn_prefix(payload: &[u8]) -> Option<(i32, u16)> {
    let mut cursor = payload;
    let entity_id = cursor.get_var_int().ok()?.0;
    cursor = cursor.get(16..)?;
    let entity_type = u16::try_from(cursor.get_var_int().ok()?.0).ok()?;
    Some((entity_id, entity_type))
}

fn rewrite_spawn(
    payload: &[u8],
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Option<Vec<u8>> {
    let spawn = CSpawnEntity::read_packet_data(payload, &layout).ok()?;
    let entity_type = u16::try_from(spawn.r#type.0).ok()?;
    let client_entity_type = stand_in_type_for_version(entity_type, layout);
    let remapped_type = if layout < JavaMinecraftVersion::V_1_14 {
        VarInt(i32::from(remap_object_type_for_version(
            client_entity_type,
            layout,
        )))
    } else {
        VarInt(i32::try_from(ids.entities.map(u32::from(client_entity_type))?).ok()?)
    };

    // A state the client lacks falls back to air, as every other state id does.
    let data = if entity_type == EntityType::FALLING_BLOCK.id {
        u16::try_from(spawn.data.0).map_or(spawn.data, |state| {
            VarInt(i32::from(remap_block_state_for_version(state, layout)))
        })
    } else {
        spawn.data
    };

    let mut buf = Vec::with_capacity(payload.len());
    CSpawnEntity {
        r#type: remapped_type,
        data,
        ..spawn
    }
    .write_packet_data(&mut buf, &layout)
    .ok()?;
    Some(buf)
}

fn add_entity(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    let out = {
        let payload = wrapper.remaining();
        if let Some((entity_id, entity_type)) = spawn_prefix(payload) {
            let client_type = stand_in_type_for_version(entity_type, layout);
            connection
                .entity_tracker
                .add_mapped(entity_id, entity_type, client_type);
        }
        rewrite_spawn(payload, layout, ids).or_else(|| entity::remap_spawn_entity(payload, layout))
    };
    wrapper.replace_remaining(out.ok_or(TranslateError::Unsupported("add entity"))?);
    Ok(())
}

fn removed_ids(payload: &[u8], layout: JavaMinecraftVersion) -> Vec<i32> {
    let mut cursor = payload;
    if layout == JavaMinecraftVersion::V_1_17 {
        return cursor
            .get_var_int()
            .map(|id| vec![id.0])
            .unwrap_or_default();
    }
    let Ok(count) = cursor.get_var_int() else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for _ in 0..count.0 {
        match cursor.get_var_int() {
            Ok(id) => ids.push(id.0),
            Err(_) => break,
        }
    }
    ids
}

fn remove_entities(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    _ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    for id in removed_ids(wrapper.remaining(), layout) {
        connection.entity_tracker.remove(id);
    }
    wrapper.passthrough_all();
    Ok(())
}

fn tags(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    _ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    rewrite_or_pass(wrapper, |payload| {
        update_tags::rewrite_update_tags(payload, layout)
    })
}

fn registry_data(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    _ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    let out = registry::build_registry_payload(layout, wrapper.remaining());
    match out {
        Some(Some(payload)) => wrapper.replace_remaining(payload),
        // The version has no such registry; sending it would fail its whole load.
        Some(None) => wrapper.cancel(),
        None => wrapper.passthrough_all(),
    }
    Ok(())
}

fn status_response(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    _ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    rewrite_or_pass(wrapper, |payload| {
        status::rewrite_status_response(payload, layout)
    })
}

fn login(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    _ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    connection.entity_tracker.clear();
    if let Ok(entity_id) = { wrapper.remaining() }.get_i32_be() {
        connection.entity_tracker.client_entity_id = Some(entity_id);
        connection
            .entity_tracker
            .add(entity_id, EntityType::PLAYER.id);
    }
    if let Some((min_y, height)) = join::login_world_bounds(wrapper.remaining(), layout) {
        connection.entity_tracker.min_y = min_y;
        connection.entity_tracker.height = height;
    }

    if layout >= join::FIRST_WITH_CONFIG_STATE {
        wrapper.passthrough_all();
        return Ok(());
    }
    rewrite(wrapper, "login", |payload| {
        join::rewrite_login(payload, layout)
    })
}

fn respawn(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    layout: JavaMinecraftVersion,
    _ids: &ComposedMappings,
) -> Result<(), TranslateError> {
    connection.entity_tracker.clear();
    // The client's own entity is never spawned again, only the world is.
    if let Some(entity_id) = connection.entity_tracker.client_entity_id {
        connection
            .entity_tracker
            .add(entity_id, EntityType::PLAYER.id);
    }
    if let Some((min_y, height)) = join::respawn_world_bounds(wrapper.remaining(), layout) {
        connection.entity_tracker.min_y = min_y;
        connection.entity_tracker.height = height;
    }
    if layout >= join::FIRST_WITH_DIMENSION_NAME {
        wrapper.passthrough_all();
        return Ok(());
    }
    rewrite(wrapper, "respawn", |payload| {
        join::rewrite_respawn(payload, layout)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_protocol::ser::NetworkWriteExt;
    use pumpkin_util::math::vector3::Vector3;

    fn spawn_payload(entity_type: u16, data: i32, version: JavaMinecraftVersion) -> Vec<u8> {
        let spawn = CSpawnEntity::new(
            VarInt(11),
            uuid::Uuid::from_u128(1),
            VarInt(i32::from(entity_type)),
            Vector3::new(1.0, 2.0, 3.0),
            0.0,
            0.0,
            0.0,
            VarInt(data),
            Vector3::new(0.0, 0.0, 0.0),
        );
        let mut buf = Vec::new();
        spawn.write_packet_data(&mut buf, &version).unwrap();
        buf
    }

    #[test]
    fn a_spawn_records_the_26_3_type_and_renumbers_it() {
        let version = JavaMinecraftVersion::V_1_20_2;
        let payload = spawn_payload(EntityType::PIG.id, 0, version);
        let ids = crate::api::MappingData::get().composed(version);
        let mut wrapper = PacketWrapper::new(&clientbound::play::ADD_ENTITY, &payload);
        let mut connection = UserConnection::new(0, version);

        add_entity(&mut wrapper, &mut connection, version, ids).unwrap();
        assert_eq!(
            connection.entity_tracker.entity_type(11),
            Some(EntityType::PIG.id)
        );

        let out = wrapper.finish().unwrap().unwrap();
        let decoded = CSpawnEntity::read_packet_data(out.payload.as_slice(), &version).unwrap();
        assert_eq!(
            u16::try_from(decoded.r#type.0).unwrap(),
            ids.entities.map(u32::from(EntityType::PIG.id)).unwrap() as u16
        );
    }

    #[test]
    fn newer_entity_spawns_use_their_via_backwards_stand_ins() {
        let version = JavaMinecraftVersion::V_1_21_9;
        let ids = crate::api::MappingData::get().composed(version);
        for (source, stand_in) in [
            (EntityType::NAUTILUS.id, EntityType::SQUID.id),
            (EntityType::ZOMBIE_NAUTILUS.id, EntityType::GLOW_SQUID.id),
            (EntityType::CAMEL_HUSK.id, EntityType::CAMEL.id),
            (EntityType::PARCHED.id, EntityType::SKELETON.id),
        ] {
            let payload = spawn_payload(source, 0, version);
            let mut wrapper = PacketWrapper::new(&clientbound::play::ADD_ENTITY, &payload);
            let mut connection = UserConnection::new(11, version);

            add_entity(&mut wrapper, &mut connection, version, ids).unwrap();

            assert_eq!(connection.entity_tracker.entity_type(11), Some(source));
            assert_eq!(
                connection.entity_tracker.client_entity_type(11),
                Some(stand_in)
            );
            let translated = wrapper.finish().unwrap().unwrap();
            let decoded =
                CSpawnEntity::read_packet_data(translated.payload.as_slice(), &version).unwrap();
            assert_eq!(
                u16::try_from(decoded.r#type.0).unwrap(),
                u16::try_from(ids.entities.map(u32::from(stand_in)).unwrap()).unwrap()
            );
        }
    }

    #[test]
    fn a_falling_block_gets_its_state_renumbered() {
        let version = JavaMinecraftVersion::V_1_20_2;
        let stone = pumpkin_data::Block::STONE.default_state.id.as_u16();
        let payload = spawn_payload(EntityType::FALLING_BLOCK.id, i32::from(stone), version);
        let ids = crate::api::MappingData::get().composed(version);
        let mut wrapper = PacketWrapper::new(&clientbound::play::ADD_ENTITY, &payload);
        let mut connection = UserConnection::new(0, version);

        add_entity(&mut wrapper, &mut connection, version, ids).unwrap();
        let out = wrapper.finish().unwrap().unwrap();
        let decoded = CSpawnEntity::read_packet_data(out.payload.as_slice(), &version).unwrap();
        assert_eq!(
            decoded.data.0,
            i32::from(
                crate::remap::block_state_remap::remap_block_state_for_version(stone, version)
            )
        );
    }

    #[test]
    fn removed_entities_leave_the_tracker_and_the_payload_alone() {
        let version = JavaMinecraftVersion::V_1_20_2;
        let mut payload = Vec::new();
        payload.write_var_int(&VarInt(2)).unwrap();
        payload.write_var_int(&VarInt(11)).unwrap();
        payload.write_var_int(&VarInt(12)).unwrap();

        let mut connection = UserConnection::new(0, version);
        connection.entity_tracker.add(11, 1);
        connection.entity_tracker.add(12, 2);
        connection.entity_tracker.add(13, 3);

        let ids = crate::api::MappingData::get().composed(version);
        let mut wrapper = PacketWrapper::new(&clientbound::play::REMOVE_ENTITIES, &payload);
        remove_entities(&mut wrapper, &mut connection, version, ids).unwrap();

        assert!(connection.entity_tracker.entity_type(11).is_none());
        assert!(connection.entity_tracker.entity_type(12).is_none());
        assert_eq!(connection.entity_tracker.entity_type(13), Some(3));
        assert_eq!(wrapper.finish().unwrap().unwrap().payload, payload);
    }

    #[test]
    fn a_respawn_forgets_every_entity_but_the_player() {
        let version = JavaMinecraftVersion::V_1_20_2;
        let mut connection = UserConnection::new(0, version);
        connection.entity_tracker.client_entity_id = Some(1);
        connection.entity_tracker.add(1, EntityType::PLAYER.id);
        connection.entity_tracker.add(11, EntityType::PIG.id);

        let ids = crate::api::MappingData::get().composed(version);
        let mut wrapper = PacketWrapper::new(&clientbound::play::RESPAWN, &[1, 2, 3]);
        respawn(&mut wrapper, &mut connection, version, ids).unwrap();

        assert_eq!(
            connection.entity_tracker.entity_type(1),
            Some(EntityType::PLAYER.id)
        );
        assert!(connection.entity_tracker.entity_type(11).is_none());
    }

    #[test]
    fn the_1_17_form_carries_a_single_id() {
        let mut payload = Vec::new();
        payload.write_var_int(&VarInt(11)).unwrap();
        assert_eq!(
            removed_ids(&payload, JavaMinecraftVersion::V_1_17),
            vec![11]
        );
        assert_eq!(
            removed_ids(&payload, JavaMinecraftVersion::V_1_17_1),
            Vec::<i32>::new()
        );
    }
}
