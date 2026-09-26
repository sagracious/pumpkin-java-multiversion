use pumpkin_data::entity::EntityType;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::NetworkWriteExt;
use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::entity_data::EntityDataListT;
use crate::api::types::{I64T, NbtT, OptionalT, STRING, TextComponentT, U8, VAR_INT, VAR_LONG};
use crate::api::{
    Ctx, MappingData, PacketWrapper, Protocol, Registry, Step, TranslateError, UserConnection,
};
use crate::data::tracked_index::tracked_index_for_version;
use crate::packet::mappings::{clientbound, serverbound};

pub struct Protocol26_2To26_1;

impl Protocol for Protocol26_2To26_1 {
    fn step(&self) -> Step {
        Step {
            from: V::V_26_2,
            to: V::V_26_1,
        }
    }

    fn register(&self, reg: &mut Registry) {
        reg.clientbound(&clientbound::play::SET_PLAYER_TEAM, player_team);
        reg.clientbound(&clientbound::play::BLOCK_UPDATE, block_update_bed_entity);
        reg.clientbound(
            &clientbound::play::SECTION_BLOCKS_UPDATE,
            section_blocks_update_bed_entities,
        );
        reg.clientbound(
            &clientbound::play::LEVEL_CHUNK_WITH_LIGHT,
            chunk_bed_entities,
        );
        reg.clientbound(&clientbound::play::SET_ENTITY_DATA, sulfur_cube_metadata);
        reg.serverbound(&serverbound::play::SPECTATE_ENTITY, spectate_entity);
    }
}

/// 26.2 makes the team color optional and moves flags/visibility/color ahead
/// of the prefix and suffix. 26.1 still expects the older mandatory color and
/// field order. Unrelated update modes have no trailing fields to rewrite.
fn player_team(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&STRING)?;
    let mode = wrapper.passthrough(&U8)?;
    if mode != 0 && mode != 2 {
        wrapper.passthrough_all();
        return Ok(());
    }

    let component = wrapper.passthrough(&TextComponentT::for_version(V::V_26_3))?;
    let prefix = wrapper.read(&TextComponentT::for_version(V::V_26_3))?;
    let suffix = wrapper.read(&TextComponentT::for_version(V::V_26_3))?;
    let visibility = wrapper.read(&VAR_INT)?;
    let collision = wrapper.read(&VAR_INT)?;
    let color = wrapper.read(&OptionalT(VAR_INT))?.unwrap_or(VarInt(15));
    let flags = wrapper.read(&U8)?;

    wrapper.write(&TextComponentT::for_version(connection.version), &component)?;
    wrapper.write(&U8, &flags)?;
    wrapper.write(&VAR_INT, &visibility)?;
    wrapper.write(&VAR_INT, &collision)?;
    wrapper.write(&VAR_INT, &color)?;
    wrapper.write(&TextComponentT::for_version(connection.version), &prefix)?;
    wrapper.write(&TextComponentT::for_version(connection.version), &suffix)?;
    wrapper.passthrough_all();
    Ok(())
}

/// Old clients have no `max_fuse` or `from_bucket` fields on sulfur cubes.
/// The general entity-data pass has already converted indices to the target
/// entity layout, so compute those two target indices from the source slots.
fn sulfur_cube_metadata(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let entity_id = wrapper.passthrough(&VAR_INT)?.0;
    let data = wrapper.read(&EntityDataListT::for_version(connection.version))?;
    let source_type = connection.entity_tracker.entity_type(entity_id);
    let client_type = connection
        .entity_tracker
        .client_entity_type(entity_id)
        .or(source_type);

    let Some(server_type) = source_type else {
        wrapper.write(&EntityDataListT::for_version(connection.version), &data)?;
        return Ok(());
    };
    if server_type != EntityType::SULFUR_CUBE.id {
        wrapper.write(&EntityDataListT::for_version(connection.version), &data)?;
        return Ok(());
    }

    let remove: Vec<u8> = [19, 20]
        .into_iter()
        .filter_map(|index| {
            tracked_index_for_version(
                client_type.unwrap_or(server_type),
                index,
                connection.version,
            )
        })
        .collect();
    let filtered: Vec<_> = data
        .into_iter()
        .filter(|entry| !remove.contains(&entry.index))
        .collect();
    wrapper.write(&EntityDataListT::for_version(connection.version), &filtered)?;
    Ok(())
}

/// The 26.2 packet carries a required entity id. 26.1 encoded the same value
/// as an optional id, so add the presence bit for the serverbound core packet.
fn spectate_entity(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let entity_id = wrapper.read(&VAR_INT)?;
    wrapper.write(&OptionalT(VAR_INT), &Some(entity_id))?;
    wrapper.passthrough_all();
    Ok(())
}

fn block_update_bed_entity(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let position = wrapper.passthrough(&I64T)?.0;
    let state = wrapper.passthrough(&VAR_INT)?.0;
    if state_is_26_2_bed(state, connection.version)
        && let Some(payload) = bed_block_entity_payload(position, connection.version)
    {
        wrapper.send_extra(&clientbound::play::BLOCK_ENTITY_DATA, payload);
    }
    Ok(())
}

fn section_blocks_update_bed_entities(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let section_position = wrapper.passthrough(&I64T)?.0;
    if (V::V_1_16..=V::V_1_19_4).contains(&connection.version) {
        wrapper.passthrough(&BOOL)?;
    }
    let count = bounded_count(
        wrapper.passthrough(&VAR_INT)?.0,
        65_536,
        "section block changes",
    )?;
    let chunk_x = (section_position >> 42) as i32;
    let section_y = (section_position << 44 >> 44) as i32;
    let chunk_z = (section_position << 22 >> 42) as i32;
    for _ in 0..count {
        let packed = wrapper.passthrough(&VAR_LONG)?.0 as u64;
        let state = (packed >> 12) as i32;
        if !state_is_26_2_bed(state, connection.version) {
            continue;
        }
        let local = (packed & 0x0fff) as i32;
        let x = (chunk_x << 4) + ((local >> 8) & 0x0f);
        let z = (chunk_z << 4) + ((local >> 4) & 0x0f);
        let y = (section_y << 4) + (local & 0x0f);
        let position = ((i64::from(x) & 0x03ff_ffff) << 38)
            | ((i64::from(z) & 0x03ff_ffff) << 12)
            | (i64::from(y) & 0x0fff);
        if let Some(payload) = bed_block_entity_payload(position, connection.version) {
            wrapper.send_extra(&clientbound::play::BLOCK_ENTITY_DATA, payload);
        }
    }
    wrapper.passthrough_all();
    Ok(())
}

fn chunk_bed_entities(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let positions = crate::packet::chunk_remap::matching_block_positions(
        wrapper.remaining(),
        ctx.layout,
        connection.entity_tracker.min_y,
        |state| state_is_26_2_bed(state, ctx.layout),
    )
    .ok_or(TranslateError::Unsupported("chunk bed block entities"))?;
    if positions.is_empty() {
        wrapper.passthrough_all();
        return Ok(());
    }

    let entity_type = bed_block_entity_type_id(ctx.layout)
        .ok_or(TranslateError::Unsupported("bed block entity type"))?;
    let additions: Vec<_> = positions
        .into_iter()
        .map(|position| (position, entity_type))
        .collect();
    let rewritten = crate::packet::chunk_remap::append_chunk_block_entities(
        wrapper.remaining(),
        ctx.layout,
        &additions,
    )
    .ok_or(TranslateError::Unsupported("chunk block entities"))?;
    wrapper.replace_remaining(rewritten);
    Ok(())
}

fn state_is_26_2_bed(client_state: i32, version: V) -> bool {
    let Ok(client_state) = u32::try_from(client_state) else {
        return false;
    };
    let Some(source_26_3) = MappingData::get()
        .composed(version)
        .blockstates
        .inverse()
        .map(client_state)
    else {
        return false;
    };
    let Some(state_26_2) = MappingData::get()
        .composed(V::V_26_2)
        .blockstates
        .map(source_26_3)
    else {
        return false;
    };
    (1931..=2186).contains(&state_26_2)
}

fn bed_block_entity_type_id(version: V) -> Option<i32> {
    let bed_id_26_3 = MappingData::get()
        .composed(V::V_26_1)
        .blockentities
        .inverse()
        .map(25)?;
    MappingData::get()
        .composed(version)
        .blockentities
        .map(bed_id_26_3)
        .and_then(|id| i32::try_from(id).ok())
}

fn bed_block_entity_payload(position: i64, version: V) -> Option<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_i64_be(position).ok()?;
    if version >= V::V_1_18 {
        VAR_INT
            .write(&mut payload, &VarInt(bed_block_entity_type_id(version)?))
            .ok()?;
    } else {
        // Before 1.18 the packet uses the block-entity action byte, where 11 is bed.
        U8.write(&mut payload, &11).ok()?;
    }
    NbtT::for_version(version)
        .write(
            &mut payload,
            &Some(pumpkin_nbt::tag::NbtTag::Compound(
                pumpkin_nbt::compound::NbtCompound::new(),
            )),
        )
        .ok()?;
    Some(payload)
}

fn bounded_count(count: i32, max: usize, name: &'static str) -> Result<usize, TranslateError> {
    let count = usize::try_from(count).map_err(|_| TranslateError::Unsupported(name))?;
    if count > max {
        return Err(TranslateError::Unsupported(name));
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{TextComponentT, WireType};
    use pumpkin_nbt::tag::NbtTag;
    use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};

    fn run(
        packet: &'static crate::packet::mappings::PacketId,
        payload: &[u8],
        handler: crate::api::Handler,
        version: V,
    ) -> Option<Vec<u8>> {
        let ids = MappingData::get().step(V::V_26_2);
        let mut wrapper = PacketWrapper::new(packet, payload);
        let mut connection = UserConnection::new(0, version);
        let context = Ctx {
            step: Protocol26_2To26_1.step(),
            mappings: ids,
            layout: V::V_26_3,
        };
        handler(&mut wrapper, &mut connection, &context).unwrap();
        wrapper.finish().unwrap().map(|out| out.payload)
    }

    fn run_full(
        packet: &'static crate::packet::mappings::PacketId,
        payload: &[u8],
        handler: crate::api::Handler,
        version: V,
    ) -> Option<crate::api::Translated> {
        let ids = MappingData::get().step(V::V_26_2);
        let mut wrapper = PacketWrapper::new(packet, payload);
        let mut connection = UserConnection::new(0, version);
        let context = Ctx {
            step: Protocol26_2To26_1.step(),
            mappings: ids,
            layout: V::V_26_3,
        };
        handler(&mut wrapper, &mut connection, &context).unwrap();
        wrapper.finish().unwrap()
    }

    fn team_payload(color: Option<VarInt>) -> Vec<u8> {
        let mut out = Vec::new();
        STRING.write(&mut out, &"red".into()).unwrap();
        U8.write(&mut out, &0).unwrap();
        TextComponentT::for_version(V::V_26_3)
            .write(&mut out, &pumpkin_util::text::TextComponent::text("Team"))
            .unwrap();
        let empty = pumpkin_util::text::TextComponent::text("");
        TextComponentT::for_version(V::V_26_3)
            .write(&mut out, &empty)
            .unwrap();
        TextComponentT::for_version(V::V_26_3)
            .write(&mut out, &empty)
            .unwrap();
        VAR_INT.write(&mut out, &VarInt(1)).unwrap();
        VAR_INT.write(&mut out, &VarInt(2)).unwrap();
        OptionalT(VAR_INT).write(&mut out, &color).unwrap();
        U8.write(&mut out, &0x03).unwrap();
        out
    }

    #[test]
    fn team_color_and_prefix_suffix_are_written_in_26_1_order() {
        for version in [V::V_26_1, V::V_1_16_2] {
            let out = run(
                &clientbound::play::SET_PLAYER_TEAM,
                &team_payload(Some(VarInt(4))),
                player_team,
                version,
            )
            .unwrap();
            let mut read = out.as_slice();
            assert_eq!(&*STRING.read(&mut read).unwrap(), "red");
            assert_eq!(U8.read(&mut read).unwrap(), 0);
            TextComponentT::for_version(version)
                .read(&mut read)
                .unwrap();
            assert_eq!(U8.read(&mut read).unwrap(), 0x03);
            assert_eq!(VAR_INT.read(&mut read).unwrap(), VarInt(1));
            assert_eq!(VAR_INT.read(&mut read).unwrap(), VarInt(2));
            assert_eq!(VAR_INT.read(&mut read).unwrap(), VarInt(4));
            TextComponentT::for_version(version)
                .read(&mut read)
                .unwrap();
            TextComponentT::for_version(version)
                .read(&mut read)
                .unwrap();
            assert!(read.is_empty());
        }
    }

    #[test]
    fn missing_team_color_defaults_to_white() {
        let out = run(
            &clientbound::play::SET_PLAYER_TEAM,
            &team_payload(None),
            player_team,
            V::V_26_1,
        )
        .unwrap();
        let mut read = out.as_slice();
        STRING.read(&mut read).unwrap();
        U8.read(&mut read).unwrap();
        TextComponentT::for_version(V::V_26_1)
            .read(&mut read)
            .unwrap();
        assert_eq!(U8.read(&mut read).unwrap(), 0x03);
        VAR_INT.read(&mut read).unwrap();
        VAR_INT.read(&mut read).unwrap();
        assert_eq!(VAR_INT.read(&mut read).unwrap(), VarInt(15));
    }

    #[test]
    fn spectate_entity_adds_the_legacy_optional_flag() {
        let out = run(
            &serverbound::play::SPECTATE_ENTITY,
            &[0x2a],
            spectate_entity,
            V::V_26_1,
        )
        .unwrap();
        assert_eq!(out, [1, 0x2a]);
    }

    fn bed_state_for(version: V) -> i32 {
        let source_26_3 = MappingData::get()
            .composed(V::V_26_2)
            .blockstates
            .inverse()
            .map(1931)
            .expect("26.2 bed state has a 26.3 mapping");
        i32::try_from(
            MappingData::get()
                .composed(version)
                .blockstates
                .map(source_26_3)
                .expect("bed exists for target"),
        )
        .unwrap()
    }

    #[test]
    fn bed_block_update_emits_a_target_block_entity_packet() {
        let version = V::V_26_1;
        let position = (17_i64 << 38) | (34_i64 << 12) | 51_i64;
        let mut payload = Vec::new();
        payload.write_i64_be(position).unwrap();
        VAR_INT
            .write(&mut payload, &VarInt(bed_state_for(version)))
            .unwrap();

        let translated = run_full(
            &clientbound::play::BLOCK_UPDATE,
            &payload,
            block_update_bed_entity,
            version,
        )
        .unwrap();
        assert_eq!(translated.payload, payload);
        assert_eq!(translated.extra.len(), 1);
        assert_eq!(
            translated.extra[0].0.v26_3,
            clientbound::play::BLOCK_ENTITY_DATA.v26_3
        );
        let mut extra = translated.extra[0].1.as_slice();
        assert_eq!(I64T.read(&mut extra).unwrap(), position);
        VAR_INT.read(&mut extra).unwrap();
        assert!(matches!(
            NbtT::for_version(version).read(&mut extra).unwrap(),
            Some(NbtTag::Compound(_))
        ));
        assert!(extra.is_empty());
    }

    #[test]
    fn bed_section_change_emits_the_world_block_entity_position() {
        for version in [V::V_26_1, V::V_1_19_4, V::V_1_16_2] {
            let chunk_x = 1_i64;
            let chunk_z = 2_i64;
            let section_y = 3_i64;
            let section_position = (chunk_x << 42) | (chunk_z << 20) | section_y;
            let local_pos = (2_u64 << 8) | (4_u64 << 4) | 3;
            let packed = ((bed_state_for(version) as u64) << 12) | local_pos;
            let mut payload = Vec::new();
            payload.write_i64_be(section_position).unwrap();
            if (V::V_1_16..=V::V_1_19_4).contains(&version) {
                payload.write_bool(true).unwrap();
            }
            payload.write_var_int(&VarInt(1)).unwrap();
            payload
                .write_var_long(&pumpkin_protocol::codec::var_long::VarLong(packed as i64))
                .unwrap();

            let translated = run_full(
                &clientbound::play::SECTION_BLOCKS_UPDATE,
                &payload,
                section_blocks_update_bed_entities,
                version,
            )
            .unwrap();
            assert_eq!(translated.payload, payload, "{version}");
            assert_eq!(translated.extra.len(), 1, "{version}");
            let mut extra = translated.extra[0].1.as_slice();
            let expected = (18_i64 << 38) | (36_i64 << 12) | 51_i64;
            assert_eq!(I64T.read(&mut extra).unwrap(), expected, "{version}");
        }
    }

    #[test]
    fn chunk_load_emits_a_bed_block_entity_for_legacy_clients() {
        let version = V::V_26_1;
        let mut sections = Vec::new();
        sections.write_i16_be(1).unwrap(); // one non-air block
        sections.write_i16_be(0).unwrap(); // fluid count
        sections.write_u8(4).unwrap();
        VAR_INT.write(&mut sections, &VarInt(2)).unwrap();
        VAR_INT.write(&mut sections, &VarInt(0)).unwrap(); // air
        VAR_INT
            .write(&mut sections, &VarInt(bed_state_for(version)))
            .unwrap();
        sections.write_i64_be(1).unwrap(); // block index zero is a bed
        for _ in 1..256 {
            sections.write_i64_be(0).unwrap();
        }
        sections.write_u8(0).unwrap(); // single biome palette
        VAR_INT.write(&mut sections, &VarInt(0)).unwrap();

        let mut payload = Vec::new();
        payload.write_i32_be(3).unwrap();
        payload.write_i32_be(-4).unwrap();
        VAR_INT.write(&mut payload, &VarInt(0)).unwrap(); // empty heightmap list
        VAR_INT
            .write(
                &mut payload,
                &VarInt(i32::try_from(sections.len()).unwrap()),
            )
            .unwrap();
        payload.extend_from_slice(&sections);
        VAR_INT.write(&mut payload, &VarInt(0)).unwrap(); // no block entities
        payload.extend_from_slice(&[0, 0]); // light tail

        let mut connection = UserConnection::new(9, version);
        connection.entity_tracker.min_y = -64;
        let context = Ctx {
            step: Protocol26_2To26_1.step(),
            mappings: MappingData::get().step(V::V_26_2),
            layout: version,
        };
        let mut wrapper = PacketWrapper::new(&clientbound::play::LEVEL_CHUNK_WITH_LIGHT, &payload);
        chunk_bed_entities(&mut wrapper, &mut connection, &context).unwrap();

        let output = wrapper.finish_with_outputs().unwrap();
        assert!(output.extra.is_empty());
        let mut read = output.payload.as_slice();
        assert_eq!(read.get_i32_be().unwrap(), 3);
        assert_eq!(read.get_i32_be().unwrap(), -4);
        assert_eq!(read.get_var_int().unwrap().0, 0); // heightmaps
        let section_len = usize::try_from(read.get_var_int().unwrap().0).unwrap();
        read = read.get(section_len..).unwrap();
        assert_eq!(read.get_var_int().unwrap().0, 1); // synthesized bed
        assert_eq!(read.get_u8().unwrap(), 0x00); // local X/Z at the chunk origin
        assert_eq!(read.get_i16_be().unwrap(), -64);
        assert_eq!(
            VAR_INT.read(&mut read).unwrap().0,
            bed_block_entity_type_id(version).unwrap()
        );
        assert!(matches!(
            NbtT::for_version(version).read(&mut read).unwrap(),
            Some(NbtTag::Compound(_))
        ));
        assert_eq!(read, [0, 0]); // light tail preserved
    }

    #[test]
    fn sulfur_cube_metadata_drops_the_two_new_fields() {
        use crate::api::entity_data::{EntityDataEntry, MetaValue};

        let version = V::V_26_1;
        let entries = vec![
            EntityDataEntry {
                index: 17,
                serializer: 0,
                value: MetaValue::Raw(vec![1]),
            },
            EntityDataEntry {
                index: 18,
                serializer: 0,
                value: MetaValue::Raw(vec![1]),
            },
            EntityDataEntry {
                index: 8,
                serializer: 0,
                value: MetaValue::Raw(vec![1]),
            },
        ];
        let list = EntityDataListT::for_version(version);
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(5)).unwrap();
        list.write(&mut payload, &entries).unwrap();

        let ids = MappingData::get().step(V::V_26_2);
        let mut wrapper = PacketWrapper::new(&clientbound::play::SET_ENTITY_DATA, &payload);
        let mut connection = UserConnection::new(0, version);
        connection
            .entity_tracker
            .add_mapped(5, EntityType::SULFUR_CUBE.id, EntityType::SLIME.id);
        let context = Ctx {
            step: Protocol26_2To26_1.step(),
            mappings: ids,
            layout: V::V_26_3,
        };
        sulfur_cube_metadata(&mut wrapper, &mut connection, &context).unwrap();
        let out = wrapper.finish().unwrap().unwrap();
        let mut read = out.payload.as_slice();
        assert_eq!(VAR_INT.read(&mut read).unwrap(), VarInt(5));
        let output = EntityDataListT::for_version(version)
            .read(&mut read)
            .unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].index, 8);
        assert!(read.is_empty());
    }
}
