//! ViaBackwards' 1.15 -> 1.14.4 compatibility step.
//!
//! Mapping tables and the pre-1.16.2 metadata serializer/index data are
//! supplied by the integration branch. This module keeps the packet-specific
//! rewrites local to the protocol boundary.

use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::CSpawnEntity;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_util::version::JavaMinecraftVersion as V;

use crate::api::MappingData;
use crate::api::connection::UserConnection;
use crate::api::rewriter::item::StructuredItemRewriter;
use crate::api::types::{
    BOOL, F32T, F64T, I16T, I32T, I64T, ItemT, STRING, U8, U8T, UUID, VAR_INT, WireType,
};
use crate::api::{Ctx, PacketWrapper, Protocol, Registry, Step, TranslateError};
use crate::packet::mappings::{clientbound, serverbound};

const MAX_CHUNK_SECTIONS: usize = 16;
const BLOCKS_PER_SECTION: usize = 4096;
const MAX_INDIRECT_BITS: u8 = 8;
const GLOBAL_PALETTE_BITS: u8 = 14;
const MAX_ARRAY_ENTRIES: i32 = 65_536;
const METADATA_END: u8 = 0xff;
const META_VAR_INT: i32 = 1;
const META_BOOLEAN: i32 = 7;

#[derive(Default)]
struct ImmediateRespawnStorage {
    enabled: bool,
}

pub struct Protocol1_15To1_14_4;

impl Protocol for Protocol1_15To1_14_4 {
    fn step(&self) -> Step {
        Step {
            from: V::V_1_15,
            to: V::V_1_14_4,
        }
    }

    fn init(&self, connection: &mut UserConnection) {
        connection.put(ImmediateRespawnStorage::default());
    }

    fn register(&self, registry: &mut Registry) {
        registry.clientbound(&clientbound::play::EXPLODE, explosion);
        registry.clientbound_layout(&clientbound::play::LEVEL_CHUNK_WITH_LIGHT, chunk);
        registry.clientbound_layout(&clientbound::play::LOGIN, login);
        registry.clientbound_layout(&clientbound::play::RESPAWN, respawn);
        registry.clientbound(&clientbound::play::GAME_EVENT, game_event);
        registry.clientbound(&clientbound::play::SET_HEALTH, set_health);
        registry.clientbound(&clientbound::play::UPDATE_ATTRIBUTES, update_attributes);
        registry.clientbound(&clientbound::play::ADD_ENTITY, legacy_spawn);
        registry.clientbound_layout(&clientbound::play::SPAWN_LIVING_ENTITY, spawn_living);
        registry.clientbound_layout(&clientbound::play::SPAWN_PLAYER, spawn_player);
        registry.serverbound(&serverbound::play::EDIT_BOOK, edit_book);
    }
}

/// ViaBackwards sends the old sound effect separately because 1.14.4 does
/// not play the explosion sound from its explosion packet.
fn explosion(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let x = wrapper.passthrough(&F32T)?;
    let y = wrapper.passthrough(&F32T)?;
    let z = wrapper.passthrough(&F32T)?;

    let mut sound = Vec::with_capacity(21);
    VAR_INT.write(&mut sound, &VarInt(243))?; // entity.generic.explode
    VAR_INT.write(&mut sound, &VarInt(4))?; // Blocks category.
    I32T.write(&mut sound, &((x * 8.0) as i32))?;
    I32T.write(&mut sound, &((y * 8.0) as i32))?;
    I32T.write(&mut sound, &((z * 8.0) as i32))?;
    F32T.write(&mut sound, &4.0)?;
    F32T.write(&mut sound, &1.0)?;
    wrapper.send_extra(&clientbound::play::SOUND, sound);
    wrapper.passthrough_all();
    Ok(())
}

/// 1.15 sends a 64x16x16 quart biome volume. 1.14 stores a 16x16 surface
/// grid in the section-data blob; each 4x4 quart sample is expanded to 4x4
/// blocks, matching ViaBackwards' `ChunkType1_15` -> `ChunkType1_14` copy.
fn chunk(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let rewritten = rewrite_chunk_1_15_to_1_14_4(wrapper.remaining(), &ctx.mappings.blockstates)
        .ok_or(TranslateError::Unsupported("1.15 chunk"))?;
    wrapper.replace_remaining(rewritten);
    Ok(())
}

fn rewrite_chunk_1_15_to_1_14_4(
    payload: &[u8],
    blockstates: &crate::api::IdMapping,
) -> Option<Vec<u8>> {
    let mut cursor = payload;
    let chunk_x = cursor.get_i32_be().ok()?;
    let chunk_z = cursor.get_i32_be().ok()?;
    let full_chunk = cursor.get_bool().ok()?;
    let mask = cursor.get_var_int().ok()?.0;
    if mask < 0 || mask as usize >= (1 << MAX_CHUNK_SECTIONS) {
        return None;
    }

    let heightmaps = crate::api::rewriter::block::RawNbtT::for_version(V::V_1_15)
        .read(&mut cursor)
        .ok()?;
    let mut biomes = None;
    if full_chunk {
        let mut input = [0i32; 1024];
        for biome in &mut input {
            *biome = cursor.get_i32_be().ok()?;
        }
        biomes = Some(expand_1_15_biomes(&input));
    }

    let data_len = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
    if data_len > cursor.len() {
        return None;
    }
    let (sections, rest) = cursor.split_at(data_len);
    let mut section_cursor = sections;
    let mut section_output = Vec::with_capacity(sections.len());
    for section in 0..MAX_CHUNK_SECTIONS {
        if mask & (1 << section) == 0 {
            continue;
        }
        let block_count = section_cursor.get_i16_be().ok()?;
        section_output.write_i16_be(block_count).ok()?;
        rewrite_section_1_13(&mut section_cursor, &mut section_output, blockstates)?;
    }
    if !section_cursor.is_empty() {
        return None;
    }

    let mut out = Vec::with_capacity(payload.len().saturating_add(1024));
    out.write_i32_be(chunk_x).ok()?;
    out.write_i32_be(chunk_z).ok()?;
    out.write_bool(full_chunk).ok()?;
    out.write_var_int(&VarInt(mask)).ok()?;
    out.extend_from_slice(&heightmaps);

    let biome_bytes = biomes.as_ref().map_or(0, |_| 256 * 4);
    let blob_len = section_output.len().checked_add(biome_bytes)?;
    out.write_var_int(&VarInt(i32::try_from(blob_len).ok()?))
        .ok()?;
    out.extend_from_slice(&section_output);
    if let Some(biomes) = biomes {
        for biome in biomes {
            out.write_i32_be(biome).ok()?;
        }
    }
    out.extend_from_slice(rest);
    Some(out)
}

fn expand_1_15_biomes(input: &[i32; 1024]) -> [i32; 256] {
    let mut output = [0i32; 256];
    for quart_z in 0..4 {
        for quart_x in 0..4 {
            let biome = input[quart_z * 4 + quart_x];
            for block_z in 0..4 {
                for block_x in 0..4 {
                    let x = quart_x * 4 + block_x;
                    let z = quart_z * 4 + block_z;
                    output[z * 16 + x] = biome;
                }
            }
        }
    }
    output
}

fn rewrite_section_1_13(
    cursor: &mut &[u8],
    output: &mut Vec<u8>,
    blockstates: &crate::api::IdMapping,
) -> Option<()> {
    let wire_bits = cursor.get_u8().ok()?;
    let bits = if wire_bits > MAX_INDIRECT_BITS {
        GLOBAL_PALETTE_BITS
    } else {
        wire_bits.max(4)
    };
    output.write_u8(bits).ok()?;
    if bits <= MAX_INDIRECT_BITS {
        let palette_len = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
        if palette_len == 0 || palette_len > BLOCKS_PER_SECTION {
            return None;
        }
        output
            .write_var_int(&VarInt(i32::try_from(palette_len).ok()?))
            .ok()?;
        for _ in 0..palette_len {
            let state = u32::try_from(cursor.get_var_int().ok()?.0).ok()?;
            let mapped = blockstates.map(state).unwrap_or(0);
            output
                .write_var_int(&VarInt(i32::try_from(mapped).ok()?))
                .ok()?;
        }
        copy_long_array(cursor, output)?;
        return Some(());
    }

    if bits > 32 {
        return None;
    }
    let long_count = usize::try_from(cursor.get_var_int().ok()?.0).ok()?;
    let mut longs = Vec::with_capacity(long_count.min(4096));
    for _ in 0..long_count {
        longs.push(cursor.get_i64_be().ok()? as u64);
    }
    let values = unpack_contiguous(&longs, bits)?;
    let mut mapped = Vec::with_capacity(values.len());
    let mask = (1u64 << bits) - 1;
    for state in values {
        let id = blockstates.map(state).unwrap_or(0);
        let id = u64::from(id);
        if id > mask {
            return None;
        }
        mapped.push(id);
    }
    let packed = pack_contiguous(&mapped, bits);
    output
        .write_var_int(&VarInt(i32::try_from(packed.len()).ok()?))
        .ok()?;
    for long in packed {
        output.write_i64_be(long as i64).ok()?;
    }
    Some(())
}

fn copy_long_array(cursor: &mut &[u8], output: &mut Vec<u8>) -> Option<()> {
    let len = cursor.get_var_int().ok()?.0;
    if !(0..=MAX_ARRAY_ENTRIES).contains(&len) {
        return None;
    }
    output.write_var_int(&VarInt(len)).ok()?;
    for _ in 0..len {
        output.write_i64_be(cursor.get_i64_be().ok()?).ok()?;
    }
    Some(())
}

fn unpack_contiguous(longs: &[u64], bits: u8) -> Option<Vec<u32>> {
    let value_count = BLOCKS_PER_SECTION;
    let mask = (1u64 << bits) - 1;
    let mut values = Vec::with_capacity(value_count);
    for index in 0..value_count {
        let start = index.checked_mul(usize::from(bits))?;
        let word = start / 64;
        let shift = start % 64;
        let mut value = *longs.get(word)? >> shift;
        if shift + usize::from(bits) > 64 {
            value |= *longs.get(word + 1)? << (64 - shift);
        }
        values.push(u32::try_from(value & mask).ok()?);
    }
    Some(values)
}

fn pack_contiguous(values: &[u64], bits: u8) -> Vec<u64> {
    let mut longs = vec![0u64; (values.len() * usize::from(bits)).div_ceil(64)];
    let mask = (1u64 << bits) - 1;
    for (index, value) in values.iter().enumerate() {
        let start = index * usize::from(bits);
        let word = start / 64;
        let shift = start % 64;
        longs[word] |= (value & mask) << shift;
        if shift + usize::from(bits) > 64 {
            longs[word + 1] |= (value & mask) >> (64 - shift);
        }
    }
    longs
}

fn login(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&I32T)?; // Entity id.
    wrapper.passthrough(&U8T)?; // Game mode.
    wrapper.passthrough(&I32T)?; // Dimension.
    wrapper.read(&I64T)?; // 1.15 seed, absent in 1.14.4.
    wrapper.passthrough(&U8T)?; // Max players.
    wrapper.passthrough(&STRING)?; // Level type.
    wrapper.passthrough(&VAR_INT)?; // View distance.
    wrapper.passthrough(&BOOL)?; // Reduced debug info.
    let respawn_screen_enabled = wrapper.read(&BOOL)?;
    connection.put(ImmediateRespawnStorage {
        enabled: !respawn_screen_enabled,
    });
    Ok(())
}

fn respawn(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough(&I32T)?; // Dimension.
    wrapper.read(&I64T)?; // 1.15 seed, absent in 1.14.4.
    wrapper.passthrough_all();
    Ok(())
}

fn game_event(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let event = wrapper.passthrough(&U8)?;
    let value = wrapper.passthrough(&F32T)?;
    if event == 11 {
        connection.put(ImmediateRespawnStorage {
            enabled: value == 1.0,
        });
    }
    Ok(())
}

fn set_health(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let health = wrapper.passthrough(&F32T)?;
    wrapper.passthrough(&VAR_INT)?; // Food.
    wrapper.passthrough(&F32T)?; // Saturation.
    if health <= 0.0
        && connection
            .get::<ImmediateRespawnStorage>()
            .is_some_and(|storage| storage.enabled)
    {
        let mut payload = Vec::new();
        VAR_INT.write(&mut payload, &VarInt(0))?;
        wrapper.send_serverbound(&serverbound::play::CLIENT_COMMAND, payload);
    }
    Ok(())
}

/// Split modern `ADD_ENTITY` into the old player/living/object spawn packets.
/// The preceding global id pass may leave either the native payload or a
/// partially remapped 1.14 payload, so decode exact candidates and use the
/// tracked 26.3 entity type as the source of truth.
fn legacy_spawn(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let mut source = wrapper.remaining();
    let (spawn, spawn_layout) = read_spawn_exact(&mut source, V::V_26_3)
        .map(|spawn| (spawn, V::V_26_3))
        .or_else(|| read_spawn_exact(&mut source, ctx.step.to).map(|spawn| (spawn, ctx.step.to)))
        .ok_or(TranslateError::Unsupported("legacy entity spawn"))?;
    let entity_id = spawn.entity_id.0;
    let source_type = connection
        .entity_tracker
        .entity_type(entity_id)
        .and_then(pumpkin_data::entity::EntityType::from_raw)
        .ok_or(TranslateError::Unsupported("tracked entity type"))?;
    let target_entities = &MappingData::get().composed(ctx.step.to).entities;
    let bee = source_type.id == pumpkin_data::entity::EntityType::BEE.id;
    let target_type = if bee {
        connection.entity_tracker.add_mapped(
            entity_id,
            source_type.id,
            pumpkin_data::entity::EntityType::PUFFERFISH.id,
        );
        u32::from(pumpkin_data::entity::EntityType::PUFFERFISH.id)
    } else {
        u32::from(source_type.id)
    };
    let target_type = target_entities
        .map(target_type)
        .and_then(|id| i32::try_from(id).ok())
        .ok_or(TranslateError::Unsupported("legacy entity type mapping"))?;

    let mut output = Vec::with_capacity(wrapper.remaining().len() + 8);
    VAR_INT.write(&mut output, &spawn.entity_id)?;
    if source_type.id == pumpkin_data::entity::EntityType::PLAYER.id {
        UUID.write(&mut output, &spawn.entity_uuid)?;
        for coordinate in [spawn.position.x, spawn.position.y, spawn.position.z] {
            F64T.write(&mut output, &coordinate)?;
        }
        U8.write(&mut output, &spawn.yaw)?;
        U8.write(&mut output, &spawn.pitch)?;
        output.push(METADATA_END);
        wrapper.replace_remaining(output);
        wrapper.set_packet(&clientbound::play::SPAWN_PLAYER);
        return Ok(());
    }

    if source_type.mob {
        UUID.write(&mut output, &spawn.entity_uuid)?;
        VAR_INT.write(&mut output, &VarInt(target_type))?;
        for coordinate in [spawn.position.x, spawn.position.y, spawn.position.z] {
            F64T.write(&mut output, &coordinate)?;
        }
        // The older living-mob packet puts yaw before pitch.
        U8.write(&mut output, &spawn.yaw)?;
        U8.write(&mut output, &spawn.pitch)?;
        U8.write(&mut output, &spawn.head_yaw)?;
        spawn.velocity.write_legacy(&mut output)?;
        if bee {
            output.extend_from_slice(&bee_stand_in_metadata()?);
        } else {
            U8.write(&mut output, &METADATA_END)?;
        }
        wrapper.replace_remaining(output);
        wrapper.set_packet(&clientbound::play::SPAWN_LIVING_ENTITY);
        return Ok(());
    }

    UUID.write(&mut output, &spawn.entity_uuid)?;
    VAR_INT.write(&mut output, &VarInt(target_type))?;
    for coordinate in [spawn.position.x, spawn.position.y, spawn.position.z] {
        F64T.write(&mut output, &coordinate)?;
    }
    U8.write(&mut output, &spawn.pitch)?;
    U8.write(&mut output, &spawn.yaw)?;
    let data = if source_type.id == pumpkin_data::entity::EntityType::FALLING_BLOCK.id
        && spawn_layout == V::V_26_3
    {
        u16::try_from(spawn.data.0)
            .ok()
            .map(|state| {
                i32::from(
                    crate::remap::block_state_remap::remap_block_state_for_version(
                        state,
                        ctx.step.to,
                    ),
                )
            })
            .unwrap_or(0)
    } else {
        spawn.data.0
    };
    I32T.write(&mut output, &data)?;
    spawn.velocity.write_legacy(&mut output)?;
    wrapper.replace_remaining(output);
    wrapper.set_packet(&clientbound::play::ADD_ENTITY);
    Ok(())
}

fn read_spawn_exact(
    input: &mut &[u8],
    version: V,
) -> Option<pumpkin_protocol::java::client::play::CSpawnEntity> {
    let original = *input;
    let mut cursor = original;
    let spawn = if version >= V::V_26_3 {
        read_spawn_26_3(&mut cursor)?
    } else {
        CSpawnEntity::read_packet_data(&mut cursor, &version).ok()?
    };
    if !cursor.is_empty() {
        return None;
    }
    *input = &[];
    Some(spawn)
}

fn read_spawn_26_3(
    cursor: &mut &[u8],
) -> Option<pumpkin_protocol::java::client::play::CSpawnEntity> {
    use pumpkin_util::math::vector3::Vector3;

    let entity_id = VAR_INT.read(cursor).ok()?;
    let entity_uuid = UUID.read(cursor).ok()?;
    let r#type = VAR_INT.read(cursor).ok()?;
    let position = Vector3::new(
        cursor.get_f64_be().ok()?,
        cursor.get_f64_be().ok()?,
        cursor.get_f64_be().ok()?,
    );
    let velocity = read_lp_vector_3d(cursor)?;
    let pitch = cursor.get_u8().ok()?;
    let yaw = cursor.get_u8().ok()?;
    let head_yaw = cursor.get_u8().ok()?;
    let data = VAR_INT.read(cursor).ok()?;
    Some(CSpawnEntity::new_packed(
        entity_id,
        entity_uuid,
        r#type,
        position,
        pitch,
        yaw,
        head_yaw,
        data,
        velocity,
    ))
}

/// The pinned Pumpkin encoder uses a one-byte zero sentinel for near-zero
/// velocity; decode that case without consuming the following rotation byte.
fn read_lp_vector_3d(cursor: &mut &[u8]) -> Option<pumpkin_util::math::vector3::Vector3<f64>> {
    use pumpkin_util::math::vector3::Vector3;

    let (&first, rest) = cursor.split_first()?;
    if first == 0 {
        *cursor = rest;
        return Some(Vector3::new(0.0, 0.0, 0.0));
    }
    *cursor = rest;
    let second = cursor.get_u8().ok()?;
    let low_bytes = [first, second];
    let low = u16::from_le_bytes(low_bytes) as i64;
    let middle = cursor.get_i32_be().ok()? as i64;
    let packed = low | (middle << 16);
    let header = packed & 0x07;
    let extended = header & 4 != 0;
    let scale = if extended {
        (i64::from(VAR_INT.read(cursor).ok()?.0) << 2) | (header & 3)
    } else {
        header & 3
    };
    if scale == 0 {
        return Some(Vector3::new(0.0, 0.0, 0.0));
    }
    let decode = |shift| {
        let quantized = ((packed >> shift) & 0x7fff) as f64;
        ((quantized / 32766.0) - 0.5) * 2.0 * scale as f64
    };
    Some(Vector3::new(decode(3), decode(18), decode(33)))
}

fn bee_stand_in_metadata() -> Result<Vec<u8>, TranslateError> {
    // 1.14.4 metadata serializers: boolean=7, VarInt=1.
    let mut output = Vec::with_capacity(8);
    U8.write(&mut output, &14)?;
    VAR_INT.write(&mut output, &VarInt(META_BOOLEAN))?;
    BOOL.write(&mut output, &false)?;
    U8.write(&mut output, &15)?;
    VAR_INT.write(&mut output, &VarInt(META_VAR_INT))?;
    VAR_INT.write(&mut output, &VarInt(2))?;
    U8.write(&mut output, &METADATA_END)?;
    Ok(output)
}

fn spawn_living(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let entity_id = wrapper.passthrough(&VAR_INT)?.0;
    wrapper.passthrough(&UUID)?;
    let type_id = wrapper.read(&VAR_INT)?.0;
    let source_type = connection.entity_tracker.entity_type(entity_id);
    let is_bee = source_type == Some(pumpkin_data::entity::EntityType::BEE.id);
    let mapped_type = if is_bee {
        connection.entity_tracker.add_mapped(
            entity_id,
            pumpkin_data::entity::EntityType::BEE.id,
            pumpkin_data::entity::EntityType::PUFFERFISH.id,
        );
        crate::api::MappingData::get()
            .composed(ctx.step.to)
            .entities
            .map(u32::from(pumpkin_data::entity::EntityType::PUFFERFISH.id))
            .and_then(|id| i32::try_from(id).ok())
            .ok_or(TranslateError::Unsupported("bee stand-in type"))?
    } else {
        u32::try_from(type_id)
            .ok()
            .and_then(|id| ctx.mappings.entities.map(id))
            .and_then(|id| i32::try_from(id).ok())
            .ok_or(TranslateError::Unsupported("living entity type"))?
    };
    wrapper.write(&VAR_INT, &VarInt(mapped_type))?;
    for _ in 0..3 {
        wrapper.passthrough(&F64T)?;
    }
    for _ in 0..3 {
        wrapper.passthrough(&U8)?;
    }
    for _ in 0..3 {
        wrapper.passthrough(&I16T)?;
    }

    if is_bee {
        wrapper.write_bytes(&bee_stand_in_metadata()?);
        return Ok(());
    }
    wrapper.write(&U8, &METADATA_END)?;
    wrapper.set_packet(&clientbound::play::SPAWN_LIVING_ENTITY);
    Ok(())
}

fn spawn_player(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    wrapper.passthrough_all();
    wrapper.write(&U8, &METADATA_END)?;
    Ok(())
}

fn update_attributes(
    wrapper: &mut PacketWrapper,
    connection: &mut UserConnection,
    _ctx: &Ctx,
) -> Result<(), TranslateError> {
    let entity_id = wrapper.passthrough(&VAR_INT)?.0;
    let count = wrapper.read(&I32T)?;
    if !(0..=4096).contains(&count) {
        return Err(TranslateError::Unsupported("attribute count"));
    }
    let is_bee = connection.entity_tracker.entity_type(entity_id)
        == Some(pumpkin_data::entity::EntityType::BEE.id);
    let mut encoded = Vec::new();
    let mut kept = 0i32;
    for _ in 0..count {
        let name = wrapper.read(&STRING)?;
        let value = wrapper.read(&F64T)?;
        let modifier_count = wrapper.read(&VAR_INT)?.0;
        if !(0..=1024).contains(&modifier_count) {
            return Err(TranslateError::Unsupported("attribute modifier count"));
        }
        let mut modifiers = Vec::new();
        for _ in 0..modifier_count {
            let uuid = wrapper.read(&UUID)?;
            let amount = wrapper.read(&F64T)?;
            let operation = wrapper.read(&U8)?;
            UUID.write(&mut modifiers, &uuid)?;
            F64T.write(&mut modifiers, &amount)?;
            U8.write(&mut modifiers, &operation)?;
        }
        if is_bee && name.as_ref() == "generic.flyingSpeed" {
            continue;
        }
        STRING.write(&mut encoded, &name)?;
        F64T.write(&mut encoded, &value)?;
        VAR_INT.write(&mut encoded, &VarInt(modifier_count))?;
        encoded.extend_from_slice(&modifiers);
        kept += 1;
    }
    let mut out = Vec::with_capacity(encoded.len() + 4);
    I32T.write(&mut out, &kept)?;
    out.extend_from_slice(&encoded);
    wrapper.replace_remaining(out);
    Ok(())
}

fn edit_book(
    wrapper: &mut PacketWrapper,
    _connection: &mut UserConnection,
    ctx: &Ctx,
) -> Result<(), TranslateError> {
    let ids = MappingData::get().composed(ctx.layout);
    let client_item = wrapper.read(&ItemT::for_version(ctx.layout))?;
    let native_item = StructuredItemRewriter::to_native(&client_item, ctx.layout, ids);
    wrapper.write(&ItemT::for_version(V::V_26_3), &native_item)?;
    wrapper.passthrough_all();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bee_biomes_expand_the_quart_grid_to_the_legacy_surface_grid() {
        let source = std::array::from_fn(|index| i32::try_from(index).unwrap());
        let output = expand_1_15_biomes(&source);
        for z in 0..16 {
            for x in 0..16 {
                let source_biome = (z / 4) * 4 + (x / 4);
                assert_eq!(output[z * 16 + x], source_biome as i32);
            }
        }
    }

    #[test]
    fn immediate_respawn_is_an_explicit_per_connection_state() {
        let mut connection = UserConnection::new(0, V::V_1_14_4);
        assert!(connection.get::<ImmediateRespawnStorage>().is_none());
        connection.put(ImmediateRespawnStorage { enabled: true });
        assert!(connection.get::<ImmediateRespawnStorage>().unwrap().enabled);
    }

    #[test]
    fn section_direct_storage_unpack_round_trips_cross_long_entries() {
        let values: Vec<u64> = (0..BLOCKS_PER_SECTION)
            .map(|index| u64::try_from(index % 511).unwrap())
            .collect();
        let packed = pack_contiguous(&values, 9);
        assert_eq!(
            unpack_contiguous(&packed, 9).unwrap(),
            values.iter().map(|v| *v as u32).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_full_1_15_chunk_moves_expanded_biomes_into_the_1_14_data_blob() {
        let mut input = Vec::new();
        input.write_i32_be(3).unwrap();
        input.write_i32_be(-2).unwrap();
        input.write_bool(true).unwrap();
        input.write_var_int(&VarInt(0)).unwrap();
        input.write_u8(0).unwrap(); // Empty named heightmap NBT.
        for index in 0..1024 {
            input.write_i32_be(index % 16).unwrap();
        }
        input.write_var_int(&VarInt(0)).unwrap(); // No section data.
        input.write_var_int(&VarInt(0)).unwrap(); // No block entities.

        let output =
            rewrite_chunk_1_15_to_1_14_4(&input, &MappingData::get().step(V::V_1_15).blockstates)
                .expect("1.15 chunk with no sections");
        let mut read = output.as_slice();
        assert_eq!(read.get_i32_be().unwrap(), 3);
        assert_eq!(read.get_i32_be().unwrap(), -2);
        assert!(read.get_bool().unwrap());
        assert_eq!(read.get_var_int().unwrap().0, 0);
        assert_eq!(read.get_u8().unwrap(), 0); // Empty heightmap.
        assert_eq!(read.get_var_int().unwrap().0, 1024);
        for z in 0..16 {
            for x in 0..16 {
                assert_eq!(read.get_i32_be().unwrap(), ((z / 4) * 4 + (x / 4)) as i32);
            }
        }
        assert_eq!(read.get_var_int().unwrap().0, 0); // Empty block entities.
        assert!(read.is_empty());
    }
}
